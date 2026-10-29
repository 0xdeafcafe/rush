//! The sheets' and the box's pieces' tests, after Go's internal/ui
//! (slimsheet_test.go, imagerefs_test.go, pastechip_test.go, and the
//! other sheets' and panels' tests). No sleeps: a model with no loop runs
//! handed-off work at once.

use super::enter::*;
use super::imagerefs::*;
use super::pastes::*;
use super::slim::*;
use super::tests::*;
use super::*;

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// The open sheet's body, as text, w wide.
pub(super) fn sheet_text(m: &mut Model, w: usize, h: usize) -> String {
    let mut s = m.sheet.take().expect("a sheet is open");
    let out = text(&s.body(m, w, h))
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    m.sheet = Some(s);
    out
}

/// Enter asks once what it should do; the answer is saved and done.
#[test]
fn enter_sheet() {
    let mut m = hosted(120, 40);
    ask_enter(&mut m, "k");
    let out = sheet_text(&mut m, 60, 20);
    assert_eq!(
        out,
        "Enter on an agent  what should it do?\n\n▍ Rename it  as in the Finder · ⌘↓ or tab opens it\n  Open it  into its Session · ctrl+r renames it\n\nSettings › General changes it\n\n↑↓ choose  ·  enter pick  ·  esc cancel"
    );
    press(&mut m, "down");
    press(&mut m, "enter");
    assert!(m.sheet.is_none());
    assert_eq!(m.kept::<Fake>().settings["enterOn"], "open");
    assert_eq!(acts(&mut m), vec![Act::EnterOn(true)]);
}

/// Every unused thing is ticked to drop; unticked, it stays; what the
/// session went without already stays gone (TestSlimDrops).
#[test]
fn slim_drops() {
    let mut m = hosted(120, 40);
    set_info(&mut m, |i| {
        i.without = vec!["mcp__old".into()];
        i.unused = Some(vec![
            Unused {
                what: "MCP server".into(),
                name: "playwright".into(),
                tokens: 1600,
                rules: vec!["mcp__playwright".into()],
            },
            Unused {
                what: "skill".into(),
                name: "pdf".into(),
                tokens: 120,
                rules: vec!["Skill(pdf)".into()],
            },
        ]);
        i.conv_tokens = Some((52_000, 30_000));
    });
    slim::open_slim(&mut m, "k");
    let drops = |m: &mut Model| m.sheet_as::<SlimSheet>().unwrap().drops();
    assert_eq!(
        drops(&mut m),
        (
            vec![
                "mcp__old".into(),
                "mcp__playwright".into(),
                "Skill(pdf)".into()
            ],
            1720
        )
    );
    let out = sheet_text(&mut m, 80, 30);
    assert!(out.contains("▍ [✕] MCP server playwright"), "{out}");
    assert!(out.contains("≈ 2k tokens less in every request"), "{out}");
    assert!(
        out.contains("it goes without 1 already · r puts everything back"),
        "{out}"
    );
    press(&mut m, "space"); // keep playwright
    assert_eq!(
        drops(&mut m),
        (vec!["mcp__old".into(), "Skill(pdf)".into()], 120)
    );
    press(&mut m, "a"); // one dropped: keep them all
    assert_eq!(drops(&mut m).1, 0);
    press(&mut m, "a");
    assert_eq!(drops(&mut m).1, 1720);
    press(&mut m, "enter");
    assert_eq!(
        acts(&mut m),
        vec![Act::Without(Some(vec![
            "mcp__old".into(),
            "mcp__playwright".into(),
            "Skill(pdf)".into()
        ]))]
    );
}

/// Without a breakdown yet #slim says when it comes.
#[test]
fn slim_waits_for_a_breakdown() {
    let mut m = hosted(120, 40);
    slim::open_slim(&mut m, "k");
    assert!(m.sheet.is_none());
}

