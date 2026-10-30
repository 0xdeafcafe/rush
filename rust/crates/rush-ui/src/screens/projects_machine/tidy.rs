//! The tidy-up of what's left running: dev servers, watchers and shells an
//! agent left when it stopped are ended as soon as they're found, and
//! orphans (processes whose session ended) two minutes after, unless
//! Settings › Orphaned processes keeps them. Go: internal/ui/cleanup.go
//! (orphanGrace :73, dueOrphans :77, tidy :221, reap :566, endLeftovers
//! :574, orphanWhat :600) and snapload.go:86. The process table is read
//! with each reading of the list, off the UI (scan::read_machine); the
//! tidy-up of done worktrees and of work that landed (tidyDone,
//! settleLanded) is tidydone.rs's.

use std::collections::HashMap;
use std::time::Duration;

use rush_core::time::Ms;
use rush_fleet::Leftover;
use rush_fleet::Role;
use rush_sys::proc::Pid;

use super::data::*;
use super::rows::*;
use crate::frame::Out;
use crate::model::Model;
use crate::style::one_line;

/// How long a process whose session ended is left before it's ended:
/// long enough for a session being restarted to take it back.
pub(crate) const ORPHAN_GRACE: Ms = 2 * 60_000;

/// When each orphan, by pid and start, was first seen one.
pub(crate) type Orphans = HashMap<(Pid, Ms), Ms>;

/// After each reading of the machine: what agents left is ended, and the
/// orphans whose grace is up. Not in tests or --render (no loop).
pub(crate) fn after_reading(m: &mut Model, left: Vec<Leftover>) {
    if data(m).frozen || m.sender().is_none() {
        return;
    }
    let (mut left, mut due) = to_end(m, left);
    // A dry run writes each down instead (rush_core::dryrun).
    left.retain(|l| {
        !rush_core::dryrun::would(
            "end-leftover",
            &format!("pid {} {}", l.pid, rush_host::events::clip(&l.cmd)),
            &format!(
                "still running after its agent “{}” stopped",
                one_line(&l.agent)
            ),
        )
    });
    due.retain(|r| {
        !rush_core::dryrun::would(
            "end-orphan",
            &format!("pid {} {}", r.pid, rush_host::events::clip(&r.cmd)),
            &format!("orphaned for over 2m0s ({})", r.label),
        )
    });
    end_leftovers(m, left);
    if !due.is_empty() {
        super::place::end_orphans(m, due, "orphaned for over 2m0s");
    }
}

/// What a reading of the machine ends: what agents left, and the orphans
/// whose grace is up. Nothing in a rush that follows: the one that leads
/// ends them (snapload.go:84 reap, cleanup.go:79).
pub(crate) fn to_end(m: &mut Model, left: Vec<Leftover>) -> (Vec<Leftover>, Vec<ProcRow>) {
    if !m.leads() {
        return (Vec::new(), Vec::new());
    }
    let due = due_orphans(m, m.now);
    (left, due)
}

/// The orphans that have been orphans for ORPHAN_GRACE, noting when each
/// new one was first seen; none in a rush that follows (cleanup.go:79).
pub(crate) fn due_orphans(m: &mut Model, now: Ms) -> Vec<ProcRow> {
    if !m.leads() {
        return Vec::new();
    }
    let st = m.kept::<Data>();
    let tab = st.prev_table.as_ref();
    // Settings not read yet can't say whether orphans are kept: none goes
    // until they are.
    if st.cfg.keep_orphans || !st.cfg_read {
        return Vec::new();
    }
    let mut seen = Orphans::new();
    let mut due = Vec::new();
    for r in st.machine.rows.iter().filter(|r| r.role == Role::Orphan) {
        let id = (r.pid, r.start);
        let first = st.orphans.get(&id).copied().unwrap_or(now);
        if now - first >= ORPHAN_GRACE {
            // Ended now; still there next time, it's tried again after
            // the grace.
            due.push(ProcRow {
                pid: r.pid,
                depth: 0,
                n: r.procs,
                label: r.label.clone(),
                cmd: r.cmd.clone(),
                mem: r.mem,
                cpu: r.cpu,
                start: proc_start(tab, r.pid, r.start),
                role: r.role,
                other: true,
            });
        } else {
            seen.insert(id, first);
        }
    }
    st.orphans = seen;
    due
}

/// Ends what reap found, off the UI, and says what it ended.
fn end_leftovers(m: &mut Model, mut left: Vec<Leftover>) {
    if left.is_empty() {
        return;
    }
    m.later(
        move || {
            let mut n = 0;
            for l in &mut left {
                l.describe(); // before it's ended, while it's there to ask
                n += l.procs.max(1);
                rush_sys::events::log(
                    "leftover-ended",
                    serde_json::json!({
                        "agent": l.agent, "pid": l.pid, "procs": l.procs,
                        "cmd": rush_host::events::clip(&l.cmd), "why": "still running after its agent stopped",
                    }),
                );
            }
            for l in &left {
                let (pid, start) = (l.pid, l.start);
                std::thread::spawn(move || {
                    rush_sys::actions::end_tree(pid, start, Duration::from_secs(3))
                });
            }
            leftover_flash(&left, n)
        },
        |m, text| {
            m.flash(&text);
            Out::Draw
        },
    );
}

