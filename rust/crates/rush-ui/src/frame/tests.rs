//! The frame's tests, ported from Go's internal/ui (places_test.go,
//! keyname_test.go, macoption_test.go, editor_test.go, inputbox_test.go,
//! overlayclick_test.go, sheets_test.go, choicesheet_test.go). No sleeps:
//! a model with no loop runs handed-off work at once.

use std::cell::RefCell;
use std::rc::Rc;

use rush_core::text::Line;
use rush_draw::{Button, Key, KeyMods, MouseKind};

use super::editor::*;
use super::inputbox::{InputBox, wrap_segs};
use super::keyname::{key_of, mac_option_on};
use super::sheet::{Sheet, sheet_row, sheet_tabs, sheet_title};
use super::*;
use crate::model::Model;
use crate::style::*;

fn model(w: usize, h: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(1_793_190_896_789);
    m.w = w;
    m.h = h;
    m
}

fn press(m: &mut Model, s: &str) -> Out {
    let p = Press::of(s).unwrap_or_else(|| panic!("no key {s:?}"));
    m.press_key(&p)
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

// ---- keyname_test.go, macoption_test.go ----

#[test]
fn key_of_round_trips() {
    for s in [
        "ctrl+enter",
        "alt+shift+up",
        "f5",
        "super+z",
        "x",
        "ctrl+c",
        "shift+tab",
        "pgdown",
        "space",
        "?",
        "<",
        "ctrl+\\",
        "alt+m",
        "esc",
        "super+backspace",
    ] {
        let (c, m) = key_of(s).unwrap_or_else(|| panic!("keyOf({s:?})"));
        assert_eq!(rush_draw::key_string(c, m), s);
        assert_eq!(Press::of(s).unwrap().s, s);
    }
    assert_eq!(key_of("ctrl+"), None);
    assert_eq!(key_of("nonsense"), None);
}

#[test]
fn mac_option_reads_alt() {
    let (c, m) = mac_option_on(true, Key::Char('ß'), KeyMods::NONE);
    assert_eq!(rush_draw::key_string(c, m), "alt+s");
    let (c, m) = mac_option_on(true, Key::Char('s'), KeyMods::NONE);
    assert_eq!(rush_draw::key_string(c, m), "s");
    let (c, m) = mac_option_on(false, Key::Char('ß'), KeyMods::NONE);
    assert_eq!(rush_draw::key_string(c, m), "ß");
}

#[test]
fn cmd_is_super_however_sent() {
    let p = Press::new(Key::Left, KeyMods::META);
    assert_eq!(p.s, "super+left");
    assert_eq!(Press::new(Key::Char('a'), KeyMods::NONE).text(), Some('a'));
    assert_eq!(Press::new(Key::Char('a'), KeyMods::CTRL).text(), None);
}

// ---- places_test.go: TestPlacesAndFocus ----

/// , . < > ctrl+\ go between places, round the bar; esc comes back to
/// Agents; with something typed, or a conversation open, < > are text.
#[test]
fn places_and_focus() {
    let mut m = model(200, 50);
    press(&mut m, ">");
    assert_eq!(m.place, PlaceId::Projects, "> from Agents goes to Projects");
    press(&mut m, "esc");
    assert_eq!(
        m.place,
        PlaceId::Agents,
        "esc from Projects comes back to Agents"
    );
    press(&mut m, ">");
    press(&mut m, ">");
    assert_eq!(m.place, PlaceId::Harnesses);
    press(&mut m, ">");
    assert_eq!(m.place, PlaceId::Settings);
    for _ in 0..3 {
        press(&mut m, "<");
    }
    assert_eq!(m.place, PlaceId::Agents, "< < < from Settings");
    press(&mut m, "<");
    assert_eq!(m.place, PlaceId::Settings, "< from Agents goes round");
    press(&mut m, ">");
    press(&mut m, ">");
    assert_eq!(
        m.place,
        PlaceId::Projects,
        "> > from Settings, round through Agents"
    );
    press(&mut m, "esc");
    press(&mut m, ".");
    assert_eq!(m.place, PlaceId::Projects, ". goes on as > does");
    press(&mut m, ",");
    assert_eq!(m.place, PlaceId::Agents, ", comes back as < does");
    press(&mut m, "ctrl+\\");
    assert_eq!(m.place, PlaceId::Projects, "ctrl+\\ goes on");
    press(&mut m, "esc");

    // Typed in the Agents box, > is text.
    m.ext.agents_box = true;
    press(&mut m, "a");
    press(&mut m, ">");
    assert_eq!(m.place, PlaceId::Agents);
    assert_eq!(m.input.string(), "a>");
    // ctrl+c clears it; then , moves again.
    press(&mut m, "ctrl+c");
    assert!(m.input.is_empty());
    press(&mut m, ",");
    assert_eq!(m.place, PlaceId::Settings);
}

/// [ ] and tab turn Agents' pages; esc from a page is the plain list.
#[test]
fn agents_pages() {
    let mut m = model(120, 40);
    let f = screen(&mut m);
    assert!(f[4].contains("Agents · Wall  [ ]"), "{f:?}");
    press(&mut m, "]");
    assert_eq!(m.agents_page, 1);
    let f = screen(&mut m);
    assert!(
        f.iter().any(|l| l.contains("nothing open right now")),
        "{f:?}"
    );
    press(&mut m, "tab");
    assert_eq!(m.agents_page, 0, "tab past the last page comes round");
    press(&mut m, "[");
    assert_eq!(m.agents_page, 1);
    press(&mut m, "esc");
    assert_eq!(m.agents_page, 0);
    // A place's pages come and go with it.
    press(&mut m, "]");
    press(&mut m, ">");
    press(&mut m, "<");
    assert_eq!(m.agents_page, 0, "coming back is the plain list");
}

/// A place's frame: the header with its tab lit, the body, a rule, its
/// keys; a flash in their place for a while.
#[test]
fn a_place_draws_in_the_frame() {
    let mut m = model(100, 24);
    press(&mut m, ">");
    let f = screen(&mut m);
    assert_eq!(f.len(), 24);
    assert!(f[2].contains(" Projects "), "{f:?}");
    assert_eq!(
        f[4].trim_end(),
        "   ▀▀▀▀▀▀",
        "no pages: nothing beside the logo {f:?}"
    );
    assert!(f[5].starts_with("────"));
    assert!(f[22].starts_with("────"), "{f:?}");
    // The place's own keys (Projects' once its group fills it), esc last.
    assert!(f[23].trim_end().ends_with("esc back"), "{f:?}");
    m.fail("something went wrong");
    let f = screen(&mut m);
    assert_eq!(f[23].trim_end(), "  something went wrong");
    assert_eq!(m.frame()[23].spans[1].style.fg, RED, "a failure is red");
    m.flash_err("select an agent first");
    assert_eq!(m.frame()[23].spans[1].style.fg, TEXT, "a refusal isn't");
}

/// The header's tabs, clicked, go to the place; a page's, to the page.
#[test]
fn clicking_the_tabs() {
    let mut m = model(120, 40);
    let f = screen(&mut m);
    let x = f[2].find(" Settings ").unwrap();
    let x = rush_core::text::width(&f[2][..x]) + 2;
    let click = |m: &mut Model, x: usize, y: usize| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Down(Button::Left),
            x: x as u16,
            y: y as u16,
            mods: KeyMods::NONE,
        })
    };
    assert_eq!(click(&mut m, x, 2), Out::Draw);
    assert_eq!(m.place, PlaceId::Settings);
    let f = screen(&mut m);
    let x = rush_core::text::width(&f[2][..f[2].find(" Agents ").unwrap()]) + 2;
    click(&mut m, x, 2);
    assert_eq!(m.place, PlaceId::Agents);
    let f = screen(&mut m);
    let x = rush_core::text::width(&f[4][..f[4].find("Wall").unwrap()]) + 1;
    click(&mut m, x, 4);
    assert_eq!(m.agents_page, 1);
}

