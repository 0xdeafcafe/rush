//! The Projects place's tests, ported from Go's internal/ui
//! (projects_test.go, worktreerow_test.go, projecttree_test.go,
//! cleanup_test.go, deletesheet_test.go): frames at Go's widths, and its
//! keys. Nothing is asked of git, GitHub or the disk (Data::frozen), and
//! nothing reads ~/.config/rush.

use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_core::time::Ms;

use rush_fleet::Scratch;
use rush_fleet::Worktree;
use rush_fleet::{Folder, GitState};

use super::data::*;
use crate::frame::{Out, PlaceId};
use crate::model::Model;
use crate::worker::Fleet;

const NOW: Ms = 1_793_190_896_789;
const HOUR: Ms = 3_600_000;

fn agent(key: &str, name: &str, root: &str, repo: &str, pid: i32, ago: Ms) -> (Agent, String) {
    let mut a = Agent {
        key: key.into(),
        display_name: name.into(),
        repo: repo.into(),
        branch: "main".into(),
        pid,
        interactive: true,
        ..Agent::default()
    };
    a.job.id = key.into();
    a.job.state = if pid != 0 {
        State::Working
    } else {
        State::Idle
    };
    a.job.cwd = repo.into();
    a.job.created_at = NOW - ago - HOUR;
    a.job.updated_at = NOW - ago;
    (a, root.to_string())
}

/// A model on Projects over agents, w by h, its config as given.
fn model(agents: Vec<(Agent, String)>, w: usize, h: usize, cfg: Cfg) -> Model {
    // As the binary does: Claude's project folder (.claude) is the
    // registry's; a registry of its own, as one Claude put in the
    // process's would reach every other test's model.
    let reg = rush_core::adapter::Registry::default();
    reg.register(rush_claude::Claude::new());
    let mut m = Model::new("0.1.0");
    m.reg = reg;
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = w;
    m.h = h;
    let d = m.kept::<Data>();
    d.frozen = true;
    d.cfg = cfg;
    d.cfg_read = true;
    let (agents, roots) = agents.into_iter().unzip();
    m.set_fleet(Fleet {
        agents,
        roots,
        ..Fleet::default()
    });
    m.set_place(PlaceId::Projects);
    m
}

fn classic() -> Cfg {
    Cfg {
        list_look: "classic".into(),
        ..Cfg::default()
    }
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press(s)
}

fn three() -> Vec<(Agent, String)> {
    vec![
        agent("a1", "fix alpha", "/src/alpha", "/src/alpha", 1, 0),
        agent("a2", "test alpha", "/src/alpha", "/src/alpha", 2, 0),
        agent("b1", "beta work", "/src/beta", "/src/beta", 3, 0),
    ]
}

/// The body as Go's projectsBody draws it, from the frame: the rows
/// between the header's rule and the hint's, out of the frame's indent,
/// and the hint. A working agent's spinner is put at its first frame.
fn body(m: &mut Model) -> String {
    let lines: Vec<String> = m.frame().iter().map(Line::plain).collect();
    let rules: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].starts_with("────"))
        .collect();
    let mut out = String::new();
    for l in &lines[rules[0] + 1..rules[1]] {
        let l = l.strip_prefix("  ").unwrap_or(l).trim_end();
        out.push_str(l);
        out.push('\n');
    }
    let rows: Vec<&str> = out.trim_end_matches('\n').split('\n').collect();
    let mut out = rows.join("\n") + "\n";
    out.push_str(&format!("HINT {}\n", lines[rules[1] + 1].trim().trim_end()));
    for s in crate::style::SPINNER {
        out = out.replace(&format!("{s} "), "· ");
    }
    out
}

/// Go's frames of the same agents (a test of Go's that printed
/// projectsBody and projectsHint), by case.
fn oracle(all: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in all.split("=== ").skip(1) {
        let (name, rest) = part.split_once('\n').unwrap();
        out.push((name.to_string(), rest.to_string()));
    }
    out
}

