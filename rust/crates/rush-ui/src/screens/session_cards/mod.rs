//! What waits in a Session's dock, and the sheets and side panels a
//! Session opens: the permission card, Claude's questions, a decisions
//! block as a card, /rewind, /fork, /compact, #slim, the export sheet,
//! the message sheet, enter's first-time question, the quick ask, /btw,
//! the sidekick's notes, rooms on a session, mentions, misdirected
//! messages, a stop with a message, handing a session to another harness,
//! and the message box's pastes and pictures.
//!
//! Go: permcard.go, permissions.go, question.go, rushmode.go's cards
//! (cardKind :5718, cardID :5737, cardKey :5754, questionKey :5476,
//! goQuestion :5617, answerQuestion :5662, sendAnswers :5679,
//! approvalBody :3706, alwaysWords :5948, cardRows :2948), modal.go
//! (cardModal :32), host/decisions.go (DecisionsOf :377, DecisionAnswer
//! :400), host/answer.go, asksheets.go, rewindsheet.go, forksheet.go,
//! compactsheet.go, slimsheet.go, entersheet.go, messagesheet.go,
//! pastes.go, imagerefs.go, clipimage.go, mention.go, misdirect.go,
//! handoff.go, handoff_ready.go, stopnote.go, quickask.go, btwpanel.go,
//! sidekick.go, discuss.go.
//!
//! A hosted session (its client, its message box) is the session group's
//! (session_core). The cards reach it through a [`Host`]: what the
//! session is and says ([`HostInfo`]) and what to do on it ([`Act`]).
//! Until one is linked ([`link`]) no session takes answers, as in Go,
//! where only a hosted session (a hostConn) has a dock of cards.

// pub mod btw;
pub mod card;
// pub mod clipimage;
// pub mod compact;
// pub mod discuss;
// pub mod enter;
// pub mod export;
// pub mod fork;
// pub mod handoff;
// pub mod imagerefs;
// pub mod mention;
// pub mod message;
// pub mod misdirect;
// pub mod pastes;
pub mod permcard;
pub mod permissions;
pub mod question;
// pub mod quickask;
// pub mod rewind;
// pub mod sidekick;
// pub mod slim;
// pub mod stopnote;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::OnceLock;

use rush_convo::{Session, StepId};
use rush_core::event::Question;
use rush_core::text::Line;
use rush_draw::MouseKind;

use crate::frame::{Ext, Mouse, Out};
use crate::model::Model;
use crate::style::*;

pub use card::CardBtn;

/// A permission mode a running harness says it has (host.Info's
/// PermissionModes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// A subagent the session runs: the call that started it, its type and
/// what it was given to do (convo.Subagent's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sub {
    pub tool_use_id: String,
    pub r#type: String,
    pub description: String,
}

/// What the cards read of a hosted session (Go's hostConn and its
/// sess.Info).
#[derive(Clone, Debug, Default)]
pub struct HostInfo {
    /// The harness it runs in (sessionAgent): "claude".
    pub kind: String,
    /// The permission mode it runs in now, and those it says it has.
    pub permission_mode: String,
    pub permission_modes: Vec<Mode>,
    /// Its host is asleep: it wakes for an answer.
    pub asleep: bool,
    /// The subagents it runs.
    pub subs: Vec<Sub>,
    /// The decisions block its last turn ended with, open (Info.Decisions).
    pub decisions: Option<Question>,
    /// What's typed in its message box.
    pub input: String,
    /// Where it works.
    pub cwd: String,
}

/// What a card asks of the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Allows request id; input is the answer's tool input (a question's
    /// answers, as JSON), always allows it for good.
    Allow {
        id: String,
        input: Option<String>,
        always: bool,
    },
    /// Denies request id, telling the agent why when there's a why.
    Deny { id: String, why: String },
    /// Switches the session to a permission mode.
    Permission(String),
    /// Sets the decisions card id aside, sending nothing.
    DismissDecisions(String),
    /// Sends text as your next message.
    Send(String),
    /// What the session goes without from its next start (#slim).
    Without(Vec<String>),
    // /rewind: back to before message uuid (its turn n), the files put
    // back or not, a note of what was learned brought back or not; or
    // down a branch left before, by its session.
    // Rewind(rewind::Rewind),
    // /fork's options, to start the fork.
    // Fork(fork::Fork),
    // /compact by a summarizer of choice.
    // Compact(compact::Compact),
}

/// How the cards reach the hosted sessions (session_core links one).
#[derive(Clone, Copy)]
pub struct Host {
    /// The hosted session behind agent key; None when it isn't hosted.
    pub info: fn(&Model, &str) -> Option<HostInfo>,
    /// Does it on agent key's session.
    pub act: fn(&mut Model, &str, Act) -> Out,
    /// Puts text in agent key's message box.
    pub set_input: fn(&mut Model, &str, &str),
}

static HOST: OnceLock<Host> = OnceLock::new();

/// Links the hosted sessions to the cards, once for the process.
pub fn link(h: Host) {
    let _ = HOST.set(h);
}

/// The cards' state, per Model.
#[derive(Default)]
pub struct Cards {
    /// A host for this model alone (tests); else the one linked.
    pub host: Option<Host>,
    /// Each session's card state, by agent key.
    pub conns: HashMap<String, Conn>,
}

