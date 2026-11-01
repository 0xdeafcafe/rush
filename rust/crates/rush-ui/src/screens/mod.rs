//! The screens ported on top of the frame, one module per group of them.
//! A group fills its own module (or turns it into a folder with a
//! mod.rs) and adds what it has through its `register`, which the frame
//! calls once as each model is made. Nothing here edits the frame: see
//! frame/ext.rs and DESIGN.md "Screens".

use crate::chores::{Chore, always};
use crate::frame::Ext;
use crate::frame::ext::{Act, Calls};
use crate::model::Model;

pub mod accounts;
pub mod bars;
pub mod cmdbar;
pub mod list_extras;
pub mod plugin_ui;
pub mod projects_machine;
pub mod session_cards;
pub mod session_core;
pub mod settings;
pub mod social;

/// Every group's screens, registered in this order.
pub(crate) fn register(x: &mut Ext) {
    session_cards::register(x);
    session_core::register(x);
    list_extras::register(x);
    cmdbar::register(x);
    projects_machine::register(x);
    settings::register(x);
    accounts::register(x);
    plugin_ui::register(x);
    social::register(x);
    // After the Session's own: the agent header goes over its rows.
    bars::register(x);
    session_core::register_last(x);
    x.calls = calls();
    x.act = Some(act);
    x.chores = chores();
}

/// What the frame asks of the screens by name (frame/ext.rs Calls).
fn calls() -> Calls {
    Calls {
        bar_open: Some(cmdbar::bar::is_open),
        key_help_open: Some(cmdbar::keyhelp::is_open),
        is_hash_cmd: Some(cmdbar::fleetslash::is_hash_cmd),
        legacy_command: Some(cmdbar::fleetslash::legacy_command),
        alias_of: Some(cmdbar::fleetslash::alias_of),
        grid_key: Some(list_extras::grid::key),
        grid_mouse: Some(list_extras::grid::mouse),
        grid_keys: Some(|m| list_extras::grid::grid(m).keys.clone()),
        grid_park: Some(list_extras::grid::park),
        grid_adopt: Some(list_extras::grid::adopt),
        leave_pane: Some(list_extras::split::leave_pane),
        preview: Some(list_extras::split::preview),
        leave_chat: Some(list_extras::split::leave_chat),
        renaming: Some(list_extras::rename::renaming),
        sub_toggle: Some(list_extras::subs::toggle),
        sub_rows: Some(list_extras::subs::rows),
        sub_row_of: Some(list_extras::subs::row_of),
        predict_moved: Some(list_extras::predict::moved),
        summary_strip: Some(list_extras::live::summary_strip),
        spin: Some(list_extras::wall::spin),
        intervene_ready: Some(session_cards::discuss::intervene_ready),
        dirty_path: Some(session_core::memedit::dirty_path),
        prime_spawns: Some(session_core::subs::prime_spawns),
        resized: Some(settings::resized),
        sub_line_fade: Some(settings::interface::sub_line_fade),
        pane_head: Some(social::rooms::pane_head),
        room_line: Some(social::rooms::room_line),
        room_member_line: Some(social::rooms::room_member_line),
        row_note: Some(accounts::guard::row_note),
        guard_header: Some(accounts::guard::header_line),
        limits_fetched: Some(bars::fetch::fetched_once),
        machine_read: Some(projects_machine::machine_read),
        plugin_status: Some(plugin_ui::hooks::plugin_status),
        view_summary: Some(|m, a| {
            plugin_ui::views::read::summary_of(m, a)
                .map(|v| plugin_ui::views::view_summary_short(&v))
        }),
        emit_box: Some(plugin_ui::hooks::emit_box),
        cleared: Some(plugin_ui::hooks::cleared),
    }
}

/// Does what the frame has a screen do (Model::act), at once.
fn act(m: &mut Model, a: Act) {
    match a {
        Act::OpenKeyHelp => cmdbar::keyhelp::open(m),
        Act::OpenRoom(key) => social::rooms::open_room(m, &key),
        Act::PickInProjects(dir) => projects_machine::pick_in_projects(m, &dir),
    }
}

/// What rush does by itself on Go's tick (chores.rs), in the order run.
fn chores() -> Vec<Chore> {
    vec![
        Chore {
            at: 3,
            every: 30,
            lead_only: false,
            when: always,
            run: settings::updates::watch_binary,
        },
        // The battery and disk, while the last frame showed them.
        Chore {
            at: 3,
            every: 30,
            lead_only: false,
            when: always,
            run: crate::chores::read_sysinfo,
        },
        Chore {
            at: 15,
            every: 60,
            lead_only: true,
            when: always,
            run: projects_machine::tidydone::settle_tick,
        },
        Chore {
            at: 30,
            every: 600,
            lead_only: true,
            when: always,
            run: projects_machine::tidydone::tidy_tick,
        },
        Chore {
            at: 20,
            every: 30,
            lead_only: true,
            when: always,
            run: list_extras::stuck::nudge_stuck_subs,
        },
        // The rooms, every second while one runs: Go reads them on each
        // reading, which its tick makes every second (model.go:1089-1092,
        // snapload.go:60), so a room's round and verdict show though no
        // session changed. Read off the UI thread, drawn only when changed.
        Chore {
            at: 1,
            every: 1,
            lead_only: false,
            when: social::rooms::any_running,
            run: social::rooms::tick_rooms,
        },
    ]
}

#[cfg(test)]
mod tests {
    // Every slot the frame calls through is filled by register.
    #[test]
    fn every_call_registered() {
        let m = crate::model::Model::new("t");
        assert_eq!(m.ext.calls.missing(), Vec::<&str>::new());
        assert!(m.ext.act.is_some());
        assert_eq!(m.ext.chores.len(), 6);
    }
}
