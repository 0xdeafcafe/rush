//! The Changes view: the session's own edits by file, each opening to its
//! hunks under the turn that made them, then the working tree as git sees
//! it, the files this session didn't touch marked as such. The files its
//! shell commands plainly wrote are there, marked by shell, and so are
//! its subagents' edits, marked with their names; the files it edited
//! outside its project are not (they're on Overview and Memory). git is
//! only ever run off the UI thread: a reading older than five seconds is
//! taken again once the view is drawn, and the frame after has it. After
//! Go's internal/convo/changes.go (FileChange :20, Changes :49,
//! mergeChanges :67, changesNow :96, by :203, shellOnly :211,
//! ProjectChanges :225, inside :247, Tree :256, Stat :275, WorkingTree
//! :292, readTree :329, realPath :378, turnsRight :451, ChangesView :471,
//! treeDiff :770), internal/ui/ownfiles.go (ownChanges :17, changeStat
//! :31, addDel :39) and internal/ui/rushmode.go (alt+r, readsFromTop).
//!
//! Its subagents' edits come from their conversations read whole while
//! Changes is open (subs.rs: rushmode.go:99-118 fullSubs, ownfiles.go:65-90
//! setHelpers, after each read). The column beside the conversation and
//! the status line count only the project's files (project_changes,
//! project_of; rushmode.go:3325, bars.go:257,268). The agents a step ran
//! (the conversation's kids) are that step's children here as Go's
//! SetChildren makes them (spawn.go:573, shownSteps :614): their edits
//! count under the step and again as their helper, as Go's do. Its
//! files are said from the session's folders, their symlinks resolved
//! too (git says /private/tmp for /tmp), and open when clicked
//! (drawer.rel render.go:2560, shortAbs :2571, fileLink :2007, linkTarget
//! :2031, Session.bases model.go:970).
//!
//! Not here yet: the file in full with its diff laid over it (alt+v,
//! FileView) and syntax colour in the hunks.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rush_convo::render::Child;
use rush_convo::{Item, Session, Status, StepId};
use rush_core::text::{Color, Line};
use rush_core::time::{Ms, now};
use rush_core::tool::{Kind, Patch};

use super::conn;
use super::paint::{dim, faint, plural, row, section, text};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// An added line's ground, and a deleted one's.
const BG_ADD: Color = Color::Rgb(0x16, 0x30, 0x1a);
const BG_DEL: Color = Color::Rgb(0x3a, 0x17, 0x14);

/// Everything this session did to one file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub add: i64,
    pub del: i64,
    pub new: bool,
    pub turns: Vec<u32>,
    pub patches: Vec<Patch>,
    /// For each patch, the step that made it ("t3:s:toolu_…").
    pub from: Vec<String>,
    /// A new file's content, and the step that created it.
    pub content: String,
    pub new_from: String,
    /// The shell commands that wrote it (shell_writes), by step, and what
    /// each ran: their lines aren't known, only that they did.
    pub shell: Vec<String>,
    pub cmds: Vec<String>,
    /// The subagents whose edits these are, by name, when not only the
    /// session's own (Helper).
    pub by: Vec<String>,
}

impl FileChange {
    /// Notes that subagent who made a change to it; "" is the session
    /// (by).
    fn by(&mut self, who: &str) {
        if !who.is_empty() && !self.by.iter().any(|b| b == who) {
            self.by.push(who.to_string());
        }
    }
}

/// The session's edits by file, in the order files were first touched,
/// its subagents' (Session.helpers) merged in, marked as theirs
/// (Changes, mergeChanges).
pub fn changes(s: &Session) -> Vec<FileChange> {
    changes_with(s, &Kids::new())
}

/// The agents each step ran, by its call id, as the conversation draws
/// them under it (Options.kids; Go's Session.SetChildren).
pub type Kids = HashMap<String, Vec<Child>>;

/// changes, with the agents the session's steps ran (kids) standing in
/// for those steps' children, as Go's SetChildren makes them: their
/// edits count under the step (marked "a subagent", or its description)
/// and again as the helper their conversation is, which is what Go's
/// Changes counts (changes.go:96 changesNow's walk, spawn.go:573
/// SetChildren, ownfiles.go:81 setHelpers' spawns).
pub fn changes_with(s: &Session, kids: &Kids) -> Vec<FileChange> {
    let mut out = changes_now(s, kids);
    for h in &s.helpers {
        merge(&mut out, &h.name, &h.sess);
    }
    out
}

/// Adds subagent name's edits (its session h) to changes, marked as its.
/// Its hunks name no step here: they're in its conversation, not this
/// one (mergeChanges).
fn merge(changes: &mut Vec<FileChange>, name: &str, h: &Session) {
    for hc in changes_now(h, &Kids::new()) {
        let i = match changes.iter().position(|f| f.path == hc.path) {
            Some(i) => i,
            None => {
                changes.push(FileChange {
                    path: hc.path.clone(),
                    ..FileChange::default()
                });
                changes.len() - 1
            }
        };
        let fc = &mut changes[i];
        fc.by(name);
        fc.add += hc.add;
        fc.del += hc.del;
        if hc.new && !fc.new && fc.patches.is_empty() {
            fc.new = true;
            fc.content = hc.content;
        }
        for p in hc.patches {
            fc.patches.push(p);
            fc.from.push(String::new());
        }
        fc.cmds.extend(hc.cmds);
        fc.shell.extend(hc.shell.iter().map(|_| String::new()));
    }
}

