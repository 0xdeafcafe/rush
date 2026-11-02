//! rush's own agent tools drawn by what they do, whichever harness called
//! them: spawn_agent as the subagent it starts, agent_result as waiting on
//! that agent with what it's doing under it, agent_send as a message to
//! it, start_agent as the session it started; the line under the
//! conversation names them the same way; Claude Code loading a tool
//! first is left out of every view but Trail.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::sync::Arc;

use rush_convo::render::{Child, Spawn};
use rush_convo::{Options, Preset, Renderer, Session};
use serde_json::json;

mod common;
use common::*;

const TASK: &str = "Review the diff for races\n\nRepo: here.";

/// The rows as drawn, without refs.
fn rows(s: &Session, o: &Options) -> String {
    Renderer::new()
        .render(s, o)
        .iter()
        .map(|l| format!("{}\n", l.line.plain().trim_end()))
        .collect()
}

/// got checked against tests/snapshots/agtools_<name>.txt; UPDATE_SNAPSHOTS=1
/// writes it. A missing one fails, printed whole, so CI shows it.
fn snapshot(name: &str, got: &str) -> bool {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("agtools_{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return true;
    }
    let want = std::fs::read_to_string(&p).unwrap_or_default();
    if got != want {
        eprintln!(
            "agtools_{name} differs; UPDATE_SNAPSHOTS=1 to accept\n=== agtools_{name} drawn ===\n{got}=== end agtools_{name} ==="
        );
    }
    got == want
}

/// Codex's astra as rush hosts it: asked the task, reading a file; done,
/// it has answered.
fn astra(done: bool) -> Child {
    let mut k = Session::new();
    k.feed(sent(TASK), at(1));
    k.feed(
        tool_use("k1", "Read", json!({"file_path": "/work/rush/src/view.rs"})),
        at(2),
    );
    if done {
        k.feed(tool_result("k1", "…", false, None), at(3));
        k.feed(say("No races found.\nOne nit in view.rs."), at(170));
    }
    Child {
        id: "c1".into(),
        spawn: Spawn {
            kind: "codex".into(),
            name: "Codex".into(),
            model: "gpt-6-astra".into(),
            prompt: TASK.into(),
            ..Spawn::default()
        },
        sess: Some(Arc::new(k)),
        live: !done,
        start: at(1),
    }
}

/// A Claude Code session that hands astra the task in the background,
/// tells it more, then waits on it; done, astra has answered.
fn waiting(done: bool) -> (Session, Options) {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(sent("ask astra to review it"), at(0));
    s.feed(
        tool_use(
            "t1",
            "mcp__rush__spawn_agent",
            json!({"agent": "astra", "prompt": TASK, "background": true}),
        ),
        at(1),
    );
    s.feed(
        tool_result(
            "t1",
            "astra started in the background as agent c1. Its answer is sent to you as a message when it finishes; agent_result with this id asks sooner.",
            false,
            None,
        ),
        at(2),
    );
    s.feed(
        tool_use(
            "t2",
            "mcp__rush__agent_send",
            json!({"id": "c1", "prompt": "Look at view.rs too\nit changed last"}),
        ),
        at(3),
    );
    s.feed(
        tool_result("t2", "Codex (c1) finished:\n\nWill do.", false, None),
        at(4),
    );
    s.feed(
        tool_use(
            "t3",
            "mcp__rush__agent_result",
            json!({"id": "c1", "wait_seconds": 600}),
        ),
        at(5),
    );
    if done {
        s.feed(
            tool_result(
                "t3",
                "Codex (c1) finished:\n\nNo races found.\nOne nit in view.rs.",
                false,
                None,
            ),
            at(171),
        );
    }
    let mut o = Options {
        width: 100,
        now: at(if done { 172 } else { 130 }),
        kind: "claude".into(),
        zone: ZONE,
        dock_activity: true,
        ..Options::default()
    };
    o.kids.insert("t1".into(), vec![astra(done)]);
    (s, o)
}

/// A session that starts sol as a session of its own.
fn started(done: bool) -> (Session, Options) {
    let mut s = Session::new();
    s.feed(sent("start sol on the parser"), at(0));
    s.feed(
        tool_use(
            "t1",
            "mcp__rush__start_agent",
            json!({"agent": "sol", "prompt": "Port the parser\nall of it"}),
        ),
        at(1),
    );
    if done {
        s.feed(
            tool_result(
                "t1",
                "sol started as session s9, beside yours in the user's list. Its answer isn't sent to you: agent_send with this id messages it, agent_result asks how it stands.",
                false,
                None,
            ),
            at(2),
        );
        s.feed(say("Started sol on the parser."), at(3));
    }
    let o = Options {
        width: 100,
        now: at(4),
        kind: "claude".into(),
        zone: ZONE,
        dock_activity: true,
        ..Options::default()
    };
    (s, o)
}

