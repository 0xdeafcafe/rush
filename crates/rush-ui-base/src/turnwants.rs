//! What a finished turn wants of you, as the list reads it: under Needs
//! you with the reason on its row, its marker, its second line. A cheap
//! model reads the turn's last message and the cache keeps its answer
//! (screens/session_core/turnwants.rs); the list reads TurnState. After
//! Go's internal/ui/turnwants.go (turnWants :28, the kinds :43, wantsOf
//! :164, wantsYou :174, wantsWord :199, wantsMark :216, wantsSay :231,
//! wantsWhole :240, wantsLabel :250, turnState :263, turnLabel :412,
//! unlooked :420).

use std::collections::HashMap;
use std::time::SystemTime;

use rush_convo::render::NeedsItem;
use rush_core::agent::Agent;
use rush_core::text::Span;
use rush_core::time::{Ms, format_rfc3339, parse_rfc3339};
use rush_fleet::Predicates;
use serde::{Deserialize, Serialize};

use crate::model::Model;
use crate::style::*;

/// Nothing for you.
pub const WANTS_DONE: &str = "done";
/// A step only you can take.
pub const WANTS_YOU: &str = "you";
/// A next step it will take if you say so.
pub const WANTS_OFFER: &str = "offer";
/// A question in plain words.
pub const WANTS_ASK: &str = "ask";
/// It couldn't do it.
pub const WANTS_PROBLEM: &str = "problem";

/// What one turn's last message wants, for the turn that ended at `at`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWants {
    #[serde(with = "rfc3339")]
    pub at: Ms,
    /// WANTS_DONE, WANTS_YOU, WANTS_OFFER, WANTS_ASK or WANTS_PROBLEM.
    #[serde(default)]
    pub wants: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub line: String,
    /// The words in the message that ask it, as the message has them, for
    /// the session to mark; "" for done, or a quote not in it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub quote: String,
    /// The rows the model laid out for the session's Needs-you section:
    /// steps, a command, replies, links, a note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<NeedsItem>,
}

/// A time as Go writes one in JSON.
mod rfc3339 {
    use rush_core::time::Ms;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &Ms, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::format_rfc3339(*at))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Ms, D::Error> {
        let s = String::deserialize(d)?;
        Ok(super::parse_rfc3339(&s))
    }
}

/// What the list knows of what turns want (turnState).
#[derive(Default)]
pub struct TurnState {
    pub got: HashMap<String, TurnWants>,
    /// The cache file's, when last read.
    pub modified: Option<SystemTime>,
    /// A model is reading one now.
    pub reading: bool,
    /// The cache file is being read again.
    pub reloading: bool,
    /// The cache file has been read, or found missing.
    pub loaded: bool,
}

/// What a's finished turn wants, if it's been read: empty while it
/// hasn't, or for a turn that isn't finished (wantsOf).
pub fn wants_of(m: &Model, a: &Agent) -> TurnWants {
    match m.kept.peek::<TurnState>().and_then(|s| s.got.get(&a.key)) {
        Some(t) if t.at == a.job.updated_at => t.clone(),
        _ => TurnWants::default(),
    }
}

/// a's finished turn, not yet looked at, ends wanting something of you: a
/// step, an offer, a question or a problem (wantsYou).
pub fn wants_you(m: &Model, a: &Agent) -> bool {
    unlooked(m.now, a) && asks(&wants_of(m, a))
}

/// t wants something of you.
pub fn asks(t: &TurnWants) -> bool {
    [WANTS_YOU, WANTS_OFFER, WANTS_ASK, WANTS_PROBLEM].contains(&t.wants.as_str())
}

/// A turn's want in a word, for a turn read with nothing more to say
/// (wantsWord).
pub fn wants_word(t: &TurnWants) -> &'static str {
    match t.wants.as_str() {
        WANTS_DONE => "done",
        WANTS_YOU => "your move",
        WANTS_OFFER => "offers a next step",
        WANTS_ASK => "asks you something",
        WANTS_PROBLEM => "hit a problem",
        _ => "",
    }
}

/// A turn's want as the row's marker (wantsMark).
pub fn wants_mark(t: &TurnWants) -> Span {
    match t.wants.as_str() {
        WANTS_DONE => sp("✓", GREEN),
        WANTS_YOU => spb("!", YELLOW),
        WANTS_PROBLEM => spb("!", RED),
        _ => spb("?", YELLOW),
    }
}

