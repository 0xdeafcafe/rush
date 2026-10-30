//! Finds out whether a newer rush is out, and installs it.
//!
//! After internal/update/update.go:1-229, changed for how the Rust rush
//! ships. Go's rush is installed with go install and asks the Go module
//! proxy; this one is built by .github/workflows/release.yml, so a new one
//! shows up as a GitHub release (its tag is the version), and it's put in
//! place one of two ways:
//!
//! - installed with Homebrew (the running rush is under a Cellar), by
//!   `brew upgrade 0xdeafcafe/tap/rush`, so brew keeps track of it;
//! - otherwise by downloading this machine's tar.gz from the release,
//!   checking its sha256, and renaming the rush in it over the running one.
//!
//! The last answer is kept in rust-update.json, apart from Go's
//! update.json: both rushes share the folder while Go's is still in use,
//! and their versions don't compare.
//!
//! The events log and the network watch (Go's events.Log, netwatch.Run and
//! netwatch.Done) are reached through [`Host`], until those modules are
//! ported.

mod archive;
mod semver;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_core::time::{Ms, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use semver::newer;

/// Where rush is released.
pub const REPO: &str = "0xdeafcafe/rush";

/// GitHub's API.
pub const API: &str = "https://api.github.com";

/// rush's formula in the tap: homebrew/core's `rush` is another program
/// (GNU's restricted shell), so it's always named in full.
pub const FORMULA: &str = "0xdeafcafe/tap/rush";

/// How often rush asks GitHub; every rush shares the answer. Asking
/// without a token is allowed 60 times an hour, far more than this.
pub const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// What looking for a newer rush is called where rush shows what waits on
/// the network.
pub const JOB: &str = "Update check";

/// The file in rush's folder where the last answer is kept.
pub const CACHE: &str = "rust-update.json";

/// How long one question to GitHub may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long downloading a release may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The most of GitHub's answer about a release that's read: its notes and a
/// list of files, a few kB.
const MAX_BODY: u64 = 1 << 20;

/// The most of a download that's read, and of the rush in it.
const MAX_DOWNLOAD: u64 = 256 << 20;

/// The version this rush was built as (RUSH_VERSION when it was built, as
/// the release workflow sets it), without a leading v. None for a rush
/// built from source without one: there's nothing to compare.
pub fn current() -> Option<String> {
    current_from(option_env!("RUSH_VERSION").unwrap_or(""))
}

/// The version v names, as [`current`] gives it.
pub fn current_from(v: &str) -> Option<String> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    (!v.is_empty()).then(|| v.to_string())
}

/// One release of rush.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// Its tag without the v: 0.2.0.
    pub version: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

impl Release {
    /// v and the version, as rush shows it.
    pub fn short(&self) -> String {
        format!("v{}", self.version)
    }

    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// A release as GitHub's API gives it.
#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
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

/// The target rush is released for on this machine, as the release's file
/// names have it; None where there's no release built for it.
pub const fn target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "aarch64"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "x86_64"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else {
        None
    }
}

/// The release's tar.gz for target: rush, its licence and readme.
pub fn tarball(version: &str, target: &str) -> String {
    format!("rush-{version}-{target}.tar.gz")
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .user_agent(format!(
            "rush/{}",
            current().as_deref().unwrap_or(env!("CARGO_PKG_VERSION"))
        ))
        .build()
        .into()
}

/// Asks GitHub for the newest release of rush.
pub fn latest() -> Result<Release, Error> {
    fetch(API)
}

fn fetch(api: &str) -> Result<Release, Error> {
    let url = format!("{api}/repos/{REPO}/releases/latest");
    let mut res = agent(TIMEOUT)
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| Error(format!("Get {url:?}: {e}")))?;
    match res.status().as_u16() {
        200 => {}
        404 => return Err(Error("GitHub has no release of rush yet".into())),
        _ => return Err(Error(format!("GitHub said {}", res.status()))),
    }
    // A release is a few kB: an answer much bigger than that isn't one, and
    // isn't kept in memory.
    let b = res
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|e| Error(e.to_string()))?;
    let r: GhRelease = serde_json::from_slice(&b).map_err(|e| Error(e.to_string()))?;
    let version = current_from(&r.tag_name)
        .ok_or_else(|| Error("GitHub's newest release of rush has no tag".into()))?;
    Ok(Release {
        version,
        assets: r
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
            })
            .collect(),
    })
}

