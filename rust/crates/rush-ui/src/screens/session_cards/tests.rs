//! The session cards' tests, ported from Go's internal/ui (permcard_test.go,
//! question_test.go, decisions_test.go, decisioncontext_test.go,
//! permissions_test.go and the sheets' and panels' tests). No sleeps: a
//! model with no loop runs handed-off work at once.

use std::collections::HashMap;
use std::path::PathBuf;

use rush_core::event::{Approval, ApprovalOption, Ask, Choice, Event, OptionKind, Question};
use rush_core::text::Line;
use rush_core::tool::{Call, Input, Kind, Replace};

use super::question::*;
use super::*;
use crate::frame::Press;
use crate::session::{SessionView, Source};

/// A host for tests: each session's info, and what was asked of it.
#[derive(Default)]
pub(super) struct Fake {
    pub info: HashMap<String, HostInfo>,
    pub acts: Vec<(String, Act)>,
}

fn fake_info(m: &Model, key: &str) -> Option<HostInfo> {
    m.kept.peek::<Fake>()?.info.get(key).cloned()
}

fn fake_act(m: &mut Model, key: &str, a: Act) -> Out {
    m.kept::<Fake>().acts.push((key.to_string(), a));
    Out::Draw
}

fn fake_input(m: &mut Model, key: &str, text: &str) {
    if let Some(i) = m.kept::<Fake>().info.get_mut(key) {
        i.input = text.to_string();
    }
}

/// A model of w×h with agent "k"'s conversation open and hosted.
pub(super) fn hosted(w: usize, h: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(1_793_190_896_789);
    m.w = w;
    m.h = h;
    m.cards().host = Some(Host {
        info: fake_info,
        act: fake_act,
        set_input: fake_input,
    });
    m.kept::<Fake>().info.insert(
        "k".into(),
        HostInfo {
            kind: "claude".into(),
            ..HostInfo::default()
        },
    );
    let src = Source::Transcript {
        path: PathBuf::from("/nonexistent/t.jsonl"),
        decode: rush_convo::decode(rush_claude::messages),
    };
    let mut v = SessionView::new("k".into(), src, 1);
    v.read = true;
    m.session = Some(v);
    m
}

pub(super) fn apply(m: &mut Model, ev: Event) {
    m.session.as_mut().unwrap().sess.apply(&ev, 1_000);
}

pub(super) fn acts(m: &mut Model) -> Vec<Act> {
    m.kept::<Fake>().acts.iter().map(|(_, a)| a.clone()).collect()
}

pub(super) fn set_info(m: &mut Model, f: impl FnOnce(&mut HostInfo)) {
    f(m.kept::<Fake>().info.get_mut("k").unwrap());
}

pub(super) fn text(ls: &[Line]) -> String {
    ls.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s:?}")))
}

fn ask(m: &mut Model, tool: &str, kind: Kind, input: Input, always: bool) {
    let call = Call {
        id: "b1".into(),
        name: tool.into(),
        kind,
        input,
        ..Call::default()
    };
    let mut options = vec![ApprovalOption {
        id: "y".into(),
        label: "Yes".into(),
        kind: OptionKind::AllowOnce,
    }];
    if always {
        options.push(ApprovalOption {
            id: "a".into(),
            label: "Always allow Bash(curl:*)".into(),
            kind: OptionKind::AllowAlways,
        });
    }
    apply(
        m,
        Event::Approval(Box::new(Approval {
            id: "r1".into(),
            call,
            options,
            ..Approval::default()
        })),
    );
}

fn rows(m: &mut Model, w: usize, max_h: usize) -> Vec<Line> {
    let i = info(m, "k").unwrap();
    card_rows(m, "k", &i, w, max_h)
}

const PERM_CMD: &str = r#"mkdir -p "$TMPDIR/xterm-src" && cd "$TMPDIR/xterm-src" && for f in cursor.c screen.c; do curl -fsSL "https://example.com/$f" -o "$f" & done; wait; ls -la"#;

// ---- permcard_test.go ----

