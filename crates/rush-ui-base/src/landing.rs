//! What's left before an agent's work lands, as its row says it: the
//! list lays itself out by these, from what git last said of its
//! projects. Asking git, and telling an agent to land its work, are
//! screens/list_extras/landing.rs's. After Go's internal/ui/landing.go
//! (landKind :17, landState :31, toLand :40, tag :43, landOf :52,
//! landTarget :97, as of 2e4662ff) and folders.go (folderEvery :22).

use std::collections::HashMap;

use rush_core::agent::Agent;
use rush_core::text::Color;
use rush_core::time::Ms;
use rush_fleet::{Folder, GitState, Predicates};

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
    pub watch: Option<(Vec<String>, rush_sys::fswait::Deep)>,
    /// Something in a watched checkout changed since git was last asked.
    pub changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The stream's news is on its way to the UI thread.
    pub told: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A wake is set for when asking again is due.
    pub due: bool,
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