/// path is root or under it; a relative path is taken to be the
/// project's (inside).
pub fn inside(root: &str, path: &str) -> bool {
    if !path.starts_with('/') {
        return true;
    }
    let root = root.trim_end_matches('/');
    root.is_empty() || path == root || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// The session's own edits: its Edit and Write calls and the files its
/// shell commands plainly write, its subagents' steps under their calls
/// marked as theirs; a file made and then removed is left out
/// (changesNow).
fn changes_now(s: &Session, kids: &Kids) -> Vec<FileChange> {
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<FileChange> = Vec::new();
    // A shell that keeps its folder between commands (Claude Code's) runs
    // each where the last left it, while that's in the project.
    let home = if s.info.cwd.is_empty() {
        s.cwd.clone()
    } else {
        s.info.cwd.clone()
    };
    let mut shell_at = home.clone();
    let mut removed: HashSet<String> = HashSet::new();
    let mut slot = |out: &mut Vec<FileChange>, path: &str| -> usize {
        *by_path.entry(path.to_string()).or_insert_with(|| {
            out.push(FileChange {
                path: path.to_string(),
                ..FileChange::default()
            });
            out.len() - 1
        })
    };
    for t in &s.turns {
        // Each step, and the steps of the subagents it ran, when their
        // calls came in this conversation: those are marked as theirs.
        let mut steps = Vec::new();
        for it in &t.items {
            if let Item::Step(id) = it {
                walk(s, *id, "", Some(kids), &mut steps);
            }
        }
        for (s, id, who) in steps {
            let st = s.step(id);
            if st.status != Status::Ok {
                continue;
            }
            match st.kind {
                Kind::Edit | Kind::Write => {}
                Kind::Shell => {
                    let input = &st.call().input;
                    let at = if input.cwd.is_empty() {
                        &shell_at
                    } else {
                        &input.cwd
                    };
                    let (writes, gone, end) = rush_convo::shell_run(&input.command, at);
                    if input.cwd.is_empty() {
                        shell_at = home.clone();
                        if !home.is_empty() && inside(&home, &end) {
                            shell_at = end;
                        }
                    }
                    let cmd = input.command.trim().split('\n').next().unwrap_or("");
                    for w in writes {
                        removed.remove(&w.path);
                        let i = slot(&mut out, &w.path);
                        let fc = &mut out[i];
                        if fc.turns.last() != Some(&t.n) {
                            fc.turns.push(t.n);
                        }
                        fc.shell.push(format!("t{}:s:{}", t.n, st.id));
                        fc.by(&who);
                        fc.cmds.push(cmd.to_string());
                    }
                    // After what it wrote: a scratch file goes last.
                    removed.extend(gone);
                    continue;
                }
                _ => continue,
            }
            let path = st.call().input.path.clone();
            removed.remove(&path);
            let i = slot(&mut out, &path);
            let fc = &mut out[i];
            if fc.turns.last() != Some(&t.n) {
                fc.turns.push(t.n);
            }
            fc.by(&who);
            let o = st.out();
            let step_ref = format!("t{}:s:{}", t.n, st.id);
            if o.created {
                let content = st.call().input.content.clone();
                fc.new = true;
                fc.add += content.lines().count() as i64;
                fc.content = content;
                fc.new_from = step_ref;
                continue;
            }
            for p in o.patches {
                for l in &p.lines {
                    if l.starts_with('+') {
                        fc.add += 1;
                    } else if l.starts_with('-') {
                        fc.del += 1;
                    }
                }
                fc.patches.push(p);
                fc.from.push(step_ref.clone());
            }
        }
    }
    // A file the session made and then removed changed nothing.
    out.retain(|fc| !(removed.contains(&fc.path) && (fc.new || fc.add + fc.del == 0)));
    out
}

/// Step id of s, then the steps under it, each with who made it ("" the
/// session): its subagent's steps when their calls came in this
/// conversation, or the agents rush found it ran (kids, a top step's
/// only), named for the step's description, else "a subagent". One agent's
/// steps are the step's own children; several are each a row of their
/// own under it, which edit nothing, and their steps are under those
/// (changesNow's walk, SetChildren's one kid and fan).
fn walk<'a>(
    s: &'a Session,
    id: StepId,
    who: &str,
    kids: Option<&'a Kids>,
    out: &mut Vec<(&'a Session, StepId, String)>,
) {
    out.push((s, id, who.to_string()));
    let st = s.step(id);
    let d = &st.call().input.description;
    let name = if d.is_empty() {
        "a subagent"
    } else {
        d.as_str()
    };
    // Only the agents read so far are its children (hostConn.children).
    let ran: Vec<&'a Session> = kids
        .and_then(|k| k.get(&*st.id))
        .map(|v| v.iter().filter_map(|c| c.sess.as_deref()).collect())
        .unwrap_or_default();
    match ran[..] {
        [] => {
            for ch in &st.children {
                walk(s, *ch, name, None, out);
            }
        }
        [k] => {
            for ch in shown_steps(k) {
                walk(k, ch, name, None, out);
            }
        }
        _ => {
            for k in ran {
                for ch in shown_steps(k) {
                    walk(k, ch, "a subagent", None, out);
                }
            }
        }
    }
}

/// The steps of an agent's session its row shows: not its task list's
/// (shownSteps, spawn.go:614).
fn shown_steps(s: &Session) -> Vec<StepId> {
    s.turns
        .iter()
        .flat_map(|t| &t.items)
        .filter_map(|it| match it {
            Item::Step(id) if !hidden(s.step(*id)) => Some(*id),
            _ => None,
        })
        .collect()
}

/// A step the conversation doesn't draw: the task list's own calls
/// (render.go hidden; rush-convo's draw::hidden).
fn hidden(st: &rush_convo::Step) -> bool {
    st.kind == Kind::Todo
        || matches!(
            &*st.tool,
            "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet"
        )
}

/// How many of changes only shell commands made: their lines aren't in
/// the counts (shellOnly).
pub fn shell_only(changes: &[FileChange]) -> usize {
    changes
        .iter()
        .filter(|fc| fc.add + fc.del == 0 && !fc.new && !fc.shell.is_empty())
        .count()
}

/// The lines files add and delete (ownfiles.go changeStat).
pub fn change_stat(files: &[FileChange]) -> (i64, i64) {
    files
        .iter()
        .fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
}

/// The open session's edits, worked out again only when a step changed,
/// its own or a subagent's (Changes' memo).
pub fn of_open(m: &mut Model) -> Arc<Vec<FileChange>> {
    let Some(v) = m.session.as_ref() else {
        return Arc::default();
    };
    let mut ver = v.sess.step_ver();
    for h in &v.sess.helpers {
        ver += h.sess.step_ver() + 1;
    }
    // And the agents its steps ran, read so far (SetChildren's ranVer).
    let mut ran = 0;
    for k in v.o.kids.values().flatten() {
        if let Some(ks) = &k.sess {
            ver += ks.step_ver() + 1;
            ran += 1;
        }
    }
    let at = (v.sess.serial(), ver, v.sess.helpers.len() + (ran << 32));
    if let Some((k, c)) = &m.kept::<Trees>().memo
        && *k == at
    {
        return c.clone();
    }
    let Some(v) = m.session.as_ref() else {
        return Arc::default();
    };
    let c = Arc::new(changes_with(&v.sess, &v.o.kids));
    m.kept::<Trees>().memo = Some((at, c.clone()));
    c
}

/// The open session's changes split in two: the files in the project,
/// under its git root (its folder until git is read), and its own files
/// outside it, such as memory notes, plans and scratch. With no folder
/// known, every file is the project's (ProjectChanges).
pub fn project_changes(m: &mut Model) -> (Vec<FileChange>, Vec<FileChange>) {
    let all = of_open(m);
    let dir = m
        .session
        .as_ref()
        .map(|v| open_dir(&v.sess))
        .unwrap_or_default();
    if dir.is_empty() || all.is_empty() {
        return (all.to_vec(), Vec::new());
    }
    let mut root = tree_of(m, &dir).root;
    if root.is_empty() {
        root = dir;
    }
    all.iter().cloned().partition(|fc| inside(&root, &fc.path))
}

/// s's edits in its project only, as project_changes splits them, read
/// without asking git: its root as last read, else its folder. For what
/// draws from a shared Model (the status line's lines and files
/// segments, bars.go:257,268).
pub fn project_of(m: &Model, s: &Session) -> Vec<FileChange> {
    // The open session's steps have the agents they ran under them.
    let all = match m.session.as_ref().filter(|v| std::ptr::eq(&v.sess, s)) {
        Some(v) => changes_with(s, &v.o.kids),
        None => changes(s),
    };
    let dir = open_dir(s);
    if dir.is_empty() || all.is_empty() {
        return all;
    }
    let root = m
        .kept
        .peek::<Trees>()
        .and_then(|t| t.by_dir.get(&dir))
        .map(|t| t.root.clone())
        .filter(|r| !r.is_empty())
        .unwrap_or(dir);
    all.into_iter()
        .filter(|fc| inside(&root, &fc.path))
        .collect()
}

/// Where the session works: the host's folder, else its agent's.
fn open_dir(s: &Session) -> String {
    if s.info.cwd.is_empty() {
        s.cwd.clone()
    } else {
        s.info.cwd.clone()
    }
}

/// What the open session added and deleted in its project: not its
/// memory or plans, which are on their own tabs.
pub fn stat(m: &mut Model) -> (i64, i64) {
    change_stat(&project_changes(m).0)
}

/// The files session key edited outside its project, split: the memory
/// it reads, which the Memory tab shows when it has one, and the rest
/// (plans, scratch, notes), which the Overview shows (ownChanges).
pub fn own_changes(m: &mut Model, key: &str) -> (Vec<FileChange>, Vec<FileChange>) {
    let own = project_changes(m).1;
    if own.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mem_tab = super::views::views(m, key).contains(&"memory");
    let read: HashSet<String> = super::memory::memory_known(m, key)
        .map(|r| r.files.into_iter().map(|f| f.path).collect())
        .unwrap_or_default();
    own.into_iter().partition(|fc| {
        mem_tab
            && (read.contains(&fc.path)
                || crate::screens::projects_machine::memory::in_memory_dir(&fc.path))
    })
}

/// A +a −d pair, each side only when it's something (addDel).
pub fn add_del(a: i64, d: i64) -> Line {
    let mut l = Line::new();
    if a > 0 {
        l.spans.push(sp(format!("+{a}"), GREEN));
    }
    if d > 0 {
        if a > 0 {
            l.spans.push(plain(" "));
        }
        l.spans.push(sp(format!("−{d}"), RED));
    }
    l
}

/// One file git reports as changed in the working tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    pub add: i64,
    pub del: i64,
    pub binary: bool,
    pub untracked: bool,
}

