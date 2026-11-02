//! One pass of the advisor: the scout proposes, the reviewer checks.
//! Port of internal/advisor/run.go. A model's call goes through Ask, so a
//! pass can be tried without spending; Cli is the real one, the agent's
//! own program run headless through its adapter's Querier.

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rush_core::adapter::{Profile, Query, Registry, global};
use rush_state::Time;
use serde::{Deserialize, Serialize};

use super::{CANDIDATE, CONFIRMED, Finding, Input, REJECTED, REVIEW_AT, TRIES, digest, dir, id_of};
use crate::catalog::find;
use crate::source::transcripts_dir;

/// The scout runs on the agent's quick model, the review on its careful
/// one: what each may spend, and how long it may take.
const SCOUT_BUDGET: f64 = 0.30;
const SCOUT_TIME: Duration = Duration::from_secs(5 * 60);
pub(crate) const REVIEW_BUDGET: f64 = 1.50;
const REVIEW_TIME: Duration = Duration::from_secs(10 * 60);

/// What the advisor's calls are called where rush shows what waits on the
/// network.
pub const JOB: &str = "Advisor";

/// What one pass found and what it cost.
#[derive(Clone, Debug, Default)]
pub struct Result {
    pub at: Time,
    pub findings: Vec<Finding>,
    /// When each Opus review ran.
    pub reviewed: Vec<Time>,
    pub spent: f64,
    pub err: Option<String>,
}

/// One call of a model: headless, read-only, its answer in schema's shape.
#[derive(Clone, Debug)]
pub struct Call {
    pub model: String,
    pub budget: f64,
    pub timeout: Duration,
    pub dirs: Vec<String>,
    pub system: &'static str,
    pub prompt: String,
    pub schema: String,
}

/// How a pass asks a model.
pub trait Ask: Sync {
    /// The agent's quick model and its careful one.
    fn models(&self, p: &Profile) -> (String, String);
    /// Runs c as p, and says what it cost (even when it failed) and its
    /// structured answer.
    fn ask(&self, p: &Profile, c: &Call) -> (f64, std::result::Result<Vec<u8>, String>);
}

/// Runs the advisor once, as acct: the scout proposes, then the reviewer
/// checks the candidates worth money, new and left from earlier passes,
/// while the day's reviews last. reviews is how many the cap still
/// allows.
pub fn pass(acct: &Profile, input: Input, reviews: i64) -> Result {
    pass_with(&Cli::new(global().clone()), acct, input, reviews)
}

