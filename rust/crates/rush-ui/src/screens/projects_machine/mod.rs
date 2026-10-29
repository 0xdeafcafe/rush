//! The Projects place and the machine: projects, worktrees, folders,
//! processes, clean-up, and the machine's own care. Go: internal/ui/
//! projects.go, projects_paths.go, projecttree.go, folders.go, prs.go,
//! tempwork.go, cleanup.go, cleansheet.go, deletesheet.go,
//! machinecare.go, workstreams.go, shells.go, memory.go, memcard.go (each file's header names the lines).

use crate::frame::{Ext, PlaceId};
use crate::model::Model;

mod care;
mod clean;
mod data;
mod delete;
pub mod memory;
mod place;
mod rows;
mod scan;
pub mod shells;
mod tidy;
mod view;

pub use care::{CareStart, CareStarter, StartCare};
pub use place::{pick_in_projects, projects_sel};

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    x.place(PlaceId::Projects, Box::new(place::Projects::default()));
    x.on_fleet.push(on_fleet);
}

/// Each reading of the list: what's due asked again while Projects shows,
/// and Clean up suggested when a lot could go (model.go:1097 tick).
fn on_fleet(m: &mut Model) {
    scan::refresh(m);
    // What agents left running is looked for with every reading, wherever
    // rush is (snapload.go:86), only with a loop to end it off the UI.
    if m.sender().is_some() {
        scan::read_machine(m);
    }
    clean::nudge_clean(m);
    care::watch_machine(m);
}