// ---- editor_test.go ----

fn edit_keys(buf: &str, pos: usize, keys: &[&str]) -> (String, usize) {
    let mut b: Vec<char> = buf.chars().collect();
    let mut pos = pos;
    for k in keys {
        let p = match k.strip_prefix('"').and_then(|k| k.strip_suffix('"')) {
            Some(t) => Press::new(Key::Char(t.chars().next().unwrap()), KeyMods::NONE),
            None => Press::of(k).unwrap(),
        };
        if let Some((nb, np)) = edit(&b, pos, &p) {
            b = nb;
            pos = np;
        }
    }
    (b.into_iter().collect(), pos)
}

#[test]
fn edits_like_go() {
    type Case<'a> = (&'a str, &'a str, usize, &'a [&'a str], &'a str, usize);
    let cases: &[Case] = &[
        ("type in the middle", "helo", 3, &["\"l\""], "hello", 4),
        (
            "word left",
            "run the tests",
            13,
            &["ctrl+left"],
            "run the tests",
            8,
        ),
        (
            "word left twice",
            "run the tests",
            13,
            &["alt+left", "alt+left"],
            "run the tests",
            4,
        ),
        (
            "word right",
            "run the tests",
            0,
            &["ctrl+right"],
            "run the tests",
            3,
        ),
        (
            "delete word back",
            "run the tests",
            13,
            &["alt+backspace"],
            "run the ",
            8,
        ),
        (
            "delete word back over spaces",
            "run the   ",
            10,
            &["ctrl+w"],
            "run ",
            4,
        ),
        (
            "delete word forward",
            "run the tests",
            4,
            &["alt+delete"],
            "run  tests",
            4,
        ),
        (
            "cmd+backspace clears to line start",
            "one\ntwo three",
            13,
            &["super+backspace"],
            "one\n",
            4,
        ),
        (
            "ctrl+u at a line start joins upward",
            "one\ntwo",
            4,
            &["ctrl+u"],
            "two",
            0,
        ),
        (
            "ctrl+k clears to the end",
            "run the tests",
            4,
            &["ctrl+k"],
            "run ",
            4,
        ),
        (
            "home and end",
            "abc",
            1,
            &["home", "\"x\"", "end", "\"y\""],
            "xabcy",
            5,
        ),
        ("new line", "ab", 1, &["shift+enter"], "a\nb", 2),
        (
            "backspace at start is a no-op",
            "ab",
            0,
            &["backspace"],
            "ab",
            0,
        ),
    ];
    for (name, buf, pos, keys, want, want_pos) in cases {
        let (got, p) = edit_keys(buf, *pos, keys);
        assert_eq!((got.as_str(), p), (*want, *want_pos), "{name}");
    }
}

