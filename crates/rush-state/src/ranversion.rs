//! The version of rush the last view ran: ranversion.json under rush's
//! folder, so a view started on a newer one can say what's new once.
//!
//! New in the port: there's no Go source (Go's rush had no release notes
//! to show). Its own file, like lastview.json, so state.json stays as Go
//! writes it. Which version is newer is rush-update's to say; this only
//! keeps the last one.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Ran {
    /// Without the v: 0.1.0-beta.8.
    version: String,
}

/// rush's folder's ranversion.json.
pub fn path() -> PathBuf {
    crate::dir().join("ranversion.json")
}

/// The version the last view ran, if one was kept.
pub fn load() -> Option<String> {
    let b = std::fs::read(path()).ok()?;
    let r: Ran = serde_json::from_slice(&b).ok()?;
    (!r.version.is_empty()).then_some(r.version)
}

/// Keeps version as the one running, and gives back the one kept before
/// it (None the first time). The same version again writes nothing.
pub fn swap(version: &str) -> io::Result<Option<String>> {
    let was = load();
    if was.as_deref() != Some(version) {
        crate::write_json(
            &path(),
            &Ran {
                version: version.to_string(),
            },
        )?;
    }
    Ok(was)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_gives_back_the_last() {
        let d = tempfile::tempdir().unwrap();
        crate::set_thread_dir(Some(d.path().to_path_buf()));
        assert_eq!(load(), None);
        for (run, was) in [
            ("0.1.0-beta.7", None),
            ("0.1.0-beta.7", Some("0.1.0-beta.7")),
            ("0.1.0-beta.8", Some("0.1.0-beta.7")),
            ("0.1.0-beta.6", Some("0.1.0-beta.8")),
        ] {
            assert_eq!(swap(run).unwrap().as_deref(), was, "{run}");
            assert_eq!(load().as_deref(), Some(run));
        }
        std::fs::write(path(), "not json").unwrap();
        assert_eq!(swap("0.2.0").unwrap(), None, "unreadable is none");
        crate::set_thread_dir(None);
    }
}
