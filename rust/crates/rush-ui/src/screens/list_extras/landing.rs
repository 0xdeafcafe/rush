//! What's left before an agent's work lands, and Earlier has it by
//! itself: an agent in a worktree of its own that's stopped work says so
//! on its row, worst first. An agent in the main checkout has nothing to
//! land, so it says nothing. After Go's internal/ui/landing.go (landKind
//! :17, landState :31, toLand :40, tag :43, landOf :52, landTarget :97,
//! landText :105, land :127, as of 2e4662ff), and folders.go's
//! refreshFolders :337 and onFolders :395 for the list (every look, as
//! of 2e4662ff), which ask git about the projects the list shows, off
//! the UI thread, every folderEvery (:22) and at once for one with no
//! answer yet. The tag goes under the name in the rail look (rail.go:96),
//! at the row's end in the classic (view.go:2336).

use std::collections::HashMap;

use rush_core::agent::Agent;
use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_fleet::{Folder, GitState, Predicates, check_folder, folder_wants};

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// How often git is asked about the list's projects again (folderEvery).
pub const FOLDER_EVERY: Ms = 15_000;

/// What's left before an agent's work lands (landKind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LandKind {
    /// Not in a worktree, at work, or git hasn't said yet.
    #[default]
    None,
    /// Files changed and not committed.
    Uncommitted,
    /// Its pull request's checks fail.
    PrFailing,
    /// Its pull request has changes asked for.
    PrChanges,
    /// Its pull request waits on checks, a review or a draft.
    PrWaiting,
    /// Its pull request could be merged now.
    PrReady,
    /// Commits not on its target, and no pull request open.
    Unmerged,
    /// Merged, or on its target: Earlier has it shortly.
    Landed,
    /// No work of its own in the worktree.
    Nothing,
}

/// What's left before an agent's work lands: what kind, and the few words
/// its row says it in (landState).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LandState {
    pub kind: LandKind,
    pub text: String,
    pub color: Color,
    /// The branch its work is meant to land on.
    pub target: String,
    pub pr: u64,
}

impl LandState {
    /// Whether all that's left is yours to do: merge it (toLand).
    pub fn to_land(&self) -> bool {
        matches!(self.kind, LandKind::PrReady | LandKind::Unmerged)
    }

    /// The state as its row shows it (tag); None with nothing to say.
    pub fn tag(&self) -> Option<rush_core::text::Span> {
        (self.kind != LandKind::None).then(|| sp(self.text.clone(), self.color))
    }
}

/// What git last said of the list's projects (Go's m.folders for the
/// list): by root, when it was last asked, and how many answers are out.
#[derive(Default)]
pub struct ListFolders {
    pub by_root: HashMap<String, Folder>,
    pub asked: Ms,
    pub looking: bool,
    /// Rust-only: the checkouts asked about, watched (rush_sys's Deep):
    /// git is asked again once something in one changed, FOLDER_EVERY
    /// apart at most, as Go's tick would have, and not while nothing
    /// does. Where there's no stream, a timer asks every FOLDER_EVERY.
    watch: Option<(Vec<String>, rush_sys::fswait::Deep)>,
    /// Something in a watched checkout changed since git was last asked.
    changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The stream's news is on its way to the UI thread.
    told: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A wake is set for when asking again is due.
    due: bool,
}

/// The linked worktree a works in, "" in the main checkout (treeOf).
pub fn tree_of(a: &Agent) -> &str {
    if !a.root.is_empty() && a.repo != a.root {
        &a.repo
    } else {
        ""
    }
}

/// The branch a's work is meant to land on: its session's, else the one
/// its worktree was made from, else main (landTarget).
pub fn land_target(a: &Agent, st: Option<&GitState>) -> String {
    if let Some(w) = &a.workspace
        && !w.target.is_empty()
    {
        return w.target.clone();
    }
    match st {
        Some(st) if !st.base.is_empty() => st.base.clone(),
        _ => "main".into(),
    }
}