#[test]
fn frames_match_go() {
    let cases: [(usize, Cfg, &[&str]); 5] = [
        (160, classic(), &[]),
        (120, Cfg::default(), &[]),
        (160, classic(), &["enter"]),
        (160, classic(), &["enter", "down"]),
        (160, classic(), &["down", "down", "down"]),
    ];
    let want = oracle(include_str!("testdata/projects.txt"));
    assert_eq!(want.len(), cases.len());
    for ((w, cfg, keys), (name, want)) in cases.into_iter().zip(want) {
        let mut m = model(three(), w, 45, cfg);
        for k in keys {
            press(&mut m, k);
        }
        same_as_go(&mut m, &name, &want);
    }
}

/// The frame's body and hint against Go's.
fn same_as_go(m: &mut Model, name: &str, want: &str) {
    {
        let got = body(m);
        // Go's body runs past the screen, its boxes to the bottom; Go's
        // frame (and the real one, checked with rush --render) shows the
        // boxes' bottom edge on the screen's last body row.
        let got_rows: Vec<&str> = got.lines().collect();
        let want_rows: Vec<&str> = want.lines().collect();
        let n = got_rows.len() - 1;
        let mut want: Vec<&str> = want_rows[..want_rows.len() - 1].to_vec();
        if want.len() > n {
            let edge = want[want.len() - 1];
            want.truncate(n - 1);
            want.push(edge);
        }
        let want = want.join("\n") + "\n" + want_rows[want_rows.len() - 1] + "\n";
        assert_eq!(got, want, "{name}");
    }
}

/// Go's richModel: a worktree checked and measured, an agent done with,
/// a folder that isn't a repository, /tmp and temp work.
fn rich(w: usize, cfg: Cfg) -> Model {
    let wt = "/src/alpha/.claude/worktrees/fix";
    let mut fix = agent("a1", "fix the parser", "/src/alpha", wt, 1, 0);
    fix.0.branch = "main".into();
    let old = agent("a2", "old probe", "/src/alpha", "/src/alpha", 0, 5 * HOUR);
    let mut dl = agent("d1", "Sort downloads", "", "", 0, 5 * HOUR);
    dl.0.job.cwd = "/Users/me/Downloads".into();
    let mut m = model(vec![fix, old, dl], w, 60, cfg);
    let d = m.kept::<Data>();
    let mut f = Folder {
        root: "/src/alpha".into(),
        worktrees: 2,
        ..Folder::default()
    };
    f.git.branch = "main".into();
    f.git.changed = 2;
    let st = GitState {
        branch: "fix".into(),
        base: "main".into(),
        base_ahead: 3,
        base_behind: 7,
        changed: 1,
        ..GitState::default()
    };
    f.trees.insert(wt.into(), st);
    d.folders.insert("/src/alpha".into(), f);
    let mut w = Worktree::default();
    w.path = wt.into();
    w.repo = "/src/alpha".into();
    w.branch = "fix".into();
    w.size = 300 << 20;
    w.checked = NOW;
    w.agents = vec!["a1".into()];
    d.wts = vec![w];
    d.agent_tmp
        .insert("/src/alpha/.claude/tmp".into(), 12 << 20);
    // Looked at, and empty (Go's peek reads the folder, not there, at once).
    for dir in ["/src/alpha/.claude/tmp", "/Users/me/Downloads"] {
        d.peeks.insert(
            dir.into(),
            PathInfo {
                at: NOW,
                ..PathInfo::default()
            },
        );
    }
    d.tmp = Scratch {
        items: 3,
        size: 4 << 20,
        stale_items: 1,
        stale: 1 << 20,
        checked: NOW,
    };
    d.checked = NOW;
    d.temp.insert(
        "d1".into(),
        TempWork {
            dirs: Vec::new(),
            bytes: 2 << 20,
            known: true,
        },
    );
    m
}

