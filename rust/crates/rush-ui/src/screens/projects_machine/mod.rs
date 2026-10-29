//! The Projects place and the machine: projects, worktrees, folders,
//! processes, clean-up, and the machine's own care. Go: internal/ui/
//! projects.go, projects_paths.go, projecttree.go, folders.go, prs.go,
//! tempwork.go, cleanup.go, cleansheet.go, deletesheet.go,
//! machinecare.go, workstreams.go, shells.go, memory.go, memcard.go,
//! memreview.go, projectmem.go (each file's header names the lines).

use crate::frame::{Ext, PlaceId};
use crate::model::Model;

mod care;
mod clean;
mod data;
pub(crate) mod delete;
pub mod memory;
pub mod memreview;
mod place;
mod projectmem;
pub(crate) mod prsheet;
mod rows;
mod scan;
pub mod scripts;
pub mod shells;
mod temp;
mod tidy;
pub(crate) mod tidydone;
mod view;

pub use care::{
    CareStart, CareStarter, StartCare, care_started, start_with_hosts as care_start_with_hosts,
};
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

/// Settings › General's Hibernate after, in minutes, as last read with
/// the settings (0 never; hibernate, model.go:2036).
pub fn hibernate_after(m: &Model) -> i64 {
    data::data(m).cfg.hibernate_minutes
}

/// The machine's agent processes as last read (snap.Machine): the header's
/// top bar reads its totals.
pub fn machine(m: &Model) -> &rush_fleet::Machine {
    &data::data(m).machine
}

/// Reads the machine now if it's due: `--render`, which has no loop to
/// read it with each reading of the list.
pub fn read_machine(m: &mut Model) {
    scan::read_machine(m);
}

/// Reads the machine's agent processes again now, however recently they
/// were read: `rush --render`'s second reading, its CPU measured from the
/// first (render.go:17).
pub fn read_machine_again(m: &mut Model) {
    scan::read_machine_again(m);
}

/// An agent's temp work as last measured, 0 before (Agent.Temp).
pub fn temp_of(m: &Model, key: &str) -> i64 {
    data::data(m).temp_of(key)
}

/// How much temp work is worth showing (tempShown).
pub const TEMP_SHOWN: i64 = data::TEMP_SHOWN;

#[cfg(test)]
mod tests;
