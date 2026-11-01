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
//! reading of the list with a loop running; nothing runs while nothing
//! changes. Only the rush that leads asks the model (lead.rs); one that
//! follows reads the leading rush's answers from the cache.
//!
//! Not here: the list's side (the Needs-you section, the row's marker,
//! wash, summary and second line, the Wall's tile and rank),
//! which the list reads through wants_you, wants_mark, wants_say and
//! turn_label.

use std::collections::HashMap;
use std::sync::RwLock;

use rush_convo::render::{NEEDS_KINDS, Needs, NeedsItem, has_quote};
use rush_core::adapter::{Registry, fit_tokens};
use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_fleet::Predicates;

use super::{conn, open_key};
use crate::frame::Out;
use crate::model::Model;

// What turns want, as the list reads it, is the frame's (crate::turnwants).
pub use rush_ui_base::turnwants::{
    TurnState, TurnWants, WANTS_ASK, WANTS_DONE, WANTS_OFFER, WANTS_PROBLEM, WANTS_YOU, asks,
    turn_label, unlooked, wants_label, wants_mark, wants_of, wants_say, wants_whole, wants_word,
    wants_you,
};

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
/// Unset, it is the model, or none in a test's sandbox (set_wanter overrides).
static WANTER: RwLock<Option<Wanter>> = RwLock::new(None);

/// Sets who reads a last message.
pub fn set_wanter(f: Wanter) {
    *WANTER.write().unwrap_or_else(|e| e.into_inner()) = Some(f);
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
    let (_, out) = rush_ui_session_cards::quickask::ask_cheap(
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

#[doc(hidden)]
pub fn state(m: &mut Model) -> &mut TurnState {
    m.kept::<TurnState>()
}

/// What a's last message asks of you, for its session's Needs-you
/// section: nothing when it asks nothing, or once a turn after it starts
/// (needsOf).
pub fn needs_of(m: &Model, a: &Agent) -> Needs {
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

/// After each reading of the list, with a loop running (model.go's tick):
/// the cache read again when it changed, and the newest finished turn not
/// read yet given to the model. With none (`rush --render`) nothing is
/// read or asked.
pub fn on_fleet(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
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
#[doc(hidden)]
pub fn read_turn(m: &mut Model) {
    let Some(a) = turn_to_read(m) else {
        return;
    };
    if rush_core::dryrun::would(
        "ask-turn-model",
        &a.key,
        &format!(
            "“{}” finished a turn: a model reads what it wants next",
            a.display_name
        ),
    ) {
        // Kept as unread for this run, as one the model couldn't read.
        let at = a.job.updated_at;
        state(m).got.insert(
            a.key.clone(),
            TurnWants {
                at,
                ..TurnWants::default()
            },
        );
        return;
    }
    let (key, at, kind, path, detail) = (
        a.key.clone(),
        a.job.updated_at,
        a.kind.as_str().to_string(),
        a.job.transcript_path.clone(),
        a.job.detail.clone(),
    );
    state(m).reading = true;
    let reg = m.reg.clone();
    let wanter = (*WANTER.read().unwrap_or_else(|e| e.into_inner())).unwrap_or(
        if rush_core::sandbox::active() {
            no_model
        } else {
            ask_model
        },
    );
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

/// The finished turn the model reads next, the newest not read yet; None
/// while one is being read, and always in a rush that follows: only the
/// rush that leads asks a model (turnwants.go:298).
#[doc(hidden)]
pub fn turn_to_read(m: &mut Model) -> Option<Agent> {
    let now = m.now;
    let leads = m.leads();
    let s = state(m);
    if !leads || !s.loaded || s.reading {
        return None;
    }
    let got = std::mem::take(&mut s.got);
    let mut pick: Option<&Agent> = None;
    for (i, a) in m.agents.iter().enumerate() {
        let stale = crate::list::stale(a, now);
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
    pick
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

/// s on one line, cut to fit a flash.
fn brief(s: &str) -> String {
    let one = s.lines().next().unwrap_or("");
    rush_draw::truncate(one, 40, "…").into_owned()
}

/// Does what a row of the Needs-you section offers: a command or value
/// copied, a reply put in your message box to send or change, a link
/// opened (actOnNeeds).
pub fn act_on_needs(m: &mut Model, key: &str, kind: &str, text: &str) {
    match kind {
        "run" => {
            m.copy_text(text.to_string());
            m.flash(&format!("copied · {}", brief(text)));
        }
        // The heading: every command, a line each.
        rush_convo::render::NEEDS_ALL => {
            m.copy_text(text.to_string());
            m.flash(&format!("copied {} commands", text.lines().count()));
        }
        "copy" => {
            // The value, not what it is.
            let v = text.split(" — ").next().unwrap_or("");
            m.copy_text(v.to_string());
            m.flash(&format!("copied · {}", brief(v)));
        }
        "reply" => {
            conn(m, key).input.set(text);
            if let Some(v) = m.session.as_mut() {
                v.sel.clear(); // back to typing
            }
            m.flash("in your message box · enter sends it");
        }
        "link" => {
            rush_ui_accounts::signin::browse(text);
            m.flash(&format!("opened {}", text.trim_start_matches("https://")));
        }
        _ => {}
    }
}
