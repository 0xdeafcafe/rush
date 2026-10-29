//! Long pastes folded into chips in both message boxes, and what goes out:
//! after Go's internal/ui/pastes.go and pastechip_test.go, model.go:
//! 1435-1469 (a paste into either box, space on a lit chip), keys.go:771
//! (the Agents box's submit), rushmode.go:4646 (the Session's send) and
//! pastes.go:251 (ctrl+g: the last paste, else the whole draft). No sleeps:
//! $EDITOR runs at Model::run_outside, on files in a temp dir.

use rush_draw::{Button, KeyMods, MouseKind};

use super::conn;
use super::host::Op;
use super::tests::{KEY, hosted, press, typed};
use crate::frame::Mouse;
use crate::model::Model;
use crate::screens::session_cards::pastes::chip_for;

const LOG: &str = "panic: nil map\ngoroutine 1 [running]:\nmain.main()\n";

/// Where text is on screen, as a click would land on its first cell.
fn at(m: &mut Model, text: &str) -> (u16, u16) {
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    f.iter()
        .enumerate()
        .find_map(|(y, l)| {
            l.find(text)
                .map(|x| (rush_core::text::width(&l[..x]) as u16 + 1, y as u16))
        })
        .unwrap_or_else(|| panic!("no {text:?} on screen:\n{}", f.join("\n")))
}

fn mouse(m: &mut Model, kind: MouseKind, (x, y): (u16, u16)) {
    m.mouse_event(&Mouse {
        kind,
        x,
        y,
        mods: KeyMods::NONE,
    });
}

/// The text a tagged paste goes out as, its id made the same.
fn untag(s: &str) -> String {
    let re = regex::Regex::new(r#"id="[0-9a-f]{4}""#).unwrap();
    re.replace_all(s, r#"id="0000""#).into_owned()
}

/// A long paste in a Session's box is a chip; enter sends it whole, between
/// the tags Claude Code sends a paste in, and the box lets it go.
#[test]
fn a_long_paste_is_a_chip_and_goes_out_whole() {
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "why");
    press(&mut m, "space");
    m.paste(LOG);
    let chip = chip_for(1, LOG);
    assert_eq!(conn(&mut m, KEY).input.string(), format!("why {chip}"));
    // A short paste keeps its line as it is.
    m.paste(" now?");
    fake.ops.lock().unwrap().clear();
    press(&mut m, "enter");
    let ops = fake.ops();
    let Some(Op::Send { text, .. }) = ops.first() else {
        panic!("sent nothing: {ops:?}");
    };
    assert_eq!(
        untag(text),
        "why \n\n<pasted_content id=\"0000\">\npanic: nil map\ngoroutine 1 [running]:\nmain.main()\n</pasted_content id=\"0000\">\n now?"
    );
    let c = conn(&mut m, KEY);
    assert!(c.input.is_empty() && c.input.pastes.text.is_empty());
}

/// ctrl+g edits the last paste in the draft, its chip following what it
/// says now; with none, the whole draft.
#[test]
fn ctrl_g_edits_the_last_paste() {
    let (mut m, _) = hosted("steer", Vec::new());
    m.paste("one\ntwo\n");
    typed(&mut m, "and");
    m.paste(LOG);
    m.set_editor_program("f() { printf 'more\\n' >> \"$1\"; }; f");
    press(&mut m, "ctrl+g");
    assert_eq!(m.run_outside(), 1);
    let edited = format!("{LOG}more");
    let c = conn(&mut m, KEY);
    assert_eq!(
        c.input.string(),
        format!("{}and{}", chip_for(1, "one\ntwo\n"), chip_for(2, &edited))
    );
    assert_eq!(c.input.pastes.text[&2], edited);
    assert_eq!(c.input.sent(false), format!("one\ntwo\nand{edited}"));
}

/// A click on a chip opens its paste in place; the pointer over one lights
/// it, and space then opens it; undo folds it again.
#[test]
fn a_chip_opens_in_place() {
    let (mut m, _) = hosted("steer", Vec::new());
    m.paste(LOG);
    let chip = chip_for(1, LOG);
    let p = at(&mut m, &chip[..6]);
    mouse(&mut m, MouseKind::Moved, p);
    let lit = conn(&mut m, KEY).input.hot;
    assert_eq!((lit.from, lit.to), (0, chip.chars().count()), "lit");
    press(&mut m, "space");
    assert_eq!(conn(&mut m, KEY).input.string(), LOG);
    press(&mut m, "ctrl+_");
    assert_eq!(conn(&mut m, KEY).input.string(), chip, "folded again");
    let p = at(&mut m, &chip[..6]);
    mouse(&mut m, MouseKind::Down(Button::Left), p);
    assert_eq!(conn(&mut m, KEY).input.string(), LOG);
}

/// The Agents box folds a long paste the same way; a short one is made one
/// line. What a # command is handed has the paste back in place, and
/// ctrl+g there edits the paste.
#[test]
fn the_agents_box_folds_pastes() {
    let mut m = crate::screens::social::feed::tests::model(120, 30);
    m.paste("  fix   the\n");
    assert_eq!(m.input.string(), "fix the");
    m.input.clear();
    m.paste(LOG);
    assert_eq!(m.input.string(), chip_for(1, LOG));
    assert_eq!(m.input.sent(false), LOG);
    m.set_editor_program("f() { printf 'x\\ny\\n' > \"$1\"; }; f");
    press(&mut m, "ctrl+g");
    assert_eq!(m.run_outside(), 1);
    assert_eq!(m.input.string(), chip_for(1, "x\ny"));
    assert_eq!(m.input.sent(false), "x\ny");
}
