//! remote.json in rush's folder: where serve listens, what it's called,
//! the other machines it shows, and what it lets in; and serve's token.
//! After Go's internal/remote/config.go (Config :22, Peer :70, Reach* :77,
//! DefaultListen :84, LoadConfig :89, loadRaw :104, UpdateConfig :118,
//! current :138, SaveConfig :159, Peer :176, Token :188, the *Or ports
//! :215-236).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::plugins;

/// remote.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where serve listens; 127.0.0.1:7878 when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub listen: String,
    /// This machine's, as the web app lists it; the hostname when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The one address browsers use: a request from it is accepted, and
    /// notifications link to it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub origin: String,
    /// When there are any, the only folders a remote client may start a
    /// session in (and their subfolders).
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "rush_core::wire::nullvec::deserialize"
    )]
    pub workspaces: Vec<String>,
    /// The other machines this one shows, as a hub.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "rush_core::wire::nullvec::deserialize"
    )]
    pub peers: Vec<Peer>,
    /// The tailnet https port; 8443 when zero.
    #[serde(rename = "tailscalePort", skip_serializing_if = "zero")]
    pub tailscale_port: i64,
    /// Where serve's SSH listens; 127.0.0.1:2222 when empty.
    #[serde(rename = "sshListen", skip_serializing_if = "String::is_empty")]
    pub ssh_listen: String,
    /// serve's SSH port on the tailnet; 2222 when zero.
    #[serde(rename = "sshPort", skip_serializing_if = "zero")]
    pub ssh_port: i64,
    /// How other devices get to serve: "" this machine only, "tailscale"
    /// or "cloudflare". The plugins' now: read only to move it into them.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reach: String,
    /// Lets in the web app.
    #[serde(skip_serializing_if = "is_false")]
    pub web: bool,
    /// Lets in rush itself over SSH: the remote-ssh plugin's.
    #[serde(skip_serializing_if = "is_false")]
    pub ssh: bool,
    /// Has serve keep the peers' sessions in this machine's rush.
    #[serde(rename = "attachPeers", skip_serializing_if = "is_false")]
    pub attach_peers: bool,
    /// The token file of the tunnel serve runs through Cloudflare.
    #[serde(
        rename = "cloudflaredTokenFile",
        skip_serializing_if = "String::is_empty"
    )]
    pub cloudflared: String,
}

fn zero(n: &i64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Another machine's serve.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Peer {
    pub name: String,
    pub url: String,
    pub token: String,
}

/// How serve is reached (Config.reach).
pub const REACH_LOCAL: &str = "";
pub const REACH_TAILSCALE: &str = "tailscale";
pub const REACH_CLOUDFLARE: &str = "cloudflare";

/// Where serve listens unless told.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7878";

/// The https port serve asks for on the tailnet unless remote.json says
/// another (expose.go:23).
pub const TAILSCALE_PORT: i64 = 8443;

/// rush's folder.
pub fn dir() -> PathBuf {
    rush_state::dir()
}

pub(crate) fn config_path() -> PathBuf {
    dir().join("remote.json")
}

/// This machine's name as the hostname gives it, up to its first dot.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into buf.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if r != 0 {
        return String::new();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let h = String::from_utf8_lossy(&buf[..n]).into_owned();
    h.split('.').next().unwrap_or_default().to_string()
}

/// remote.json with its defaults, and how serve is reached as its plugins
/// say; a missing one is the defaults.
pub fn load_config() -> Result<Config, String> {
    plugins::migrate();
    let (c, err) = match load_raw() {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e)),
    };
    let mut c = plugins::with_switches(c);
    if c.listen.is_empty() {
        c.listen = DEFAULT_LISTEN.into();
    }
    if c.name.is_empty() {
        c.name = hostname();
    }
    match err {
        Some(e) => Err(e),
        None => Ok(c),
    }
}

