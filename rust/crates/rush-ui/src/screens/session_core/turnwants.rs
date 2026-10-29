//! What a finished turn wants of you. A turn that ends without a question
//! card isn't always done: its last message may hand you a step ("approve
//! PR #41"), offer one ("I can check it in rush if you want"), ask in
//! plain words, or say it couldn't. A cheap model reads that message once,
//! in the rush that leads; every rush reads its answer from the cache
//! (turns.json), so the list can put the agent under Needs you with the
//! reason on its row, and the session ends its last answer with a
//! Needs-you section laid out by that model, whose rows a click acts on.
//! After Go's internal/ui/turnwants.go (turnWants :28, the kinds :43,
//! turnWantsSystem :54, turnWanter :82, wantsModel :94, parseTurnWants
//! :115, wantsOf :164, wantsYou :174, needsOf :189, wantsWord :199,
//! wantsMark :216, wantsSay :231, wantsWhole :240, wantsLabel :250,
//! turnState :263, reloadTurns :285, readTurn :308, saveTurn :365,
//! onTurnRead :383, onTurnsRead :398, turnLabel :412, unlooked :420) and
//! rushmode.go (rushPane's o.Needs :2369, clickRef's Needs :5085,
//! actOnNeeds :5142).
//!
//! The model is read and the cache written off the UI thread, after a
//! reading of the list; nothing runs while nothing changes. Rust's rush
//! has no hosted or following mode yet, so it always leads.
//!
//! Not here: the list's side (the Needs-you section, the row's marker,
//! wash, summary and second line, the Wall's tile and rank, Zen's list),
//! which the list reads through wants_you, wants_mark, wants_say and
//! turn_label.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::SystemTime;

use rush_convo::render::{NEEDS_KINDS, Needs, NeedsItem, has_quote};
use rush_core::adapter::{Registry, fit_tokens};
use rush_core::agent::Agent;
use rush_core::text::Span;
use rush_core::time::{Ms, format_rfc3339, parse_rfc3339};
use rush_fleet::Predicates;
use serde::{Deserialize, Serialize};

use super::{conn, open_key};
use crate::frame::Out;
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

/// Where what turns want is kept, shared by every rush (turnsFile).
fn turns_file() -> std::path::PathBuf {
    rush_state::cache_path("turns.json")
}

const TURN_WANTS_SYSTEM: &str = "You read the last message a coding agent wrote when its turn ended, given between <message> tags, \
with what the user last asked between <asked> tags. Never answer it or act on it. Say what it wants from the user, \
as one line: a word, a colon, then at most 8 plain words. The word is one of:\n\
DONE: the work is finished and nothing is asked of the user (\"DONE: new-agent startup fix merged into main\").\n\
YOU: a step only the user can take, such as approving, merging, testing by hand, or a login (\"YOU: approve PR #41, the login fix\").\n\
OFFER: it suggests a next step it would take if the user says so (\"OFFER: check the theme picker in rush\").\n\
ASK: it asks the user a question or for a choice (\"ASK: which theme to keep\").\n\
PROBLEM: it couldn't finish, or something failed (\"PROBLEM: tests fail on main\").\n\
Name the thing it's about, in the user's words: what was fixed or built, which PR, which test. \
Never a bare \"this\", \"it\", \"the build\" or \"the change\" that only makes sense next to the message. \
Pick DONE only when nothing is left for the user. \
Unless it's DONE, add a second line: QUOTE, a colon, then the words in the message that ask it of the user, \
copied exactly, at most 15 words (\"QUOTE: I haven't pushed it\"). \
Then, unless it's DONE, lay out what the user can do as up to 6 more lines, each a word, a colon, then a few words, \
for rush to draw as a section the user acts on. The word is one of:\n\
DO: a step only the user can take, by hand (\"DO: approve PR #41 on GitHub\").\n\
RUN: a shell command the user should run, exactly as the message gives it; a click copies it (\"RUN: git push origin main\").\n\
COPY: a value the user must paste somewhere, exactly as the message gives it, such as a hostname, an ID or a key name; \
one per line, with what it is after \" — \" (\"COPY: d1u5r91gd2oxgy.cloudfront.net — the CNAME target\").\n\
REPLY: a short reply the user could send to say yes, pick an option or answer, in the user's own voice; a click puts it in their message box (\"REPLY: yes, push it\").\n\
LINK: a URL from the message the user needs to open.\n\
NOTE: one fact the user needs to decide, at most 10 words.\n\
Spell out each step with its exact names and values from the message; never a step that sends the user back to the message. \
Use only the lines that fit, in the order the user would act; an offer or question wants a REPLY for each choice. \
Never invent a command or link the message doesn't have. Nothing else in the reply.";

