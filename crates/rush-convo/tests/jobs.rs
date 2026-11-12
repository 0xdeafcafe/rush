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

/// A task notice that says the run still goes (or waits) isn't its end;
/// one with no status, or an end's, is (Rust-only).
#[test]
fn interim_notice_keeps_a_task_running() {
    let cases: &[(&str, bool, &str)] = &[
        ("running", true, ""),
        ("pending", true, ""),
        ("", false, "completed"),
        ("completed", false, "completed"),
        ("failed", false, "failed"),
        ("killed", false, "stopped"),
    ];
    for &(status, running, ended) in cases {
        let mut s = Session::new();
        s.apply(
            &started("a1", "toolu_01", TaskKind::Subagent, "Explore", true),
            1,
        );
        s.apply(&done("a1", "toolu_01", status, ""), 2);
        let j = s.job("a1").unwrap();
        assert_eq!(j.running(), running, "{status:?}");
        assert_eq!(j.status, ended, "{status:?}");
        assert_eq!(
            s.task_status.get("a1").map(|s| &**s),
            (!running).then_some(ended),
            "{status:?}"
        );
    }
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
    // Folded, its start and its end, how many lines lie between counted.
    assert!(render(&s, &o).contains(" of 11 lines"), "collapse ignored");
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

/// rush's news of pull requests, several in a row, is one card: a row a
/// pull request, how it stands at its last news and, quieter, what came
/// before; opened, each its own card again. One alone keeps its own.
#[test]
fn a_run_of_pr_news_is_one_card() {
    let news = |id: &str, pr: &str, body: &str| Exchange {
        id: id.into(),
        direction: "received".into(),
        phase: "message".into(),
        text: format!(
            "<rush-pr-news pr=\"{pr}\">\nrush is following {pr} for you (rush pr follow).\n\n{body}\n</rush-pr-news>"
        ),
        sender: peer("", "rush", &format!("rush · {pr}")),
        ..Exchange::default()
    };
    let state = |v: &str| format!("o/r#1 Title\n  VERDICT: {v}");
    let run = [
        news(
            "a",
            "o/r#1",
            &format!(
                "New since rush last told you:\n- CONFLICT with main\n\n{}",
                state("WORK")
            ),
        ),
        news(
            "b",
            "o/r#2",
            &format!(
                "New since rush last told you:\n- FAILED ci / check (required)\n\n{}",
                state("WORK")
            ),
        ),
        news(
            "c",
            "o/r#2",
            &format!(
                "New since rush last told you:\n- FAILED ci / lint (required)\n\n{}",
                state("WORK")
            ),
        ),
        news("d", "o/r#1", &state("READY FOR APPROVAL")),
        news(
            "e",
            "o/r#1",
            &format!(
                "It merged. rush has stopped following it.\n\n{}",
                state("MERGED")
            ),
        ),
        news("f", "o/r#2", &state("READY FOR APPROVAL")),
    ];
    let mut s = Session::new();
    for (k, e) in run.iter().enumerate() {
        s.apply(&Event::Exchange(Box::new(e.clone())), 1000 + k as i64);
    }
    let o = Options {
        width: 100,
        now: 9000,
        ..Options::default()
    };
    let out = render(&s, &o);
    assert!(out.contains("rush · 2 pull requests"), "{out}");
    assert!(out.contains("6 messages"), "{out}");
    let row = |pr: &str| {
        out.lines()
            .find(|l| l.contains(pr))
            .unwrap_or("")
            .to_string()
    };
    let one = row("o/r#1  ");
    assert!(
        one.contains("merged") && one.contains("(conflict, ready before)"),
        "{out}"
    );
    let two = row("o/r#2  ");
    assert!(
        two.contains("ready") && two.contains("(failed ×2 before)"),
        "{out}"
    );
    assert!(
        !out.contains("CONFLICT with main"),
        "folded, the messages aren't drawn: {out}"
    );

    // Opened, each is its own card again.
    let r = s.turns[0].n;
    let mut o = o;
    o.open.insert(format!("t{r}:news:0"), true);
    let out = render(&s, &o);
    assert!(out.contains("CONFLICT with main"), "{out}");
    assert_eq!(out.matches("rush · o/r#2").count(), 3, "{out}");

    // One alone is its own card.
    let mut s = Session::new();
    s.apply(&Event::Exchange(Box::new(run[0].clone())), 1000);
    let out = render(
        &s,
        &Options {
            width: 100,
            now: 9000,
            ..Options::default()
        },
    );
    assert!(
        out.contains("CONFLICT with main") && !out.contains("messages"),
        "{out}"
    );
}

// exchange_test.go: TestFoldedResultReadsAsMarkdown, as deecb643 has it.
#[test]
fn folded_result_reads_as_markdown() {
    let mut s = Session::new();
    let e = Exchange {
        id: "r".into(),
        direction: "received".into(),
        phase: "result".into(),
        text: "Codex (agent 2b3cbe11) finished:\n\nPhase 2 is **complete**.\n- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight".into(),
        sender: peer("2b3cbe11", "codex", ""),
        ..Exchange::default()
    };
    s.apply(&Event::Exchange(Box::new(e)), 1000);
    let out = render(
        &s,
        &Options {
            width: 120,
            now: 2000,
            ..Options::default()
        },
    );
    for bad in ["finished:", "**", "one - two"] {
        assert!(!out.contains(bad), "folded card shows {bad:?}:\n{out}");
    }
    // Its start and its end show, what lies between counted.
    for want in [
        "Phase 2 is complete.",
        "two",
        "seven",
        "eight",
        "… 4 of 9 lines",
    ] {
        assert!(out.contains(want), "folded card lacks {want:?}:\n{out}");
    }
    assert!(
        !out.contains("three") && !out.contains("six"),
        "folded card:\n{out}"
    );
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

/// The files a shell command sends its output to, a cd before them
/// heeded; /dev and a variable's file left out (TestJobWrites).
#[test]
fn a_commands_writes() {
    let mut s = Session::new();
    s.info.cwd = "/repo".into();
    let cmd = r#"cd /lw/langwatch; go run ./cmd/visualdiff run > .visualdiff.log 2>&1 & echo hi >/dev/null; make 2>&1 | tee -a "build.log"; echo x >> $OUT; cd sub && ls > ~/ls.txt"#;
    let home = std::env::var("HOME").unwrap();
    assert_eq!(
        s.writes(cmd),
        [
            "/lw/langwatch/.visualdiff.log".to_string(),
            "/lw/langwatch/build.log".into(),
            format!("{home}/ls.txt"),
        ]
    );
}

/// The redirects that name a file for a running step's live tail.
#[test]
fn redirects_a_step_tails() {
    let mut s = Session::new();
    s.info.cwd = "/repo".into();
    for (cmd, want) in [
        ("make > out.log", vec!["/repo/out.log"]),
        ("make >> out.log 2>&1", vec!["/repo/out.log"]),
        ("make &> /tmp/b.log", vec!["/tmp/b.log"]),
        ("make 2>&1 | tee /tmp/t.log", vec!["/tmp/t.log"]),
        ("make 2>&1 | tee -a /tmp/t.log", vec!["/tmp/t.log"]),
        ("cd sub && make > o.log", vec!["/repo/sub/o.log"]),
        ("make 2>/dev/null", vec![]),
        ("make 2>&1", vec![]),
        ("make > $OUT", vec![]),
    ] {
        assert_eq!(s.writes(cmd), want, "{cmd}");
    }
}

/// A background task is known for what it is from the start: one of a
/// kind rush doesn't know, started for a subagent type, is a subagent
/// (not a task until its run is found); the session's own background
/// work is counted by kind, a subagent's own command left to it, so the
/// Session's header and its column agree (Rust-only).
#[test]
fn background_counts_by_kind() {
    let sub = |id: &str, kind: TaskKind, agent: &str| {
        Event::TaskStarted(TaskStarted {
            id: id.into(),
            call_id: format!("c-{id}"),
            kind,
            label: id.into(),
            agent: agent.into(),
            background: true,
        })
    };
    let listed = |ts: &[(&str, TaskKind, &str)]| {
        Event::Background(Background {
            tasks: ts
                .iter()
                .map(|(id, kind, parent)| BackgroundTask {
                    id: (*id).into(),
                    kind: *kind,
                    parent: (*parent).into(),
                    ..BackgroundTask::default()
                })
                .collect(),
        })
    };
    type Case = (&'static str, Vec<Event>, (usize, usize, usize));
    let cases: Vec<Case> = vec![
        ("none", vec![], (0, 0, 0)),
        (
            "a subagent of an unknown kind, by its type",
            vec![sub("a1", TaskKind::Other, "Explore")],
            (1, 0, 0),
        ),
        (
            "a task of an unknown kind with no type",
            vec![sub("t1", TaskKind::Other, "")],
            (0, 0, 1),
        ),
        (
            "three subagents and their own commands",
            vec![
                sub("a1", TaskKind::Subagent, "Explore"),
                sub("a2", TaskKind::Subagent, "Plan"),
                sub("a3", TaskKind::Subagent, ""),
                listed(&[
                    ("a1", TaskKind::Subagent, ""),
                    ("a2", TaskKind::Subagent, ""),
                    ("a3", TaskKind::Subagent, ""),
                    ("b1", TaskKind::Shell, "a1"),
                    ("b2", TaskKind::Shell, "a2"),
                    ("b3", TaskKind::Shell, "a3"),
                    ("b4", TaskKind::Shell, ""),
                ]),
            ],
            (3, 0, 1),
        ),
        (
            "a workflow and a monitor",
            vec![
                sub("w", TaskKind::Workflow, ""),
                sub("m", TaskKind::Monitor, ""),
            ],
            (0, 1, 1),
        ),
    ];
    for (name, evs, want) in cases {
        let mut s = Session::new();
        for e in &evs {
            s.apply(e, 1);
        }
        assert_eq!(s.background_counts(), want, "{name}");
    }
    let mut s = Session::new();
    s.apply(&sub("a1", TaskKind::Other, "Explore"), 1);
    assert_eq!(s.job_kind(s.job("a1").unwrap()), "subagent");
}
