//! The tidy-up of what's left running: dev servers, watchers and shells an
//! agent left when it stopped are ended as soon as they're found, and
//! orphans (processes whose session ended) two minutes after, unless
//! Settings › Orphaned processes keeps them. Go: internal/ui/cleanup.go
//! (orphanGrace :73, dueOrphans :77, tidy :221, reap :566, endLeftovers
//! :574, orphanWhat :600) and snapload.go:86. The process table is read
//! with each reading of the list, off the UI (scan::read_machine); the
//! tidy-up of done worktrees and of work that landed (tidyDone,
//! settleLanded) isn't here.

use std::collections::HashMap;
use std::time::Duration;

use rush_core::time::Ms;
use rush_fleet::Role;
use rush_fleet::Leftover;
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
    end_leftovers(m, left);
    let due = due_orphans(m, m.now);
    if !due.is_empty() {
        super::place::end_orphans(m, due, "orphaned for over 2m0s");
    }
}

/// The orphans that have been orphans for ORPHAN_GRACE, noting when each
/// new one was first seen.
pub(crate) fn due_orphans(m: &mut Model, now: Ms) -> Vec<ProcRow> {
    let st = m.kept::<Data>();
    let tab = st.prev_table.as_ref();
    if st.cfg.keep_orphans {
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
                        "cmd": clip_log(&l.cmd), "why": "still running after its agent stopped",
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

/// A long command cut for the events log.
fn clip_log(s: &str) -> String {
    if s.chars().count() <= 300 {
        return s.to_string();
    }
    let mut out: String = s.chars().take(300).collect();
    out.push('…');
    out
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
