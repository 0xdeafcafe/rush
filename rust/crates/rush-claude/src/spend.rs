//! Prices sessions' transcripts, each read only as far as it grew since
//! the last run. Port of internal/adapters/claude/spend.go and
//! agent/spend.go:33 (SpendTarget).
//!
//! What was read of each transcript (its Totals) is kept between runs, as
//! Go's state.CostCache keeps it, but in a file of the Rust rush's own:
//! CACHE_FILE in rush's cache folder, never Go's costs.json, which Go
//! still reads and writes in a format of its own. It's read at the first
//! run, on the spend worker, and written after a run that read something
//! new, at most every SAVE_EVERY, and on flush (spend.go:83-190); written
//! beside, then renamed over, so a reader never sees half of one. A
//! transcript that shrank since is read again from its start (scan).
//!
//! As in Go (spend.go files: claude.SubagentTranscripts and
//! claude.WorkflowTranscripts, since 2c6ffb47), what a workflow's agents
//! spent counts too: Claude Code writes them under
//! subagents/workflows/<run>/, and they're the session's spend as much as
//! a subagent's is. Only their cost, tokens and today's share are taken:
//! not their folders, pull requests or last word, which stay the
//! session's and its subagents' (spend.go Run, `i >= len(files)-nFlows`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rush_core::agent::Spend;
use rush_core::time::Ms;
use rush_state::CostCache;

use crate::jobs::mtime;
use crate::json::day;
use crate::transcript::{Totals, scan, subagent_transcripts, subagents_dir};

/// Where Claude Code writes a workflow's agents, under the subagents
/// folder: one folder per run.
const WORKFLOWS: &str = "workflows";

/// The Rust rush's spend cache, in rush's cache folder.
pub const CACHE_FILE: &str = "costs-rs.json";
/// The spend cache's format: a file of another starts over. Bump it when
/// Totals changes what it keeps.
/// 1: Totals as transcript.rs has it.
pub const CACHE_VERSION: i64 = 1;
/// How often a run that read something saves the cache (spend.go:176).
pub const SAVE_EVERY: Ms = 30_000;

/// One session's transcript, to price.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub key: String,
    pub path: PathBuf,
    /// An agent that may be writing: its subagents' transcripts are
    /// checked every run. Others are only looked at when their own
    /// transcript or subagents folder changes.
    pub live: bool,
    /// A conversation nothing has open: looked at only every so often.
    pub past: bool,
}

/// How often a past conversation is looked at again.
pub const PAST_EVERY: Ms = 15_000;
/// How often a subagent's transcript that hasn't grown in RUN_STALE is
/// looked at: one woken again is priced that late.
pub const QUIET_EVERY: Ms = 30_000;
/// A subagent run quiet this long is taken as finished.
pub const RUN_STALE: Ms = 15 * 60_000;

/// How a target's files stood at its last scan.
#[derive(Clone, Debug, Default)]
struct Seen {
    main: u64,
    /// The times of its subagents folder and its workflows' folders,
    /// which change as runs start (folder_times).
    sub_dirs: Vec<Ms>,
    subs: Vec<PathBuf>,
    /// Its workflows' agents' transcripts (workflow_transcripts).
    flows: Vec<PathBuf>,
    /// Some quiet subagents weren't looked at: the scan after isn't skipped
    /// for nothing having changed until they are.
    deferred: bool,
    at: Ms,
}

/// Prices Claude Code's transcripts, keeping what it read.
#[derive(Default)]
pub struct Scanner {
    cache: CostCache<Totals>,
    /// The cache is kept in CACHE_FILE between runs: read at the first.
    kept: bool,
    /// Where it's kept, once read.
    file: Option<PathBuf>,
    saved: Ms,
    sizes: HashMap<PathBuf, u64>,
    /// When each file was last seen to grow, and when the subagents' that
    /// haven't lately were last looked at.
    grew: HashMap<PathBuf, Ms>,
    quiet_at: Ms,
    seen: HashMap<PathBuf, Seen>,
    buf: Vec<u8>,
    day: u32,
}

impl Scanner {
    /// A scanner whose cache lives for the process only.
    pub fn new() -> Scanner {
        Scanner {
            cache: CostCache::new(CACHE_VERSION),
            buf: Vec::with_capacity(64 << 10),
            ..Scanner::default()
        }
    }

