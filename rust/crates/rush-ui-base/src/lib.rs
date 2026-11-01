//! What every one of rush's screens builds on: the model, the frame and
//! its extension points (frame/ext.rs: the hooks, places and slots the
//! screens fill in), the agent list, the config, the workers that read
//! the fleet, and the data the screens share. The screens themselves, and
//! the entry points that run the TUI, are in rush-ui, which makes the
//! model with them registered (rush_ui::new_model, Model::bare).

pub mod accounts;
pub mod away;
pub mod bridge;
pub mod changes;
pub mod chores;
pub mod clanker;
pub mod config;
pub mod context;
pub mod finish;
pub mod frame;
pub mod header;
pub mod host;
pub mod imagerefs;
pub mod keybind;
pub mod landing;
pub mod lead;
pub mod list;
pub mod listcache;
pub mod look;
pub mod mention;
pub mod model;
pub mod overlay;
pub mod paint;
pub mod pastes;
pub mod predict;
pub mod rooms;
pub mod sess;
pub mod session;
pub mod shells;
pub mod sidecol;
pub mod spawns;
pub mod split;
pub mod startdir;
pub mod style;
pub mod turnwants;
pub mod update;
pub mod worker;

pub use model::Model;
pub use worker::{Fleet, Note};

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