/// The body at url, up to limit bytes.
fn download(url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let mut res = agent(DOWNLOAD_TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| Error(format!("Get {url:?}: {e}")))?;
    if res.status() != 200 {
        return Err(Error(format!("Get {url:?}: {}", res.status())));
    }
    res.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| Error(format!("Get {url:?}: {e}")))
}

/// The last answer, shared by every Rust rush (rust-update.json).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Cache {
    /// When GitHub was last asked, in Unix ms.
    #[serde(default)]
    checked: Ms,
    #[serde(default)]
    latest: Release,
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

/// rust-update.json, where the last answer is kept.
pub fn cache_path() -> PathBuf {
    state_dir().join(CACHE)
}

/// The newest rush that's out, and whether it's newer than this one, asking
/// GitHub only when no rush has in the last [`EVERY`].
pub fn check(host: &dyn Host) -> (Release, bool) {
    let Some(cur) = current() else {
        return (Release::default(), false);
    };
    check_in(&state_dir(), API, &cur, host, now())
}

fn check_in(dir: &Path, api: &str, cur: &str, host: &dyn Host, now: Ms) -> (Release, bool) {
    let path = dir.join(CACHE);
    let mut c: Cache = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if now - c.checked > EVERY.as_millis() as Ms || c.latest.version.is_empty() {
        if !host.net_run(JOB) {
            return (Release::default(), false);
        }
        let l = fetch(api);
        host.net_done(JOB, l.as_ref().err().map(|e| e.0.as_str()));
        let l = match l {
            Ok(l) => l,
            Err(e) => {
                host.log("update-checked", json!({"current": cur, "err": e.0}));
                return (Release::default(), false);
            }
        };
        host.log(
            "update-checked",
            json!({"current": cur, "latest": l.version}),
        );
        c = Cache {
            checked: now,
            latest: l,
        };
        save(dir, &c);
    }
    let newer = newer(cur, &c.latest.version);
    (c.latest, newer)
}

fn save(dir: &Path, c: &Cache) {
    if let Ok(b) = serde_json::to_vec(c) {
        let _ = create_dir(dir);
        let _ = write_file(&dir.join(CACHE), &b, 0o600);
    }
}

/// The brew that installed the rush at exe (canonical): the one in the bin
/// of the Homebrew whose Cellar it's in. None when it isn't in one.
pub fn homebrew(exe: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = exe.components().collect();
    let i = parts
        .windows(2)
        .position(|w| w[0].as_os_str() == "Cellar" && w[1].as_os_str() == "rush")?;
    let prefix: PathBuf = parts[..i].iter().collect();
    Some(prefix.join("bin").join("brew"))
}

/// Puts the newest rush in place of the one that's running: with brew when
/// brew installed it, else from the release's download. What's running
/// keeps running; the next rush is the new one.
pub fn install(host: &dyn Host) -> Result<Release, Error> {
    host.log("update-install", Value::Null);
    let res = if current().is_none() {
        Err(Error(
            "this rush was built from source: pull and build it again to update".into(),
        ))
    } else {
        std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|e| Error(format!("can't find the rush that's running: {e}")))
            .and_then(|exe| install_at(API, &exe, &state_dir(), host))
    };
    let mut f = json!({"version": res.as_ref().map_or("", |l| l.version.as_str())});
    if let Err(e) = &res {
        f["err"] = Value::String(e.0.clone());
    }
    host.log("update-installed", f);
    res
}