/// Reads what a last message wants (text) given what was last asked;
/// None when no model could say.
pub type Wanter = fn(&Registry, &str, &str) -> Option<TurnWants>;

/// Who reads a last message (turnWanter): a cheap model, except in tests,
/// which never spend a model call.
static WANTER: RwLock<Wanter> = RwLock::new(if cfg!(test) { no_model } else { ask_model });

/// Sets who reads a last message.
pub fn set_wanter(f: Wanter) {
    *WANTER.write().unwrap_or_else(|e| e.into_inner()) = f;
}

fn no_model(_: &Registry, _: &str, _: &str) -> Option<TurnWants> {
    None
}

/// The cheap model's reading (turnWanter).
fn ask_model(reg: &Registry, text: &str, asked: &str) -> Option<TurnWants> {
    let prompt = format!(
        "<asked>\n{}\n</asked>\n<message>\n{}\n</message>",
        fit_tokens(asked, 500),
        fit_tokens(text, 3000)
    );
    let (_, out) = crate::screens::session_cards::quickask::ask_cheap(
        reg,
        TURN_WANTS_SYSTEM,
        &prompt,
        wants_model(text),
    )
    .ok()?;
    parse_turn_wants(&out)
}

/// The model that reads a last message: Haiku for most, but Sonnet for a
/// long or busy one (several questions, a list of options, commands in
/// code), where Haiku lays out the Needs-you section badly (wantsModel).
pub fn wants_model(text: &str) -> &'static str {
    if text.chars().count() > 1500 || text.matches('?').count() >= 3 || text.contains("```") {
        return "sonnet";
    }
    let items = text
        .split('\n')
        .map(str::trim)
        .filter(|l| {
            let b = l.as_bytes();
            l.starts_with("- ")
                || l.starts_with("* ")
                || b.len() > 2 && b[0].is_ascii_digit() && (b[1] == b'.' || b[1] == b')')
        })
        .count();
    if items >= 4 {
        return "sonnet";
    }
    "haiku"
}