/// What git sees in a folder: changed and untracked files against HEAD.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub head: String,
    pub root: String,
    pub files: Vec<TreeFile>,
    pub err: String,
    /// When it was read.
    pub at: Ms,
}

impl Tree {
    /// Lines added and deleted in the working tree.
    pub fn stat(&self) -> (i64, i64) {
        self.files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
    }
}

/// A session (its serial) at a version of its steps and its helpers', and
/// how many helpers it has.
type Ver = (u64, u64, usize);

/// Each folder's latest reading of git, and each opened file's diff.
#[derive(Default)]
pub struct Trees {
    by_dir: HashMap<String, Tree>,
    reading: HashSet<String>,
    diffs: HashMap<String, (Ms, Vec<String>)>,
    diff_reading: HashSet<String>,
    /// The open session's edits, and the session and step version they're
    /// of.
    memo: Option<(Ver, Arc<Vec<FileChange>>)>,
    /// Paths with their symlinks resolved, and those being resolved
    /// (realPath's cache).
    real: HashMap<String, String>,
    real_reading: HashSet<String>,
}

/// The paths with their symlinks resolved, as far as known: one not yet
/// is resolved off the UI thread, and is itself until then (realPath,
/// changes.go:218). /tmp is /private/tmp on macOS, where git says the
/// latter.
fn real_paths(m: &mut Model, paths: &[String]) -> Vec<String> {
    let ts = m.kept::<Trees>();
    let todo: Vec<String> = paths
        .iter()
        .filter(|p| !ts.real.contains_key(*p) && !ts.real_reading.contains(*p))
        .cloned()
        .collect();
    let out = paths
        .iter()
        .map(|p| ts.real.get(p).cloned().unwrap_or_else(|| p.clone()))
        .collect();
    if !todo.is_empty() {
        ts.real_reading.extend(todo.iter().cloned());
        m.later(
            move || {
                todo.into_iter()
                    .map(|p| {
                        let r = std::fs::canonicalize(&p)
                            .map(|r| r.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| p.clone());
                        (p, r)
                    })
                    .collect::<Vec<_>>()
            },
            |m, done| {
                let ts = m.kept::<Trees>();
                let mut moved = false;
                for (p, r) in done {
                    ts.real_reading.remove(&p);
                    moved |= p != r;
                    ts.real.insert(p, r);
                }
                if moved { Out::Draw } else { Out::Idle }
            },
        );
    }
    out
}