/// What the flash says once what was left is ended.
pub(crate) fn leftover_flash(left: &[Leftover], n: usize) -> String {
    let Some(l) = left.first() else {
        return String::new();
    };
    let what = trim_cmd(&orphan_what(&l.cmd), 60);
    let mut text = format!("ended what {} left running: {what}", one_line(&l.agent));
    if n > 1 {
        text += &format!(" ({n} processes)");
    }
    text
}

/// The command a Bash-tool shell was running, without Claude Code's
/// wrapper around it (cleanup.go:600 orphanWhat).
pub(crate) fn orphan_what(cmd: &str) -> String {
    let Some(i) = cmd.find("eval '") else {
        return cmd.to_string();
    };
    let mut rest = cmd[i + 6..].replace(r#"'"'"'"#, "'");
    if let Some(j) = rest.find("' ") {
        rest.truncate(j);
    }
    rest.strip_suffix('\'').unwrap_or(&rest).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_fleet::ProcRow as MRow;
    use std::time::UNIX_EPOCH;

    fn orphan(pid: Pid) -> MRow {
        MRow {
            pid,
            role: Role::Orphan,
            label: "ended session".into(),
            cmd: "node".into(),
            mem: 1 << 20,
            cpu: 0.0,
            procs: 1,
            start: pid as Ms * 1000,
            key: String::new(),
        }
    }

    #[test]
    fn orphans_go_after_their_grace() {
        let mut m = Model::new("t");
        m.follows = false;
        m.kept::<Data>().cfg_read = true;
        m.kept::<Data>().machine.rows = vec![orphan(7), orphan(8)];
        assert!(due_orphans(&mut m, 1_000).is_empty());
        assert!(due_orphans(&mut m, 1_000 + ORPHAN_GRACE - 1).is_empty());
        m.kept::<Data>().machine.rows = vec![orphan(7)];
        let due = due_orphans(&mut m, 1_000 + ORPHAN_GRACE);
        assert_eq!(due.iter().map(|r| r.pid).collect::<Vec<_>>(), [7]);
        assert!(
            m.kept::<Data>().orphans.is_empty(),
            "8 went; 7 is tried again after the grace"
        );
        m.kept::<Data>().cfg.keep_orphans = true;
        m.kept::<Data>().machine.rows = vec![orphan(9)];
        assert!(due_orphans(&mut m, 0).is_empty());
        assert!(due_orphans(&mut m, ORPHAN_GRACE * 2).is_empty());
    }

    // Only a row the machine reading called an orphan is ever due, and
    // none before Settings is read (it may say keep them).
    #[test]
    fn only_orphans_and_only_once_settings_say() {
        let mut m = Model::new("t");
        m.follows = false;
        let mut other = orphan(5);
        other.role = Role::Other;
        m.kept::<Data>().machine.rows = vec![orphan(7), other];
        assert!(due_orphans(&mut m, 0).is_empty());
        assert!(
            due_orphans(&mut m, ORPHAN_GRACE * 2).is_empty(),
            "settings unread"
        );
        m.kept::<Data>().cfg_read = true;
        assert!(
            due_orphans(&mut m, ORPHAN_GRACE * 3).is_empty(),
            "first seen now"
        );
        let due = due_orphans(&mut m, ORPHAN_GRACE * 5);
        assert_eq!(due.iter().map(|r| r.pid).collect::<Vec<_>>(), [7]);
    }

    fn leftover(pid: Pid) -> Leftover {
        Leftover {
            pid,
            start: UNIX_EPOCH,
            agent: "fix the tests".into(),
            cmd: "pnpm dev".into(),
            procs: 1,
        }
    }

    // A rush that follows ends nothing on its own: not what an agent left
    // running, nor an orphan past its grace. Leading, it ends both.
    #[test]
    fn only_the_rush_that_leads_ends_leftovers_and_orphans() {
        for follows in [true, false] {
            let mut m = Model::new("t");
            m.follows = follows;
            m.now = ORPHAN_GRACE * 3;
            m.kept::<Data>().cfg_read = true;
            m.kept::<Data>().machine.rows = vec![orphan(7)];
            m.kept::<Data>().orphans.insert((7, 7000), 0);
            let (left, due) = to_end(&mut m, vec![leftover(42)]);
            let pids = |v: &[Leftover]| v.iter().map(|l| l.pid).collect::<Vec<_>>();
            if follows {
                assert!(
                    left.is_empty() && due.is_empty(),
                    "following ended something"
                );
                assert!(due_orphans(&mut m, ORPHAN_GRACE * 4).is_empty());
            } else {
                assert_eq!(pids(&left), [42]);
                assert_eq!(due.iter().map(|r| r.pid).collect::<Vec<_>>(), [7]);
            }
        }
    }

    #[test]
    fn says_what_was_left() {
        let l = |cmd: &str, procs| Leftover {
            pid: 1,
            start: UNIX_EPOCH,
            agent: "fix the\ntests".into(),
            cmd: cmd.into(),
            procs,
        };
        assert_eq!(
            leftover_flash(&[l("/bin/zsh -c eval 'pnpm dev' < /dev/null", 3)], 3),
            "ended what fix the tests left running: pnpm dev (3 processes)"
        );
        assert_eq!(
            leftover_flash(&[l("node server.js", 1)], 1),
            "ended what fix the tests left running: node server.js"
        );
    }
}
