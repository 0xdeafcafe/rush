//! The tests of rush-ui-projects-machine's tidy.rs that need the whole UI.

use rush_core::time::Ms;
use rush_fleet::Leftover;
use rush_fleet::Role;
use rush_sys::proc::Pid;
use rush_ui_projects_machine::data::*;
use rush_ui_projects_machine::tidy::*;

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
    let mut m = crate::new_model("t");
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
    let mut m = crate::new_model("t");
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
        let mut m = crate::new_model("t");
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
