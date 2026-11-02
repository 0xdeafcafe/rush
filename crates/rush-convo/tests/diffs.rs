//! A turn's changes in the diff box (draw/diffbox.rs), in the Code view:
//! one column on a narrower pane, side by side on a wide one; a new,
//! deleted and renamed file; long unchanged stretches folded and a fold
//! opened; the words that changed on their own ground.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::{SIDE_BY_SIDE, plain};
use rush_convo::{Options, Preset, RenderedLine, Renderer, Session};
use rush_core::event::{self, Event, PartKind};
use rush_core::text::Color;
use rush_core::tool::{Call, Input, Kind, Output, Patch};

mod common;

use common::*;

/// The width the narrow snapshots are drawn at, and the wide.
const NARROW: usize = 100;
const WIDE: usize = 180;

fn call(id: &str, kind: Kind, input: Input) -> Ev {
    Ev::Do(Event::Message(event::Message {
        role: "assistant".into(),
        id: format!("m-{id}"),
        parts: vec![event::Part {
            kind: PartKind::ToolCall,
            call: Some(Box::new(Call {
                id: id.into(),
                name: "apply_patch".into(),
                kind,
                input,
                ..Call::default()
            })),
            ..event::Part::default()
        }],
        ..event::Message::default()
    }))
}

fn result(id: &str, out: Output) -> Ev {
    Ev::Do(Event::Message(event::Message {
        role: "user".into(),
        parts: vec![event::Part {
            kind: PartKind::ToolResult,
            output: Some(Box::new(Output {
                call_id: id.into(),
                text: "ok".into(),
                ..out
            })),
            ..event::Part::default()
        }],
        ..event::Message::default()
    }))
}

fn path(p: &str) -> Input {
    Input {
        path: format!("/work/rush/{p}"),
        ..Input::default()
    }
}

fn patch(old: i64, new: i64, lines: &[&str]) -> Patch {
    Patch {
        old_start: old,
        new_start: new,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        ..Patch::default()
    }
}

/// A turn that asks, then makes the changes evs gives, then ends.
fn session(evs: Vec<Ev>) -> Session {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(sent("make the change"), at(0));
    for (i, e) in evs.into_iter().enumerate() {
        s.feed(e, at(1 + i as i64));
    }
    s.feed(done(0.10), at(100));
    s
}

/// The small edit: a comment and a constant changed.
fn small_edit() -> Vec<Ev> {
    vec![
        call("e1", Kind::Edit, path("crates/rush-update/src/lib.rs")),
        result(
            "e1",
            Output {
                patches: vec![patch(
                    44,
                    44,
                    &[
                        " pub const FORMULA: &str = \"0xdeafcafe/tap/rush\";",
                        " ",
                        "-/// without a token GitHub allows 60 calls an hour, far more than this.",
                        "-pub const EVERY: Duration = Duration::from_secs(6 * 60 * 60);",
                        "+/// without a token GitHub allows 60 calls an hour, once an hour.",
                        "+pub const EVERY: Duration = Duration::from_secs(60 * 60);",
                        " ",
                        " pub fn check() {}",
                    ],
                )],
                ..Output::default()
            },
        ),
    ]
}

fn new_file() -> Vec<Ev> {
    let content = "//! Says hello.\n\npub fn hello() -> &'static str {\n    \"hello\"\n}\n";
    let input = Input {
        content: content.into(),
        ..path("crates/rush-hello/src/lib.rs")
    };
    vec![
        call("w1", Kind::Write, input),
        result(
            "w1",
            Output {
                created: true,
                ..Output::default()
            },
        ),
    ]
}

fn deleted_file() -> Vec<Ev> {
    vec![
        call("d1", Kind::Delete, path("crates/rush-old/src/lib.rs")),
        result("d1", Output::default()),
    ]
}

/// Two hunks, the first with a long unchanged stretch between its
/// changes, the second further down the file.
fn big_change() -> Vec<Ev> {
    let mut first = vec![" fn a() {}", "-fn b() {}", "+fn b() { a() }"];
    let middle: Vec<String> = (1..=12)
        .map(|i| format!(" const C{i}: u32 = {i};"))
        .collect();
    first.extend(middle.iter().map(String::as_str));
    first.extend(["-fn c() {}", "+fn c() { b() }", " fn d() {}"]);
    vec![
        call("e2", Kind::Edit, path("src/big.rs")),
        result(
            "e2",
            Output {
                patches: vec![
                    patch(10, 10, &first),
                    patch(80, 80, &[" fn y() {}", "-fn z() {}", " fn end() {}"]),
                ],
                ..Output::default()
            },
        ),
    ]
}

