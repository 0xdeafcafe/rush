//! Ends a process group when the process that started it goes away,
//! however it goes. A host killed with SIGKILL can't stop the agent it
//! runs, and an agent told nothing runs its turn to the end, still writing
//! the transcript the next host resumes. Closing its stdin isn't enough
//! either: an agent mid-turn only reads it once the turn is over.
//!
//! The guard is a shell in its own process group, reading a pipe only this
//! process writes to. The pipe closes when this process exits, killed or
//! not, and the shell then ends the group: SIGTERM first, SIGKILL if it
//! hasn't gone a second later.
//!
//! Ported from internal/pgguard/pgguard.go:1-62.

use std::io::PipeWriter;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

const SCRIPT: &str = r#"read -r _
kill -s TERM -- "-$1" 2>/dev/null || exit 0
for _ in 1 2 3 4 5; do
  sleep 0.2
  kill -0 -- "-$1" 2>/dev/null || exit 0
done
kill -s KILL -- "-$1" 2>/dev/null
exit 0"#;

/// A guard on one process group. Dropped without [`Guard::release`], its
/// pipe closes and it ends the group, as this process exiting would.
pub struct Guard {
    pub(crate) pid: i32,
    w: Option<PipeWriter>,
    reaped: Arc<Mutex<bool>>,
}

/// Guards the process group led by pgid. With no shell to run there is no
/// guard, and None is returned.
pub fn watch(pgid: i32) -> Option<Guard> {
    let (r, w) = std::io::pipe().ok()?;
    let mut child = Command::new("/bin/sh")
        .args(["-c", SCRIPT, "rush-guard", &pgid.to_string()])
        .stdin(r)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?; // w closes as it drops
    let pid = child.id() as i32;
    let reaped = Arc::new(Mutex::new(false));
    let r2 = reaped.clone();
    std::thread::spawn(move || {
        crate::cmd::wait_exit(pid);
        let mut g = r2.lock().unwrap();
        let _ = child.wait();
        *g = true;
    });
    Some(Guard {
        pid,
        w: Some(w),
        reaped,
    })
}

impl Guard {
    /// Lets the guard go once the group has ended on its own. It's killed
    /// before its pipe closes, so it never signals a group id that's gone
    /// and may be another's.
    pub fn release(mut self) {
        let g = self.reaped.lock().unwrap();
        if !*g {
            // SAFETY: pid is our child, not yet reaped.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
        }
        drop(g);
        self.w.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{alive, gone_within};
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    // A process that dies without a word takes the group it guarded with
    // it, children and all. The test re-runs itself as that process.
    #[test]
    fn group_ends_with_its_parent() {
        if std::env::var("PGGUARD_PARENT").as_deref() == Ok("1") {
            let mut sh = Command::new("/bin/sh")
                .args(["-c", "sleep 60 & echo $!; wait"])
                .stdout(Stdio::piped())
                .process_group(0)
                .spawn()
                .unwrap();
            let g = watch(sh.id() as i32);
            let mut line = String::new();
            BufReader::new(sh.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            let mut out = std::io::stdout();
            writeln!(out, "PGGUARD {} {}", sh.id(), line.trim()).unwrap();
            out.flush().unwrap();
            // Held until the test kills this process.
            let _ = sh.wait();
            drop(g);
            std::process::exit(0);
        }
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args([
                "pgguard::tests::group_ends_with_its_parent",
                "--exact",
                "--nocapture",
            ])
            .env("PGGUARD_PARENT", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pids = None;
        for line in BufReader::new(parent.stdout.take().unwrap()).lines() {
            let line = line.unwrap();
            if let Some(rest) = line.strip_prefix("PGGUARD ") {
                let f: Vec<i32> = rest.split(' ').map(|s| s.parse().unwrap()).collect();
                pids = Some((f[0], f[1]));
                break;
            }
        }
        let (shell, child) = pids.expect("the parent never said its pids");
        parent.kill().unwrap();
        parent.wait().unwrap();
        let ok = gone_within(shell, Duration::from_secs(3))
            && gone_within(child, Duration::from_secs(3));
        if !ok {
            // SAFETY: the group this test made.
            unsafe { libc::kill(-shell, libc::SIGKILL) };
            panic!("the group outlived its parent: {shell} {child}");
        }
    }

    // Released, the guard leaves the group alone.
    #[test]
    fn release_leaves_the_group() {
        let mut c = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let g = watch(c.id() as i32).expect("no shell");
        let guard = g.pid;
        g.release();
        assert!(
            gone_within(guard, Duration::from_secs(3)),
            "the guard outlived its release"
        );
        assert!(alive(c.id() as i32), "a released guard ended the group");
        let _ = c.kill();
        let _ = c.wait();
    }
}