    /// A scanner that keeps its cache in CACHE_FILE between runs. It reads
    /// nothing until its first run, so that's off the UI thread, in the
    /// cache folder of the thread that runs it.
    pub fn kept() -> Scanner {
        Scanner {
            kept: true,
            ..Scanner::new()
        }
    }

    /// The totals kept for one transcript, as last scanned.
    pub fn totals(&self, path: &Path) -> Option<&Totals> {
        self.cache.files.get(&*path.to_string_lossy())
    }

    /// Writes the cache to its file, if it's kept and changed.
    pub fn save(&mut self) {
        if let Some(f) = &self.file {
            let _ = self.cache.save_to(f);
        }
    }

    /// A target's transcripts, its own and its subagents' first and its
    /// workflows' agents' after them from the index returned, or None when
    /// nothing about one that isn't live has changed since the last scan.
    fn files(&mut self, t: &Target, now: Ms) -> Option<(Vec<PathBuf>, usize)> {
        let prev = self.seen.get_mut(&t.path);
        if let Some(p) = &prev
            && t.past
            && !t.live
            && now - p.at < PAST_EVERY
        {
            return None;
        }
        let main = std::fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0);
        let sub_dirs = folder_times(&t.path);
        let (subs, flows) = match prev {
            Some(p) if !t.live && p.main == main && p.sub_dirs == sub_dirs && !p.deferred => {
                p.at = now;
                return None;
            }
            Some(p) if p.sub_dirs == sub_dirs => {
                (std::mem::take(&mut p.subs), std::mem::take(&mut p.flows))
            }
            _ => (subagent_transcripts(&t.path), workflow_transcripts(&t.path)),
        };
        let mut files = Vec::with_capacity(1 + subs.len() + flows.len());
        files.push(t.path.clone());
        files.extend(subs.iter().cloned());
        let own = files.len();
        files.extend(flows.iter().cloned());
        self.seen.insert(
            t.path.clone(),
            Seen {
                main,
                sub_dirs,
                subs,
                flows,
                deferred: false,
                at: now,
            },
        );
        Some((files, own))
    }

    /// Scans every target whose files grew and returns their new spend, by
    /// key.
    pub fn run(&mut self, targets: &[Target], now: Ms) -> HashMap<String, Spend> {
        if self.kept && self.file.is_none() {
            let f = rush_state::cache_path(CACHE_FILE);
            self.cache = CostCache::load_from(&f, CACHE_VERSION);
            self.file = Some(f);
        }
        let mut out = HashMap::new();
        let today = day(now);
        if today != self.day {
            // A new day: today's spend starts again, so everything is summed afresh.
            self.day = today;
            self.sizes.clear();
            self.seen.clear();
            self.grew.clear();
        }
        let quiet = now - self.quiet_at >= QUIET_EVERY;
        if quiet {
            self.quiet_at = now;
        }
        for t in targets {
            let Some((files, own)) = self.files(t, now) else {
                continue;
            };
            let (mut changed, mut deferred) = (false, false);
            let mut sizes: Vec<Option<u64>> = Vec::with_capacity(files.len());
            for (i, f) in files.iter().enumerate() {
                if let Some(&sz) = self.sizes.get(f)
                    && i > 0
                    && t.live
                    && !quiet
                    && now - self.grew.get(f).copied().unwrap_or(0) > RUN_STALE
                {
                    sizes.push(Some(sz)); // a live session's subagent, quiet a while: as it was
                    deferred = true;
                    continue;
                }
                let Ok(m) = std::fs::metadata(f) else {
                    sizes.push(None);
                    continue;
                };
                sizes.push(Some(m.len()));
                if self.sizes.get(f).copied().unwrap_or(0) != m.len() {
                    changed = true;
                    self.grew.insert(f.clone(), now);
                }
            }
            if let Some(st) = self.seen.get_mut(&t.path) {
                st.deferred = deferred;
            }
            if !changed && !self.sizes.is_empty() && self.sizes.contains_key(&t.path) {
                continue;
            }
            let mut sp = Spend::default();
            for (n, (f, size)) in files.iter().zip(&sizes).enumerate() {
                let tot = self.cache.get_mut(&f.to_string_lossy());
                // Of a session's many subagent transcripts, most haven't
                // grown since they were last read: their totals stand.
                let mut read = false;
                if size.is_none_or(|s| s != tot.size) {
                    let before = tot.offset;
                    let _ = scan(f, tot, &mut self.buf);
                    read = tot.offset != before;
                }
                self.sizes.insert(f.clone(), tot.size);
                let (c, u) = tot.spend();
                sp.cost += c;
                sp.usage.add(u);
                sp.today += tot.day_spend(today);
                // A workflow's agent: what it spent, nothing more.
                let ours = n < own;
                if *f == t.path {
                    sp.model.clone_from(&tot.last_model);
                    sp.first = tot.first;
                    sp.last = tot.last;
                    sp.halt.clone_from(&tot.halt);
                    sp.progress.clone_from(&tot.progress);
                    sp.progress_at = tot.progress_at;
                    sp.context = tot.context();
                    sp.compacts = tot.compacts;
                    sp.dir.clone_from(&tot.dir);
                    sp.dir_at = tot.dir_at;
                }
                if ours && tot.last > sp.last {
                    sp.last = tot.last;
                }
                for p in tot.prs.iter().filter(|_| ours) {
                    if !sp.prs.contains(p) {
                        sp.prs.push(p.clone());
                    }
                }
                for d in tot.dirs.iter().filter(|_| ours) {
                    if !sp.dirs.contains(d) {
                        sp.dirs.push(d.clone());
                    }
                }
                if read {
                    self.cache.mark_dirty();
                }
            }
            if self.buf.capacity() > 8 << 20 {
                self.buf = Vec::with_capacity(64 << 10);
            }
            sp.ready = true;
            out.insert(t.key.clone(), sp);
        }
        if now - self.saved > SAVE_EVERY {
            self.save();
            self.saved = now;
        }
        out
    }
}

