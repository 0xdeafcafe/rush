//! The tasks beside a turn and messages between agents, as events fold
//! them in: after internal/convo/jobs_test.go (TestJobs,
//! TestWokenSubagentRunsAgain, TestWokenByABackgroundTask,
//! TestWokenByAHeredoc, TestWokenByATaskThatEndedMidTurn) and
//! exchange_test.go.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::plain;
use rush_convo::{Item, Options, Renderer, Session, exchange_peer};
use rush_core::event::{
    self, Background, BackgroundTask, Event, Exchange, Peer, TaskDone, TaskKind, TaskStarted,
    TaskUpdated,
};

const MIN: i64 = 60_000;

fn started(id: &str, call: &str, kind: TaskKind, label: &str, bg: bool) -> Event {
    Event::TaskStarted(TaskStarted {
        id: id.into(),
        call_id: call.into(),
        kind,
        label: label.into(),
        background: bg,
        ..TaskStarted::default()
    })
}

fn done(id: &str, call: &str, status: &str, summary: &str) -> Event {
    Event::TaskDone(TaskDone {
        id: id.into(),
        call_id: call.into(),
        status: status.into(),
        summary: summary.into(),
        ..TaskDone::default()
    })
}

fn background(ids: &[(&str, TaskKind)]) -> Event {
    Event::Background(Background {
        tasks: ids
            .iter()
            .map(|(id, kind)| BackgroundTask {
                id: (*id).into(),
                kind: *kind,
                ..BackgroundTask::default()
            })
            .collect(),
    })
}

fn turn_end() -> Event {
    Event::TurnEnd(event::TurnEnd {
        reason: "done".into(),
        ..event::TurnEnd::default()
    })
}

fn delta(text: &str) -> Event {
    Event::Delta(event::Delta {
        kind: event::PartKind::Text,
        text: text.into(),
        ..event::Delta::default()
    })
}

#[test]
fn jobs() {
    let mut s = Session::new();
    let cmd = "for i in $(seq 1 30); do echo tick $i; sleep 1; done";
    s.apply(&started("b1", "toolu_01", TaskKind::Shell, cmd, false), 1);
    let j = s.job("b1").unwrap();
    assert!(j.running() && !j.background && j.kind() == "shell" && j.tool_use_id == "toolu_01");
    s.apply(&background(&[("b1", TaskKind::Shell)]), 1);
    s.apply(
        &Event::TaskUpdated(TaskUpdated {
            id: "b1".into(),
            background: Some(true),
            ..TaskUpdated::default()
        }),
        1,
    );
    let j = s.job("b1").unwrap();
    assert!(j.background && j.running(), "backgrounded: {j:?}");
    s.apply(&background(&[]), 1);
    s.apply(
        &Event::TaskUpdated(TaskUpdated {
            id: "b1".into(),
            status: "killed".into(),
            ..TaskUpdated::default()
        }),
        1,
    );
    s.apply(&done("b1", "toolu_01", "stopped", cmd), 1);
    assert_eq!(s.job("b1").unwrap().status, "stopped");
    assert_eq!(s.task_status.get("b1").map(|s| &**s), Some("stopped"));
    // Its call isn't in the conversation, and its name is a command: a
    // step stands in for it.
    assert!(s.made_call("toolu_01"));

    s.apply(&background(&[("b2", TaskKind::Shell)]), 1);
    s.apply(
        &started("b2", "toolu_02", TaskKind::Shell, "tick loop", true),
        1,
    );
    let run: Vec<_> = s.running_jobs().map(|j| j.id.as_str()).collect();
    assert_eq!(run, ["b2"]);
    // A turn ending doesn't end what runs in the background.
    s.apply(&turn_end(), 2);
    assert_eq!(s.running_jobs().count(), 1);
}

