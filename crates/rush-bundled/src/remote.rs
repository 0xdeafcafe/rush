//! The bundled plugins that are rush serve's switches: remote keeps serve
//! running (remote mode), remote-tailscale and remote-cloudflared say how
//! other devices reach it, and remote-ssh lets in rush itself over SSH.
//! Each is off until you turn it on, from Settings › Remote or Settings ›
//! Plugins. Only remote has anything to do as it's switched (it installs or
//! removes serve's LaunchAgent: rush-remote's, handed in by the binary with
//! [`on_switch`], as rush-remote sits above this crate); serve reads the
//! rest every few seconds and does the work itself.
//!
//! After Go's internal/remote/plugins.go (Plugin* :22, init :29,
//! switchRemote :55, runSwitch :65).

use std::sync::{Arc, OnceLock};

use rush_plugin::{Bundle, Manifest, Requires};

pub const PLUGIN_REMOTE: &str = "remote";
pub const PLUGIN_TAILSCALE: &str = "remote-tailscale";
pub const PLUGIN_CLOUDFLARED: &str = "remote-cloudflared";
pub const PLUGIN_SSH: &str = "remote-ssh";

/// What turning remote mode on or off does.
pub type Switch = fn(bool) -> Result<(), String>;

static SWITCH: OnceLock<Switch> = OnceLock::new();

/// Hands in what turning remote mode on or off does (rush-remote's
/// service::switch_remote): the binary does, as it starts.
pub fn on_switch(f: Switch) {
    let _ = SWITCH.set(f);
}

fn switch_remote(on: bool) -> Result<(), String> {
    match SWITCH.get() {
        Some(f) => f(on),
        None => Err("remote mode isn't switched by this rush".into()),
    }
}

/// The four, as rush registers them.
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
            run: Some(Arc::new(crate::run_switch)),
            switched: switched.then(|| {
                Arc::new(switch_remote) as Arc<dyn Fn(bool) -> Result<(), String> + Send + Sync>
            }),
        })
        .collect()
}