/// pass, asking through ask.
pub fn pass_with(ask: &dyn Ask, acct: &Profile, mut input: Input, mut reviews: i64) -> Result {
    let mut res = Result {
        at: Time::now(),
        ..Result::default()
    };
    let briefs_dir = dir().join("briefs");
    if input.briefs.is_none() {
        let paths: Vec<String> = input.top.iter().map(|s| s.path.clone()).collect();
        input.briefs = Some(super::briefs(&briefs_dir, &paths));
    }
    let digest = digest(&input);
    let (scout_model, review_model) = ask.models(acct);
    // The scout reads the briefs alone; the reviewer may check them
    // against the transcripts.
    let b = briefs_dir.to_string_lossy().into_owned();
    let scout_dirs = vec![b.clone()];
    let review_dirs = vec![b, transcripts_dir(acct)];

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Scout {
        findings: Vec<Proposal>,
    }
    let (cost, out) = ask.ask(
        acct,
        &Call {
            model: scout_model,
            budget: SCOUT_BUDGET,
            timeout: SCOUT_TIME,
            dirs: scout_dirs,
            system: SCOUT_SYSTEM,
            prompt: digest.clone(),
            schema: scout_schema(),
        },
    );
    res.spent += cost;
    let scout =
        match out.and_then(|b| serde_json::from_slice::<Scout>(&b).map_err(|e| e.to_string())) {
            Ok(s) => s,
            Err(e) => {
                res.err = Some(format!("haiku: {e}"));
                return res;
            }
        };

    let mut cands: Vec<Finding> = scout
        .findings
        .iter()
        .map(|p| p.finding(CANDIDATE, res.at))
        .filter(|f| !f.title.is_empty() && !input.settled.contains(&f.id))
        .collect();
    for f in &input.pending {
        match cands.iter_mut().find(|c| c.id == f.id) {
            Some(c) => c.tries = f.tries, // proposed again: its failed reviews still count
            None => cands.push(f.clone()),
        }
    }
    let mut roots = vec![acct.dir.clone()];
    roots.extend(input.top.iter().map(|s| s.project.clone()));
    // The most at stake is checked first.
    cands.sort_by(|a, b| b.weekly.total_cmp(&a.weekly));
    for c in cands.iter_mut() {
        c.open = check_open(&c.open, &roots);
        if reviews == 0 || c.weekly < REVIEW_AT || c.tries >= TRIES {
            continue;
        }
        if let Some(r) = input.reserve.as_mut()
            && let Err(e) = r()
        {
            res.err = Some(e);
            break;
        }
        reviews -= 1;
        res.reviewed.push(Time::now());
        let (cost, out) = ask.ask(
            acct,
            &Call {
                model: review_model.clone(),
                budget: REVIEW_BUDGET,
                timeout: REVIEW_TIME,
                dirs: review_dirs.clone(),
                system: REVIEW_SYSTEM,
                prompt: review_prompt(&digest, c),
                schema: review_schema(),
            },
        );
        res.spent += cost;
        let v = match out
            .and_then(|b| serde_json::from_slice::<Verdict>(&b).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(e) => {
                res.err = Some(format!("opus: {e}"));
                c.tries += 1; // it stays a candidate, until it has failed too often
                continue;
            }
        };
        if !v.confirmed {
            c.status = REJECTED.into();
            c.note = v.note;
            continue;
        }
        // Kept under the candidate's ID, so the scout proposing it again
        // finds it already checked.
        let id = std::mem::take(&mut c.id);
        let open = check_open(&v.proposal.open, &roots);
        *c = v.proposal.finding(CONFIRMED, res.at);
        c.id = id;
        c.note = v.note;
        c.open = open;
    }
    res.findings = cands;
    res
}

