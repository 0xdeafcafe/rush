//! The roster of running rush views, so `rush reload` and #reload all can
//! tell the others to reload, and the lock that makes one view the one on
//! a folder.
//!
//! Ported from internal/instances/instances.go, with proc.Args
//! (internal/proc/proc_darwin.go:127-167, proc_linux.go:123-129). The
//! events Go logs here ("views-reloaded", "view-follows", "view-leads")
//! are left to the events module once it's ported.

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Mutex;

/// Holds one file per running view: <pid>, with the arguments it runs.
pub fn dir() -> PathBuf {
    crate::dir().join("ui")
}

/// This view's entry in the roster: it's listed until this is dropped.
pub struct Registered(Option<PathBuf>);

impl Drop for Registered {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

fn mkdir() -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir())
}

/// Lists this view until what it returns is dropped. Call it once the
/// SIGUSR1 handler is set: a view that isn't listed is never signalled.
pub fn register() -> Registered {
    let p = dir().join(std::process::id().to_string());
    let args: Vec<Vec<u8>> = std::env::args_os().map(|a| a.as_bytes().to_vec()).collect();
    if mkdir().is_err() || write_mode(&p, &args.join(&0u8), 0o600).is_err() {
        return Registered(None);
    }
    Registered(Some(p))
}

fn write_mode(p: &std::path::Path, b: &[u8], mode: u32) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(p)?
        .write_all(b)
}

/// Signals every listed view but skip to reload, and says how many. A pid
/// is signalled only while the process there still runs the arguments it
/// listed (a reused pid doesn't); otherwise its entry is dropped.
pub fn reload(skip: i32) -> usize {
    let Ok(ents) = fs::read_dir(dir()) else {
        return 0;
    };
    let mut n = 0;
    for e in ents.flatten() {
        let name = e.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid == skip {
            continue;
        }
        if !listed(pid) {
            let _ = fs::remove_file(e.path());
            continue;
        }
        // SAFETY: kill only sends a signal.
        if unsafe { libc::kill(pid, libc::SIGUSR1) } == 0 {
            n += 1;
        }
    }
    n
}

/// Whether pid is a view that's listed and still runs the arguments it
/// listed: a reused pid isn't.
fn listed(pid: i32) -> bool {
    let b = fs::read(dir().join(pid.to_string())).unwrap_or_default();
    if b.is_empty() {
        return false;
    }
    let want: Vec<&[u8]> = b.split(|c| *c == 0).collect();
    let got = proc_args(pid);
    got.len() == want.len() && got.iter().zip(&want).all(|(g, w)| g.as_slice() == *w)
}

/// Whether another view holds the lock, read from the pid it wrote rather
/// than by taking it, so a view starting then never finds it taken: an SSH
/// view leads only while none does.
pub fn led() -> bool {
    let b = fs::read(dir().join("lock")).unwrap_or_default();
    let pid: i32 = String::from_utf8_lossy(&b).trim().parse().unwrap_or(0);
    pid > 0 && pid as u32 != std::process::id() && listed(pid)
}

/// The lock file of the view that runs: kept so it isn't closed, which
/// would let it go.
static HELD: Mutex<Option<fs::File>> = Mutex::new(None);

/// Makes this the one rush view on this folder, until it exits: there is
/// one, as a view switches logins and restarts sessions' hosts for all of
/// them, and two would do it twice. Err is the pid of another that holds
/// it. An exec (#reload) lets it go, and the new rush takes it again.
pub fn lock() -> Result<u32, i32> {
    let p = dir().join("lock");
    if mkdir().is_err() {
        return Ok(0); // nowhere to lock: as before, rather than no rush at all
    }
    let Ok(f) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&p)
    else {
        return Ok(0);
    };
    // SAFETY: flock on a descriptor f owns.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let b = fs::read(&p).unwrap_or_default();
        return Err(String::from_utf8_lossy(&b).trim().parse().unwrap_or(0));
    }
    let _ = f.set_len(0);
    let _ = std::os::unix::fs::FileExt::write_at(&f, std::process::id().to_string().as_bytes(), 0);
    *HELD.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    Ok(std::process::id())
}