#[test]
fn rich_frames_match_go() {
    let cases: [(usize, Cfg, &str, &[&str]); 10] = [
        (160, classic(), "", &[]),
        (160, classic(), "", &["enter"]),
        (160, classic(), "", &["enter", "down"]),
        (140, classic(), "p/Users/me/Downloads", &[]),
        (140, classic(), PANE_TEMP, &[]),
        (140, classic(), PANE_TEMP, &["enter"]),
        (140, classic(), PANE_SYSTEM, &[]),
        (120, Cfg::default(), "", &[]),
        (100, Cfg::default(), "", &[]),
        (200, classic(), PANE_PRS, &[]),
    ];
    let want = oracle(include_str!("testdata/projects_rich.txt"));
    assert_eq!(want.len(), cases.len());
    for ((w, cfg, pick, keys), (name, want)) in cases.into_iter().zip(want) {
        let mut m = rich(w, cfg);
        if !pick.is_empty() {
            super::pick_in_projects(&mut m, pick);
        }
        for k in keys {
            press(&mut m, k);
        }
        same_as_go(&mut m, &name, &want);
    }
}

/// The bottom row's keys, as the frame draws them.
fn hint(m: &mut Model) -> String {
    let lines: Vec<String> = m.frame().iter().map(Line::plain).collect();
    let rule = lines
        .iter()
        .rposition(|l| l.starts_with("────"))
        .expect("the hint's rule");
    lines[rule + 1].trim().to_string()
}

#[test]
fn keys_move_and_hint_as_go() {
    let want = include_str!("testdata/projects_keys.txt");
    let (table, clean) = want.split_once("=== clean\n").unwrap();
    for row in table.lines() {
        let mut parts = row.split('\t');
        let (keys, sel, want_hint) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let mut m = rich(160, classic());
        m.frame();
        for k in keys.trim_matches(['[', ']']).split(' ') {
            press(&mut m, k);
            m.frame();
        }
        assert_eq!(data(&m).sel, sel, "{keys}");
        assert_eq!(hint(&mut m), want_hint, "{keys}");
    }
    // c opens Clean up over the page.
    let mut m = rich(160, classic());
    m.frame();
    press(&mut m, "c");
    let mut s = m.sheet.take().expect("Clean up");
    let got: Vec<String> = s
        .body(&m, 130, 40)
        .iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect();
    let mut got = got.join("\n") + "\n";
    for sp in crate::style::SPINNER {
        got = got.replace(&format!("─ {sp} fix"), "─ · fix");
    }
    assert_eq!(got, clean);
}

/// Up and down go from project to project in the list; enter goes into
/// the one picked, on the right, and left comes back (projects_test.go
/// TestProjectsMoveKeepsLevel).
#[test]
fn moving_keeps_level() {
    let mut m = model(three(), 120, 40, classic());
    m.frame();
    let step = |m: &mut Model, k: &str, want: &str| {
        press(m, k);
        m.frame();
        assert_eq!(data(m).sel, want, "{k}");
    };
    step(&mut m, "down", "p/src/beta");
    step(&mut m, "up", "p/src/alpha");
    step(&mut m, "enter", "p/src/alpha");
    assert!(hint(&mut m).contains("enter open"), "in alpha, on an agent");
    step(&mut m, "down", "p/src/alpha");
    step(&mut m, "left", "p/src/alpha");
    assert!(hint(&mut m).contains("enter into it"), "back in the list");
    press(&mut m, "esc");
    assert_eq!(m.place, PlaceId::Agents, "esc goes back to Agents");
}