/// A command to allow gets the framed card: its question and where it
/// runs in the top edge, the chain a step to a line, and a button for
/// each answer.
#[test]
fn permission_card() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| i.cwd = "/src/app".into());
    ask(
        &mut m,
        "Bash",
        Kind::Shell,
        Input {
            command: PERM_CMD.into(),
            ..Input::default()
        },
        false,
    );
    // Typing as it came: it waits for ↑.
    set_info(&mut m, |i| i.input = "x".into());
    let out = text(&rows(&mut m, 90, 40));
    set_info(&mut m, |i| i.input.clear());
    for w in [
        "╭─ ● Run this command?",
        "in /src/app",
        r#"$ mkdir -p "$TMPDIR/xterm-src" &&"#,
        r#"  cd "$TMPDIR/xterm-src" &&"#,
        "  wait;",
        "  ls -la",
        "y Allow once",
        "n Deny",
        "↑ to answer",
    ] {
        assert!(out.contains(w), "card missing {w:?}:\n{out}");
    }
    for l in rows(&mut m, 90, 40) {
        assert_eq!(l.width(), 90, "a row not 90 wide: {:?}", l.plain());
    }
    assert!(m.cards().conns["k"].btns.len() >= 2);

    // With the keys, ←→ pick a button and enter presses it: left from
    // Allow once goes round to Deny.
    press(&mut m, "up");
    assert!(m.cards().conns["k"].card_focus, "up didn't give the card the keys");
    press(&mut m, "left");
    assert_eq!(m.cards().conns["k"].btn_pick, 1);
    let focused = text(&rows(&mut m, 90, 40));
    assert!(focused.contains("←→ pick"), "{focused}");
    press(&mut m, "enter");
    assert_eq!(
        acts(&mut m),
        vec![Act::Deny {
            id: "r1".into(),
            why: String::new()
        }]
    );
    assert!(!m.cards().conns["k"].card_focus);
}

/// A file change keeps its diff inside the frame.
#[test]
fn permission_card_edit() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| i.cwd = "/src/app".into());
    ask(
        &mut m,
        "Edit",
        Kind::Edit,
        Input {
            path: "/src/app/main.go".into(),
            edits: vec![Replace {
                old: "a := 1".into(),
                new: "a := 2".into(),
                ..Replace::default()
            }],
            ..Input::default()
        },
        false,
    );
    let out = text(&rows(&mut m, 78, 40));
    for w in ["Change this file?", "main.go", "− a := 1", "+ a := 2"] {
        assert!(out.contains(w), "card missing {w:?}:\n{out}");
    }
}

