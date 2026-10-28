//! Finds out whether a newer rush is out, and installs it.
//!
//! Ported from internal/update/update.go:1-229.
//!
//! rush is installed with go install, so the Go module proxy is where a new
//! one shows up, and go install is how it's put in place: over the rush
//! that's running, wherever that lives. This keeps Go's way for now, so the
//! two rushes agree on what's newest and share update.json; a rush built
//! with cargo has to change how it's installed before the Go one retires.
//!
//! The events log and the network watch (Go's events.Log, netwatch.Run and
//! netwatch.Done) are reached through [`Host`], until those modules are
//! ported.

mod gotime;

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_core::time::{Ms, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MODULE: &str = "github.com/0xdeafcafe/rush";
pub const PKG: &str = "github.com/0xdeafcafe/rush/cmd/rush";

/// How often rush asks the proxy; every rush shares the answer.
pub const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// What looking for a newer rush is called where rush shows what waits on
/// the network.
pub const JOB: &str = "Update check";

/// How long one question to the proxy may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// One version of rush, as the module proxy knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    #[serde(rename = "Version", alias = "version", default)]
    pub version: String,
    /// When it was made, in Unix ms; 0 is unknown (Go's zero time).
    #[serde(rename = "Time", alias = "time", default, with = "gotime")]
    pub time: Ms,
}

impl Info {
    /// v for a tag, and the commit and day for a pseudo-version.
    pub fn short(&self) -> String {
        if let Some((_, commit)) = pseudo(&self.version) {
            return format!("{} ({})", &commit[..7], gotime::local_month_day(self.time));
        }
        self.version
            .strip_suffix("+dirty")
            .unwrap_or(&self.version)
            .to_string()
    }
}

/// A pseudo-version's time and commit: Go's `(\d{14})-([0-9a-f]{12})(\+dirty)?$`.
fn pseudo(v: &str) -> Option<(&str, &str)> {
    let v = v.strip_suffix("+dirty").unwrap_or(v);
    let b = v.as_bytes();
    if b.len() < 27 {
        return None;
    }
    let commit = &v[v.len() - 12..];
    let stamp = &v[v.len() - 27..v.len() - 13];
    let ok = b[b.len() - 13] == b'-'
        && commit
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        && stamp.bytes().all(|c| c.is_ascii_digit());
    ok.then_some((stamp, commit))
}

/// Why asking or installing failed, in words to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// What the rest of rush does around an update: the events log, and the
/// network watch that says whether the network may be asked now.
pub trait Host {
    /// Go's events.Log(kind, fields).
    fn log(&self, _kind: &str, _fields: Value) {}
    /// Go's netwatch.Run(job): false when the network is known to be down.
    fn net_run(&self, _job: &str) -> bool {
        true
    }
    /// Go's netwatch.Done(job, err).
    fn net_done(&self, _job: &str, _err: Option<&str>) {}
}

/// A host that logs nothing and always asks.
pub struct Quiet;

impl Host for Quiet {}

/// The rush that's running. Built with no version (Go's go build from a
/// checkout; for cargo, no RUSH_VERSION when it was built), it's None:
/// there's nothing to compare.
pub fn current() -> Option<Info> {
    current_from(option_env!("RUSH_VERSION").unwrap_or(""))
}

/// The rush whose version is v, as Go's build info gives it.
pub fn current_from(v: &str) -> Option<Info> {
    if v.is_empty() || v == "(devel)" {
        return None;
    }
    let mut i = Info {
        version: v.to_string(),
        time: 0,
    };
    if let Some((stamp, _)) = pseudo(v) {
        i.time = gotime::parse_stamp(stamp)?;
    }
    Some(i)
}

/// Asks the module proxy for the newest rush.
pub fn latest() -> Result<Info, Error> {
    fetch(&proxy(), "@latest")
}

fn fetch(proxy: &str, path: &str) -> Result<Info, Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{proxy}/{MODULE}/{path}");
    let mut res = agent
        .get(&url)
        .call()
        .map_err(|e| Error(format!("Get {url:?}: {e}")))?;
    if res.status() != 200 {
        return Err(Error(format!("the Go module proxy said {}", res.status())));
    }
    let mut b = Vec::new();
    res.body_mut()
        .as_reader()
        .read_to_end(&mut b)
        .map_err(|e| Error(e.to_string()))?;
    serde_json::from_slice(&b).map_err(|e| Error(e.to_string()))
}

/// The first proxy in GOPROXY that's a URL, or Go's own.
pub fn proxy() -> String {
    proxy_from(&std::env::var("GOPROXY").unwrap_or_default())
}

fn proxy_from(goproxy: &str) -> String {
    goproxy
        .split([',', '|'])
        .find(|p| p.starts_with("https://") || p.starts_with("http://"))
        .map_or_else(
            || "https://proxy.golang.org".to_string(),
            |p| p.trim_end_matches('/').to_string(),
        )
}

/// Whether latest came out after cur. A tag's time isn't in its name, so
/// it's asked for.
pub fn newer(cur: &Info, latest: &Info) -> bool {
    newer_at(&proxy(), cur, latest)
}

fn newer_at(proxy: &str, cur: &Info, latest: &Info) -> bool {
    let v = cur.version.strip_suffix("+dirty").unwrap_or(&cur.version);
    if v == latest.version {
        return false;
    }
    let mut t = cur.time;
    if t == 0
        && let Ok(i) = fetch(proxy, &format!("@v/{v}.info"))
    {
        t = i.time;
    }
    t != 0 && latest.time > t
}

