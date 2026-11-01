//! The session's own edits by file, worked out from its conversation:
//! its Edit and Write calls, the files its shell commands plainly wrote,
//! its subagents' edits marked with their names. The worker that reads a
//! conversation works them out (worker.rs); the Changes view draws them
//! (screens/session_core/changes.rs). After Go's internal/convo/changes.go
//! (FileChange :20, Changes :49, mergeChanges :67, changesNow :96, by
//! :203, shellOnly :211, inside :247), internal/ui/ownfiles.go (changeStat
//! :31) and spawn.go (SetChildren :573, shownSteps :614).

use std::collections::{HashMap, HashSet};

use rush_convo::render::Child;
use rush_convo::{Item, Session, Status, StepId};
use rush_core::tool::{Kind, Patch};

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
                    let run = rush_convo::shell_run_kept(&input.command, at);
                    let (writes, gone, end) = (&run.0, &run.1, &run.2);
                    if input.cwd.is_empty() {
                        shell_at = home.clone();
                        if !home.is_empty() && inside(&home, end) {
                            shell_at = end.clone();
                        }
                    }
                    let cmd = input.command.trim().split('\n').next().unwrap_or("");
                    for w in writes.iter() {
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
                    removed.extend(gone.iter().cloned());
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

/// memo_ver, of a session and the agents its steps ran.
pub fn memo_key_of(s: &Session, kids: &Kids) -> Ver {
    let mut ver = s.step_ver();
    for h in &s.helpers {
        ver += h.sess.step_ver() + 1;
    }
    let mut ran = 0;
    for k in kids.values().flatten() {
        if let Some(ks) = &k.sess {
            ver += ks.step_ver() + 1;
            ran += 1;
        }
    }
    (s.serial(), ver, s.helpers.len() + (ran << 32))
}

/// A session (its serial) at a version of its steps and its helpers', and
/// how many helpers it has.
pub type Ver = (u64, u64, usize);
