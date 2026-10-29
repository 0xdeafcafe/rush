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