/// remote.json as written, without the defaults.
pub fn load_raw() -> Result<Config, String> {
    match std::fs::read(config_path()) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// Changes remote.json with f; a running serve sees it within a few
/// seconds.
pub fn update_config(f: impl FnOnce(&mut Config)) -> Result<(), String> {
    let mut c = load_raw()?;
    f(&mut c);
    save_config(&c)
}

/// remote.json as it was last read, and the file's time and size then.
static LIVE: Mutex<Option<(Option<SystemTime>, u64, Config)>> = Mutex::new(None);

/// remote.json as it is now, read again only when it has changed: what
/// serve is reached by and lets in can change while it runs.
pub fn current() -> Config {
    plugins::migrate();
    let st = std::fs::metadata(config_path());
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(st) = st else {
        *live = None;
        return plugins::with_switches(Config::default());
    };
    let (m, n) = (st.modified().ok(), st.len());
    let stale = live.as_ref().is_none_or(|(lm, ln, _)| *lm != m || *ln != n);
    if stale {
        let c = match load_raw() {
            Ok(c) => c,
            Err(_) => live.as_ref().map(|l| l.2.clone()).unwrap_or_default(),
        };
        *live = Some((m, n, c));
    }
    plugins::with_switches(live.as_ref().map(|l| l.2.clone()).unwrap_or_default())
}

/// Writes remote.json, readable only by you: it holds peers' tokens.
pub fn save_config(c: &Config) -> Result<(), String> {
    let b = serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?;
    write_private(&config_path(), &b)
}

/// Writes path through a temp file and a rename, readable only by you.
pub(crate) fn write_private(path: &std::path::Path, b: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let parent = path.parent().unwrap_or(path);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|e| e.to_string())?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(b).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

impl Config {
    /// The peer called name.
    pub fn peer(&self, name: &str) -> Option<Peer> {
        self.peers.iter().find(|p| p.name == name).cloned()
    }

    /// The tailnet https port, 8443 unless set.
    pub fn tailscale_port_or(&self) -> i64 {
        if self.tailscale_port > 0 {
            self.tailscale_port
        } else {
            TAILSCALE_PORT
        }
    }

    /// Where serve's SSH listens.
    pub fn ssh_listen_or(&self) -> String {
        if self.ssh_listen.is_empty() {
            "127.0.0.1:2222".into()
        } else {
            self.ssh_listen.clone()
        }
    }

    /// serve's SSH port on the tailnet.
    pub fn ssh_port_or(&self) -> i64 {
        if self.ssh_port > 0 {
            self.ssh_port
        } else {
            2222
        }
    }
}

/// Random bytes from the system.
pub(crate) fn rand_bytes(n: usize) -> Vec<u8> {
    use ring::rand::SecureRandom;
    let mut b = vec![0u8; n];
    // The system's generator doesn't fail on macOS or Linux.
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    b
}

/// b as lowercase hex.
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// This machine's serve token, made on first use. Delete the file and
/// restart serve to change it.
pub fn token() -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir().join("remote-token");
    if let Ok(b) = std::fs::read_to_string(&path)
        && b.trim().len() >= 32
    {
        return Ok(b.trim().to_string());
    }
    let t = hex(&rand_bytes(32));
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    // create_new: two serves starting at once agree on one token.
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path);
    match f {
        Ok(mut f) => {
            f.write_all(format!("{t}\n").as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(t)
        }
        Err(e) => match std::fs::read_to_string(&path) {
            Ok(b) => Ok(b.trim().to_string()),
            Err(_) => Err(e.to_string()),
        },
    }
}

/// Where bin is on PATH, as exec.LookPath finds it.
pub fn look_path(bin: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// a, or b when a is empty.
pub(crate) fn or(a: &str, b: &str) -> String {
    if a.is_empty() { b.into() } else { a.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_writes_as_go_does() {
        let c = Config {
            listen: "127.0.0.1:9".into(),
            peers: vec![Peer {
                name: "mini".into(),
                url: "http://x".into(),
                token: "t".into(),
            }],
            web: true,
            ..Config::default()
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(
            s,
            r#"{"listen":"127.0.0.1:9","peers":[{"name":"mini","url":"http://x","token":"t"}],"web":true}"#
        );
        let back: Config = serde_json::from_str(r#"{"peers":null,"workspaces":null}"#).unwrap();
        assert!(back.peers.is_empty());
        assert_eq!(c.tailscale_port_or(), 8443);
        assert_eq!(c.ssh_listen_or(), "127.0.0.1:2222");
        assert_eq!(c.ssh_port_or(), 2222);
        assert_eq!(c.peer("mini").unwrap().url, "http://x");
        assert!(c.peer("other").is_none());
    }
}
