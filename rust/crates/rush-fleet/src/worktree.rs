//! Each linked git worktree an agent works in, and whether it can go
//! without losing anything. Port of internal/fleet/worktree.go: Worktree
//! (:27), Safe/Pushed/Losses (:70-93), git (:102-120), workDirs (:122),
//! FindWorktrees (:134), Check/Doomed/look (:221-268), changes (:270),
//! unkept (:288), RemoveWorktree/TidyWorktree (:333-396), mainCheckout
//! (:411-465), linkedWorktree (:467), TranscriptCwd (:492) and
//! SubWorktree(s) (:529-560).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use rush_core::adapter::TempDir;
use rush_core::agent::{Agent, PrStatus};
use rush_core::time::{Ms, now};
use rush_gh::ghpr::Gh;
use serde_json::json;

use crate::Predicates;
use crate::git::git_at;
use crate::pr::from_status;
use crate::temp::{disk_usage, disk_usage_background};
use crate::verdict::{MADE_BY_RUSH, made_by};

/// A linked git worktree an agent works in, and whether it can go without
/// losing anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: String,
    /// The main checkout it belongs to.
    pub repo: String,
    pub branch: String,
    /// Keys of the agents working in it.
    pub agents: Vec<String>,
    /// Under a repo's .claude/worktrees: made for an agent.
    pub claude: bool,
    pub size: i64,

    /// Files with uncommitted changes, untracked ones included.
    pub changed: usize,
    /// Tracked files gone from disk, nothing staged: nothing lost.
    pub deleted: usize,
    /// Commits on no remote and not in main.
    pub unpushed: usize,
    /// Commits on no remote, main or not.
    pub off_remote: usize,
    /// The repo has no remote at all.
    pub no_remote: bool,
    pub locked: bool,
    pub err: String,
    pub checked: Ms,

    /// Who made it (MADE_BY_RUSH, …), and whether that's rush, for a
    /// session: only those ever go without being ticked by hand.
    pub made: &'static str,
    pub own: bool,
    /// Names a session working in it now; "" when none is.
    pub in_use: String,
    /// Its branch's pull request, as GitHub last said.
    pub pr: Option<PrStatus>,
    /// The branch its work is meant to land on, and the commit its
    /// session's worktree started from, when that's known.
    pub target: String,
    pub base: String,
    /// Its HEAD's commit, and whether that's on target, here or on origin.
    pub head: String,
    pub landed: bool,
    /// HEAD is in its merged pull request's head.
    pub(crate) in_pr: bool,
    /// HEAD has a commit made in this worktree since base.
    pub(crate) own_work: bool,
}

impl Worktree {
    /// Every change committed, and on a remote or in the main checkout's
    /// branch, or in its merged pull request: removing it loses nothing
    /// but files git ignores. See verdict for whether it should go.
    pub fn safe(&self) -> bool {
        self.err.is_empty()
            && self.checked != 0
            && self.changed == 0
            && (self.unpushed == 0 || self.merged_pr())
            && !self.locked
    }

    /// Safe with every commit on a remote: what the automatic tidy-up asks.
    pub fn pushed(&self) -> bool {
        self.safe() && self.off_remote == 0 && !self.no_remote
    }

    /// What removing it would throw away, for a confirmation.
    pub fn losses(&self) -> String {
        let mut out = Vec::new();
        if self.changed > 0 {
            out.push(plural(self.changed, "file") + " with uncommitted changes");
        }
        if self.unpushed > 0 && !self.merged_pr() {
            out.push(plural(self.unpushed, "commit") + " not pushed or in main");
        }
        if self.locked {
            out.push("a lock someone put on it".into());
        }
        out.join(" · ")
    }

    /// Looks at it with git: what's uncommitted, what's unpushed, and how
    /// much disk it takes. It can take seconds on a big checkout.
    pub fn check(&mut self) {
        if self.check_git() {
            self.size = disk_usage_background(&[TempDir {
                path: self.path.clone(),
                keep: false,
            }]);
        }
    }

    /// Check without the size: only what git says.
    pub(crate) fn check_git(&mut self) -> bool {
        self.look().2
    }

