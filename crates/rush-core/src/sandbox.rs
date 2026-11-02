//! Whether this process is a test running in the throwaway home
//! `rush-testenv` makes before any test starts, and the tripwires that
//! hold it there. Not in Go: Go's tests set their own HOME per test.
//!
//! The test binary's constructor sets [`ROOT_ENV`] to the throwaway home
//! and [`REAL_HOME_ENV`] to the home it replaced; every process a test
//! starts inherits both. The real binary never has them, so nothing here
//! changes what it does.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The throwaway home's folder, set for the whole test process.
pub const ROOT_ENV: &str = "RUSH_TEST_SANDBOX";
/// The home the test process was started with, which it must never touch.
pub const REAL_HOME_ENV: &str = "RUSH_TEST_REAL_HOME";

struct Sandbox {
    root: PathBuf,
    real_home: Option<PathBuf>,
}

fn sandbox() -> Option<&'static Sandbox> {
    static S: OnceLock<Option<Sandbox>> = OnceLock::new();
    S.get_or_init(|| {
        let root = std::env::var_os(ROOT_ENV).filter(|r| !r.is_empty())?;
        let real_home = std::env::var_os(REAL_HOME_ENV)
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .filter(|h| h.is_absolute() && h != Path::new("/"));
        Some(Sandbox {
            root: PathBuf::from(root),
            real_home,
        })
    })
    .as_ref()
}

/// Whether this process is a test in the throwaway home.
pub fn active() -> bool {
    sandbox().is_some()
}

/// The throwaway home's folder, in a test.
pub fn root() -> Option<&'static Path> {
    sandbox().map(|s| s.root.as_path())
}

/// The home the test replaced, in a test.
pub fn real_home() -> Option<&'static Path> {
    sandbox().and_then(|s| s.real_home.as_deref())
}

/// This checkout: tests may read their fixtures from it.
fn source_tree() -> &'static Path {
    // rust/crates/rush-core → the repository.
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.ancestors().nth(3).unwrap_or(here)
}

/// Whether path is somewhere a test must never reach: under the real
/// home, and neither in the throwaway home nor in this checkout. Always
/// false outside a test.
pub fn forbidden(path: &Path) -> bool {
    let Some(s) = sandbox() else {
        return false;
    };
    let Some(real) = &s.real_home else {
        return false;
    };
    path.starts_with(real) && !path.starts_with(&s.root) && !path.starts_with(source_tree())
}

/// Panics when a test resolved what (rush's folder, its cache, a socket)
/// to path under the real home. Does nothing outside a test.
pub fn guard(what: &str, path: &Path) {
    if forbidden(path) {
        panic!(
            "test sandbox: {what} resolved to {}, under the real home {}; \
             tests must stay in {}",
            path.display(),
            real_home()
                .map(|h| h.display().to_string())
                .unwrap_or_default(),
            root().map(|r| r.display().to_string()).unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The constructor's variables are read once; outside a test binary
    // that links rush-testenv there are none, and nothing is forbidden.
    #[test]
    fn outside_a_sandbox_nothing_is_forbidden() {
        if active() {
            let real = real_home().unwrap();
            assert!(forbidden(&real.join(".config/rush")));
            assert!(!forbidden(&root().unwrap().join(".config/rush")));
            assert!(!forbidden(&source_tree().join("rust")));
        } else {
            assert!(!forbidden(Path::new("/Users/someone/.config/rush")));
        }
    }
}
