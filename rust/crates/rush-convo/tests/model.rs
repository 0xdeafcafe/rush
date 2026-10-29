//! The session model, fed events: after internal/convo/convo_test.go
//! (TestModel, TestStreamingAndCrash, TestTasks, TestSubagentNumbers),
//! neutral_test.go (TestSubagentCallMidStreamKeepsWordsOnce,
//! TestCompactAsksAreNotYours) and compactby.go's CompactedPrompt.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::{Item, Session, Status};
use rush_core::event::{self, Event};
use rush_core::usage::TokenUsage;
use serde_json::json;

mod common;

use common::*;

/// Two turns: a finished one with clean steps and an answer, and a live one
/// with a failed command, a write and a subagent.
fn session() -> Session {
    let mut s = Session::new();
    let evs = vec![
        (0, sent("the ux right now is totally broken when i attach")),
        (1, say("Looking at how attach restores the terminal modes.")),
        (
            2,
            tool_use(
                "r1",
                "Read",
                json!({"file_path": "/work/rush/internal/daemon/attach.go"}),
            ),
        ),
        (
            3,
            tool_result(
                "r1",
                "…",
                false,
                Some(
                    json!({"type": "text", "file": {"numLines": 166, "startLine": 1, "totalLines": 166}}),
                ),
            ),
        ),
        (
            3,
            tool_use(
                "g1",
                "Grep",
                json!({"pattern": "1049h", "path": "/work/rush/internal"}),
            ),
        ),
        (
            4,
            tool_result(
                "g1",
                "internal/daemon/attach.go\ninternal/ui/live.go",
                false,
                None,
            ),
        ),
        (
            4,
            tool_use("b1", "Bash", json!({"command": "go build ./..."})),
        ),
        (
            6,
            tool_result("b1", "", false, Some(json!({"stdout": "", "stderr": ""}))),
        ),
        (
            7,
            tool_use(
                "e1",
                "Edit",
                json!({"file_path": "/work/rush/internal/daemon/attach.go"}),
            ),
        ),
        (
            8,
            tool_result(
                "e1",
                "ok",
                false,
                Some(
                    json!({"structuredPatch": [{"oldStart": 60, "oldLines": 2, "newStart": 60, "newLines": 3,
                    "lines": [" \tfor _, m := range info.DecModes {", "-\t\tfmt.Fprint(out, x)", "+\t\tfmt.Fprint(out, y)", "+\t\tfmt.Fprint(out, z)"]}]}),
                ),
            ),
        ),
        (
            9,
            say("Fixed the **alt screen**. Attach now clears to the alternate screen first."),
        ),
        (10, done(0.52)),
        (20, sent("add modern key stuff to input too")),
        (21, thinking("hmm")),
        (
            22,
            tool_use(
                "b2",
                "Bash",
                json!({"command": "cd /work/rush/internal/ui && go vet ./... 2>&1 | head -50"}),
            ),
        ),
        (
            23,
            tool_result(
                "b2",
                "Exit code 1\ninternal/ui/editor.go:41:2: unreachable code",
                true,
                Some(
                    json!({"stdout": "internal/ui/editor.go:41:2: unreachable code", "stderr": ""}),
                ),
            ),
        ),
        (
            24,
            tool_use(
                "w1",
                "Write",
                json!({"file_path": "/work/rush/internal/ui/editor.go", "content": "package ui\n"}),
            ),
        ),
        (
            25,
            tool_result(
                "w1",
                "ok",
                false,
                Some(json!({"type": "create", "content": "package ui\n"})),
            ),
        ),
        (
            26,
            tool_use(
                "a1",
                "Task",
                json!({"subagent_type": "Explore", "description": "find the preview pane"}),
            ),
        ),
        (
            27,
            sub_tool_use("a1", "a1g", "Grep", json!({"pattern": "previewLines"})),
        ),
        (
            28,
            tool_use("b3", "Bash", json!({"command": "go test -race ./..."})),
        ),
        (30, delta("Building the line editor")),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    s
}

fn step<'a>(s: &'a Session, id: &str) -> &'a rush_convo::Step {
    s.step(s.step_by_call(id).unwrap_or_else(|| panic!("no step {id}")))
}

