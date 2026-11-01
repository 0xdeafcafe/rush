//! A message to another session names it as the list does: after
//! internal/convo/messages.go:66-90 (SetPeers, sentTo).

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::HashMap;

use rush_convo::{Options, Renderer, Session, set_peers};
use serde_json::json;

mod common;

use common::*;

fn drawn(to: &str) -> String {
    let mut s = Session::new();
    s.feed(sent("tell them"), at(0));
    s.feed(
        tool_use("m1", "SendMessage", json!({"to": to, "message": "hi"})),
        at(1),
    );
    let o = Options {
        width: 100,
        now: at(2),
        ..Options::default()
    };
    Renderer::new()
        .render(&s, &o)
        .iter()
        .map(|l| l.line.plain())
        .collect::<Vec<_>>()
        .join("\n")
}

// The names are process-wide, so one test sets them in turn.
#[test]
fn a_message_names_its_recipient_as_the_list_does() {
    assert!(drawn("rush-8a").contains("to rush-8a"));
    set_peers(HashMap::from([("rush-8a".into(), "Fix login".into())]));
    let out = drawn("rush-8a");
    assert!(out.contains("Fix login (rush-8a)"), "{out}");
    // A long name is cut at 32 cells with an ellipsis.
    set_peers(HashMap::from([("rush-9c".into(), "n".repeat(40))]));
    let out = drawn("rush-9c");
    let want = format!("{}… (rush-9c)", "n".repeat(31));
    assert!(out.contains(&want), "{out}");
}