#[test]
fn selection() {
    let run = |buf: &str, pos: usize, anchor: Option<usize>, keys: &[&str]| {
        let mut e = Edited {
            buf: buf.chars().collect(),
            pos,
            anchor,
            copied: String::new(),
        };
        let mut copied = String::new();
        for k in keys {
            let p = match k.strip_prefix('"').and_then(|k| k.strip_suffix('"')) {
                Some(t) => Press::new(Key::Char(t.chars().next().unwrap()), KeyMods::NONE),
                None => Press::of(k).unwrap(),
            };
            if let Some(n) = edit_sel(&e.buf, e.pos, e.anchor, &p) {
                if !n.copied.is_empty() {
                    copied = n.copied.clone();
                }
                e = n;
            }
        }
        (e.buf.iter().collect::<String>(), e.pos, e.anchor, copied)
    };
    let back = ["shift+left"; 5];
    let (b, p, a, _) = run("hello world", 11, None, &back);
    assert_eq!((p, a), (6, Some(11)), "select back");
    let (b, _, a, _) = run(&b, p, a, &["\"X\""]);
    assert_eq!(
        (b.as_str(), a),
        ("hello X", None),
        "typing replaces the selection"
    );
    let (b, _, _, c) = run("run the tests", 0, None, &["ctrl+shift+right", "ctrl+c"]);
    assert_eq!((c.as_str(), b.as_str()), ("run", "run the tests"), "copy");
    let (b, _, _, _) = run("run the tests", 13, None, &["shift+home", "backspace"]);
    assert_eq!(b, "", "delete the selection");
    let (_, p, a, _) = run("abc", 1, None, &["shift+right", "left"]);
    assert_eq!((p, a), (1, None), "moving clears the selection");
    let (_, _, _, c) = run("run the tests", 4, None, &["super+a", "ctrl+c"]);
    assert_eq!(c, "run the tests", "select all");
    let (_, p, a, c) = run("run the tests", 4, None, &["alt+c"]);
    assert_eq!((c.as_str(), p, a), ("run the tests", 4, None), "copy all");
}

