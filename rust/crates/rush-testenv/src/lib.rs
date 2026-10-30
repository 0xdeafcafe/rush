//! For tests only (a dev-dependency): before any test of the binary that
//! links it starts, a constructor moves the whole process into a throwaway
//! home, so no test, and no thread or process a test starts, can read or
//! write the real one, reach a live rush's sockets, or run the user's
//! claude, codex or rush.
//!
//! It sets, for the whole process: HOME, RUSH_HOME, RUSH_CACHE,
//! XDG_CONFIG_HOME, XDG_CACHE_HOME, XDG_DATA_HOME, CLAUDE_CONFIG_DIR and
//! CODEX_HOME to folders in one temp dir; TMPDIR to a folder in it; PATH
//! to an empty folder of its own and the system's
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), so no user shim is found; and drops
//! every variable naming a live session, host, socket or plugin. It sets
//! `rush_core::sandbox`'s variables, which arm the tripwires in rush
//! itself: rush's folder resolving under the real home panics, and
//! nothing is signalled that the test didn't start.
//!
//! Link it from every crate's tests with `rush_testenv::sandboxed!();` at
//! the crate root under `#[cfg(test)]`, and at the top of each integration
//! test file: that links the constructor in and adds a test saying it ran.
//! Not in Go, whose tests set HOME per test.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The throwaway home's folder (rush_core::sandbox::ROOT_ENV).
pub const ROOT_ENV: &str = "RUSH_TEST_SANDBOX";
/// The home it replaced (rush_core::sandbox::REAL_HOME_ENV).
pub const REAL_HOME_ENV: &str = "RUSH_TEST_REAL_HOME";

/// What PATH holds after the fake bin folder: the system's own, which has
/// git and sh but nothing a user installed.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Variables a test keeps although they start with RUSH_ or AGTOP_: they
/// say what to test, not where a live rush is.
const KEEP: &[&str] = &[
    "RUSH_FIXTURES",
    "RUSH_PARITY",
    "RUSH_BENCH_TRANSCRIPT",
    "RUSH_BENCH_COLD",
];

/// Prefixes of variables that name a live session, host, socket, plugin
/// or the agent this runs under, all dropped.
const DROP_PREFIXES: &[&str] = &[
    "RUSH_",
    "AGTOP_",
    "CLAUDE_CODE_",
    "CLAUDE_",
    "CODEX_",
    "ANTHROPIC_",
    "OPENAI_",
    "PI_CODING_AGENT_",
    "TMUX",
];

/// Single variables dropped for the same reason.
const DROP: &[&str] = &[
    "CLAUDECODE",
    "ZDOTDIR",
    "OLLAMA_HOST",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "PGGUARD_PARENT",
    "WATCH_TASK_CHILD",
];

#[used]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(
    any(target_os = "linux", target_os = "android", target_os = "freebsd"),
    unsafe(link_section = ".init_array")
)]
static INIT: extern "C" fn() = init;

/// Runs before main, so before libtest starts a thread: setting the
/// environment is sound here, and every test sees it.
extern "C" fn init() {
    if let Err(e) = enter() {
        // Running on against the real home is what this is here to stop.
        eprintln!("rush-testenv: can't make the throwaway home, so no test runs: {e}");
        std::process::abort();
    }
}

fn enter() -> std::io::Result<()> {
    // A process a test started (the test binary run again, rush itself)
    // stays in the sandbox it was started in.
    if let Some(root) = std::env::var_os(ROOT_ENV).filter(|r| !r.is_empty())
        && Path::new(&root).is_dir()
    {
        return Ok(());
    }
    let real_home = real_home();
    let root = make_root()?;
    let home = root.join("home");
    let bin = root.join("bin");
    let config = home.join(".config");
    let dirs = [
        ("HOME", home.clone()),
        ("RUSH_HOME", config.join("rush")),
        (
            "RUSH_CACHE",
            home.join("Library").join("Caches").join("rush"),
        ),
        ("XDG_CONFIG_HOME", config.clone()),
        ("XDG_CACHE_HOME", home.join(".cache")),
        ("XDG_DATA_HOME", home.join(".local").join("share")),
        ("CLAUDE_CONFIG_DIR", home.join(".claude")),
        ("CODEX_HOME", home.join(".codex")),
    ];
    for (_, d) in &dirs {
        std::fs::create_dir_all(d)?;
    }
    std::fs::create_dir_all(&bin)?;
    // Temp dirs a test makes (tempfile) land in the throwaway home too.
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    // Tests that commit need a name; the real ~/.gitconfig is out of reach.
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = rush tests\n\temail = tests@rush.invalid\n",
    )?;

    let drop: Vec<OsString> = std::env::vars_os()
        .map(|(k, _)| k)
        .filter(|k| dropped(k))
        .collect();
    // SAFETY: before main, so no other thread reads the environment.
    unsafe {
        for k in drop {
            std::env::remove_var(k);
        }
        for (k, d) in &dirs {
            std::env::set_var(k, d);
        }
        let path = format!("{}:{SYSTEM_PATH}", bin.display());
        std::env::set_var("PATH", path);
        std::env::set_var("TMPDIR", &tmp);
        std::env::set_var(ROOT_ENV, &root);
        if let Some(h) = &real_home {
            std::env::set_var(REAL_HOME_ENV, h);
        }
    }
    OWNED.set(root).ok();
    // SAFETY: atexit takes a plain function, run once at exit.
    unsafe { libc::atexit(cleanup) };
    Ok(())
}

