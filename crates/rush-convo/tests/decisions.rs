//! A message the decisions card sent, drawn as what you chose: after
//! internal/convo/decisions_test.go.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::plain;
use rush_convo::{Options, Renderer, Session};

mod common;

use common::*;

const LOADER_ANSWER: &str = "The lane is done.\n\nDecisions:\n\
    1. **Committing the loader work:** rush's permission check stopped the lane's commit, so this needs your go-ahead. (a) yes: I commit it on perf/loader as one commit per fix, nothing pushed; (b) no: leave it uncommitted for you to look at first.\n\
    My picks: 1a.";

fn decisions_session(text: &str) -> Session {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(say(text), at(1));
    s.feed(done(0.0), at(2));
    s
}

/// A turn of yours starting with text, and ending.
fn answer(s: &mut Session, text: &str) {
    let n = s.turns.len() as i64;
    s.feed(sent(text), at(10 * n));
    s.feed(say("On it."), at(10 * n + 1));
    s.feed(done(0.0), at(10 * n + 2));
}

fn draw(s: &Session) -> String {
    plain(&Renderer::new().render(
        s,
        &Options {
            width: 100,
            ..Options::default()
        },
    ))
}

// What the card sends is drawn as what you chose, a decision a row: its
// topic, then a tick and the option, or a pencil and your own words, the
// topic of those found in the block it answers
// (TestDecisionAnswerDrawnAsChoices).
#[test]
fn decision_answer_drawn_as_choices() {
    let mut s = decisions_session(LOADER_ANSWER);
    answer(&mut s, "1a. Committing the loader work: yes");
    let got = draw(&s);
    for want in ["  · your decisions", "1. Committing the loader work  ✓ yes"] {
        assert!(got.contains(want), "missing {want:?} in\n{got}");
    }
    let mut s = decisions_session(LOADER_ANSWER);
    answer(&mut s, "1: only the first fix, please");
    let got = draw(&s);
    assert!(
        got.contains("1. Committing the loader work  ✎ only the first fix, please"),
        "your own words, under its topic:\n{got}"
    );
    answer(&mut s, "1a 2b please");
    let got = draw(&s);
    assert!(
        got.contains("1a 2b please") && got.matches("your decisions").count() == 1,
        "shorthand stays as written:\n{got}"
    );
}

/// A block is open until a message of yours comes after it, picking from
/// it or not, as rush's host takes its card away for either: the column
/// beside a wide conversation lists it, the catch-up card counts it and
/// the block says "not answered" only until then. A turn another agent
/// began leaves it open; a newer block is open in its turn.
#[test]
fn open_decisions_until_answered() {
    let mut s = decisions_session(LOADER_ANSWER);
    let open = rush_convo::render::open_decisions(&s);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].1[0].topic, "Committing the loader work");
    assert!(draw(&s).contains("not answered"));
    answer(&mut s, "a task finished");
    s.turns.last_mut().unwrap().from = "a background task".into();
    assert_eq!(rush_convo::render::open_decisions(&s).len(), 1);
    answer(&mut s, "something else first");
    assert!(rush_convo::render::open_decisions(&s).is_empty());
    let got = draw(&s);
    assert!(!got.contains("not answered"), "{got}");
    s.feed(sent("and now?"), at(35));
    s.feed(say(LOADER_ANSWER), at(36));
    s.feed(done(0.0), at(37));
    let open = rush_convo::render::open_decisions(&s);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].0, s.turns.last().unwrap().n);
    answer(&mut s, "1a. Committing the loader work: yes");
    assert!(rush_convo::render::open_decisions(&s).is_empty());
}

/// What a decisions block reads as: per case, each decision's ref, topic
/// and option labels, then the picks (ref and letter) and the text left
/// after it.
type Want = (&'static str, &'static str, &'static [&'static str]);

/// A decision as read: its ref, topic and option labels.
type Read = (String, String, Vec<String>);

struct Case {
    name: &'static str,
    text: &'static str,
    want: &'static [Want],
    picks: &'static [&'static str],
    after: &'static str,
}

