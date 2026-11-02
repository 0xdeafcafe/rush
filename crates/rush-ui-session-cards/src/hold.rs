//! A decisions block that comes while you're typing waits: its card isn't
//! put over the box and doesn't take its keys. The box's bottom edge says
//! it's there instead ("◆ 1 decision waiting · alt+j answer · alt+shift+j
//! stash and answer"), counting each that comes.
//!
//! Typing is the box holding text with a key pressed in the last
//! [`HOLD_MS`]. Once you stop with the box empty (you sent it, or
//! cleared it), the card shows as ever. While what you typed is still
//! there, the notice stays: the card shares the box (your own answer is
//! typed there, and on the card letters and digits answer), so it waits
//! to be asked for. alt+j opens it now with your text left in the box;
//! alt+shift+j sets your text aside, box, cursor and undo alike, opens
//! it, and puts the text back once the decision is answered or set aside.
//!
//! New in the Rust port: Go's cardModal (modal.go:32) only kept the keys
//! off a card that came mid-word.

use std::time::{Duration, Instant};

use rush_core::text::Line;

use super::{conn, info_any, now, open_key, question};
use crate::ModelCards;
use crate::frame::{Editor, Out, Press};
use crate::model::Model;
use crate::style::*;

/// How long after a key in the box you count as still typing.
pub const HOLD_MS: rush_core::time::Ms = 3_000;

/// What a session's box holds back.
#[derive(Clone, Debug, Default)]
pub struct Hold {
    /// The decisions cards that came while you were typing, oldest first:
    /// none is drawn while any is here.
    pub ids: Vec<String>,
    /// The decisions card last seen coming, held or not.
    pub seen: String,
    /// What the box held when alt+shift+j set it aside for the decisions
    /// card with this id: it comes back once that card is answered or
    /// set aside.
    pub stash: Option<(String, Editor)>,
}

/// The keymap's actions: open the decision now, or set what's typed
/// aside and open it.
pub const ANSWER: &str = "session.decide";
pub const STASH_ANSWER: &str = "session.decide.stash";

/// Holds a decisions card that comes while agent key's box is being typed
/// in, lets it go once you've stopped with the box empty, and puts back
/// what was set aside once its card has gone. Run before the card is
/// worked out, each frame and each key.
pub fn sync(m: &mut Model, key: &str) {
    let Some(i) = info_any(m, key) else {
        return;
    };
    let now = now(m);
    let idle = now.saturating_sub(m.cards().last_key);
    let typed = !i.input.is_empty();
    let c = conn(m, key);
    // An approval waiting doesn't make it a new one when it's answered.
    let waiting = question::decisions_waiting(c, &i, 0)
        .map(|d| d.id)
        .unwrap_or_default();
    if waiting.is_empty() {
        c.hold.ids.clear();
        c.hold.seen.clear();
    } else if waiting != c.hold.seen {
        c.hold.seen.clone_from(&waiting);
        // Another while one is held waits with it, and counts.
        if (typed && idle < HOLD_MS) || !c.hold.ids.is_empty() {
            c.hold.ids.push(waiting.clone());
        }
    }
    let mut wake = None;
    if !c.hold.ids.is_empty() && !typed {
        if idle >= HOLD_MS {
            c.hold.ids.clear();
        } else {
            wake = Some(HOLD_MS - idle);
        }
    }
    let back = c
        .hold
        .stash
        .take_if(|(id, _)| *id != waiting)
        .map(|(_, e)| e);
    if let Some(ms) = wake {
        m.ask_wake(Instant::now() + Duration::from_millis(ms.max(0) as u64));
    }
    if let Some(e) = back {
        put_back(m, key, e);
    }
}

/// What was set aside comes back: in place of an empty box as it was,
/// cursor and all; before anything typed since, so nothing is lost.
fn put_back(m: &mut Model, key: &str, e: Editor) {
    let c = crate::sess::conn(m, key);
    if c.input.is_empty() {
        c.input = e;
        return;
    }
    let (pos, mut text) = (e.pos, e.text);
    text.push('\n');
    text.extend(c.input.text.iter().copied());
    c.input.undo.save(&c.input.text, c.input.pos, false);
    c.input.text = text;
    c.input.pos = pos;
    c.input.anchor = None;
}

/// How many decisions are held off agent key's box.
pub fn held(m: &Model, key: &str) -> usize {
    super::conn_ref(m, key).map_or(0, |c| c.hold.ids.len())
}

/// Whether what agent key's box held is set aside for a decision.
pub fn stashed(m: &Model, key: &str) -> bool {
    super::conn_ref(m, key).is_some_and(|c| c.hold.stash.is_some())
}

/// What agent key's box says on its bottom edge of a decision held or
/// its text set aside; empty for neither.
pub fn notice(m: &mut Model, key: &str) -> Line {
    if stashed(m, key) {
        return line([
            sp("stashed", ORANGE),
            sp(" · comes back after the decision", DIM),
        ]);
    }
    let n = held(m, key);
    if n == 0 {
        return Line::new();
    }
    let answer = crate::keybind::bound_key(m, ANSWER);
    let stash = crate::keybind::bound_key(m, STASH_ANSWER);
    let what = if n == 1 { "decision" } else { "decisions" };
    let mut l = line([sp(format!("◆ {n} {what} waiting"), YELLOW)]);
    for (k, w) in [(answer, "answer"), (stash, "stash and answer")] {
        if !k.is_empty() {
            l.spans.push(sp(format!(" · {k} {w}"), DIM));
        }
    }
    l
}

/// alt+j and alt+shift+j on the open session; every key first lets the
/// hold see what's typed (a session_keys hook).
pub fn hold_key_hook(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    sync(m, &key);
    let stash = match k.s.as_str() {
        "alt+j" => false,
        "alt+shift+j" | "alt+J" => true,
        _ => return Out::Pass,
    };
    open(m, &key, stash)
}

/// Opens agent key's decisions card now, the keys on it: what's typed
/// stays in the box, or with stash is set aside until the card has gone.
/// Pass when no decision waits.
pub fn open(m: &mut Model, key: &str, stash: bool) -> Out {
    let Some(i) = info_any(m, key) else {
        return Out::Pass;
    };
    let c = conn(m, key);
    let Some(d) = question::decisions_waiting(c, &i, 0) else {
        return Out::Pass;
    };
    c.hold.ids.clear();
    c.card_focus = true;
    if stash && !i.input.is_empty() && c.hold.stash.is_none() {
        let b = &mut crate::sess::conn(m, key).input;
        let kept = b.clone();
        b.undo.save(&b.text, b.pos, false);
        b.clear();
        conn(m, key).hold.stash = Some((d.id, kept));
    }
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
        v.sel.clear();
    }
    Out::Draw
}
