//! The decisions card wraps its words to the card's width, on word
//! boundaries, and never cuts one with "…" where it fits.

use rush_core::event::{Ask, Choice, Question};
use rush_core::text::width;

const WIDTHS: [usize; 4] = [60, 80, 120, 200];

use super::question::*;
use super::*;

const LONG: &str = "Migrate the entitlement module, which handles usage, to the new event pipeline and replay every deprecated service 🚀 日本語のとても長い説明";

fn fixture() -> Question {
    let opt = |l: &str, d: &str, p: &str| Choice {
        label: l.into(),
        description: d.into(),
        preview: p.into(),
        ..Choice::default()
    };
    Question {
        id: "w1".into(),
        asks: vec![
            Ask {
                text: format!(
                    "{LONG}. Which upgrade path do you want to take for the whole module?"
                ),
                header: "Upgrade path for the entitlement module and its services".into(),
                options: vec![
                    opt(
                        &format!(
                            "Upgrade in place, then replay every event ({LONG}) (Recommended)"
                        ),
                        &format!("{LONG} {LONG}"),
                        &format!("{LONG}\n    indented {LONG}"),
                    ),
                    opt("Short", "A short one", ""),
                ],
                ..Ask::default()
            },
            Ask {
                text: "Ship it?".into(),
                header: "A second header that is also rather long for a chip".into(),
                options: vec![opt("yes", "", ""), opt("no", "", "")],
                ..Ask::default()
            },
        ],
        ..Question::default()
    }
}

fn draw(req: &Question, w: usize, focus: bool, answer: bool) -> Vec<String> {
    let mut c = Conn {
        card_focus: focus,
        ..Conn::default()
    };
    draw_on(&mut c, req, w, answer)
}

fn draw_on(c: &mut Conn, req: &Question, w: usize, answer: bool) -> Vec<String> {
    if answer {
        let (_, qs) = questions(req);
        c.sync_question(req, None);
        c.q_answer
            .insert(qs[0].question.clone(), qs[0].options[0].label.clone());
        c.q_answer.insert(qs[1].question.clone(), "yes".into());
        c.q_idx = 2;
    }
    let ls = question_card(c, req, None, w, 0);
    ls.iter().map(|l| l.plain()).collect()
}

/// Cells before the first `needle` in a row.
fn col(row: &str, needle: &str) -> usize {
    width(
        &row[..row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"))],
    )
}

/// Cells before a row's first word, past the card's edge and bars.
fn text_col(row: &str) -> usize {
    let s = row.trim_start_matches(['▍', '▌', ' ']);
    width(row) - width(s)
}

#[test]
fn long_words_wrap_and_never_end_in_an_ellipsis() {
    let req = fixture();
    let words: Vec<&str> = LONG
        .split_whitespace()
        .filter(|w| w.is_ascii())
        .chain(["Upgrade", "entitlement", "second", "header", "chip"])
        .collect();
    for w in WIDTHS {
        for (focus, answered) in [(true, false), (false, false), (true, true)] {
            let rows = draw(&req, w, focus, answered);
            let all = rows.join("\n");
            assert!(!all.contains('…'), "w={w}: a cut:\n{all}");
            for r in &rows {
                assert!(width(r) <= w, "w={w}: {r:?} too wide");
            }
            for word in &words {
                assert!(all.contains(word), "w={w}: {word:?} lost:\n{all}");
            }
        }
    }
}

#[test]
fn an_options_wrapped_lines_line_up_under_its_text() {
    let req = fixture();
    for w in [60, 80, 120] {
        let rows = draw(&req, w, true, false);
        let at = rows
            .iter()
            .position(|r| r.contains(" 1  Upgrade in place"))
            .expect("option 1");
        let x = col(&rows[at], "Upgrade");
        // The list's own column: what's left of the preview beside it.
        let list = |r: &String| -> String {
            r.chars().take_while(|c| *c != '╭' && *c != '│').collect()
        };
        let end = rows[at..]
            .iter()
            .position(|r| list(r).trim_matches(['▍', ' ']).is_empty())
            .map_or(rows.len(), |n| at + n);
        assert!(end - at > 3, "w={w}: it wraps");
        for r in &rows[at + 1..end] {
            assert_eq!(text_col(&list(r)), x, "w={w}: {r:?} not under the text");
        }
    }
}

#[test]
fn the_review_wraps_names_and_answers_in_their_columns() {
    let req = fixture();
    for w in [60, 80, 120, 200] {
        let rows = draw(&req, w, true, true);
        let at = rows
            .iter()
            .position(|r| r.contains("Send these answers?"))
            .expect("review");
        let first = rows[at + 2..]
            .iter()
            .position(|r| r.contains("✓ Upgrade in place"))
            .map(|n| at + 2 + n)
            .expect("answer");
        let ans_x = col(&rows[first], "✓ Upgrade");
        let name_x = col(&rows[first], "Upgrade path");
        let end = rows[first..]
            .iter()
            .position(|r| r.contains("A second header"))
            .map(|n| first + n)
            .expect("second");
        for r in &rows[first + 1..end] {
            let name = r.get(..ans_x.min(r.len())).unwrap_or(r);
            if !name.trim_matches(['▍', ' ']).is_empty() {
                assert_eq!(text_col(name), name_x, "w={w}: {r:?}");
            }
            let rest: String = r.chars().skip_while(|_| false).collect();
            if width(&rest) > ans_x + 2 {
                let tail = &rest[rest
                    .char_indices()
                    .find(|(i, _)| width(&rest[..*i]) >= ans_x)
                    .map_or(rest.len(), |(i, _)| i)..];
                if !tail.trim().is_empty() {
                    assert!(
                        tail.starts_with("  "),
                        "w={w}: {r:?} answer not under its words"
                    );
                }
            }
        }
    }
}

#[test]
fn the_full_view_and_the_preview_wrap_too() {
    let req = fixture();
    for w in WIDTHS {
        let mut c = Conn {
            card_focus: true,
            q_full: true,
            ..Conn::default()
        };
        let rows = draw_on(&mut c, &req, w, false);
        let all = rows.join("\n");
        assert!(!all.contains('…'), "w={w}:\n{all}");
        // The box keeps one width: every framed row is as wide as its top.
        let top = rows.iter().find(|r| r.contains('╭')).expect("box");
        let (l, r) = (col(top, "╭"), col(top, "╮"));
        for row in rows.iter().filter(|r| r.contains('│')) {
            assert_eq!(col(row, "│"), l, "w={w}: {row:?}");
            assert_eq!(width(row.trim_end()) - 1, r, "w={w}: {row:?}");
        }
    }
}

#[test]
fn a_decisions_card_wraps_its_header_and_hint() {
    let mut req = fixture();
    req.asks.truncate(1);
    for w in WIDTHS {
        for focus in [true, false] {
            let mut c = Conn {
                card_focus: focus,
                q_decide: true,
                ..Conn::default()
            };
            let rows = draw_on(&mut c, &req, w, false);
            let all = rows.join("\n");
            assert!(!all.contains('…'), "w={w}:\n{all}");
            assert!(rows.iter().all(|r| width(r) <= w), "w={w}:\n{all}");
            assert!(
                all.contains("entitlement") && all.contains("services"),
                "w={w}:\n{all}"
            );
        }
    }
}