/// A pasted image goes in at the cursor, spaced off the words
/// (TestImageGoesInAtTheCursor), and markers resolve in the order they
/// appear (TestImageMarkersResolve).
#[test]
fn image_markers() {
    let mut r = ImageRefs::default();
    let buf = chars("look here please");
    let got: String = with_marks(&buf, 9, &mut r, &["/a.png".into()])
        .into_iter()
        .collect();
    assert_eq!(got, "look here [Image #1] please");
    assert_eq!(r.path[&1], "/a.png");

    let mut r = ImageRefs::default();
    let (one, _two, three) = (r.add("/one.png"), r.add("/two.png"), r.add("/three.png"));
    let text = format!("first {three} then {one} and [Image #9]");
    let (got, images) = r.resolve(&text);
    assert_eq!(got, "first [Image #1] then [Image #2] and [Image #9]");
    assert_eq!(images, vec!["/three.png".to_string(), "/one.png".into()]);
    assert_eq!(r.paths(&one), "[image: /one.png]");
    assert!(is_marker("[Image #3]") && !is_marker("x [Image #3]"));
}

/// A dropped or typed path to an image becomes its marker where it was
/// (TestImagePathsInline).
#[test]
fn image_paths_inline() {
    let dir = std::env::temp_dir().join(format!("rush-imgrefs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("shot one.png");
    std::fs::write(&p, "x").unwrap();
    let p = p.to_string_lossy().into_owned();
    let mut r = ImageRefs::default();
    let got = r.inline(
        &format!("before '{p}' after"),
        &crate::frame::editor::look_disk,
    );
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got.as_deref(), Some("before [Image #1] after"));
    assert_eq!(r.path[&1], p);
}

/// An image in the box is drawn by its name; each drawn place maps back
/// into its marker (TestImageChipNames).
#[test]
fn image_chip_names() {
    let mut r = ImageRefs::default();
    let a = r.add("/Users/me/rush-icon-v2-c.png");
    let buf = chars(&format!("{a} x"));
    let (shown, at) = r.shown(&buf);
    let shown: String = shown.into_iter().collect();
    assert_eq!(shown, "[▣ rush-icon-v2-c.png] x");
    assert_eq!(at[0], 0);
    assert_eq!(at[5], 1, "inside the name is inside the marker");
    assert_eq!(*at.last().unwrap(), buf.len());
    assert_eq!(
        chip_name("/a/abcdefghijklmnopqrstuvwxyz0123456789.png")
            .chars()
            .count(),
        32
    );
}

const PASTED: &str = "one\ntwo\nthree\nfour";

/// A paste is kept as its chip; expanded it's the text again, tagged as
/// Claude Code sends a paste; a chip opens in place (pastes.go).
#[test]
fn paste_chips() {
    assert!(is_long_paste("a\nb") && !is_long_paste("a\n") && is_long_paste(&"x".repeat(801)));
    let mut p = Pastes::default();
    let chip = p.add(PASTED);
    assert_eq!(chip, "[pasted text #1 · 4 lines: one two three four]");
    let draft = format!("see {chip} please");
    assert_eq!(p.expand(&draft, None), format!("see {PASTED} please"));
    assert_eq!(
        p.expand(&draft, Some(0xbeef)),
        format!(
            "see \n\n<pasted_content id=\"beef\">\n{PASTED}\n</pasted_content id=\"beef\">\n please"
        )
    );
    let buf = chars(&draft);
    assert_eq!(p.last_in(&buf), 1);
    let (sp, ok) = p.chip_at(&buf, 5);
    assert!(ok && sp.from == 4 && sp.to == 4 + chip.chars().count());
    let (opened, pos) = p.open(&buf, sp).unwrap();
    assert_eq!(
        opened.iter().collect::<String>(),
        format!("see {PASTED} please")
    );
    assert_eq!(pos, 4 + PASTED.chars().count());
    assert!(!p.chip_at(&buf, 1).1, "off the chip");

    // A sent message back in a box: each tagged paste a chip again.
    let mut q = Pastes::default();
    let back: String = q
        .unfold("look <pasted_content id=\"0001\">\nx\ny\n</pasted_content id=\"0001\"> ok")
        .into_iter()
        .collect();
    assert_eq!(back, "look [pasted text #1 · 2 lines: x y] ok");

    // What the editor sends back replaces its paste and its chip.
    let edited = p.apply_edit(&buf, 1, "a\nb");
    assert_eq!(
        edited.iter().collect::<String>(),
        "see [pasted text #1 · 2 lines: a b] please"
    );
    assert_eq!(p.to_edit(&edited), ("a\nb".to_string(), 1));
}
