//! Events and sessions the render tests and benches share: Go's
//! benchSession (bench_test.go:24) and the helpers convo_test.go builds
//! its fixtures with.
#![allow(dead_code)]

use std::cell::RefCell;
use std::fmt::Write as _;

use rush_convo::Session;
use rush_core::adapter::MessageDecoder;
use rush_core::event::{self, Event, PartKind};
use serde_json::json;
use serde_json::value::RawValue;

pub const T0: i64 = 1_790_200_800_000; // 2026-09-23T22:00:00Z

/// Go's dump is drawn in the zone of the machine it ran on: Europe in
/// September, two hours east.
pub const ZONE: i64 = 7200;

pub fn at(sec: i64) -> i64 {
    T0 + sec * 1000
}

/// What a test feeds a session: an event, or what the host says you sent.
#[allow(clippy::large_enum_variant)]
pub enum Ev {
    Sent(String, Vec<Box<str>>),
    Do(Event),
}

impl From<Event> for Ev {
    fn from(e: Event) -> Ev {
        Ev::Do(e)
    }
}

pub trait Feed {
    fn feed(&mut self, ev: impl Into<Ev>, at: i64);
}

impl Feed for Session {
    fn feed(&mut self, ev: impl Into<Ev>, at: i64) {
        match ev.into() {
            Ev::Sent(text, images) => self.sent(text, images, at),
            Ev::Do(e) => self.apply(&e, at),
        }
    }
}

thread_local! {
    /// Claude Code's messages read as a transcript's are: one decoder, so
    /// a result is read knowing its call.
    static CLAUDE: RefCell<Box<dyn MessageDecoder>> = RefCell::new(rush_claude::messages());
}

/// A message in Claude Code's words, as rush's event.
pub fn claude(
    role: &str,
    content: serde_json::Value,
    result: Option<serde_json::Value>,
) -> event::Message {
    let m = RawValue::from_string(json!({"role": role, "content": content}).to_string()).unwrap();
    let r = result.map(|v| RawValue::from_string(v.to_string()).unwrap());
    let mut evs = CLAUDE.with(|d| d.borrow_mut().message(role, &m, r.as_deref()));
    match evs.pop() {
        Some(Event::Message(m)) => m,
        e => panic!("not a message: {e:?}"),
    }
}

pub fn msg(role: &str, content: serde_json::Value) -> event::Message {
    claude(role, content, None)
}

pub fn ev(m: event::Message) -> Ev {
    Ev::Do(Event::Message(m))
}

pub fn sent(text: &str) -> Ev {
    Ev::Sent(text.into(), Vec::new())
}

pub fn sent_images(text: &str, images: &[&str]) -> Ev {
    Ev::Sent(text.into(), images.iter().map(|s| (*s).into()).collect())
}

pub fn say(text: &str) -> Ev {
    ev(msg("assistant", json!([{"type": "text", "text": text}])))
}

pub fn tool_use(id: &str, name: &str, input: serde_json::Value) -> Ev {
    ev(msg(
        "assistant",
        json!([{"type": "tool_use", "id": id, "name": name, "input": input}]),
    ))
}

pub fn sub_tool_use(parent: &str, id: &str, name: &str, input: serde_json::Value) -> Ev {
    let mut m = msg(
        "assistant",
        json!([{"type": "tool_use", "id": id, "name": name, "input": input}]),
    );
    m.parent = parent.into();
    ev(m)
}

pub fn tool_result(
    id: &str,
    text: &str,
    is_err: bool,
    structured: Option<serde_json::Value>,
) -> Ev {
    ev(claude(
        "user",
        json!([{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": is_err}]),
        structured,
    ))
}

pub fn done(cost: f64) -> Ev {
    Ev::Do(Event::TurnEnd(event::TurnEnd {
        reason: "done".into(),
        cost,
        ..event::TurnEnd::default()
    }))
}

pub fn delta(text: &str) -> Ev {
    Ev::Do(Event::Delta(event::Delta {
        index: 0,
        kind: PartKind::Text,
        text: text.into(),
    }))
}

pub fn thinking(text: &str) -> Ev {
    Ev::Do(Event::Delta(event::Delta {
        index: 0,
        kind: PartKind::Thinking,
        text: text.into(),
    }))
}

/// The agent asks leave to run its call id (a tool of name), as request req.
pub fn approval(call: &str, name: &str, req: &str) -> Ev {
    Ev::Do(Event::Approval(Box::new(event::Approval {
        id: req.into(),
        call: rush_core::tool::Call {
            id: call.into(),
            name: name.into(),
            kind: rush_claude::tools::kind_of(name),
            ..Default::default()
        },
        ..Default::default()
    })))
}

pub const ANSWER: &str = "## What changed\n\nThe **renderer** now caches each turn, so `Render` only redraws what moved.\n\n- the live turn redraws per frame\n- finished turns come from the cache\n- see https://example.com/docs/render for more\n\n1. build it\n2. run the tests\n\n```go\nfunc f() { return }\n```\n\nThat should make streaming feel instant on long sessions, even at wide widths where every row is padded.";

/// n finished turns of narration, reads, greps, commands, edits and a
/// markdown answer, then a live turn streaming its answer (benchEvents).
pub fn bench_events(n: usize) -> Vec<(i64, Ev)> {
    let mut out = Vec::new();
    let mut sec = 0;
    let mut add = |ev: Ev| {
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
        m.tokens = Some(rush_core::usage::TokenUsage {
            input: 10,
            output: 200,
            cache_read: 50000,
            ..Default::default()
        });
        add(ev(m));
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
        add(ev(m));
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
    for (t, e) in bench_events(n) {
        s.feed(e, t);
    }
    s
}