#[test]
fn chips_go_whole() {
    let chip = "[pasted text #1 · 5 lines: a b c d e]";
    let draft: Vec<char> = format!("look at {chip} please").chars().collect();
    let end = format!("look at {chip}").chars().count();
    let e = edit_chips(&draft, end, None, &Press::of("backspace").unwrap()).unwrap();
    assert_eq!(e.buf.iter().collect::<String>(), "look at  please");
    assert_eq!(e.pos, "look at ".len());
    // → onto it selects it whole; → again goes past.
    let at = "look at ".len();
    let e = edit_chips(&draft, at, None, &Press::of("right").unwrap()).unwrap();
    assert_eq!((e.pos, e.anchor), (end, Some(at)));
    let e = edit_chips(&draft, e.pos, e.anchor, &Press::of("right").unwrap()).unwrap();
    assert_eq!((e.pos, e.anchor), (end, None));
}

#[test]
fn undo_and_redo() {
    let mut e = Editor::default();
    for c in "hello".chars() {
        e.key(&Press::new(Key::Char(c), KeyMods::NONE));
    }
    e.key(&Press::of("ctrl+w").unwrap());
    assert_eq!(e.string(), "");
    e.key(&Press::of("super+z").unwrap());
    assert_eq!(e.string(), "hello", "the word comes back");
    e.key(&Press::of("super+z").unwrap());
    assert_eq!(e.string(), "", "typing undoes as one step");
    e.key(&Press::of("ctrl+y").unwrap());
    assert_eq!(e.string(), "hello");
}

#[test]
fn clean_paste_like_go() {
    let input = "goroutine 1:\r\n\tmain.go:78 +0x5ec\n\x1b[1mab\tc\rd";
    let want = "goroutine 1:\n    main.go:78 +0x5ec\n[1mab   c\nd";
    assert_eq!(clean_paste(input), want);
}

