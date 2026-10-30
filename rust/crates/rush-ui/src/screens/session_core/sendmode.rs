//! How enter sends to a session that's working: into its queue, into the
//! turn under way, stopping it first, or into a room. Each session keeps
//! its own; ctrl+t goes round what its agent can do. After Go's
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
    /// enter opens a room on what's typed, not sending it.
    Discuss,
}

impl SendMode {
    /// What enter does in this mode, as the flash says it.
    pub fn words(self) -> &'static str {
        match self {
            SendMode::Queue => "queues",
            SendMode::Guide => "guides: it reads it at its next step",
            SendMode::Stop => "stops it and sends",
            SendMode::Discuss => "opens a room on what's typed: its verdict comes back here",
        }
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            SendMode::Queue => "queue",
            SendMode::Guide => "guide",
            SendMode::Stop => "stop & send",
            SendMode::Discuss => "discuss",
        }
    }

    /// How enter sends now, on the box's edge: guide is what the box calls
    /// coalescing.
    pub fn state(self) -> &'static str {
        match self {
            SendMode::Queue => "queuing",
            SendMode::Guide => "coalescing",
            SendMode::Stop => "stop & send",
            SendMode::Discuss => "discussing",
        }
    }

    /// The mode a switch goes to, named on the edge.
    pub fn switch(self) -> &'static str {
        match self {
            SendMode::Queue => "queue",
            SendMode::Guide => "coalesce",
            SendMode::Stop => "stop & send",
            SendMode::Discuss => "discuss",
        }
    }

    /// What enter does, beside it in the hints.
    pub fn key_words(self) -> &'static str {
        match self {
            SendMode::Queue => "queue it",
            SendMode::Guide => "guide it",
            SendMode::Stop => "stop & send",
            SendMode::Discuss => "discuss it",
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
    out.push(SendMode::Discuss);
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
    if next == SendMode::Discuss {
        return format!("enter {}", next.words());
    }
    format!("enter {} while it works", next.words())
}

/// What the Session box's hints say of sending, as (key, what) pairs, in
/// order (boxKeys without the stash, which is the stash plugin's).
pub struct BoxState {
    /// A turn is running in a hosted session.
    pub live: bool,
    /// It works: live, or (not hosted) its row says so.
    pub working: bool,
    pub typed: bool,
    pub queued: bool,
    /// The way picked, None for none picked.
    pub picked: Option<SendMode>,
    pub mode: SendMode,
    /// The box was wiped lately.
    pub cleared: bool,
}

pub const SEND_NOW: &str = "ctrl+enter";
pub const SEND_MODE: &str = "ctrl+t";
pub const STOP: &str = "ctrl+x";
/// The keys that bring back what you cleared (stash.go:186).
pub const UNDO_HINT: &str = "cmd+z or ctrl+/";

pub fn box_keys(b: &BoxState) -> Vec<(&'static str, &'static str)> {
    let mut pairs = Vec::new();
    if b.typed && b.picked == Some(SendMode::Discuss) {
        pairs.push(("enter", "discuss it in a room"));
    } else if b.typed && b.live {
        pairs.push(("enter", b.mode.key_words()));
        pairs.push((SEND_NOW, "send now"));
    } else if b.typed && b.working {
        pairs.push(("enter", "queue it"));
        pairs.push((SEND_NOW, "send now"));
    } else if b.typed {
        pairs.push(("enter", "send"));
    } else if b.queued {
        pairs.push((SEND_NOW, "send the queue now"));
    }
    if b.live {
        pairs.push((SEND_MODE, "queue · guide · stop · discuss"));
    }
    if b.working {
        pairs.push((STOP, "stop it"));
    }
    if b.cleared {
        pairs.push((UNDO_HINT, "bring back what you cleared"));
    }
    pairs
}
