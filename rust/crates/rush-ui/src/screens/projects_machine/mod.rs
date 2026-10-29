//! The Projects place and the machine: projects, worktrees, folders,
//! processes, shells, clean-up (Go: projects.go, projecttree.go,
//! folders.go, machinecare.go, shells.go, cleanup.go).

use crate::frame::Ext;

mod data;
mod delete;
mod rows;
mod scan;
mod view;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(_x: &mut Ext) {}
