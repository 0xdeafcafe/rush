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
    let rows: Vec<String> = (0..2)
        .map(|i| line_of(&subs, false, i, 80, 7, NOW, 0).plain())
        .collect();
    assert!(
        rows[0].starts_with("├ ") && rows[0].contains("find the render loop · Explore"),
        "{rows:?}"
    );
    assert!(rows[0].trim_end().ends_with("3m"), "{rows:?}");
    assert!(
        rows[1].starts_with("└ ") && rows[1].contains("bench the frame rate · general-purpose")
    );
    assert_eq!(rows[0].chars().count(), 80);
    for i in 0..4 {
        subs.push(tile(&format!("x{i}"), "", "more", 0));
    }
    assert_eq!(shown(false, subs.len()), (3, 3));
    assert_eq!(
        line_of(&subs, false, 3, 80, 7, NOW, 0).plain().trim_end(),
        "└ 3 more subagents"
    );
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
        win(
            "named",
            "run() { claude -p \"$1\"; }; run hi &",
            "",
            NOW - MIN,
            NOW + MIN,
        ),
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
    assert_eq!(
        claim(&wins, "codex", "review the diff", NOW, true),
        "tool-a"
    );
    assert_eq!(claim(&wins, "codex", "hi", NOW, false), "shell");
    assert!(names("/usr/local/bin/claude -p x", "claude") && !names("claudette", "claude"));
    assert!(hosted_live("working") && !hosted_live("idle") && !hosted_live("stopped"));
}