static OWNED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Removes the throwaway home this process made, at exit.
extern "C" fn cleanup() {
    if let Some(root) = OWNED.get() {
        let _ = std::fs::remove_dir_all(root);
    }
}

fn dropped(k: &OsStr) -> bool {
    let Some(k) = k.to_str() else {
        return false;
    };
    if KEEP.contains(&k) {
        return false;
    }
    DROP.contains(&k) || DROP_PREFIXES.iter().any(|p| k.starts_with(p))
}

/// The home this process was started with: HOME, else the user's entry in
/// the password file.
fn real_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    // SAFETY: getpwuid returns static storage or null; read at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
        Some(PathBuf::from(dir.to_string_lossy().into_owned()))
    }
}

/// A new folder of this process's own in /tmp.
fn make_root() -> std::io::Result<PathBuf> {
    // /tmp, not TMPDIR: an agent may point TMPDIR into the home, and a
    // short path keeps screens that show one as they'd be.
    let tmp = Path::new("/tmp");
    for n in 0..1000u32 {
        let d = tmp.join(format!("rush-test-{}-{n}", std::process::id()));
        match std::fs::create_dir(&d) {
            // The canonical path, so a path read back (getcwd, realpath)
            // starts with it.
            Ok(()) => return d.canonicalize(),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free name"))
}

/// The throwaway home's folder. Panics when the constructor didn't run.
pub fn root() -> PathBuf {
    PathBuf::from(std::env::var_os(ROOT_ENV).expect("rush-testenv's constructor didn't run"))
}

/// Panics unless this process runs in the throwaway home: the test
/// [`sandboxed!`] adds.
pub fn assert_sandboxed() {
    let root = root();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    assert!(home.starts_with(&root), "HOME {home:?} isn't in {root:?}");
    for k in [
        "RUSH_HOME",
        "RUSH_CACHE",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "TMPDIR",
    ] {
        let v = PathBuf::from(std::env::var_os(k).unwrap_or_default());
        assert!(v.starts_with(&root), "{k} {v:?} isn't in {root:?}");
    }
    let path = std::env::var("PATH").unwrap_or_default();
    assert_eq!(
        path,
        format!("{}:{SYSTEM_PATH}", root.join("bin").display()),
        "PATH"
    );
    for k in ["RUSH_SESSION", "RUSH_IPC_FD", "AGTOP_IPC_FD", "CLAUDECODE"] {
        assert!(std::env::var_os(k).is_none(), "{k} is still set");
    }
}

/// Links the constructor into this test binary and adds a test saying it
/// ran. Put it at the crate root under `#[cfg(test)]`, and at the top of
/// each integration test file.
#[macro_export]
macro_rules! sandboxed {
    () => {
        #[test]
        fn runs_in_the_test_sandbox() {
            $crate::assert_sandboxed();
        }
    };
}

#[cfg(test)]
mod tests {
    sandboxed!();

    #[test]
    fn drops_what_names_a_live_rush() {
        use super::dropped;
        use std::ffi::OsStr;
        for k in ["RUSH_SESSION", "AGTOP_IPC_FD", "CLAUDECODE", "TMUX_PANE"] {
            assert!(dropped(OsStr::new(k)), "{k}");
        }
        for k in ["RUSH_FIXTURES", "PATH", "TERM", "UPDATE_SNAPSHOTS"] {
            assert!(!dropped(OsStr::new(k)), "{k}");
        }
    }
}