#[test]
fn woken_subagent_runs_again() {
    let mut s = Session::new();
    s.apply(&started("a1", "tA", TaskKind::Subagent, "", true), 1);
    s.apply(&done("a1", "tA", "completed", ""), 1);
    assert!(!s.job("a1").unwrap().running());
    assert_eq!(s.task_status.get("a1").map(|s| &**s), Some("completed"));
    s.apply(&started("a1", "tS", TaskKind::Subagent, "", true), 1 + MIN);
    let j = s.job("a1").unwrap();
    assert!(j.running() && j.end == 0 && !s.task_status.contains_key("a1"));
    // Still the run its Agent call started.
    assert_eq!(j.tool_use_id, "tA");
    assert!(s.subagent_job("", "tA").is_some());
    s.apply(&done("a1", "", "completed", ""), 2 * MIN);
    s.apply(&background(&[("a1", TaskKind::Subagent)]), 3 * MIN);
    assert!(s.job("a1").unwrap().running());
}

#[test]
fn woken_by_a_background_task() {
    let mut s = Session::new();
    s.sent("run the tests in the background".into(), Vec::new(), 1);
    s.apply(
        &started("b1", "toolu_09", TaskKind::Shell, "go test ./...", true),
        1,
    );
    s.apply(&turn_end(), 1);
    s.apply(
        &done("b1", "toolu_09", "completed", "go test ./..."),
        1 + MIN,
    );
    s.apply(&delta("The tests pass."), 1 + MIN + 1000);
    assert_eq!(s.turns.len(), 2);
    let t = &s.turns[1];
    assert_eq!(t.from, "background shell · completed");
    assert_eq!(t.cause, "go test ./...");
}

#[test]
fn woken_by_a_heredoc() {
    let mut s = Session::new();
    s.sent("go".into(), Vec::new(), 1);
    let cmd = "python3 - <<'EOF'\nif x:\n    print(1)\nEOF";
    s.apply(&started("b1", "toolu_09", TaskKind::Shell, cmd, true), 1);
    s.apply(&turn_end(), 1);
    s.apply(&done("b1", "toolu_09", "completed", ""), 1 + MIN);
    s.apply(&delta("done"), 1 + MIN + 1000);
    let t = s.turns.last().unwrap();
    assert_eq!(t.command, cmd);
    assert_eq!(t.cause, "python3 - <<'EOF'");
}

#[test]
fn woken_by_a_task_that_ended_mid_turn() {
    let mut s = Session::new();
    s.sent("run the tests in the background".into(), Vec::new(), 1);
    s.apply(
        &started("b1", "toolu_09", TaskKind::Shell, "go test ./...", true),
        1,
    );
    s.apply(&done("b1", "toolu_09", "completed", "go test ./..."), 1);
    s.apply(&turn_end(), 5 * MIN);
    s.apply(&delta("The tests pass."), 5 * MIN + 1000);
    assert_eq!(s.turns.len(), 2);
    assert_eq!(s.turns[1].cause, "go test ./...");
    // Your next message starts afresh: nothing held is put down to it.
    s.apply(&turn_end(), 6 * MIN);
    s.sent("thanks".into(), Vec::new(), 6 * MIN);
    assert_eq!(s.turns.last().unwrap().cause, "");
}

#[test]
fn foreground_jobs_end_with_their_turn() {
    let mut s = Session::new();
    s.sent("go".into(), Vec::new(), 1);
    s.apply(&started("b1", "", TaskKind::Shell, "build", false), 1);
    s.apply(&turn_end(), 2);
    assert_eq!(s.job("b1").unwrap().status, "completed");
}

fn peer(id: &str, kind: &str, name: &str) -> Peer {
    Peer {
        session_id: id.into(),
        kind: kind.into(),
        name: name.into(),
    }
}

fn render(s: &Session, o: &Options) -> String {
    plain(&Renderer::new().render(s, o))
}