/// A turn's want in plain words: what it said, "approve PR #41", with no
/// label in front; the row's marker already says what kind. Cut short for
/// a row of the list; the session's section has it whole (wantsSay).
pub fn wants_say(t: &TurnWants) -> String {
    let s = wants_whole(t);
    if s.chars().count() > 56 {
        return s.chars().take(55).collect::<String>() + "…";
    }
    s
}

/// wants_say not cut short (wantsWhole).
pub fn wants_whole(t: &TurnWants) -> String {
    if t.line.is_empty() || t.wants == WANTS_DONE && t.line.eq_ignore_ascii_case("done") {
        return wants_word(t).to_string();
    }
    t.line.clone()
}

/// A turn's want on a row's second line, dim like the branch beside it.
/// The row's marker already shows what it wants, so only done has a mark
/// of its own here (wantsLabel).
pub fn wants_label(t: &TurnWants) -> Vec<Span> {
    let say = wants_say(t);
    if say.is_empty() {
        return Vec::new();
    }
    if t.wants == WANTS_DONE {
        return vec![wants_mark(t), plain(" "), sp(say, DIM)];
    }
    vec![sp(say, DIM)]
}

/// What a's finished turn wants, for its row's second line, while it's
/// still waiting to be looked at; nothing otherwise (turnLabel).
pub fn turn_label(m: &Model, a: &Agent) -> Vec<Span> {
    if !unlooked(m.now, a) {
        return Vec::new();
    }
    let t = wants_of(m, a);
    wants_label(&t)
}

/// A finished turn you haven't looked at yet (unlooked).
pub fn unlooked(now: Ms, a: &Agent) -> bool {
    a.your_turn(now) || a.just_finished(now) && !a.seen && !a.checking
}

/// Why a session sits under Needs you: what kind of thing it wants, a
/// line that says it, and the words it's read from. The list's section
/// and `rush session needs` both ask [`needs_you_of`]. Rust-only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NeedsYou {
    /// decision, approval, question, problem, command, step or offer.
    pub kind: String,
    pub reason: String,
    /// What it's read from: the call waiting, the card's topics, the
    /// halt's error, or the last message's own words.
    pub text: String,
}

/// What a needs of you, as the list's Needs you section has it, given
/// what its finished turn was read to want (t: [`wants_of`], or the
/// cache's reading); None when it isn't there. Its live state first (a
/// question, a permission, a decisions card, a halt), then the reading of
/// a turn you haven't looked at yet.
pub fn needs_you_of(now: Ms, a: &Agent, t: &TurnWants) -> Option<NeedsYou> {
    let mk = |kind: &str, reason: &str, text: &str| NeedsYou {
        kind: kind.into(),
        reason: reason.into(),
        text: text.into(),
    };
    if crate::list::asks(a) {
        if !a.needs_you() && !a.waiting() {
            // Halted: its last turn ended on an error.
            let text = a.spend.halt.as_ref().map_or("", |h| h.text.as_str());
            return Some(mk("problem", &a.halt_reason(now), text));
        }
        let needs = a.job.needs.as_str();
        if a.decides {
            let topics: Vec<&str> = a
                .decisions_key
                .split('\x1f')
                .filter(|s| !s.is_empty())
                .collect();
            return Some(mk("decision", needs, &topics.join("; ")));
        }
        let question =
            needs.starts_with("asks:") || needs == "has a question" || needs.ends_with('?');
        let kind = if question { "question" } else { "approval" };
        let reason = if needs.is_empty() {
            "waits on you"
        } else {
            needs
        };
        return Some(mk(kind, reason, needs));
    }
    if !unlooked(now, a) || !asks(t) {
        return None;
    }
    let kind = match t.wants.as_str() {
        WANTS_YOU if t.ui.iter().any(|i| i.kind == "run") => "command",
        WANTS_YOU => "step",
        WANTS_OFFER => "offer",
        WANTS_ASK => "question",
        _ => "problem",
    };
    let text = if t.quote.is_empty() {
        t.ui.iter()
            .map(|i| format!("{}: {}", i.kind, i.text))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        t.quote.clone()
    };
    Some(mk(kind, &wants_whole(t), &text))
}