#[test]
fn image_paths_and_drops() {
    let dir = std::env::temp_dir().join(format!("rush-frame-img-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("Screen Shot 1.png").to_string_lossy().into_owned();
    let b = dir.join("b.jpg").to_string_lossy().into_owned();
    for p in [&a, &b] {
        std::fs::write(p, "x").unwrap();
    }
    let look: Look = &look_disk;
    let escaped = a.replace(' ', "\\ ");
    let missing = dir.join("missing.png").to_string_lossy().into_owned();
    let cases = [
        (escaped.clone(), 1),
        (format!("'{a}'"), 1),
        (format!("{escaped} {b}"), 2),
        (format!("{escaped}\n{b}\n"), 2),
        (format!("look at {b}"), 0),
        (missing, 0),
    ];
    for (paste, want) in cases {
        let got = image_paths(&paste, look).map_or(0, |v| v.len());
        assert_eq!(got, want, "{paste:?}");
    }
    let url = format!("file://{}", a.replace(' ', "%20"));
    assert_eq!(image_paths(&url, look), Some(vec![a.clone()]));
    let (rest, imgs) = extract_images(
        &format!("{escaped} still here\nand /var/nope.png stays"),
        look,
    );
    assert_eq!(imgs, vec![a.clone()]);
    assert_eq!(rest, "still here\nand /var/nope.png stays");
    assert!(is_drop(&escaped, look));
    assert!(!is_drop("just words", look));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- inputbox_test.go ----

#[test]
fn box_click_maps_to_text() {
    let b = InputBox {
        w: 24,
        text: "one two three four five six".chars().collect(),
        lead: line([plain("❯ ")]),
        max_rows: 6,
        ..InputBox::default()
    };
    let segs = wrap_segs(&b.text, b.w - 4 - 2);
    assert_eq!(segs.len(), 2);
    assert_eq!(
        b.text[segs[0].from..segs[0].to].iter().collect::<String>(),
        "one two three "
    );
    assert_eq!(b.at(0, 4), 0, "first char");
    assert_eq!(b.at(0, 8), 4, "start of two");
    assert_eq!(b.at(1, 4), segs[1].from);
    assert_eq!(b.at(1, 99), b.text.len());
    // A drag can leave the box.
    assert_eq!(b.near(-3, 50), 0);
    assert_eq!(b.near(9, 0), b.text.len());
    assert_eq!(b.near(0, 200), "one two three ".len());
    assert_eq!(b.near(1, -5), "one two three ".len());
}

/// Wide runes take two cells: a click on either is that rune, and the
/// cells after count from its end, on its own row and wrapped ones
/// (inputbox.go:451 col).
#[test]
fn box_click_counts_wide_runes() {
    let text: Vec<char> = "ab世界c defghijk 漢字 xyz".chars().collect();
    let b = InputBox {
        w: 20,
        cursor: text.len(),
        text,
        lead: line([plain("❯ ")]),
        max_rows: 6,
        ..InputBox::default()
    };
    // "│ ❯ " is 4 cells; a row has 13.
    let rows: Vec<String> = b
        .segs()
        .iter()
        .map(|s| b.text[s.from..s.to].iter().collect())
        .collect();
    assert_eq!(rows, ["ab世界c ", "defghijk ", "漢字 xyz"]);
    // (row, column from the box's left edge, the text position)
    let cases: &[(usize, isize, usize)] = &[
        (0, 4, 0),  // a
        (0, 5, 1),  // b
        (0, 6, 2),  // 世, first cell
        (0, 7, 2),  // 世, second cell
        (0, 8, 3),  // 界
        (0, 9, 3),  // 界's second cell
        (0, 10, 4), // c
        (0, 11, 5), // the space
        (0, 13, 6), // past the row: its end
        (1, 4, 6),  // d on the second row
        (1, 12, 14),
        (1, 30, 15),
        (2, 4, 15), // 漢 on the third
        (2, 5, 15),
        (2, 6, 16), // 字
        (2, 7, 16),
        (2, 8, 17),  // the space after
        (2, 9, 18),  // x
        (2, 30, 21), // past the text: its end
        (3, 4, 21),  // under the box's rows: the cursor
    ];
    for &(row, col, want) in cases {
        assert_eq!(b.at(row, col), want, "row {row}, column {col}");
    }
}

#[test]
fn box_draws_its_edge_and_labels() {
    let b = InputBox {
        w: 40,
        focused: true,
        top_l: line([sp("new agent in rush", TEXT)]),
        holder: "describe a task".into(),
        lead: line([sp("❯ ", ORANGE)]),
        max_rows: 4,
        ..InputBox::default()
    };
    let rows: Vec<String> = b.lines().iter().map(Line::plain).collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        format!("╭─ new agent in rush {}─╮", "─".repeat(17))
    );
    assert_eq!(rush_core::text::width(&rows[0]), 40);
    assert_eq!(rows[1].trim_end_matches([' ', '│']), "│ ❯  describe a task");
    assert_eq!(rush_core::text::width(&rows[1]), 40);
    assert_eq!(rows[2], format!("╰{}╯", "─".repeat(38)));
    // The edge is orange with the keys, quiet without.
    assert_eq!(b.lines()[0].spans[0].style.fg, ORANGE);
    let quiet = InputBox {
        focused: false,
        ..b.clone()
    };
    assert_eq!(quiet.lines()[0].spans[0].style.fg, super::inputbox::EDGE);
    // Rows past max_rows scroll to keep the cursor in view.
    let long = InputBox {
        w: 24,
        text: "a ".repeat(60).chars().collect(),
        cursor: 0,
        max_rows: 3,
        ..b
    };
    assert_eq!(long.rows(), 3);
    assert_eq!(long.lines().len(), 5);
}

/// The wrap measures each row once and breaks as the first version did
/// (TestBoxDrawsTheSame's oracle, on a few texts).
#[test]
fn wraps_at_spaces_and_breaks_long_words() {
    let segs = |s: &str, w| -> Vec<String> {
        let t: Vec<char> = s.chars().collect();
        wrap_segs(&t, w)
            .iter()
            .map(|g| t[g.from..g.to].iter().collect())
            .collect()
    };
    assert_eq!(segs("one two three", 8), ["one two ", "three"]);
    assert_eq!(
        segs("longwordwithoutspaces", 6),
        ["longwo", "rdwith", "outspa", "ces"]
    );
    assert_eq!(segs("a\nb", 10), ["a", "b"]);
    assert_eq!(segs("中文中文中文", 4), ["中文", "中文", "中文"]);
    assert_eq!(segs("", 4), [""]);
}

// ---- overlayclick_test.go: TestSheetClickRouting ----

#[derive(Default)]
struct Clicks {
    tab: usize,
    cur: usize,
    picked: String,
    keyed: Vec<String>,
}

struct ClickSheet(Rc<RefCell<Clicks>>);

const ROWS: [&str; 4] = ["alpha", "bravo", "charlie", "delta"];

impl Sheet for ClickSheet {
    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let s = self.0.borrow();
        let mut out = vec![
            sheet_title("Click", "a sheet to click", w),
            Line::new(),
            sheet_tabs(&["One", "Two", "Three"], s.tab),
            Line::new(),
        ];
        for (i, r) in ROWS.iter().enumerate() {
            out.push(sheet_row(line([plain(*r)]), i == s.cur, w));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[
                ("↑↓", "choose"),
                ("enter", "pick"),
                ("x", "remove"),
                ("esc", "close"),
            ],
        ));
        out
    }

    fn key(&mut self, _m: &mut Model, k: &Press) -> Out {
        let mut s = self.0.borrow_mut();
        s.keyed.push(k.s.clone());
        match k.s.as_str() {
            "up" => s.cur = s.cur.saturating_sub(1),
            "down" => s.cur = (s.cur + 1).min(ROWS.len() - 1),
            "]" => s.tab = (s.tab + 1) % 3,
            "enter" => {
                s.picked = ROWS[s.cur].into();
                return Out::Close;
            }
            "x" => s.picked = format!("x on {}", ROWS[s.cur]),
            "esc" => return Out::Close,
            _ => {}
        }
        Out::Draw
    }
}

