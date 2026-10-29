//! The Session: a hosted conversation's message box, sending, interrupting,
//! its views and its own keys (Go: sending.go, sendmode.go, live.go,
//! hosted.go, sessionviews.go, stream.go, the pane's keys).
//! Empty until its group fills it.

use crate::frame::Ext;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(_x: &mut Ext) {}
