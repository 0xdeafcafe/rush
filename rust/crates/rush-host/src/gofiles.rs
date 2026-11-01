//! What Go rush writes and sends (tests/fixtures, written by
//! Go rush at go-final from internal/host), read and written again by Rust,
//! byte for byte.

use rush_core::event::Event;

use crate::lines::{self, Line, decode};
use crate::op::Op;
use crate::wire::{Config, Info};

/// A fixture, its exchanges' times (Go's time.Now(), made in UTC there)
/// in this machine's zone, as Go writes them here.
fn fixture(name: &str) -> String {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    rush_core::wire::in_local_zone(&text, "At")
}

#[test]
fn config_json_as_go_writes_it() {
    for name in ["config_full.json", "config_empty.json"] {
        let want = fixture(name);
        let mut c = Config::parse(want.as_bytes()).unwrap();
        if name == "config_empty.json" {
            c.kind.clear(); // read, an empty kind is Claude Code's, in Go too
        }
        assert_eq!(
            String::from_utf8(c.to_json_indent()).unwrap(),
            want,
            "{name}"
        );
    }
    let c = Config::parse(fixture("config_full.json").as_bytes()).unwrap();
    assert_eq!(c.account.name, "work");
    assert_eq!(c.account.dir, "/x/.claude");
    assert_eq!(c.account.kind.as_str(), "claude");
    assert_eq!(c.retry_base, std::time::Duration::from_secs(15));
    assert!(c.cause.is_empty(), "the cause is never saved");
    // An older rush's config names no kind: it's Claude Code's.
    let c = Config::parse(br#"{"id":"a","account":{"name":"n","configDir":"/d"}}"#).unwrap();
    assert_eq!(
        (c.kind.as_str(), c.account.kind.as_str()),
        ("claude", "claude")
    );
}

#[test]
fn info_json_as_go_writes_it() {
    for name in ["info_full.json", "info_empty.json"] {
        let want = fixture(name);
        let mut i = Info::parse(want.as_bytes()).unwrap();
        if name == "info_empty.json" {
            i.kind.clear(); // read, an empty kind is Claude Code's
        }
        assert_eq!(String::from_utf8(i.to_json()).unwrap(), want, "{name}");
    }
    let i = Info::parse(fixture("info_full.json").as_bytes()).unwrap();
    assert_eq!((i.generation, i.turn, i.proto), (3, 7, 11));
    assert_eq!(
        i.queue_images,
        vec![Vec::<String>::new(), vec!["/p.png".to_string()]]
    );
    assert!(i.queue_exchanges[0].is_none() && i.queue_exchanges[1].as_ref().unwrap().id == "x1");
    assert!(!i.lost, "lost is never written");
}

#[test]
fn ops_as_go_sends_them() {
    for (n, want) in fixture("ops.jsonl").lines().enumerate() {
        let o = Op::parse(want.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(crate::json::line(&o)).unwrap(),
            want,
            "op {n}"
        );
    }
}

#[test]
fn host_lines_as_go_sends_them() {
    let want: Vec<String> = fixture("lines.jsonl").lines().map(str::to_string).collect();
    let info = Info::parse(fixture("info_full.json").as_bytes()).unwrap();
    // publish's line wraps info.json's bytes; serve's is the map's, the
    // same.
    assert_eq!(
        String::from_utf8(lines::info_line(&info.to_json())).unwrap(),
        want[0]
    );
    assert_eq!(want[1], want[0]);
    assert_eq!(
        String::from_utf8(lines::answered_line("r1")).unwrap(),
        want[2]
    );
    assert_eq!(
        String::from_utf8(lines::error_line("no pending request r9")).unwrap(),
        want[3]
    );
    assert_eq!(
        String::from_utf8(lines::time_line(1_793_275_200_123)).unwrap(),
        want[4]
    );
    assert_eq!(
        String::from_utf8(lines::echo_line("hi <b> & you", &[], None)).unwrap(),
        want[5]
    );
    let Line::Sent(s) = decode(want[6].as_bytes()).unwrap() else {
        panic!("not a sent line")
    };
    let ex = s.exchange.clone().unwrap();
    assert_eq!(
        String::from_utf8(lines::echo_line(&s.text, &s.images, Some(&ex))).unwrap(),
        want[6]
    );
    let Line::Commands(c) = decode(want[7].as_bytes()).unwrap() else {
        panic!("not commands")
    };
    assert_eq!(
        String::from_utf8(lines::commands_line(&c)).unwrap(),
        want[7]
    );
    let Line::Reply { id, body, error } = decode(want[8].as_bytes()).unwrap() else {
        panic!("not a reply")
    };
    assert_eq!(
        String::from_utf8(lines::reply_line(&id, body.as_ref(), &error)).unwrap(),
        want[8]
    );
    let Line::Reply { id, body, error } = decode(want[9].as_bytes()).unwrap() else {
        panic!("not a reply")
    };
    assert!(body.is_none());
    assert_eq!(
        String::from_utf8(lines::reply_line(&id, None, &error)).unwrap(),
        want[9]
    );
    let Line::Context(c) = decode(want[10].as_bytes()).unwrap() else {
        panic!("not a context line")
    };
    assert_eq!(
        String::from_utf8(lines::context_line(&c)).unwrap(),
        want[10]
    );
    for l in &want[11..13] {
        let Line::Event(ev) = decode(l.as_bytes()).unwrap() else {
            panic!("not an event: {l}")
        };
        assert_eq!(
            &String::from_utf8(lines::event_line(&ev).unwrap()).unwrap(),
            l
        );
    }
    let Line::Event(Event::TurnEnd(t)) = decode(want[11].as_bytes()).unwrap() else {
        panic!()
    };
    assert_eq!((t.text.as_str(), t.cost), ("Said hi.", 0.01));
    let Line::Info(i) = decode(want[0].as_bytes()).unwrap() else {
        panic!()
    };
    assert_eq!(*i, info);
    let Line::Stamp(t) = decode(want[4].as_bytes()).unwrap() else {
        panic!()
    };
    assert_eq!(t.to_ms(), 1_793_275_200_123);
    assert!(matches!(decode(want[2].as_bytes()).unwrap(), Line::Answered(id) if id == "r1"));
    assert!(
        matches!(decode(want[3].as_bytes()).unwrap(), Line::Error(e) if e == "no pending request r9")
    );
    assert!(lines::is_echo(want[5].as_bytes()) && lines::is_echo(want[6].as_bytes()));
    assert!(lines::is_event_line(want[11].as_bytes(), &["turn_end"]));
    assert!(!lines::is_event_line(want[11].as_bytes(), &["turn"]));
}
