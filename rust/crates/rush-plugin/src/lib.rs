//! Running other people's code next to your agents without trusting it.
//! Ported from Go `internal/plugin` (plugin.go:1-65 for this part); the
//! files on disk, the wire protocol and the words are Go rush's, so Go and
//! Rust rush share plugins, approvals and a broker.
//!
//! A plugin is a folder in root() with a plugin.json manifest and a
//! program. It runs in its own process, started by the broker (`rush
//! plugind`), in a macOS sandbox that denies everything it wasn't given:
//! it reads its own folder and the system's libraries, writes only its
//! data folder, starts no other programs, and reaches the network only
//! through rush's proxy, to the hosts its manifest names. It runs at
//! utility QoS, and the broker ends it if it grows past its memory limit.
//!
//! It talks to rush over one end of a socket pair, handed to it as fd 3:
//! no path on disk, so nothing else can connect to either side. The
//! messages are JSON-RPC 2.0 in length-prefixed frames. A plugin whose
//! manifest says "protocol": "mcp" is an ordinary MCP server spoken to on
//! stdio instead, so an off-the-shelf one runs sandboxed as it is.
//!
//! Nothing runs until you approve it (`rush plugin approve`), which records
//! a digest of every file in its folder along with the manifest you read.
//! If a file changes, the plugin stops running until you approve it again.
//! Agents, prompt text and tools all come from the manifest as approved,
//! not as it is on disk now.

use std::path::PathBuf;

pub mod bundled;
pub mod cli;
pub mod client;
pub mod cmd;
pub mod ctx;
pub mod describe;
pub mod gopath;
mod jsonshape;
pub mod manifest;
pub mod origin;
pub mod plugin;
pub mod project;
pub mod proxy;
pub mod rpc;
pub mod sandbox;
pub mod services;
pub mod sidebar;
pub mod sys;
pub mod text;
pub mod ui;
pub mod uisettings;

pub use bundled::{
    Bundle, Registered, bundle_named, bundled_on, bundles, bundles_on, enabled, mark_bundled,
    register_bundle, run_bundled, set_bundled,
};
pub use cli::{CLI_TIMEOUT, CliResult, CliRun, CliSpec, MAX_CLI_OUTPUT, cli_plugins, run_cli};
pub use client::{
    Broker, broker_lock, broker_log, broker_sock, dial_broker, dial_broker_with, ensure_broker,
};
pub use ctx::{Cancel, Ctx, CtxErr};
pub use describe::describe;
pub use manifest::*;
pub use origin::{Origin, dev_changed, origins, rebuild, set_dev, short_commit};
pub use plugin::{
    Approval, Contributions, Plugin, approvals, approve, decline, digest, for_session, installed,
    load, pending, revoke, verify,
};
pub use project::{Offer, in_projects, install, skip};
pub use proxy::Proxy;
pub use rpc::{CallError, Conn, Error, Handler, MAX_FRAME, Raw};
pub use sandbox::{Launch, supported};
pub use services::{
    Answer, CleanupReport, SERVICE_CLEANUP_REPORT, SERVICE_TIMEOUT, ask_service, providers,
};
pub use sidebar::{Sidebar, SidebarAgent, SidebarSection, Sidebars};
pub use text::{clean, clip, printable};
pub use ui::*;
pub use uisettings::{MAX_SETTING_TEXT, set_setting, setting_values, setting_values_of};

/// Holds one folder per installed plugin.
pub fn root() -> PathBuf {
    rush_state::dir().join("plugins")
}

/// Holds each plugin's data folder, the one place it can write. It is
/// apart from the plugin's own folder so the digest of that stays fixed.
pub fn data_root() -> PathBuf {
    rush_state::dir().join("plugin-data")
}

/// A plugin's data folder.
pub fn data_dir(name: &str) -> PathBuf {
    data_root().join(name)
}

/// Where a plugin's stdout and stderr go.
pub fn log_path(name: &str) -> PathBuf {
    data_root().join(format!("{name}.log"))
}

/// Writes a file whole through a temporary beside it, as Go's
/// os.WriteFile(tmp) then os.Rename: never half a file.
pub(crate) fn write_atomic(path: &std::path::Path, b: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| gopath::os_err("open", &tmp, &e))?;
    f.write_all(b)
        .map_err(|e| gopath::os_err("write", &tmp, &e))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| {
        format!(
            "rename {} {}: {}",
            tmp.display(),
            path.display(),
            gopath::errno_text(&e)
        )
    })
}

/// os.MkdirAll(dir, 0o700).
pub(crate) fn mkdir_all(dir: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| gopath::os_err("mkdir", dir, &e))
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