fn click_at(m: &mut Model, text: &str) -> Out {
    let f = screen(m);
    let (x, y) = f
        .iter()
        .enumerate()
        .find_map(|(y, l)| l.find(text).map(|x| (rush_core::text::width(&l[..x]), y)))
        .unwrap_or_else(|| panic!("no {text:?} on screen:\n{}", f.join("\n")));
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: x as u16 + 1,
        y: y as u16,
        mods: KeyMods::NONE,
    })
}

#[test]
fn sheet_click_routing() {
    let mut m = model(120, 40);
    let open = |m: &mut Model, c: Clicks| {
        let s = Rc::new(RefCell::new(c));
        m.open_sheet(Box::new(ClickSheet(s.clone())));
        s
    };
    let s = open(&mut m, Clicks::default());
    click_at(&mut m, "charlie");
    assert_eq!(s.borrow().picked, "charlie", "{:?}", s.borrow().keyed);
    assert!(m.sheet.is_none());

    let s = open(
        &mut m,
        Clicks {
            cur: 3,
            ..Clicks::default()
        },
    );
    click_at(&mut m, "bravo");
    assert_eq!(
        s.borrow().picked,
        "bravo",
        "a row above: {:?}",
        s.borrow().keyed
    );

    let s = open(&mut m, Clicks::default());
    click_at(&mut m, "Three");
    assert_eq!(s.borrow().tab, 2, "a tab: {:?}", s.borrow().keyed);
    assert!(m.sheet.is_some());
    click_at(&mut m, "One");
    assert_eq!(s.borrow().tab, 0, "back to the first tab");

    click_at(&mut m, "x remove");
    assert_eq!(
        s.borrow().picked,
        "x on alpha",
        "a key: {:?}",
        s.borrow().keyed
    );
    assert!(m.sheet.is_some());

    // The title is no row: the cursor stays and nothing's picked.
    s.borrow_mut().picked.clear();
    click_at(&mut m, "a sheet to click");
    assert_eq!((s.borrow().cur, s.borrow().picked.as_str()), (0, ""));
    assert!(m.sheet.is_some());

    // Outside the box is esc.
    screen(&mut m);
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: 0,
        y: 0,
        mods: KeyMods::NONE,
    });
    assert!(m.sheet.is_none());
}

/// A sheet has every key until it closes; the screen behind is faded.
#[test]
fn a_sheet_takes_the_keys() {
    let mut m = model(120, 40);
    let s = Rc::new(RefCell::new(Clicks::default()));
    m.open_sheet(Box::new(ClickSheet(s.clone())));
    press(&mut m, ">");
    assert_eq!(m.place, PlaceId::Agents, "< > are the sheet's");
    press(&mut m, "down");
    assert_eq!(s.borrow().cur, 1);
    let f = m.frame();
    // Behind the box, the header is faded to the faint ink.
    assert!(f[0].spans.iter().all(|sp| sp.style.fg == FAINT));
    let rows: Vec<String> = f.iter().map(Line::plain).collect();
    assert!(rows.iter().any(|r| r.contains("▍ bravo")), "{rows:?}");
    press(&mut m, "esc");
    assert!(m.sheet.is_none());
    press(&mut m, ">");
    assert_eq!(m.place, PlaceId::Projects);
}