#[test]
fn model() {
    let s = session();
    assert_eq!(s.turns.len(), 2);
    let (done, live) = (&s.turns[0], &s.turns[1]);
    assert!(!done.live && live.live);
    assert_eq!(
        done.outcome(),
        "Fixed the alt screen. Attach now clears to the alternate screen first."
    );
    assert_eq!(done.cost, 0.52);
    assert_eq!(done.step_count(), 4);
    let b2 = step(&s, "b2");
    assert_eq!((b2.status, b2.exit), (Status::Failed, 1));
    let b1 = step(&s, "b1");
    assert_eq!((b1.status, b1.exit), (Status::Ok, 0));
    let a1 = step(&s, "a1");
    assert_eq!(a1.children.len(), 1);
    assert_eq!(&*s.step(a1.children[0]).id, "a1g");
    // The subagent's step is in the turn, but not one of its items.
    assert_eq!(live.step_count(), 5);
    let shown: Vec<&str> = live
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Step(id) => Some(&*s.step(*id).id),
            _ => None,
        })
        .collect();
    assert_eq!(shown, ["b2", "w1", "a1", "b3"]);
    assert!(matches!(&live.items[0], Item::Thinking(t) if t == "hmm"));
    assert!(
        matches!(live.items.last(), Some(Item::Text { text, answer: false }) if text == "Building the line editor")
    );
    let e1 = step(&s, "e1");
    assert_eq!(e1.out().patches.len(), 1);
    assert_eq!(step(&s, "r1").out().lines.map(|l| l.total), Some(166));
    assert_eq!(step(&s, "w1").call().input.content, "package ui\n");
    assert_eq!(s.tools["Bash"].calls, 3);
    assert_eq!(s.tools["Bash"].failed, 1);
    assert_eq!(s.tools["Bash"].time, 3000);
}

#[test]
fn an_approval_waits_until_answered() {
    let mut s = session();
    s.feed(approval("b3", "Bash", "req-1"), at(29));
    let b3 = s.step_by_call("b3").unwrap();
    assert_eq!(s.pending(), [b3]);
    assert_eq!(step(&s, "b3").status, Status::Waiting);
    // An ask about a call not yet seen makes its step.
    s.feed(approval("b4", "Bash", "req-2"), at(30));
    assert_eq!(s.pending().len(), 2);
    s.settle("req-1");
    assert_eq!(step(&s, "b3").status, Status::Running);
    assert!(step(&s, "b3").approval.is_none());
    assert_eq!(s.pending(), [s.step_by_call("b4").unwrap()]);
}

#[test]
fn ending_loses_unfinished_steps() {
    let mut s = session();
    s.feed(done(0.6), at(40));
    assert_eq!(step(&s, "b3").status, Status::Lost);
    assert_eq!(s.turns[1].cost, 0.6 - 0.52);
    assert_eq!(s.turns[1].outcome(), "Building the line editor");
}

#[test]
fn streaming() {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(delta("Work"), at(1));
    s.feed(delta("ing on it"), at(1));
    assert!(matches!(&s.turns[0].items[0], Item::Text { text, .. } if text == "Working on it"));
    // The whole message replaces the streamed text rather than repeating it.
    s.feed(say("Working on it."), at(2));
    assert_eq!(s.turns[0].items.len(), 1);
    assert!(matches!(&s.turns[0].items[0], Item::Text { text, .. } if text == "Working on it."));
    assert_eq!(s.turns[0].streamed, 13);
}

#[test]
fn subagent_call_mid_stream_keeps_words_once() {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(tool_use("task", "Agent", json!({})), at(1));
    s.feed(delta("Only vf-runner"), at(2));
    s.feed(
        sub_tool_use("task", "b1", "Bash", json!({"command": "go vet ./..."})),
        at(3),
    );
    s.feed(say("Only vf-runner is still running."), at(4));
    let n = s.turns[0]
        .items
        .iter()
        .filter(|it| matches!(it, Item::Text { .. }))
        .count();
    assert_eq!(n, 1);
}

