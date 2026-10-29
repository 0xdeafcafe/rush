//! The screens ported on top of the frame, one module per group of them.
//! A group fills its own module (or turns it into a folder with a
//! mod.rs) and adds what it has through its `register`, which the frame
//! calls once as each model is made. Nothing here edits the frame: see
//! frame/ext.rs and DESIGN.md "Screens".

use crate::frame::Ext;

pub mod accounts;
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
    session_core::register(x);
    session_cards::register(x);
    list_extras::register(x);
    cmdbar::register(x);
    projects_machine::register(x);
    settings::register(x);
    accounts::register(x);
    plugin_ui::register(x);
    social::register(x);
}
