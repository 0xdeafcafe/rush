//! The bundled plugins that are serve's switches: remote keeps serve
//! running (remote mode), remote-tailscale and remote-cloudflared say how
//! other devices reach it, and remote-ssh lets in rush itself over SSH.
//! Each is off until you turn it on, from Settings › Remote or Settings ›
//! Plugins. Only remote has anything to do as it's switched (it installs
//! or removes serve's LaunchAgent); serve reads the rest every few seconds
//! and does the work itself.
//!
//! After Go's internal/remote/plugins.go (Plugin* :22, init :29, switchRemote
//! :55, runSwitch :65, switches :82, PluginOn :89, SwitchesChanged :103,
//! withSwitches :111, migrate :129). The plugins themselves are
//! rush-bundled's (its remote.rs), which sits below this crate.

use std::collections::BTreeMap;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use crate::config::{
    Config, REACH_CLOUDFLARE, REACH_LOCAL, REACH_TAILSCALE, load_raw, save_config,
};
use crate::service;

pub use rush_bundled::remote::{PLUGIN_CLOUDFLARED, PLUGIN_REMOTE, PLUGIN_SSH, PLUGIN_TAILSCALE};

const ALL: [&str; 4] = [
    PLUGIN_REMOTE,
    PLUGIN_TAILSCALE,
    PLUGIN_CLOUDFLARED,
    PLUGIN_SSH,
];

/// Installs serve's LaunchAgent, or removes it, as the remote plugin is
/// turned on or off: what rush-bundled's remote plugin runs as it's
/// switched (rush_bundled::remote::on_switch).
pub fn switch_remote(on: bool) -> Result<(), String> {
    // This rush as installed: the LaunchAgent outlives it (a Homebrew
    // upgrade removes the old version's folder).
    let exe = rush_sys::selfexe::resolved().ok_or("can't find the running rush")?;
    service::set_remote(on, &exe.to_string_lossy())
}

/// The switches as last read: serve asks on every request, and they're
/// read again at most every 2s.
static SWITCHES: Mutex<Option<(Instant, BTreeMap<&'static str, bool>)>> = Mutex::new(None);

/// Whether one of serve's plugins is on.
pub fn plugin_on(name: &str) -> bool {
    let mut s = SWITCHES.lock().unwrap_or_else(|e| e.into_inner());
    if s.as_ref()
        .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2))
    {
        let on = ALL
            .into_iter()
            .map(|n| (n, rush_plugin::bundled_on(n)))
            .collect();
        *s = Some((Instant::now(), on));
    }
    s.as_ref()
        .and_then(|(_, on)| on.get(name).copied())
        .unwrap_or(false)
}

/// Has the next plugin_on read the switches again.
pub fn switches_changed() {
    *SWITCHES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// c with reach and ssh as the plugins say: remote.json's own are only
/// read once, by migrate.
pub fn with_switches(mut c: Config) -> Config {
    c.ssh = plugin_on(PLUGIN_SSH);
    c.reach = if plugin_on(PLUGIN_TAILSCALE) {
        REACH_TAILSCALE
    } else if plugin_on(PLUGIN_CLOUDFLARED) {
        REACH_CLOUDFLARE
    } else {
        REACH_LOCAL
    }
    .into();
    c
}

static MIGRATED: Once = Once::new();

/// Moves what remote.json said before serve's switches were plugins into
/// them, once: its reach and ssh, and remote mode found running.
pub fn migrate() {
    MIGRATED.call_once(|| {
        let Ok(mut c) = load_raw() else {
            return;
        };
        if c.reach != REACH_LOCAL || c.ssh {
            if c.reach == REACH_TAILSCALE {
                let _ = rush_plugin::mark_bundled(PLUGIN_TAILSCALE, true);
            }
            if c.reach == REACH_CLOUDFLARE {
                let _ = rush_plugin::mark_bundled(PLUGIN_CLOUDFLARED, true);
            }
            if c.ssh {
                let _ = rush_plugin::mark_bundled(PLUGIN_SSH, true);
            }
            c.reach = REACH_LOCAL.into();
            c.ssh = false;
            let _ = save_config(&c);
        }
        if service::remote_on() && !rush_plugin::bundled_on(PLUGIN_REMOTE) {
            let _ = rush_plugin::mark_bundled(PLUGIN_REMOTE, true);
        }
        switches_changed();
    });
}
