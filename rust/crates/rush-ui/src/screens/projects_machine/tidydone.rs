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
//! Go looks every 600 ticks (tidyDone) and 60 (settleLanded); here they
//! go with readings of the list, at most every 10 minutes and every
//! minute, so an idle rush never wakes for them.

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
pub(crate) const KEEP_FINISHED: usize = 3;
/// How often done work is looked at, and work that landed.
const TIDY_EVERY: Ms = 10 * 60_000;
const SETTLE_EVERY: Ms = 60_000;
/// How long after rush starts each first looks (Go's ticks 30 and 15).
const TIDY_FIRST: Ms = 30_000;
const SETTLE_FIRST: Ms = 15_000;
/// How long a worktree kept, or a session's that stayed, waits before
/// it's looked at again.
const LOOK_AGAIN: Ms = 3_600_000;

/// What the clean-up of done work remembers (cleanup's kept, settling,
/// settled, stayed, markedLanded).
#[derive(Default)]
pub(crate) struct Tidy {
    /// When this rush first read the list, and when each last looked.
    pub started: Ms,
    pub tidied_at: Ms,
    pub settled_at: Ms,
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
pub(crate) fn tidyable(
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

/// What the clean-up of done work takes off the UI thread.
pub(crate) struct TidyPlan {
    agents: Vec<Agent>,
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
pub(crate) struct Tidied {
    pub wts: Vec<Worktree>,
    pub gone: Vec<String>,
    /// Worktrees kept, by path, and why.
    pub kept: Vec<(String, String)>,
    pub temp: HashMap<String, super::temp::TempSize>,
    pub freed: i64,
    pub failed: Option<String>,
}

/// What's due of done work now, or None: not yet, or the rush follows,
/// or the clean-up is off (tidyDone's choice, on the UI thread).
pub(crate) fn tidy_plan(m: &mut Model, now: Ms) -> Option<TidyPlan> {
    if !m.leads() {
        return None;
    }
    let after = data(m).cfg.cleanup_after();
    let t = m.kept::<Tidy>();
    if t.started == 0 {
        t.started = now;
    }
    let due_at = if t.tidied_at == 0 {
        t.started + TIDY_FIRST
    } else {
        t.tidied_at + TIDY_EVERY
    };
    if t.tidying || after == 0 || now < due_at {
        return None;
    }
    let d = data(m);
    let mut temp_due = Vec::new();
    let mut stale = Vec::new();
    let mut due = HashMap::new();
    let mut finished = HashMap::new();
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
    t.tidied_at = now;
    Some(TidyPlan {
        agents: m.agents.clone(),
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
pub(crate) fn tidy(reg: &rush_core::adapter::Registry, p: TidyPlan) -> Tidied {
    let mut out = Tidied {
        wts: rush_fleet::find_worktrees(&p.agents, Some(rush_gh::ghpr::Gh::shared())),
        ..Tidied::default()
    };
    for i in tidyable(&out.wts, &p.due, &p.finished, &session_gone) {
        let w = &out.wts[i];
        if p.kept.contains(&w.path) {
            continue;
        }
        match rush_fleet::tidy_worktree(w) {
            Ok(size) => {
                out.gone.push(w.path.clone());
                out.freed += size;
            }
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

/// After each reading of the list, with a loop: done work's clean-up and
/// the look for work that landed, when due (tidy's tidyDone and
/// settleLanded).
pub(crate) fn after_fleet(m: &mut Model) {
    if m.sender().is_none() || data(m).frozen || !data(m).cfg_read {
        return;
    }
    let now = m.now;
    if let Some(p) = tidy_plan(m, now) {
        let reg = m.reg.clone();
        m.later(move || tidy(&reg, p), on_tidied);
    }
    if let Some(looks) = settle_looks(m, now) {
        m.later(move || settle(looks), on_landed);
    }
}

/// A worktree by the session working in it, quoted, or by its branch, or
/// its folder's name when rush knows neither (worktreeName).
pub(crate) fn worktree_name(m: &Model, wts: &[Worktree], path: &str) -> String {
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
pub(crate) fn on_tidied(m: &mut Model, t: Tidied) -> Out {
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
                w.bytes = s.bytes;
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
pub(crate) struct Look {
    key: String,
    name: String,
    /// The rush session's id, for the pull requests it follows.
    id: String,
    w: Worktree,
}

/// Whether any pull request a links is still open or a draft (openPR).
fn open_pr(a: &Agent) -> bool {
    a.prs
        .iter()
        .any(|p| p.state == "OPEN" || p.state == "DRAFT")
}

/// The sessions to look at for work that landed, once a minute, or None:
/// not yet, a look still out, or the rush follows. One at work, a peer's,
/// one asking you something, one with a pull request still open, or one
/// settled already or that stayed within the hour is passed over
/// (settleLanded).
pub(crate) fn settle_looks(m: &mut Model, now: Ms) -> Option<Vec<Look>> {
    if !m.leads() {
        return None;
    }
    let t = m.kept::<Tidy>();
    if t.started == 0 {
        t.started = now;
    }
    let due_at = if t.settled_at == 0 {
        t.started + SETTLE_FIRST
    } else {
        t.settled_at + SETTLE_EVERY
    };
    if t.settling || now < due_at {
        return None;
    }
    t.settled_at = now;
    let t = m.kept.peek::<Tidy>()?;
    let mut looks = Vec::new();
    for a in &m.agents {
        if a.job.live()
            || a.busy()
            || !a.peer.is_empty()
            || t.settled.contains(&a.key)
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
        w.pr.clone_from(&a.pr);
        w.agents = vec![a.key.clone()];
        looks.push(Look {
            key: a.key.clone(),
            name: one_line(&a.display_name),
            id: if a.rush {
                a.job.id.clone()
            } else {
                String::new()
            },
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
pub(crate) struct Landed {
    /// Agent key → why it's finished.
    pub done: Vec<(String, String)>,
    pub did: Vec<String>,
    /// Agent key → why its worktree stays.
    pub stays: HashMap<String, String>,
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
pub(crate) fn settle(mut looks: Vec<Look>) -> Landed {
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
pub(crate) fn on_landed(m: &mut Model, l: Landed) -> Out {
    let now = m.now;
    m.kept::<Tidy>().settling = false;
    let mut marked = 0;
    let mut stays: Vec<(String, String)> = Vec::new();
    for (k, why) in &l.done {
        if let Some(s) = l.stays.get(k) {
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
            m.kept::<Tidy>().settled.insert(k.clone());
        }
        let done = m.agents.iter().any(|a| a.key == *k && a.done);
        if !done && m.kept::<Tidy>().marked_landed.insert(k.clone()) {
            crate::screens::cmdbar::stops::toggle_done(m, k);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, repo: &str, own: bool, agents: &[&str]) -> Worktree {
        let mut w = Worktree::default();
        w.path = path.into();
        w.repo = repo.into();
        w.own = own;
        w.agents = agents.iter().map(|s| s.to_string()).collect();
        w
    }

    // cleanup.go:316: only rush's own, all its sessions due, past the
    // newest three finished in its repository; one whose session is gone
    // goes first.
    #[test]
    fn tidyable_keeps_the_newest_three_and_others_worktrees() {
        let wts = vec![
            wt("/r/.claude/worktrees/a", "/r", true, &["a"]),
            wt("/r/.claude/worktrees/b", "/r", true, &["b"]),
            wt("/r/.claude/worktrees/c", "/r", true, &["c"]),
            wt("/r/.claude/worktrees/d", "/r", true, &["d"]),
            wt("/r/.claude/worktrees/e", "/r", true, &["e"]),
            wt("/r/elsewhere", "/r", false, &["f"]),
            wt("/r/.claude/worktrees/gone", "/r", true, &[]),
            wt("/r/.claude/worktrees/w", "/r", true, &["w"]),
        ];
        let due: HashMap<String, bool> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|k| (k.to_string(), *k != "b"))
            .collect();
        let finished: HashMap<String, Ms> =
            [("a", 1), ("b", 2), ("c", 5), ("d", 4), ("e", 3), ("f", 1)]
                .iter()
                .map(|(k, t)| (k.to_string(), *t))
                .collect();
        let gone = |w: &Worktree| w.path.ends_with("/gone");
        let got: Vec<&str> = tidyable(&wts, &due, &finished, &gone)
            .into_iter()
            .map(|i| wts[i].path.as_str())
            .collect();
        // c d e are the newest three; b isn't due; f isn't rush's; w works;
        // the one whose session is gone is the oldest of all.
        assert_eq!(got, ["/r/.claude/worktrees/a", "/r/.claude/worktrees/gone"]);
        let finished: HashMap<String, Ms> = HashMap::new();
        let got: Vec<&str> = tidyable(&wts, &due, &finished, &gone)
            .into_iter()
            .map(|i| wts[i].path.as_str())
            .collect();
        assert!(
            got.is_empty(),
            "the gone one is within the newest three: {got:?}"
        );
    }

    fn agent(key: &str) -> Agent {
        Agent {
            key: key.into(),
            display_name: key.into(),
            rush: true,
            job: rush_core::agent::Job {
                id: key.into(),
                state: rush_core::agent::State::Done,
                ..Default::default()
            },
            workspace: Some(rush_core::agent::Workspace {
                path: format!("/r/.claude/worktrees/{key}"),
                repo: "/r".into(),
                branch: format!("rush/{key}"),
                target: "main".into(),
                own: true,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn model(follows: bool) -> Model {
        let mut m = Model::new("t");
        m.follows = follows;
        m.agents = vec![agent("a")];
        m.roots = vec!["/r".into()];
        let d = m.kept::<Data>();
        d.cfg_read = true;
        m
    }

    // Only the rush that leads tidies done work or settles landed work.
    #[test]
    fn only_the_lead_tidies_and_settles() {
        let now = 10 * 3_600_000;
        let mut m = model(true);
        assert!(tidy_plan(&mut m, now).is_none());
        assert!(settle_looks(&mut m, now).is_none());
        let mut m = model(false);
        // The first look waits for rush to have run a little.
        assert!(tidy_plan(&mut m, now).is_none());
        assert!(tidy_plan(&mut m, now + TIDY_FIRST).is_some());
        assert!(
            tidy_plan(&mut m, now + TIDY_FIRST).is_none(),
            "one out at a time"
        );
        let looks = settle_looks(&mut m, now + SETTLE_FIRST).expect("looks");
        assert_eq!(looks.len(), 1);
        assert_eq!(looks[0].w.branch, "rush/a");
    }

    // One asking you something, or with a pull request open, isn't looked
    // at.
    #[test]
    fn settle_passes_over_open_work() {
        let mut m = model(false);
        m.agents[0].prs = vec![rush_core::adapter::Pr {
            state: "OPEN".into(),
            ..Default::default()
        }];
        assert!(settle_looks(&mut m, 3_600_000).is_none());
    }

    // onLanded: marked done once, with the news; a worktree kept is said
    // once, without its folder's prefix, a click away in Projects.
    #[test]
    fn landed_marks_done_and_says_so() {
        let mut m = model(false);
        let l = Landed {
            done: vec![("a".into(), "pull request #9 merged".into())],
            did: vec!["a: removed a, deleted branch rush/a".into()],
            stays: HashMap::new(),
        };
        on_landed(&mut m, l);
        assert!(m.agents[0].done);
        assert!(
            m.said
                .iter()
                .any(|s| s.text == "work landed: a: removed a, deleted branch rush/a"),
            "{:?}",
            m.said.iter().map(|s| &s.text).collect::<Vec<_>>()
        );
        let mut m = model(false);
        let mut stays = HashMap::new();
        stays.insert(
            "a".into(),
            "a stays: a isn't safe to remove: 1 file with uncommitted changes".into(),
        );
        on_landed(
            &mut m,
            Landed {
                done: vec![("a".into(), "its branch is on main".into())],
                did: Vec::new(),
                stays,
            },
        );
        let texts: Vec<&str> = m.said.iter().map(|s| s.text.as_str()).collect();
        assert!(
            texts.contains(
                &"kept “a”'s worktree: 1 file with uncommitted changes · open it in Projects"
            ),
            "{texts:?}"
        );
        assert!(
            texts.contains(&"work landed: 1 put away in Earlier"),
            "{texts:?}"
        );
    }

    // onTidied: what went and what was kept, in Go's words.
    #[test]
    fn tidied_says_what_went_and_what_stayed() {
        let mut m = model(false);
        on_tidied(
            &mut m,
            Tidied {
                wts: vec![wt("/r/.claude/worktrees/a", "/r", true, &["a"])],
                gone: vec!["/r/.claude/worktrees/a".into()],
                kept: vec![(
                    "/r/.claude/worktrees/b".into(),
                    "b isn't safe to remove: 1 commit not pushed or in main".into(),
                )],
                freed: 2 << 30,
                ..Tidied::default()
            },
        );
        let texts: Vec<&str> = m.said.iter().map(|s| s.text.as_str()).collect();
        assert!(
            texts.contains(
                &"kept b's worktree: 1 commit not pushed or in main · open it in Projects"
            ),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("removed the worktrees of done work: “a”")
                    && t.ends_with("nothing lost, branches kept")),
            "{texts:?}"
        );
    }
}