/// git in dir, its output when it ran clean (GIT_OPTIONAL_LOCKS=0: a look
/// must not hold up the agent's own git).
fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Reads what git sees in dir (readTree). Off the UI thread only.
pub fn read_tree(dir: &str) -> Tree {
    let mut t = Tree {
        at: now(),
        ..Tree::default()
    };
    let Some(top) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        t.err = "not a git repository".into();
        return t;
    };
    let root = top.trim().to_string();
    t.root = root.clone();
    let base = match git(dir, &["rev-parse", "--short", "HEAD"]) {
        Some(h) => {
            t.head = h.trim().to_string();
            "HEAD"
        }
        None => {
            t.head = "no commits yet".into();
            "--cached" // staged files are all there is
        }
    };
    let out = git(&root, &["diff", base, "--numstat", "-z", "--no-renames"]).unwrap_or_default();
    for rec in out.split('\0') {
        let f: Vec<&str> = rec.splitn(3, '\t').collect();
        if f.len() != 3 || f[2].is_empty() {
            continue;
        }
        let path = Path::new(&root).join(f[2]).to_string_lossy().into_owned();
        let mut tf = TreeFile {
            path,
            ..TreeFile::default()
        };
        if f[0] == "-" {
            tf.binary = true;
        } else {
            tf.add = f[0].parse().unwrap_or(0);
            tf.del = f[1].parse().unwrap_or(0);
        }
        t.files.push(tf);
    }
    let untracked =
        git(&root, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    for p in untracked.split('\0').filter(|p| !p.is_empty()) {
        t.files.push(TreeFile {
            path: Path::new(&root).join(p).to_string_lossy().into_owned(),
            untracked: true,
            ..TreeFile::default()
        });
    }
    t.files.sort_by(|a, b| a.path.cmp(&b.path));
    t
}