// ---- the question box (confirmKey) ----

#[test]
fn a_question_waits_for_its_keys() {
    let mut m = model(100, 30);
    let said = Rc::new(RefCell::new(String::new()));
    let ask = |m: &mut Model, said: &Rc<RefCell<String>>| {
        let (a, b, c) = (said.clone(), said.clone(), said.clone());
        let mut q = Confirm::new("Quit with unsaved changes?", move |_| {
            *a.borrow_mut() = "yes".into();
            Out::Draw
        })
        .detail("they're lost");
        q.on_no = Some(Box::new(move |_| {
            *b.borrow_mut() = "no".into();
            Out::Draw
        }));
        q.no_text = "keep editing".into();
        q.again = "right".into();
        q.more.push((
            "s".into(),
            "save first".into(),
            Box::new(move |_| {
                *c.borrow_mut() = "save".into();
                Out::Draw
            }),
        ));
        m.ask(q);
    };
    ask(&mut m, &said);
    let rows = screen(&mut m);
    assert!(
        rows.iter()
            .any(|r| r.contains("Quit with unsaved changes?")),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("y yes   s save first   n keep editing   esc cancel")),
        "{rows:?}"
    );
    press(&mut m, "q");
    assert!(m.confirm.is_some(), "other keys wait");
    press(&mut m, "n");
    assert_eq!(*said.borrow(), "no");
    assert!(m.confirm.is_none());
    ask(&mut m, &said);
    press(&mut m, "s");
    assert_eq!(*said.borrow(), "save");
    ask(&mut m, &said);
    press(&mut m, "right");
    assert_eq!(*said.borrow(), "yes", "again says yes");
    ask(&mut m, &said);
    said.borrow_mut().clear();
    press(&mut m, "esc");
    assert_eq!(*said.borrow(), "", "esc cancels");
    assert!(m.confirm.is_none());
}

// ---- the picker and the choice sheet ----

#[test]
fn a_picker_narrows_as_you_type() {
    let mut m = model(120, 40);
    let got = Rc::new(RefCell::new(String::new()));
    let g = got.clone();
    let rows = ["~/src/rush", "~/src/photon", "~/notes"]
        .iter()
        .map(|d| Pick::new(*d).under("Projects"))
        .collect();
    let mut p = Picker::new(
        "Start new sessions in",
        rows,
        Box::new(move |_, pick, typed| {
            *g.borrow_mut() = pick.map_or(typed.to_string(), |p| p.value.clone());
            Out::Draw
        }),
    );
    p.query = Some(("Folder ❯ ".into(), Vec::new()));
    p.empty = "no folder matches · type a path".into();
    m.open_sheet(Box::new(p));
    let f = screen(&mut m);
    assert!(
        f.iter().any(|r| r.contains("Start new sessions in")),
        "{f:?}"
    );
    assert!(f.iter().any(|r| r.contains(" Projects")));
    for c in "pho".chars() {
        m.press_key(&Press::new(Key::Char(c), KeyMods::NONE));
    }
    assert_eq!(m.picker().unwrap().shown().len(), 1);
    press(&mut m, "enter");
    assert_eq!(*got.borrow(), "~/src/photon");
    assert!(m.sheet.is_none());
}

#[test]
fn a_choice_sheet_marks_whats_running() {
    let mut m = model(120, 40);
    let got = Rc::new(RefCell::new(String::new()));
    let g = got.clone();
    let opts = vec![
        choice::Choice {
            id: String::new(),
            note: "the agent's own".into(),
        },
        choice::Choice {
            id: "opus".into(),
            note: "the most able".into(),
        },
        choice::Choice {
            id: "sonnet".into(),
            note: "quicker".into(),
        },
    ];
    let s = ChoiceSheet::new(
        "model",
        "for this session, from the next turn",
        opts,
        Box::new(move |_, c| {
            *g.borrow_mut() = c.id.clone();
            Out::Draw
        }),
    )
    .running("opus");
    m.open_sheet(Box::new(s));
    let f = screen(&mut m);
    assert!(
        f.iter().any(|r| r.contains("Model  for this session")),
        "{f:?}"
    );
    assert!(f.iter().any(|r| r.contains("▍ ✓ opus")), "{f:?}");
    assert!(
        f.iter()
            .any(|r| r.contains("/model <name> takes one not listed"))
    );
    press(&mut m, "down");
    press(&mut m, "enter");
    assert_eq!(*got.borrow(), "sonnet");
    assert!(m.sheet.is_none());
}

