//! Getting started: a card at the foot of the list that ticks off as you
//! do each thing, and a one-time tip the first time something happens
//! that a key helps with. After Go's internal/ui/onboard.go (steps :17,
//! activeSteps :32, didStep :41, stepsDone :57, showCard :68,
//! startedLines :73, tips :99, noteProgress :125), view.go:1130 (the card
//! at the list's foot while it keeps 8 rows) and keys.go:974 (#tips).
//!
//! Go ticks a step where the thing is done (startRename, markDone, …);
//! here the keys that do them are noted as they go by ([`saw_key`]) and
//! the states (a Session open, the keys' help) on each key, reading and
//! frame, so no other screen has to call in.

use rush_core::text::Line;
use rush_fleet::Predicates;

use super::state::{accounts, cfg, cfg_mut, ensure_store, peek, save};
use crate::frame::mackeys::in_apple_terminal;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// A Getting started step: its id in config.json, what it says, the key.
pub struct Step {
    pub id: &'static str,
    pub what: &'static str,
    pub key: &'static str,
}

/// Getting started: each is ticked off when you do it (steps).
pub const STEPS: [Step; 9] = [
    Step {
        id: "start",
        what: "Start an agent",
        key: "enter",
    },
    Step {
        id: "session",
        what: "Open its Session",
        key: "{ }",
    },
    Step {
        id: "hash",
        what: "Run a command",
        key: "#",
    },
    Step {
        id: "next",
        what: "Jump to who needs you",
        key: "ctrl+n",
    },
    Step {
        id: "rename",
        what: "Rename an agent",
        key: "ctrl+r",
    },
    Step {
        id: "pin",
        what: "Pin one to the top",
        key: "#pin",
    },
    Step {
        id: "done",
        what: "Put one away",
        key: "#done",
    },
    Step {
        id: "keys",
        what: "See all the keys",
        key: "?",
    },
    Step {
        id: "mackeys",
        what: "Give Terminal.app the ⌘ keys",
        key: "#mackeys on",
    },
];

/// The steps that apply here: the ⌘ keys only in Terminal.app, which
/// keeps them for itself (activeSteps).
pub fn active_steps(apple_terminal: bool) -> Vec<&'static Step> {
    STEPS
        .iter()
        .filter(|s| apple_terminal || s.id != "mackeys")
        .collect()
}

fn active() -> Vec<&'static Step> {
    active_steps(in_apple_terminal())
}

/// Getting started and the tips are on: not in a view another app hosts
/// (m.onboard, false in NewHosted).
fn on(m: &Model) -> bool {
    !peek(m).hosted
}

/// Ticks off a Getting started step (didStep).
pub fn did_step(m: &mut Model, id: &str) {
    // Getting started's progress is the leading rush's to note in
    // config.json.
    if !on(m) || !m.leads() {
        return;
    }
    ensure_store(m);
    let o = &mut cfg_mut(m).onboarding;
    if o.steps.iter().any(|s| s == id) {
        return;
    }
    o.steps.push(id.to_string());
    let hidden = o.hidden;
    save(m);
    if steps_done(m) == active().len() && !hidden {
        m.flash("✓ all set · ? for keys");
    }
}

/// How many of the steps that apply are done (stepsDone).
pub fn steps_done(m: &Model) -> usize {
    let done = &cfg(m).onboarding.steps;
    active()
        .iter()
        .filter(|s| done.iter().any(|d| d == s.id))
        .count()
}

/// Getting started is under the list: until every step is done, or you
/// put it away with #tips off (showCard).
pub fn show_card(m: &Model) -> bool {
    on(m) && !cfg(m).onboarding.hidden && steps_done(m) < active().len()
}

/// Getting started, w wide, with a blank line above (startedLines).
pub fn started_lines(done: &[String], steps: &[&Step], w: usize) -> Vec<Line> {
    let w = w.saturating_sub(4); // the keys end where the box's top-right label does
    let n = steps
        .iter()
        .filter(|s| done.iter().any(|d| d == s.id))
        .count();
    let mut out = vec![
        Line::new(),
        line([
            plain(" "),
            spb("✦ Getting started", ORANGE),
            sp(format!("  {n}/{}", steps.len()), FAINT),
        ]),
    ];
    for s in steps {
        if done.iter().any(|d| d == s.id) {
            out.push(line([plain(" "), sp("✓ ", GREEN), sp(s.what, FAINT)]));
            continue;
        }
        let gap = w as isize
            - rush_core::text::width(s.what) as isize
            - rush_core::text::width(s.key) as isize
            - 2;
        let mut l = line([plain(" "), sp("○ ", DIM), sp(s.what, TEXT)]);
        if gap >= 2 {
            l.spans.push(plain(blanks(gap as usize)));
            l.spans.push(spb(s.key, ORANGE));
        }
        out.push(l);
    }
    // # is the way in to most of rush: say so, and how this card goes.
    let hint = |long: bool| {
        line([
            plain(" "),
            spb("#", ORANGE),
            sp(
                if long {
                    " for commands: pin, done, group, sort… · "
                } else {
                    " for commands · "
                },
                FAINT,
            ),
            spb("#tips off", ORANGE),
            sp(" hides this", FAINT),
        ])
    };
    let mut h = hint(true);
    if h.width() > w + 3 {
        h = hint(false);
    }
    out.push(Line::new());
    out.push(if h.width() > w + 3 { fit(h, w + 3) } else { h });
    out
}

