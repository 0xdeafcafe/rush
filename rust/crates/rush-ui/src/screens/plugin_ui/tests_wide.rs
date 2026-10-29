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
fn diffs(name: &str, sets: &[Vec<Line>], txt: &str, sty: &str, skip: fn(&str) -> bool) -> Vec<String> {
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
    assert!(all.is_empty(), "{} rows differ:\n{}", all.len(), all.join("\n"));
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
