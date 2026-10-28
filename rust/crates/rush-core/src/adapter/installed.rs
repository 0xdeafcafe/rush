//! Finding an agent's program on this machine. After Go's
//! internal/agent/installed.go (Find, commonDirs); the registry keeps what
//! was found.

use std::path::{Path, PathBuf};

/// Where agents' installers put their programs, for a rush started with a
/// thin PATH (from the Dock, launchd or a menu bar).
const COMMON_DIRS: [&str; 9] = [
    ".local/bin",
    "bin",
    ".npm-global/bin",
    ".bun/bin",
    ".cargo/bin",
    "go/bin",
    ".volta/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
];

/// Where programs are looked for: PATH, and the home folder relative
/// install places are under. Taken from the environment unless a test
/// sets its own.
#[derive(Clone, Debug, Default)]
pub(super) struct Env {
    pub path: String,
    pub home: String,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            path: std::env::var("PATH").unwrap_or_default(),
            home: std::env::var("HOME").unwrap_or_default(),
        }
    }
}

/// Where program name is: on PATH, or in one of dirs or the usual install
/// places. Relative dirs are under the home folder.
pub fn find(name: &str, dirs: &[String]) -> Option<String> {
    find_in(&Env::from_process(), name, dirs)
}

pub(super) fn find_in(env: &Env, name: &str, dirs: &[String]) -> Option<String> {
    if let Some(p) = look_path(&env.path, name) {
        return Some(p);
    }
    let common = COMMON_DIRS.iter().map(|s| s.to_string());
    for d in dirs.iter().cloned().chain(common) {
        let d = if Path::new(&d).is_absolute() {
            PathBuf::from(d)
        } else if env.home.is_empty() {
            continue;
        } else {
            Path::new(&env.home).join(d)
        };
        let p = d.join(name);
        if executable(&p) {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// Go's exec.LookPath: a name with a slash is tried as it is; else each of
/// PATH's folders (an empty one is the current folder).
fn look_path(path: &str, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        return executable(Path::new(name)).then(|| name.to_string());
    }
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = Path::new(dir).join(name);
        if executable(&p) {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

fn executable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    if m.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
