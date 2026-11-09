//! A process becoming a newer build of itself: what a host needs to move
//! onto a newly installed rush without stopping the agent it runs. exec
//! keeps the process (its pid, its children, its parent); the files it
//! keeps open across it are only those told to stay (`keep_open`), as
//! every file Rust opens is closed on exec.
//!
//! Rust-only: Go's host never moved in place.

use std::ffi::CString;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Keeps fd open across an exec.
pub fn keep_open(fd: RawFd) -> std::io::Result<()> {
    set_cloexec(fd, false)
}

/// Closes fd on exec again, as Rust opens every file: a move that didn't
/// happen leaves nothing open for what the process starts next.
pub fn close_on_exec(fd: RawFd) -> std::io::Result<()> {
    set_cloexec(fd, true)
}

/// Whether fd is open in this process.
pub fn is_open(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only reads the descriptor's flags.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

fn set_cloexec(fd: RawFd, on: bool) -> std::io::Result<()> {
    // SAFETY: F_GETFD/F_SETFD on a descriptor read and set its flags only.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let want = if on {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, want) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Replaces this process with exe run with args (args[0] included), in
/// this process's environment as it is now. It returns only when it
/// couldn't: why.
pub fn exec(exe: &Path, args: &[String]) -> std::io::Error {
    let Ok(path) = CString::new(exe.as_os_str().as_bytes()) else {
        return std::io::Error::other("the program's path has a NUL in it");
    };
    let mut owned = Vec::with_capacity(args.len());
    for a in args {
        match CString::new(a.as_bytes()) {
            Ok(c) => owned.push(c),
            Err(_) => return std::io::Error::other("an argument has a NUL in it"),
        }
    }
    let mut argv: Vec<*const libc::c_char> = owned.iter().map(|c| c.as_ptr()).collect();
    argv.push(std::ptr::null());
    // SAFETY: path and argv are NUL-terminated and outlive the call; execv
    // takes the environment as std::env has it (environ).
    unsafe { libc::execv(path.as_ptr(), argv.as_ptr()) };
    std::io::Error::last_os_error()
}

/// Waits for pid, a child of this process (one it took over across an
/// exec, so no std Child has it), to end, and reaps it: its wait status,
/// None when it isn't this process's child.
pub fn reap(pid: i32) -> Option<i32> {
    loop {
        let mut status = 0;
        // SAFETY: waitpid on one pid, its status written to ours.
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Some(status);
        }
        if r < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::process::Command;

    // A file kept open crosses into what the process execs; one left to
    // close on exec doesn't. The child is a fork and exec, as the host's
    // own exec is, of a shell that writes to the file by its number
    // (through /dev/fd: dash takes no fd past 9 after >&).
    #[test]
    fn a_kept_file_survives_exec_and_the_rest_close() {
        for keep in [true, false] {
            let (mut r, w) = std::io::pipe().unwrap();
            let fd = w.as_raw_fd();
            if keep {
                keep_open(fd).unwrap();
            }
            let st = Command::new("/bin/sh")
                .args(["-c", &format!("echo crossed >/dev/fd/{fd}")])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            drop(w);
            let mut got = String::new();
            r.read_to_string(&mut got).unwrap();
            if keep {
                assert!(st.success());
                assert_eq!(got, "crossed\n");
            } else {
                assert!(!st.success(), "a closed file was written to");
                assert_eq!(got, "");
            }
        }
    }

    #[test]
    fn close_on_exec_undoes_keep_open() {
        let (_r, w) = std::io::pipe().unwrap();
        let fd = w.as_raw_fd();
        keep_open(fd).unwrap();
        // SAFETY: reads the flags of our own pipe.
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        close_on_exec(fd).unwrap();
        // SAFETY: as above.
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert!(is_open(fd));
        assert!(keep_open(-1).is_err());
    }

    #[test]
    fn reap_takes_a_child_by_its_pid() {
        // As a child an exec handed on: no std Child waits on it.
        let pid = Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .map(|c| {
                let pid = c.id() as i32;
                std::mem::forget(c);
                pid
            })
            .unwrap();
        let st = reap(pid).unwrap();
        assert!(libc::WIFEXITED(st) && libc::WEXITSTATUS(st) == 3);
        assert_eq!(reap(pid), None, "reaped twice");
    }

    #[test]
    fn a_failed_exec_says_why() {
        let e = exec(Path::new("/nowhere/rush"), &["rush".into()]);
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }
}
