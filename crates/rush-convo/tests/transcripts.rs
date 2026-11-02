//! What reading real transcripts showed drawn wrong: a message to a
//! subagent named by its id (messages.go:33 recipient), and times from
//! before the clocks changed (Go's time.Local, render.go's clock).

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::plain;
use rush_convo::{Options, Renderer, Session};
use rush_core::time::Ms;
use serde_json::json;

mod common;

use common::*;

fn render(s: &Session, o: &Options) -> String {
    plain(&Renderer::new().render(s, o))
}

fn opts() -> Options {
    Options {
        width: 110,
        now: at(4000),
        ..Options::default()
    }
}

/// A session that started a subagent and then sent it a message.
fn messaged(to: &str) -> Session {
    let mut s = Session::new();
    s.feed(sent("trace both"), at(0));
    s.feed(
        tool_use(
            "c1",
            "Agent",
            json!({"description": "trace dead gen", "prompt": "find it", "name": "tracer"}),
        ),
        at(1),
    );
    s.feed(
        tool_result(
            "c1",
            "Async agent launched successfully.",
            false,
            Some(json!({"status": "async_launched", "agentId": "ac56909d3e1f2a7b8", "description": "trace dead gen"})),
        ),
        at(2),
    );
    s.feed(
        tool_use(
            "c2",
            "SendMessage",
            json!({"to": to, "message": "keep going"}),
        ),
        at(3),
    );
    s.feed(tool_result("c2", "sent", false, None), at(4));
    s.feed(done(0.01), at(5));
    s
}

/// A message to a subagent this session started is to what it was asked
/// to do, found by its id or its name; one to an id it didn't start is
/// to "agent" and the id's start; any other name is itself (S48).
#[test]
fn messages_name_the_subagent() {
    for (to, want) in [
        ("ac56909d3e1f2a7b8", "→ to trace dead gen"),
        ("tracer", "→ to trace dead gen"),
        ("a979af0c1d2e3f4a5", "→ to agent a979af0"),
        ("reviewer", "→ to reviewer"),
    ] {
        let out = render(&messaged(to), &opts());
        assert!(out.contains(want), "{to}: missing {want:?} in:\n{out}");
    }
}

/// An hour east before at(1000), none after: as the clocks going back.
fn summer(t: Ms) -> i64 {
    if t < at(1000) { 3600 } else { 0 }
}

/// Each time is in the zone's offset then: a message sent before the
/// clocks changed keeps its hour (S45, S66). T0 is 22:00 UTC.
#[test]
fn times_keep_the_offset_they_had() {
    let mut s = Session::new();
    s.info.started_at = at(0);
    s.feed(sent("before"), at(0));
    s.feed(say("ok"), at(1));
    s.feed(done(0.01), at(2));
    s.feed(sent("after"), at(2000));
    s.feed(say("ok"), at(2001));
    s.feed(done(0.01), at(2002));
    let o = Options {
        zone_at: Some(summer),
        history_open: true,
        ..opts()
    };
    let out = render(&s, &o);
    for w in ["You  23:00", "You  22:33", "started 23:00"] {
        assert!(out.contains(w), "missing {w:?} in:\n{out}");
    }
    // Without it, every time is in the one zone given.
    let out = render(&s, &Options { zone: 0, ..opts() });
    assert!(out.contains("You  22:00"), "{out}");
}

/// A turn read from a history, whose events carry no times, still under
/// way: its working line says what Go's does for its zero time.
#[test]
fn an_untimed_turn_under_way_reads_as_gos() {
    let mut s = Session::new();
    s.feed(sent("sleep 60"), 0);
    let out = render(&s, &opts());
    assert!(out.contains("clearing its throat…"), "{out}");
}