/// The last answer, shared by every rush (update.json).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Cache {
    #[serde(default, with = "gotime")]
    checked: Ms,
    #[serde(default)]
    latest: Info,
}

/// rush's folder: $RUSH_HOME, else ~/.config/rush, or ~/.config/agtop while
/// that's the one in use (internal/state/state.go:27-62).
fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let rush = home.join(".config").join("rush");
    let agtop = home.join(".config").join("agtop");
    if !rush.join("config.json").exists() && agtop.join("config.json").exists() {
        return agtop;
    }
    rush
}

/// update.json, where the last answer is kept.
pub fn cache_path() -> PathBuf {
    state_dir().join("update.json")
}

/// The newest rush that's out, and whether it's newer than this one, asking
/// the proxy only when no rush has in the last [`EVERY`].
pub fn check(host: &dyn Host) -> (Info, bool) {
    let Some(cur) = current() else {
        return (Info::default(), false);
    };
    check_in(&state_dir(), &proxy(), &cur, host, now())
}

fn check_in(dir: &Path, proxy: &str, cur: &Info, host: &dyn Host, now: Ms) -> (Info, bool) {
    let path = dir.join("update.json");
    let mut c: Cache = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if now - c.checked > EVERY.as_millis() as Ms || c.latest.version.is_empty() {
        if !host.net_run(JOB) {
            return (Info::default(), false);
        }
        let l = fetch(proxy, "@latest");
        host.net_done(JOB, l.as_ref().err().map(|e| e.0.as_str()));
        let l = match l {
            Ok(l) => l,
            Err(e) => {
                host.log(
                    "update-checked",
                    json!({"current": cur.version, "err": e.0}),
                );
                return (Info::default(), false);
            }
        };
        host.log(
            "update-checked",
            json!({"current": cur.version, "latest": l.version}),
        );
        c = Cache {
            checked: now,
            latest: l,
        };
        if let Ok(b) = serde_json::to_vec(&c) {
            let _ = create_dir(dir);
            let _ = write_file(&path, &b);
        }
    }
    let newer = newer_at(proxy, cur, &c.latest);
    (c.latest, newer)
}

/// Puts the newest rush over the one that's running, with go install. What's
/// running keeps running; the next rush is the new one.
pub fn install(host: &dyn Host) -> Result<Info, Error> {
    host.log("update-install", Value::Null);
    let res = install_inner(host);
    let mut f = json!({"version": res.as_ref().map_or("", |l| l.version.as_str())});
    if let Err(e) = &res {
        f["err"] = Value::String(e.0.clone());
    }
    host.log("update-installed", f);
    res
}

fn install_inner(host: &dyn Host) -> Result<Info, Error> {
    let Some(go) = look_path("go") else {
        return Err(Error(
            "updating needs Go, and go isn't on your PATH — install it from https://go.dev/dl"
                .into(),
        ));
    };
    let l = latest()?;
    let mut cmd = std::process::Command::new(go);
    cmd.arg("install").arg(format!("{PKG}@{}", l.version));
    // Into the folder the running rush is in, so it's the one replaced; one
    // built with no version is left alone, and Go picks the folder. The rush
    // there now is kept beside it first, so a host the new one can't run
    // goes back to it (see host.Restart).
    if current().is_some()
        && let Some(exe) = std::env::current_exe()
            .ok()
            .and_then(|e| std::fs::canonicalize(e).ok())
        && exe.file_name().is_some_and(|n| n == "rush")
    {
        if let Some(dir) = exe.parent() {
            cmd.env("GOBIN", dir);
        }
        if let Err(e) = keep(&exe) {
            host.log(
                "update-kept",
                json!({"exe": exe.to_string_lossy(), "err": e.to_string()}),
            );
        }
    }
    let out = cmd
        .output()
        .map_err(|e| Error(format!("go install: {e}")))?;
    if !out.status.success() {
        let mut both = out.stdout;
        both.extend_from_slice(&out.stderr);
        let said = String::from_utf8_lossy(&both).trim().to_string();
        let why = if said.is_empty() {
            exit_words(&out.status)
        } else {
            said
        };
        return Err(Error(format!("go install: {}", why.trim())));
    }
    if let Ok(c) = serde_json::to_vec(&Cache {
        checked: now(),
        latest: l.clone(),
    }) {
        let _ = write_file(&cache_path(), &c);
    }
    Ok(l)
}

/// How a command ended, as Go's exec.ExitError says it.
fn exit_words(s: &std::process::ExitStatus) -> String {
    if let Some(code) = s.code() {
        return format!("exit status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return format!("signal: {sig}");
        }
    }
    s.to_string()
}

/// The first file named name on PATH that can be run.
fn look_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Where the rush installed at exe is kept when [`install`] puts a newer one
/// over it: the way back.
pub fn prev(exe: &Path) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(".prev");
    PathBuf::from(s)
}

/// Makes prev(exe) the rush at exe: a hard link, so it costs nothing (go
/// install removes the file before writing the new one, so the link keeps
/// the old), or a copy where it can't link.
fn keep(exe: &Path) -> io::Result<()> {
    let prev = prev(exe);
    let _ = std::fs::remove_file(&prev);
    if std::fs::hard_link(exe, &prev).is_ok() {
        return Ok(());
    }
    let mut src = std::fs::File::open(exe)?;
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o755);
    }
    let mut dst = o.open(&prev)?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

#[cfg(unix)]
fn create_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Go's os.WriteFile(path, b, 0o600).
fn write_file(path: &Path, b: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(b)
}

#[cfg(test)]
mod tests;