#[test]
fn shell_lines() {
    use super::permcard::shell_lines;
    for (cmd, want) in [
        ("a && b || c; d | e", vec!["a &&", "b ||", "c;", "d |", "e"]),
        (r#"echo "x && y"; z"#, vec![r#"echo "x && y";"#, "z"]),
        ("x=$(a && b) && c", vec!["x=$(a && b) &&", "c"]),
        ("a 2>&1 | b", vec!["a 2>&1 |", "b"]),
        ("go test \\\n  ./... && echo ok", vec!["go test   ./... &&", "echo ok"]),
        ("one\ntwo", vec!["one", "two"]),
    ] {
        assert_eq!(shell_lines(cmd), want, "{cmd:?}");
    }
}

/// With the box empty, y, a and n answer a permission card; once a
/// message is under way they're the box's, and enter denies with it as
/// the reason (question_test.go TestCardsNeedFocus).
#[test]
fn cards_need_focus() {
    let mut m = hosted(120, 50);
    ask(
        &mut m,
        "Bash",
        Kind::Shell,
        Input {
            command: "curl x".into(),
            ..Input::default()
        },
        true,
    );
    set_info(&mut m, |i| i.input = "why".into());
    rows(&mut m, 90, 40); // it came while typing: no keys
    let i = info(&m, "k").unwrap();
    assert_eq!(card_key(&mut m, "k", &i, "y", false), Out::Pass);
    assert_eq!(card_key(&mut m, "k", &i, "1", false), Out::Pass);
    card_key(&mut m, "k", &i, "enter", false);
    assert_eq!(
        acts(&mut m),
        vec![Act::Deny {
            id: "r1".into(),
            why: "why".into()
        }]
    );
    assert_eq!(info(&m, "k").unwrap().input, "");
    // Empty, a answers it for good; alt+y from anywhere.
    let i = info(&m, "k").unwrap();
    card_key(&mut m, "k", &i, "a", true);
    card_key(&mut m, "k", &i, "alt+y", false);
    let a = acts(&mut m);
    assert_eq!(
        a[1],
        Act::Allow {
            id: "r1".into(),
            input: None,
            always: true
        }
    );
    assert_eq!(
        a[2],
        Act::Allow {
            id: "r1".into(),
            input: None,
            always: false
        }
    );
}

/// Always allow's button says the agent's rule; yolo shows only when the
/// harness has a don't-ask mode it isn't in (TestCardYolo).
#[test]
fn card_buttons_and_yolo() {
    let mut m = hosted(120, 50);
    set_info(&mut m, |i| {
        i.permission_modes = vec![
            Mode {
                id: "default".into(),
                ..Mode::default()
            },
            Mode {
                id: "bypassPermissions".into(),
                ..Mode::default()
            },
        ];
        i.permission_mode = "default".into();
    });
    ask(
        &mut m,
        "Bash",
        Kind::Shell,
        Input {
            command: "curl x".into(),
            ..Input::default()
        },
        true,
    );
    let out = text(&rows(&mut m, 120, 40));
    assert!(out.contains("a Always allow Bash(curl:*)"), "{out}");
    assert!(out.contains("o Yolo"), "{out}");
    let i = info(&m, "k").unwrap();
    card_key(&mut m, "k", &i, "up", true);
    assert!(m.cards().conns["k"].card_focus);
    card_key(&mut m, "k", &i, "o", true);
    let a = acts(&mut m);
    assert_eq!(
        a,
        vec![
            Act::Allow {
                id: "r1".into(),
                input: None,
                always: false
            },
            Act::Permission("bypassPermissions".into())
        ]
    );
    set_info(&mut m, |i| i.permission_mode = "bypassPermissions".into());
    let out = text(&rows(&mut m, 120, 40));
    assert!(!out.contains("o Yolo"), "{out}");
}

#[test]
fn always_words() {
    use super::permcard::{always_label, always_words};
    assert_eq!(always_words("Always allow Bash(pnpm lint:*)"), "always allow Bash(pnpm lint:*)");
    assert_eq!(always_words("Yes, for this session"), "allow for this session");
    assert_eq!(always_words(""), "always allow");
    assert_eq!(always_label("Yes, for this session"), "Allow for this session");
}

// ---- question_test.go ----

fn ask_req() -> Question {
    Question {
        id: "q1".into(),
        asks: vec![
            Ask {
                text: "Which rules first?".into(),
                header: "Lint".into(),
                options: vec![choice("no-floating-promises"), choice("explicit return types")],
                ..Ask::default()
            },
            Ask {
                text: "Which packages?".into(),
                multi: true,
                options: vec![choice("mcp"), choice("skills"), choice("web")],
                ..Ask::default()
            },
        ],
        ..Question::default()
    }
}

fn choice(l: &str) -> Choice {
    Choice {
        label: l.into(),
        ..Choice::default()
    }
}

fn qk(c: &mut Conn, req: &Question, s: &str, empty: bool) -> Res {
    let mut input = String::new();
    question_key(c, req, None, s, empty, &mut input)
}

#[test]
fn answer_questions() {
    let mut c = Conn::default();
    let req = ask_req();
    // A digit answers a single-choice question and moves to the next.
    assert_ne!(qk(&mut c, &req, "1", true), Res::No);
    assert_eq!(c.q_idx, 1);
    // Multi-select: digits toggle, enter confirms.
    for d in ["1", "3", "3", "2"] {
        qk(&mut c, &req, d, true);
    }
    assert_eq!(qk(&mut c, &req, "enter", true), Res::Yes);
    assert_eq!(c.q_idx, 2, "two questions end on the review");
    assert_eq!(c.q_answer["Which rules first?"], "no-floating-promises");
    assert_eq!(c.q_answer["Which packages?"], "mcp, skills");
    // From the review a digit goes back to a question, the cursor on what
    // was chosen, and answering returns to the review.
    c.card_focus = true;
    qk(&mut c, &req, "1", true);
    assert_eq!((c.q_idx, c.q_cursor), (0, 0));
    qk(&mut c, &req, "down", true);
    qk(&mut c, &req, "enter", true);
    assert_eq!(c.q_idx, 2);
    assert_eq!(c.q_answer["Which rules first?"], "explicit return types");
    // ← from the review is the last question, its ticks kept.
    qk(&mut c, &req, "left", true);
    assert_eq!(c.q_idx, 1);
    assert!(c.picks(1)[&0] && c.picks(1)[&1] && !c.picks(1).get(&2).copied().unwrap_or(false));
    qk(&mut c, &req, "right", true);
    match qk(&mut c, &req, "enter", true) {
        Res::Do(Act::Allow { id, input: Some(_), .. }) => assert_eq!(id, "q1"),
        r => panic!("enter on the review should reply: {r:?}"),
    }
    assert_eq!(c.q_for, "");
    // A lone question sends as soon as it's answered.
    let one = Question {
        id: "q2".into(),
        asks: vec![Ask {
            text: "Go?".into(),
            options: vec![choice("yes"), choice("no")],
            ..Ask::default()
        }],
        ..Question::default()
    };
    let mut c1 = Conn::default();
    assert!(matches!(qk(&mut c1, &one, "2", true), Res::Do(_)));
    // Typed text answers in your own words.
    let mut c2 = Conn::default();
    let mut input = "both, but start with promises".to_string();
    question_key(&mut c2, &req, None, "enter", false, &mut input);
    assert_eq!(c2.q_answer["Which rules first?"], "both, but start with promises");
    assert_eq!(input, "");
    // Letters still type while a question is up.
    assert_eq!(qk(&mut c2, &req, "a", true), Res::No);
}

/// On a multi-select question, enter on an option only ticks it; the
/// Continue button under the options moves on.
#[test]
fn multi_select_continue() {
    let mut c = Conn {
        card_focus: true,
        ..Conn::default()
    };
    let req = ask_req();
    qk(&mut c, &req, "1", true);
    qk(&mut c, &req, "enter", true);
    qk(&mut c, &req, "enter", true);
    assert_eq!(c.q_idx, 1);
    assert!(!c.picks(1).get(&0).copied().unwrap_or(false));
    qk(&mut c, &req, "enter", true);
    for _ in 0..4 {
        qk(&mut c, &req, "down", true);
    }
    assert_eq!(c.q_cursor, 4, "the cursor reaches Continue");
    assert_eq!(qk(&mut c, &req, "down", true), Res::No, "↓ past Continue leaves");
    let (_, qs) = questions(&req);
    let lines = draw_question(&mut c, "", &qs, 100, 0, 0);
    assert!(text(&lines).contains("Continue"));
    qk(&mut c, &req, "enter", true);
    assert_eq!(c.q_idx, 2);
    assert_eq!(c.q_answer["Which packages?"], "mcp");
}

#[test]
fn question_card_draws() {
    let req = Question {
        id: "q9".into(),
        asks: vec![
            Ask {
                text: "Which layout?".into(),
                header: "Layout".into(),
                options: vec![
                    Choice {
                        label: "Split (Recommended)".into(),
                        description: "Agents left, Session right".into(),
                        preview: "```\n┌────┬────────┐\n│ A  │ S      │\n└────┴────────┘\n```".into(),
                        ..Choice::default()
                    },
                    Choice {
                        label: "Stacked".into(),
                        preview: "┌────────┐\n│ A      │\n├────────┤\n│ S      │\n└────────┘".into(),
                        ..Choice::default()
                    },
                ],
                ..Ask::default()
            },
            Ask {
                text: "Theme?".into(),
                header: "Theme".into(),
                options: vec![choice("dark"), choice("light")],
                ..Ask::default()
            },
        ],
        ..Question::default()
    };
    let mut c = Conn {
        card_focus: true,
        ..Conn::default()
    };
    let draw = |c: &mut Conn, w: usize| -> String {
        let ls = question_card(c, &req, None, w, 0);
        for l in &ls {
            assert!(l.width() <= w, "row wider than {w}: {:?}", l.plain());
        }
        text(&ls)
    };
    let wide = draw(&mut c, 120);
    for want in [
        " Layout ",
        "○ Theme",
        "○ send",
        "0 of 2 answered",
        "1  Split  ★ recommended",
        "╭─ Split ─",
        "│ A  │ S      │",
        "←→ questions",
    ] {
        assert!(wide.contains(want), "wide card missing {want:?}:\n{wide}");
    }
    assert!(
        wide.lines()
            .any(|l| l.contains("Split  ★ recommended") && l.contains("╭─ Split")),
        "preview beside the options:\n{wide}"
    );
    assert!(!wide.contains("```"));
    let narrow = draw(&mut c, 70);
    assert!(narrow.contains("◇ preview") && narrow.contains("│ A  │ S      │"), "{narrow}");
    qk(&mut c, &req, "1", true);
    qk(&mut c, &req, "2", true);
    let review = draw(&mut c, 120);
    for want in [
        "✓ Layout Split",
        " send ",
        "Send these answers?",
        "Layout   ✓ Split",
        "Theme    ✓ light",
        "sends your answers",
    ] {
        assert!(review.contains(want), "review missing {want:?}:\n{review}");
    }
}

#[test]
fn split_ask() {
    for (q, lead, ask) in [
        ("Which layout?", "", "Which layout?"),
        ("The check blocked this. Proceed?", "The check blocked this.", "Proceed?"),
        ("Context here.\n\nWhich one, e.g. this?", "Context here.", "Which one, e.g. this?"),
        ("Use e.g. foo?", "", "Use e.g. foo?"),
        ("Pick one. Done.", "", "Pick one. Done."),
    ] {
        assert_eq!(super::question::split_ask(q), (lead.into(), ask.into()), "{q:?}");
    }
}

/// A long question keeps to its height by folding what isn't under the
/// cursor, and a description is never cut while there's room.
#[test]
fn question_card_folds() {
    let long = "a reason that goes on ".repeat(20);
    let o = |l: &str| Choice {
        label: l.into(),
        description: long.clone(),
        ..Choice::default()
    };
    let req = Question {
        id: "q5".into(),
        asks: vec![Ask {
            text: "Go?".into(),
            options: vec![o("one"), o("two"), o("three")],
            ..Ask::default()
        }],
        ..Question::default()
    };
    let mut c = Conn {
        card_focus: true,
        ..Conn::default()
    };
    let full = question_card(&mut c, &req, None, 100, 0);
    assert!(!text(&full).contains('…'), "cut with room to spare:\n{}", text(&full));
    let folded = question_card(&mut c, &req, None, 100, 16);
    assert!(folded.len() <= 16 && folded.len() < full.len(), "{} of {}", folded.len(), full.len());
}

#[test]
fn answers_carry_preview() {
    let req = Question {
        id: "q3".into(),
        asks: vec![
            Ask {
                text: "Which?".into(),
                options: vec![
                    Choice {
                        label: "a".into(),
                        preview: "A!".into(),
                        ..Choice::default()
                    },
                    choice("b"),
                ],
                ..Ask::default()
            },
            Ask {
                text: "And?".into(),
                options: vec![choice("x")],
                ..Ask::default()
            },
        ],
        ..Question::default()
    };
    let answers: HashMap<String, String> = [("Which?", "a"), ("And?", "my own")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let v: serde_json::Value = serde_json::from_str(&answer_input(&req, &answers)).unwrap();
    assert_eq!(v["questions"].as_array().unwrap().len(), 2);
    assert_eq!(v["answers"]["Which?"], "a");
    assert_eq!(v["answers"]["And?"], "my own");
    assert_eq!(v["annotations"]["Which?"]["preview"], "A!");
    assert!(v["annotations"].get("And?").is_none());
}

/// Full view shows one option and its whole preview; a first click on an
/// option only puts the cursor there, a second picks it
/// (TestQuestionFullViewAndClicks).
#[test]
fn question_full_view_and_clicks() {
    let mut m = hosted(104, 90);
    let tall = format!("{}\nlast row", "row\n".repeat(20).trim_end());
    let q = Question {
        id: "q1".into(),
        call_id: "b1".into(),
        asks: vec![Ask {
            text: "Which?".into(),
            options: vec![
                Choice {
                    label: "tall".into(),
                    preview: tall,
                    ..Choice::default()
                },
                Choice {
                    label: "plain".into(),
                    description: "no picture".into(),
                    ..Choice::default()
                },
            ],
            ..Ask::default()
        }],
        ..Question::default()
    };
    apply(&mut m, Event::Question(q));
    let draw = |m: &mut Model| text(&m.frame());
    // Up gives the card the keys (a question waits for ↑).
    press(&mut m, "up");
    assert!(m.cards().conns["k"].card_focus);
    assert!(!draw(&mut m).contains("last row"), "the list cuts a tall preview");
    press(&mut m, "v");
    let out = draw(&mut m);
    assert!(out.contains("last row") && !out.contains("plain"), "{out}");
    press(&mut m, "down");
    let out = draw(&mut m);
    assert!(out.contains("no preview for this option") && out.contains("no picture"), "{out}");
    press(&mut m, "esc");
    assert!(!m.cards().conns["k"].q_full);

    m.cards().conns.get_mut("k").unwrap().q_cursor = 0;
    draw(&mut m);
    let c = &m.cards().conns["k"];
    let row = c
        .q_hit
        .iter()
        .filter(|(_, k)| *k == "@1")
        .map(|(r, _)| *r)
        .min()
        .unwrap();
    let y = c.card_top + row;
    m.cards().conns.get_mut("k").unwrap().q_cursor = 1;
    let click = |m: &mut Model| {
        m.mouse(rush_draw::MouseKind::Down(rush_draw::Button::Left), 8, y as u16);
    };
    click(&mut m);
    assert!(acts(&mut m).is_empty(), "one click never answers");
    assert_eq!(m.cards().conns["k"].q_cursor, 1);
    click(&mut m);
    assert!(
        matches!(acts(&mut m).first(), Some(Act::Allow { id, .. }) if id == "q1"),
        "a second click picks it"
    );
}

// ---- decisions_test.go, decisioncontext_test.go ----

const DECISIONS_MSG: &str = "Decisions:\n\
1. **Scope:** (a) the full recommendation; (b) the easier alternative.\n\
2. **Answer format:** (a) `1a. Topic: label` lines; (b) keep `Q:/A:`.\n\
My picks: 1a 2b.";

fn decisions_model(msg: &str) -> Model {
    let mut m = hosted(120, 50);
    let d = decisions_of(msg, "d1");
    set_info(&mut m, |i| i.decisions = d);
    m
}

/// A turn that ended with decisions shows them as a card with the
/// agent's picks marked and under the cursor; a pick of your own, then a
/// takes its picks for the rest, and the answers go as your next message.
#[test]
fn decisions_card() {
    let mut m = decisions_model(DECISIONS_MSG);
    let i = info(&m, "k").unwrap();
    assert_eq!(card_kind(&m, "k", &Conn::default(), &i), "decisions");
    card_key(&mut m, "k", &i, "up", true);
    let card = text(&rows(&mut m, 100, 0));
    for want in [
        "? decide",
        "2 decisions",
        "1. Scope",
        "the full recommendation  ★ its pick",
        "a takes its picks",
        "s sends nothing",
    ] {
        assert!(card.contains(want), "card missing {want:?}:\n{card}");
    }
    assert_eq!(m.cards().conns["k"].q_cursor, 0);
    card_key(&mut m, "k", &i, "2", true);
    let c = &m.cards().conns["k"];
    assert_eq!((c.q_idx, c.q_cursor, c.card_focus), (1, 1, true));
    assert_eq!(card_key(&mut m, "k", &i, "a", true), Out::Draw);
    assert_eq!(
        acts(&mut m),
        vec![Act::Send(
            "1b. Scope: the easier alternative\n\n2b. Answer format: keep Q:/A:".into()
        )]
    );
    let c = m.cards().conns["k"].clone();
    assert_eq!(card_kind(&m, "k", &c, &i), "", "the card stays once answered");
}

/// What's typed goes as a message, not an answer, unless you chose to
/// answer in your own words; s sets the card aside and sends nothing.
#[test]
fn decisions_card_typing_and_dismiss() {
    let mut m = decisions_model(DECISIONS_MSG);
    let i = info(&m, "k").unwrap();
    assert_eq!(card_key(&mut m, "k", &i, "enter", false), Out::Pass);
    for k in ["up", "down", "down", "enter"] {
        card_key(&mut m, "k", &i, k, true);
    }
    let c = &m.cards().conns["k"];
    assert!(c.q_own && !c.card_focus);
    set_info(&mut m, |i| i.input = "only the parser".into());
    let i = info(&m, "k").unwrap();
    assert_ne!(card_key(&mut m, "k", &i, "enter", false), Out::Pass);
    let c = &m.cards().conns["k"];
    assert_eq!(c.q_answer["1. Scope"], "only the parser");
    assert!(!c.q_own);
    let i = info(&m, "k").unwrap();
    assert_ne!(card_key(&mut m, "k", &i, "s", true), Out::Pass);
    assert_eq!(acts(&mut m), vec![Act::DismissDecisions("d1".into())]);
    let c = m.cards().conns["k"].clone();
    assert_eq!(card_kind(&m, "k", &c, &i), "");
}

const LOADER_MSG: &str = "The lane is done.\n\nDecisions:\n\
1. **Committing the loader work:** rush's permission check stopped the lane's commit, so this needs your go-ahead. (a) yes: I commit it on perf/loader as one commit per fix, nothing pushed; (b) no: leave it uncommitted for you to look at first.\n\
My picks: 1a.";

/// One option a line, "no" a label like "yes", the pick starred; a takes
/// it.
#[test]
fn decisions_card_loader() {
    let mut m = decisions_model(LOADER_MSG);
    let i = info(&m, "k").unwrap();
    card_key(&mut m, "k", &i, "up", true);
    let card = text(&rows(&mut m, 100, 0));
    for want in [
        "rush's permission check stopped",
        "1  yes  ★ its pick",
        "I commit it on perf/loader",
        "2  no",
        "leave it uncommitted for you to look at first",
        "a takes its picks",
    ] {
        assert!(card.contains(want), "card missing {want:?}:\n{card}");
    }
    assert!(!card.contains("no: leave"));
    card_key(&mut m, "k", &i, "a", true);
    assert_eq!(
        acts(&mut m),
        vec![Act::Send("1a. Committing the loader work: yes".into())]
    );
}

/// A decision's card is titled by the decision, then says what's at
/// stake; the title isn't said again under it.
#[test]
fn decision_card_shows_context() {
    let mut c = Conn {
        q_decide: true,
        ..Conn::default()
    };
    let qs = vec![Q {
        question: "Sessions that woke show no history.\n\n1. What to make easier to find".into(),
        header: "What to make easier to find".into(),
        multi: false,
        options: vec![
            Choice {
                label: "history".into(),
                description: "look where the session started".into(),
                ..Choice::default()
            },
            choice("names"),
        ],
    }];
    let t = text(&draw_question(&mut c, "", &qs, 100, 0, 0));
    let title = t.find("? What to make easier to find").unwrap();
    let at = t.find("Sessions that woke show no history.").unwrap();
    assert!(title < at, "{t}");
    assert!(!t.contains("1. What to make easier to find"), "{t}");
    assert!(t.contains("look where the session started"), "{t}");
}

#[test]
fn decision_answer_numbers() {
    let q = decisions_of(DECISIONS_MSG, "d").unwrap();
    assert_eq!(q.asks.len(), 2);
    assert_eq!(q.asks[0].id, "1");
    assert!(q.asks[0].options[0].picked && q.asks[1].options[1].picked);
    let a: HashMap<String, String> = [
        ("1. Scope".to_string(), "my own way".to_string()),
        ("2. Answer format".to_string(), "keep `Q:/A:`".to_string()),
    ]
    .into_iter()
    .collect();
    let got = decision_answer(&q, &a);
    assert!(got.starts_with("1: my own way\n\n2"), "{got}");
    assert!(decisions_of("no decisions here", "d").is_none());
}