/// A new file longer than a box shows until opened.
fn long_file() -> Vec<Ev> {
    let content: String = (1..=60).map(|i| format!("line {i}\n")).collect();
    let input = Input {
        content,
        ..path("notes/long.txt")
    };
    vec![
        call("w2", Kind::Write, input),
        result(
            "w2",
            Output {
                created: true,
                ..Output::default()
            },
        ),
    ]
}

fn opts(w: usize, open: &[&str]) -> Options {
    let mut o = Options {
        width: w,
        now: at(200),
        zone: ZONE,
        speaker: "Claude".into(),
        preset: Preset::Code,
        ..Options::default()
    };
    for r in open {
        o.open.insert(r.to_string(), true);
    }
    o
}

fn lines(s: &Session, w: usize, open: &[&str]) -> Vec<RenderedLine> {
    Renderer::new().render(s, &opts(w, open))
}

fn draw(s: &Session, w: usize, open: &[&str]) -> String {
    plain(&lines(s, w, open))
}

/// got checked against tests/snapshots/<name>.txt; UPDATE_SNAPSHOTS=1
/// writes it. A missing one fails, with what was drawn, so CI shows it.
fn snapshot(name: &str, got: &str) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap_or_default();
    assert!(
        got == want,
        "{name} differs; UPDATE_SNAPSHOTS=1 to accept\n=== {name} drawn ===\n{got}=== end {name} ==="
    );
}

#[test]
fn diffs_at_both_widths() {
    let cases: [(&str, fn() -> Vec<Ev>); 5] = [
        ("small", small_edit),
        ("new", new_file),
        ("deleted", deleted_file),
        ("big", big_change),
        ("long", long_file),
    ];
    for (name, evs) in cases {
        let s = session(evs());
        snapshot(&format!("diff_{name}_narrow"), &draw(&s, NARROW, &[]));
        snapshot(&format!("diff_{name}_wide"), &draw(&s, WIDE, &[]));
    }
}

#[test]
fn a_file_says_what_happened_to_it() {
    let got = draw(&session(new_file()), NARROW, &[]);
    let top = got.lines().find(|l| l.contains("╭─")).expect(got.as_str());
    assert!(
        top.contains("crates/rush-hello/src/lib.rs") && top.contains("new file"),
        "{top}"
    );
    assert!(top.contains("+5"), "{top}");
    let got = draw(&session(deleted_file()), NARROW, &[]);
    let top = got.lines().find(|l| l.contains("╭─")).expect(got.as_str());
    assert!(top.contains("deleted"), "{top}");
    let mut input = path("src/old.rs");
    input.to = "/work/rush/src/new.rs".into();
    let s = session(vec![
        call("m1", Kind::Move, input),
        result("m1", Output::default()),
    ]);
    let got = draw(&s, NARROW, &[]);
    let top = got.lines().find(|l| l.contains("╭─")).expect(got.as_str());
    assert!(
        top.contains("src/new.rs") && top.contains("renamed from src/old.rs"),
        "{top}"
    );
}

#[test]
fn the_box_shows_old_and_new_numbers() {
    let got = draw(&session(small_edit()), NARROW, &[]);
    for want in [
        "44  44   pub const FORMULA",
        "46     − /// without a token",
        "    46 + /// without a token",
        "47     − pub const EVERY",
        "    47 + pub const EVERY",
        "48  48   ",
    ] {
        assert!(got.contains(want), "missing {want:?} in:\n{got}");
    }
    // Every row of the box the same width, inside the pane.
    let rows: Vec<&str> = got
        .lines()
        .map(|l| l.split_once('|').unwrap().1)
        .filter(|l| l.contains('│') || l.contains('╭') || l.contains('╰'))
        .collect();
    let w = rows[0].trim_end().chars().count();
    assert!(w <= NARROW, "{got}");
    for r in &rows {
        assert_eq!(r.trim_end().chars().count(), w, "{r:?} in:\n{got}");
    }
}

