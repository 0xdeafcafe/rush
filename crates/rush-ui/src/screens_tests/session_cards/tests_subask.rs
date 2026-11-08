//! A subagent's ask as its parent's card (subask.rs): who asks what, its
//! keys and buttons as the permission card's, and one rush can't answer
//! opening the subagent. Rust-only: there's no Go for it.

use super::question::{card_key, card_kind};
use super::tests::{acts, hosted, press, rows, set_info, text};
use super::*;

fn asking(request: &str, call: &str) -> SubAsk {
    SubAsk {
        run: "spawn:h9".into(),
        host: "h9".into(),
        who: "Codex · fix the flaky test".into(),
        request: request.into(),
        call: call.into(),
        reason: "stop the stalled helper".into(),
        needs: "Bash kill -TERM 79335".into(),
        question: None,
    }
}

fn answer(allow: bool, why: &str) -> Act {
    Act::AnswerSub {
        host: "h9".into(),
        request: "call_1".into(),
        allow,
        why: why.into(),
    }
}

/// The card says which subagent asks and what, a command a step to a
/// line, with allow, deny and open; y allows it on the subagent's host,
/// and ←→ enter press the buttons as the permission card's do.
#[test]
fn a_subagents_ask_is_its_parents_card() {
    let mut m = hosted(120, 50);
    let a = asking(
        "call_1",
        "Bash: kill -TERM 79335 && python3 helper.py (in /src/app)",
    );
    set_info(&mut m, |i| {
        i.sub_asks = vec![a.clone(), asking("call_2", "Bash: ls")]
    });
    let out = text(&rows(&mut m, 100, 40));
    for w in [
        "╭─ ● Let it run this command?",
        "⇉ from Codex · fix the flaky test",
        "in /src/app",
        "1 of 2",
        "$ kill -TERM 79335 &&",
        "  python3 helper.py",
        "stop the stalled helper",
        "y Allow once",
        "n Deny",
        "o Open it",
    ] {
        assert!(out.contains(w), "card missing {w:?}:\n{out}");
    }
    for l in rows(&mut m, 100, 40) {
        assert_eq!(l.width(), 100, "a row not 100 wide: {:?}", l.plain());
    }
    let c = m.cards().conns["k"].clone();
    assert_eq!(card_kind(&m, "k", &c, &info(&m, "k").unwrap()), "subask");

    press(&mut m, "y");
    assert_eq!(acts(&mut m), vec![answer(true, "")]);

    // With the keys, left from Allow once goes round to Open it.
    m.kept::<super::tests::Fake>().acts.clear();
    press(&mut m, "up");
    assert!(m.cards().conns["k"].card_focus);
    press(&mut m, "left");
    assert_eq!(m.cards().conns["k"].btn_pick, 2);
    press(&mut m, "left");
    press(&mut m, "enter");
    assert_eq!(acts(&mut m), vec![answer(false, "")]);
    assert!(!m.cards().conns["k"].card_focus);
}

/// What's typed is why not: enter denies it, telling the subagent, and
/// the box is emptied.
#[test]
fn typed_words_deny_a_subagents_ask() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| {
        i.sub_asks = vec![asking("call_1", "Bash: rm -rf build")];
        i.input = "use cargo clean".into();
    });
    let out = text(&rows(&mut m, 100, 40));
    assert!(out.contains("or type why not and enter"), "{out}");
    let i = info(&m, "k").unwrap();
    card_key(&mut m, "k", &i, "enter", false);
    assert_eq!(acts(&mut m), vec![answer(false, "use cargo clean")]);
    assert_eq!(info(&m, "k").unwrap().input, "");
}

/// One rush can't answer (a question, or an agent it doesn't host) has
/// only Open it: y does nothing, o opens the subagent.
#[test]
fn a_subagents_question_opens_it() {
    let mut m = hosted(120, 50);
    let mut a = asking("", "");
    a.needs = "Which branch should I use?".into();
    set_info(&mut m, |i| i.sub_asks = vec![a]);
    let out = text(&rows(&mut m, 100, 40));
    for w in [
        "A subagent needs you",
        "Which branch should I use?",
        "open it to answer",
        "o Open it",
    ] {
        assert!(out.contains(w), "card missing {w:?}:\n{out}");
    }
    assert!(!out.contains("Allow once"), "{out}");
    let i = info(&m, "k").unwrap();
    assert_eq!(card_key(&mut m, "k", &i, "y", true), Out::Pass);
    assert!(acts(&mut m).is_empty(), "nothing to allow");
    card_key(&mut m, "k", &i, "o", true);
    assert_eq!(acts(&mut m), vec![Act::OpenRun("spawn:h9".into())]);
}

