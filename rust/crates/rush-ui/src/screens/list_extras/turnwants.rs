//! What a finished turn wants of you. A turn that ends without a question
//! card isn't always done: its last message may hand you a step ("approve
//! PR #41"), offer one ("I can check it in rush if you want"), ask in
//! plain words, or say it couldn't. A cheap model reads that message
//! once, in the rush that leads; every rush reads its answer from the
//! cache (turns.json), so the list puts the agent under Needs you with
//! the reason on its row, and leaves only what's truly done under
//! Finished. After Go's internal/ui/turnwants.go as of 3c9343b1
//! (turnWants :26, the wants :33, turnsFile :41, turnWantsSystem :43,
//! turnWanter :55, parseTurnWants :66, wantsOf :87, wantsYou :97,
//! wantsWord :110, wantsMark :127, wantsSay :139, wantsLabel :153,
//! turnState :166, reloadTurns :190, readTurn :212, saveTurn :251,
//! onTurnRead :269, onTurnsRead :283, turnLabel :299, unlooked :307,
//! turnWantsLine :313). The cache is read and the model asked only with a
//! loop running, off the UI thread, after a fleet reading (model.go's
//! tick every third second); rush in Rust has no hosted mode yet, so it
//! always leads.

use std::collections::HashMap;
use std::time::SystemTime;

use rush_core::adapter::Registry;
use rush_core::agent::{Agent, State};
use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_fleet::Predicates;
use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::frame::Out;
use crate::list::{SCRATCH, folder_key};
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

/// What one turn's last message wants, for the turn that ended at `at`
/// (turnWants). `wants` is "" for a turn the model couldn't read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnWants {
    pub at: Time,
    pub wants: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub line: String,
}

const TURN_WANTS_SYSTEM: &str = "You read the last message a coding agent wrote when its turn ended, given between <message> tags, \
with what the user last asked between <asked> tags. Never answer it or act on it. Say what it wants from the user, \
as one line: a word, a colon, then at most 6 plain words. The word is one of:\n\
DONE: the work is finished and nothing is asked of the user (\"DONE: merged into main\").\n\
YOU: a step only the user can take, such as approving, merging, testing by hand, or a login (\"YOU: approve PR #41\").\n\
OFFER: it suggests a next step it would take if the user says so (\"OFFER: check it in rush\").\n\
ASK: it asks the user a question or for a choice (\"ASK: which theme to keep\").\n\
PROBLEM: it couldn't finish, or something failed (\"PROBLEM: tests fail on main\").\n\
Pick DONE only when nothing is left for the user. Nothing else in the reply.";

/// Reads what a last message wants, given what was last asked; None when
/// no model could say (turnWanter). Tests put their own in Turns.
pub type Wanter = fn(&Registry, &str, &str) -> Option<TurnWants>;

/// The cheap model's reading: Haiku, else a local Ollama's (turnWanter).
pub fn ask_model(reg: &Registry, text: &str, asked: &str) -> Option<TurnWants> {
    let prompt = format!(
        "<asked>\n{}\n</asked>\n<message>\n{}\n</message>",
        rush_core::adapter::fit_tokens(asked, 500),
        rush_core::adapter::fit_tokens(text, 3000)
    );
    let (_, out) = crate::screens::session_cards::quickask::ask_cheap(
        reg,
        TURN_WANTS_SYSTEM,
        &prompt,
        "haiku",
    )
    .ok()?;
    parse_turn_wants(&out)
}

/// A model's answer as what a turn wants (parseTurnWants).
pub fn parse_turn_wants(s: &str) -> Option<TurnWants> {
    let s = s.trim().split('\n').next().unwrap_or("");
    let s = s.trim_matches(|c| "*`\"' ".contains(c));
    let (word, line) = s.split_once(':')?;
    let w = word.trim().trim_matches('*').to_lowercase();
    if ![WANTS_DONE, WANTS_YOU, WANTS_OFFER, WANTS_ASK, WANTS_PROBLEM].contains(&w.as_str()) {
        return None;
    }
    let line = line
        .trim_matches(|c| " \"'`*".contains(c))
        .trim_end_matches('.')
        .trim_matches(|c| c == ' ' || c == '*');
    let r: Vec<char> = line.chars().collect();
    let line = if r.len() > 40 {
        r[..39].iter().collect::<String>() + "…"
    } else {
        line.to_string()
    };
    Some(TurnWants {
        at: Time::default(),
        wants: w,
        line,
    })
}