#[test]
fn tasks() {
    let mut s = Session::new();
    s.feed(sent("plan"), at(0));
    s.feed(
        tool_use(
            "c1",
            "TaskCreate",
            json!({"subject": "Build editor", "activeForm": "Building editor"}),
        ),
        at(1),
    );
    s.feed(
        tool_result(
            "c1",
            "Task #1 created successfully: Build editor",
            false,
            None,
        ),
        at(1),
    );
    s.feed(
        tool_use("c2", "TaskCreate", json!({"subject": "Wire keys"})),
        at(2),
    );
    s.feed(
        tool_result("c2", "Task #2 created successfully: Wire keys", false, None),
        at(2),
    );
    s.feed(
        tool_use(
            "u1",
            "TaskUpdate",
            json!({"taskId": "1", "status": "in_progress"}),
        ),
        at(3),
    );
    let (now, done, total) = s.current();
    assert_eq!(now.map(|t| t.subject.as_str()), Some("Build editor"));
    assert_eq!((done, total), (0, 2));
    s.feed(
        tool_use(
            "t1",
            "TodoWrite",
            json!({"todos": [{"content": "A", "activeForm": "Doing A", "status": "completed"}, {"content": "B", "status": "pending"}]}),
        ),
        at(4),
    );
    let (now, done, total) = s.current();
    assert!(now.is_none());
    assert_eq!((done, total), (1, 2));
    assert_eq!(s.tasks[1].id, "2");
}

#[test]
fn requests_count_once() {
    let mut s = Session::new();
    // Claude Code writes one line per content block of the same response;
    // each carries the usage so far and must count once.
    for (i, out) in [5, 40, 12].into_iter().enumerate() {
        let mut m = msg("assistant", json!([{"type": "text", "text": "hi"}]));
        m.id = "msg_1".into();
        m.model = "claude-opus-4-1".into();
        m.tokens = Some(TokenUsage {
            input: 100,
            output: out,
            ..Default::default()
        });
        s.feed(ev(m), 1_000_000 + i as i64 * 60_000);
    }
    assert_eq!(&*s.turns[0].model, "claude-opus-4-1");
    assert_eq!(s.context, 112);
    let mut m = msg("assistant", json!([]));
    m.id = "x".into();
    m.model = "<synthetic>".into();
    m.tokens = Some(TokenUsage {
        input: 9,
        ..Default::default()
    });
    s.feed(ev(m), 1_000_000);
    assert_eq!(s.requests.len(), 1);
    assert_eq!(
        (s.requests[0].usage.output, s.requests[0].usage.input),
        (40, 100)
    );
    assert_eq!(s.last - s.first, 120_000);
}

#[test]
fn subagent_requests_name_their_agent() {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(
        tool_use("a1", "Task", json!({"subagent_type": "Explore"})),
        at(1),
    );
    let mut m = msg("assistant", json!([{"type": "text", "text": "…"}]));
    m.id = "s1".into();
    m.parent = "a1".into();
    m.tokens = Some(TokenUsage {
        input: 1,
        ..Default::default()
    });
    s.feed(ev(m), at(2));
    assert_eq!(
        (&*s.requests[0].agent, &*s.requests[0].run),
        ("Explore", "a1")
    );
    // A subagent's words stay inside it, and its usage isn't the context's.
    assert_eq!(s.turns[0].items.len(), 1);
    assert_eq!(s.context, 0);
}

#[test]
fn compact_asks_are_not_yours() {
    let mut s = Session::new();
    s.feed(sent("/compact"), at(0));
    let t = s.live().unwrap();
    assert_eq!((t.prompt.as_str(), t.cause.as_str()), ("", "/compact"));
    s.feed(done(0.0), at(1));
    let carried = "This conversation was compacted by Claude haiku to make room. Here is the summary of everything before this point:\n\ndid X, doing Y\n\nTake this as what you know of the work so far. Reply only \"ready\" and wait for my next message.";
    s.feed(sent(carried), at(2));
    let t = s.live().unwrap();
    assert!(t.prompt.is_empty());
    assert_eq!(t.items.len(), 1);
    match &t.items[0] {
        Item::Compact { compact, summary } => {
            assert_eq!(summary, "did X, doing Y");
            assert_eq!(&*compact.trigger, "by Claude haiku");
        }
        it => panic!("{it:?}"),
    }
}

#[test]
fn compaction_divider_and_context() {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(
        Event::Status(event::Status {
            busy: true,
            text: "compacting".into(),
        }),
        at(1),
    );
    assert_eq!(s.compacting(), Some(at(1)));
    s.feed(
        Event::Compacted(event::Compacted {
            trigger: "auto".into(),
            before: 1000,
            after: 50,
        }),
        at(3),
    );
    assert_eq!(s.compacting(), None);
    assert_eq!(s.context, 50);
    assert_eq!(s.compact_rate_ns(), 2_000_000);
    assert!(matches!(s.turns[0].items[0], Item::Compact { .. }));
}

