//! What waits in a Session's dock: permission cards, questions, decisions,
//! usage limits as a modal, the finish card (Go: permcard.go, question.go,
//! decisions, modal.go's cards, finish.go, handoff_ready.go).
//! Empty until its group fills it.

use crate::frame::Ext;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(_x: &mut Ext) {}