/// A click on a button presses it.
#[test]
fn a_click_answers_a_subagents_ask() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| {
        i.sub_asks = vec![asking("call_1", "Bash: make")]
    });
    rows(&mut m, 100, 40);
    let (top, _, _) = super::card_top(&m, "k");
    let b = m.cards().conns["k"]
        .btns
        .iter()
        .find(|b| b.key == "n")
        .cloned()
        .expect("a deny button");
    let i = info(&m, "k").unwrap();
    // The pane starts 2 in; the button's middle row is its name's.
    super::question::click(&mut m, "k", &i, 2 + b.x + 1, top as usize + b.y + 1);
    assert_eq!(acts(&mut m), vec![answer(false, "")]);
}

/// The session's own ask comes first; the subagent's is under it.
#[test]
fn the_sessions_own_ask_goes_first() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| {
        i.sub_asks = vec![asking("call_1", "Bash: make")]
    });
    super::tests::ask(
        &mut m,
        "Bash",
        rush_core::tool::Kind::Shell,
        rush_core::tool::Input {
            command: "ls".into(),
            ..Default::default()
        },
        false,
    );
    let c = m.cards().conns.get("k").cloned().unwrap_or_default();
    assert_eq!(card_kind(&m, "k", &c, &info(&m, "k").unwrap()), "approval");
}

/// A subagent's question, whole, as rush hosts it.
fn questioning(host: &str) -> SubAsk {
    let choice = |l: &str| rush_core::event::Choice {
        label: l.into(),
        description: format!("work on {l}"),
        ..Default::default()
    };
    SubAsk {
        run: "spawn:h9".into(),
        host: host.into(),
        who: "Codex · fix the flaky test".into(),
        request: "q_1".into(),
        needs: "Which branch?".into(),
        question: Some(rush_core::event::Question {
            id: "q_1".into(),
            asks: vec![rush_core::event::Ask {
                header: "Branch".into(),
                text: "Which branch?".into(),
                options: vec![choice("main"), choice("next")],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..SubAsk::default()
    }
}

/// A subagent's question rush hosts is the session's own question card,
/// saying whose it is: picked, its answers go to the subagent's host as
/// the card's own would go to the session's; skipped, the subagent is
/// told so; alt+o opens it.
#[test]
fn a_subagents_question_is_answered_on_its_card() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| i.sub_asks = vec![questioning("h9")]);
    let out = text(&rows(&mut m, 100, 40));
    for w in [
        "? Branch",
        "⇉ from Codex · fix the flaky test",
        "Which branch?",
        "main",
        "work on next",
    ] {
        assert!(out.contains(w), "card missing {w:?}:\n{out}");
    }
    assert!(!out.contains("Claude asks"), "{out}");
    let c = m.cards().conns["k"].clone();
    assert_eq!(
        card_kind(&m, "k", &c, &info(&m, "k").unwrap()),
        "subquestion"
    );
    assert!(!c.card_focus, "its letters are a message's until ↑");

    press(&mut m, "up");
    press(&mut m, "down");
    press(&mut m, "enter");
    let got = acts(&mut m);
    let [
        Act::AnswerSubQuestion {
            host,
            request,
            input,
        },
    ] = got.as_slice()
    else {
        panic!("not a subagent's answer: {got:?}");
    };
    assert_eq!((host.as_str(), request.as_str()), ("h9", "q_1"));
    let v: serde_json::Value = serde_json::from_str(input).unwrap();
    assert_eq!(v["answers"]["Which branch?"], "next", "{input}");

    m.kept::<super::tests::Fake>().acts.clear();
    rows(&mut m, 100, 40);
    let i = info(&m, "k").unwrap();
    card_key(&mut m, "k", &i, "alt+o", true);
    assert_eq!(acts(&mut m), vec![Act::OpenRun("spawn:h9".into())]);

    m.kept::<super::tests::Fake>().acts.clear();
    press(&mut m, "up");
    press(&mut m, "s");
    let got = acts(&mut m);
    assert!(
        matches!(
            got.as_slice(),
            [Act::AnswerSub { host, request, allow: false, why }]
                if host == "h9" && request == "q_1" && why.contains("skipped")
        ),
        "{got:?}"
    );
}

/// A question of a subagent rush doesn't host stays one to open.
#[test]
fn an_unhosted_subagents_question_opens_it() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| i.sub_asks = vec![questioning("")]);
    let out = text(&rows(&mut m, 100, 40));
    assert!(out.contains("open it to answer"), "{out}");
    let c = m.cards().conns["k"].clone();
    assert_eq!(card_kind(&m, "k", &c, &info(&m, "k").unwrap()), "subask");
}