/// The times of a session's subagents folder, its workflows folder and
/// each workflow run's folder: one changes as a subagent, a run or a run's
/// agent starts. Empty past the first when there's no subagents folder.
fn folder_times(main: &Path) -> Vec<Ms> {
    let subs = subagents_dir(main);
    let Some(at) = mtime(&subs) else {
        return vec![0];
    };
    let mut out = vec![at];
    let flows = subs.join(WORKFLOWS);
    let Some(at) = mtime(&flows) else {
        return out;
    };
    out.push(at);
    let mut runs: Vec<(PathBuf, Ms)> = std::fs::read_dir(&flows)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_dir()
                .then(|| (e.path(), m.modified().map_or(0, crate::jobs::ms_of)))
        })
        .collect();
    runs.sort();
    out.extend(runs.into_iter().map(|(_, t)| t));
    out
}

/// The transcripts of the agents a session's workflows ran, sorted:
/// subagents/workflows/<run>/*.jsonl.
pub fn workflow_transcripts(main: &Path) -> Vec<PathBuf> {
    let Ok(runs) = std::fs::read_dir(subagents_dir(main).join(WORKFLOWS)) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = runs
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| std::fs::read_dir(e.path()).ok())
        .flat_map(|ents| ents.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tests::TempDir;
    use std::io::Write;

    fn grow(p: &Path) {
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(b"{}\n").unwrap();
    }

    fn setup(d: &TempDir) -> (PathBuf, PathBuf) {
        let main = d.0.join("s.jsonl");
        let subs = d.0.join("s").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let sub = subs.join("agent-a1.jsonl");
        std::fs::write(&main, "{}\n").unwrap();
        std::fs::write(&sub, "{}\n").unwrap();
        (main, sub)
    }

    fn size(p: &Path) -> u64 {
        std::fs::metadata(p).unwrap().len()
    }

    // A live session's subagent transcript quiet past RUN_STALE is looked
    // at every QUIET_EVERY, not every scan; the session's own, every scan.
    #[test]
    fn skips_quiet_subagents() {
        let d = TempDir::new("quiet");
        let (main, sub) = setup(&d);
        let mut s = Scanner::new();
        let targets = [Target {
            key: "k".into(),
            path: main.clone(),
            live: true,
            past: false,
        }];
        let t0 = rush_core::time::now();
        s.run(&targets, t0);
        assert!(
            s.sizes.contains_key(&sub),
            "the subagent's transcript wasn't read"
        );
        s.grew.insert(sub.clone(), t0 - RUN_STALE - 60_000);
        let was = s.sizes[&sub];
        grow(&sub);
        grow(&main);
        s.run(&targets, t0 + 1);
        assert_eq!(
            s.sizes[&sub], was,
            "a quiet subagent was looked at before QUIET_EVERY"
        );
        assert_eq!(
            s.sizes[&main],
            size(&main),
            "the session's own transcript wasn't looked at"
        );
        s.quiet_at = 0;
        s.run(&targets, t0 + 2);
        assert_eq!(
            s.sizes[&sub],
            size(&sub),
            "a quiet subagent wasn't looked at after QUIET_EVERY"
        );
    }

    // A subagent's write left unread while its session was live is read
    // once the session isn't, though its own transcript didn't change.
    #[test]
    fn catches_up_deferred() {
        let d = TempDir::new("deferred");
        let (main, sub) = setup(&d);
        let mut s = Scanner::new();
        let live = [Target {
            key: "k".into(),
            path: main.clone(),
            live: true,
            past: false,
        }];
        let t0 = rush_core::time::now();
        s.run(&live, t0);
        s.grew.insert(sub.clone(), t0 - RUN_STALE - 60_000);
        grow(&sub);
        s.run(&live, t0 + 1); // deferred
        let idle = [Target {
            live: false,
            ..live[0].clone()
        }];
        s.run(&idle, t0 + 2);
        assert_eq!(
            s.sizes[&sub],
            size(&sub),
            "the deferred write was never read"
        );
    }

    #[test]
    fn prices_a_session_and_its_subagents() {
        let d = TempDir::new("price");
        let (main, sub) = setup(&d);
        let line = |id: &str, out: i64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-09-23T10:00:00Z","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"https://github.com/a/b/pull/{out}"}}],"usage":{{"output_tokens":{out}}}}}}}"#
            )
        };
        std::fs::write(&main, line("m1", 1) + "\n").unwrap();
        std::fs::write(&sub, line("s1", 2) + "\n").unwrap();
        let mut s = Scanner::new();
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let now = rush_core::time::now();
        let out = s.run(&t, now);
        let sp = &out["k"];
        assert!(sp.ready);
        assert_eq!(sp.usage.output, 3);
        assert_eq!(sp.model, "claude-opus-5-5");
        assert_eq!(sp.prs.len(), 2);
        assert!((sp.cost - 3.0 * 20.0 / 1e6).abs() < 1e-12);
        // Nothing changed: nothing to report.
        assert!(s.run(&t, now + 1).is_empty());
    }

    fn line(id: &str, out: i64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-23T10:00:00Z","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"x"}}],"usage":{{"output_tokens":{out}}}}}}}"#
        ) + "\n"
    }

    /// A session with a subagent, its cache kept in a temp cache folder.
    fn kept(d: &TempDir) -> (PathBuf, PathBuf, [Target; 1]) {
        let (main, sub) = setup(d);
        std::fs::write(&main, line("m1", 1)).unwrap();
        std::fs::write(&sub, line("s1", 2)).unwrap();
        let cache = d.0.join("cache");
        rush_state::set_thread_cache_dir(Some(cache.clone()));
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        (main, cache, t)
    }

    fn output(s: &mut Scanner, t: &[Target]) -> i64 {
        s.run(t, rush_core::time::now())["k"].usage.output
    }

    // What one run read is saved, and the next run starts from it.
    #[test]
    fn keeps_the_cache_between_runs() {
        let d = TempDir::new("kept");
        let (main, cache, t) = kept(&d);
        let mut a = Scanner::kept();
        assert_eq!(output(&mut a, &t), 3);
        let file = cache.join(CACHE_FILE);
        let back: CostCache<Totals> = CostCache::load_from(&file, CACHE_VERSION);
        assert_eq!(back.version, CACHE_VERSION);
        assert_eq!(back.files.len(), 2);
        assert_eq!(
            back.files.get(&*main.to_string_lossy()),
            a.totals(&main),
            "the saved totals aren't what was read"
        );
        // Rewritten in place, its size the same: a scanner that read the
        // cache doesn't read it again.
        std::fs::write(&main, line("m1", 9)).unwrap();
        let mut b = Scanner::kept();
        assert_eq!(output(&mut b, &t), 3, "the transcript was read again");
    }

    // A transcript that grew since is read only from where the last run
    // stopped; one that shrank is read again from its start.
    #[test]
    fn reads_only_what_grew() {
        let d = TempDir::new("grew");
        let (main, _, t) = kept(&d);
        output(&mut Scanner::kept(), &t);
        // Its first line changed, at the same length, and a line added: only
        // the added line is read.
        std::fs::write(&main, line("m1", 9) + &line("m2", 4)).unwrap();
        assert_eq!(output(&mut Scanner::kept(), &t), 1 + 4 + 2);
        // Shorter than what was read: read again.
        std::fs::write(&main, line("m3", 7)).unwrap();
        assert_eq!(output(&mut Scanner::kept(), &t), 7 + 2);
    }

    // A cache of another version, or one that doesn't read, starts over.
    #[test]
    fn starts_over_on_another_version() {
        let d = TempDir::new("stale");
        let (main, cache, t) = kept(&d);
        std::fs::create_dir_all(&cache).unwrap();
        let file = cache.join(CACHE_FILE);
        let size = std::fs::metadata(&main).unwrap().len();
        let stale = format!(
            r#"{{"version":{},"files":{{"{}":{{"offset":{size},"size":{size},"cost":1000.0}}}}}}"#,
            CACHE_VERSION - 1,
            main.display()
        );
        for old in [
            stale.as_str(),
            "{not json",
            r#"{"version":1,"files":{"x":7}}"#,
        ] {
            std::fs::write(&file, old).unwrap();
            let mut s = Scanner::kept();
            let sp = s.run(&t, rush_core::time::now()).remove("k").unwrap();
            assert_eq!(sp.usage.output, 3, "{old}");
            assert!(sp.cost < 1.0, "{old}: the stale totals were used");
            let back: CostCache<Totals> = CostCache::load_from(&file, CACHE_VERSION);
            assert_eq!(back.files.len(), 2, "{old}: not written afresh");
        }
    }

    // Go's costs.json, beside it, is never read or written.
    #[test]
    fn leaves_gos_costs_json_alone() {
        let d = TempDir::new("gos");
        let (main, cache, t) = kept(&d);
        std::fs::create_dir_all(&cache).unwrap();
        let gos = cache.join("costs.json");
        let size = std::fs::metadata(&main).unwrap().len();
        // Were it read, this would stand for the transcript.
        let theirs = format!(
            r#"{{"version":{CACHE_VERSION},"files":{{"{}":{{"offset":{size},"size":{size},"cost":1000.0}}}}}}"#,
            main.display()
        );
        std::fs::write(&gos, &theirs).unwrap();
        let mut s = Scanner::kept();
        assert_eq!(output(&mut s, &t), 3);
        std::fs::write(&main, line("m1", 1) + &line("m2", 4)).unwrap();
        assert_eq!(output(&mut s, &t), 7);
        s.save();
        assert_eq!(std::fs::read_to_string(&gos).unwrap(), theirs);
        let mut names: Vec<String> = std::fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["costs-rs.json", "costs.json"]);
    }

    /// An assistant line that spent out output tokens, said a pull
    /// request, worked in dir, at the time given.
    fn said(id: &str, out: i64, pr: u32, dir: &str, at: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","cwd":"{dir}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"https://github.com/a/b/pull/{pr}"}}],"usage":{{"output_tokens":{out}}}}}}}"#
        ) + "\n"
    }

    fn workflow_agent(main: &Path, run: &str, name: &str, body: &str) -> PathBuf {
        let dir = subagents_dir(main).join(WORKFLOWS).join(run);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    // A workflow's agents' transcripts (subagents/workflows/<run>/) are
    // priced with the session's: their cost, tokens and today's share. Go
    // leaves them out (spend.go:76); the transcripts say they were spent.
    #[test]
    fn prices_workflow_agents() {
        let d = TempDir::new("flows");
        let (main, sub) = setup(&d);
        std::fs::write(&main, said("m1", 1, 1, "/w/main", "2026-09-23T10:00:00Z")).unwrap();
        std::fs::write(&sub, said("s1", 2, 2, "/w/sub", "2026-09-23T10:01:00Z")).unwrap();
        workflow_agent(
            &main,
            "wf_1",
            "agent-a.jsonl",
            &said("w1", 4, 3, "/w/flow", "2026-09-23T11:00:00Z"),
        );
        workflow_agent(
            &main,
            "wf_2",
            "agent-b.jsonl",
            &said("w2", 8, 4, "/w/flow2", "2026-09-23T12:00:00Z"),
        );
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let sp = s_run(&mut Scanner::new(), &t);
        assert_eq!(sp.usage.output, 1 + 2 + 4 + 8);
        assert!((sp.cost - 15.0 * 20.0 / 1e6).abs() < 1e-12, "{}", sp.cost);
        // Its pull requests, folders and last word stay the session's and
        // its subagents'.
        assert_eq!(sp.prs.len(), 2, "{:?}", sp.prs);
        assert!(
            !sp.dirs.iter().any(|d| d.starts_with("/w/flow")),
            "{:?}",
            sp.dirs
        );
        assert_eq!(sp.last, last_of(&sub));
    }

    fn s_run(s: &mut Scanner, t: &[Target]) -> Spend {
        s.run(t, rush_core::time::now())
            .remove("k")
            .expect("priced")
    }

    /// When a lone transcript's last line was written, as scanned.
    fn last_of(p: &Path) -> Ms {
        let mut tot = Totals::default();
        let _ = scan(p, &mut tot, &mut Vec::new());
        tot.last
    }

    // A session that isn't live is priced again when a workflow starts,
    // though its own transcript and subagents folder didn't change.
    #[test]
    fn a_new_workflow_is_priced() {
        let d = TempDir::new("newflow");
        let (main, sub) = setup(&d);
        std::fs::write(&main, line("m1", 1)).unwrap();
        std::fs::write(&sub, line("s1", 2)).unwrap();
        workflow_agent(&main, "wf_1", "agent-a.jsonl", &line("w1", 4));
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let mut s = Scanner::new();
        let now = rush_core::time::now();
        assert_eq!(s.run(&t, now)["k"].usage.output, 7);
        assert!(s.run(&t, now + 1).is_empty(), "nothing changed");
        workflow_agent(&main, "wf_2", "agent-b.jsonl", &line("w2", 8));
        assert_eq!(s.run(&t, now + 2)["k"].usage.output, 15);
    }

    // A live session's workflow agent is read again as it grows.
    #[test]
    fn a_growing_workflow_agent_is_priced() {
        let d = TempDir::new("growflow");
        let (main, _) = setup(&d);
        let agent = workflow_agent(&main, "wf_1", "agent-a.jsonl", &line("w1", 4));
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: true,
            past: false,
        }];
        let mut s = Scanner::new();
        let now = rush_core::time::now();
        assert_eq!(s.run(&t, now)["k"].usage.output, 4);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&agent)
            .unwrap();
        f.write_all(line("w2", 5).as_bytes()).unwrap();
        assert_eq!(s.run(&t, now + 1)["k"].usage.output, 9);
    }

    // Only a run's folder's *.jsonl are a workflow's agents: not the
    // run's own record beside them, nor anything else.
    #[test]
    fn lists_workflow_agents() {
        let d = TempDir::new("listflows");
        let main = d.0.join("s.jsonl");
        assert!(workflow_transcripts(&main).is_empty(), "no folder: none");
        let b = workflow_agent(&main, "wf_2", "agent-b.jsonl", "");
        let a = workflow_agent(&main, "wf_1", "agent-a.jsonl", "");
        workflow_agent(&main, "wf_1", "agent-a.meta.json", "");
        let flows = subagents_dir(&main).join(WORKFLOWS);
        std::fs::write(flows.join("stray.jsonl"), "").unwrap();
        assert_eq!(workflow_transcripts(&main), [a, b]);
        // The subagents' own list is as Go's: the folder's *.jsonl only.
        assert!(subagent_transcripts(&main).is_empty());
    }
}
