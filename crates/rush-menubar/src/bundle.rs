//! Where the menu bar app lives, building it, and starting and stopping
//! it. Port of internal/menubar/app.go:1-230.
//!
//! Go's app is Swift, compiled on this Mac the first time it's needed. The
//! Rust app is rush itself: the bundle's program is a copy of this rush
//! binary, which runs the app when it finds itself started from inside
//! the bundle ([`crate::bundled`]), so building it needs no compiler.

use std::fs;
use std::io::Write as _;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The app's identity: macOS keeps its notification settings under it.
pub const BUNDLE_ID: &str = "dev.rush.menubar";

/// The bundle's program's name.
pub const EXECUTABLE: &str = "rush-menubar";

/// The app's icon, which its notifications carry: the bottle on its tile,
/// and on a full square for macOS 26 on, which draws its own tile (Go's
/// docs/brand/finish-icon.sh makes both). The menu bar's bottle is drawn
/// by the app.
const ICONS: [(&str, &[u8]); 2] = [
    (
        "AppIcon-full.icns",
        include_bytes!("../icons/AppIcon-full.icns"),
    ),
    ("AppIcon.icns", include_bytes!("../icons/AppIcon.icns")),
];

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// The menu bar app's folder in rush's.
pub fn dir() -> PathBuf {
    rush_state::dir().join("menubar")
}

fn lock_path() -> PathBuf {
    dir().join("feed.lock")
}

/// Where the built app lives.
pub fn app_path() -> PathBuf {
    dir().join("rush.app")
}

/// Marks the feed as running for as long as the returned guard lives, with
/// the app's pid, so rush leaves notifications to it and can quit it.
pub struct Hold(Option<fs::File>);

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            let _ = f.set_len(0);
        }
    }
}

/// Takes the feed's lock (app.go:42 hold).
pub fn hold() -> Hold {
    let _ = make_dir(&dir());
    let Ok(f) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path())
    else {
        return Hold(None);
    };
    if !flock(&f, libc::LOCK_EX | libc::LOCK_NB) {
        return Hold(None);
    }
    let _ = f.set_len(0);
    let pid = std::env::var("RUSH_MENUBAR_PID").unwrap_or_default();
    let _ = (&f).write_all(pid.as_bytes());
    Hold(Some(f))
}

/// Whether the menu bar app is up, so rush's own notifications would only
/// repeat its.
pub fn running() -> bool {
    let Ok(f) = fs::File::open(lock_path()) else {
        return false;
    };
    if !flock(&f, libc::LOCK_SH | libc::LOCK_NB) {
        return true;
    }
    flock(&f, libc::LOCK_UN);
    false
}

fn flock(f: &fs::File, how: libc::c_int) -> bool {
    // SAFETY: flock on an fd this File owns.
    unsafe { libc::flock(f.as_raw_fd(), how) == 0 }
}

fn make_dir(d: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(d)
}

/// An error starting the menu bar app, in words.
pub type Error = String;

/// Builds the app if rush changed since it was last built, and opens it;
/// one already running is left alone unless it was just rebuilt. rushes
/// starting together take turns, so only the first builds and opens it.
pub fn start() -> Result<(), Error> {
    let _ = make_dir(&dir());
    // Held until start returns: rushes starting together take turns.
    let _turn = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir().join("start.lock"))
        .inspect(|f| {
            flock(f, libc::LOCK_EX);
        });
    let rebuilt = match build_app() {
        Ok(r) => r,
        Err(e) => {
            log("menubar-start", fields(json!({ "err": e })));
            return Err(e);
        }
    };
    if rebuilt {
        stop();
        // Go waits up to five seconds for the old one to let go of the
        // feed's lock: the lock is waited on, not polled.
        wait_unlocked(std::time::Duration::from_secs(5));
        // The lock goes when the feed does; the app itself may be a moment
        // behind, and Launch Services refuses to open over it.
        wait_gone(std::time::Duration::from_secs(5));
    } else if running() {
        return Ok(());
    }
    let mut res = run("/usr/bin/open", &[app_path().as_os_str()]);
    if rebuilt && res.is_err() {
        // Launch Services can refuse (-600, -10810) while the old app is
        // still going away: once it's confirmed gone, open it once more.
        wait_gone(std::time::Duration::from_secs(5));
        res = run("/usr/bin/open", &[app_path().as_os_str()]);
    }
    let mut f = Map::new();
    f.insert("rebuilt".into(), Value::Bool(rebuilt));
    if let Err(e) = &res {
        f.insert("err".into(), Value::String(e.clone()));
    }
    log("menubar-start", f);
    res
}

