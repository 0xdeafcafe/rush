//! How enter sends to a session that's working: into its queue, into the
//! turn under way, or stopping it first (Go's fourth, into a room, went
//! with rooms: Rust-only). Each session keeps its own; ctrl+t goes round what its agent can do. After Go's
//! internal/ui/sendmode.go (sendModes :28, sendModeOf :41, nextSendMode
//! :52, cycleSendMode :65, the words :79-91, sendModeTop :95, boxKeys
//! :105, sendModeKeys :149).

use rush_core::adapter::{Feature, Registry};

/// One way enter sends while the session works.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SendMode {
    #[default]
    Queue,
    Guide,
    Stop,
}

impl SendMode {
    /// What enter does in this mode, as the flash says it.
    pub fn words(self) -> &'static str {
        match self {
            SendMode::Queue => "queues",
            SendMode::Guide => "guides: it reads it at its next step",
            SendMode::Stop => "stops it and sends",
        }
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            SendMode::Queue => "queue",
            SendMode::Guide => "guide",
            SendMode::Stop => "stop & send",
        }
    }

    /// What enter does now, on the box's edge, said as that. Not Go's
    /// words ("queuing", and "coalescing" for guide, which named how the
    /// host batches what's typed and told nobody what enter would do).
    pub fn state(self) -> &'static str {
        match self {
            SendMode::Queue => "enter queues it for when it's done",
            SendMode::Guide => "enter guides it at its next step",
            SendMode::Stop => "enter stops it and sends",
        }
    }

    /// The mode a switch goes to, named on the edge.
    pub fn switch(self) -> &'static str {
        match self {
            SendMode::Queue => "queue",
            SendMode::Guide => "guide",
            SendMode::Stop => "stop & send",
        }
    }

    /// What enter does, beside it in the hints.
    pub fn key_words(self) -> &'static str {
        match self {
            SendMode::Queue => "queue it",
            SendMode::Guide => "guide it",
            SendMode::Stop => "stop & send",
        }
    }
}

/// The ways an agent of kind takes a message mid-turn.
pub fn modes(reg: &Registry, kind: &str) -> Vec<SendMode> {
    let mut out = vec![SendMode::Queue];
    if reg.supports(kind, Feature::Guide) {
        out.push(SendMode::Guide);
    }
    if reg.supports(kind, Feature::Interrupt) {
        out.push(SendMode::Stop);
    }
    out
}

/// How enter sends now: the pick, while the agent can.
pub fn mode_of(picked: Option<SendMode>, modes: &[SendMode]) -> SendMode {
    match picked {
        Some(p) if modes.contains(&p) => p,
        _ => SendMode::Queue,
    }
}

/// The way after the current one, round.
pub fn next_mode(picked: Option<SendMode>, modes: &[SendMode]) -> SendMode {
    let cur = mode_of(picked, modes);
    let mut next = modes[0];
    for (i, k) in modes.iter().enumerate() {
        if *k == cur {
            next = modes[(i + 1) % modes.len()];
        }
    }
    next
}

/// The flash after ctrl+t picked next.
pub fn cycled(next: SendMode) -> String {
    format!("enter {} while it works", next.words())
}

/// What the Session box's hints say of sending, as (key, what) pairs, in
/// order (boxKeys, the stash's keys with them).
#[derive(Clone, Copy, Debug, Default)]
pub struct BoxState {
    /// A turn is running in a hosted session.
    pub live: bool,
    /// It works: live, or (not hosted) its row says so.
    pub working: bool,
    pub typed: bool,
    pub queued: bool,
    pub mode: SendMode,
    /// Its agent takes a message mid-turn more ways than one: ctrl+t
    /// goes round them.
    pub ways: bool,
    /// The box was wiped lately.
    pub cleared: bool,
    /// esc with nothing typed stops the turn under way (canInterrupt).
    pub esc_stops: bool,
    /// The stash runs: what the aside key does to what's typed ("set
    /// aside", "keep as a draft"); "" when it doesn't.
    pub aside: &'static str,
    /// What it did, said of what comes back ("set aside").
    pub aside_did: &'static str,
    /// This box has something set aside, to come back.
    pub has_aside: bool,
}

pub const SEND_NOW: &str = "ctrl+enter";
pub const SEND_MODE: &str = "ctrl+t";
pub const STOP: &str = "ctrl+x";
/// The keys that bring back what you cleared (stash.go:186).
pub const UNDO_HINT: &str = "cmd+z or ctrl+/";
/// Stand-ins for the stash's keys, which the dock names as bound.
pub const ASIDE: &str = "ctrl+p";
pub const HISTORY: &str = "ctrl+r";
/// The one word for what ctrl+r opens, in the hints and the flashes.
pub const HISTORY_WORD: &str = "history";

/// The box's own keys under it, in a fixed order: what enter does now,
/// then the (at most) two keys that matter most in this state. The way
/// back and f1 follow them (the dock's). Not Go's boxKeys, which listed
/// every key that applied and left the row to cut the rest, so the keys
/// for what's set aside or cleared were the first to go (Rust-only).
pub fn box_keys(b: &BoxState) -> Vec<(&'static str, String)> {
    let mut pairs: Vec<(&'static str, String)> = Vec::new();
    if b.typed {
        let enter = if b.live {
            b.mode.key_words()
        } else if b.working {
            "queue it"
        } else {
            "send"
        };
        pairs.push(("enter", enter.into()));
    }
    let mut more: Vec<(&'static str, String)> = Vec::new();
    let on = !b.aside.is_empty();
    if b.typed {
        if b.working {
            more.push((SEND_NOW, "send now".into()));
        }
        if b.live && b.ways {
            // Not Go's "queue · guide · stop · discuss", four names with
            // nothing to say which is on or what the key does to them:
            // the box's edge says what enter does now and what ctrl+t
            // makes it. Rust-only: not said of an agent that only queues.
            more.push((SEND_MODE, "what enter does while it works".into()));
        }
        if on {
            more.push((ASIDE, b.aside.into()));
        }
        if b.working {
            more.push((STOP, "stop it".into()));
        }
    } else {
        if b.cleared {
            more.push((UNDO_HINT, "bring back what you cleared".into()));
        }
        if b.has_aside && on {
            more.push((ASIDE, format!("bring back what you {}", b.aside_did)));
        }
        if b.queued {
            more.push((SEND_NOW, "send the queue now".into()));
        }
        // Chosen before you type, as well as after.
        if b.live && b.ways {
            more.push((SEND_MODE, "what enter does while it works".into()));
        }
        // esc stops a live turn and says so on the row; ctrl+x stops one
        // esc can't.
        if b.working && !b.esc_stops {
            more.push((STOP, "stop it".into()));
        }
    }
    if on {
        more.push((HISTORY, HISTORY_WORD.into()));
    }
    pairs.extend(more.into_iter().take(2));
    pairs
}

/// What esc does now, said beside it on the hint row: it clears what's
/// typed first, then stops the turn, then leaves (paneKey's esc). Go
/// said "back" whatever it would do (Rust-only).
pub fn esc_word(b: &BoxState, alone: bool) -> &'static str {
    if b.typed {
        "clear"
    } else if b.esc_stops {
        "stop the turn"
    } else if alone {
        "leave the box"
    } else {
        "back"
    }
}