#[test]
fn errors_and_interrupts() {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(say("Usage limit reached"), at(1));
    s.feed(
        Event::TurnEnd(event::TurnEnd {
            reason: "error".into(),
            err: "Usage limit reached\nmore".into(),
            ..event::TurnEnd::default()
        }),
        at(2),
    );
    assert_eq!(s.turns[0].err, "Usage limit reached");
    // The agent's own copy of the error is the ✗ row's.
    assert!(s.turns[0].items.is_empty());
    s.feed(sent("again"), at(3));
    s.feed(
        Event::TurnEnd(event::TurnEnd {
            reason: "max_turns".into(),
            ..event::TurnEnd::default()
        }),
        at(4),
    );
    assert_eq!(s.turns[1].err, "max turns");
    s.feed(sent("once more"), at(5));
    s.feed(
        tool_use("x", "Bash", json!({"command": "sleep 100"})),
        at(6),
    );
    s.feed(
        ev(msg(
            "user",
            json!([{"type": "text", "text": "[Request interrupted by user for tool use]"}]),
        )),
        at(7),
    );
    let t = &s.turns[2];
    assert!(t.stopped && !t.live && t.err.is_empty());
    assert_eq!(step(&s, "x").status, Status::Lost);
}

#[test]
fn rejections_are_denied() {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(tool_use("x", "Bash", json!({"command": "rm -rf /"})), at(1));
    s.feed(
        tool_result(
            "x",
            "The user doesn't want to proceed with this tool use.",
            true,
            None,
        ),
        at(2),
    );
    assert_eq!(step(&s, "x").status, Status::Denied);
    assert_eq!(s.tools["Bash"].failed, 0);
}

#[test]
fn injected_text_opens_a_turn() {
    let mut s = Session::new();
    let note = "<task-notification>\n<task-id>a1</task-id>\n<status>killed</status>\n<summary>Agent \"lint lane\" finished</summary>\n</task-notification>";
    s.feed(
        ev(msg("user", json!([{"type": "text", "text": note}]))),
        at(0),
    );
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].from, "background task · stopped");
    assert_eq!(s.turns[0].prompt, "Agent \"lint lane\" finished");
    assert_eq!(s.task_status.get("a1").map(|s| &**s), Some("stopped"));
}

#[test]
fn tool_result_images_land_on_their_step() {
    let mut s = Session::new();
    s.feed(sent("look"), at(0));
    s.feed(tool_use("r", "Read", json!({"file_path": "/a.png"})), at(1));
    s.feed(
        ev(msg(
            "user",
            json!([{"type": "tool_result", "tool_use_id": "r", "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aW1n"}}]}]),
        )),
        at(2),
    );
    let st = step(&s, "r");
    assert_eq!(st.images.len(), 1);
    // What it is and how big, not its bytes.
    assert_eq!(
        (&*st.images[0].media_type, st.images[0].bytes),
        ("image/png", 3)
    );
}

/// What a search of the open session finds, narrowed by is:, file: and
/// turn: (convo_test.go TestSearch, TestSearchEdges).
#[test]
fn search() {
    let s = session();
    let cases: [(&str, &[&str]); 7] = [
        ("is:failed", &["t2:s:b2"]),
        ("is:edit", &["t1:s:e1", "t2:s:w1"]),
        ("file:editor.go", &["t2:s:w1"]),
        ("alt screen", &["t1"]),
        ("is:you attach", &["t1"]),
        ("unreachable", &["t2:s:b2"]),
        ("turn:2 is:cmd", &["t2:s:b2", "t2:s:b3"]),
    ];
    for (q, want) in cases {
        let got: Vec<String> = s.search(q).into_iter().map(|h| h.r#ref).collect();
        assert_eq!(got, want, "{q}");
    }
    assert!(!s.search("is:agent").is_empty());
    assert_eq!(s.search("is:claude").len(), s.search("is:agent").len());
    // A subagent's step opens under its parent.
    assert_eq!(s.parent_ref("t2:s:a1g"), "t2:s:a1");
    assert_eq!(s.parent_ref("t2:s:b2"), "");
}