    /// Looks at it afresh with git and says, line by line, what removing it
    /// would lose: its uncommitted files as git status names them, and its
    /// commits nowhere else, as "sha subject". Never on the UI's thread.
    pub fn doomed(&mut self) -> (Vec<String>, Vec<String>) {
        let (f, c, _) = self.look();
        (f, c)
    }

    fn look(&mut self) -> (Vec<String>, Vec<String>, bool) {
        self.checked = now();
        self.err.clear();
        let Ok(status) = git(
            &self.path,
            &["status", "--porcelain", "--untracked-files=normal"],
        ) else {
            self.err = "git status failed".into();
            return (Vec::new(), Vec::new(), false);
        };
        let (files, deleted) = changes(&status);
        self.deleted = deleted;
        self.changed = files.len();
        let remotes = git(&self.path, &["remote"]).unwrap_or_default();
        self.no_remote = remotes.trim().is_empty();
        let Ok((commits, off)) = unkept(&self.path, &self.repo, self.no_remote) else {
            self.err = "couldn't compare with the remote".into();
            return (files, Vec::new(), false);
        };
        self.unpushed = commits.len();
        self.off_remote = off;
        self.landed();
        (files, commits, true)
    }

    /// Takes in what a session working in it knows of it: its target and
    /// base, and whether it's working there now.
    fn agent_says(&mut self, a: &Agent) {
        if let Some(ws) = &a.workspace
            && ws.path == self.path
        {
            if !ws.target.is_empty() {
                self.target.clone_from(&ws.target);
            }
            if !ws.base.is_empty() {
                self.base.clone_from(&ws.base);
            }
        }
        if self.in_use.is_empty() && a.in_use() {
            self.in_use = first_non_empty(&[&a.display_name, "a session"]).to_string();
        }
    }
}

pub(crate) fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// git -C dir args, within 20s, its output less trailing newlines.
pub(crate) fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    git_within(Some(Duration::from_secs(20)), dir, args)
}

/// git with a time limit of its own; None is none, for what mustn't be
/// stopped halfway: git worktree remove killed partway leaves a checkout
/// hollowed out. An error says what git wrote to stderr, when it ran.
pub(crate) fn git_within(
    limit: Option<Duration>,
    dir: &str,
    args: &[&str],
) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0"); // a look must not block the agent's own git
    let out = match limit {
        Some(l) => rush_sys::cmd::output_within(&mut cmd, l)
            .map_err(|e| e.to_string())
            .and_then(|o| if o.ok() { Ok(o.out) } else { Err(o.err_text()) })?,
        None => {
            let o = cmd
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| e.to_string())?;
            if !o.status.success() {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                if err.is_empty() {
                    return Err(format!("exit status {}", o.status.code().unwrap_or(-1)));
                }
                return Err(format!("git: {err}"));
            }
            o.stdout
        }
    };
    Ok(String::from_utf8_lossy(&out)
        .trim_end_matches('\n')
        .to_string())
}

/// The folders an agent is known to have worked in.
fn work_dirs(a: &Agent) -> Vec<String> {
    let mut out = a.spend.dirs.clone();
    for d in [&a.job.worktree_path, &a.job.cwd] {
        if !d.is_empty() {
            out.push(d.clone());
        }
    }
    out
}