fn read(text: &str) -> (Vec<Read>, Vec<String>, String) {
    let (_, ds, after) = rush_convo::decisions::decisions_in(text);
    let picks = ds
        .iter()
        .flat_map(|d| {
            d.options
                .iter()
                .filter(|o| o.recommended)
                .map(|o| format!("{}{}", d.r#ref, o.key))
        })
        .collect();
    let got = ds
        .into_iter()
        .map(|d| {
            (
                d.r#ref,
                d.topic,
                d.options.into_iter().map(|o| o.label).collect(),
            )
        })
        .collect();
    (got, picks, after.into_owned())
}

#[test]
fn decisions_blocks_read() {
    let cases = [
        Case {
            name: "letters in a word, a roman numeral and code before (a)",
            text: "Decisions:\n1. **Fix the test(s):** step (i) fails on `f(x)`. (a) skip it; (b) fix it.\nMy picks: 1b.",
            want: &[("1", "Fix the test(s)", &["skip it", "fix it"])],
            picks: &["1b"],
            after: "",
        },
        Case {
            name: "an option mentioning another letter keeps it as text",
            text: "Decisions:\n1. **Store:** (a) memory, like (c) said; (b) disk.",
            want: &[("1", "Store", &["memory, like (c) said", "disk"])],
            picks: &[],
            after: "",
        },
        Case {
            name: "unindented option lines, bullets",
            text: "Decisions:\n1. **Store:** where sessions live.\n(a) memory\n- (b) disk\n\nMy picks: 1b.",
            want: &[("1", "Store", &["memory", "disk"])],
            picks: &["1b"],
            after: "",
        },
        Case {
            name: "upper case and bare letters",
            text: "Decisions:\n1. **Store:** (A) memory; (B) disk.\n2. **Look:** a) tabs; b) a list.\nI'd go with 1B and 2a.",
            want: &[
                ("1", "Store", &["memory", "disk"]),
                ("2", "Look", &["tabs", "a list"]),
            ],
            picks: &["1b", "2a"],
            after: "",
        },
        Case {
            name: "a numbered list after the block leaves it the block",
            text: "Decisions:\n1. **Store:** (a) memory; (b) disk.\nRecommend: 1a.\n\nNext steps:\n1. land it\n2. pick 2b of the fixes",
            want: &[("1", "Store", &["memory", "disk"])],
            picks: &["1a"],
            after: "Next steps:\n1. land it\n2. pick 2b of the fixes",
        },
        Case {
            name: "the picks line wherever it is after",
            text: "Decisions:\n1. **Store:** (a) memory; (b) disk.\n\nBoth work.\nMy pick: 1b.\n\nThanks.",
            want: &[("1", "Store", &["memory", "disk"])],
            picks: &["1b"],
            after: "Both work.\nThanks.",
        },
        Case {
            name: "per-option lines say more of the inline options",
            text: "Decisions:\n1. **Look:** (a) tabs; (b) a list.\n   (a) tabs: each file a tab, costs width.\n   (b) a list: one a row.",
            want: &[("1", "Look", &["tabs", "a list"])],
            picks: &[],
            after: "",
        },
        Case {
            name: "a nested list stays in its item; lazy numbering",
            text: "## Decisions\n1. **Copy:** what moves:\n   1. copy\n   2. paste\n   (a) both; (b) copy only.\n1. **Ship:** (a) yes; (b) no.",
            want: &[
                ("1", "Copy", &["both", "copy only"]),
                ("2", "Ship", &["yes", "no"]),
            ],
            picks: &[],
            after: "",
        },
        Case {
            name: "no topic, a bold heading",
            text: "**Decisions**\n1. (a) yes; (b) no.",
            want: &[("1", "Decision 1", &["yes", "no"])],
            picks: &[],
            after: "",
        },
        Case {
            name: "a fence of four ticks holds three",
            text: "Decisions:\n1. **Look:** (a) tabs; (b) list.\n   (a) tabs:\n   ````\n   ```\n   1. (a) x; (b) y\n   ```\n   ````\n2. **Ship:** (a) yes; (b) no.",
            want: &[
                ("1", "Look", &["tabs", "list"]),
                ("2", "Ship", &["yes", "no"]),
            ],
            picks: &[],
            after: "",
        },
    ];
    for c in cases {
        let (got, picks, after) = read(c.text);
        let want: Vec<Read> = c
            .want
            .iter()
            .map(|(r, t, o)| {
                (
                    r.to_string(),
                    t.to_string(),
                    o.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect();
        assert_eq!(got, want, "{}", c.name);
        assert_eq!(picks, c.picks, "{}: picks", c.name);
        assert_eq!(after, c.after, "{}: after", c.name);
    }
}

/// A preview goes to the option line just above it, and per-option lines
/// add their words to the option's detail.
#[test]
fn previews_named_by_the_line_above() {
    let text = "Decisions:\n1. **Look:** how it shows.\n   (a) tabs\n   (b) list\n   ```\n   - one\n   ```\n";
    let (_, ds, _) = rush_convo::decisions::decisions_in(text);
    assert_eq!(ds[0].options[0].preview, "");
    assert_eq!(ds[0].options[1].preview, "- one");
    let text = "Decisions:\n1. **Look:** (a) tabs; (b) a list.\n   (a) tabs: each file a tab, costs width.\n";
    let (_, ds, _) = rush_convo::decisions::decisions_in(text);
    assert_eq!(ds[0].options[0].detail, "each file a tab, costs width");
    assert_eq!(ds[0].options[1].detail, "");
}

/// An answer after words of your own still answers the block: its picks
/// read as chosen, not only suggested.
#[test]
fn an_answer_after_typed_text_counts() {
    let mut s = decisions_session(LOADER_ANSWER);
    assert!(draw(&s).contains("its suggestion"));
    answer(
        &mut s,
        "thanks, that's clear\n1a. Committing the loader work: yes",
    );
    let got = draw(&s);
    assert!(
        got.contains("its pick") && !got.contains("its suggestion"),
        "{got}"
    );
}

/// A block whose card was set aside says so, and isn't open for the
/// catch-up or the chat any more.
#[test]
fn a_block_set_aside() {
    let s = decisions_session(LOADER_ANSWER);
    let r = rush_convo::render::decisions_ref(&format!("t{}", s.turns[0].n));
    let mut o = Options {
        width: 100,
        ..Options::default()
    };
    o.open.insert(rush_convo::render::decisions_gone(&r), true);
    let got = plain(&Renderer::new().render(&s, &o));
    assert!(got.contains("set aside · d opens the card"), "{got}");
    assert!(!got.contains("not answered"), "{got}");
}
