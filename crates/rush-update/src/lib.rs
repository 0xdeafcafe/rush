//! Finds out whether a newer rush is out, and installs it.
//!
//! After internal/update/update.go:1-229, changed for how the Rust rush
//! ships. Go's rush is installed with go install and asks the Go module
//! proxy; this one is built by .github/workflows/release.yml, so a new one
//! shows up as a GitHub release (its tag is the version), and it's put in
//! place one of two ways:
//!
//! - installed with Homebrew (the running rush is under a Cellar), by
//!   `brew update` then `brew upgrade 0xdeafcafe/tap/rush`, so brew keeps
//!   track of it, and checked to have put the new version in its Cellar;
//! - otherwise by downloading this machine's tar.gz from the release,
//!   checking its sha256, and renaming the rush in it over the running one.
//!
//! The last answer is kept in rust-update.json, apart from Go's
//! update.json: both rushes share the folder while Go's is still in use,
//! and their versions don't compare.
//!
//! What's new between two versions comes from the releases' notes
//! (notes.rs, new in the port), kept in rust-notes.json the same way.
//!
//! The events log and the network watch (Go's events.Log, netwatch.Run and
//! netwatch.Done) are reached through [`Host`], until those modules are
//! ported.

mod archive;
mod notes;
mod semver;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_core::time::{Ms, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use notes::{Item, NOTES_CACHE, NOTES_JOB, Notes, between, of, text, tidy};
pub use semver::newer;

/// Where rush is released.
pub const REPO: &str = "0xdeafcafe/rush";

/// GitHub's API.
pub const API: &str = "https://api.github.com";

/// rush's formula in the tap: homebrew/core's `rush` is another program
/// (GNU's restricted shell), so it's always named in full.
pub const FORMULA: &str = "0xdeafcafe/tap/rush";

/// How often rush asks GitHub; every rush shares the answer. Asking
/// without a token is allowed 60 times an hour; rush asks once an hour.
pub const EVERY: Duration = Duration::from_secs(60 * 60);

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
    /// What an install is doing now, to show while it runs: a step of its
    /// own, or the last line brew printed. Rust-only.
    fn step(&self, _what: &str) {}
}

/// A host that logs nothing and always asks.
pub struct Quiet;

impl Host for Quiet {}