/// A file a finding may send you to: one that exists, under the account's
/// folder or a folder its sessions worked in. A model wrote it, so
/// anything else is dropped.
pub(crate) fn check_open(p: &str, roots: &[String]) -> String {
    let path = Path::new(p);
    if p.is_empty() || !path.is_absolute() {
        return String::new();
    }
    let clean = clean(path);
    if !std::fs::metadata(&clean).is_ok_and(|m| m.is_file()) {
        return String::new();
    }
    for r in roots.iter().filter(|r| !r.is_empty()) {
        if clean.starts_with(clean_dir(r)) && clean != clean_dir(r) {
            return clean.to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn clean_dir(r: &str) -> PathBuf {
    clean(Path::new(r))
}

/// p with . and .. taken out, as Go's filepath.Clean.
fn clean(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// A finding as a model writes it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Proposal {
    title: String,
    detail: String,
    evidence: Vec<String>,
    #[serde(rename = "weeklyCost")]
    weekly: f64,
    fix: String,
    open: String,
}

impl Proposal {
    fn finding(&self, status: &str, at: Time) -> Finding {
        Finding {
            id: id_of(&self.title),
            title: self.title.trim().to_string(),
            detail: self.detail.trim().to_string(),
            evidence: self.evidence.clone(),
            weekly: self.weekly.max(0.0),
            open: self.open.clone(),
            fix: if find(&self.fix).is_some() {
                self.fix.clone()
            } else {
                String::new()
            },
            status: status.into(),
            at,
            ..Finding::default()
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Verdict {
    confirmed: bool,
    note: String,
    #[serde(flatten)]
    proposal: Proposal,
}

fn review_prompt(digest: &str, c: &Finding) -> String {
    let p = Proposal {
        title: c.title.clone(),
        detail: c.detail.clone(),
        evidence: c.evidence.clone(),
        weekly: c.weekly,
        fix: c.fix.clone(),
        open: c.open.clone(),
    };
    let b = serde_json::to_string_pretty(&p).unwrap_or_default();
    format!("## Candidate\n{b}\n\n{digest}")
}

/// The real Ask: the agent's own program, headless, through its adapter's
/// Querier, waiting while the network is down.
pub struct Cli {
    reg: Registry,
}

impl Cli {
    pub fn new(reg: Registry) -> Cli {
        Cli { reg }
    }
}

impl Ask for Cli {
    fn models(&self, p: &Profile) -> (String, String) {
        self.reg
            .get(p.kind.as_str())
            .and_then(|a| a.querier().map(|q| q.query_models()))
            .unwrap_or_default()
    }

    fn ask(&self, p: &Profile, c: &Call) -> (f64, std::result::Result<Vec<u8>, String>) {
        if !rush_net::netwatch::run(JOB) {
            return (0.0, Err(rush_net::netwatch::Offline.to_string()));
        }
        let (cost, res) = self.run(p, c);
        match &res {
            Ok(_) => rush_net::netwatch::done(JOB, None),
            Err(e) => {
                let e = std::io::Error::other(e.clone());
                rush_net::netwatch::done(JOB, Some(&e));
            }
        }
        (cost, res)
    }
}

/// The passes' processes, by process group, for stop.
static RUNNING: Mutex<Option<HashSet<i32>>> = Mutex::new(None);

fn running(f: impl FnOnce(&mut HashSet<i32>)) {
    let mut r = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    f(r.get_or_insert_with(HashSet::new));
}

/// Ends any pass still running, as rush quits: a program left behind
/// would spend on an answer nobody reads.
pub fn stop() {
    running(|r| {
        for &pid in r.iter() {
            // SAFETY: kill only sends a signal.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    });
}

impl Cli {
    fn run(&self, p: &Profile, c: &Call) -> (f64, std::result::Result<Vec<u8>, String>) {
        if let Err(e) = std::fs::create_dir_all(dir()) {
            return (0.0, Err(e.to_string()));
        }
        let Some(q) = self
            .reg
            .get(p.kind.as_str())
            .and_then(|a| a.querier().is_some().then_some(a))
        else {
            return (0.0, Err(format!("the advisor can't run on {}", p.kind)));
        };
        let q = q.querier().expect("checked");
        let (args, mut env) = q.query_command(
            p,
            &Query {
                model: c.model.clone(),
                system: c.system.into(),
                prompt: c.prompt.clone(),
                schema: c.schema.clone(),
                dirs: c.dirs.clone(),
                budget: c.budget,
            },
        );
        // The program is where the profile's agent was found installed.
        let prog = self.reg.path(p.kind.as_str());
        if prog.is_empty() {
            return (
                0.0,
                Err(format!(
                    "the advisor runs {}, which isn't installed",
                    p.kind
                )),
            );
        }
        env.push("RUSH_ADVISOR=1".into());
        let mut cmd = Command::new(&prog);
        cmd.args(&args)
            .current_dir(dir())
            .env_clear()
            .envs(env.iter().filter_map(|e| e.split_once('=')))
            // Its own process group, so stopping it takes whatever it
            // started too.
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(ch) => ch,
            Err(e) => return (0.0, Err(e.to_string())),
        };
        let pid = child.id() as i32;
        running(|r| {
            r.insert(pid);
        });
        let prompt = c.prompt.clone();
        let mut stdin = child.stdin.take();
        let feed = std::thread::spawn(move || {
            if let Some(s) = stdin.as_mut() {
                let _ = s.write_all(prompt.as_bytes());
            }
        });
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        let read_out = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(o) = out.as_mut() {
                let _ = o.read_to_end(&mut b);
            }
            b
        });
        let read_err = std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(o) = err.as_mut() {
                let _ = o.read_to_end(&mut b);
            }
            b
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Ok(s),
                Ok(None) if start.elapsed() > c.timeout => {
                    // SAFETY: kill only sends a signal.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break Err("timed out".to_string());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => break Err(e.to_string()),
            }
        };
        running(|r| {
            r.remove(&pid);
        });
        let _ = feed.join();
        let stdout = read_out.join().unwrap_or_default();
        let stderr = read_err.join().unwrap_or_default();

        let Some(ans) = q.query_answer(&stdout) else {
            // Stopped or broken halfway: what it spent isn't known, so
            // count what it was allowed to.
            let why = match status {
                Ok(s) if s.success() => "its answer couldn't be read".to_string(),
                Ok(s) => format!("{s}: {}", first_line(&String::from_utf8_lossy(&stderr))),
                Err(e) => format!("{e}: {}", first_line(&String::from_utf8_lossy(&stderr))),
            };
            return (c.budget, Err(why));
        };
        if let Some(e) = ans.err {
            return (ans.cost, Err(e.to_string()));
        }
        if ans.out.is_empty() || ans.out == b"null" {
            return (ans.cost, Err("no answer".into()));
        }
        (ans.cost, Ok(ans.out))
    }
}

fn first_line(s: &str) -> String {
    let s = s.trim().split('\n').next().unwrap_or("");
    if s.len() > 200 {
        let mut end = 200;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        return format!("{}…", &s[..end]);
    }
    s.to_string()
}

const SCOUT_SYSTEM: &str = "You are rush's advisor. rush is a terminal dashboard that runs the user's coding agents (Claude Code and others). You get a digest of the last 7 days of their agents' figures, which rush has already worked out.

Find up to 5 specific changes to how this user works with their agents that would cut tokens and cost, or make the agents faster. Look at habits and setup: CLAUDE.md and memory, settings, savers, model and effort choice, subagent use, sessions left to grow instead of starting fresh, files read again and again, noisy shell output, and prompts that make an agent explore more than it needs.

Rules:
- Every finding rests on evidence: figures from the digest, or lines in a session's brief. Put brief paths, and the lines you saw, in evidence.
- Each costly session has a brief: one short line for each prompt you gave (with the context size then), each tool call (with \"→ size\" of what it sent back) and each compaction. Briefs are small, so read the ones that matter. You can't read the transcripts themselves.
- Read only what a finding needs: two or three briefs at most. Each is about 16k tokens.
- Give no generic advice. If the figures don't show a problem, return fewer findings, or none.
- Don't repeat anything under \"Already said\".
- title: one short line, what to change. detail: one or two sentences, why, with the figure behind it.
- weeklyCost: the dollars a week this change would plausibly save, worked out from the digest's figures. That's the share it would cut, never the whole figure it touches. Be conservative, and use 0 if you can't tell.
- fix: a saver id from the digest's list when one addresses it, otherwise \"\". open: the absolute path of a file the user should edit, when that's the change, otherwise \"\".";

const REVIEW_SYSTEM: &str = "You check findings for rush's advisor. A cheaper model read a digest of the user's coding-agent figures and proposed the candidate below. Your job is to decide whether it's true and worth the user's time.

Check its evidence against the sessions' briefs (one short line per prompt, tool call and compaction) and the digest's figures. When a brief isn't enough, grep the raw JSONL transcript. Use Grep with output_mode \"count\" or \"files_with_matches\", or a head_limit, because one transcript line can be hundreds of kilobytes. Never read a transcript whole. Confirm it only if the evidence holds, the change would really help this user, and the cost estimate is sound (correct it if not). Reject anything generic, already handled by a saver that's on, or unsupported.

When you confirm it, rewrite title and detail to be exact and actionable, in plain words: title is one short line, and detail is one or two sentences with the figure behind it. Keep or correct fix and open. note: one line on what you checked, or why you rejected it.";

const FINDING_PROPS: &str = r#""title":{"type":"string"},"detail":{"type":"string"},"evidence":{"type":"array","items":{"type":"string"}},"weeklyCost":{"type":"number"},"fix":{"type":"string"},"open":{"type":"string"}"#;

fn scout_schema() -> String {
    format!(
        r#"{{"type":"object","properties":{{"findings":{{"type":"array","maxItems":5,"items":{{"type":"object","properties":{{{FINDING_PROPS}}},"required":["title","detail","evidence","weeklyCost","fix","open"]}}}}}},"required":["findings"]}}"#
    )
}

fn review_schema() -> String {
    format!(
        r#"{{"type":"object","properties":{{"confirmed":{{"type":"boolean"}},"note":{{"type":"string"}},{FINDING_PROPS}}},"required":["confirmed","note","title","detail","evidence","weeklyCost","fix","open"]}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{Query as Q, View};
    use crate::scan::Bucket;
    use std::sync::Mutex as M;

    /// Answers as the scout or the reviewer would, and keeps the calls.
    struct Fake {
        scout: (f64, std::result::Result<Vec<u8>, String>),
        review: (f64, std::result::Result<Vec<u8>, String>),
        calls: M<Vec<Call>>,
    }

    impl Fake {
        fn new(scout: &str, review: std::result::Result<&str, &str>) -> Fake {
            Fake {
                scout: (0.01, Ok(scout.as_bytes().to_vec())),
                review: match review {
                    Ok(r) => (0.5, Ok(r.as_bytes().to_vec())),
                    Err(e) => (REVIEW_BUDGET, Err(e.to_string())),
                },
                calls: M::new(Vec::new()),
            }
        }
    }

    impl Ask for Fake {
        fn models(&self, _: &Profile) -> (String, String) {
            ("haiku".into(), "opus".into())
        }
        fn ask(&self, _: &Profile, c: &Call) -> (f64, std::result::Result<Vec<u8>, String>) {
            self.calls.lock().unwrap().push(c.clone());
            if c.model == "haiku" {
                self.scout.clone()
            } else {
                self.review.clone()
            }
        }
    }

    fn acct(dir: &Path) -> Profile {
        Profile {
            kind: crate::AGENT.into(),
            name: String::new(),
            dir: dir.to_string_lossy().into_owned(),
        }
    }

    fn input() -> Input {
        let now = Time::now();
        Input {
            view: View {
                q: Q {
                    from: now,
                    to: now,
                    ..Q::default()
                },
                total: Bucket {
                    req: 1,
                    ..Bucket::default()
                },
                ..View::default()
            },
            ..Input::default()
        }
    }

    #[test]
    fn passes() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let fake = Fake::new(
            r#"{"findings":[{"title":"Big one","detail":"d","evidence":["/p/s.jsonl"],"weeklyCost":5,"fix":"rtk","open":""},{"title":"Small one","detail":"d","evidence":[],"weeklyCost":0.2,"fix":"nope","open":""},{"title":"Second big","detail":"d","evidence":[],"weeklyCost":3,"fix":"","open":""}]}"#,
            Ok(
                r#"{"confirmed":true,"note":"seen in s.jsonl","title":"Checked one","detail":"exact","evidence":[],"weeklyCost":4,"fix":"rtk","open":""}"#,
            ),
        );
        let mut inp = input();
        inp.pending = vec![Finding {
            id: "old".into(),
            title: "Left from before".into(),
            weekly: 2.0,
            status: CANDIDATE.into(),
            ..Finding::default()
        }];
        let a = tempfile::tempdir().unwrap();
        let res = pass_with(&fake, &acct(a.path()), inp, 1);
        assert!(res.err.is_none(), "{:?}", res.err);
        assert_eq!((res.findings.len(), res.reviewed.len()), (4, 1));
        assert!(res.findings[2].id == "old" && res.findings[2].status == CANDIDATE);
        let f = &res.findings[0];
        assert!(
            f.status == CONFIRMED
                && f.title == "Checked one"
                && f.id == id_of("Big one")
                && !f.note.is_empty(),
            "{f:?}"
        );
        assert!(res.findings[1].status == CANDIDATE && res.findings[3].fix.is_empty());
        assert!((res.spent - 0.51).abs() < 1e-9, "spent {}", res.spent);
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls[0].budget, 0.30);
        assert_eq!(calls[1].budget, 1.50);
        assert!(calls[1].prompt.starts_with("## Candidate\n"));
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn skips_settled() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let fake = Fake::new(
            r#"{"findings":[{"title":"Old news","detail":"","evidence":[],"weeklyCost":9,"fix":"","open":""}]}"#,
            Ok(
                r#"{"confirmed":true,"note":"","title":"x","detail":"","evidence":[],"weeklyCost":9,"fix":"","open":""}"#,
            ),
        );
        let mut inp = input();
        inp.settled = vec![id_of("old  NEWS")];
        let a = tempfile::tempdir().unwrap();
        let res = pass_with(&fake, &acct(a.path()), inp, 3);
        assert!(res.findings.is_empty() && res.reviewed.is_empty());
        assert_eq!(
            fake.calls.lock().unwrap().len(),
            1,
            "the reviewer was paid to look again"
        );
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn failed_reviews_count() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let fake = Fake::new(
            r#"{"findings":[{"title":"Flaky","detail":"","evidence":[],"weeklyCost":9,"fix":"","open":""}]}"#,
            Err("its answer couldn't be read"),
        );
        let reserved = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r2 = reserved.clone();
        let mut inp = input();
        inp.pending = vec![Finding {
            id: id_of("Flaky"),
            title: "Flaky".into(),
            weekly: 9.0,
            status: CANDIDATE.into(),
            tries: TRIES - 1,
            ..Finding::default()
        }];
        inp.reserve = Some(Box::new(move || {
            r2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }));
        let a = tempfile::tempdir().unwrap();
        let res = pass_with(&fake, &acct(a.path()), inp, 3);
        assert_eq!(reserved.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(res.reviewed.len(), 1);
        assert_eq!(res.findings[0].tries, TRIES);
        assert!(res.spent >= REVIEW_BUDGET);
        let mut r = super::super::Record::default();
        r.merge(res);
        assert!(
            r.pending().is_empty(),
            "a candidate that keeps failing is reviewed again"
        );
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn scout_failing_says_so() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let mut fake = Fake::new("", Ok("{}"));
        fake.scout = (0.15, Err("over budget".into()));
        let a = tempfile::tempdir().unwrap();
        let res = pass_with(&fake, &acct(a.path()), input(), 3);
        assert!(res.err.as_deref().is_some_and(|e| e.contains("budget")) && res.spent == 0.15);
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn opens_only_what_s_there() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let inside = r.join("CLAUDE.md");
        std::fs::write(&inside, "").unwrap();
        let other = tempfile::tempdir().unwrap();
        let out = other.path().join("secret");
        std::fs::write(&out, "").unwrap();
        let roots = vec![r.to_string_lossy().into_owned()];
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let up = r.join("..").join(r.file_name().unwrap()).join("CLAUDE.md");
        for (p, want) in [
            (s(&inside), s(&inside)),
            (s(&out), String::new()),
            ("CLAUDE.md".into(), String::new()),
            (s(&up), s(&inside)),
            (s(&r.join("missing.md")), String::new()),
        ] {
            assert_eq!(check_open(&p, &roots), want, "{p}");
        }
    }
}
