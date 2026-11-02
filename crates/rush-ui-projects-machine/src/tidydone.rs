//! The automatic clean-up of done work, and of work that landed: what goes
//! by itself, in the rush that leads only. Done work untouched for
//! Settings' Clean up done work after has its worktree removed, when the
//! one rule says it can go (rush-fleet's verdict: every change committed,
//! and pushed or on its target), and its temp work emptied; one that would
//! lose work is kept, and said once. A session whose work has landed (its
//! pull request merged, or its branch on its target) is put away in
//! Earlier, and its worktree goes with its branch by the same rule
//! (rush-fleet's settle). Nothing here removes what git hasn't said is
//! safe: rush-fleet looks again with git just before each removal.
//!
//! Go: internal/ui/cleanup.go (tidyDone :236, keepFinished :306, tidyable
//! :316, onTidied :406-436, onCleanedQuiet :440, settleLanded :741-780,
//! stirring :788, onLanded :800-830, sessionGone :836, openPR :842,
//! worktreeName :852, worktreeRepo :868) and model.go:2626 (earlier).
//! Go looks every 600 ticks (tidyDone) and 60 (settleLanded); so does
//! this, on the tick in chores.rs, which wakes for nothing else.

use std::collections::{HashMap, HashSet};

use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_fleet::{Predicates, Worktree};

use super::data::*;
use super::rows::disk;
use crate::frame::Out;
use crate::frame::messages::NEWS_REF_PROJECTS;
use crate::model::Model;
use crate::style::one_line;

/// How many finished worktrees of rush's each repository keeps, newest
/// first, however long ago they were done.
pub const KEEP_FINISHED: usize = 3;
/// How long a worktree kept, or a session's that stayed, waits before
/// it's looked at again.
const LOOK_AGAIN: Ms = 3_600_000;

/// What the clean-up of done work remembers (cleanup's kept, settling,
/// settled, stayed, markedLanded).
#[derive(Default)]
pub struct Tidy {
    /// A look still out.
    pub tidying: bool,
    pub settling: bool,
    /// Done worktrees found unsafe, and when: said once, looked at again
    /// after an hour.
    pub kept: HashMap<String, Ms>,
    /// Sessions whose work landed, marked done once, so one you move back
    /// stays back.
    pub settled: HashSet<String>,
    /// Sessions whose worktree stayed (work it would lose), and when.
    pub stayed: HashMap<String, Ms>,
    pub marked_landed: HashSet<String>,
    /// Sessions put away as landed whose host still runs: their worktree
    /// and branch wait until it has stopped or gone to sleep, and are kept
    /// if you send the session something first.
    pub waiting: HashSet<String>,
}

/// Whether a is Earlier's: put away, or left a day without a turn and
/// asking nothing (model.go:2626 earlier).
fn earlier_of(d: &Data, a: &Agent, now: Ms) -> bool {
    a.done || earlier(d, a, now)
}

/// The worktrees the automatic clean-up may try: rush's own (one not made
/// by rush only ever goes by hand), each of whose sessions is due, or
/// whose session is gone (as finished long ago), past the newest
/// KEEP_FINISHED finished ones in its repository. tidy_worktree then asks
/// the rule of each (tidyable).
pub fn tidyable(
    wts: &[Worktree],
    due: &HashMap<String, bool>,
    finished: &HashMap<String, Ms>,
    gone: &dyn Fn(&Worktree) -> bool,
) -> Vec<usize> {
    let mut by_repo: HashMap<&str, Vec<(usize, Ms)>> = HashMap::new();
    for (i, w) in wts.iter().enumerate() {
        if !w.own {
            continue;
        }
        if w.agents.is_empty() {
            if gone(w) {
                // Finished long ago: the first to go.
                by_repo.entry(&w.repo).or_default().push((i, 0));
            }
            continue;
        }
        let mut last = 0;
        let mut done = true;
        for k in &w.agents {
            match finished.get(k) {
                Some(&t) => last = last.max(t),
                None => done = false,
            }
        }
        if done {
            by_repo.entry(&w.repo).or_default().push((i, last));
        }
    }
    let mut out = Vec::new();
    for (_, mut fs) in by_repo {
        fs.sort_by_key(|&(_, last)| std::cmp::Reverse(last));
        for &(i, _) in fs.iter().skip(KEEP_FINISHED) {
            if wts[i].agents.iter().all(|k| due.get(k) == Some(&true)) {
                out.push(i);
            }
        }
    }
    out.sort_unstable();
    out
}