// ---- hooks ----

#[test]
fn hooks_come_first_and_commands_run() {
    let mut m = model(100, 30);
    fn grab(m: &mut Model, k: &Press) -> Out {
        if k.s == "ctrl+k" {
            m.flash("bar");
            return Out::Draw;
        }
        Out::Pass
    }
    fn hello(m: &mut Model, arg: &str) -> Out {
        m.flash(&format!("hello {arg}"));
        Out::Draw
    }
    m.ext.keys.insert(0, grab); // before the command bar's own
    m.ext.command("hello", "says hello", hello);
    m.ext.agents_box = true;
    press(&mut m, "ctrl+k");
    assert_eq!(m.flash.as_ref().unwrap().0, "bar");
    for c in "#Hello:there".chars() {
        m.press_key(&Press::new(Key::Char(c), KeyMods::NONE));
    }
    press(&mut m, "enter");
    assert_eq!(m.flash.as_ref().unwrap().0, "hello there");
    assert!(m.input.is_empty());
    for c in "#nope".chars() {
        m.press_key(&Press::new(Key::Char(c), KeyMods::NONE));
    }
    press(&mut m, "enter");
    assert_eq!(
        m.flash.as_ref().unwrap().0,
        "unknown command #nope · # lists rush's"
    );
}

/// Work handed off with no loop runs at once; with one, it comes back
/// as a Note::Apply.
#[test]
fn later_runs_off_the_ui_thread() {
    let mut m = model(100, 30);
    m.later(
        || 6 * 7,
        |m, v| {
            m.flash(&format!("got {v}"));
            Out::Draw
        },
    );
    assert_eq!(m.flash.as_ref().unwrap().0, "got 42");

    let lp: rush_draw::Loop<crate::worker::Note> = rush_draw::Loop::new();
    let mut m = Model::with_workers("0.1.0", lp.sender());
    m.later(
        || "off",
        |m, v| {
            m.flash(v);
            Out::Quit
        },
    );
    lp.run_with(&mut m, |_| Ok(())).unwrap();
    assert_eq!(m.flash.as_ref().unwrap().0, "off");
}

/// ctrl+c with nothing typed arms quitting; only a second one quits
/// (keys.go:650 quitKey), in the list and in a place alike.
#[test]
fn ctrl_c_twice_quits() {
    let mut m = model(120, 40);
    m.ext.agents_box = true;
    press(&mut m, "a");
    assert_eq!(press(&mut m, "ctrl+c"), Out::Draw, "clears the box");
    assert!(m.input.is_empty());
    assert_ne!(press(&mut m, "ctrl+c"), Out::Quit, "the first only arms");
    assert_eq!(m.flash.as_ref().unwrap().0, "ctrl+c again to quit");
    assert_eq!(press(&mut m, "ctrl+c"), Out::Quit);

    let mut m = model(120, 40);
    press(&mut m, ".");
    assert_ne!(m.place, PlaceId::Agents);
    assert_ne!(press(&mut m, "ctrl+c"), Out::Quit);
    assert_eq!(press(&mut m, "ctrl+c"), Out::Quit);
}

/// A task starting "#123" or "# Plan" is a message, not a command: # needs
/// a letter after it (keys.go:841 isHashCmd).
#[test]
fn hash_then_a_letter_is_a_command() {
    fn took(m: &mut Model, t: &str) -> Out {
        m.flash(&format!("started {t}"));
        Out::Draw
    }
    for (typed, said) in [
        ("#123 fix the login bug", "started #123 fix the login bug"),
        ("# Plan the migration", "started # Plan the migration"),
        ("#nope", "unknown command #nope · # lists rush's"),
    ] {
        let mut m = model(120, 40);
        m.ext.agents_box = true;
        m.ext.submit.push(took);
        m.input.set(typed);
        press(&mut m, "enter");
        assert_eq!(m.flash.as_ref().unwrap().0, said);
    }
}
