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
//! withSwitches :111, migrate :129). rush-bundled registers [`bundles`].

use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use rush_plugin::rpc::{no_method, to_raw};
use rush_plugin::{Bundle, Conn, Manifest, Requires};

use crate::config::{
    Config, REACH_CLOUDFLARE, REACH_LOCAL, REACH_TAILSCALE, load_raw, save_config,
};
use crate::service;

pub const PLUGIN_REMOTE: &str = "remote";
pub const PLUGIN_TAILSCALE: &str = "remote-tailscale";
pub const PLUGIN_CLOUDFLARED: &str = "remote-cloudflared";
pub const PLUGIN_SSH: &str = "remote-ssh";

const ALL: [&str; 4] = [
    PLUGIN_REMOTE,
    PLUGIN_TAILSCALE,
    PLUGIN_CLOUDFLARED,
    PLUGIN_SSH,
];

/// The four plugins, as rush-bundled registers them.
pub fn bundles() -> Vec<Bundle> {
    let list: [(&str, &str, Requires, bool); 4] = [
        (
            PLUGIN_REMOTE,
            "Remote mode: keeps rush serve running from login on, even with rush closed, so your sessions are on your phone \
             and other devices. How they reach it is remote-tailscale's or remote-cloudflared's; Settings › Remote pairs a device.",
            Requires {
                os: vec!["darwin".into()],
                ..Requires::default()
            },
            true,
        ),
        (
            PLUGIN_TAILSCALE,
            "Lets rush serve be reached over your tailnet, with the installed tailscale: https on port 8443, \
             and SSH on 2222 with remote-ssh. Off removes only those mappings.",
            Requires::default(),
            false,
        ),
        (
            PLUGIN_CLOUDFLARED,
            "Lets rush serve's web app be reached through your own Cloudflare tunnel, with the installed cloudflared \
             and the tunnel's token in cloudflared-token in rush's folder. Off stops only that tunnel.",
            Requires {
                bin: vec!["cloudflared".into()],
                ..Requires::default()
            },
            false,
        ),
        (
            PLUGIN_SSH,
            "Rush itself, the whole terminal app, from any SSH app (Termius, Blink, ssh) with a paired key. Like tmux, \
             with no tmux: each device has its own rush, kept 30 minutes after it drops.",
            Requires::default(),
            false,
        ),
    ];
    list.into_iter()
        .map(|(name, about, req, switched)| Bundle {
            manifest: Manifest {
                name: name.into(),
                category: "integrations".into(),
                description: about.into(),
                command: vec!["rush".into()],
                requires: req,
                memory_mb: 16,
                ..Manifest::default()
            },
            optional: true,
            run: Some(Arc::new(run_switch)),
            switched: switched.then(|| {
                Arc::new(switch_remote) as Arc<dyn Fn(bool) -> Result<(), String> + Send + Sync>
            }),
        })
        .collect()
}

/// Installs serve's LaunchAgent, or removes it, as the remote plugin is
/// turned on or off.
fn switch_remote(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    service::set_remote(on, &exe.to_string_lossy())
}

/// A switch on its connection to the broker: it has nothing to do but be
/// on.
fn run_switch(s: UnixStream) -> Result<(), String> {
    let conn = Conn::new(
        s,
        Some(Arc::new(|_, _, method, _| match method {
            "initialize" => to_raw(&serde_json::Map::new()).map(Some),
            "tools.list" => to_raw(&serde_json::json!({ "tools": [] })).map(Some),
            _ => Err(no_method(method)),
        })),
    )
    .map_err(|e| e.to_string())?;
    conn.wait();
    Ok(())
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