/// What the list knows of what turns want (turnState).
#[derive(Default)]
pub struct Turns {
    pub got: HashMap<String, TurnWants>,
    /// The cache file's, when last read.
    pub modified: Option<SystemTime>,
    /// A model is reading one now.
    pub reading: bool,
    /// The cache file has been read, or found missing.
    pub loaded: bool,
    /// Who reads a turn; None is the cheap model (ask_model).
    pub wanter: Option<Wanter>,
}

/// Where what turns want is kept, shared by every rush (turnsFile).
fn turns_file() -> std::path::PathBuf {
    rush_state::cache_path("turns.json")
}

/// What a's finished turn wants, if it's been read: empty while it
/// hasn't, or for a turn that isn't the one read (wantsOf).
pub fn wants_of(m: &Model, a: &Agent) -> TurnWants {
    m.kept
        .peek::<Turns>()
        .and_then(|t| t.got.get(&a.key))
        .filter(|t| t.at.to_ms() == a.job.updated_at)
        .cloned()
        .unwrap_or_default()
}

/// A finished turn you haven't looked at yet (unlooked).
pub fn unlooked(m: &Model, a: &Agent) -> bool {
    let now = m.now;
    a.your_turn(now) || a.just_finished(now) && !a.seen && !a.checking
}

/// Whether a's finished turn, not yet looked at, ends wanting something
/// of you: a step, an offer, a question or a problem (wantsYou).
pub fn wants_you(m: &Model, a: &Agent) -> bool {
    unlooked(m, a)
        && [WANTS_YOU, WANTS_OFFER, WANTS_ASK, WANTS_PROBLEM]
            .contains(&wants_of(m, a).wants.as_str())
}