/// A session's cards' state: Go's hostConn fields the cards keep.
#[derive(Clone, Debug, Default)]
pub struct Conn {
    /// The card has the keys (↑ gave them, or it came while you weren't
    /// typing).
    pub card_focus: bool,
    /// The button enter presses.
    pub btn_pick: usize,
    /// The button under the pointer ("btn:y").
    pub sub_hover: String,
    /// The buttons as last drawn, from the card's top.
    pub btns: Vec<CardBtn>,
    /// The card last shown, and whether you were typing as it came.
    pub card_shown: String,
    pub card_typing: bool,
    /// When the card last shown came (cardAt).
    pub card_at: rush_core::time::Ms,
    /// The card last answered with the keys on it.
    pub card_again: String,
    /// The question answered: its id, which question, the option under
    /// the cursor, what's ticked, the answers by question text.
    pub q_for: String,
    pub q_idx: usize,
    pub q_cursor: usize,
    pub q_picks: HashMap<usize, HashMap<usize, bool>>,
    pub q_answer: HashMap<String, String>,
    /// The option clicked once (1-based), so a second click picks it.
    pub q_click: usize,
    /// What was typed for each question.
    pub q_typed: HashMap<usize, String>,
    /// The option under the cursor alone, its preview as tall as fits.
    pub q_full: bool,
    /// You chose to answer in your own words.
    pub q_own: bool,
    /// It's a decisions block, not a question of the agent's.
    pub q_decide: bool,
    /// The decisions card set aside, and one opened again.
    pub q_gone: String,
    pub q_again: Option<Question>,
    /// What a click on each of the question card's rows presses.
    pub q_hit: HashMap<usize, String>,
    pub q_rows: Vec<Line>,
    /// The decisions card answered here.
    pub dec_answered: String,
    /// Where the card is: its first row on screen.
    pub card_top: usize,
    // A stop waiting on its message (stopnote.go).
    // pub stop_note: Option<stopnote::StopNote>,
}

impl Model {
    /// The cards' state.
    pub fn cards(&mut self) -> &mut Cards {
        self.kept::<Cards>()
    }
}

/// The host the cards use: this model's, else the one linked.
pub fn host(m: &Model) -> Option<Host> {
    m.kept
        .peek::<Cards>()
        .and_then(|c| c.host)
        .or_else(|| HOST.get().copied())
}

/// What the cards know of agent key's hosted session.
pub fn info(m: &Model, key: &str) -> Option<HostInfo> {
    (host(m)?.info)(m, key)
}

/// Does a on agent key's session; without a host, says it can't.
pub fn act(m: &mut Model, key: &str, a: Act) -> Out {
    match host(m) {
        Some(h) => (h.act)(m, key, a),
        None => {
            m.flash_err("this session can't take answers here");
            Out::Draw
        }
    }
}

/// Puts text in agent key's message box.
pub fn set_input(m: &mut Model, key: &str, text: &str) {
    if let Some(h) = host(m) {
        (h.set_input)(m, key, text);
    }
}

/// How long a card that came while you were typing ignores its letters
/// (modal.go:22 cardGrace).
pub const CARD_GRACE: rush_core::time::Ms = 600;

impl Conn {
    /// The card came mid-word moments ago: its letters are still yours
    /// (modal.go:51 cardGuarded).
    pub fn card_guarded(&self, now: rush_core::time::Ms) -> bool {
        self.card_typing && now.saturating_sub(self.card_at) < CARD_GRACE
    }
}

/// The open conversation's agent key.
pub fn open_key(m: &Model) -> Option<String> {
    m.session.as_ref().map(|v| v.key.clone())
}

/// The conversation of agent key, when it's the one open.
pub fn session<'a>(m: &'a Model, key: &str) -> Option<&'a Session> {
    m.session.as_ref().filter(|v| v.key == key).map(|v| &v.sess)
}

/// The approvals agent key's session waits on, oldest first.
pub fn pending(m: &Model, key: &str) -> Vec<StepId> {
    session(m, key).map(Session::pending).unwrap_or_default()
}

/// Agent key's card state, made on first use.
pub fn conn<'a>(m: &'a mut Model, key: &str) -> &'a mut Conn {
    m.cards().conns.entry(key.to_string()).or_default()
}

/// Adds the cards to the open Session's dock and keys, and the sheets'
/// and panels' hooks.
pub fn register(x: &mut Ext) {
    x.session_dock.push(dock);
    x.session_keys.push(question::card_key_hook);
    x.mouse.push(mouse);
}

/// The cards waiting under the open conversation, w wide: indented as
/// the conversation is (paneDock's cards).
fn dock(m: &mut Model, w: usize) -> Vec<Line> {
    let Some(key) = open_key(m) else {
        return Vec::new();
    };
    let Some(inf) = info(m, &key) else {
        return Vec::new();
    };
    let cw = w.saturating_sub(4);
    let max_h = m.h * 3 / 5;
    let rows = question::card_rows(m, &key, &inf, cw, max_h);
    if rows.is_empty() {
        return Vec::new();
    }
    // The card's first row on screen: the dock is the screen's last rows
    // but the status line.
    let n = rows.len();
    let top = m.h.saturating_sub(1 + n);
    conn(m, &key).card_top = top;
    rows.into_iter()
        .map(|r| {
            let mut l = line([plain("  ")]);
            join(&mut l, r);
            l
        })
        .collect()
}

/// A click on the open Session's card: on a button it presses it, on a
/// question's option it puts the cursor there (a second click picks it).
fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if !matches!(e.kind, MouseKind::Down(_)) || m.sheet.is_some() || m.confirm.is_some() {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(inf) = info(m, &key) else {
        return Out::Pass;
    };
    question::click(m, &key, &inf, e.x as usize, e.y as usize)
}