#[test]
fn shell_command_argv() {
    use crate::screens::projects_machine::shells::shell_command;
    for (args, want) in [
        (
            &["/bin/zsh", "-lc", "go build && go test"][..],
            "go build && go test",
        ),
        (
            &["bash", "--noprofile", "-c", "echo 'hello world'"],
            "echo 'hello world'",
        ),
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
    use crate::screens::projects_machine::shells::*;
    assert_eq!(pause_word("call-x"), "pause it");
    note_paused("call-x", PausedPart::default());
    assert_eq!(pause_word("call-x"), "let it go on");
    assert!(paused_ids().contains("call-x"));
    assert!(take_paused("call-x").is_some());
    assert_eq!(pause_word("call-x"), "pause it");
}

#[test]
fn list_toggle_and_stuck_words() {
    use super::hosted::list_toggle_key;
    assert!(list_toggle_key("ctrl+6") && list_toggle_key("ctrl+^") && !list_toggle_key("ctrl+7"));
    assert!(super::stuck::nudge_text("find the loop", true).contains(
        "\"find the loop\" has written nothing for 5 minutes. rush moved it to the background"
    ));
    let unmoved = super::stuck::nudge_text("find the loop", false);
    assert!(
        unmoved.contains(
            "\"find the loop\" has written nothing for 5 minutes: look at what it has done so far."
        ) && !unmoved.contains("background"),
        "{unmoved}"
    );
    assert!(
        super::stuck::ask_text(31 * MIN).starts_with("You have written nothing for 31 minutes.")
    );
    assert_eq!(
        super::stuck::nudge_done("ann", true),
        "ann's subagent went quiet · moved to the background, ann checks it"
    );
    assert_eq!(
        super::stuck::nudge_done("ann", false),
        "ann's subagent went quiet · ann checks it"
    );
}

mod nudges {
    use std::path::Path;
    use std::sync::Arc;

    use rush_core::agent::{Agent, State, SubagentTile, Task};

    use super::super::stuck::{nudge_done, nudge_stuck_subs, nudge_text};
    use super::super::tests::{MIN, NOW, agent, model_of};
    use crate::model::Model;
    use crate::screens::session_core::host::{Fake, Op, set_hosts};

    /// A rush session at work whose transcript holds the call t1, and its
    /// run s1 ("find the loop") on that call, quiet for quiet.
    fn session(dir: &Path, quiet: i64) -> Agent {
        let main = dir.join("s.jsonl");
        std::fs::write(
            &main,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"description":"find the loop"}}]}}"#.to_string() + "\n",
        )
        .unwrap();
        let mut a = agent("h1", "ann", State::Working, 4242);
        a.rush = true;
        a.interactive = false;
        a.job.transcript_path = main.to_string_lossy().into_owned();
        a.subagents = vec![SubagentTile {
            id: "s1".into(),
            kind: "general-purpose".into(),
            description: "find the loop".into(),
            tool_use_id: "t1".into(),
            depth: 1,
            mod_time: NOW - quiet,
            ..SubagentTile::default()
        }];
        a
    }

    fn rush(a: Agent) -> (Model, Arc<Fake>) {
        let mut m = model_of(vec![a], 120, 40);
        m.follows = false;
        let f = Fake::new();
        set_hosts(&mut m, Arc::new(f.clone()));
        (m, f)
    }

    fn last_said(m: &Model) -> String {
        m.said().last().map(|l| l.text.clone()).unwrap_or_default()
    }

    // Quiet past five minutes, waiting on the model: the call the session
    // waits on goes to the background, then the session is told so. Once.
    #[test]
    fn a_quiet_run_is_moved_and_its_session_told() {
        let d = tempfile::tempdir().unwrap();
        let (mut m, f) = rush(session(d.path(), 6 * MIN));
        nudge_stuck_subs(&mut m);
        assert_eq!(
            f.ops(),
            vec![
                Op::Background { id: "t1".into() },
                Op::Guide {
                    text: nudge_text("find the loop", true),
                    images: Vec::new()
                },
            ]
        );
        assert_eq!(last_said(&m), nudge_done("ann", true));
        nudge_stuck_subs(&mut m);
        assert_eq!(f.ops().len(), 2, "told twice");
    }

    // The background move failed: the session is told without the claim
    // that rush moved it.
    #[test]
    fn a_failed_move_isnt_claimed() {
        let d = tempfile::tempdir().unwrap();
        let (mut m, f) = rush(session(d.path(), 6 * MIN));
        *f.refuse_background.lock().unwrap() = Some("its host can't".into());
        nudge_stuck_subs(&mut m);
        let ops = f.ops();
        assert_eq!(
            ops,
            vec![Op::Guide {
                text: nudge_text("find the loop", false),
                images: Vec::new()
            }]
        );
        let Op::Guide { text, .. } = &ops[0] else {
            unreachable!()
        };
        assert!(!text.contains("moved"), "{text}");
        assert_eq!(last_said(&m), nudge_done("ann", false));
    }

    // A run another run started (the parity driver's own subagent): its
    // call isn't the session's, which neither waits on it nor can move it.
    // The session is told nothing.
    #[test]
    fn a_nested_run_isnt_the_sessions() {
        let d = tempfile::tempdir().unwrap();
        let mut a = session(d.path(), 6 * MIN);
        a.subagents[0].depth = 2;
        let (mut m, f) = rush(a);
        nudge_stuck_subs(&mut m);
        assert!(f.ops().is_empty(), "{:?}", f.ops());
        assert_eq!(*f.dials.lock().unwrap(), 0);
        // Its meta saying nothing of depth, its call in another run's
        // transcript is enough.
        let mut a = session(d.path(), 6 * MIN);
        a.subagents[0].tool_use_id = "t-in-the-driver".into();
        let (mut m, f) = rush(a);
        nudge_stuck_subs(&mut m);
        assert!(f.ops().is_empty(), "{:?}", f.ops());
        assert!(!last_said(&m).contains("subagent"), "{}", last_said(&m));
    }

    // Waiting on a command it moved to the background, which the session
    // still runs: waiting as in a tool, not stuck before 30 minutes.
    #[test]
    fn a_run_waiting_on_its_background_command() {
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join("agent-s1.jsonl");
        let lines = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"c1","name":"Bash","input":{"command":"sleep 600","run_in_background":true}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"Command running in background with ID: bq81z. Output is being written to: /tmp/bq81z.output"}]}}"#,
        ];
        std::fs::write(&run, lines.join("\n") + "\n").unwrap();
        let with = |quiet: i64| {
            let mut a = session(d.path(), quiet);
            a.subagents[0].path = run.to_string_lossy().into_owned();
            a.job.running = vec![Task {
                id: "bq81z".into(),
                kind: "shell".into(),
                label: "sleep 600".into(),
                started_at: NOW - quiet,
            }];
            a
        };
        let (mut m, f) = rush(with(6 * MIN));
        nudge_stuck_subs(&mut m);
        assert!(f.ops().is_empty(), "6m: {:?}", f.ops());
        let (mut m, f) = rush(with(29 * MIN));
        nudge_stuck_subs(&mut m);
        assert!(f.ops().is_empty(), "29m: {:?}", f.ops());
        let (mut m, f) = rush(with(31 * MIN));
        nudge_stuck_subs(&mut m);
        assert_eq!(f.ops().len(), 2, "31m: {:?}", f.ops());
    }

    // A rush that follows leaves it to the one that leads: two would tell
    // the session twice.
    #[test]
    fn following_tells_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (mut m, f) = rush(session(d.path(), 6 * MIN));
        m.follows = true;
        nudge_stuck_subs(&mut m);
        assert!(f.ops().is_empty());
        assert_eq!(*f.dials.lock().unwrap(), 0);
        m.follows = false;
        nudge_stuck_subs(&mut m);
        assert_eq!(f.ops().len(), 2, "leading again: {:?}", f.ops());
    }

    // A run a restart killed is no running tile (rush-fleet's loader): with
    // none quiet, nothing is sent, nor its session dialled.
    #[test]
    fn a_killed_run_tells_nothing() {
        let d = tempfile::tempdir().unwrap();
        let mut a = session(d.path(), 6 * MIN);
        a.subagents.clear();
        let (mut m, f) = rush(a);
        nudge_stuck_subs(&mut m);
        assert!(f.ops().is_empty() && *f.dials.lock().unwrap() == 0);
    }
}