/// A process's arguments; empty when they can't be read (another user's
/// process, or none at pid).
pub fn proc_args(pid: i32) -> Vec<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        darwin_args(pid)
    }
    #[cfg(target_os = "linux")]
    {
        let b = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        // strings.TrimRight(b, "\x00"): every NUL at the end.
        let end = b.iter().rposition(|c| *c != 0).map_or(0, |i| i + 1);
        let b = &b[..end];
        if b.is_empty() {
            return Vec::new();
        }
        b.split(|c| *c == 0).map(<[u8]>::to_vec).collect()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        Vec::new()
    }
}

/// kern.procargs2: argc, the executable's path, padding, then argv.
#[cfg(target_os = "macos")]
fn darwin_args(pid: i32) -> Vec<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: asks the size only.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size < 4
    {
        return Vec::new();
    }
    let mut buf = vec![0u8; size];
    // SAFETY: buf holds size bytes.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size < 4
    {
        return Vec::new();
    }
    buf.truncate(size);
    let argc = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut b = &buf[4..];
    if let Some(i) = b.iter().position(|c| *c == 0) {
        b = &b[i..];
    }
    while b.first() == Some(&0) {
        b = &b[1..];
    }
    // argc is the kernel's word: never more than the buffer could hold.
    let mut args = Vec::with_capacity(argc.min(b.len()));
    while args.len() < argc && !b.is_empty() {
        match b.iter().position(|c| *c == 0) {
            Some(i) => {
                args.push(b[..i].to_vec());
                b = &b[i + 1..];
            }
            None => {
                args.push(b.to_vec());
                break;
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    fn home() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        crate::set_thread_dir(Some(d.path().to_path_buf()));
        d
    }

    #[test]
    fn register_and_remove() {
        let _d = home();
        let p = dir().join(std::process::id().to_string());
        let done = register();
        assert!(p.exists(), "not listed after register");
        drop(done);
        assert!(!p.exists(), "still listed after it ended");
    }

    /// A child that's killed and reaped when the test ends.
    struct Child(std::process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // spawn returns once the child has exec'd, so its arguments are its own.
    fn sleep(secs: &str) -> Child {
        Child(Command::new("sleep").arg(secs).spawn().unwrap())
    }

    fn list(pid: u32, args: &[&str]) -> PathBuf {
        let p = dir().join(pid.to_string());
        fs::write(&p, args.join("\0")).unwrap();
        p
    }

    #[test]
    fn reload_signals_only_the_running_view() {
        let _d = home();
        mkdir().unwrap();
        // A view: a process whose arguments are the ones it listed.
        let mut live = sleep("30");
        // A reused pid: the process there isn't running what was listed.
        let mut reused = sleep("31");
        let stale = list(reused.0.id(), &["rush"]);
        list(live.0.id(), &["sleep", "30"]);
        assert_eq!(reload(0), 1, "signalled views");
        // sleep dies of SIGUSR1: that is the signal arriving.
        let st = live.0.wait().unwrap();
        assert_eq!(
            st.signal(),
            Some(libc::SIGUSR1),
            "live view wasn't sent SIGUSR1: {st:?}"
        );
        assert!(!stale.exists(), "the stale entry wasn't dropped");
        assert!(
            reused.0.try_wait().unwrap().is_none(),
            "the reused pid was signalled"
        );
    }

    // One view runs on a folder: another is told which holds it. Led reads
    // who holds the lock without taking it: a listed, running view.
    #[test]
    fn one_view_and_led() {
        let _d = home();
        assert_eq!(
            lock(),
            Ok(std::process::id()),
            "the first view didn't get the lock"
        );
        // A second, as another process would: its own open file.
        assert_eq!(lock(), Err(std::process::id() as i32), "a second view ran");
        *HELD.lock().unwrap() = None;

        let _d = home();
        mkdir().unwrap();
        assert!(!led(), "led with no lock");
        let view = sleep("30");
        let pid = view.0.id().to_string();
        fs::write(dir().join("lock"), &pid).unwrap();
        assert!(!led(), "led by a pid that isn't a listed view");
        fs::write(dir().join(&pid), "sleep\x0030").unwrap();
        assert!(led(), "not led by the listed view holding it");
        assert!(lock().is_ok(), "led took the lock");
        *HELD.lock().unwrap() = None;
    }
}
