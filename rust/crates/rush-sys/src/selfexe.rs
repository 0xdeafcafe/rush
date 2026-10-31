//! The running rush as a file to start again. An install removes the old
//! rush before it writes the new one (go install does; a package manager
//! may too), so a rush started in that gap fails with "no such file or
//! directory", or starts half a file. path waits out the gap.
//!
//! Ported from internal/selfexe/selfexe.go:1-49.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long path waits for an install to finish writing rush.
pub const SETTLE: Duration = Duration::from_secs(3);

/// std::env::current_exe once the file there can be started: until the
/// file is there and has stopped growing, for up to SETTLE.
pub fn path() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    wait(&exe, SETTLE)?;
    Ok(exe)
}

/// The running rush, links resolved, as hooks and other files that
/// outlive this rush name it: one Homebrew installed is named by its opt
/// link (see [`installed`]), which a `brew upgrade` keeps. None when the
/// running program can't be found.
pub fn resolved() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    Some(installed(&exe))
}

/// exe (links resolved) as its installer keeps naming it. Homebrew puts
/// each version in a folder of its own, <prefix>/Cellar/<name>/<version>,
/// and removes it once a newer one is in, so a hook naming that folder
/// stops working at the next upgrade; <prefix>/opt/<name> always links
/// the installed version. Anything else, or an opt link that isn't this
/// rush, is exe itself.
pub fn installed(exe: &Path) -> PathBuf {
    let s = exe.to_string_lossy();
    if let Some(i) = s.rfind("/Cellar/") {
        let (prefix, rest) = (&s[..i], &s[i + "/Cellar/".len()..]);
        let mut parts = rest.splitn(3, '/');
        if let (Some(name), Some(_version), Some(inside)) =
            (parts.next(), parts.next(), parts.next())
        {
            let opt = PathBuf::from(format!("{prefix}/opt/{name}/{inside}"));
            if std::fs::canonicalize(&opt).is_ok_and(|p| p == exe) {
                return opt;
            }
        }
    }
    exe.to_path_buf()
}

/// Blocks until the file at exe is there and not being written, for up to
/// settle. A file last written a while ago is settled at once.
pub fn wait(exe: &Path, settle: Duration) -> std::io::Result<()> {
    const TICK: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + settle;
    let mut last: Option<(u64, SystemTime)> = None;
    loop {
        let got = std::fs::metadata(exe).and_then(|m| Ok((m.len(), m.modified()?)));
        if let Ok((size, at)) = &got {
            let old = SystemTime::now()
                .duration_since(*at)
                .is_ok_and(|d| d > Duration::from_secs(1));
            if old || last == Some((*size, *at)) {
                return Ok(());
            }
        }
        last = got.as_ref().ok().copied();
        if Instant::now() > deadline {
            return match got {
                Err(e) => Err(std::io::Error::new(
                    e.kind(),
                    format!(
                        "rush isn't at {} (is it being installed?): {e}",
                        exe.display()
                    ),
                )),
                Ok(_) => Ok(()),
            };
        }
        std::thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A Homebrew rush is named by its opt link, which outlives an
    // upgrade; anything else, or an opt link to another version, is left.
    #[test]
    fn installed_names_homebrews_opt_link() {
        let d = tempfile::tempdir().unwrap();
        let prefix = std::fs::canonicalize(d.path()).unwrap();
        let keg = prefix.join("Cellar/rush/1.2.3/bin");
        std::fs::create_dir_all(&keg).unwrap();
        std::fs::write(keg.join("rush"), "rush").unwrap();
        std::fs::create_dir_all(prefix.join("opt")).unwrap();
        std::os::unix::fs::symlink("../Cellar/rush/1.2.3", prefix.join("opt/rush")).unwrap();
        assert_eq!(
            installed(&keg.join("rush")),
            prefix.join("opt/rush/bin/rush")
        );

        // The opt link moved on to another version: this one is left as is.
        let old = prefix.join("Cellar/rush/1.2.2/bin");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("rush"), "rush").unwrap();
        assert_eq!(installed(&old.join("rush")), old.join("rush"));

        // Not Homebrew's.
        let cargo = prefix.join("cargo/bin/rush");
        assert_eq!(installed(&cargo), cargo);
    }

    // selfexe_test.go TestWaitOutlastsAnInstall: a rush removed by an
    // install and written again a moment later is waited for, not failed
    // on.
    #[test]
    fn wait_outlasts_an_install() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("rush");
        let e2 = exe.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            std::fs::write(e2, "new").unwrap();
        });
        wait(&exe, SETTLE).unwrap();
        assert!(exe.exists(), "returned before the file was back");
        t.join().unwrap();
    }

    // selfexe_test.go TestWaitGivesUpOnAMissingRush
    #[test]
    fn wait_gives_up_on_a_missing_rush() {
        let d = tempfile::tempdir().unwrap();
        let e = wait(&d.path().join("rush"), Duration::from_millis(100)).unwrap_err();
        assert!(e.to_string().contains("is it being installed?"), "{e}");
    }

    // selfexe_test.go TestWaitIsInstantOnASettledRush
    #[test]
    fn wait_is_instant_on_a_settled_rush() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("rush");
        std::fs::write(&exe, "old").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&exe)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let start = Instant::now();
        wait(&exe, SETTLE).unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(20),
            "a settled rush took {:?}",
            start.elapsed()
        );
    }
}