/// A spawn_agent call in the foreground, found working, then done.
fn spawned(done: bool) -> (Session, Options) {
    let mut s = Session::new();
    s.feed(sent("ask astra to review it"), at(0));
    s.feed(
        tool_use(
            "t1",
            "mcp__rush__spawn_agent",
            json!({"agent": "astra", "prompt": TASK}),
        ),
        at(1),
    );
    if done {
        s.feed(
            tool_result(
                "t1",
                "Codex (c1) finished:\n\nNo races found.\nOne nit in view.rs.",
                false,
                None,
            ),
            at(171),
        );
    }
    let mut o = Options {
        width: 100,
        now: at(if done { 172 } else { 130 }),
        kind: "claude".into(),
        zone: ZONE,
        dock_activity: true,
        ..Options::default()
    };
    o.kids.insert("t1".into(), vec![astra(done)]);
    (s, o)
}

/// Each tool, running and done, against its snapshot.
#[test]
fn each_tool_running_and_done() {
    let mut same = true;
    for done in [false, true] {
        let state = if done { "done" } else { "running" };
        let (s, o) = waiting(done);
        same &= snapshot(&format!("result_send_{state}"), &rows(&s, &o));
        let (s, o) = started(done);
        same &= snapshot(&format!("start_{state}"), &rows(&s, &o));
        let (s, o) = spawned(done);
        same &= snapshot(&format!("spawn_{state}"), &rows(&s, &o));
    }
    assert!(same, "a drawing differs from its snapshot");
}

/// The rows say what each call does and to which agent, never the
/// tool's name.
#[test]
fn rows_by_meaning() {
    let (s, o) = waiting(false);
    let out = rows(&s, &o);
    for want in [
        "→ astra  Look at view.rs too",
        "◌ waiting on astra",
        "now · reading view.rs",
    ] {
        assert!(out.contains(want), "no {want:?} in\n{out}");
    }
    assert!(!out.contains("mcp__rush"), "{out}");
    assert!(!out.contains("agent_result"), "{out}");
    let (s, o) = waiting(true);
    let out = rows(&s, &o);
    assert!(out.contains("↩ astra answered  No races found."), "{out}");
    assert!(
        !out.contains("now · "),
        "done, it says what it's doing:\n{out}"
    );
    let (s, o) = started(true);
    let out = rows(&s, &o);
    assert!(
        out.contains("⧉ started sol as its own session  Port the parser"),
        "{out}"
    );
}

/// The line under the conversation names what the agent waits on, and
/// its harness when that's another, not the tool.
#[test]
fn the_working_line_names_the_agent() {
    let (s, o) = waiting(false);
    let out = rows(&s, &o);
    assert!(
        out.contains("waiting on astra (Codex)…"),
        "no working line in\n{out}"
    );
    assert!(!out.contains("running mcp__"), "{out}");
    let (s, o) = spawned(false);
    let out = rows(&s, &o);
    assert!(out.contains("asking astra (Codex)…"), "{out}");
    let (s, o) = started(false);
    let out = rows(&s, &o);
    assert!(out.contains("starting sol…"), "{out}");
    // Another server's tool by its name and server.
    let mut s = Session::new();
    s.feed(sent("look it up"), at(0));
    s.feed(
        tool_use(
            "t1",
            "mcp__claude_ai_Linear__get_issue",
            json!({"id": "ENG-1"}),
        ),
        at(1),
    );
    let o = Options {
        width: 100,
        now: at(5),
        dock_activity: true,
        ..Options::default()
    };
    let out = rows(&s, &o);
    assert!(
        out.contains("running get_issue · claude_ai_Linear…"),
        "{out}"
    );
}

/// Claude Code loading a tool before it calls it is left out of Normal
/// and Chat, and kept in Trail, where its time is accounted for.
#[test]
fn a_tool_load_shows_in_trail_alone() {
    let mut s = Session::new();
    s.feed(sent("ask astra"), at(0));
    s.feed(
        tool_use(
            "l1",
            "ToolSearch",
            json!({"query": "select:mcp__rush__agent_result", "max_results": 1}),
        ),
        at(1),
    );
    s.feed(
        tool_result(
            "l1",
            "loaded",
            false,
            Some(json!({"matches": ["mcp__rush__agent_result"]})),
        ),
        at(2),
    );
    s.feed(
        tool_use("t1", "mcp__rush__agent_result", json!({"id": "c1"})),
        at(3),
    );
    s.feed(
        tool_result("t1", "Codex (c1) finished:\n\nDone.", false, None),
        at(4),
    );
    s.feed(say("astra is done."), at(5));
    s.feed(done(0.01), at(6));
    let opts = |p: Preset| Options {
        width: 100,
        now: at(9),
        zone: ZONE,
        preset: p,
        ..Options::default()
    };
    for p in [Preset::Normal, Preset::Chat] {
        let out = rows(&s, &opts(p));
        assert!(
            !out.contains("loaded") && !out.contains("ToolSearch"),
            "{} shows the load:\n{out}",
            p.name()
        );
    }
    let out = rows(&s, &opts(Preset::Normal));
    assert!(out.contains("↩ Codex answered"), "{out}");
    let out = rows(&s, &opts(Preset::Trail));
    assert!(out.contains("ToolSearch"), "Trail lost the load:\n{out}");
    let mut same = snapshot("load_normal", &rows(&s, &opts(Preset::Normal)));
    same &= snapshot("load_trail", &rows(&s, &opts(Preset::Trail)));
    assert!(same, "a drawing differs from its snapshot");
}
