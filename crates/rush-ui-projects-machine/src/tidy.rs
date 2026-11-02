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
pub const ORPHAN_GRACE: Ms = 2 * 60_000;

/// When each orphan, by pid and start, was first seen one.
pub type Orphans = HashMap<(Pid, Ms), Ms>;

/// After each reading of the machine: what agents left is ended, and the
/// orphans whose grace is up. Not in tests or --render (no loop).
pub fn after_reading(m: &mut Model, left: Vec<Leftover>) {
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
pub fn to_end(m: &mut Model, left: Vec<Leftover>) -> (Vec<Leftover>, Vec<ProcRow>) {
    if !m.leads() {
        return (Vec::new(), Vec::new());
    }
    let due = due_orphans(m, m.now);
    (left, due)
}

/// The orphans that have been orphans for ORPHAN_GRACE, noting when each
/// new one was first seen; none in a rush that follows (cleanup.go:79).
pub fn due_orphans(m: &mut Model, now: Ms) -> Vec<ProcRow> {
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
pub fn leftover_flash(left: &[Leftover], n: usize) -> String {
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
pub fn orphan_what(cmd: &str) -> String {
    let Some(i) = cmd.find("eval '") else {
        return cmd.to_string();
    };
    let mut rest = cmd[i + 6..].replace(r#"'"'"'"#, "'");
    if let Some(j) = rest.find("' ") {
        rest.truncate(j);
    }
    rest.strip_suffix('\'').unwrap_or(&rest).to_string()
}