/// What's left before a's work lands, from what git and GitHub last
/// said: no git is run here (landOf).
pub fn land_of(m: &Model, a: &Agent) -> LandState {
    let tree = tree_of(a);
    if tree.is_empty() || a.done || !a.peer.is_empty() || a.job.live() || a.busy() || a.checking {
        return LandState::default();
    }
    let st = m
        .kept
        .peek::<ListFolders>()
        .and_then(|f| f.by_root.get(&a.root))
        .and_then(|f| f.trees.get(tree))
        .filter(|st| st.err.is_empty());
    let mut l = LandState {
        target: land_target(a, st),
        ..LandState::default()
    };
    let pr = a.pr.as_ref();
    if let Some(pr) = pr {
        l.pr = pr.number;
    }
    let n = pr.map_or(String::new(), |pr| format!("PR #{} ", pr.number));
    let open = pr.is_some_and(|p| p.open());
    let mut set = |k, c, t: String| {
        l.kind = k;
        l.color = c;
        l.text = t;
    };
    match (st, pr) {
        (Some(st), _) if st.changed > 0 => set(
            LandKind::Uncommitted,
            RED,
            format!("±{} not committed", st.changed),
        ),
        (_, Some(pr)) if pr.merged() => {
            set(LandKind::Landed, GREEN, "landed · Earlier soon".into())
        }
        (_, Some(pr)) if open && pr.checks == "fail" => {
            set(LandKind::PrFailing, RED, format!("{n}✗ checks"))
        }
        (_, Some(pr)) if open && pr.review == "changes" => {
            set(LandKind::PrChanges, YELLOW, format!("{n}changes asked"))
        }
        (_, Some(pr)) if pr.state == "draft" => set(LandKind::PrWaiting, DIM, format!("{n}draft")),
        (_, Some(pr)) if open && pr.checks == "pending" => {
            set(LandKind::PrWaiting, DIM, format!("{n}checks running"))
        }
        (_, Some(_)) if open => set(LandKind::PrReady, ORANGE, format!("{n}ready to merge")),
        (None, _) => {}
        (Some(st), _) if st.base_ahead > 0 => {
            let t = format!(
                "{} commit{} not on {}",
                st.base_ahead,
                if st.base_ahead == 1 { "" } else { "s" },
                l.target
            );
            set(LandKind::Unmerged, ORANGE, t)
        }
        (Some(_), _) => set(LandKind::Nothing, DIM, "nothing to land".into()),
    }
    l
}

/// What the agent is told to do to land its work (landText).
pub fn land_text(l: &LandState) -> String {
    let pr = format!("pull request #{}", l.pr);
    let t = &l.target;
    match l.kind {
        LandKind::Uncommitted => format!(
            "Land your work on {t}: commit what's left in this worktree, then get it onto {t} the way this project does (open a pull request and merge it once it's green, or merge the branch straight in)."
        ),
        LandKind::PrFailing => {
            format!("The checks on {pr} are failing. Fix them, push, and merge it once it's green.")
        }
        LandKind::PrChanges => format!(
            "Changes were asked for on {pr}. Make them, push, and merge it once it's approved and green."
        ),
        LandKind::PrWaiting => format!(
            "Get {pr} merged: mark it ready if it's a draft, see its checks through, and merge it once it's green."
        ),
        LandKind::PrReady => format!("Merge {pr} into {t}."),
        LandKind::Unmerged => format!(
            "Land your work on {t}: get this branch onto {t} the way this project does (open a pull request and merge it once it's green, or merge the branch straight in)."
        ),
        _ => String::new(),
    }
}

/// Tells agent i to land its work, as a message you see it get (land:
/// alt+p, #land), or says why not.
pub fn land(m: &mut Model, i: Option<usize>) {
    let Some(i) = i else {
        m.flash_err("select an agent first");
        return;
    };
    let a = m.agents[i].clone();
    let name = &a.display_name;
    let l = land_of(m, &a);
    if a.interactive {
        m.flash_err(&format!("{name} is open in another terminal"));
    } else if a.job.live() || a.busy() {
        m.flash(&format!("{name} is still working"));
    } else if tree_of(&a).is_empty() {
        m.flash(&format!(
            "{name} works in the main checkout: nothing to land"
        ));
    } else if l.kind == LandKind::None {
        m.flash(&format!(
            "git hasn't said yet what {name} has to land · try again in a moment"
        ));
    } else if l.kind == LandKind::Landed {
        m.flash(&format!("{name}'s work has landed · Earlier has it soon"));
    } else if l.kind == LandKind::Nothing {
        m.flash(&format!(
            "{name} has nothing to land · alt+d puts it in Earlier"
        ));
    } else {
        // replyTo: you see it go, then its answer.
        m.flash(&format!("sending to {name}…"));
        m.reply_to(&a, &land_text(&l));
    }
}

/// alt+p in the list (keys.go:470).
pub(super) fn key(m: &mut Model, k: &Press) -> Out {
    if k.s != "alt+p" {
        return Out::Pass;
    }
    let i = m.selected();
    land(m, i);
    Out::Draw
}

/// #land (keys.go:994).
pub(super) fn command(m: &mut Model, _: &str) -> Out {
    let i = m.selected();
    land(m, i);
    Out::Draw
}

/// Asks git about the projects the list shows, but Earlier's, off the
/// UI thread: every FOLDER_EVERY, and at once when a project or worktree
/// has no answer yet (refreshFolders, onFolders). Only with a loop
/// running; a model drawn once (tests, --render) is handed its answers.
pub(super) fn refresh(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
    ask_folders(m);
}

