//! The Needs-you section: after internal/convo/needs_test.go.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::{HashMap, HashSet};

use rush_convo::render::{
    NEEDS_OWN, Needs, NeedsItem, has_quote, needs_choices, needs_of, needs_ref,
};
use rush_convo::{Options, Renderer, Session};

mod common;

use common::*;

/// Two finished turns whose answers both say the quote.
fn needs_session() -> Session {
    let mut s = Session::new();
    s.feed(sent("merge it"), at(0));
    s.feed(say("Merged. I haven't pushed it yet."), at(1));
    s.feed(done(0.0), at(2));
    s.feed(sent("and the other one"), at(10));
    s.feed(
        say("The Claude Code change is merged into main as `de90d0e3`. It merged cleanly, rush builds, \
             and the new test passes on main. I haven't pushed it. It takes effect for sessions started after rush is rebuilt."),
        at(11),
    );
    s.feed(done(0.0), at(12));
    s
}

fn opts(width: usize, needs: Needs) -> Options {
    Options {
        width,
        now: at(20),
        open: HashMap::from([("t1".into(), true), ("t2".into(), true)]),
        needs,
        ..Options::default()
    }
}

// The last turn ends with a Needs-you section: what it wants and the rows
// laid out for it; the answer itself isn't marked, and an earlier turn
// gets no section (TestNeedsSection).
#[test]
fn needs_section() {
    let s = needs_session();
    let mut r = Renderer::new();
    // Drawn once without: the cache mustn't keep it.
    let before = r.render(&s, &opts(46, Needs::default()));
    let before: Vec<String> = before.iter().map(|l| l.line.plain()).collect();
    assert!(
        !before.join("\n").contains("Needs you"),
        "a section with nothing asked:\n{}",
        before.join("\n")
    );
    let n = Needs {
        kind: "you".into(),
        reason: "push the merge".into(),
        quote: "I haven't pushed it.".into(),
        items: [
            ("do", "rebuild rush"),
            ("run", "git push origin main"),
            ("copy", "docs.example.com — the CNAME target"),
            ("reply", "yes, push it"),
        ]
        .iter()
        .map(|(k, t)| NeedsItem {
            kind: (*k).into(),
            text: (*t).into(),
        })
        .collect(),
    };
    let lines = r.render(&s, &opts(80, n));
    let got: Vec<String> = lines
        .iter()
        .map(|l| l.line.plain().trim_end().to_string())
        .collect();
    let refs: HashSet<&str> = lines.iter().map(|l| l.r#ref.as_str()).collect();
    let all = got.join("\n");
    let head = got
        .iter()
        .position(|l| l.contains("╭ ! Needs you · push the merge"));
    let answer = got
        .iter()
        .position(|l| l.contains("The Claude Code change"));
    assert!(
        head.is_some()
            && answer.is_some()
            && head > answer
            && all.matches("Needs you").count() == 1,
        "section at {head:?}, answer at {answer:?}:\n{all}"
    );
    for want in [
        "☐ rebuild rush",
        "$ git push origin main",
        "click copies",
        " 1  yes, push it",
        "╰  ✎  Something else  type your own answer below",
        "⧉ docs.example.com — the CNAME target",
    ] {
        assert!(all.contains(want), "no {want:?} in\n{all}");
    }
    assert!(
        refs.contains(needs_ref("run", "git push origin main").as_str())
            && refs.contains(needs_ref("reply", "yes, push it").as_str())
            && refs.contains(needs_ref(NEEDS_OWN, "").as_str()),
        "the command and reply rows aren't clickable"
    );
    assert!(
        !all.contains('▌') && !all.contains("it said"),
        "the answer is still marked:\n{all}"
    );
}

/// The section's replies are choices, laid out as a question card's
/// options: each a row with its key on a cap, then a row for your own
/// words; the row picked has its cap lit and says what enter does.
#[test]
fn needs_replies_are_choices() {
    let s = needs_session();
    let n = Needs {
        kind: "ask".into(),
        reason: "which way".into(),
        quote: String::new(),
        items: [
            ("note", "both build"),
            ("reply", "ship it now"),
            ("reply", "wait for CI"),
        ]
        .iter()
        .map(|(k, t)| NeedsItem {
            kind: (*k).into(),
            text: (*t).into(),
        })
        .collect(),
    };
    assert_eq!(needs_choices(&n), vec!["ship it now", "wait for CI"]);
    let section = |sel: &str| -> Vec<String> {
        let mut o = opts(80, n.clone());
        o.selected = sel.into();
        let lines = Renderer::new().render(&s, &o);
        let got: Vec<String> = lines
            .iter()
            .map(|l| l.line.plain().trim_end().to_string())
            .collect();
        let at = got.iter().position(|l| l.contains("Needs you")).unwrap();
        got[at..].to_vec()
    };
    let want = [
        "▍   ╭ ? Needs you · which way",
        "▍   │ · both build",
        "▍   │  1  ship it now                                           click to reply",
        "▍   │  2  wait for CI                                           click to reply",
        "▍   ╰  ✎  Something else  type your own answer below          ↑ then 1–2 picks",
    ];
    let got = section("");
    assert_eq!(got, want, "\n{}", got.join("\n"));
    let got = section(&needs_ref("reply", "wait for CI"));
    assert!(
        got[3].contains(" 2  wait for CI") && got[3].contains("enter puts it in your box"),
        "\n{}",
        got.join("\n")
    );
    assert!(!got[4].contains("picks"), "\n{}", got.join("\n"));
    // The picked one's cap is lit: its own ground, not the others'.
    let mut o = opts(80, n.clone());
    o.selected = needs_ref("reply", "wait for CI");
    let lines = Renderer::new().render(&s, &o);
    let ground = |text: &str, cap: &str| {
        let l = lines
            .iter()
            .find(|l| l.line.plain().contains(text))
            .unwrap();
        l.line
            .spans
            .iter()
            .find(|sp| sp.text == cap)
            .map(|sp| sp.style.bg)
            .unwrap()
    };
    assert_ne!(ground("wait for CI", " 2 "), ground("ship it now", " 1 "));
}

// TestNeedsRef.
#[test]
fn needs_ref_round_trips() {
    assert_eq!(needs_of(&needs_ref("run", "a: b")), Some(("run", "a: b")));
    assert_eq!(needs_of("t1:s:x"), None);
}

// TestHasQuote.
#[test]
fn quote_found() {
    for (text, q, want) in [
        (
            "merged as `de90d0e3`.\nI  haven't pushed it.",
            "merged as de90d0e3. I haven't pushed it",
            true,
        ),
        ("**Push** it now", "push it", true),
        ("push it", "", false),
        ("push it", "pull it", false),
    ] {
        assert_eq!(has_quote(text, q), want, "has_quote({text:?}, {q:?})");
    }
}
