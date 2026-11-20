//! A worktree's one mark, and the words that go with it: what Projects
//! draws for a worktree wherever it draws one (the tree, a project's
//! WORKTREES, the top line's "can go"). Rust-only (the Projects redesign,
//! docs/projects-redesign-task-list.md, slice 1); it reads the clean-up
//! scan's verdict (rush-fleet/src/verdict.rs:106 Worktree::verdict) and
//! nothing else, so no two places can say different things of one
//! worktree.

use rush_fleet::{Predicates, VerdictKind, Worktree};

use super::data::agents;
use crate::model::Model;

/// A worktree's mark: one per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// ✱ an agent is working in it.
    Working,
    /// ✓ it can go: removing it loses nothing.
    CanGo,
    /// ⚠ keep: removing it would lose work (the words say what).
    Keep,
    /// Blank: kept for now (its pull request open, a session in it but
    /// idle, not looked at yet).
    Kept,
}

impl Mark {
    /// The glyph drawn for it, one cell.
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::Working => "✱",
            Mark::CanGo => "✓",
            Mark::Keep => "⚠",
            Mark::Kept => " ",
        }
    }
}

/// A worktree's mark and why, in a few words. working: an agent is in a
/// turn in it now (the model's word, fresher than the scan's).
pub fn worktree_mark(t: &Worktree, working: bool) -> (Mark, String) {
    let v = t.verdict();
    if working {
        let why = if t.in_use.is_empty() {
            "an agent is working in it".to_string()
        } else {
            v.reason
        };
        return (Mark::Working, why);
    }
    if t.checked == 0 && t.in_use.is_empty() {
        return (Mark::Kept, "not looked at yet".into());
    }
    match v.kind {
        VerdictKind::KeepInUse => (Mark::Kept, format!("{} is open in it", t.in_use)),
        VerdictKind::KeepPrOpen => (Mark::Kept, v.reason),
        VerdictKind::CanGo => (Mark::CanGo, v.reason),
        VerdictKind::WouldLose => (Mark::Keep, v.reason),
    }
}

/// An agent is in a turn in the worktree at path.
pub fn working_in(m: &Model, path: &str) -> bool {
    agents(m).any(|g| g.tree() == path && (g.live() || g.busy()))
}

/// t as the list has it now: who's in it is the model's, read this frame,
/// not what the last scan saw (the scan's verdict reads in_use).
pub fn worktree_now(m: &Model, t: &Worktree) -> Worktree {
    let mut t = t.clone();
    t.in_use = agents(m)
        .filter(|g| g.tree() == t.path && g.a.in_use())
        .map(|g| {
            let n = crate::style::one_line(&g.a.display_name);
            if n.is_empty() { "a session".into() } else { n }
        })
        .next()
        .unwrap_or_default();
    t
}

/// t's mark and words as the list has it now.
pub fn mark_now(m: &Model, t: &Worktree) -> (Mark, String) {
    worktree_mark(&worktree_now(m, t), working_in(m, &t.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::agent::PrStatus;

    #[test]
    fn each_state_has_one_mark() {
        fn looked(f: &dyn Fn(&mut Worktree)) -> Worktree {
            let mut t = Worktree::default();
            t.path = "/r/.claude/worktrees/x".into();
            t.checked = 1;
            t.target = "main".into();
            f(&mut t);
            t
        }
        let pr = |state: &str| PrStatus {
            number: 7,
            state: state.into(),
            ..PrStatus::default()
        };
        let cases: Vec<(&str, Worktree, bool, Mark, &str)> = vec![
            (
                "working, named",
                looked(&|t: &mut Worktree| t.in_use = "fix it".into()),
                true,
                Mark::Working,
                "fix it is working in it",
            ),
            (
                "working, the scan not caught up",
                looked(&|_| {}),
                true,
                Mark::Working,
                "an agent is working in it",
            ),
            (
                "working, with work only it has",
                looked(&|t: &mut Worktree| {
                    t.in_use = "fix it".into();
                    t.changed = 2;
                }),
                true,
                Mark::Working,
                "fix it is working in it",
            ),
            (
                "open but idle",
                looked(&|t: &mut Worktree| t.in_use = "fix it".into()),
                false,
                Mark::Kept,
                "fix it is open in it",
            ),
            (
                "not looked at",
                Worktree::default(),
                false,
                Mark::Kept,
                "not looked at yet",
            ),
            (
                "pull request open",
                looked(&|t: &mut Worktree| {
                    t.pr = Some(pr("open"));
                    t.unpushed = 0;
                }),
                false,
                Mark::Kept,
                "its pull request #7 is open",
            ),
            (
                "pushed",
                looked(&|_| {}),
                false,
                Mark::CanGo,
                "every commit is pushed",
            ),
            (
                "landed",
                looked(&|t: &mut Worktree| t.landed = true),
                false,
                Mark::CanGo,
                "its work is on main",
            ),
            (
                "uncommitted",
                looked(&|t: &mut Worktree| t.changed = 1),
                false,
                Mark::Keep,
                "1 file with uncommitted changes",
            ),
            (
                "unpushed",
                looked(&|t: &mut Worktree| t.unpushed = 2),
                false,
                Mark::Keep,
                "2 commits not pushed or in main",
            ),
            (
                "git failed",
                looked(&|t: &mut Worktree| t.err = "not a git repository".into()),
                false,
                Mark::Keep,
                "not a git repository",
            ),
        ];
        for (name, t, working, mark, why) in cases {
            assert_eq!(
                worktree_mark(&t, working),
                (mark, why.to_string()),
                "{name}"
            );
        }
    }

    #[test]
    fn glyphs() {
        for (m, g) in [
            (Mark::Working, "✱"),
            (Mark::CanGo, "✓"),
            (Mark::Keep, "⚠"),
            (Mark::Kept, " "),
        ] {
            assert_eq!(m.glyph(), g);
        }
    }
}
