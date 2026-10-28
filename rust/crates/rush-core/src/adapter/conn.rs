//! A running session, and what a session rush hosts can do beyond it.
//! After Go's internal/agent: adapter.go (Conn, Answerer) and conn.go.
//!
//! Each optional part is a Go interface a Conn has only when its agent
//! can; here a method on [`Conn`] hands it out, None by default. The host
//! offers an op only to a session whose Conn has it.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use serde_json::value::RawValue;

use super::{Input, Result};
use crate::event::Event;
use crate::time::Ms;
use crate::usage;
use crate::wire::RawJson;

/// A running session.
pub trait Conn: Send {
    /// What the session says, until it ends: taken once, by the one who
    /// reads it (None after). Go's Events channel.
    fn events(&mut self) -> Option<Receiver<Event>>;
    fn send(&self, input: Input) -> Result<()>;
    fn answer(&self, approval_id: &str, option_id: &str) -> Result<()>;
    fn interrupt(&self) -> Result<()>;
    fn set_model(&self, model: &str) -> Result<()>;
    fn set_mode(&self, mode: &str) -> Result<()>;
    fn close(&self) -> Result<()>;

    fn answerer(&self) -> Option<&dyn Answerer> {
        None
    }
    fn responder(&self) -> Option<&dyn Responder> {
        None
    }
    fn asker(&self) -> Option<&dyn Asker> {
        None
    }
    fn context_reader(&self) -> Option<&dyn ContextReader> {
        None
    }
    fn task_stopper(&self) -> Option<&dyn TaskStopper> {
        None
    }
    fn backgrounder(&self) -> Option<&dyn Backgrounder> {
        None
    }
    fn homed(&self) -> Option<&dyn Homed> {
        None
    }
    fn hearer(&self) -> Option<&dyn Hearer> {
        None
    }
    /// The session holds a sign-in its profile has since left: it should
    /// rest once its turn ends, and pick up on the new one (Go's Staler).
    fn stale(&self) -> bool {
        false
    }
    /// It keeps its own quota readings where its agent reads them: the
    /// host doesn't record its Quota events again (Go's QuotaKeeper).
    fn keeps_quota(&self) -> bool {
        false
    }
    /// Why its session ended, once events has closed; None when it ended
    /// well or can't say (Go's Ender).
    fn err(&self) -> Option<super::Error> {
        None
    }
    /// Its agent's process, when it is a process of its own (Go's PIDer).
    fn pid(&self) -> Option<i32> {
        None
    }
    /// Its agent's lines reach StartOptions.tap: the caller has them there,
    /// and needn't keep its events too (Go's Tapper).
    fn taps(&self) -> bool {
        false
    }
}

/// Takes answers to a Question: each Ask's chosen labels, keyed by the
/// Ask's id, or its text when it has none.
pub trait Answerer {
    fn answer_question(&self, id: &str, answers: &HashMap<String, Vec<String>>) -> Result<()>;
}

/// Takes an approval's answer with more than an option: the call's input
/// as you changed it, in the agent's own words, and a refusal's message,
/// and whether a refusal stops the turn.
pub trait Responder {
    fn allow(&self, approval_id: &str, input: Option<&RawValue>, always: bool) -> Result<()>;
    fn deny(&self, approval_id: &str, message: &str, interrupt: bool) -> Result<()>;
}

/// Passes a request in the agent's own control protocol through for a
/// client, and waits for its reply.
pub trait Asker {
    fn ask(&self, req: &RawValue) -> Result<RawJson>;
}

/// Breaks down what fills the context window, as the agent counts it.
pub trait ContextReader {
    fn context_usage(&self) -> Result<usage::Context>;
}

/// Stops one task beside the turn, leaving the turn and the rest running.
pub trait TaskStopper {
    fn stop_task(&self, id: &str) -> Result<()>;
}

/// Moves a call the turn is waiting on into the background, so the turn
/// carries on; an empty id moves every one.
pub trait Backgrounder {
    fn background(&self, call_id: &str) -> Result<()>;
}

/// Runs in the home of a login, which a switch can leave behind. home is
/// that home; moved is whether new sessions run in another now. A moved
/// session needn't follow while its prompt cache is warm: the new login's
/// cache is empty, so its next turn would write the whole context again.
/// login is the account it started signed in as.
pub trait Homed {
    fn home(&self) -> String;
    fn moved(&self) -> bool;
    fn login(&self) -> String;
}

/// Can say whether its agent took in a message sent at since, as text:
/// None when it can't tell.
pub trait Hearer {
    fn heard(&self, text: &str, since: Ms) -> Option<bool>;
}
