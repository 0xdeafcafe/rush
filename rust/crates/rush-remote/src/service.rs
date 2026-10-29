//! Remote mode: rush serve kept running by the system, from login on and
//! after a crash, so this machine's sessions are in reach of your devices
//! whether or not rush is open. On a Mac that's a LaunchAgent of serve's
//! own; elsewhere it's yours to run. And asking this machine's serve
//! things, with its token.
//!
//! After Go's internal/remote/service.go (serviceLabel :25, servicePlist
//! :27, ServiceLog :33, RemoteOn :36, SetRemote :46, LocalPairing :107,
//! Up :114) and devices.go (LocalDevices :264, LocalRevoke :271, localCall
//! :277).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::config::{dir, load_config, load_raw, token, update_config};
use crate::devices::Device;
use crate::pair::Pairing;
use crate::plugins::{PLUGIN_CLOUDFLARED, PLUGIN_SSH, PLUGIN_TAILSCALE, switches_changed};

pub const SERVICE_LABEL: &str = "com.github.0xdeafcafe.rush.serve";

fn service_plist() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/LaunchAgents")
        .join(format!("{SERVICE_LABEL}.plist"))
}

/// Where the kept serve writes.
pub fn service_log() -> PathBuf {
    dir().join("serve.log")
}

/// Whether remote mode is on: serve is kept running.
pub fn remote_on() -> bool {
    service_plist().exists()
}

/// s with HTML's five escaped, as html.EscapeString does.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
}

/// The LaunchAgent that keeps exe's serve running.
pub fn plist(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string><string>serve</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>PATH</key><string>{}</string>
    <key>RUSH_HOME</key><string>{}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict></plist>
"#,
        esc(exe),
        esc(&std::env::var("PATH").unwrap_or_default()),
        esc(&dir().to_string_lossy()),
        esc(&service_log().to_string_lossy()),
        esc(&service_log().to_string_lossy()),
    )
}

/// Turns remote mode on (serve kept running by exe; reached over
/// Tailscale, turning remote-tailscale on, unless it's reached some other
/// way already, letting in the web app and SSH) or off (serve stopped,
/// which takes its tailnet mapping down). Off leaves remote.json's choices
/// as they were, for the next time.
pub fn set_remote(on: bool, exe: &str) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("remote mode keeps serve running on macOS; here, run rush serve as a service of your own".into());
    }
    rush_core::sandbox::guard("serve's LaunchAgent", &service_plist());
    if rush_core::sandbox::active() {
        // A test never reaches the user's launchd.
        return Err("remote mode isn't switched in a test's sandbox".into());
    }
    // SAFETY: getuid only asks.
    let uid = unsafe { libc::getuid() };
    let plist_path = service_plist();
    // Not loaded is fine.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{SERVICE_LABEL}")])
        .output();
    if !on {
        if let Ok(cfg) = load_config() {
            // launchd may stop serve before it takes it down itself.
            crate::expose::unmap_tailscale(&cfg);
        }
        return match std::fs::remove_file(&plist_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    // Reached some way, and letting something in: over Tailscale, with the
    // web app and SSH, unless you chose otherwise before.
    if !rush_plugin::bundled_on(PLUGIN_TAILSCALE) && !rush_plugin::bundled_on(PLUGIN_CLOUDFLARED) {
        rush_plugin::set_bundled(PLUGIN_TAILSCALE, true)?;
    }
    if !load_raw().unwrap_or_default().web && !rush_plugin::bundled_on(PLUGIN_SSH) {
        update_config(|c| c.web = true)?;
        rush_plugin::set_bundled(PLUGIN_SSH, true)?;
    }
    switches_changed();
    if let Some(d) = plist_path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&plist_path, plist(exe)).map_err(|e| e.to_string())?;
    let out = Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{uid}")])
        .arg(&plist_path)
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if !out.status.success() {
        let mut b = out.stdout;
        b.extend_from_slice(&out.stderr);
        return Err(format!(
            "launchctl: exit status {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&b).trim()
        ));
    }
    Ok(())
}

const NOT_ANSWERING: &str = "rush serve isn't answering: turn Remote mode on";

/// One request to this machine's serve, with its token.
fn local_call<T: DeserializeOwned>(post: bool, path: &str) -> Result<T, String> {
    let cfg = load_config()?;
    let tok = token()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("http://{}{path}", cfg.listen);
    let auth = format!("Bearer {tok}");
    let res = if post {
        agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send("{}")
    } else {
        agent
            .get(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .call()
    };
    let mut res = res.map_err(|_| NOT_ANSWERING.to_string())?;
    let status = res.status();
    let b = res
        .body_mut()
        .with_config()
        .limit(4 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if status != 200 {
        let e: crate::server::ErrorBody = serde_json::from_slice(&b).unwrap_or_default();
        return Err(if e.error.is_empty() {
            crate::client::status_line(status.as_u16())
        } else {
            e.error
        });
    }
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}

/// The devices paired with this machine, through its serve.
pub fn local_devices() -> Result<Vec<Device>, String> {
    local_call::<Option<Vec<Device>>>(false, "/api/devices").map(Option::unwrap_or_default)
}

/// Unpairs a device, ending what it has open.
pub fn local_revoke(id: &str) -> Result<(), String> {
    local_call::<serde_json::Value>(true, &format!("/api/devices/{id}/revoke")).map(|_| ())
}

/// Asks this machine's serve for a code to pair a phone with: an error
/// says serve isn't up yet, or can't be asked.
pub fn local_pairing() -> Result<Pairing, String> {
    local_call(true, "/api/pair")
}

/// Whether this machine's serve answers.
pub fn up() -> bool {
    let Ok(cfg) = load_config() else {
        return false;
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .http_status_as_error(false)
        .build()
        .into();
    agent
        .get(&format!("http://{}/api/machines", cfg.listen))
        .call()
        .is_ok_and(|r| matches!(r.status().as_u16(), 200 | 401))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_escapes_what_it_holds() {
        let p = plist("/Apps/R&D/rush");
        assert!(p.contains("<string>/Apps/R&amp;D/rush</string><string>serve</string>"));
        assert!(p.contains(&format!("<key>Label</key><string>{SERVICE_LABEL}</string>")));
        assert!(p.contains("serve.log</string>"));
    }
}
