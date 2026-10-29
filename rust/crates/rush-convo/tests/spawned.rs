//! The agents a command or a spawn_agent call ran, once found: the step
//! is drawn as the agent, its own steps under it and what it said back
//! as its body; several are a row each under the step. After Go's
//! internal/convo/spawn_test.go (TestSpawnFound, TestSpawnReplies,
//! TestSpawnToolCall, TestSpawnToolBriefWraps) and internal/ui
//! spawns_test.go (TestFanOutFollowed's rows).

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::sync::Arc;

use rush_convo::render::{Child, Spawn, plain};
use rush_convo::{Options, Renderer, Session};
use serde_json::json;

mod common;
use common::*;

fn draw(s: &Session, o: &Options) -> String {
    plain(&Renderer::new().render(s, o))
}

/// One found agent's session: asked, it read six files.
fn reader(asked: &str) -> Session {
    let mut k = Session::new();
    k.feed(sent(asked), at(1));
    for (i, f) in ["a.go", "b.go", "c.go", "d.go", "e.go", "f.go"]
        .iter()
        .enumerate()
    {
        let id = format!("r{i}");
        k.feed(
            tool_use(&id, "Read", json!({"file_path": format!("/work/rush/{f}")})),
            at(2 + i as i64),
        );
        k.feed(tool_result(&id, "…", false, None), at(2 + i as i64));
    }
    k
}

fn codex(prompt: &str) -> Spawn {
    Spawn {
        kind: "codex".into(),
        name: "Codex".into(),
        model: "gpt-5".into(),
        prompt: prompt.into(),
        ..Spawn::default()
    }
}

/// Found, a command's agent is its row, its latest steps under it while
/// the command runs (TestSpawnFound).
#[test]
fn a_found_agents_steps_under_its_row() {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(sent("get a second opinion"), at(0));
    s.feed(
        tool_use(
            "b1",
            "Bash",
            json!({"command": "codex exec -m gpt-5 \"review the diff for races\""}),
        ),
        at(1),
    );
    let sp = codex("review the diff for races");
    let mut o = Options {
        width: 100,
        now: at(9),
        kind: "claude".into(),
        ..Options::default()
    };
    o.ran.insert("b1".into(), sp.clone());
    o.kids.insert(
        "b1".into(),
        vec![Child {
            id: "c".into(),
            spawn: sp,
            sess: Some(Arc::new(reader("review the diff for races"))),
            ..Child::default()
        }],
    );
    let out = draw(&s, &o);
    for want in [
        "review the diff for races · codex gpt-5", // named as the registry has it: none here
        "6 steps",
        "⋯ 2 steps before",
        "c.go",
        "f.go",
    ] {
        assert!(out.contains(want), "no {want:?} in\n{out}");
    }
    assert!(
        !out.contains("b.go"),
        "an early step shows before the row is opened:\n{out}"
    );
}

/// Opened, a found agent's row shows what it said back: not the command
/// that ran it, nor its banner (TestSpawnReplies).
#[test]
fn a_found_agents_reply_is_its_body() {
    let mut s = Session::new();
    s.feed(sent("get a second opinion"), at(0));
    s.feed(
        tool_use(
            "b1",
            "Bash",
            json!({"command": "codex exec \"review the diff for races\""}),
        ),
        at(1),
    );
    s.feed(
        tool_result(
            "b1",
            "OpenAI Codex v0.1\nworkdir: /work\nNo races found.",
            false,
            None,
        ),
        at(5),
    );
    let mut k = Session::new();
    k.feed(sent("review the diff for races"), at(1));
    k.feed(say("No races found."), at(4));
    let sp = codex("review the diff for races");
    let mut o = Options {
        width: 100,
        now: at(9),
        verbose: true,
        ..Options::default()
    };
    o.ran.insert("b1".into(), sp.clone());
    o.kids.insert(
        "b1".into(),
        vec![Child {
            id: "c".into(),
            spawn: sp,
            sess: Some(Arc::new(k)),
            ..Child::default()
        }],
    );
    let out = draw(&s, &o);
    assert!(
        out.contains("No races found.")
            && !out.contains("$ codex exec")
            && !out.contains("workdir"),
        "the reply isn't its body:\n{out}"
    );
}