#[test]
fn exchange_replay_folds_and_keeps_origin() {
    let mut s = Session::new();
    let e = Exchange {
        id: "msg".into(),
        direction: "received".into(),
        phase: "message".into(),
        text: format!("first line\n{}last line", "detail ".repeat(120)),
        sender: peer("from", "codex", "Reviewer"),
        receiver: peer("to", "claude", "Builder"),
        ..Exchange::default()
    };
    s.apply(&Event::Exchange(Box::new(e.clone())), 1000);
    s.sent_exchange(&e, 2000);
    s.apply(
        &Event::Message(event::Message {
            role: "user".into(),
            parts: vec![event::Part {
                kind: event::PartKind::Text,
                text: e.text.clone(),
                ..event::Part::default()
            }],
            ..event::Message::default()
        }),
        3000,
    );
    s.apply(&Event::TurnEnd(event::TurnEnd::default()), 4000);
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].items.len(), 1);
    assert!(s.turns[0].prompt.is_empty());
    assert!(matches!(s.turns[0].items[0], Item::Exchange(_)));

    let r = "exchange:received:msg:from";
    let o = Options {
        width: 180,
        verbose: true,
        now: 5000,
        ..Options::default()
    };
    let open = render(&s, &o);
    for want in ["◇ Reviewer from", "◇ Builder to", "last line"] {
        assert!(open.contains(want), "missing {want:?} in {open}");
    }
    let mut o = o;
    o.open.insert(r.into(), false);
    assert!(!render(&s, &o).contains("last line"), "collapse ignored");
    assert_eq!(exchange_peer(r), Some("from"));
    let copied = s.exchange_text(r).unwrap();
    assert!(copied.contains(&e.text) && copied.contains("Reviewer"));
}

#[test]
fn report_only_exchange_opens_no_live_turn() {
    let mut s = Session::new();
    let e = Exchange {
        id: "result".into(),
        direction: "received".into(),
        phase: "result".into(),
        text: "done".into(),
        ..Exchange::default()
    };
    s.apply(&Event::Exchange(Box::new(e)), 1000);
    assert!(s.live().is_none());
}

#[test]
fn archived_exchange_replaces_the_agents_copy() {
    let mut s = Session::new();
    let user = |t: &str| {
        Event::Message(event::Message {
            role: "user".into(),
            parts: vec![event::Part {
                kind: event::PartKind::Text,
                text: t.into(),
                ..event::Part::default()
            }],
            ..event::Message::default()
        })
    };
    s.apply(&user("review this"), 1000);
    s.apply(&say_event("done"), 2000);
    s.apply(&Event::TurnEnd(event::TurnEnd::default()), 3000);
    let e = Exchange {
        id: "old".into(),
        at: 1000,
        direction: "received".into(),
        phase: "message".into(),
        text: "review this".into(),
        sender: peer("", "codex", "Reviewer"),
        ..Exchange::default()
    };
    s.restore_exchanges(std::slice::from_ref(&e));
    s.restore_exchanges(std::slice::from_ref(&e));
    assert_eq!(s.turns.len(), 1);
    assert!(s.turns[0].prompt.is_empty() && s.live().is_none());
    let n = s.turns[0]
        .items
        .iter()
        .filter(|i| matches!(i, Item::Exchange(_)))
        .count();
    assert_eq!(n, 1);

    // Identical words far apart in time aren't taken for it.
    for when in [0, 1000 + 24 * 60 * MIN] {
        let mut s = Session::new();
        s.apply(&user("yes"), 1000);
        s.apply(&Event::TurnEnd(event::TurnEnd::default()), 2000);
        s.restore_exchanges(&[Exchange {
            id: "later".into(),
            at: when,
            direction: "received".into(),
            text: "yes".into(),
            ..Exchange::default()
        }]);
        assert_eq!(s.turns[0].prompt, "yes");
    }
}

#[test]
fn opened_exchange_offers_to_fold() {
    let mut s = Session::new();
    let e = Exchange {
        id: "markdown".into(),
        direction: "received".into(),
        text: "## Heading\n\n- one\n- two\n\n| Name | State |\n|---|---|\n| rush | ready |".into(),
        ..Exchange::default()
    };
    s.apply(&Event::Exchange(Box::new(e)), 1000);
    let out = render(
        &s,
        &Options {
            width: 80,
            verbose: true,
            now: 2000,
            selected: "exchange:received:markdown:".into(),
            ..Options::default()
        },
    );
    assert!(!out.contains("space expands"), "{out}");
    assert!(out.contains("space folds"), "{out}");
}

fn say_event(text: &str) -> Event {
    Event::Message(event::Message {
        role: "assistant".into(),
        parts: vec![event::Part {
            kind: event::PartKind::Text,
            text: text.into(),
            ..event::Part::default()
        }],
        ..event::Message::default()
    })
}