/// The linked worktrees of every repository the agents work in, each with
/// the agents working in it. Nothing is checked yet. gh says each
/// branch's pull request, as GitHub last said it.
pub fn find_worktrees(agents: &[Agent], gh: Option<&Gh>) -> Vec<Worktree> {
    let mut repos: Vec<String> = Vec::new();
    let mut seen = HashMap::new();
    for a in agents {
        for d in work_dirs(a) {
            let main = main_checkout_in(&d, &mut seen);
            if !main.is_empty() && !repos.contains(&main) {
                repos.push(main);
            }
        }
    }
    let mut out = Vec::new();
    for repo in &repos {
        let Ok(list) = git(repo, &["worktree", "list", "--porcelain"]) else {
            continue;
        };
        let mut w: Option<Worktree> = None;
        let flush = |w: &mut Option<Worktree>, out: &mut Vec<Worktree>| {
            if let Some(x) = w.take()
                && x.path != *repo
            {
                out.push(x);
            }
        };
        for l in list.split('\n') {
            if let Some(p) = l.strip_prefix("worktree ") {
                flush(&mut w, &mut out);
                w = Some(Worktree {
                    path: p.to_string(),
                    repo: repo.clone(),
                    claude: p.contains("/.claude/worktrees/"),
                    target: git_at(repo).1, // the main checkout's branch, unless its session says
                    made: made_by(p, ""),
                    ..Worktree::default()
                });
                continue;
            }
            let Some(x) = w.as_mut() else { continue };
            if let Some(b) = l.strip_prefix("branch ") {
                x.branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
                x.made = made_by(&x.path, &x.branch);
                x.own = x.made == MADE_BY_RUSH;
                if let Some(g) = gh {
                    x.pr = g.for_branch(repo, &x.branch).map(from_status);
                }
            } else if l == "locked" || l.starts_with("locked ") {
                x.locked = true;
            } else if l == "prunable" || l.starts_with("prunable ") {
                w = None; // its folder is already gone
            }
        }
        flush(&mut w, &mut out);
    }
    let dirs: Vec<Vec<String>> = agents.iter().map(work_dirs).collect();
    for w in &mut out {
        for (j, a) in agents.iter().enumerate() {
            if dirs[j].iter().any(|d| within(d, &w.path)) {
                w.agents.push(a.key.clone());
                w.agent_says(a);
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether d is path or under it.
fn within(d: &str, path: &str) -> bool {
    d.starts_with(path) && (d.len() == path.len() || d.as_bytes()[path.len()] == b'/')
}

/// The lines of git status --porcelain that removing the worktree would
/// lose. A tracked file gone from disk and nothing staged (" D") loses
/// nothing: its content is in HEAD.
pub(crate) fn changes(status: &str) -> (Vec<String>, usize) {
    let mut lost = Vec::new();
    let mut deleted = 0;
    for l in status.split('\n') {
        if l.is_empty() {
            continue;
        }
        if l.starts_with(" D ") {
            deleted += 1;
        } else {
            lost.push(l.to_string());
        }
    }
    (lost, deleted)
}

/// The worktree's commits that are nowhere else, as "sha subject": on no
/// remote, and not in the main checkout's branch, as themselves or
/// cherry-picked. off counts those on no remote, in main or not.
fn unkept(path: &str, repo: &str, no_remote: bool) -> Result<(Vec<String>, usize), String> {
    let mut args = vec!["rev-list", "HEAD"];
    if !no_remote {
        args.extend(["--not", "--remotes"]);
    }
    let out = git(path, &args)?;
    if out.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let shas: Vec<&str> = out.split_whitespace().collect();
    let short = |s: &str| s[..s.len().min(7)].to_string();
    let all = || (shas.iter().map(|s| short(s)).collect(), shas.len());
    let Ok(base) = git(repo, &["rev-parse", "HEAD"]) else {
        return Ok(all());
    };
    // git cherry marks with + what base has no equivalent of, and doesn't
    // list what base already contains.
    let Ok(cherry) = git(path, &["cherry", "-v", &base, "HEAD"]) else {
        return Ok(all());
    };
    let mut unique: HashMap<&str, &str> = HashMap::new();
    for l in cherry.split('\n') {
        if let Some(rest) = l.strip_prefix("+ ") {
            let (sha, subject) = rest.split_once(' ').unwrap_or((rest, ""));
            unique.insert(sha, subject);
        }
    }
    let lost = shas
        .iter()
        .filter_map(|s| {
            unique
                .get(s)
                .map(|subject| format!("{} {subject}", short(s)))
        })
        .collect();
    Ok((lost, shas.len()))
}

/// Removes a worktree through git, which also forgets it in the main
/// checkout; its branch stays. Unless force is set, git refuses one with
/// uncommitted changes, and this refuses one with unpushed commits.
pub fn remove_worktree(w: &Worktree, force: bool) -> Result<(), String> {
    let r = remove(w, force, false).map(|_| ());
    worktree_event(
        "worktree-removed",
        w,
        json!({"force": force}),
        r.as_ref().err(),
    );
    r
}

/// RemoveWorktree for the automatic clean-up: only a worktree the rule
/// says can go goes. It says how much disk went.
pub fn tidy_worktree(w: &Worktree) -> Result<i64, String> {
    let r = remove(w, false, true);
    let size = *r.as_ref().unwrap_or(&0);
    worktree_event(
        "worktree-tidied",
        w,
        json!({"bytes": size, "why": "done, and the rule says it can go"}),
        r.as_ref().err(),
    );
    r
}

/// Logs a worktree removed, or refused.
pub(crate) fn worktree_event(
    kind: &str,
    w: &Worktree,
    mut f: serde_json::Value,
    err: Option<&String>,
) {
    f["path"] = json!(w.path);
    f["repo"] = json!(w.repo);
    f["branch"] = json!(w.branch);
    f["agents"] = json!(w.agents);
    if let Some(e) = err {
        f["err"] = json!(e);
    }
    rush_sys::events::log(kind, f);
}

/// Removes w; measure is the automatic clean-up, which asks for Pushed
/// rather than Safe.
pub(crate) fn remove(w: &Worktree, force: bool, measure: bool) -> Result<i64, String> {
    linked_worktree(w)?;
    let mut deleted = false;
    let base = Path::new(&w.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !force {
        let mut c = w.clone();
        c.check_git();
        deleted = c.deleted > 0;
        if !c.safe() {
            return Err(format!(
                "{base} isn't safe to remove: {}",
                first_non_empty(&[&c.losses(), &c.err])
            ));
        }
        let v = c.verdict();
        if measure && !v.can_go() {
            return Err(format!("{base} isn't safe to remove: {}", v.reason));
        }
    }
    let size = if measure {
        disk_usage(&[TempDir {
            path: w.path.clone(),
            keep: false,
        }])
    } else {
        0
    };
    let mut args = vec!["worktree", "remove"];
    if deleted {
        // git refuses a worktree with deleted files; checked above, they
        // lose nothing.
        args.push("--force");
    }
    if force {
        args.extend(["--force", "--force"]);
    }
    args.push(&w.path);
    git_within(None, &w.repo, &args)?;
    Ok(size)
}

pub(crate) fn first_non_empty<'a>(s: &[&'a str]) -> &'a str {
    s.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// The main checkout of the repository dir is in, through any linked
/// worktree; dir itself when it isn't in one.
pub fn main_checkout(dir: &str) -> String {
    let m = main_checkout_in(dir, &mut HashMap::new());
    if m.is_empty() { dir.to_string() } else { m }
}

/// The main checkout of the repository dir is in, by looking for .git
/// upwards: stats only, no git. A linked worktree's .git is a file naming
/// <main>/.git/worktrees/<name>. seen remembers answers; "" when none.
pub(crate) fn main_checkout_in(dir: &str, seen: &mut HashMap<String, String>) -> String {
    let mut walked = Vec::new();
    let mut main = String::new();
    let mut d = Path::new(dir);
    loop {
        let s = d.to_string_lossy();
        if s == "/" || s == "." || s.is_empty() {
            break;
        }
        if let Some(m) = seen.get(s.as_ref()) {
            main = m.clone();
            break;
        }
        walked.push(s.to_string());
        let p = d.join(".git");
        if let Ok(st) = std::fs::metadata(&p) {
            if st.is_dir() {
                main = s.to_string();
                break;
            }
            let b = std::fs::read_to_string(&p).unwrap_or_default();
            let t = b.trim();
            let g = t.strip_prefix("gitdir:").unwrap_or(t).trim();
            let g = if g.starts_with('/') {
                g.to_string()
            } else {
                d.join(g).to_string_lossy().into_owned()
            };
            if let Some(i) = g.find("/.git/worktrees/") {
                main = g[..i].to_string();
            }
            break;
        }
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    if !main.is_empty() {
        for d in walked {
            seen.insert(d, main.clone());
        }
    }
    main
}

/// Refuses anything that isn't a linked worktree: a repo's own checkout,
/// a folder holding the repo, or one whose .git doesn't point back into
/// the repo's worktrees. Only a linked worktree is ever removed.
pub(crate) fn linked_worktree(w: &Worktree) -> Result<(), String> {
    let path = clean(&w.path);
    let repo = clean(&w.repo);
    if path == repo
        || format!("{repo}/").starts_with(&format!("{path}/"))
        || path == "/"
        || repo.is_empty()
    {
        return Err(format!(
            "{path} is the repository itself, not a worktree; it's never removed"
        ));
    }
    let dot = Path::new(&path).join(".git");
    match std::fs::symlink_metadata(&dot) {
        Ok(m) if !m.is_dir() => {}
        _ => {
            return Err(format!(
                "{path} isn't a linked worktree; it's never removed"
            ));
        }
    }
    let b = std::fs::read_to_string(&dot).unwrap_or_default();
    // As Go reads it: the prefix off first, then the space
    // (worktree.go:479).
    let g = b.strip_prefix("gitdir:").unwrap_or(&b).trim();
    if !g.starts_with(&(clean(&format!("{repo}/.git/worktrees")) + "/")) {
        return Err(format!(
            "{path} doesn't belong to {repo} as a worktree; it's never removed"
        ));
    }
    Ok(())
}

/// filepath.Clean, for the absolute paths worktrees have.
pub(crate) fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            c => parts.push(c),
        }
    }
    let j = parts.join("/");
    match (abs, j.is_empty()) {
        (true, _) => format!("/{j}"),
        (false, true) => ".".into(),
        (false, false) => j,
    }
}

/// Transcripts' folders as their heads say: one never changes. Bounded.
static CWDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The folder a transcript's head says its agent ran in: Claude Code's
/// lines and Codex's session_meta both say.
pub fn transcript_cwd(path: &str) -> String {
    if let Some(c) = CWDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(path))
    {
        return c.clone();
    }
    let Ok(f) = std::fs::File::open(path) else {
        return String::new();
    };
    const MARK: &[u8] = b"\"cwd\":\"";
    // Its first line says it, after the prompt: mostly in the first few
    // kilobytes.
    let mut b = Vec::with_capacity(16 << 10);
    let mut f = f.take(16 << 10);
    if f.read_to_end(&mut b).is_err() {
        return String::new();
    }
    let mut i = memchr::memmem::find(&b, MARK);
    if i.is_none() && b.len() == 16 << 10 {
        let mut f = f.into_inner().take(48 << 10);
        let _ = f.read_to_end(&mut b);
        i = memchr::memmem::find(&b, MARK);
    }
    let Some(i) = i else {
        return String::new();
    };
    let s = &b[i + MARK.len() - 1..];
    let Some(j) = s[1..].iter().position(|&c| c == b'"') else {
        return String::new();
    };
    let cwd: String = serde_json::from_slice(&s[..j + 2]).unwrap_or_default();
    if cwd.is_empty() {
        return String::new();
    }
    let mut m = CWDS.lock().unwrap_or_else(|e| e.into_inner());
    let m = m.get_or_insert_with(HashMap::new);
    if m.len() > 8192 {
        m.clear();
    }
    m.insert(path.to_string(), cwd.clone());
    cwd
}

/// The checkout a subagent's transcript says it worked in, by its branch
/// (or folder), when that isn't its session's, whose folder is cwd: ""
/// when it's the same. None while that can't be told.
pub fn sub_worktree(transcript: &str, cwd: &str) -> Option<String> {
    sub_worktrees(&[transcript], cwd).remove(transcript)
}

/// SubWorktree for many runs of one session, asking about each folder
/// once: by transcript, those that can be told.
pub fn sub_worktrees(transcripts: &[&str], cwd: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if cwd.is_empty() {
        return out;
    }
    let own = git_at(cwd).0;
    let mut seen: HashMap<String, (String, String)> = HashMap::new();
    for t in transcripts {
        let sub = transcript_cwd(t);
        if sub.is_empty() {
            continue;
        }
        let (repo, branch) = seen
            .entry(sub.clone())
            .or_insert_with(|| git_at(&sub))
            .clone();
        let mut label = String::new();
        if !repo.is_empty() && repo != own {
            label = if branch.is_empty() {
                Path::new(&repo)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                branch
            };
        }
        out.insert(t.to_string(), label);
    }
    out
}