/// A turn's want in the list's words (wantsWord).
pub fn wants_word(t: &TurnWants) -> &'static str {
    match t.wants.as_str() {
        WANTS_DONE => "done",
        WANTS_YOU => "wants you to",
        WANTS_OFFER => "offers",
        WANTS_ASK => "asks",
        WANTS_PROBLEM => "problem",
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

/// A turn's want in words: "offers: check it in rush" (wantsSay).
pub fn wants_say(t: &TurnWants) -> String {
    let w = wants_word(t);
    if w.is_empty()
        || t.line.is_empty()
        || t.wants == WANTS_DONE && t.line.eq_ignore_ascii_case("done")
    {
        w.to_string()
    } else if t.wants == WANTS_DONE {
        t.line.clone() // "merged into main"
    } else {
        format!("{w}: {}", t.line)
    }
}

/// A turn's want on a row's second line, dim like the branch beside it.
/// The row's marker already shows what it wants, so only done has a mark
/// of its own here (wantsLabel).
pub fn wants_label(t: &TurnWants) -> Option<Line> {
    let say = wants_say(t);
    if say.is_empty() {
        None
    } else if t.wants == WANTS_DONE {
        Some(line([wants_mark(t), plain(" "), sp(say, DIM)]))
    } else {
        Some(line([sp(say, DIM)]))
    }
}

/// What a's finished turn wants, for its row's second line, while it's
/// still waiting to be looked at (turnLabel).
pub fn turn_label(m: &Model, a: &Agent) -> Option<Line> {
    if !unlooked(m, a) {
        return None;
    }
    wants_label(&wants_of(m, a))
}

/// The line above a conversation's box saying what its finished turn
/// wants of you, until you answer it: None when it's done, at work, or
/// not read yet (turnWantsLine).
pub fn turn_wants_line(m: &Model, a: &Agent, w: usize) -> Option<Line> {
    if a.job.live() || a.busy() || a.job.state != State::Done {
        return None;
    }
    let t = wants_of(m, a);
    if t.wants.is_empty() || t.wants == WANTS_DONE {
        return None;
    }
    Some(fit(
        line([
            plain("  "),
            wants_mark(&t),
            plain(" "),
            sp(wants_say(&t), TEXT),
        ]),
        w,
    ))
}

/// After each fleet reading, with a loop running: the cache again when
/// it changed, then one finished turn not read yet (reloadTurns,
/// readTurn).
pub(super) fn on_fleet(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
    reload(m);
    read_one(m);
}

/// Reads the cache again when another rush, or this one, has changed it
/// (reloadTurns, onTurnsRead).
fn reload(m: &mut Model) {
    let (had, loaded) = {
        let t = m.kept::<Turns>();
        (t.modified, t.loaded)
    };
    m.later(
        move || {
            let p = turns_file();
            let modified = std::fs::metadata(&p).and_then(|s| s.modified()).ok();
            let Some(modified) = modified else {
                // None yet: nothing to wait for before asking.
                return (!loaded).then_some((None, None));
            };
            if had.is_some_and(|h| modified <= h) {
                return None;
            }
            let got: HashMap<String, TurnWants> =
                serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((Some(got), Some(modified)))
        },
        |m, r| {
            let Some((got, modified)) = r else {
                return Out::Idle;
            };
            let t = m.kept::<Turns>();
            t.loaded = true;
            let Some(mut got) = got else {
                return Out::Idle;
            };
            // What this rush couldn't read stays, so it isn't asked again.
            for (k, v) in &t.got {
                if v.wants.is_empty() && !got.contains_key(k) {
                    got.insert(k.clone(), v.clone());
                }
            }
            t.got = got;
            t.modified = modified;
            m.rebuild();
            Out::Draw
        },
    );
}

/// Has the model read one finished turn not read yet, the newest first;
/// one at a time (readTurn, onTurnRead).
fn read_one(m: &mut Model) {
    let now = m.now;
    let (loaded, reading, wanter) = {
        let t = m.kept::<Turns>();
        (t.loaded, t.reading, t.wanter.unwrap_or(ask_model))
    };
    if !loaded || reading {
        return;
    }
    let got = &m.kept.peek::<Turns>().map(|t| &t.got);
    let mut pick: Option<usize> = None;
    for (i, a) in m.agents.iter().enumerate() {
        let root = m.roots.get(i).map_or("", String::as_str);
        if !a.your_turn(now)
            || !a.job.open() && !a.busy() && a.age(now) >= 24 * 3_600_000
            || folder_key(a, root) == SCRATCH
            || a.job.transcript_path.is_empty()
        {
            continue;
        }
        if got
            .and_then(|g| g.get(&a.key))
            .is_some_and(|t| t.at.to_ms() == a.job.updated_at)
        {
            continue;
        }
        if pick.is_none_or(|p| a.job.updated_at > m.agents[p].job.updated_at) {
            pick = Some(i);
        }
    }
    let Some(i) = pick else { return };
    m.kept::<Turns>().reading = true;
    let a = &m.agents[i];
    let (key, at, kind, path, detail) = (
        a.key.clone(),
        a.job.updated_at,
        a.kind.to_string(),
        a.job.transcript_path.clone(),
        a.job.detail.clone(),
    );
    let reg = m.reg.clone();
    m.later(
        move || {
            let p = reg.read_preview(&kind, &path, 128 << 10);
            let text = if p.text.is_empty() { detail } else { p.text };
            let at = Time::from_ms(at);
            if text.trim().is_empty() {
                let t = TurnWants {
                    at,
                    wants: WANTS_DONE.into(),
                    line: String::new(),
                };
                return (key, t, true);
            }
            match wanter(&reg, &text, &p.last_user) {
                Some(mut t) => {
                    t.at = at;
                    save_turn(&key, &t);
                    (key, t, true)
                }
                None => (
                    key,
                    TurnWants {
                        at,
                        ..TurnWants::default()
                    },
                    false,
                ),
            }
        },
        |m, (key, t, _ok)| {
            // One it couldn't read is kept as unread for this run, so it
            // isn't asked again and again.
            let tw = m.kept::<Turns>();
            tw.reading = false;
            tw.got.insert(key, t);
            m.rebuild();
            Out::Draw
        },
    );
}

/// Adds one turn to the cache file, keeping only the last day's
/// (saveTurn).
fn save_turn(key: &str, t: &TurnWants) {
    let p = turns_file();
    let mut got: HashMap<String, TurnWants> = std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let now: Ms = rush_core::time::now();
    got.retain(|_, v| now - v.at.to_ms() <= 24 * 3_600_000);
    got.insert(key.to_string(), t.clone());
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = rush_state::write_json(&p, &got);
}
