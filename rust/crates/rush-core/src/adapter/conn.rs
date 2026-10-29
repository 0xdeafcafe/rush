//! A running session, and what a session rush hosts can do beyond it.
//! After Go's internal/agent: adapter.go (Conn, Answerer) and conn.go.
//!
//! Each optional part is a Go interface a Conn has only when its agent
//! can; here a method on [`Conn`] hands it out, None by default. The host
//! offers an op only to a session whose Conn has it.
//!
//! Go's Conn hands out a channel of events, read by a goroutine per
//! session. Here the caller gives Driver::start a [`Sink`], and the
//! adapter's own reader thread pushes into it: N sessions need no
//! forwarding threads, and one channel (a [`Tagged`] sink per session)
//! can carry them all.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use serde_json::value::RawValue;

use super::{Input, Result};
use crate::event::Event;
use crate::time::Ms;
use crate::usage;
use crate::wire::RawJson;

/// Where a running session's events go (Go's Events channel). The
/// adapter's reader thread calls it, in order; ended is called once, last.
///
/// Neither may block: they run on the adapter's only reader thread, and a
/// sink that waits (on a full bounded channel, a lock the UI holds, a
/// rush-draw lane's `send`) stalls the agent itself, whose pipe then
/// fills. A sink hands each event on at once: into an unbounded queue
/// (as [`Tagged`] does), or with rush-draw's `Sender::try_send`, keeping
/// what a full lane hands back on its own side (coalesced or buffered) to
/// send with the next.
pub trait Sink: Send + Sync {
    /// One thing the session said.
    fn event(&self, ev: Event);
    /// The session has ended: err is why, None when it ended well or the
    /// agent can't say (Go closing Events, then its Ender).
    fn ended(&self, err: Option<super::Error>);
}

/// What a [`Tagged`] sink sends: its session's key, and what it said.
#[derive(Debug)]
pub enum Said<K> {
    Event(K, Event),
    Ended(K, Option<super::Error>),
}

/// A Sink that sends into one channel shared by many sessions, each event
/// tagged with its session's key: the host's single queue.
pub struct Tagged<K> {
    pub key: K,
    pub tx: Sender<Said<K>>,
}

impl<K: Clone + Send + Sync> Sink for Tagged<K> {
    fn event(&self, ev: Event) {
        let _ = self.tx.send(Said::Event(self.key.clone(), ev));
    }
    fn ended(&self, err: Option<super::Error>) {
        let _ = self.tx.send(Said::Ended(self.key.clone(), err));
    }
}

/// A running session. Any thread may call it, at once: the host answers
/// an approval while another thread sends.
pub trait Conn: Send + Sync {
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
    /// Its agent's process, when it is a process of its own (Go's PIDer).
    fn pid(&self) -> Option<i32> {
        None
    }
    /// Its agent's lines reach StartOptions.tap: the caller has them there,
    /// and needn't keep its events too (Go's Tapper).
    fn taps(&self) -> bool {
        false
    }
    /// Ends the background tasks its agent's own records say have ended,
    /// though no word of it came over its connection, and says which (Go's
    /// `interface{ EndTasks() []string }`, which the host asks of a Conn:
    /// Kimi's).
    fn end_tasks(&self) -> Vec<String> {
        Vec::new()
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
