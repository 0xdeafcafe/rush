//! plugin_ui against Go at the widths 80, 120 and 200 column screens give
//! it: testdata/wide/*.txt are what Go draws, plain; *.sty the same rows'
//! colours, as runs of cells `fg.bg.mods:count` (a space's colour and
//! weight left out), up to each row's last non-space cell. Written by a
//! throwaway Go test: a sheet's body is its panel (sheetWidth) less 4,
//! its height the screen's less 6, on screens 80x24, 120x40 and 200x50.

use rush_core::text::{Color, Line, Mods};

use super::*;

const SCREENS: [(usize, usize); 3] = [(80, 24), (120, 40), (200, 50)];

/// A sheet's body width on a screen sw wide (sheet.go:38 sheetWidth).
fn body_w(want: usize, sw: usize) -> usize {
    40.max(sw.saturating_sub(6).min(want)) - 4
}

fn hex(c: Color) -> String {
    match c {
        Color::Default => "-".into(),
        Color::Rgb(r, g, b) => format!("{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(n) => format!("i{n}"),
    }
}

/// One row's colours as runs, as the Go oracle writes them.
pub(super) fn runs(l: &Line) -> String {
    let mut cells: Vec<(char, String)> = Vec::new();
    for s in &l.spans {
        let bg = if s.style.bg == Color::Default {
            l.bg
        } else {
            s.style.bg
        };
        let m = s.style.mods;
        for ch in s.text.chars() {
            let w = rush_core::text::width(ch.encode_utf8(&mut [0; 4]));
            if w == 0 && !cells.is_empty() {
                continue;
            }
            let mut md = String::new();
            for (f, c) in [
                (Mods::BOLD, 'b'),
                (Mods::DIM, 'd'),
                (Mods::ITALIC, 'i'),
                (Mods::UNDERLINE, 'u'),
                (Mods::REVERSE, 'r'),
                (Mods::STRIKE, 's'),
            ] {
                if m.contains(f) && (ch != ' ' || "urs".contains(c)) {
                    md.push(c);
                }
            }
            let fg = if ch == ' ' {
                "-".into()
            } else {
                hex(s.style.fg)
            };
            let k = format!("{fg}.{}.{md}", hex(bg));
            cells.push((ch, k.clone()));
            for _ in 1..w {
                cells.push(('\0', k.clone()));
            }
        }
    }
    while cells.last().is_some_and(|c| c.0 == ' ') {
        cells.pop();
    }
    let mut out: Vec<String> = Vec::new();
    let mut last = String::new();
    let mut n = 0;
    for (_, k) in cells {
        if k == last {
            n += 1;
            continue;
        }
        if n > 0 {
            out.push(format!("{last}:{n}"));
        }
        last = k;
        n = 1;
    }
    if n > 0 {
        out.push(format!("{last}:{n}"));
    }
    out.join(" ")
}

/// Every row that differs from Go's, plain or in colour; skip leaves out
/// rows a fixture can't make the same (the data folder's path).
fn diffs(
    name: &str,
    sets: &[Vec<Line>],
    txt: &str,
    sty: &str,
    skip: fn(&str) -> bool,
) -> Vec<String> {
    let split = |s: &str| -> Vec<Vec<String>> {
        s.strip_suffix('\n')
            .unwrap_or(s)
            .split("\n=====\n")
            .map(|set| set.split('\n').map(String::from).collect())
            .collect()
    };
    let (txt, sty) = (split(txt), split(sty));
    let mut out = Vec::new();
    if txt.len() != sets.len() {
        out.push(format!("{name}: {} sets, Go has {}", sets.len(), txt.len()));
        return out;
    }
    for (i, got) in sets.iter().enumerate() {
        let plain: Vec<String> = got
            .iter()
            .map(|l| l.plain().trim_end_matches(' ').to_string())
            .collect();
        let colours: Vec<String> = got.iter().map(runs).collect();
        let (wp, ws) = (&txt[i], &sty[i]);
        // Rows a fixture can't match are dropped on both sides.
        let keep = |p: &[String]| -> Vec<bool> {
            let mut skipping = false;
            p.iter()
                .map(|r| {
                    if skip(r) {
                        skipping = true;
                        return false;
                    }
                    // Its path wraps under it, as long as the home is, and
                    // pushes what follows out of a short window: the rest
                    // of the list goes too.
                    if skipping && !r.trim().is_empty() {
                        return false;
                    }
                    skipping = false;
                    true
                })
                .collect()
        };
        let pick = |v: &[String], k: &[bool]| -> Vec<String> {
            v.iter()
                .zip(k)
                .filter(|(_, k)| **k)
                .map(|(s, _)| s.clone())
                .collect()
        };
        let (kg, kw) = (keep(&plain), keep(wp));
        let (gp, gs, wp, ws) = (
            pick(&plain, &kg),
            pick(&colours, &kg),
            pick(wp, &kw),
            pick(ws, &kw),
        );
        if gp.len() != wp.len() {
            out.push(format!(
                "{name} set {i}: {} rows, Go has {}\n got: {gp:#?}\nwant: {wp:#?}",
                gp.len(),
                wp.len()
            ));
            continue;
        }
        for r in 0..gp.len() {
            if gp[r] != wp[r] {
                out.push(format!(
                    "{name} set {i} row {r}:\n got {:?}\nwant {:?}",
                    gp[r], wp[r]
                ));
            } else if gs[r] != ws[r] {
                out.push(format!(
                    "{name} set {i} row {r} colours ({:?}):\n got {}\nwant {}",
                    gp[r], gs[r], ws[r]
                ));
            }
        }
    }
    out
}

fn none(_: &str) -> bool {
    false
}

fn data_dir(r: &str) -> bool {
    r.starts_with("    • writes only ")
}

fn check(all: Vec<String>) {
    assert!(
        all.is_empty(),
        "{} rows differ:\n{}",
        all.len(),
        all.join("\n")
    );
}

#[test]
fn pick_wide() {
    let m = model();
    let mut sets = Vec::new();
    for (sw, sh) in SCREENS {
        let (w, h) = (body_w(112, sw), 8.max(sh - 6));
        let mut s = pick::PickSheet::new("haven", pick());
        sets.push(s.body(&m, w, h));
        s.cur = 2;
        sets.push(s.body(&m, w, h));
        s.tab = 1;
        sets.push(s.body(&m, w, h));
        s.tab = 0;
        s.filter = "zzz".chars().collect();
        sets.push(s.body(&m, w, h));
        let mut s = pick::PickSheet::new(STASH_PLUGIN, pick());
        sets.push(s.body(&m, w, h));
    }
    check(diffs(
        "pick",
        &sets,
        include_str!("testdata/wide/pick.txt"),
        include_str!("testdata/wide/pick.sty"),
        none,
    ));
}

#[test]
fn offer_wide() {
    let m = model();
    let mut sets = Vec::new();
    for (sw, sh) in SCREENS {
        let (w, h) = (body_w(100, sw), 8.max(sh - 6));
        let mut s = offer::OfferSheet::new(offer());
        sets.push(s.body(&m, w, h));
        let mut o2 = offer();
        o2.update = true;
        o2.was = "0123456789abcdef".into();
        o2.commit = "fedcba9876543210".into();
        o2.commits = 3;
        let mut s = offer::OfferSheet::new(o2);
        s.cur = 0;
        s.err = "install.sh failed: exit 1".into();
        sets.push(s.body(&m, w, h));
        s.busy = true;
        sets.push(s.body(&m, w, h));
    }
    check(diffs(
        "offer",
        &sets,
        include_str!("testdata/wide/offer.txt"),
        include_str!("testdata/wide/offer.sty"),
        none,
    ));
}

#[test]
fn approval_wide() {
    let m = model();
    let mut sets = Vec::new();
    for (sw, sh) in SCREENS {
        let (w, h) = (body_w(100, sw), 8.max(sh - 6));
        let mut s = approval::ApprovalSheet {
            p: weather(),
            rest: Vec::new(),
            cur: 0,
            busy: false,
            err: String::new(),
            changed: false,
        };
        sets.push(s.body(&m, w, h));
        s.changed = true;
        s.cur = 1;
        s.err = "couldn't write approvals".into();
        sets.push(s.body(&m, w, h));
    }
    check(diffs(
        "approval",
        &sets,
        include_str!("testdata/wide/approval.txt"),
        include_str!("testdata/wide/approval.sty"),
        data_dir,
    ));
}

#[test]
fn optional_wide() {
    let m = model();
    let picks = vec![
        opt(
            "drafts",
            "Keeps what you clear or send, so nothing typed is lost.",
            "Messages",
            false,
        ),
        opt(
            "queue",
            "Holds messages until the agent is free, then sends them in order, one at a time, each when the last is answered.",
            "Messages",
            true,
        ),
        opt("yap", "Talk instead of typing.", "Voice", false),
    ];
    let mut sets = Vec::new();
    for (sw, sh) in SCREENS {
        let (w, h) = (body_w(100, sw), 8.max(sh - 6));
        let mut s = picks::OptionalSheet {
            picks: picks.clone(),
            first: true,
            never: true,
            cur: 0,
            busy: false,
        };
        sets.push(s.body(&m, w, h));
        s.first = false;
        s.cur = 1;
        sets.push(s.body(&m, w, h));
        s.never = false;
        s.busy = true;
        sets.push(s.body(&m, w, h));
    }
    check(diffs(
        "optional",
        &sets,
        include_str!("testdata/wide/optional.txt"),
        include_str!("testdata/wide/optional.sty"),
        none,
    ));
}

#[test]
fn layout_wide() {
    let m = model();
    let mut sets = Vec::new();
    for (sw, sh) in SCREENS {
        let (w, h) = (body_w(76, sw), 8.max(sh - 6));
        let mut v = layout::LayoutSheet::default();
        sets.push(v.body(&m, w, h));
        v.cur = 2;
        sets.push(v.body(&m, w, h));
    }
    check(diffs(
        "layout",
        &sets,
        include_str!("testdata/wide/layout.txt"),
        include_str!("testdata/wide/layout.sty"),
        none,
    ));
}

#[test]
fn plugins_wide() {
    let m = model();
    let mut sets = Vec::new();
    for (sw, sh) in SCREENS {
        let (w, h) = (body_w(112, sw), 8.max(sh - 6));
        let mut p = plugins_sheet();
        sets.push(p.body(&m, w, h));
        p.cur[sheet::INSTALLED] = 1;
        p.done = "turning on notes".into();
        p.changed = true;
        sets.push(p.body(&m, w, h));
        p.tab = sheet::DISCOVER;
        p.done.clear();
        p.changed = false;
        sets.push(p.body(&m, w, h));
        p.query = "sl".chars().collect();
        p.query_pos = 2;
        sets.push(p.body(&m, w, h));
        p.tab = sheet::MARKETS;
        sets.push(p.body(&m, w, h));
        p.adding = true;
        p.source = "owner/re".chars().collect();
        p.source_pos = 8;
        sets.push(p.body(&m, w, h));
        p.adding = false;
        p.busy = "updating acme…".into();
        sets.push(p.body(&m, w, h));
        p.busy.clear();
        p.err = "claude plugin marketplace update failed: no network".into();
        sets.push(p.body(&m, w, h));
        let mut q = plugins_sheet();
        q.loaded = false;
        sets.push(q.body(&m, w, h));
    }
    check(diffs(
        "plugins",
        &sets,
        include_str!("testdata/wide/plugins.txt"),
        include_str!("testdata/wide/plugins.sty"),
        none,
    ));
}

#[test]
fn doc_wide() {
    let dir = temp_dir("docwide");
    let md = dir.join("memory").join("note.md");
    std::fs::create_dir_all(md.parent().unwrap()).unwrap();
    std::fs::write(&md, MD_TEXT).unwrap();
    let js = dir.join("settings.json");
    std::fs::write(&js, JS_TEXT).unwrap();
    let mut sets = Vec::new();
    for p in [&md, &js] {
        for (sw, sh) in SCREENS {
            let (w, h) = (sw - 6, sh - 6);
            let mut e = doc::DocEditor::open_now(&p.to_string_lossy());
            e.view(w, h, true);
            e.pos = e.buf.len() / 2;
            sets.push(e.view(w, h, true));
            e.pos = e.buf.len();
            sets.push(e.view(w, h, false));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    check(diffs(
        "doc",
        &sets,
        include_str!("testdata/wide/doc.txt"),
        include_str!("testdata/wide/doc.sty"),
        none,
    ));
}

#[test]
fn views_wide() {
    use views::*;
    let v: View = serde_json::from_str(include_str!("views/testdata/all.json")).unwrap();
    let small = View {
        name: "prs".into(),
        title: "Pull requests".into(),
        summary: "2 open".into(),
        updated: v.updated,
        ..Default::default()
    };
    let now = v.updated.to_ms() + 90_000;
    let vs = vec![v, small];
    let mut sets = Vec::new();
    for (sw, sh) in [(120, 40), (200, 50)] {
        let w = views_width(sw);
        let mut st = ViewsState::default();
        sets.push(views_panel(&vs, &[], w, sh - 8, now, &mut st));
        st.scroll = 30;
        sets.push(views_panel(&vs, &[], w, sh - 8, now, &mut st));
    }
    check(diffs(
        "views",
        &sets,
        include_str!("testdata/wide/views.txt"),
        include_str!("testdata/wide/views.sty"),
        none,
    ));
}

/// Every sheet, the editor and the panel at every width and height down
/// to nothing, with wide characters and with nothing in them: no panic,
/// and no row of the panel or the editor wider than it was given.
#[test]
fn narrow_wide_and_empty() {
    use views::*;
    let m = model();
    let wide = "日本語のテキスト🙂".repeat(6);
    let mut wpick = pick();
    for it in &mut wpick.items {
        it.text = format!("{wide}\n{wide}");
        it.meta = wide.clone();
    }
    wpick.title = wide.clone();
    let mut woffer = offer();
    woffer.name = wide.clone();
    woffer.description = wide.clone();
    let mut wplug = weather();
    wplug.manifest.name = wide.clone();
    wplug.manifest.description = wide.clone();
    let mut wsheet = plugins_sheet();
    for p in wsheet
        .installed
        .iter_mut()
        .chain(wsheet.available.iter_mut())
    {
        p.name = wide.clone();
        p.description = wide.clone();
    }
    wsheet.err = wide.clone();
    let mut empty = plugins_sheet();
    empty.installed.clear();
    empty.available.clear();
    empty.markets.clear();
    let v: View = serde_json::from_str(include_str!("views/testdata/all.json")).unwrap();
    // Every word in every block made wide.
    fn widen(x: &mut serde_json::Value, k: &str, wide: &str) {
        match x {
            serde_json::Value::String(s)
                if ![
                    "kind", "tone", "style", "accent", "link", "name", "updated", "open", "copy",
                ]
                .contains(&k) =>
            {
                s.push_str(wide)
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(|v| widen(v, k, wide)),
            serde_json::Value::Object(o) => o.iter_mut().for_each(|(k, v)| widen(v, k, wide)),
            _ => {}
        }
    }
    let mut raw: serde_json::Value =
        serde_json::from_str(include_str!("views/testdata/all.json")).unwrap();
    widen(&mut raw, "", &wide);
    let mut wv: View = serde_json::from_value(raw).unwrap();
    wv.name = "wide".into();
    let vs = vec![v, wv, View::default()];
    let dir = temp_dir("narrow");
    let md = dir.join("w.md");
    std::fs::write(&md, format!("# {wide}\n- {wide}\n```\n{wide}\n```\n")).unwrap();
    let mut ed = doc::DocEditor::open_now(&md.to_string_lossy());
    let empty_md = dir.join("e.md");
    std::fs::write(&empty_md, "").unwrap();
    let mut ed0 = doc::DocEditor::open_now(&empty_md.to_string_lossy());
    let fits = |what: &str, w: usize, ls: &[Line]| {
        for l in ls {
            assert!(
                l.width() <= w.max(1),
                "{what} at {w}: {:?} is {}",
                l.plain(),
                l.width()
            );
        }
    };
    // A sheet's rows are fitted to its box by the frame, as Go's
    // overlayBox does: only that it draws at all is checked.
    let sheet_fits = |_w: usize, _ls: Vec<Line>| {};
    for w in 0..90 {
        for h in [0, 1, 3, 8, 20] {
            for p in [pick(), wpick.clone(), rush_plugin::Pick::default()] {
                let mut s = pick::PickSheet::new("haven", p);
                sheet_fits(w, s.body(&m, w, h));
                s.filter = "日".chars().collect();
                sheet_fits(w, s.body(&m, w, h));
            }
            for o in [offer(), woffer.clone(), Offer::default()] {
                sheet_fits(w, offer::OfferSheet::new(o).body(&m, w, h));
            }
            for p in [weather(), wplug.clone(), Plugin::default()] {
                let mut s = approval::ApprovalSheet {
                    p,
                    rest: Vec::new(),
                    cur: 0,
                    busy: false,
                    err: wide.clone(),
                    changed: false,
                };
                sheet_fits(w, s.body(&m, w, h));
            }
            for picks in [vec![], vec![opt(&wide, &wide, &wide, true)]] {
                let mut s = picks::OptionalSheet {
                    picks,
                    first: true,
                    never: true,
                    cur: 0,
                    busy: false,
                };
                sheet_fits(w, s.body(&m, w, h));
            }
            sheet_fits(w, layout::LayoutSheet::default().body(&m, w, h));
            for p in [&mut wsheet, &mut empty] {
                for tab in [sheet::INSTALLED, sheet::DISCOVER, sheet::MARKETS] {
                    p.tab = tab;
                    sheet_fits(w, p.body(&m, w, h));
                }
            }
            let got = ed.view(w, h, true);
            if w >= 8 {
                fits("editor", w, &got);
            }
            let _ = ed0.view(w, h, true);
            let mut st = ViewsState::default();
            for tab in ["tests", "wide", ""] {
                st.tab = tab.into();
                let got = views_panel(&vs, &[], w, h.max(60), 0, &mut st);
                if w >= 30 {
                    fits("views", w, &got);
                }
                // every row opened
                let rows: Vec<String> = st.acts.iter().map(|a| a.open.clone()).collect();
                for k in rows.into_iter().filter(|k| !k.is_empty()) {
                    st.open.insert(k, true);
                }
                let got = views_panel(&vs, &[], w, h.max(60), 0, &mut st);
                if w >= 30 {
                    fits("views opened", w, &got);
                }
            }
            st.shut = true;
            let _ = views_panel(&vs, &[], w, h, 0, &mut st);
            st.shut = false;
            st.past = true;
            let _ = views_panel(&[], &vs, w, h, 0, &mut st);
            let _ = views_panel(&[], &[], w, h, 0, &mut ViewsState::default());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The editor's keys at every place in an empty file, a markdown one with
/// wide characters and a JSON one: none panics, and the cursor stays in
/// the text.
#[test]
fn doc_keys_everywhere() {
    use crate::frame::Press;
    let dir = temp_dir("dockeys");
    let keys = [
        "left",
        "right",
        "up",
        "down",
        "home",
        "end",
        "ctrl+a",
        "ctrl+e",
        "ctrl+left",
        "ctrl+right",
        "alt+left",
        "alt+right",
        "shift+left",
        "shift+right",
        "shift+up",
        "shift+down",
        "shift+home",
        "shift+end",
        "enter",
        "tab",
        "shift+tab",
        "alt+up",
        "alt+down",
        "ctrl+t",
        "ctrl+b",
        "ctrl+i",
        "backspace",
        "delete",
        "ctrl+k",
        "ctrl+u",
        "ctrl+w",
        "alt+backspace",
        "ctrl+z",
        "ctrl+y",
        "pgup",
        "pgdown",
        "ctrl+home",
        "ctrl+end",
        "\"",
        "[",
        "{",
        "}",
        "]",
        "*",
        "`",
        "x",
        "日",
        "space",
    ];
    for (name, text) in [
        ("e.md", ""),
        ("w.md", "# 日本\n- [ ] 🙂 item\n\t1. 語\n> q\n```\nx\n```\n"),
        ("j.json", "{\n  \"a\": [1, \"日\"],\n  \"b\": {}\n}\n"),
        ("e.json", ""),
    ] {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        let mut e = doc::DocEditor::open_now(&p.to_string_lossy());
        let n = e.buf.len();
        for start in 0..=n {
            for k in keys {
                let k = Press::of(k).unwrap_or_else(|| panic!("no key {k}"));
                e.change(text.chars().collect(), start, false);
                e.view(12, 5, true);
                e.key(&k);
                e.key(&k);
                assert!(e.pos <= e.buf.len(), "{name} {} at {start}", k.s);
                e.view(12, 5, true);
                e.view(3, 2, true);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin's pick and the optional plugins with nothing in them, every
/// key pressed: none panics.
#[test]
fn empty_sheets_keys() {
    let keys = [
        "up",
        "down",
        "[",
        "]",
        "tab",
        "shift+tab",
        "space",
        "backspace",
        "ctrl+d",
        "x",
        "日",
        "pgup",
        "pgdown",
        "home",
        "end",
        "enter",
    ];
    let (mut m, _) = faked(Fake::default());
    for k in keys {
        m.open_sheet(Box::new(pick::PickSheet::new(
            "p",
            rush_plugin::Pick::default(),
        )));
        press(&mut m, &[k, k]);
        let mut p = pick();
        p.tabs = vec!["a".into(), "b".into(), "c".into()];
        p.actions.clear();
        m.open_sheet(Box::new(pick::PickSheet::new("p", p)));
        press(&mut m, &[k, "ctrl+d", k]);
        m.open_sheet(Box::new(picks::OptionalSheet {
            picks: Vec::new(),
            first: true,
            never: true,
            cur: 0,
            busy: false,
        }));
        press(&mut m, &[k, k]);
        m.open_sheet(Box::new(layout::LayoutSheet::default()));
        press(&mut m, &[k]);
    }
}

/// Optional plugins: ones asked about, running elsewhere or on already
/// aren't offered; the rest come with their category's title
/// (pluginpicks_test.go TestOptionalFound).
#[test]
fn optional_found() {
    let b = |name: &str, optional: bool, os: &[&str]| rush_plugin::Bundle {
        manifest: Manifest {
            name: name.into(),
            category: "machine".into(),
            requires: rush_plugin::Requires {
                os: os.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            ..Manifest::default()
        },
        optional,
        run: None,
        switched: None,
    };
    let bundles = vec![
        b("always", false, &[]),
        b("asked", true, &[]),
        b("fresh", true, &[]),
        b("running", true, &[]),
        b("elsewhere", true, &["plan9"]),
    ];
    let on = BTreeMap::from([("running".to_string(), true)]);
    let got = picks::found(&["asked".into()], &bundles, &on);
    assert_eq!(got.picks.len(), 1, "{:?}", got.picks);
    assert_eq!(got.picks[0].name, "fresh");
    assert_eq!(got.picks[0].category, "Your machine");
    assert_eq!(got.already_on, ["running"]);
}

/// The sheet keeps its height while the cursor moves through rows that
/// wrap to different heights (pluginpicks_test.go
/// TestOptionalSheetKeepsItsHeight).
#[test]
fn optional_keeps_its_height() {
    let long = "a long description that wraps onto more lines ".repeat(6);
    let cats = [
        "Your machine",
        "Your machine",
        "Other tools",
        "Other tools",
        "Other tools",
        "Agents",
        "Agents",
        "Agents",
    ];
    let picks: Vec<picks::OptPick> = cats
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let about = if i == 2 { long.as_str() } else { "short" };
            opt(&((b'a' + i as u8) as char).to_string(), about, c, false)
        })
        .collect();
    for h in [40, 16] {
        let (mut m, _) = faked(Fake::default());
        let mut s = picks::OptionalSheet {
            picks: picks.clone(),
            first: false,
            never: false,
            cur: 0,
            busy: false,
        };
        let want = s.body(&m, 100, h).len();
        for _ in &picks {
            s.key(&mut m, &crate::frame::Press::of("down").unwrap());
            let got = s.body(&m, 100, h).len();
            assert_eq!(
                got, want,
                "h={h}: at row {} the sheet is {got} lines",
                s.cur
            );
        }
    }
}

/// Go's %q undone, enough for testdata/wide/json.tsv; None for bytes
/// that aren't UTF-8.
fn unquote(s: &str) -> Option<String> {
    let s = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'u' => {
                let h: String = it.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            'x' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// The editor's JSON problems in Go's words (jsontext's), at Go's line
/// and column, for every kind of mistake in testdata/wide/json.tsv.
#[test]
fn json_problems_match_go() {
    let mut bad = Vec::new();
    for row in include_str!("testdata/wide/json.tsv").lines() {
        let f: Vec<&str> = row.split('\t').collect();
        let Some(text) = unquote(f[0]) else { continue };
        let got = doc::json_problems(&text);
        let got = match got.first() {
            None => "ok".to_string(),
            Some(d) => format!("{}\t{}\t{}", d.line, d.col, d.msg),
        };
        let want = f[1..].join("\t");
        if got != want {
            bad.push(format!("{}\n got {got}\nwant {want}", f[0]));
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}