/// A spawn_agent call is drawn as the agent it starts, its prompt as the
/// brief and the answer as its reply; found, the agent's own steps go
/// under it (TestSpawnToolCall).
#[test]
fn a_spawn_agent_call() {
    let mut s = Session::new();
    s.feed(sent("ask astra"), at(0));
    s.feed(
        tool_use(
            "t1",
            "mcp__rush__spawn_agent",
            json!({"agent": "astra", "prompt": "Review the diff for races"}),
        ),
        at(1),
    );
    // Not found yet: open, its row names the agent alone and its task
    // shows once, under it.
    let o = Options {
        width: 100,
        now: at(2),
        ..Options::default()
    };
    let out = draw(&s, &o);
    assert_eq!(
        out.matches("Review the diff for races").count(),
        1,
        "the task twice:\n{out}"
    );
    assert!(out.contains("⇉ astra"), "{out}");
    let mut k = Session::new();
    k.feed(sent("Review the diff for races"), at(1));
    k.feed(
        tool_use(
            "k1",
            "Bash",
            json!({"command": "git diff", "description": "Read the diff"}),
        ),
        at(2),
    );
    k.feed(tool_result("k1", "+x", false, None), at(3));
    let sp = Spawn {
        kind: "codex".into(),
        name: "Codex".into(),
        prompt: "Review the diff for races".into(),
        ..Spawn::default()
    };
    let mut o = Options {
        width: 100,
        now: at(9),
        verbose: true,
        ..Options::default()
    };
    o.ran.insert("t1".into(), sp.clone());
    o.kids.insert(
        "t1".into(),
        vec![Child {
            id: "c1".into(),
            spawn: sp,
            sess: Some(Arc::new(k)),
            live: true,
            start: at(1),
        }],
    );
    s.feed(
        tool_result("t1", "Codex (c1) finished:\n\nNo races found.", false, None),
        at(5),
    );
    let out = draw(&s, &o);
    for want in ["Codex", "Review the diff for races", "Read the diff"] {
        assert!(out.contains(want), "no {want:?} in\n{out}");
    }
    assert!(
        !out.contains("mcp__rush") && !out.contains("spawn agent"),
        "drawn as the tool, not the agent:\n{out}"
    );
}

/// Open, a spawn_agent call's task is wrapped to the pane rather than cut
/// at its edge (TestSpawnToolBriefWraps).
#[test]
fn a_spawn_agent_calls_task_wraps() {
    let mut s = Session::new();
    s.feed(sent("ask astra"), at(0));
    let prompt = "Code review, read-only: do NOT edit files (other agents work in this repo; the working tree is shared) TAILWORD\n\nRepo: here.";
    s.feed(
        tool_use(
            "t1",
            "mcp__rush__spawn_agent",
            json!({"agent": "astra", "prompt": prompt}),
        ),
        at(1),
    );
    let o = Options {
        width: 70,
        now: at(2),
        ..Options::default()
    };
    let out = draw(&s, &o);
    assert_eq!(out.matches("Code review, read-only").count(), 1, "{out}");
    assert!(out.contains("TAILWORD"), "cut, not wrapped:\n{out}");
}

/// A command that ran several agents is a row each under it, what it's
/// for on its own row; the wrapper doesn't show (TestFanOutFollowed).
#[test]
fn a_command_that_ran_several() {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("three reviews"), at(0));
    s.feed(
        tool_use(
            "b1",
            "Bash",
            json!({"command": "run() { perl -e 'exec @ARGV' claude -p \"$1\"; }\nrun a &\nrun b &\nwait", "description": "Three reviews"}),
        ),
        at(1),
    );
    let mut o = Options {
        width: 110,
        now: at(30),
        kind: "claude".into(),
        ..Options::default()
    };
    let kids = ["correctness", "design", "tests"]
        .iter()
        .enumerate()
        .map(|(i, lens)| {
            let asked = format!("review it. YOUR LENS: {lens}");
            let mut k = Session::new();
            k.feed(sent(&asked), at(2));
            k.feed(say(&format!("reviewed: {lens}")), at(10 + i as i64));
            Child {
                id: format!("kid{i}"),
                spawn: Spawn {
                    kind: "claude".into(),
                    name: "Claude Code".into(),
                    prompt: asked,
                    ..Spawn::default()
                },
                sess: Some(Arc::new(k)),
                live: i == 0,
                start: at(2),
            }
        })
        .collect();
    o.kids.insert("b1".into(), kids);
    let out = draw(&s, &o);
    assert_eq!(
        out.matches("⇉ review it. YOUR LENS:").count(),
        3,
        "a row each:\n{out}"
    );
    assert!(
        out.contains("⇉ Three reviews") && out.contains("3 agents"),
        "{out}"
    );
    assert!(!out.contains("perl"), "the wrapper shows:\n{out}");
    // Opened, one says what it said back.
    o.open.insert("t1:s:b1/kid1".into(), true);
    let out = draw(&s, &o);
    assert!(out.contains("reviewed: design"), "{out}");
}

