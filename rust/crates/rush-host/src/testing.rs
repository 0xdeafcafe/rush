//! A rush folder of a test's own: never the real ~/.config/rush.

use std::path::PathBuf;

/// A rush folder and cache folder for this thread, under /tmp so a
/// session's socket path fits in the 104 bytes macOS allows; removed when
/// dropped.
pub struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Home {
        let dir = tempfile::Builder::new()
            .prefix("rh-")
            .tempdir_in("/tmp")
            .unwrap();
        let d = std::fs::canonicalize(dir.path()).unwrap();
        rush_state::set_thread_dir(Some(d.clone()));
        rush_state::set_thread_cache_dir(Some(d.join("cache")));
        Home { dir }
    }

    pub fn path(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap()
    }

    pub fn cache(&self) -> PathBuf {
        self.path().join("cache")
    }

    /// Makes another thread use this home: one a test starts itself.
    pub fn enter(path: &std::path::Path) {
        rush_state::set_thread_dir(Some(path.to_path_buf()));
        rush_state::set_thread_cache_dir(Some(path.join("cache")));
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
        rush_state::set_thread_cache_dir(None);
    }
}