/// The last reading of git for dir, read again off the UI thread when
/// it's older than five seconds (WorkingTree); "reading git…" until the
/// first is in.
pub fn tree_of(m: &mut Model, dir: &str) -> Tree {
    let now_ = m.now;
    let ts = m.kept::<Trees>();
    let have = ts.by_dir.get(dir).cloned();
    let stale = have.as_ref().is_none_or(|t| now_ - t.at > 5000);
    if stale && !ts.reading.contains(dir) {
        ts.reading.insert(dir.to_string());
        let d = dir.to_string();
        m.later(move || read_tree(&d), {
            let d = dir.to_string();
            move |m: &mut Model, t: Tree| {
                let ts = m.kept::<Trees>();
                ts.reading.remove(&d);
                let same = ts
                    .by_dir
                    .get(&d)
                    .is_some_and(|o| o.files == t.files && o.head == t.head && o.err == t.err);
                ts.by_dir.insert(d, t);
                if same { Out::Idle } else { Out::Draw }
            }
        });
    }
    let ts = m.kept::<Trees>();
    ts.by_dir.get(dir).cloned().unwrap_or(Tree {
        err: "reading git…".into(),
        at: now_,
        ..Tree::default()
    })
}

/// git's diff of one working-tree file against HEAD (the file itself when
/// it's untracked), read off the UI thread with each reading of the tree
/// (treeDiff); "reading the diff…" until it's in.
fn tree_diff(m: &mut Model, t: &Tree, f: &TreeFile) -> Vec<String> {
    let k = format!("{}\0{}", t.root, f.path);
    let ts = m.kept::<Trees>();
    let have = ts.diffs.get(&k).cloned();
    if have.as_ref().is_none_or(|d| d.0 != t.at) && !ts.diff_reading.contains(&k) {
        ts.diff_reading.insert(k.clone());
        let (root, f, at) = (t.root.clone(), f.clone(), t.at);
        let key = k.clone();
        m.later(
            move || {
                let out = if f.untracked {
                    Command::new("git")
                        .args([
                            "-C",
                            &root,
                            "diff",
                            "--no-index",
                            "--",
                            "/dev/null",
                            &f.path,
                        ])
                        .env("GIT_OPTIONAL_LOCKS", "0")
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                        .unwrap_or_default()
                } else {
                    git(&root, &["diff", "HEAD", "--", &f.path]).unwrap_or_default()
                };
                diff_lines(&out)
            },
            move |m, lines| {
                let ts = m.kept::<Trees>();
                ts.diff_reading.remove(&key);
                ts.diffs.insert(key, (at, lines));
                Out::Draw
            },
        );
    }
    match have {
        Some((_, l)) => l,
        None => vec!["reading the diff…".into()],
    }
}

/// A diff's lines, less the header lines that say what the row already
/// does, 400 at most.
pub fn diff_lines(out: &str) -> Vec<String> {
    let mut lines: Vec<String> = out
        .trim_end_matches('\n')
        .split('\n')
        .filter(|l| {
            !(l.starts_with("diff --git")
                || l.starts_with("index ")
                || l.starts_with("--- ")
                || l.starts_with("+++ ")
                || l.starts_with("new file")
                || l.starts_with('\\'))
        })
        .map(str::to_string)
        .collect();
    if lines.len() > 400 {
        let more = lines.len() - 400;
        lines.truncate(400);
        lines.push(format!("… {more} more lines"));
    }
    lines
}

/// A changed file's row's right side: its counts and the turns that made
/// it, the turns left out when they'd crowd the row (turnsRight).
fn turns_right(mut counts: Line, turns: &[u32], capw: usize) -> Line {
    let room = capw.saturating_sub(4 + counts.width() + 3);
    let mut b = String::new();
    for (i, n) in turns.iter().enumerate() {
        if i > 0 {
            b.push(' ');
        }
        b.push_str(&format!("#{n}"));
        if b.len() > room {
            return Line::new();
        }
    }
    counts.spans.push(plain("   "));
    counts.spans.push(dim(b));
    counts
}

/// The folders the session says its paths from: its host's folder and
/// its agent's, each with its symlinks resolved as well, those resolved
/// off the UI thread (Session.bases, model.go:970).
fn bases(m: &mut Model, dirs: &[String]) -> Vec<String> {
    let dirs: Vec<String> = dirs.iter().filter(|d| !d.is_empty()).cloned().collect();
    let real = real_paths(m, &dirs);
    let mut out = Vec::with_capacity(4);
    for (d, r) in dirs.into_iter().zip(real) {
        let same = d == r;
        out.push(d);
        if !same {
            out.push(r);
        }
    }
    out
}

