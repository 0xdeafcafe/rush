//! The card the conversation opens on: after Go's
//! internal/convo/opening_test.go (TestOpeningCard,
//! TestOpeningCardStartedBeside, TestOpeningBrief).

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::HashMap;

use rush_convo::render::plain;
use rush_convo::{Options, Renderer, Session};

mod common;

use common::*;

/// A turn you sent and its answer, ended.
fn session() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(sent("the ux right now is totally broken"), at(0));
    s.feed(say("Looking at how attach restores the terminal."), at(1));
    s.feed(done(0.02), at(2));
    s
}

fn render(s: &Session, o: &Options) -> String {
    plain(&Renderer::new().render(s, o))
}

fn opts() -> Options {
    Options {
        width: 110,
        now: at(40),
        history_open: true,
        ..Options::default()
    }
}

/// The first turn opens on a card: when the session began and who began
/// it, what it runs as, and its commands, listed once opened
/// (TestOpeningCard).
#[test]
fn opening_card() {
    let mut s = session();
    s.info.started_at = at(0);
    s.info.model = "claude-opus-5-5[1m]".into();
    s.info.effort = "high".into();
    s.info.spawned_by = "p1".into();
    s.commands = vec!["review".into(), "ship".into()];
    let mut o = opts();
    o.parent = "fix login".into();
    let out = render(&s, &o);
    for w in [
        "◆ session · started",
        "by fix login, as its subagent",
        "opus 5.5 · 1M · high",
        "2 commands and skills ▸ list",
    ] {
        assert!(out.contains(w), "missing {w:?} in:\n{out}");
    }
    assert_eq!(out.matches("◆ session").count(), 1, "{out}");
    assert!(
        out.find("◆ session") < out.find("the ux"),
        "card not above the first prompt:\n{out}"
    );
    o.open = HashMap::from([("t1:opening:skills".to_string(), true)]);
    let out = render(&s, &o);
    assert!(out.contains("/review  /ship"), "{out}");
}

/// One another session started beside it says who did, and isn't its
/// subagent (TestOpeningCardStartedBeside); without its name, "another
/// session".
#[test]
fn opening_card_started_beside() {
    let mut s = session();
    s.info.started_at = at(0);
    s.info.model = "sonnet".into();
    s.info.started_from = "p1".into();
    let mut o = opts();
    o.started_from = "fix login".into();
    let out = render(&s, &o);
    assert!(out.contains("· by fix login"), "{out}");
    assert!(!out.contains("subagent"), "{out}");
    o.started_from.clear();
    let out = render(&s, &o);
    assert!(out.contains("· by another session"), "{out}");
}

/// Who began it, each way the card says it (startedBy).
#[test]
fn started_by_words() {
    type Set = fn(&mut Session, &mut Options);
    let cases: [(&str, Set); 4] = [
        ("by you", |_, _| {}),
        ("by another session, as its subagent", |s, _| {
            s.info.spawned_by = "p1".into()
        }),
        ("by the weather plugin", |s, _| {
            s.info.started_by = "weather".into()
        }),
        ("by planner, as its subagent", |_, o| {
            o.parent = "planner".into()
        }),
    ];
    for (want, set) in cases {
        let mut s = session();
        s.info.started_at = at(0);
        let mut o = opts();
        set(&mut s, &mut o);
        let out = render(&s, &o);
        assert!(out.contains(want), "{want:?} not in:\n{out}");
    }
}

/// A transcript read cold, with nothing known of how it began, has no
/// card.
#[test]
fn no_card_read_cold() {
    let out = render(&session(), &opts());
    assert!(!out.contains("◆ session"), "{out}");
}

/// A hosted session's card says what was added to its agent's prompt,
/// and opens on the brief it was started with (TestOpeningBrief).
#[test]
fn opening_brief() {
    let mut s = session();
    s.info.started_at = at(0);
    s.info.model = "sonnet".into();
    let mut o = opts();
    o.hosted = true;
    let out = render(&s, &o);
    assert!(out.contains("prompt: rush's own\n"), "{out}");
    o.brief = "Work on the login bug.\nReport back.".into();
    let out = render(&s, &o);
    assert!(out.contains("and its brief · 2 lines ▸ show"), "{out}");
    assert!(!out.contains("Report back."), "{out}");
    o.open = HashMap::from([("t1:opening:brief".to_string(), true)]);
    let out = render(&s, &o);
    assert!(out.contains("Report back."), "{out}");
}

/// The card changes as what it reads does: a renderer that drew it once
/// draws it again when the session that started it gets a name.
#[test]
fn card_redrawn_when_it_changes() {
    let mut s = session();
    s.info.started_at = at(0);
    s.info.started_from = "p1".into();
    let mut r = Renderer::new();
    let mut o = opts();
    let out = plain(&r.render(&s, &o));
    assert!(out.contains("by another session"), "{out}");
    o.started_from = "fix login".into();
    let out = plain(&r.render(&s, &o));
    assert!(out.contains("by fix login"), "{out}");
}
