//! Plugins on screen: their sheets, approvals, picks and views (Go:
//! pluginui.go, pluginsheet.go, pluginapproval.go, pluginpick.go, view.go,
//! viewblocks*.go).
//! Empty until its group fills it.

use crate::frame::Ext;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(_x: &mut Ext) {}