/// path as the session would say it: inside one of its folders, from
/// there; else as it reads best (drawer.rel, render.go:2560).
fn rel(path: &str, bases: &[String]) -> String {
    for b in bases {
        let b = b.trim_end_matches('/');
        if path == b {
            return ".".into();
        }
        if let Some(r) = path.strip_prefix(b).and_then(|r| r.strip_prefix('/')) {
            return r.to_string();
        }
    }
    short_abs(path)
}

/// A path outside the session as it reads best: home as ~, and the
/// middle of a long one left out so its file's name still shows
/// (shortAbs, render.go:2571).
fn short_abs(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let p = match p.strip_prefix(&format!("{home}/")) {
        Some(rest) if !home.is_empty() => format!("~/{rest}"),
        _ => p.to_string(),
    };
    let parts: Vec<&str> = p.split('/').collect();
    if p.len() <= 48 || parts.len() <= 4 {
        return p;
    }
    let n = parts.len();
    [parts[0], parts[1], "…", parts[n - 2], parts[n - 1]].join("/")
}

/// name made to open the file at path when clicked, a relative path taken
/// from dir; name as it is when there's no telling where it is
/// (drawer.fileLink, render.go:2007).
fn file_link(path: &str, dir: &str, mut name: rush_core::text::Span) -> rush_core::text::Span {
    let abs = if path.is_empty() || path.starts_with('/') || path.starts_with("~/") {
        path.to_string()
    } else if dir.is_empty() {
        String::new()
    } else {
        format!("{}/{path}", dir.trim_end_matches('/'))
    };
    if let Some(u) = rush_convo::render::link_target(&abs) {
        name.style = name.style.linked(&u);
    }
    name
}

fn plus() -> rush_core::text::Span {
    spb("+", GREEN)
}
fn minus() -> rush_core::text::Span {
    spb("−", RED)
}