#[test]
fn side_by_side_from_its_width() {
    assert_eq!(SIDE_BY_SIDE, 160);
    let s = session(small_edit());
    let paired = |got: &str| {
        got.lines()
            .any(|l| l.contains("from_secs(6 * 60 * 60)") && l.contains("from_secs(60 * 60)"))
    };
    let narrow = draw(&s, SIDE_BY_SIDE - 1, &[]);
    assert!(!paired(&narrow), "one column below it:\n{narrow}");
    let wide = draw(&s, SIDE_BY_SIDE, &[]);
    assert!(paired(&wide), "side by side from it:\n{wide}");
    // The removed line and the added one it pairs with on one row; the
    // unpaired leaves its other side blank.
    let row = wide
        .lines()
        .find(|l| l.contains("from_secs(6 * 60 * 60)"))
        .unwrap();
    assert!(row.contains("47 − ") && row.contains("47 + "), "{row}");
    let s = session(big_change());
    let wide = draw(&s, SIDE_BY_SIDE, &[]);
    let row = wide.lines().find(|l| l.contains("fn z() {}")).unwrap();
    assert!(!row.contains(" + "), "{row}");
}

#[test]
fn a_long_stretch_folds_and_opens() {
    let s = session(big_change());
    let got = draw(&s, NARROW, &[]);
    let fold = got
        .lines()
        .find(|l| l.contains("▸ 6 lines"))
        .expect(got.as_str());
    let r = fold.split('|').next().unwrap().trim();
    assert_eq!(r, "t1:file:0:fold:0");
    assert!(got.contains("C3: u32") && got.contains("C10: u32"), "{got}");
    assert!(
        !got.contains("C4: u32") && !got.contains("C9: u32"),
        "{got}"
    );
    // Lines left out between the hunks.
    assert!(got.contains("⋯ 54 lines"), "{got}");
    let open = draw(&s, NARROW, &[r]);
    assert!(
        open.contains("C4: u32") && open.contains("C9: u32"),
        "{open}"
    );
    assert!(open.contains("▾ 6 lines"), "{open}");
    // Side by side too.
    let open = draw(&s, WIDE, &[r]);
    assert!(open.contains("C4: u32"), "{open}");
}

#[test]
fn a_long_change_shows_all_when_asked() {
    let s = session(long_file());
    let got = draw(&s, NARROW, &[]);
    let all = got
        .lines()
        .find(|l| l.contains("▸ show all"))
        .expect(got.as_str());
    assert!(all.contains("20 more lines"), "{all}");
    assert_eq!(all.split('|').next().unwrap().trim(), "t1:file:0:all");
    assert!(got.contains("line 40") && !got.contains("line 41"), "{got}");
    let open = draw(&s, NARROW, &["t1:file:0:all"]);
    assert!(
        open.contains("line 60") && open.contains("▾ show less"),
        "{open}"
    );
    // The file's own row opened shows it whole.
    let open = draw(&s, NARROW, &["t1:file:0"]);
    assert!(open.contains("line 60") && !open.contains("show"), "{open}");
}

#[test]
fn the_words_that_changed_have_a_stronger_ground() {
    const DEL: Color = Color::Rgb(0x3a, 0x17, 0x14);
    const DEL_HI: Color = Color::Rgb(0x62, 0x24, 0x1e);
    const ADD: Color = Color::Rgb(0x16, 0x30, 0x1a);
    const ADD_HI: Color = Color::Rgb(0x22, 0x52, 0x2b);
    let s = session(small_edit());
    for w in [NARROW, WIDE] {
        let ls = lines(&s, w, &[]);
        let row = |has: &str| {
            ls.iter()
                .find(|l| l.line.plain().contains(has))
                .unwrap_or_else(|| panic!("no {has:?}"))
        };
        let on = |l: &RenderedLine, bg: Color| -> String {
            l.line
                .spans
                .iter()
                .filter(|s| s.style.bg == bg)
                .map(|s| s.text.as_str())
                .collect()
        };
        let del = row("from_secs(6 * 60 * 60)");
        assert_eq!(on(del, DEL_HI), "6 * ", "width {w}");
        assert!(on(del, DEL).contains("pub const EVERY"), "width {w}");
        let add = row("once an hour.");
        assert_eq!(on(add, ADD_HI), "once an hour", "width {w}");
        assert!(on(add, ADD).contains("/// without a token"), "width {w}");
        // An unchanged line has no ground.
        let same = row("pub const FORMULA");
        assert!(same.line.spans.iter().all(|s| s.style.bg == Color::Default));
    }
}