/// A worktree of rush's whose session is no more: rush names each after
/// its session, and the session's folder is gone (sessionGone).
fn session_gone(w: &Worktree) -> bool {
    let base = std::path::Path::new(&w.path)
        .file_name()
        .map(|b| b.to_os_string())
        .unwrap_or_default();
    std::fs::metadata(rush_fleet::host_root().join(base))
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// Whether the host of a session is still running it: its process alive
/// and not stopped or asleep (info.json), or coming back (lost). One rush
/// can't read is taken as running, so its worktree stays. A session rush
/// doesn't host has only its process (pid) to say.
pub fn host_running(id: &str, pid: i32) -> bool {
    if id.is_empty() {
        return pid != 0 && rush_host::alive(i64::from(pid));
    }
    match rush_host::read_info(id) {
        // A host publishes "stopped" just before it exits: its process
        // has to be gone too.
        Ok(i) => i.lost || !(i.sleeping || i.state == "stopped" && !rush_host::alive(i.host_pid)),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// What the clean-up of done work takes off the UI thread.
pub struct TidyPlan {
    agents: Vec<Agent>,
    /// Each session's host (its id, else "", and pid), by key.
    hosts: HashMap<String, (String, i32)>,
    due: HashMap<String, bool>,
    finished: HashMap<String, Ms>,
    kept: HashSet<String>,
    temp_due: Vec<Agent>,
    stale: Vec<Agent>,
    after: Ms,
    now: Ms,
}

/// What the clean-up of done work did (tidiedMsg).
#[derive(Default)]
pub struct Tidied {
    pub wts: Vec<Worktree>,
    pub gone: Vec<String>,
    /// Worktrees kept, by path, and why.
    pub kept: Vec<(String, String)>,
    pub temp: HashMap<String, super::temp::TempSize>,
    pub freed: i64,
    pub failed: Option<String>,
}

/// What's due of done work now, or None: a look still out, the rush
/// follows, or the clean-up is off (tidyDone's choice, on the UI thread).
pub fn tidy_plan(m: &mut Model, now: Ms) -> Option<TidyPlan> {
    if !m.leads() {
        return None;
    }
    let after = data(m).cfg.cleanup_after();
    if m.kept::<Tidy>().tidying || after == 0 {
        return None;
    }
    let d = data(m);
    let mut temp_due = Vec::new();
    let mut stale = Vec::new();
    let mut due = HashMap::new();
    let mut finished = HashMap::new();
    let mut hosts = HashMap::new();
    for g in agents(m) {
        let a = g.a;
        let is_due = due_in(m, d, std::slice::from_ref(&a.key), now) == Some(0);
        if a.temp >= TEMP_SHOWN && is_due {
            temp_due.push(a.clone());
        } else if a.rush && a.pid == 0 && untouched(d, &g, now) >= after {
            // Stopped, done or not: the tmp folder rush gave it goes.
            stale.push(a.clone());
        }
        due.insert(a.key.clone(), is_due);
        let id = if a.rush {
            a.job.id.clone()
        } else {
            String::new()
        };
        hosts.insert(a.key.clone(), (id, a.pid));
        if earlier_of(d, a, now) && !a.in_use() {
            finished.insert(a.key.clone(), now - untouched(d, &g, now));
        }
    }
    let t = m.kept::<Tidy>();
    let kept = t
        .kept
        .iter()
        .filter(|&(_, &at)| now - at < LOOK_AGAIN)
        .map(|(p, _)| p.clone())
        .collect();
    t.tidying = true;
    Some(TidyPlan {
        agents: m.agents.clone(),
        hosts,
        due,
        finished,
        kept,
        temp_due,
        stale,
        after,
        now,
    })
}

/// The clean-up of done work, off the UI thread: each worktree the rule
/// lets go removed (rush-fleet looks again with git first), the rest kept
/// with why; done agents' temp work emptied, and measured after.
pub fn tidy(reg: &rush_core::adapter::Registry, p: TidyPlan) -> Tidied {
    let mut out = Tidied {
        wts: rush_fleet::find_worktrees(&p.agents, Some(rush_gh::ghpr::Gh::shared())),
        ..Tidied::default()
    };
    for i in tidyable(&out.wts, &p.due, &p.finished, &session_gone) {
        let w = &out.wts[i];
        if p.kept.contains(&w.path) {
            continue;
        }
        // Put away is not stopped: a session still running keeps its
        // folder until its host has stopped or gone to sleep.
        if w.agents.iter().any(|k| {
            p.hosts
                .get(k)
                .is_some_and(|(id, pid)| host_running(id, *pid))
        }) {
            continue;
        }
        match rush_fleet::tidy_worktree(w) {
            Ok(size) => {
                out.gone.push(w.path.clone());
                out.freed += size;
            }
            // A dry run wrote it down: neither gone nor kept.
            Err(e) if rush_fleet::dry_run_err(&e) => {}
            Err(e) => out.kept.push((w.path.clone(), e)),
        }
    }
    let measure = |a: &Agent| {
        let at = rush_core::time::now();
        let start = std::time::Instant::now();
        let bytes = rush_fleet::disk_usage(&rush_fleet::temp_dirs(reg, a));
        super::temp::TempSize {
            bytes,
            at,
            took: start.elapsed(),
        }
    };
    for a in &p.temp_due {
        if let Err(e) = rush_fleet::clean_temp(reg, a)
            && out.failed.is_none()
        {
            out.failed = Some(e);
        }
        let s = measure(a);
        out.freed += a.temp - s.bytes;
        out.temp.insert(a.key.clone(), s);
    }
    for a in &p.stale {
        match rush_fleet::clean_stale_temp(a, p.after, p.now) {
            Err(e) => {
                if out.failed.is_none() {
                    out.failed = Some(e);
                }
            }
            Ok(false) => {}
            Ok(true) => {
                let s = measure(a);
                out.freed += (a.temp - s.bytes).max(0);
                out.temp.insert(a.key.clone(), s);
            }
        }
    }
    out
}

/// Whether the chores below may run: with a loop, the list read and
/// Settings' config with it.
fn may_tidy(m: &mut Model) -> bool {
    m.sender().is_some() && !data(m).frozen && data(m).cfg_read
}

/// Every 10 minutes of the tick (chores.rs), in the rush that leads:
/// done work's clean-up (tidyDone).
pub fn tidy_tick(m: &mut Model) {
    if !may_tidy(m) {
        return;
    }
    if let Some(p) = tidy_plan(m, rush_core::time::now()) {
        let reg = m.reg.clone();
        m.later(move || tidy(&reg, p), on_tidied);
    }
}

/// Every minute of the tick (chores.rs), in the rush that leads: the look
/// for work that landed (settleLanded).
pub fn settle_tick(m: &mut Model) {
    if !may_tidy(m) {
        return;
    }
    if let Some(looks) = settle_looks(m, rush_core::time::now()) {
        m.later(move || settle(looks), on_landed);
    }
}

/// A worktree by the session working in it, quoted, or by its branch, or
/// its folder's name when rush knows neither (worktreeName).
pub fn worktree_name(m: &Model, wts: &[Worktree], path: &str) -> String {
    for a in &m.agents {
        if a.workspace.as_ref().is_some_and(|w| w.path == path)
            || a.repo == path && !a.root.is_empty() && a.root != path
        {
            return format!("“{}”", one_line(&a.display_name));
        }
    }
    if let Some(w) = wts.iter().find(|w| w.path == path && !w.branch.is_empty()) {
        return w.branch.clone();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The main checkout a worktree belongs to, "" if unknown (worktreeRepo).
fn worktree_repo(m: &Model, wts: &[Worktree], path: &str) -> String {
    if let Some(w) = wts.iter().find(|w| w.path == path) {
        return w.repo.clone();
    }
    m.agents
        .iter()
        .filter_map(|a| a.workspace.as_ref())
        .find(|w| w.path == path)
        .map(|w| w.repo.clone())
        .unwrap_or_default()
}

/// What the clean-up of done work did, taken in and said (onTidied).
pub fn on_tidied(m: &mut Model, t: Tidied) -> Out {
    let now = m.now;
    m.kept::<Tidy>().tidying = false;
    let gone: HashSet<&String> = t.gone.iter().collect();
    let wts: Vec<Worktree> = t
        .wts
        .iter()
        .filter(|w| !gone.contains(&w.path))
        .cloned()
        .collect();
    for (p, why) in &t.kept {
        let said = m.kept::<Tidy>().kept.contains_key(p);
        if !said {
            let base = std::path::Path::new(p)
                .file_name()
                .map(|b| b.to_string_lossy().into_owned())
                .unwrap_or_default();
            let why = why
                .strip_prefix(&format!("{base} isn't safe to remove: "))
                .unwrap_or(why);
            let name = worktree_name(m, &wts, p);
            let repo = worktree_repo(m, &wts, p);
            m.news(
                &format!("kept {name}'s worktree: {why} · open it in Projects"),
                &format!("{NEWS_REF_PROJECTS}{repo}"),
            );
        }
        m.kept::<Tidy>().kept.insert(p.clone(), now);
    }
    if !t.temp.is_empty() {
        for a in m.agents.iter_mut() {
            if let Some(s) = t.temp.get(&a.key) {
                a.temp = s.bytes;
            }
        }
        let st = m.kept::<Data>();
        for (k, s) in &t.temp {
            if let Some(w) = st.temp.get_mut(k) {
                (w.bytes, w.known) = (s.bytes, true);
            }
        }
        st.temp_sizes.set(t.temp.clone());
        m.rebuild();
    }
    let after = data(m).cfg.cleanup_after();
    if !t.gone.is_empty() || !t.temp.is_empty() || !t.kept.is_empty() || t.failed.is_some() {
        rush_sys::events::log(
            "tidy",
            serde_json::json!({
                "worktreesRemoved": t.gone,
                "worktreesKept": t.kept.iter().cloned().collect::<HashMap<_, _>>(),
                "tempCleaned": t.temp.len(), "freed": t.freed,
                "why": format!("done work untouched for {}", go_duration(after)),
                "err": t.failed.clone().unwrap_or_default(),
            }),
        );
    }
    if !t.gone.is_empty() {
        let names: Vec<String> = t.gone.iter().map(|p| worktree_name(m, &t.wts, p)).collect();
        m.news(
            &format!(
                "removed the worktrees of done work: {} · freed {}, nothing lost, branches kept",
                names.join(", "),
                disk(t.freed)
            ),
            "",
        );
    } else if t.freed >= TEMP_SHOWN {
        m.news(
            &format!("cleaned up done agents' temp work: freed {}", disk(t.freed)),
            "",
        );
    }
    if let Some(e) = &t.failed {
        m.news(&format!("couldn't clean up done work: {e}"), "");
    }
    m.kept::<Data>().wts = wts;
    Out::Draw
}

/// A whole-second time as Go's Duration.String says it: "3h0m0s".
fn go_duration(ms: Ms) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}h{m}m{s}s")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

/// One session whose work may have landed, to look at with git.
pub struct Look {
    pub key: String,
    pub name: String,
    /// The rush session's id, for the pull requests it follows.
    pub id: String,
    pub pid: i32,
    pub w: Worktree,
}

/// Whether any pull request a links is still open or a draft (openPR).
fn open_pr(a: &Agent) -> bool {
    a.prs
        .iter()
        .any(|p| p.state == "OPEN" || p.state == "DRAFT")
}

/// The sessions to look at for work that landed, or None: a look still
/// out, or the rush follows. One at work, a peer's,
/// one asking you something, one with a pull request still open, or one
/// settled already or that stayed within the hour is passed over
/// (settleLanded).
pub fn settle_looks(m: &mut Model, now: Ms) -> Option<Vec<Look>> {
    if !m.leads() {
        return None;
    }
    if m.kept::<Tidy>().settling {
        return None;
    }
    m.kept.peek::<Tidy>()?;
    // A message brought it back from put away: its worktree is kept.
    let back: Vec<String> = m
        .agents
        .iter()
        .filter(|a| !a.done)
        .map(|a| a.key.clone())
        .collect();
    for k in &back {
        m.kept::<Tidy>().waiting.remove(k);
    }
    let t = m.kept.peek::<Tidy>()?;
    let mut looks = Vec::new();
    for a in &m.agents {
        if a.job.live()
            || a.busy()
            || !a.peer.is_empty()
            || t.settled.contains(&a.key) && !t.waiting.contains(&a.key)
            || t.stayed
                .get(&a.key)
                .is_some_and(|&at| now - at < LOOK_AGAIN)
        {
            continue;
        }
        if crate::list::asks(a) {
            continue; // a question still open isn't finished, merged or not
        }
        if open_pr(a) {
            continue; // another of its pull requests is still open
        }
        let mut w = Worktree::default();
        if let Some(ws) = &a.workspace {
            w.path.clone_from(&ws.path);
            w.repo.clone_from(&ws.repo);
            w.branch.clone_from(&ws.branch);
            w.target.clone_from(&ws.target);
            w.base.clone_from(&ws.base);
            w.own = ws.own;
        } else if a
            .pr
            .as_ref()
            .is_some_and(|p| p.merged() && !p.head_oid.is_empty())
            && !a.root.is_empty()
            && a.repo != a.root
        {
            // One in a worktree of its own making, known by its merged
            // pull request alone: git isn't asked about the others.
            w.path.clone_from(&a.repo);
            w.repo.clone_from(&a.root);
            w.branch.clone_from(&a.branch);
        } else {
            continue;
        }
        w.pr = a.pr.as_deref().cloned();
        w.agents = vec![a.key.clone()];
        looks.push(Look {
            key: a.key.clone(),
            name: one_line(&a.display_name),
            id: if a.rush {
                a.job.id.clone()
            } else {
                String::new()
            },
            pid: a.pid,
            w,
        });
    }
    if looks.is_empty() {
        return None;
    }
    m.kept::<Tidy>().settling = true;
    Some(looks)
}

/// What the look for work that landed found (landedMsg).
#[derive(Default)]
pub struct Landed {
    /// Agent key → why it's finished.
    pub done: Vec<(String, String)>,
    pub did: Vec<String>,
    /// Agent key → why its worktree stays.
    pub stays: HashMap<String, String>,
    /// Agent key → its worktree's path and branch, for a landed session
    /// whose host still runs: put away, its worktree left for now.
    pub waits: HashMap<String, (String, String)>,
}

/// A rush session come to life since the list was read: in a turn, with
/// work of its own running, or something queued. Its worktree is looked at
/// again later; one rush can't read is left too (stirring).
fn stirring(id: &str) -> bool {
    match rush_host::read_info(id) {
        Ok(i) => {
            i.state != "idle" && i.state != "stopped"
                || !i.background.is_empty()
                || !i.queue.is_empty()
        }
        Err(_) => true,
    }
}

/// Each look's worktree asked whether its work landed, the cheap way
/// first; one that has is settled by the rule: removed if rush made it and
/// it loses nothing, its branch deleted, main brought up to date. Off the
/// UI thread (settleLanded's work).
pub fn settle(looks: Vec<Look>) -> Landed {
    settle_with(looks, &|l| host_running(&l.id, l.pid))
}

/// settle, with running saying whether a look's session is still run by
/// its host: such a session's work has landed, but its worktree and
/// branch wait until it has stopped or gone to sleep.
pub fn settle_with(mut looks: Vec<Look>, running: &dyn Fn(&Look) -> bool) -> Landed {
    let mut out = Landed::default();
    for l in &mut looks {
        if !std::fs::metadata(&l.w.path).is_ok_and(|s| s.is_dir()) {
            continue; // gone already
        }
        if !l.id.is_empty() && !rush_prdrive::follow::following(&l.id).is_empty() {
            continue; // still driving a pull request
        }
        let (ok, why) = l.w.landing();
        if !ok || !l.id.is_empty() && stirring(&l.id) {
            continue;
        }
        out.done.push((l.key.clone(), why));
        if running(l) {
            out.waits
                .insert(l.key.clone(), (l.w.path.clone(), l.w.branch.clone()));
            continue;
        }
        match rush_fleet::settle(&l.w) {
            Err(e) => {
                out.stays.insert(l.key.clone(), e);
            }
            Ok(did) if !did.is_empty() => {
                out.did.push(format!("{}: {}", l.name, did.join(", ")));
            }
            Ok(_) => {}
        }
    }
    out
}

/// Sessions whose work landed: each put away in Earlier once, so one you
/// bring back stays back; a worktree that stayed is said once (onLanded).
pub fn on_landed(m: &mut Model, l: Landed) -> Out {
    let now = m.now;
    m.kept::<Tidy>().settling = false;
    let mut marked = 0;
    let mut stays: Vec<(String, String)> = Vec::new();
    for (k, why) in &l.done {
        if let Some((path, branch)) = l.waits.get(k) {
            m.kept::<Tidy>().settled.insert(k.clone());
            if m.kept::<Tidy>().waiting.insert(k.clone()) {
                rush_sys::events::log(
                    "worktree-kept",
                    serde_json::json!({
                        "agent": k, "path": path, "branch": branch,
                        "why": "its session is still running",
                    }),
                );
            }
        } else if let Some(s) = l.stays.get(k) {
            if !m.kept::<Tidy>().stayed.contains_key(k) {
                let (mut name, mut repo, mut s) =
                    ("a done session".to_string(), String::new(), s.as_str());
                if let Some(i) = m.agents.iter().position(|a| a.key == *k) {
                    let a = &m.agents[i];
                    name = format!("“{}”", one_line(&a.display_name));
                    repo = crate::list::folder_key(a, m.roots.get(i).map_or("", String::as_str))
                        .to_string();
                    if let Some((_, r)) = s.split_once(" isn't safe to remove: ") {
                        s = r;
                    }
                }
                stays.push((
                    format!("kept {name}'s worktree: {s} · open it in Projects"),
                    repo,
                ));
            }
            m.kept::<Tidy>().stayed.insert(k.clone(), now);
        } else {
            m.kept::<Tidy>().waiting.remove(k);
            m.kept::<Tidy>().settled.insert(k.clone());
        }
        let done = m.agents.iter().any(|a| a.key == *k && a.done);
        if !done
            && !rush_core::dryrun::would("put-away-landed", k, &format!("its work landed: {why}"))
            && m.kept::<Tidy>().marked_landed.insert(k.clone())
        {
            m.act(crate::frame::ext::Act::ToggleDone(k.clone()));
            marked += 1;
            rush_sys::events::log("landed-done", serde_json::json!({"agent": k, "why": why}));
        }
    }
    if !l.did.is_empty() {
        m.news(&format!("work landed: {}", l.did.join(" · ")), "");
    } else if marked > 0 {
        m.news(&format!("work landed: {marked} put away in Earlier"), "");
    }
    for (text, repo) in stays {
        m.news(&text, &format!("{NEWS_REF_PROJECTS}{repo}"));
    }
    Out::Draw
}
