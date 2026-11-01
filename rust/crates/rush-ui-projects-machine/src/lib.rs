//! The Projects place and the machine: projects, worktrees, folders,
//! processes, clean-up, and the machine's own care. Go: internal/ui/
//! projects.go, projects_paths.go, projecttree.go, folders.go, prs.go,
//! tempwork.go, cleanup.go, cleansheet.go, deletesheet.go,
//! machinecare.go, workstreams.go, shells.go, memory.go, memcard.go,
//! memreview.go, projectmem.go (each file's header names the lines).

// The base's modules, so crate::frame::.. and the like read as they did
// in rush-ui. Each is the one module, not a copy.
use rush_ui_base::{config, frame, header, host, list, look, model, sess, session, style, worker};

use crate::frame::{Ext, PlaceId};
use crate::model::Model;

#[doc(hidden)]
pub mod care;
#[doc(hidden)]
pub mod clean;
pub mod data;
pub mod delete;
pub mod memory;
pub mod memreview;
pub mod place;
#[doc(hidden)]
pub mod projectmem;
pub mod prsheet;
#[doc(hidden)]
pub mod rows;
#[doc(hidden)]
pub mod scan;
pub mod scripts;
pub use rush_ui_base::shells;
#[doc(hidden)]
pub mod temp;
#[doc(hidden)]
pub mod tidy;
pub mod tidydone;
#[doc(hidden)]
pub mod view;

pub use care::{
    CareStart, CareStarter, StartCare, care_started, start_with_hosts as care_start_with_hosts,
};
pub use data::{PANE_PRS, PANE_SYSTEM, PANE_TEMP};
pub use place::{pick_in_projects, projects_sel};
pub use view::gh_pr_line;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    x.place(PlaceId::Projects, Box::new(place::Projects::default()));
    x.on_fleet.push(on_fleet);
}

/// Each reading of the list: what's due asked again while Projects shows
/// (model.go:1097 tick). Clean up is suggested once the worktrees are
/// measured (scan::on_worktrees).
fn on_fleet(m: &mut Model) {
    scan::refresh(m);
    scan::refresh_open_prs(m);
    // What agents left running is looked for with every reading, wherever
    // rush is (snapload.go:86), only with a loop to end it off the UI.
    if m.sender().is_some() {
        scan::read_machine(m);
    }
    care::watch_machine(m);
}

/// Nothing asked of git, GitHub, the disk or the process table, so a
/// frame of Projects is the same on every machine (snapshot tests).
#[doc(hidden)]
pub fn freeze(m: &mut Model) {
    m.kept::<data::Data>().frozen = true;
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

/// Whether the machine's processes have been read yet.
pub fn machine_read(m: &Model) -> bool {
    data::data(m).machine_at != 0
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

/// An agent's temp work as Projects measured it, None before it has.
pub fn temp_known(m: &Model, key: &str) -> Option<i64> {
    data::data(m).temp_known(key)
}

/// How much temp work is worth showing (tempShown).
pub const TEMP_SHOWN: i64 = data::TEMP_SHOWN;

#[cfg(test)]
rush_testenv::sandboxed!();
