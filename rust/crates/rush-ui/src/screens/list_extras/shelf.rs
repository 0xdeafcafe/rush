//! The shelf: what a new agent can start as, on the Agents box's top
//! edge. The one it will start as is the chip; the others are a word
//! each, ⌥1…9 or a click away, for the next session only: the profile's
//! providers in order, then setups the fleet ran lately. After Go's
//! internal/ui/shelf.go (shelfMax :25, shelf :28, shelfWord :49, setupKey
//! :66, shelfLine :71, shelfPick :120, shelfKey :130, shelfClick :141)
//! and view.go:2744 (promptLines' shelf on the box's top right).

use rush_core::text::Line;

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::accounts::look::model_word;
use crate::screens::accounts::recent::recent_setups;
use crate::screens::accounts::rows::start_profile;
use crate::screens::accounts::setup::{
    StartOver, by_profile, next_start, setup_chip, start_defaults, start_with_words, start_words,
};
use crate::screens::accounts::state::accounts;
use crate::style::*;

/// How many setups the shelf holds: one per ⌥ digit (shelfMax).
pub const SHELF_MAX: usize = 9;

/// A setup on the box's top edge, by screen column (shelfHit).
#[derive(Clone, Debug)]
pub struct Hit {
    pub x0: usize,
    pub x1: usize,
    pub o: StartOver,
}

/// o as words, for telling setups apart (setupKey).
fn setup_key(m: &Model, o: &StartOver) -> String {
    start_words(m, o).join(" · ")
}

/// What a new session in dir can start as, each once (shelf).
pub fn shelf(m: &Model, dir: &str) -> Vec<StartOver> {
    let first = by_profile(m, dir);
    let mut seen = vec![setup_key(m, &first)];
    let mut out = vec![first.clone()];
    let mut add = |o: StartOver| {
        let k = setup_key(m, &o);
        if out.len() < SHELF_MAX && !seen.contains(&k) {
            seen.push(k);
            out.push(o);
        }
    };
    let inst = start_profile(m, dir).installed();
    if inst.len() > 1 {
        for k in &inst[1..] {
            add(start_defaults(m, k));
        }
    }
    for o in recent_setups(m, &first) {
        add(o);
    }
    out
}

/// o as the shelf names it: whose model, in which harness when not its
/// own, which model (shelfWord).
pub fn shelf_word(m: &Model, o: &StartOver) -> String {
    let reg = &m.reg;
    let mut word = reg.provider_label(&reg.provider_of(&o.kind));
    let h = reg.harness_of(&o.kind);
    if h.as_str() != o.kind {
        word.push('·');
        word.push_str(&reg.harness_label(h.as_str()).to_lowercase());
    }
    if o.billing == rush_state::profiles::BILLING_KEY {
        word.push_str("·key");
    }
    if !o.model.is_empty() {
        word.push('·');
        word.push_str(&model_word(reg, &o.kind, &o.model));
    }
    word
}

/// The shelf as the box's edge shows it, at most w wide: the next
/// session's setup as a chip, the others numbered; its hits from the
/// line's left edge (shelfLine).
pub fn shelf_line(m: &Model, dir: &str, w: usize) -> (Line, Vec<Hit>) {
    let cur = next_start(m, dir);
    let tiles = shelf(m, dir);
    let chip = setup_chip(m, &cur);
    let cur_key = setup_key(m, &cur);
    let on = tiles.iter().position(|o| setup_key(m, o) == cur_key);
    // Each tile's text, the chosen one as the chip; one off the shelf
    // (from the start sheet, or #new) goes first, unnumbered.
    let mut segs: Vec<(Line, StartOver, bool)> = Vec::new();
    if on.is_none() {
        segs.push((chip.clone(), cur.clone(), true));
    }
    for (i, o) in tiles.into_iter().enumerate() {
        if Some(i) == on {
            segs.push((chip.clone(), o, true));
        } else {
            let l = line([
                sp(format!("⌥{}", i + 1), DIM),
                plain(" "),
                sp(shelf_word(m, &o), TEXT),
            ]);
            segs.push((l, o, false));
        }
    }
    // Too wide: the others go from the end, the chosen one stays.
    let width = |segs: &[(Line, StartOver, bool)]| {
        segs.iter()
            .map(|s| s.0.width() + 1)
            .sum::<usize>()
            .saturating_sub(1)
    };
    while segs.len() > 1 && width(&segs) > w {
        let mut at = segs.len() - 1;
        if segs[at].2 {
            at -= 1;
        }
        segs.remove(at);
    }
    let mut out = Line::new();
    let mut hits = Vec::new();
    for (i, (l, o, _)) in segs.into_iter().enumerate() {
        if i > 0 {
            out.spans.push(plain(" "));
        }
        let x0 = out.width();
        join(&mut out, l);
        hits.push(Hit {
            x0,
            x1: out.width(),
            o,
        });
    }
    (out, hits)
}

/// Starts the next session as o (shelfPick).
pub fn pick(m: &mut Model, o: StartOver) {
    let dir = super::prompt::start_dir(m);
    if setup_key(m, &o) == setup_key(m, &by_profile(m, &dir)) {
        accounts(m).start_over = None; // the profile's own again
    } else {
        accounts(m).start_over = Some(o);
    }
    let w = start_with_words(m, &dir);
    m.flash(&format!("the next session starts as {w}"));
}

/// ⌥1…9 in the list with the box a new prompt: the shelf's setup of that
/// number (shelfKey).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    let Some(d) = s.strip_prefix("alt+").and_then(|d| d.parse::<usize>().ok()) else {
        return Out::Pass;
    };
    if !(1..=9).contains(&d) || m.session.is_some() || m.on_place() {
        return Out::Pass;
    }
    let dir = super::prompt::start_dir(m);
    let tiles = shelf(m, &dir);
    if let Some(o) = tiles.into_iter().nth(d - 1) {
        pick(m, o);
    }
    Out::Draw
}