/// Asks git about the projects the list shows when it's time, off the UI
/// thread; with no loop (`rush --render`), at once, as Go's Frame drains
/// what a reading started.
pub fn ask_folders(m: &mut Model) {
    if m.kept::<ListFolders>().looking {
        return;
    }
    let listed: Vec<Agent> = m
        .order
        .iter()
        .filter(|&&i| m.group_of.get(&m.agents[i].key).map(String::as_str) != Some("Earlier"))
        .map(|&i| m.agents[i].clone())
        .collect();
    let wants = folder_wants(&listed);
    if wants.is_empty() {
        return;
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    watch(m, &wants);
    let f = m.kept::<ListFolders>();
    let due = now - f.asked >= FOLDER_EVERY;
    let watching = f.watch.is_some();
    let changed = f.changed.load(std::sync::atomic::Ordering::Relaxed);
    // Watched, only what changed is asked again; else every FOLDER_EVERY.
    let mut stale = due && (changed || !watching);
    for (root, trees) in &wants {
        match f.by_root.get(root) {
            None => stale = true,
            Some(had) => stale = stale || trees.iter().any(|t| !had.trees.contains_key(t)),
        }
    }
    if !stale {
        let left = f.asked + FOLDER_EVERY - now;
        if (changed || !watching) && !f.due {
            wake_at(m, left);
        }
        return;
    }
    f.changed.store(false, std::sync::atomic::Ordering::Relaxed);
    f.looking = true;
    f.asked = now;
    rush_draw::perf_note(|| format!("folders asked {} watched={watching}", wants.len()));
    let wants: Vec<(String, Vec<String>)> = wants.into_iter().collect();
    m.later(
        move || {
            // A few projects ask git at once, not one per repository
            // (folderGate).
            let mut out = Vec::with_capacity(wants.len());
            for chunk in wants.chunks(4) {
                std::thread::scope(|s| {
                    let hs: Vec<_> = chunk
                        .iter()
                        .map(|(root, trees)| s.spawn(move || check_folder(root, trees, false)))
                        .collect();
                    out.extend(hs.into_iter().filter_map(|h| h.join().ok()));
                });
            }
            out
        },
        |m, got: Vec<Folder>| {
            let f = m.kept::<ListFolders>();
            f.looking = false;
            for g in got {
                f.by_root.insert(g.root.clone(), g);
            }
            m.rebuild();
            Out::Draw
        },
    );
}

/// Asks again once `left` has passed (only with a loop running).
fn wake_at(m: &mut Model, left: Ms) {
    if m.sender().is_none() {
        return;
    }
    m.kept::<ListFolders>().due = true;
    let left = std::time::Duration::from_millis(left.clamp(0, FOLDER_EVERY) as u64);
    m.later(
        move || std::thread::sleep(left),
        |m, ()| {
            m.kept::<ListFolders>().due = false;
            ask_folders(m);
            Out::Idle
        },
    );
}

/// Keeps the stream on the checkouts git is asked about: each root, and
/// each worktree not under one. What asking git writes (a checkout's
/// index, its objects) doesn't count as a change.
fn watch(m: &mut Model, wants: &HashMap<String, Vec<String>>) {
    let Some(tx) = m.sender() else { return };
    let mut paths: Vec<String> = wants.keys().cloned().collect();
    for (root, trees) in wants {
        for t in trees {
            if !std::path::Path::new(t).starts_with(root) {
                paths.push(t.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    let f = m.kept::<ListFolders>();
    if f.watch.as_ref().is_some_and(|(p, _)| *p == paths) {
        return;
    }
    let roots: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    let mut skip = Vec::new();
    for r in &roots {
        let git = r.join(".git");
        skip.push(git.join("index"));
        skip.push(git.join("index.lock"));
        skip.push(git.join("objects"));
        skip.push(git.join("worktrees")); // each worktree's own index
    }
    let (changed, told) = (f.changed.clone(), f.told.clone());
    // Two: news that comes while the last is being handled still lands.
    let tx = tx.lane(2);
    let tell: rush_sys::fswait::Tell = Box::new(move |v| {
        rush_draw::perf_note(|| format!("folders changed {v:?}"));
        changed.store(true, std::sync::atomic::Ordering::Relaxed);
        if told.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return; // the last news is still on its way
        }
        let told = told.clone();
        let sent = tx.try_send(crate::worker::Note::Apply(Box::new(
            move |m: &mut Model| {
                m.kept::<ListFolders>()
                    .told
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                ask_folders(m);
                Out::Idle
            },
        )));
        if sent.is_err() {
            told.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    });
    f.watch = rush_sys::fswait::Deep::new(&roots, &skip, tell).map(|d| (paths, d));
    let on = f.watch.is_some();
    rush_draw::perf_note(|| format!("folders watched {roots:?} on={on}"));
}

/// Under the name in the rail look: what's left before Earlier has it
/// (rail.go:96).
pub fn where_tag(m: &Model, a: &Agent) -> Option<Line> {
    land_of(m, a).tag().map(|s| line([s]))
}
