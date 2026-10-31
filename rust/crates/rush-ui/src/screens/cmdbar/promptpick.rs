//! The Agents box's picker: rush's # commands, a command's values and @
//! mentions offered over the box as they're typed, ↑↓ to choose, tab to
//! complete, enter to run. Port of internal/ui/fleetslash.go:236-305
//! (promptPicker :236, fleetSlashLines :256, fleetSlashKey :282), the
//! picker over Getting started (view.go:1100 pickerOverCard, :1140-1146)
//! and over the box (view.go:2764-2767), and the selection going back to
//! the first row as the text changes (keys.go:546-548). A new session's
//! / commands (newSessionCommands) aren't offered yet: rush doesn't read
//! them for a session not started.

use rush_core::event::Command;
use rush_core::text::Line;

use super::fleetslash::{completed, hash_matches, needs_arg, picker_move, picker_rows};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::session_cards::mention::{complete_mention, mention_matches, mention_rows};

/// The picker's row, and what was typed when it was last looked at: the
/// row goes back to the first as the text changes.
#[derive(Default)]
struct PickState {
    sel: usize,
    typed: String,
    /// Getting started took the list's foot last frame: the picker goes
    /// over it, not over the box (cardShown).
    card_shown: bool,
}

/// Whether the Agents box has the keys for its picker: no session open
/// or beside it with the keys, nothing over the screen, not renaming
/// (promptPicker's inKind, sessionFocused, acceptsText).
fn open(m: &Model) -> bool {
    m.ext.agents_box
        && m.session.is_none()
        && m.sheet.is_none()
        && m.confirm.is_none()
        && !m.help
        && !m.on_place()
        && !m.keys_beside
        && !crate::screens::list_extras::live::embedded(m)
        && crate::screens::list_extras::rename::renaming(m).is_none()
}

/// What the picker offers and the prefix its picks take: # for rush's
/// commands, @ for an agent (promptPicker).
fn picker(m: &Model) -> (Vec<Command>, &'static str) {
    if !open(m) {
        return (Vec::new(), "");
    }
    let input = &m.input.text;
    let back = input.len() - m.input.pos.min(input.len());
    let cmds = hash_matches(m, input, back, &[]);
    if !cmds.is_empty() {
        return (cmds, "#");
    }
    let names = mention_matches(m, input, back);
    if !names.is_empty() {
        let cmds = names
            .into_iter()
            .map(|name| Command {
                name,
                ..Command::default()
            })
            .collect();
        return (cmds, "@");
    }
    (Vec::new(), "")
}

/// The picker's row, kept in range, back on the first once the text
/// changed.
fn sel(m: &mut Model, n: usize) -> usize {
    let typed = m.input.string();
    let st = m.kept::<PickState>();
    if st.typed != typed {
        st.typed = typed;
        st.sel = 0;
    }
    st.sel = st.sel.min(n.saturating_sub(1));
    st.sel
}

/// The picker's rows, w wide (fleetSlashLines).
fn lines(m: &mut Model, w: usize) -> Vec<Line> {
    let (cmds, lead) = picker(m);
    if cmds.is_empty() {
        return Vec::new();
    }
    let sel = sel(m, cmds.len());
    if lead == "@" {
        let input = m.input.text.clone();
        let back = input.len() - m.input.pos.min(input.len());
        return mention_rows(m, &input, back, sel, w);
    }
    picker_rows(
        &cmds,
        sel,
        w,
        "#",
        |_| None,
        "↑↓ · tab completes · enter runs",
    )
}

/// The picker over the Agents box: a list_dock before the box's own,
/// unless it went over Getting started (view.go:2764).
pub(super) fn dock(m: &mut Model, w: usize) -> Vec<Line> {
    if m.kept::<PickState>().card_shown {
        return Vec::new();
    }
    lines(m, w)
}

/// Getting started at the list's foot, the picker over its last rows
/// while one's typed, so the list doesn't jump as it opens
/// (view.go:1134-1146): a list_foot before the card's own.
pub(super) fn foot(m: &mut Model, w: usize, body_h: usize) -> Vec<(Line, String)> {
    let card = crate::screens::accounts::onboard::foot(m, w, body_h);
    let was = std::mem::replace(&mut m.kept::<PickState>().card_shown, !card.is_empty());
    if card.is_empty() || !was {
        return card;
    }
    let pick = lines(m, w);
    if pick.is_empty() {
        return card;
    }
    let blank = card.len().saturating_sub(pick.len());
    std::iter::repeat_n(Line::new(), blank)
        .chain(pick)
        .map(|l| (l, String::new()))
        .collect()
}

/// The picker's keys while a command or a tag is typed in the Agents
/// box: ↑↓ choose, tab completes, enter runs a command that needs
/// nothing more (fleetSlashKey). A list_keys hook, before the list's.
pub(super) fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    if !matches!(s, "up" | "down" | "tab" | "enter") {
        return Out::Pass;
    }
    let (cmds, lead) = picker(m);
    if cmds.is_empty() {
        return Out::Pass;
    }
    let at = sel(m, cmds.len());
    let pick = cmds[at].clone();
    match s {
        "up" | "down" => {
            m.kept::<PickState>().sel = picker_move(at, cmds.len(), s);
            Out::Draw
        }
        _ if lead == "@" => {
            let back = m.input.text.len() - m.input.pos.min(m.input.text.len());
            let (text, back) = complete_mention(&m.input.text, back, &pick.name);
            m.input.pos = text.len() - back;
            m.input.text = text;
            m.input.anchor = None;
            Out::Draw
        }
        "tab" => {
            set(m, completed(lead, &pick, true));
            Out::Draw
        }
        _ => {
            set(m, completed(lead, &pick, false));
            if needs_arg(&pick) {
                return Out::Draw;
            }
            m.submit()
        }
    }
}

/// The box's text set to text, the cursor at its end, the picker on its
/// first row.
fn set(m: &mut Model, text: Vec<char>) {
    m.input.pos = text.len();
    m.input.text = text;
    m.input.anchor = None;
    let typed = m.input.string();
    let st = m.kept::<PickState>();
    st.sel = 0;
    st.typed = typed;
}
