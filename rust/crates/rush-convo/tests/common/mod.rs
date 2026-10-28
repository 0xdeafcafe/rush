//! Events and sessions the render tests and benches share: Go's
//! benchSession (bench_test.go:24) and the helpers convo_test.go builds
//! its fixtures with.
#![allow(dead_code)]

use std::fmt::Write as _;

use rush_convo::{Event, Part, Session, TurnEnd};
use rush_core::message::{Message, Role};
use serde_json::json;

pub const T0: i64 = 1_790_200_800_000; // 2026-09-23T22:00:00Z

/// Go's dump is drawn in the zone of the machine it ran on: Europe in
/// September, two hours east.
pub const ZONE: i64 = 7200;

pub fn at(sec: i64) -> i64 {
    T0 + sec * 1000
}

pub fn sent(text: &str) -> Event {
    Event::Sent {
        text: text.into(),
        images: Vec::new(),
    }
}

pub fn sent_images(text: &str, images: &[&str]) -> Event {
    Event::Sent {
        text: text.into(),
        images: images.iter().map(|s| (*s).into()).collect(),
    }
}

pub fn msg(role: &str, content: serde_json::Value) -> Message {
    let mut m = rush_claude::decode_message(&json!({"role": role, "content": content}).to_string())
        .unwrap();
    m.role = if role == "user" {
        Role::User
    } else {
        Role::Assistant
    };
    m
}

pub fn say(text: &str) -> Event {
    Event::Message(msg("assistant", json!([{"type": "text", "text": text}])))
}

pub fn tool_use(id: &str, name: &str, input: serde_json::Value) -> Event {
    Event::Message(msg(
        "assistant",
        json!([{"type": "tool_use", "id": id, "name": name, "input": input}]),
    ))
}

pub fn sub_tool_use(parent: &str, id: &str, name: &str, input: serde_json::Value) -> Event {
    let Event::Message(mut m) = tool_use(id, name, input) else {
        unreachable!()
    };
    m.parent_tool_use_id = parent.into();
    Event::Message(m)
}

pub fn tool_result(
    id: &str,
    text: &str,
    is_err: bool,
    structured: Option<serde_json::Value>,
) -> Event {
    let mut m = msg(
        "user",
        json!([{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_err}]),
    );
    m.tool_result =
        structured.map(|v| serde_json::value::RawValue::from_string(v.to_string()).unwrap());
    Event::Message(m)
}

pub fn done(cost: f64) -> Event {
    Event::TurnEnd(TurnEnd {
        reason: "done".into(),
        cost,
        err: String::new(),
    })
}

pub fn delta(text: &str) -> Event {
    Event::Delta {
        part: Part::Text,
        text: text.into(),
    }
}

pub const ANSWER: &str = "## What changed\n\nThe **renderer** now caches each turn, so `Render` only redraws what moved.\n\n- the live turn redraws per frame\n- finished turns come from the cache\n- see https://example.com/docs/render for more\n\n1. build it\n2. run the tests\n\n```go\nfunc f() { return }\n```\n\nThat should make streaming feel instant on long sessions, even at wide widths where every row is padded.";

/// n finished turns of narration, reads, greps, commands, edits and a
/// markdown answer, then a live turn streaming its answer (benchEvents).
pub fn bench_events(n: usize) -> Vec<(i64, Event)> {
    let mut out = Vec::new();
    let mut sec = 0;
    let mut add = |ev: Event| {
        out.push((at(sec), ev));
        sec += 1;
    };
    let mut stdout = String::new();
    for i in 0..30 {
        writeln!(stdout, "ok  \tgithub.com/x/y/pkg{i}\t0.{:03}s", i * 7).unwrap();
    }
    for t in 1..=n {
        let id = |k: &str| format!("{k}{t}");
        add(sent(&format!(
            "turn {t}: look at /work/rush/internal/ui/view.go and fix the @internal/convo/render.go wrap, then run /review https://example.com/x"
        )));
        let mut m = msg(
            "assistant",
            json!([{"type": "text", "text": "Looking at how the **pane** draws its `rows` and where the wrap happens; the renderer is in render.go."}]),
        );
        m.id = id("m");
        m.model = "claude-opus-5-5".into();
        m.usage = Some(rush_core::message::Usage {
            input: 10,
            output: 200,
            cache_read: 50000,
            ..Default::default()
        });
        add(Event::Message(m));
        add(tool_use(
            &id("r"),
            "Read",
            json!({"file_path": "/work/rush/internal/ui/view.go"}),
        ));
        add(tool_result(
            &id("r"),
            "…",
            false,
            Some(
                json!({"type": "text", "file": {"numLines": 400, "startLine": 1, "totalLines": 1589}}),
            ),
        ));
        add(tool_use(
            &id("g"),
            "Grep",
            json!({"pattern": "func wrap", "path": "/work/rush/internal"}),
        ));
        add(tool_result(
            &id("g"),
            "internal/ui/style.go\ninternal/convo/style.go",
            false,
            None,
        ));
        add(tool_use(
            &id("b"),
            "Bash",
            json!({"command": "cd /work/rush && go test ./... 2>&1 | tail -40", "description": "Run the tests"}),
        ));
        add(tool_result(
            &id("b"),
            &stdout,
            false,
            Some(json!({"stdout": stdout, "stderr": ""})),
        ));
        add(tool_use(
            &id("e"),
            "Edit",
            json!({"file_path": "/work/rush/internal/convo/render.go"}),
        ));
        add(tool_result(
            &id("e"),
            "ok",
            false,
            Some(
                json!({"structuredPatch": [{"oldStart": 60, "oldLines": 3, "newStart": 60, "newLines": 4,
            "lines": [" \tfor _, t := range s.Turns {", "-\t\tout = append(out, s.turn(t, o)...)", "+\t\tls := s.turn(t, o)", "+\t\tout = append(out, ls...)", " \t}"]}]}),
            ),
        ));
        add(tool_use(
            &id("f"),
            "Bash",
            json!({"command": "go vet ./..."}),
        ));
        add(tool_result(
            &id("f"),
            "Exit code 1\ninternal/ui/editor.go:41:2: unreachable code",
            true,
            Some(json!({"stdout": "", "stderr": "internal/ui/editor.go:41:2: unreachable code"})),
        ));
        let mut m = msg("assistant", json!([{"type": "text", "text": ANSWER}]));
        m.id = id("a");
        m.model = "claude-opus-5-5".into();
        add(Event::Message(m));
        add(done(0.42));
    }
    add(sent("now make streaming quicker"));
    add(tool_use(
        "lr",
        "Read",
        json!({"file_path": "/work/rush/internal/ui/rushmode.go"}),
    ));
    add(tool_result(
        "lr",
        "…",
        false,
        Some(
            json!({"type": "text", "file": {"numLines": 400, "startLine": 1, "totalLines": 1765}}),
        ),
    ));
    for w in ANSWER.split_whitespace() {
        add(delta(&format!("{w} ")));
    }
    out
}

pub fn bench_session(n: usize) -> Session {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    for (t, ev) in bench_events(n) {
        s.apply(ev, t);
    }
    s
}
