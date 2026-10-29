//! What rush remembers itself, never inside a Claude config folder, so
//! Claude Code updates cannot clobber it and it cannot corrupt Claude Code:
//! config.json and state.json under rush's folder, the cost cache, the
//! vault of sign-ins, the roster of running views, what every rush process
//! knows together about each provider's health, and editing an agent's
//! JSON settings in place.
//!
//! Every file is read and written as Go rush does, byte for byte, so the
//! two can be switched between at any point (tests/fixtures are files Go
//! wrote; gofixtures/gen.sh writes them again).
//!
//! Ported from internal/state, internal/instances, internal/health and
//! internal/settingsfile; agents and failure hold the parts of
//! internal/agent they need until that's ported.

pub mod agents;
pub mod config;
pub mod costcache;
pub mod dir;
pub mod envconfig;
pub use rush_core::failure;
pub mod health;
pub mod instances;
pub mod json;
pub mod profiles;
pub mod settingsfile;
pub mod setup;
pub mod store;
pub mod time;
pub mod vault;
pub mod writebehind;

pub use config::{Config, Dispatch, Folder, Login, SignIn, Start};
pub use costcache::CostCache;
pub use dir::{cache_dir, cache_path, dir, expand_home, key, set_thread_cache_dir, set_thread_dir};
pub use envconfig::{env_bool, env_name};
pub use profiles::{LOGINS_KIND, Pick, Profile, Room, Seat, ids};
pub use store::{
    Overlay, Store, config_on_disk, keep_before, set_on_save, write_bytes, write_json,
};
pub use time::Time;
pub use vault::{Keys, api_key, put_api_key, vault};
pub use writebehind::{flush, write_behind};

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
