//! Subagent rows, spawned agents' steps, shells and the stuck nudges,
//! after Go's internal/ui subrows_test.go, spawns_test.go (TestClaim*),
//! shells_test.go (TestShellCommandArgv) and hosted_test.go
//! (TestListToggle).

use rush_core::agent::State;

use super::spawns::{Window, claim, hosted_live, names};
use super::subs::*;
use super::tests::{MIN, NOW, agent, model_of};

fn tile(id: &str, typ: &str, what: &str, born: i64) -> SubTile {
    SubTile {
        id: id.into(),
        typ: typ.into(),
        description: what.into(),
        born,
    }
}

/// Running subagents get rows under their agent; past four the last
/// counts the rest; ← tucks them away.
#[test]
fn subagent_rows() {
    let mut subs = vec![
        tile("s1", "Explore", "find the render loop", NOW - 3 * MIN),
        tile("s2", "general-purpose", "bench the frame rate", 0),
    ];
    let rows: Vec<String> = (0..2).map(|i| line_of(&subs, false, i, 80, 7, NOW, 0).plain()).collect();
    assert!(rows[0].starts_with("├ ") && rows[0].contains("find the render loop · Explore"), "{rows:?}");
    assert!(rows[0].trim_end().ends_with("3m"), "{rows:?}");
    assert!(rows[1].starts_with("└ ") && rows[1].contains("bench the frame rate · general-purpose"));
    assert_eq!(rows[0].chars().count(), 80);
    for i in 0..4 {
        subs.push(tile(&format!("x{i}"), "", "more", 0));
    }
    assert_eq!(shown(false, subs.len()), (3, 3));
    assert_eq!(line_of(&subs, false, 3, 80, 7, NOW, 0).plain().trim_end(), "└ 3 more subagents");
    assert_eq!(shown(true, subs.len()), (0, 0));

    let mut m = model_of(vec![agent("a1", "one", State::Working, 1)], 120, 40);
    assert_eq!(rows_of(&m, 6), 4);
    toggle(&mut m, "default/a1");
    assert_eq!(rows_of(&m, 6), 0);
    toggle(&mut m, "default/a1");
    assert_eq!(rows_of(&m, 6), 4);
    assert!(wide(150, false, 1) && !wide(149, false, 1) && !wide(200, true, 1));
    assert_eq!((list_width(150), list_width(300)), (60, 72));
}

fn rows_of(m: &crate::model::Model, n: usize) -> usize {
    super::subs::rows(m, "default/a1", n)
}

fn win(step: &str, command: &str, asked: &str, from: i64, to: i64) -> Window {
    Window {
        step: step.into(),
        command: command.into(),
        asked: asked.into(),
        from,
        to,
    }
}

#[test]
fn claim_outside_windows() {
    let wins = [win("a", "claude -p hi", "", NOW - 60 * MIN, NOW - 50 * MIN)];
    assert_eq!(claim(&wins, "claude", "", NOW, false), "");
}

/// Of two overlapping steps the one naming the program claims, else the
/// later; while a subagent works, none.
#[test]
fn claim_overlap() {
    let mut wins = vec![
        win("named", "run() { claude -p \"$1\"; }; run hi &", "", NOW - MIN, NOW + MIN),
        win("later", "go test ./...", "", NOW - 1_000, NOW + MIN),
    ];
    assert_eq!(claim(&wins, "claude", "", NOW, false), "named");
    wins[0].command = "make lint".into();
    assert_eq!(claim(&wins, "claude", "", NOW, false), "later");
    assert_eq!(claim(&wins, "claude", "", NOW, true), "");
}

/// A spawn_agent call claims the agent it asked, and only that one.
#[test]
fn claim_spawn_tool() {
    let wins = [
        win("shell", "codex exec hi", "", NOW - MIN, NOW + MIN),
        win("tool-a", "", "review the diff", NOW - 2_000, NOW + MIN),
        win("tool-b", "", "write the docs", NOW - 1_000, NOW + MIN),
    ];
    assert_eq!(claim(&wins, "codex", "review the diff", NOW, true), "tool-a");
    assert_eq!(claim(&wins, "codex", "hi", NOW, false), "shell");
    assert!(names("/usr/local/bin/claude -p x", "claude") && !names("claudette", "claude"));
    assert!(hosted_live("working") && !hosted_live("idle") && !hosted_live("stopped"));
}

#[test]
fn shell_command_argv() {
    use super::shells::shell_command;
    for (args, want) in [
        (&["/bin/zsh", "-lc", "go build && go test"][..], "go build && go test"),
        (&["bash", "--noprofile", "-c", "echo 'hello world'"], "echo 'hello world'"),
        (&["sh", "script.sh", "-c", "unrelated"], ""),
        (&["node", "-c", "unrelated"], ""),
        (&["bash", "--rcfile", "profile", "-c", "unrelated"], ""),
        (&["bash", "--", "-c", "unrelated"], ""),
    ] {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        assert_eq!(shell_command(&args), want, "{args:?}");
    }
}

#[test]
fn pause_words() {
    use super::shells::*;
    assert_eq!(pause_word("call-x"), "pause it");
    pause("call-x", Paused::default());
    assert_eq!(pause_word("call-x"), "let it go on");
    assert!(paused_ids().contains("call-x"));
    assert!(resume("call-x").is_some());
    assert_eq!(pause_word("call-x"), "pause it");
}

#[test]
fn list_toggle_and_stuck_words() {
    use super::hosted::list_toggle_key;
    assert!(list_toggle_key("ctrl+6") && list_toggle_key("ctrl+^") && !list_toggle_key("ctrl+7"));
    assert!(super::stuck::nudge_text("find the loop").contains("\"find the loop\" has written nothing for 5 minutes"));
    assert!(super::stuck::ask_text(31 * MIN).starts_with("You have written nothing for 31 minutes."));
    assert_eq!(super::stuck::nudge_done("ann"), "ann's subagent went quiet · moved to the background, ann checks it");
}