/// The Changes view's rows, w wide (ChangesView).
pub fn lines(m: &mut Model, key: &str, w: usize) -> Vec<(Line, String)> {
    let Some(v) = m.session.as_ref() else {
        return Vec::new();
    };
    if v.sess.partial {
        return vec![
            (Line::new(), String::new()),
            (line([plain("  "), dim("counting…")]), String::new()),
        ];
    }
    let sel = v.sel.clone();
    let verbose = v.verbose;
    let dir = open_dir(&v.sess);
    let dirs = [v.sess.info.cwd.clone(), v.sess.cwd.clone()];
    let bases = bases(m, &dirs);
    // Its own files are on Overview and Memory.
    let (chg, own) = project_changes(m);
    let (open, marks) = {
        let c = conn(m, key);
        (c.vs.open.clone(), c.vs.marks.clone())
    };
    let mut out: Vec<(Line, String)> = Vec::new();
    let add = |out: &mut Vec<(Line, String)>, r: &str, left: Line, right: Line| {
        let left = if !r.is_empty() && r == sel {
            // The keyboard's mark in the gutter.
            let mut l = line([sp("▏", SUB)]);
            let mut first = true;
            for s in left.spans {
                if first {
                    first = false;
                    if let Some(t) = s.text.strip_prefix(' ') {
                        l.spans
                            .push(rush_core::text::Span::new(t.to_string(), s.style));
                        continue;
                    }
                }
                l.spans.push(s);
            }
            l
        } else {
            left
        };
        out.push((row(None, left, right, w, w), r.to_string()));
    };
    let rule = |out: &mut Vec<(Line, String)>, title: &str, meta: &str| {
        if !out.is_empty() {
            out.push((row(None, Line::new(), Line::new(), w, w), String::new()));
        }
        out.push((
            row(None, section(title, meta, w, 3), Line::new(), w, w),
            String::new(),
        ));
    };
    let (adds, dels) = change_stat(&chg);
    let mut meta = "nothing edited yet".to_string();
    if !chg.is_empty() {
        let seen = chg
            .iter()
            .filter(|f| marks.get(&f.path).copied().unwrap_or(false))
            .count();
        let mut shell = String::new();
        let n = shell_only(&chg);
        if n > 0 {
            shell = format!(" · {n} by shell, their lines not counted");
        }
        meta = format!(
            "{} · +{adds} −{dels}{shell}{}",
            plural(chg.len(), "file"),
            rush_convo::render::key_word(&format!(
                " · {seen} of {} reviewed · alt+r marks one",
                chg.len()
            ))
        );
    }
    if !own.is_empty() {
        meta.push_str(&format!(
            " · {} outside the project on Overview",
            plural(own.len(), "file")
        ));
    }
    rule(&mut out, "This session", &meta);
    let bw = w.saturating_sub(16);
    for fc in chg.iter() {
        let r = format!("chg:{}", fc.path);
        let is_open = open.get(&r).copied().unwrap_or(false);
        let arrow = faint(if is_open { "▾" } else { "▸" });
        let mut counts = if fc.new {
            line([sp(format!("new · {} lines", fc.add), GREEN)])
        } else if fc.add + fc.del == 0 && !fc.shell.is_empty() {
            line([dim("by shell")]) // what it changed isn't known
        } else {
            let mut l = line([
                sp(format!("+{}", fc.add), GREEN),
                plain(" "),
                sp(format!("−{}", fc.del), RED),
            ]);
            if !fc.shell.is_empty() {
                l.spans.push(dim(" · shell"));
            }
            l
        };
        match fc.by.len() {
            0 => {}
            1 => counts.spans.push(dim(format!(" · {}", fc.by[0]))),
            n => counts.spans.push(dim(format!(" · {n} subagents"))),
        }
        let marked = marks.get(&fc.path).copied().unwrap_or(false);
        let (mark, name) = if marked {
            (sp("✓ ", GREEN), dim(rel(&fc.path, &bases)))
        } else {
            (plain("  "), text(rel(&fc.path, &bases)))
        };
        let name = file_link(&fc.path, &dir, name);
        add(
            &mut out,
            &r,
            line([plain("  "), arrow, plain(" "), mark, name]),
            turns_right(counts, &fc.turns, w),
        );
        if !is_open {
            continue;
        }
        let pad = "       ";
        // Each hunk under a row naming the turn that made it; enter on it goes
        // to that step in the conversation.
        let hunk = |out: &mut Vec<(Line, String)>, from: &str, what: &str| {
            if from.is_empty() {
                // A subagent's: its step is in its own conversation.
                add(
                    out,
                    "",
                    line([
                        plain(pad),
                        sp("@ ", BLUE),
                        dim(what.to_string()),
                        plain("  "),
                        sp("by a subagent", SUB),
                    ]),
                    Line::new(),
                );
                return;
            }
            let turn = from.split(':').next().unwrap_or("");
            add(
                out,
                &format!("jump:{from}"),
                line([
                    plain(pad),
                    sp("@ ", BLUE),
                    dim(what.to_string()),
                    plain("  "),
                    sp(format!("#{}", turn.trim_start_matches('t')), SUB),
                ]),
                line([dim("enter goes to the step")]),
            );
        };
        for (i, from) in fc.shell.iter().enumerate() {
            let cmd = fc.cmds.get(i).map_or("", String::as_str);
            let cmd = rush_draw::truncate(cmd, 10.max(bw.saturating_sub(30)), "…");
            hunk(&mut out, from, &format!("shell · {cmd}"));
        }
        if fc.new {
            hunk(&mut out, &fc.new_from, "created");
            for (i, l) in fc.content.trim_end_matches('\n').split('\n').enumerate() {
                if i >= 40 && !verbose {
                    add(
                        &mut out,
                        "",
                        line([plain(pad), dim("… ctrl+o shows the rest")]),
                        Line::new(),
                    );
                    break;
                }
                add(
                    &mut out,
                    "",
                    line([
                        plain(pad),
                        sp("▏", GREEN),
                        dim(format!("{:5} ", i + 1)),
                        sp(fit_str(l, bw).trim_end().to_string(), SUB),
                    ]),
                    Line::new(),
                );
            }
        }
        for (pi, p) in fc.patches.iter().enumerate() {
            let from = fc.from.get(pi).map_or("", String::as_str);
            hunk(&mut out, from, &format!("line {}", p.new_start));
            let (mut old_n, mut new_n) = (p.old_start, p.new_start);
            for l in &p.lines {
                let l = if l.is_empty() { " " } else { l.as_str() };
                if l.starts_with('\\') {
                    continue; // "\ No newline at end of file"
                }
                let body = fit_str(&l[1..], bw).trim_end().to_string();
                let (bg, n, sign, ink) = match l.as_bytes()[0] {
                    b'+' => {
                        new_n += 1;
                        (Some(BG_ADD), new_n - 1, plus(), TEXT)
                    }
                    b'-' => {
                        old_n += 1;
                        (Some(BG_DEL), old_n - 1, minus(), TEXT)
                    }
                    _ => {
                        old_n += 1;
                        new_n += 1;
                        (None, new_n - 1, plain(" "), SUB)
                    }
                };
                let left = line([
                    plain(pad),
                    dim(format!("{n:5} ")),
                    sign,
                    plain(" "),
                    sp(body, ink),
                ]);
                out.push((row(bg, left, Line::new(), w, w), String::new()));
            }
        }
    }
    if dir.is_empty() {
        return out;
    }
    let tree = tree_of(m, &dir);
    if !tree.err.is_empty() {
        rule(&mut out, "Working tree", &tree.err);
        return out;
    }
    let mine: Vec<String> = chg.iter().map(|f| f.path.clone()).collect();
    let real = real_paths(m, &mine);
    let ours: HashSet<&str> = mine.iter().chain(real.iter()).map(|p| p.as_str()).collect();
    let others = tree
        .files
        .iter()
        .filter(|f| !ours.contains(f.path.as_str()))
        .count();
    let mut meta = format!(
        "{} changed against {}",
        plural(tree.files.len(), "file"),
        tree.head
    );
    if others > 0 {
        meta.push_str(&format!(" · {others} not from this session"));
    }
    rule(&mut out, "Working tree", &meta);
    if tree.files.is_empty() {
        add(
            &mut out,
            "",
            line([plain("    "), dim("clean")]),
            Line::new(),
        );
    }
    for (i, f) in tree.files.iter().enumerate() {
        if i >= 60 && !verbose {
            add(
                &mut out,
                "",
                line([
                    plain("    "),
                    dim(format!(
                        "… {} more · ctrl+o shows all",
                        tree.files.len() - i
                    )),
                ]),
                Line::new(),
            );
            break;
        }
        let mut counts = if f.untracked {
            line([sp("new", GREEN)])
        } else if f.binary {
            line([dim("binary")])
        } else {
            line([
                sp(format!("+{}", f.add), GREEN),
                plain(" "),
                sp(format!("−{}", f.del), RED),
            ])
        };
        let is_ours = ours.contains(f.path.as_str());
        let (who, mark) = if is_ours {
            (sp("this session", ORANGE), sp("●", ORANGE))
        } else {
            (dim("not from this session"), dim("·"))
        };
        counts.spans.push(plain("   "));
        counts.spans.push(who);
        let r = format!("tree:{}", f.path);
        let is_open = open.get(&r).copied().unwrap_or(false);
        let arrow = faint(if is_open { "▾" } else { "▸" });
        add(
            &mut out,
            &r,
            line([
                plain("  "),
                arrow,
                plain(" "),
                mark,
                plain(" "),
                file_link(&f.path, &dir, text(rel(&f.path, &bases))),
            ]),
            counts,
        );
        if is_open {
            for l in tree_diff(m, &tree, f) {
                if l.starts_with("@@") {
                    let t = rush_draw::truncate(&l, w.saturating_sub(10), "…").into_owned();
                    out.push((
                        row(
                            None,
                            line([plain("       "), sp(t, BLUE)]),
                            Line::new(),
                            w,
                            w,
                        ),
                        String::new(),
                    ));
                    continue;
                }
                let (bg, sign) = if l.starts_with('+') {
                    (Some(BG_ADD), plus())
                } else if l.starts_with('-') {
                    (Some(BG_DEL), minus())
                } else {
                    (None, plain(" "))
                };
                let body = l
                    .strip_prefix(['+', '-', ' '])
                    .unwrap_or(&l)
                    .replace('\t', "    ");
                let left = line([
                    plain("       "),
                    sign,
                    plain(" "),
                    text(fit_str(&body, w.saturating_sub(12)).trim_end().to_string()),
                ]);
                out.push((row(bg, left, Line::new(), w, w), String::new()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lines_drop_headers() {
        let d = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n";
        assert_eq!(diff_lines(d), vec!["@@ -1 +1 @@", "-a", "+b"]);
    }

    #[test]
    fn turns_leave_the_row_when_they_crowd_it() {
        let c = line([plain("+1 −0")]);
        assert_eq!(turns_right(c.clone(), &[3, 4], 80).plain(), "+1 −0   #3 #4");
        let many: Vec<u32> = (1..60).collect();
        assert_eq!(turns_right(c, &many, 40).plain(), "");
    }

    // render.go:2560 rel, :2571 shortAbs: from the first folder it's in,
    // its symlinks resolved too; else home as ~ and a long one's middle
    // left out.
    #[test]
    fn rel_paths() {
        let b = ["/tmp/w".to_string(), "/private/tmp/w".to_string()];
        let home = std::env::var("HOME").unwrap();
        for (p, want) in [
            ("/tmp/w/src/a.rs".to_string(), "src/a.rs"),
            ("/private/tmp/w/src/a.rs".into(), "src/a.rs"),
            ("/tmp/w".into(), "."),
            ("/tmp/wx/a.rs".into(), "/tmp/wx/a.rs"),
            ("/x/a.rs".into(), "/x/a.rs"),
            (format!("{home}/n/a.rs"), "~/n/a.rs"),
            (
                "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w.rs".into(),
                "/a/…/v/w.rs",
            ),
        ] {
            assert_eq!(rel(&p, &b), want, "{p}");
        }
    }

    // render.go:2007 fileLink: a path, made absolute from the folder,
    // opens as a file:// URL; one with nowhere to be from isn't linked.
    #[test]
    fn file_rows_link() {
        let l = file_link("src/a b.rs", "/w", text("src/a b.rs"));
        assert_eq!(
            l.style.link.url().as_deref(),
            Some("file:///w/src/a%20b.rs")
        );
        let l = file_link("/x/a.rs", "", text("a.rs"));
        assert_eq!(l.style.link.url().as_deref(), Some("file:///x/a.rs"));
        assert_eq!(
            file_link("a.rs", "", text("a.rs"))
                .style
                .link
                .url()
                .as_deref(),
            None
        );
    }
}