/// Waits until no feed holds the lock, at most d: a shared lock taken on a
/// thread of its own, which blocks in the kernel until the feed's goes.
fn wait_unlocked(d: std::time::Duration) {
    let Ok(f) = fs::File::open(lock_path()) else {
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        flock(&f, libc::LOCK_SH);
        flock(&f, libc::LOCK_UN);
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(d);
}

/// Waits until no process runs the app's program, at most d.
fn wait_gone(d: std::time::Duration) {
    let prog = app_path().join("Contents").join("MacOS").join(EXECUTABLE);
    let pat = quote_meta(&prog.to_string_lossy());
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        // pgrep exits 0 when something matches.
        let up = Command::new("/usr/bin/pgrep")
            .arg("-f")
            .arg(&pat)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !up {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Quits the app, every copy of it, and with it the feed.
pub fn stop() {
    log("menubar-stop", Map::new());
    let pid = fs::read_to_string(lock_path())
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    if let Some(pid) = pid
        && pid > 1
        && running()
    {
        // a plain signal to a pid the feed wrote.
        let _ = rush_sys::proc::kill(pid, libc::SIGTERM);
    }
    let prog = app_path().join("Contents").join("MacOS").join(EXECUTABLE);
    let _ = Command::new("/usr/bin/pkill")
        .arg("-f")
        .arg(quote_meta(&prog.to_string_lossy()))
        .stdin(Stdio::null())
        .output();
}

/// Stops the app opening at login.
pub fn forget() {
    let _ = fs::remove_file(login_plist());
}

/// The launch agent that opens the app at login.
pub fn login_plist() -> PathBuf {
    rush_home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{BUNDLE_ID}.plist"))
}

fn rush_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// What the stamp beside the app is made from: the app's own makings (this
/// rush, which the bundle's program is a copy of, by path, size and
/// modification time), the rush it runs, whether macOS draws icons' tiles,
/// and the icons.
pub(crate) fn stamp(bin: &Path, full: bool) -> String {
    let mut h = Sha256::new();
    h.update(b"rush-menubar-rs\x00");
    h.update(bin.to_string_lossy().as_bytes());
    h.update(b"\x00");
    h.update(if full { b"true" as &[u8] } else { b"false" });
    if let Ok(m) = fs::metadata(bin) {
        h.update(m.len().to_le_bytes());
        if let Ok(t) = m.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            h.update(d.as_nanos().to_le_bytes());
        }
    }
    for (name, b) in ICONS {
        h.update(b"\x00");
        h.update(name.as_bytes());
        h.update(b"\x00");
        h.update(b);
    }
    let sum = h.finalize();
    sum[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Makes the app bundle, unless the one there was built from this rush.
/// Returns whether it built one.
pub fn build_app() -> Result<bool, Error> {
    let app = app_path();
    let bin = crate::exe();
    let full = tiles();
    let stamp = stamp(&bin, full);
    // Beside it: inside would break its signature.
    let stamp_path = PathBuf::from(format!("{}.stamp", app.display()));
    if app.exists() && fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp) {
        return Ok(false);
    }
    let _ = fs::remove_dir_all(&app);
    for d in ["MacOS", "Resources"] {
        fs::create_dir_all(app.join("Contents").join(d)).map_err(|e| e.to_string())?;
    }
    let prog = app.join("Contents").join("MacOS").join(EXECUTABLE);
    // A copy, never a link: signing it changes the file.
    fs::copy(&bin, &prog).map_err(|e| format!("building the menu bar app: {e}"))?;
    for (name, b) in ICONS {
        let name = match name {
            "AppIcon.icns" if full => continue,
            "AppIcon-full.icns" if !full => continue,
            "AppIcon-full.icns" => "AppIcon.icns",
            n => n,
        };
        fs::write(app.join("Contents").join("Resources").join(name), b)
            .map_err(|e| e.to_string())?;
    }
    let version = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    fs::write(
        app.join("Contents").join("Info.plist"),
        plist(version, &bin.to_string_lossy()),
    )
    .map_err(|e| e.to_string())?;
    // Notifications need a signed app; an ad-hoc signature is enough on the
    // Mac that built it.
    let out = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("signing the menu bar app: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "signing the menu bar app: {}\n{}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    // Launch Services and Notification Center keep an app's icon by its
    // path and version; a new version, registered again, has them take
    // this build's (an older build may have had none).
    let _ = Command::new(LSREGISTER)
        .arg("-f")
        .arg(&app)
        .stdin(Stdio::null())
        .output();
    fs::write(&stamp_path, stamp).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Unregisters the app at path with Launch Services: for tests, whose
/// app mustn't stand in for the real one.
pub fn unregister(path: &Path) {
    let _ = Command::new(LSREGISTER)
        .arg("-u")
        .arg(path)
        .stdin(Stdio::null())
        .output();
}

/// Whether macOS draws app icons' tiles itself, as it does from 26 on;
/// before, an icon brings its own.
fn tiles() -> bool {
    let out = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    major(&out) >= 26
}

/// The major version of a "26.1" product version.
pub(crate) fn major(v: &str) -> i32 {
    v.trim()
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0)
}

/// Runs a program to its end with nothing of it on rush's terminal: its
/// output is caught, and what it said on stderr goes in the error.
fn run(prog: &str, args: &[&std::ffi::OsStr]) -> Result<(), Error> {
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.trim();
    let name = Path::new(prog)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if said.is_empty() {
        Err(out.status.to_string())
    } else {
        Err(format!("{name} said: {said} ({})", out.status))
    }
}

pub(crate) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Go's regexp.QuoteMeta: s matched as it is.
pub(crate) fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The app's Info.plist (app.go:211 plist).
pub(crate) fn plist(version: u64, bin: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
	<key>CFBundleName</key><string>rush</string>
	<key>CFBundleDisplayName</key><string>rush</string>
	<key>CFBundleExecutable</key><string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>{version}</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>LSUIElement</key><true/>
	<key>RushBinary</key><string>{}</string>
</dict>
</plist>
"#,
        xml_escape(bin)
    )
}

pub(crate) fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

pub(crate) fn log(kind: &str, f: Map<String, Value>) {
    rush_events::log(kind, f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_programs_stderr_is_in_the_error() {
        let args: [&std::ffi::OsStr; 2] = ["-c".as_ref(), "echo boom >&2; exit 1".as_ref()];
        let e = run("/bin/sh", &args).unwrap_err();
        assert!(e.contains("boom"), "{e}");
        assert!(e.contains("sh said"), "{e}");
        let ok: [&std::ffi::OsStr; 2] = ["-c".as_ref(), "echo noise >&2".as_ref()];
        assert_eq!(run("/bin/sh", &ok), Ok(()));
    }

    #[test]
    fn lock_is_held_while_the_feed_runs() {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        assert!(!running(), "nothing holds it yet");
        let h = hold();
        assert!(running());
        drop(h);
        assert!(!running());
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn plist_names_the_rush_it_runs() {
        let p = plist(7, "/a&b/<rush>");
        assert!(p.contains("<key>RushBinary</key><string>/a&amp;b/&lt;rush&gt;</string>"));
        assert!(p.contains("<key>CFBundleVersion</key><string>7</string>"));
        assert!(p.contains("<key>CFBundleIdentifier</key><string>dev.rush.menubar</string>"));
        assert!(p.contains("<key>LSUIElement</key><true/>"));
    }

    #[test]
    fn versions_and_patterns() {
        for (v, want) in [("26.1\n", 26), ("15.6.1", 15), ("", 0), ("x", 0)] {
            assert_eq!(major(v), want, "{v:?}");
        }
        assert_eq!(quote_meta("/a.app/x+y"), r"/a\.app/x\+y");
    }

    #[test]
    fn stamp_follows_what_its_made_from() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("rush");
        fs::write(&bin, b"one").unwrap();
        let a = stamp(&bin, true);
        assert_eq!(a.len(), 16);
        assert_eq!(a, stamp(&bin, true));
        assert_ne!(a, stamp(&bin, false));
        fs::write(&bin, b"rush two").unwrap();
        assert_ne!(a, stamp(&bin, true), "a new rush is a new app");
    }

    /// Builds a real app bundle (signing it, registering it): only when
    /// asked, as Go's TestBuildApp.
    #[test]
    fn build_app_when_asked() {
        if std::env::var_os("RUSH_BUILD_APP").is_none() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        let app = app_path();
        assert_eq!(build_app(), Ok(true));
        let ok = Command::new("/usr/bin/codesign")
            .args(["--verify", "--verbose"])
            .arg(&app)
            .status()
            .unwrap();
        assert!(ok.success());
        let ok = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(app.join("Contents/Info.plist"))
            .status()
            .unwrap();
        assert!(ok.success());
        assert_eq!(build_app(), Ok(false), "nothing changed, nothing built");
        unregister(&app);
        rush_state::set_thread_dir(None);
    }
}
