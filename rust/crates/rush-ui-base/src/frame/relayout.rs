//! A long session laid out again for a new width (the list's edge
//! dragged, the terminal resized). Go draws the Session for at most
//! relayBudget a frame, its end first, and asks for another frame until
//! it's all drawn afresh (internal/ui/relayout.go:13 relayBudget, :21
//! relayout). Here the conversation is drawn a window at a time
//! (convo::Renderer, DESIGN.md "Drawing a window"): a new width lays out
//! only the turns in view, so no frame waits on the rest and there's no
//! follow-up frame to ask for. The budget stays, for a screen that lays
//! out more than its window.

use std::time::Duration;

/// The most a frame spends laying a session out again.
pub const RELAY_BUDGET: Duration = Duration::from_millis(6);
/// How soon the next frame comes while some is left as it was.
pub const RELAY_EVERY: Duration = Duration::from_millis(16);