fn install_at(api: &str, exe: &Path, dir: &Path, host: &dyn Host) -> Result<Release, Error> {
    let l = fetch(api)?;
    match homebrew(exe) {
        Some(brew) => brew_upgrade(&brew)?,
        None => replace(exe, &l, host)?,
    }
    save(
        dir,
        &Cache {
            checked: now(),
            latest: l.clone(),
        },
    );
    Ok(l)
}

/// `brew upgrade 0xdeafcafe/tap/rush`, with the tap brought up to date
/// first even when brew updated itself within the day.
fn brew_upgrade(brew: &Path) -> Result<(), Error> {
    if !is_executable(brew) {
        return Err(Error(format!(
            "rush was installed with Homebrew, but there's no brew at {}",
            brew.display()
        )));
    }
    let out = std::process::Command::new(brew)
        .args(["upgrade", FORMULA])
        .env("HOMEBREW_AUTO_UPDATE_SECS", "0")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error(format!("brew upgrade: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let mut both = out.stdout;
    both.extend_from_slice(&out.stderr);
    let said = String::from_utf8_lossy(&both).trim().to_string();
    let why = if said.is_empty() {
        exit_words(&out.status)
    } else {
        said
    };
    Err(Error(format!("brew upgrade: {why}")))
}

/// Downloads l's tar.gz for this machine, checks it against its .sha256,
/// and renames the rush in it over exe. The rush there now is kept beside
/// it first, so a host the new one can't run goes back to it (see
/// host.Restart).
fn replace(exe: &Path, l: &Release, host: &dyn Host) -> Result<(), Error> {
    let Some(target) = target() else {
        return Err(Error(
            "there's no release of rush built for this machine".into(),
        ));
    };
    if exe.file_name().is_none_or(|n| n != "rush") {
        return Err(Error(format!(
            "{} isn't called rush, so it's left alone",
            exe.display()
        )));
    }
    let name = tarball(&l.version, target);
    let missing = |n: &str| Error(format!("release {} has no {n}", l.short()));
    let tgz = l.asset(&name).ok_or_else(|| missing(&name))?;
    let sum_name = format!("{name}.sha256");
    let sum = l.asset(&sum_name).ok_or_else(|| missing(&sum_name))?;

    let want = String::from_utf8_lossy(&download(&sum.url, 4 << 10)?)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let b = download(&tgz.url, MAX_DOWNLOAD)?;
    let got = hex(&Sha256::digest(&b));
    if got != want {
        return Err(Error(format!(
            "{name} isn't what was released: its sha256 is {got}, not {want}"
        )));
    }
    let bin = archive::find(&b, "rush", MAX_DOWNLOAD).map_err(|e| Error(format!("{name}: {e}")))?;

    let dir = exe.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".rush.new-{}", std::process::id()));
    let write = || -> io::Result<()> {
        write_file(&tmp, &bin, 0o755)?;
        std::fs::File::open(&tmp)?.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error(format!("can't write to {}: {e}", dir.display())));
    }
    if let Err(e) = keep(exe) {
        host.log(
            "update-kept",
            json!({"exe": exe.to_string_lossy(), "err": e.to_string()}),
        );
    }
    // A rename is all or nothing: anything starting rush sees the old one
    // or the new one, never half of it.
    std::fs::rename(&tmp, exe).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error(format!("can't replace {}: {e}", exe.display()))
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
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

/// Makes prev(exe) the rush at exe: a hard link, so it costs nothing (the
/// new rush is renamed over exe, so the link keeps the old), or a copy where
/// it can't link.
fn keep(exe: &Path) -> io::Result<()> {
    let prev = prev(exe);
    let _ = std::fs::remove_file(&prev);
    if std::fs::hard_link(exe, &prev).is_ok() {
        return Ok(());
    }
    let mut src = std::fs::File::open(exe)?;
    let mut dst = open_new(&prev, 0o755)?;
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

fn open_new(path: &Path, _mode: u32) -> io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(_mode);
    }
    o.open(path)
}

/// Go's os.WriteFile(path, b, mode).
fn write_file(path: &Path, b: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write;
    open_new(path, mode)?.write_all(b)
}

#[cfg(test)]
mod tests;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