/// Delete says what goes before anything does: the path, that git removes
/// it as a worktree, the branch that stays, and loudly, the files lost and
/// the commits only that branch keeps; y does nothing while an agent runs
/// in it (deletesheet_test.go TestDeleteSheetSaysWhatGoes).
#[test]
fn delete_says_what_goes() {
    use super::delete::{DeleteSheet, Doomed};
    use crate::frame::Press;
    use crate::frame::sheet::Sheet;
    let done = agent("done", "Fix the parser", "/r", "/r", 0, HOUR);
    let run = agent("run", "Still going", "/r", "/r", 7, 0);
    let mut m = model(vec![done, run], 140, 60, classic());
    let mut wt = Worktree::default();
    wt.path = "/r/.claude/worktrees/devclean".into();
    wt.repo = "/r".into();
    wt.branch = "worktree-devclean".into();
    wt.checked = NOW;
    wt.changed = 2;
    wt.unpushed = 1;
    wt.size = 5 << 20;
    wt.agents = vec!["done".into()];
    let mut s = DeleteSheet {
        items: vec![Doomed {
            wt: Some(wt),
            looked: true,
            files: vec![" M internal/ui/projects.go".into(), "?? notes.txt".into()],
            commits: vec!["abc1234 feat(ui): a thing".into()],
            ..Doomed::default()
        }],
        reading: false,
        back: None,
        scroll: 0,
    };
    let text = |s: &mut DeleteSheet, m: &Model| {
        s.body(m, 110, 50)
            .iter()
            .map(Line::plain)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = text(&mut s, &m);
    for want in [
        "/r/.claude/worktrees/devclean",
        "worktree · git removes it",
        "5M",
        "branch worktree-devclean stays",
        "2 uncommitted files LOST for good",
        "internal/ui/projects.go",
        "notes.txt",
        "1 commit not pushed or in main",
        "only worktree-devclean keeps them",
        "abc1234 feat(ui): a thing",
        "worked here: Fix the parser",
        "1 would lose work",
        "y delete, losing it",
    ] {
        assert!(body.contains(want), "lacks {want:?}:\n{body}");
    }
    s.items[0].wt.as_mut().unwrap().agents = vec!["run".into()];
    let body = text(&mut s, &m);
    assert!(
        body.contains("can't go: Still going is running in it") && !body.contains("y delete"),
        "a running agent should block it:\n{body}"
    );
    assert_ne!(
        s.key(&mut m, &Press::of("y").unwrap()),
        Out::Close,
        "y went ahead with an agent running in it"
    );
}

/// Done work goes by itself after the wait, counted from when it was put
/// away or last worked on, whichever is later; never while it runs, isn't
/// done, or the tidy-up is off; a worktree shared with recent work waits
/// for it (cleanup_test.go TestCleanupDue).
#[test]
fn done_work_is_due_after_its_wait() {
    let min = 60_000;
    let mut agents = Vec::new();
    let mut done = std::collections::HashMap::new();
    let mut add = |key: &str, put_away: Option<Ms>, active: Ms, pid: i32| {
        let (mut a, root) = agent(key, key, "/r", "/r", pid, active);
        a.job.state = if pid != 0 { State::Idle } else { State::Done };
        a.interactive = false;
        a.past = pid == 0;
        if let Some(t) = put_away {
            done.insert(key.to_string(), NOW - t);
        }
        agents.push((a, root));
    };
    add("old", Some(5 * HOUR), 6 * HOUR, 0);
    add("recent", Some(HOUR), 2 * HOUR, 0);
    add("running", Some(5 * HOUR), 6 * HOUR, 42);
    add("notdone", None, 9 * HOUR, 0);
    add("touched", Some(5 * HOUR), 30 * min, 0); // done long ago, but worked on since
    let mut m = model(agents, 120, 40, Cfg::default());
    m.kept::<Data>().done = done;
    let due = |m: &Model, keys: &[&str]| {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        due_in(m, data(m), &keys, NOW)
    };
    assert_eq!(due(&m, &["old"]), Some(0), "old");
    let d = due(&m, &["recent"]).expect("recent is due some time");
    assert!(
        (110 * min..=2 * HOUR).contains(&d),
        "recent: {d}, want about 2h"
    );
    let d = due(&m, &["touched"]).expect("touched is due some time");
    assert!(d >= 2 * HOUR, "touched: {d}, want 2h30m");
    for k in ["running", "notdone"] {
        assert_eq!(due(&m, &[k]), None, "{k} should never be due");
    }
    assert!(
        due(&m, &["old", "recent"]).is_some_and(|d| d > 0),
        "a worktree shared with a recent one waits for it"
    );
    m.kept::<Data>().cfg.cleanup_hours = -1;
    assert_eq!(due(&m, &["old"]), None, "off");
}

/// A worktree's row says what it has: its name once (not again as its
/// branch), its own commits, how far behind what it came from it is, who
/// runs in it and its size (worktreerow_test.go
/// TestWorktreeRowSaysWhatItHas).
#[test]
fn worktree_row_says_what_it_has() {
    let st = GitState {
        branch: "worktree-devclean".into(),
        base: "main".into(),
        base_ahead: 3,
        base_behind: 89,
        ..GitState::default()
    };
    let mut wt = Worktree::default();
    wt.size = 3 << 20;
    let got = super::view::worktree_line("/r/.claude/worktrees/devclean", &st, &wt, 1, 120).plain();
    for want in [
        "devclean",
        "3 commits",
        "main · 89 behind",
        "● 1 running",
        "3M",
    ] {
        assert!(got.contains(want), "row {got:?} lacks {want:?}");
    }
    assert!(
        !got.contains("worktree-devclean"),
        "row {got:?} repeats the name as its branch"
    );
}

/// An agent untouched for hours says it can go (projects_test.go
/// TestProjectsSayWhatCanGo, its first half: x hands the agent to the
/// list's own close, which isn't this module's).
#[test]
fn an_untouched_agent_says_it_can_go() {
    let old = agent(
        "old",
        "probe-codex",
        "/src/alpha",
        "/src/alpha",
        0,
        5 * HOUR,
    );
    let mut m = model(vec![old], 160, 40, classic());
    let page = body(&mut m);
    assert!(
        page.contains("can go"),
        "no 'can go' for an agent untouched 5h:\n{page}"
    );
    let fresh = agent("new", "probe-codex", "/src/alpha", "/src/alpha", 0, HOUR);
    let mut m = model(vec![fresh], 160, 40, classic());
    let page = body(&mut m);
    assert!(
        !page.contains("can go"),
        "'can go' for an agent touched an hour ago:\n{page}"
    );
}

/// Enter in Clean up hands what's ticked to Delete, and esc there comes
/// back to Clean up (deletesheet_test.go TestCleanSheetHandsToDelete).
#[test]
fn clean_up_hands_to_delete() {
    use super::clean::{CleanSheet, open_clean_sheet};
    use super::delete::DeleteSheet;
    let mut m = model(Vec::new(), 140, 50, classic());
    let d = m.kept::<Data>();
    d.checked = NOW;
    let mut wt = Worktree::default();
    wt.path = "/nowhere/.claude/worktrees/gone".into();
    wt.repo = "/nowhere".into();
    wt.checked = NOW;
    wt.size = 10;
    wt.own = true;
    d.wts = vec![wt];
    open_clean_sheet(&mut m);
    assert!(m.sheet_as::<CleanSheet>().is_some(), "Clean up didn't open");
    press(&mut m, "enter");
    let d = m.sheet_as::<DeleteSheet>().expect("enter opened Delete");
    assert!(
        d.items.len() == 1 && d.items[0].wt.is_some(),
        "Delete has {} items",
        d.items.len()
    );
    press(&mut m, "esc");
    assert!(
        m.sheet_as::<CleanSheet>().is_some(),
        "esc didn't come back to Clean up"
    );
}

/// A rush session's temp work is the folder rush gives it first, then its
/// agent's scratch (fleet/temp.go:27-40 TempDirs): Temporary counts it,
/// worked out by rush_fleet's own TempDirs off the UI thread.
#[test]
fn a_rush_sessions_own_temp_folder() {
    let mut m = Model::new("t");
    let mut a = Agent {
        key: "default/a:e57fb8f4".into(),
        rush: true,
        ..Agent::default()
    };
    a.job.id = "e57fb8f4".into();
    let mut b = a.clone();
    b.key = "default/b".into();
    b.rush = false;
    m.agents = vec![a, b];
    super::scan::measure_temp(&mut m);
    let d = m.kept::<Data>();
    let dirs = &d.temp["default/a:e57fb8f4"].dirs;
    assert_eq!(dirs[0].path, rush_fleet::temp_dir("e57fb8f4"));
    assert!(dirs[0].keep);
    assert!(!d.listing_temp, "still working them out");
    // Not a rush session: none of rush's.
    assert!(d.temp.get("default/b").is_none_or(|t| {
        t.dirs
            .iter()
            .all(|d| d.path != rush_fleet::temp_dir("e57fb8f4"))
    }));
}

/// x on an agent in a project always asks, idle or not: y hides it, z
/// stops it and keeps it, r restarts it (projects.go:1034 askClose); the
/// agent isn't hidden by the key alone.
#[test]
fn x_on_an_agent_asks_first() {
    let mut idle = agent("a1", "fix alpha", "/src/alpha", "/src/alpha", 0, 0);
    idle.0.interactive = false;
    let mut m = model(vec![idle], 120, 40, classic());
    m.frame();
    press(&mut m, "enter");
    m.frame();
    press(&mut m, "down");
    m.frame();
    assert!(hint(&mut m).contains("enter open"), "on the agent");
    // g is keep going on it, as ctrl+b (projects.go:1020).
    press(&mut m, "g");
    let f = m.flash.clone().map(|f| f.0).unwrap_or_default();
    assert!(!f.contains("isn't in this rush"), "{f}");
    m.flash = None;
    press(&mut m, "x");
    let c = m.confirm.as_ref().expect("asked");
    assert_eq!(c.question, "Hide fix alpha?");
    assert!(c.detail.contains("z just stops it"), "{}", c.detail);
    assert!(m.agents.iter().any(|a| a.key == "a1"), "still listed");
}

/// What agents left outside their projects shows in Clean up, ticked when
/// it's untouched scratch in a temp folder (cleansheet.go:72-83), and on
/// its project's page as Left outside (projects.go:584-598).
#[test]
fn left_outside_is_shown_and_offered() {
    let (a1, root) = agent("a1", "fix alpha", "/src/alpha", "/src/alpha", 0, 0);
    let mut m = model(vec![(a1.clone(), root)], 160, 40, classic());
    {
        let d = m.kept::<Data>();
        d.checked = NOW;
        d.outside = vec![
            rush_fleet::LeftItem {
                path: "/tmp/wavec-msg.sh".into(),
                agent: a1.clone(),
                temp: true,
                size: 2048,
                ..Default::default()
            },
            rush_fleet::LeftItem {
                path: "/Users/someone/.config/sbtest.txt".into(),
                agent: a1,
                size: 10,
                ..Default::default()
            },
        ];
    }
    m.frame();
    press(&mut m, "enter");
    let b = body(&mut m);
    assert!(
        b.contains("LEFT OUTSIDE  2 · 2K · c cleans up what's scratch"),
        "{b}"
    );
    assert!(b.contains("/tmp/wavec-msg.sh"), "{b}");
    assert!(b.contains("fix alpha · scratch"), "{b}");
    press(&mut m, "c");
    let mut s = m.sheet.take().expect("Clean up");
    let got: String = s
        .body(&m, 130, 40)
        .iter()
        .map(|l| l.plain() + "\n")
        .collect();
    assert!(got.contains("Left outside projects"), "{got}");
    assert!(got.contains("✓ /tmp/wavec-msg.sh"), "{got}");
    assert!(got.contains("scratch, untouched since"), "{got}");
    assert!(got.contains("outside temp: may be meant to stay"), "{got}");
}