/// The rows as Go's convo tests print them: no refs.
fn rows(s: &Session, o: &Options) -> String {
    Renderer::new()
        .render(s, o)
        .iter()
        .map(|l| format!("{}\n", l.line.plain().trim_end()))
        .collect()
}

/// Several agents a command ran, one found, and a spawn_agent call's
/// brief, against Go's rows for the same sessions at 80, 120 and 200
/// columns (testdata dumped from Go's Render; a registry names Codex
/// "Codex" there, none here).
#[test]
fn against_gos_rows() {
    let widths = [80, 120, 200];
    let mut got = String::new();
    let mut fan = Session::new();
    fan.cwd = "/work/app".into();
    fan.feed(sent("three reviews"), at(0));
    fan.feed(
        tool_use(
            "b1",
            "Bash",
            json!({"command": "run() { perl -e 'exec @ARGV' claude -p \"$1\"; }\nrun a &\nrun b &\nwait", "description": "Three reviews"}),
        ),
        at(1),
    );
    let kids: Vec<Child> = ["correctness", "design", "tests"]
        .iter()
        .enumerate()
        .map(|(i, lens)| {
            let asked = format!("review it. YOUR LENS: {lens}");
            let mut k = Session::new();
            k.feed(sent(&asked), at(2));
            k.feed(say(&format!("reviewed: {lens}")), at(10 + i as i64));
            Child {
                id: format!("kid{i}"),
                spawn: Spawn {
                    kind: "claude".into(),
                    name: "Claude Code".into(),
                    prompt: asked,
                    ..Spawn::default()
                },
                sess: Some(Arc::new(k)),
                live: i == 0,
                start: at(2),
            }
        })
        .collect();
    for open in [false, true] {
        for w in widths {
            let mut o = Options {
                width: w,
                now: at(30),
                kind: "claude".into(),
                zone: ZONE,
                ..Options::default()
            };
            o.kids.insert("b1".into(), kids.clone());
            if open {
                o.open.insert("t1:s:b1/kid1".into(), true);
            }
            got += &format!("== fan open {open} width {w}\n{}", rows(&fan, &o));
        }
    }
    let mut one = Session::new();
    one.cwd = "/work/rush".into();
    one.feed(sent("get a second opinion"), at(0));
    one.feed(
        tool_use(
            "b1",
            "Bash",
            json!({"command": "codex exec -m gpt-5 \"review the diff for races\""}),
        ),
        at(1),
    );
    for w in widths {
        let mut o = Options {
            width: w,
            now: at(9),
            kind: "claude".into(),
            zone: ZONE,
            ..Options::default()
        };
        let sp = codex("review the diff for races");
        o.ran.insert("b1".into(), sp.clone());
        o.kids.insert(
            "b1".into(),
            vec![Child {
                id: "c".into(),
                spawn: sp,
                sess: Some(Arc::new(reader("review the diff for races"))),
                ..Child::default()
            }],
        );
        got += &format!("== one width {w}\n{}", rows(&one, &o));
    }
    let mut sa = Session::new();
    sa.feed(sent("ask astra"), at(0));
    sa.feed(
        tool_use(
            "t1",
            "mcp__rush__spawn_agent",
            json!({"agent": "astra", "prompt": "Code review, read-only: do NOT edit files (other agents work in this repo; the working tree is shared) TAILWORD\n\nRepo: here."}),
        ),
        at(1),
    );
    for w in widths {
        let o = Options {
            width: w,
            now: at(2),
            kind: "claude".into(),
            zone: ZONE,
            ..Options::default()
        };
        got += &format!("== spawn_agent width {w}\n{}", rows(&sa, &o));
    }
    let want = include_str!("testdata/go_spawned.txt");
    for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "row {i}:\n{got}");
    }
    assert_eq!(got.lines().count(), want.lines().count(), "\n{got}");
}