/// A model's answer as what a turn wants: its first line, then the quote
/// and the section's rows on the lines after it (parseTurnWants).
pub fn parse_turn_wants(s: &str) -> Option<TurnWants> {
    let s = s.trim();
    let (first, more) = s.split_once('\n').unwrap_or((s, ""));
    let (word, line) = first
        .trim_matches(|c| matches!(c, '*' | '`' | '"' | '\'' | ' '))
        .split_once(':')?;
    let w = word.trim().trim_matches('*').to_lowercase();
    if ![WANTS_DONE, WANTS_YOU, WANTS_OFFER, WANTS_ASK, WANTS_PROBLEM].contains(&w.as_str()) {
        return None;
    }
    let line = line
        .trim_matches(|c| matches!(c, ' ' | '"' | '\'' | '`' | '*'))
        .trim_end_matches('.')
        .trim_matches([' ', '*']);
    let mut line = line.to_string();
    if line.chars().count() > 120 {
        line = line.chars().take(119).collect::<String>() + "…";
    }
    let mut t = TurnWants {
        wants: w,
        line,
        ..TurnWants::default()
    };
    if t.wants == WANTS_DONE {
        return Some(t);
    }
    for l in more.split('\n') {
        let l = l.trim().trim_start_matches(['-', '*', '•', ' ']);
        let Some((k, q)) = l.split_once(':') else {
            continue;
        };
        let k = k.trim_matches(['*', ' ']).to_lowercase();
        if k == "quote" && t.quote.is_empty() {
            t.quote = q.trim_matches(['*', '"', '“', '”', ' ']).to_string();
        } else if NEEDS_KINDS.contains(&k.as_str()) && t.ui.len() < 6 {
            let q = q.trim_matches(['*', ' ']);
            let q = if k == "run" || k == "link" || k == "copy" {
                q.trim_matches(['`', '<', '>'])
            } else {
                q.trim_matches(['"', '“', '”'])
            };
            if !q.is_empty() {
                t.ui.push(NeedsItem {
                    kind: k,
                    text: q.to_string(),
                });
            }
        }
    }
    Some(t)
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

fn state(m: &mut Model) -> &mut TurnState {
    m.kept::<TurnState>()
}

/// What a's finished turn wants, if it's been read: empty while it
/// hasn't, or for a turn that isn't finished (wantsOf).
pub fn wants_of(m: &mut Model, a: &Agent) -> TurnWants {
    match state(m).got.get(&a.key) {
        Some(t) if t.at == a.job.updated_at => t.clone(),
        _ => TurnWants::default(),
    }
}

/// a's finished turn, not yet looked at, ends wanting something of you: a
/// step, an offer, a question or a problem (wantsYou).
pub fn wants_you(m: &mut Model, a: &Agent) -> bool {
    unlooked(m.now, a) && asks(&wants_of(m, a))
}

/// t wants something of you.
fn asks(t: &TurnWants) -> bool {
    [WANTS_YOU, WANTS_OFFER, WANTS_ASK, WANTS_PROBLEM].contains(&t.wants.as_str())
}

/// What a's last message asks of you, for its session's Needs-you
/// section: nothing when it asks nothing, or once a turn after it starts
/// (needsOf).
pub fn needs_of(m: &mut Model, a: &Agent) -> Needs {
    let t = wants_of(m, a);
    if !asks(&t) {
        return Needs::default();
    }
    Needs {
        kind: t.wants.clone(),
        reason: wants_whole(&t),
        quote: t.quote,
        items: t.ui,
    }
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
pub fn turn_label(m: &mut Model, a: &Agent) -> Vec<Span> {
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

/// After each reading of the list: the cache read again when it changed,
/// and the newest finished turn not read yet given to the model.
pub fn on_fleet(m: &mut Model) {
    reload(m);
    read_turn(m);
}

/// Reads the cache again when another rush, or this one, has changed it
/// (reloadTurns).
fn reload(m: &mut Model) {
    let s = state(m);
    if s.reloading {
        return;
    }
    s.reloading = true;
    let (had, loaded) = (s.modified, s.loaded);
    m.later(
        move || {
            let path = turns_file();
            let modified = std::fs::metadata(&path).and_then(|st| st.modified()).ok();
            let Some(at) = modified else {
                // None yet: nothing to wait for before asking.
                return (!loaded).then(|| (HashMap::new(), None));
            };
            if had.is_some_and(|h| at <= h) {
                return None;
            }
            let got = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<HashMap<String, TurnWants>>(&b).ok())?;
            Some((got, Some(at)))
        },
        |m, r| {
            let s = state(m);
            s.reloading = false;
            let Some((mut got, at)) = r else {
                return Out::Idle;
            };
            s.loaded = true;
            if at.is_none() {
                return Out::Idle;
            }
            // What this rush couldn't read stays, so it isn't asked again.
            for (k, t) in &s.got {
                if t.wants.is_empty() && !got.contains_key(k) {
                    got.insert(k.clone(), t.clone());
                }
            }
            s.got = got;
            s.modified = at;
            m.rebuild();
            Out::Draw
        },
    );
}

/// Has the model read one finished turn not read yet, the newest first,
/// one at a time (readTurn).
fn read_turn(m: &mut Model) {
    let now = m.now;
    let s = state(m);
    if !s.loaded || s.reading {
        return;
    }
    let got = std::mem::take(&mut s.got);
    let mut pick: Option<&Agent> = None;
    for (i, a) in m.agents.iter().enumerate() {
        let stale = !a.job.open() && !a.busy() && a.age(now) >= 24 * 3_600_000;
        let root = m.roots.get(i).map_or("", String::as_str);
        if !a.your_turn(now)
            || stale
            || crate::list::folder_key(a, root) == crate::list::SCRATCH
            || a.job.transcript_path.is_empty()
        {
            continue;
        }
        if got.get(&a.key).is_some_and(|t| t.at == a.job.updated_at) {
            continue;
        }
        if pick.is_none_or(|p| a.job.updated_at > p.job.updated_at) {
            pick = Some(a);
        }
    }
    let pick = pick.cloned();
    state(m).got = got;
    let Some(a) = pick else {
        return;
    };
    let (key, at, kind, path, detail) = (
        a.key.clone(),
        a.job.updated_at,
        a.kind.as_str().to_string(),
        a.job.transcript_path.clone(),
        a.job.detail.clone(),
    );
    state(m).reading = true;
    let reg = m.reg.clone();
    let wanter = *WANTER.read().unwrap_or_else(|e| e.into_inner());
    let k = key.clone();
    m.later(
        move || {
            let p = reg.read_preview(&kind, &path, 128 << 10);
            let text = if p.text.is_empty() { detail } else { p.text };
            if text.trim().is_empty() {
                let t = TurnWants {
                    at,
                    wants: WANTS_DONE.into(),
                    ..TurnWants::default()
                };
                save_turn(&k, &t);
                return Some(t);
            }
            let mut t = wanter(&reg, &text, &p.last_user)?;
            t.at = at;
            if !has_quote(&text, &t.quote) {
                t.quote.clear(); // a quote the message doesn't hold says nothing
            }
            // A command, value or link the message doesn't have is made
            // up: dropped.
            t.ui.retain(|it| {
                let v = it.text.split(" — ").next().unwrap_or("");
                !matches!(it.kind.as_str(), "run" | "link" | "copy") || text.contains(v)
            });
            save_turn(&k, &t);
            Some(t)
        },
        move |m, r| {
            on_turn_read(m, key, at, r);
            Out::Draw
        },
    );
}

/// Adds one turn to the cache file, keeping only the last day's
/// (saveTurn). Off the UI thread only.
fn save_turn(key: &str, t: &TurnWants) {
    let path = turns_file();
    let mut got: HashMap<String, TurnWants> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let now = rush_core::time::now();
    got.retain(|_, v| now - v.at <= 24 * 3_600_000);
    got.insert(key.to_string(), t.clone());
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = rush_state::write_json(&path, &got);
}

/// Keeps what the model said of a turn. One it couldn't read is kept as
/// unread for this run, so it isn't asked again and again (onTurnRead).
fn on_turn_read(m: &mut Model, key: String, at: Ms, r: Option<TurnWants>) {
    let s = state(m);
    s.reading = false;
    let t = r.unwrap_or(TurnWants {
        at,
        ..TurnWants::default()
    });
    s.got.insert(key, t);
    m.rebuild();
}

/// Sets the open session's Needs-you section from what its last turn
/// wants (rushPane's o.Needs).
pub fn sync_needs(m: &mut Model) {
    let Some(key) = open_key(m) else {
        return;
    };
    let Some(a) = super::agent(m, &key).cloned() else {
        return;
    };
    let n = needs_of(m, &a);
    if let Some(v) = m.session.as_mut()
        && v.o.needs != n
    {
        v.o.needs = n;
    }
}

/// Does what a row of the Needs-you section offers: a command or value
/// copied, a reply put in your message box to send or change, a link
/// opened (actOnNeeds).
pub fn act_on_needs(m: &mut Model, key: &str, kind: &str, text: &str) {
    match kind {
        "run" => m.copy_text(text.to_string()),
        "copy" => {
            // The value, not what it is.
            let v = text.split(" — ").next().unwrap_or("");
            m.copy_text(v.to_string());
        }
        "reply" => {
            conn(m, key).input.set(text);
            if let Some(v) = m.session.as_mut() {
                v.sel.clear(); // back to typing
            }
            m.flash("in your message box · enter sends it");
        }
        "link" => {
            crate::screens::accounts::signin::browse(text);
            m.flash(&format!("opened {}", text.trim_start_matches("https://")));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tw(wants: &str, line: &str, quote: &str, ui: &[(&str, &str)]) -> TurnWants {
        TurnWants {
            wants: wants.into(),
            line: line.into(),
            quote: quote.into(),
            ui: ui
                .iter()
                .map(|(k, t)| NeedsItem {
                    kind: (*k).into(),
                    text: (*t).into(),
                })
                .collect(),
            ..TurnWants::default()
        }
    }

    /// A model's one line becomes what the turn wants; anything else is no
    /// answer, so the agent stays where it was (TestParseTurnWants).
    #[test]
    fn parse_turn_wants_reads_the_model() {
        for (input, want) in [
            (
                "OFFER: check it in rush",
                tw("offer", "check it in rush", "", &[]),
            ),
            (
                "**YOU:** approve PR #41.",
                tw("you", "approve PR #41", "", &[]),
            ),
            (
                "DONE: merged into main\nmore text",
                tw("done", "merged into main", "", &[]),
            ),
            (
                "problem: tests fail on main",
                tw("problem", "tests fail on main", "", &[]),
            ),
            (
                "YOU: push the merge\nQUOTE: I haven't pushed it",
                tw("you", "push the merge", "I haven't pushed it", &[]),
            ),
            (
                "ASK: which theme\n**QUOTE:** \"Which theme?\"",
                tw("ask", "which theme", "Which theme?", &[]),
            ),
            (
                "DONE: merged\nQUOTE: it's merged",
                tw("done", "merged", "", &[]),
            ),
            (
                "YOU: push the merge\nQUOTE: not pushed\nDO: rebuild rush\nRUN: `git push`\n- REPLY: \"yes, push it\"\nLINK: <https://x.y/1>\nNOTE: tests pass\nPICK: no",
                tw(
                    "you",
                    "push the merge",
                    "not pushed",
                    &[
                        ("do", "rebuild rush"),
                        ("run", "git push"),
                        ("reply", "yes, push it"),
                        ("link", "https://x.y/1"),
                        ("note", "tests pass"),
                    ],
                ),
            ),
            ("DONE: merged\nDO: nothing", tw("done", "merged", "", &[])),
        ] {
            assert_eq!(parse_turn_wants(input), Some(want), "{input:?}");
        }
        for input in ["", "I think it's done", "MAYBE: who knows"] {
            assert_eq!(parse_turn_wants(input), None, "{input:?}");
        }
    }

    /// A short, plain message is read by Haiku; a long or busy one by
    /// Sonnet (TestWantsModel).
    #[test]
    fn long_or_busy_messages_go_to_sonnet() {
        let long = "word ".repeat(400);
        for (text, want) in [
            ("Merged. I haven't pushed it.", "haiku"),
            (long.as_str(), "sonnet"),
            ("Which? This? Or that?", "sonnet"),
            ("Run:\n```\ngit push\n```", "sonnet"),
            ("Options:\n1. a\n2. b\n- c\n- d", "sonnet"),
            ("Two:\n- a\n- b", "haiku"),
        ] {
            assert_eq!(wants_model(text), want, "{text:.30}");
        }
    }

    /// What a turn wants in words: the line it said, a word when it said
    /// nothing more, cut short for a row; done has its own mark.
    #[test]
    fn wants_in_words() {
        let t = tw("you", "approve PR #52", "", &[]);
        assert_eq!(wants_say(&t), "approve PR #52");
        assert_eq!(wants_whole(&tw("offer", "", "", &[])), "offers a next step");
        assert_eq!(wants_whole(&tw("done", "Done", "", &[])), "done");
        let long = tw("ask", &"x".repeat(80), "", &[]);
        assert_eq!(wants_say(&long).chars().count(), 56);
        assert!(wants_say(&long).ends_with('…'));
        assert_eq!(wants_mark(&t).text, "!");
        assert_eq!(wants_mark(&tw("problem", "", "", &[])).style.fg, RED);
        assert_eq!(wants_mark(&tw("ask", "", "", &[])).text, "?");
        let done: String = wants_label(&tw("done", "merged into main", "", &[]))
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(done, "✓ merged into main");
    }

    /// A finished turn whose last message offers, asks, hands you a step
    /// or says it couldn't wants you, and its label says what; one that's
    /// done, or not read yet, or read for an older turn, doesn't; looked
    /// at, it asks nothing more (TestFinishedTurnsThatWantYouNeedYou's
    /// session side).
    #[test]
    fn finished_turns_that_want_you() {
        let now = 1_800_000_000_000;
        let mut m = Model::new("0.1.0");
        m.now = now;
        let agent = |key: &str| {
            let mut a = Agent {
                key: key.into(),
                pid: 7,
                ..Agent::default()
            };
            a.job.state = rush_core::agent::State::Done;
            a.job.updated_at = now - 5 * 60_000;
            a
        };
        let (mut offer, done, unread, older) = (
            agent("offer"),
            agent("done"),
            agent("unread"),
            agent("older"),
        );
        let at = offer.job.updated_at;
        let s = m.kept::<TurnState>();
        s.got.insert(
            "offer".into(),
            TurnWants {
                at,
                ..tw("offer", "check it in rush", "", &[])
            },
        );
        s.got.insert(
            "done".into(),
            TurnWants {
                at,
                ..tw("done", "merged into main", "", &[])
            },
        );
        s.got.insert(
            "older".into(),
            TurnWants {
                at: at - 3_600_000,
                ..tw("ask", "which theme", "", &[])
            },
        );
        assert!(wants_you(&mut m, &offer));
        for a in [&done, &unread, &older] {
            assert!(!wants_you(&mut m, a), "{}", a.key);
        }
        let label = |m: &mut Model, a: &Agent| -> String {
            turn_label(m, a).iter().map(|s| s.text.as_str()).collect()
        };
        assert_eq!(label(&mut m, &offer), "check it in rush");
        assert_eq!(label(&mut m, &done), "✓ merged into main");
        offer.seen = true;
        assert!(!wants_you(&mut m, &offer));
        assert_eq!(label(&mut m, &offer), "");
    }

    /// The cache keeps Go's form: times as RFC 3339, empty fields left out.
    #[test]
    fn cache_reads_as_go_writes_it() {
        let j = r#"{"k":{"at":"2026-10-28T12:34:56.789Z","wants":"you","line":"approve PR #41","ui":[{"kind":"run","text":"git push"}]}}"#;
        let got: HashMap<String, TurnWants> = serde_json::from_str(j).unwrap();
        let t = &got["k"];
        assert_eq!(t.at, parse_rfc3339("2026-10-28T12:34:56.789Z"));
        assert_eq!(t.ui[0].text, "git push");
        let back = serde_json::to_string(t).unwrap();
        assert!(
            back.contains(r#""at":"2026-10-28T12:34:56.789Z""#),
            "{back}"
        );
        assert!(!back.contains("quote"), "{back}");
    }
}