/// The target rush is released for on this machine, as the release's file
/// names have it; None where there's no release built for it.
pub const fn target() -> Option<&'static str> {
    if cfg!(target_arch = "aarch64") {
        Some("aarch64-apple-darwin")
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
/// GitHub only when no rush has in the last [`EVERY`]; forced (you asked:
/// r on Settings › Updates), asking it now whatever was kept.
pub fn check(host: &dyn Host, force: bool) -> (Release, bool) {
    let Some(cur) = current() else {
        return (Release::default(), false);
    };
    check_in(&state_dir(), API, &cur, host, now(), force)
}

fn check_in(
    dir: &Path,
    api: &str,
    cur: &str,
    host: &dyn Host,
    now: Ms,
    force: bool,
) -> (Release, bool) {
    let path = dir.join(CACHE);
    let mut c: Cache = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if force || now - c.checked > EVERY.as_millis() as Ms || c.latest.version.is_empty() {
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
/// keeps running; the next rush is the new one. A release that isn't newer
/// than this rush is never installed: no downgrade, no reinstall.
pub fn install(host: &dyn Host) -> Result<Release, Error> {
    host.log("update-install", Value::Null);
    let res = match current() {
        None => Err(Error(
            "this rush was built from source: pull and build it again to update".into(),
        )),
        Some(cur) => std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|e| Error(format!("can't find the rush that's running: {e}")))
            .and_then(|exe| install_at(API, &cur, &exe, &state_dir(), host)),
    };
    let mut f = json!({"version": res.as_ref().map_or("", |l| l.version.as_str())});
    if let Err(e) = &res {
        f["err"] = Value::String(e.0.clone());
    }
    host.log("update-installed", f);
    res
}

/// Installs the newest release over exe, when it's newer than cur, the
/// rush that's running. By semver's order, as the check's: a nightly
/// (0.1.0-nightly.DATE) comes before its release (0.1.0).
fn install_at(
    api: &str,
    cur: &str,
    exe: &Path,
    dir: &Path,
    host: &dyn Host,
) -> Result<Release, Error> {
    host.step("asking GitHub for the newest rush…");
    let l = fetch(api)?;
    if !newer(cur, &l.version) {
        return Err(Error(format!(
            "rush v{cur} is the newest: {} isn't newer",
            l.short()
        )));
    }
    match homebrew(exe) {
        Some(brew) => brew_upgrade(&brew, &l.version, host)?,
        None => {
            host.step(&format!("downloading {}…", l.short()));
            replace(exe, &l, host)?
        }
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

/// `brew update`, then `brew upgrade 0xdeafcafe/tap/rush`, then a look in
/// brew's Cellar for version: brew answers an upgrade it has nothing new
/// for with a warning and success, so success alone isn't trusted.
///
/// The update is run by name rather than left to the upgrade's own
/// (HOMEBREW_AUTO_UPDATE_SECS=0, as before): that one is skipped without a
/// word when the environment rush started in has HOMEBREW_NO_AUTO_UPDATE,
/// and it hides why a tap couldn't be fetched. brew has no update of one
/// tap; `brew update` fetches every tap that's a clone, as this one is, and
/// only reads homebrew/core's API. Each line brew prints is a step.
pub fn brew_upgrade(brew: &Path, version: &str, host: &dyn Host) -> Result<(), Error> {
    if !is_executable(brew) {
        return Err(Error(format!(
            "rush was installed with Homebrew, but there's no brew at {}",
            brew.display()
        )));
    }
    host.step("brew update: fetching the tap…");
    let updated = run_brew(brew, &["update", "--quiet"], &[], host)
        .map_err(|e| Error(format!("brew update: {e}")))?;
    let update_failed = (!updated.0.success()).then(|| words(&updated));
    host.step(&format!("brew upgrade {FORMULA}…"));
    // Updated just now: not again.
    let out = run_brew(
        brew,
        &["upgrade", FORMULA],
        &[("HOMEBREW_NO_AUTO_UPDATE", "1")],
        host,
    )
    .map_err(|e| Error(format!("brew upgrade: {e}")))?;
    if !out.0.success() {
        return Err(Error(format!("brew upgrade: {}", words(&out))));
    }
    if in_cellar(brew, version) {
        return Ok(());
    }
    let mut why = format!("brew upgrade left rush as it was: brew's tap has no v{version} yet");
    if let Some(u) = update_failed {
        why.push_str(&format!(" (brew update: {u})"));
    }
    if !out.1.is_empty() {
        why.push_str(&format!(" · brew said: {}", out.1));
    }
    Err(Error(why))
}

/// What brew said, or how it ended when it said nothing.
fn words(out: &(std::process::ExitStatus, String)) -> String {
    if out.1.is_empty() {
        exit_words(&out.0)
    } else {
        out.1.clone()
    }
}

/// Runs brew with args, its hints off and env set, each line it prints to
/// stdout a step as it comes: how it ended and all it said, stdout then
/// stderr.
fn run_brew(
    brew: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    host: &dyn Host,
) -> io::Result<(std::process::ExitStatus, String)> {
    use std::io::{BufRead, Read};
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(brew);
    cmd.args(args)
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env_remove("HOMEBREW_NO_AUTO_UPDATE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn()?;
    let err = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = e.read_to_end(&mut b);
            b
        })
    });
    let mut said = Vec::new();
    if let Some(o) = child.stdout.take() {
        for l in io::BufReader::new(o).split(b'\n') {
            let Ok(l) = l else { break };
            let t = String::from_utf8_lossy(&l);
            let t = t.trim();
            if !t.is_empty() {
                host.step(t);
            }
            said.extend_from_slice(&l);
            said.push(b'\n');
        }
    }
    let status = child.wait()?;
    said.extend(err.and_then(|h| h.join().ok()).unwrap_or_default());
    Ok((status, String::from_utf8_lossy(&said).trim().to_string()))
}

/// Whether the Homebrew this brew is of has rush version in its Cellar:
/// <prefix>/Cellar/rush/<version>, or <version>_<revision>.
fn in_cellar(brew: &Path, version: &str) -> bool {
    let Some(prefix) = brew.parent().and_then(Path::parent) else {
        return false;
    };
    let Ok(rd) = std::fs::read_dir(prefix.join("Cellar").join("rush")) else {
        return false;
    };
    rd.flatten().any(|e| {
        let n = e.file_name();
        let n = n.to_string_lossy();
        n == version
            || n.strip_prefix(version)
                .and_then(|r| r.strip_prefix('_'))
                .is_some_and(|r| !r.is_empty() && r.bytes().all(|c| c.is_ascii_digit()))
    })
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
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return format!("signal: {sig}");
        }
    }
    s.to_string()
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
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

fn create_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

fn open_new(path: &Path, mode: u32) -> io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
    }
    o.open(path)
}

/// Go's os.WriteFile(path, b, mode).
fn write_file(path: &Path, b: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write;
    open_new(path, mode)?.write_all(b)
}

/// A stand-in brew, for the tests here and the screens': nothing reaches
/// the real one.
#[doc(hidden)]
pub mod testing {
    use std::path::Path;

    /// A brew in prefix/bin that notes each run in prefix/ran (its args and
    /// HOMEBREW_NO_AUTO_UPDATE), answers `update` at once, and on an
    /// upgrade prints a line and exits with code: one that exits 0 puts
    /// installs in prefix/Cellar/rush, as brew would; one that fails says
    /// "Error: no bottle".
    pub fn fake_brew(prefix: &Path, code: i32, installs: Option<&str>) {
        let bin = prefix.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let brew = bin.join("brew");
        let put = match installs {
            Some(v) if code == 0 => format!(
                "mkdir -p '{}'\n",
                prefix.join("Cellar/rush").join(v).display()
            ),
            _ => String::new(),
        };
        let fail = if code == 0 {
            ""
        } else {
            "echo 'Error: no bottle' >&2\n"
        };
        std::fs::write(
            &brew,
            format!(
                "#!/bin/sh\necho \"$* noauto=$HOMEBREW_NO_AUTO_UPDATE\" >> '{}'\n[ \"$1\" = update ] && exit 0\necho '==> Upgrading 0xdeafcafe/tap/rush'\n{put}{fail}exit {code}\n",
                prefix.join("ran").display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(test)]
mod tests;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