/// The card at the list's foot (Ext::list_foot), left out when the list's
/// body_h rows would keep fewer than 8 (view.go:1133). Only once rush's
/// config is loaded (off the UI thread, on the first reading): a model
/// with no loop (tests, `--render`) never reads or writes the real one
/// for it.
pub fn foot(m: &mut Model, w: usize, body_h: usize) -> Vec<(Line, String)> {
    note_state(m);
    if peek(m).store.is_none() || !show_card(m) {
        return Vec::new();
    }
    let steps = active();
    let card = started_lines(&cfg(m).onboarding.steps, &steps, w);
    if body_h.saturating_sub(card.len()) < 8 {
        return Vec::new();
    }
    card.into_iter().map(|l| (l, String::new())).collect()
}

/// The steps that are a state rather than a key: in a Session, the keys'
/// help open (noteProgress, openKeyHelp).
fn note_state(m: &mut Model) {
    if !on(m) || peek(m).store.is_none() {
        return;
    }
    if m.session.is_some() && m.focused().is_some() {
        did_step(m, "session");
    }
    if m.help {
        did_step(m, "keys");
    }
}

/// Every key, before anything has it: the steps a key does (startRename,
/// jumpNext, command, submit's "start").
pub fn saw_key(m: &mut Model, k: &Press) {
    if !on(m) || peek(m).store.is_none() {
        return;
    }
    note_state(m);
    let free = m.sheet.is_none() && m.confirm.is_none() && !m.on_place() && !m.help;
    if !free {
        return;
    }
    let in_list = m.session.is_none();
    match k.s.as_str() {
        "ctrl+r" if in_list && m.focused().is_some() => did_step(m, "rename"),
        "ctrl+n" if m.tally().blocked > 0 => did_step(m, "next"),
        "enter" if in_list => {
            let text = m.input.string();
            let t = text.trim();
            if t.is_empty() {
                return;
            }
            let Some(cmd) = t.strip_prefix('#') else {
                if !t.starts_with('/') {
                    did_step(m, "start");
                }
                return;
            };
            let mut words = cmd.split_whitespace();
            let name = words.next().unwrap_or("").to_lowercase();
            let arg = words.next().unwrap_or("");
            did_step(m, "hash");
            match name.as_str() {
                "pin" | "done" if m.focused().is_some() => did_step(m, &name),
                "mackeys" if !arg.is_empty() => did_step(m, "mackeys"),
                _ => {}
            }
        }
        _ => {}
    }
}

/// A tip: shown once, the first time it'd help (tips).
struct Tip {
    id: &'static str,
    text: &'static str,
    when: fn(&Model) -> bool,
}

const TIPS: [Tip; 4] = [
    Tip {
        id: "needs",
        text: "● needs you · ctrl+n to go there",
        when: |m| m.tally().blocked > 0 && m.focused().is_none_or(|i| !m.agents[i].needs_you()),
    },
    Tip {
        id: "session",
        text: "[ ] to switch view · esc to go back",
        when: |m| m.session.is_some(),
    },
    Tip {
        id: "finished",
        text: "✓ finished · #done to put it away",
        when: |m| {
            m.focused()
                .is_some_and(|i| m.agents[i].just_finished(m.now))
        },
    },
    Tip {
        id: "hash",
        text: "enter to run it on the picked agent",
        when: |m| m.session.is_none() && m.input.string().starts_with('#'),
    },
];

/// After each reading of the list (Go's tick): the states that are steps,
/// then the first tip that's due, when nothing else is being said
/// (noteProgress).
pub fn note_progress(m: &mut Model) {
    // Tips are shown, and noted in config.json, by the rush that leads.
    if !on(m) || peek(m).store.is_none() || !m.leads() {
        return;
    }
    note_state(m);
    let o = &cfg(m).onboarding;
    // Every tip seen: one rush no longer gives may still be in the file.
    if TIPS.iter().all(|t| o.tips.iter().any(|s| s == t.id))
        || m.confirm.is_some()
        || m.sheet.is_some()
        || m.on_place()
        || !m.loaded
    {
        return;
    }
    if let Some((_, at, _)) = &m.flash
        && rush_core::time::now() - at < 6000
    {
        return;
    }
    let seen = o.tips.clone();
    for t in &TIPS {
        if !seen.iter().any(|s| s == t.id) && (t.when)(m) {
            cfg_mut(m).onboarding.tips.push(t.id.to_string());
            save(m);
            m.flash(t.text);
            return;
        }
    }
}

/// #tips: Getting started and the tips from the top; #tips off puts them
/// away (keys.go:974).
pub fn tips_command(m: &mut Model, arg: &str) -> Out {
    ensure_store(m);
    let o = &mut cfg_mut(m).onboarding;
    if arg.trim() == "off" {
        o.hidden = true;
        save(m);
        m.flash("Getting started put away · #tips brings it back");
        return Out::Draw;
    }
    o.hidden = false;
    o.steps.clear();
    o.tips.clear();
    save(m);
    m.flash("Getting started and tips from the top");
    Out::Draw
}

/// Turns Getting started and the tips off for this model: a view another
/// app hosts (NewHosted).
pub fn set_hosted(m: &mut Model, hosted: bool) {
    accounts(m).hosted = hosted;
}
