//! Stores idle transcripts compressed the way macOS stores its own system
//! files (decmpfs): the file keeps its name and contents, every program
//! reads it as before, and the system decompresses as it reads. A file
//! written to again goes back to being stored plainly.
//!
//! Ported from internal/squeeze/squeeze.go:1-60, squeeze_darwin.go:1-147
//! and squeeze_other.go. The format is the system's (zlib blocks of 64 KB
//! in the resource fork, type 4), the same as Go writes.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What a pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Files compressed.
    pub files: usize,
    /// Their disk use before.
    pub before: i64,
    /// And after.
    pub after: i64,
    /// Files left as they were because something didn't check out.
    pub failed: usize,
}

/// Compresses the .jsonl transcripts under dirs untouched for idle, that
/// aren't compressed already. A file that fails any check is left exactly
/// as it was.
pub fn transcripts<P: AsRef<Path>>(dirs: &[P], idle: Duration) -> Pass {
    let mut r = Pass::default();
    let mut failed: Vec<String> = Vec::new(); // the first few, and why, for the log
    let cutoff = SystemTime::now()
        .checked_sub(idle)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for dir in dirs {
        // A folder that's a link isn't walked into, as Go's WalkDir doesn't:
        // what it links to is another folder's, compressed from there.
        if std::fs::symlink_metadata(dir.as_ref()).is_ok_and(|m| m.file_type().is_symlink()) {
            continue;
        }
        walk(dir.as_ref(), &mut |p| {
            if !p.to_string_lossy().ends_with(".jsonl") {
                return;
            }
            let Ok(info) = std::fs::symlink_metadata(p) else {
                return;
            };
            let old = info.modified().is_ok_and(|m| m <= cutoff);
            if info.len() < 64 << 10 || !old || compressed(&info) {
                return;
            }
            let before = disk_use(&info);
            if rush_core::dryrun::would(
                "compress-transcript",
                &p.to_string_lossy(),
                "a transcript untouched for two days, stored compressed (it reads the same)",
            ) {
                return;
            }
            if let Err(e) = file(p) {
                r.failed += 1;
                if failed.len() < 5 {
                    failed.push(format!("{}: {e}", p.display()));
                }
                return;
            }
            r.files += 1;
            r.before += before;
            if let Ok(st) = std::fs::metadata(p) {
                r.after += disk_use(&st);
            }
        });
    }
    if r.failed > 0 {
        crate::events::log(
            "squeeze-failed",
            serde_json::json!({"failed": r.failed, "compressed": r.files, "errs": failed}),
        );
    }
    r
}

/// Every file under dir, in lexical order, as Go's filepath.WalkDir:
/// links aren't followed into, and unreadable folders are passed by.
fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(PathBuf, bool)> = rd
        .flatten()
        .map(|e| (e.path(), e.file_type().is_ok_and(|t| t.is_dir())))
        .collect();
    entries.sort();
    for (p, is_dir) in entries {
        if is_dir {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}

#[cfg(target_os = "macos")]
pub use darwin::{compressed, encode, file};

#[cfg(target_os = "macos")]
fn disk_use(info: &Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    info.blocks() as i64 * 512
}

#[cfg(not(target_os = "macos"))]
pub fn compressed(_: &Metadata) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn disk_use(info: &Metadata) -> i64 {
    info.len() as i64
}

/// Only macOS stores files compressed.
#[cfg(not(target_os = "macos"))]
pub fn file(_: &Path) -> Result<(), String> {
    Err("only on macOS".into())
}

#[cfg(target_os = "macos")]
mod darwin {
    use std::ffi::CString;
    use std::fs::Metadata;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    /// decmpfs compresses 64 KB at a time.
    const BLOCK: usize = 64 << 10;
    /// zlib blocks in the resource fork.
    const TYPE_ZLIB_FORK: u32 = 4;
    const UF_COMPRESSED: u32 = libc::UF_COMPRESSED;

    /// Whether a file is stored compressed already.
    pub fn compressed(info: &Metadata) -> bool {
        use std::os::macos::fs::MetadataExt;
        info.st_flags() & UF_COMPRESSED != 0
    }

    /// The resource map every decmpfs resource fork ends with: one resource
    /// of type 'cmpf', id 1, no name.
    const FORK_MAP: [u8; 50] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1c, 0, 0x32, 0, 0, b'c', b'm', //
        b'p', b'f', 0, 0, 0, 0x0a, 0, 1, 0xff, 0xff, 0, 0, 0, 0, 0, 0, //
        0, 0,
    ];

    /// Builds the decmpfs header and resource fork for data.
    pub fn encode(data: &[u8]) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
        let n = data.len().div_ceil(BLOCK);
        let mut blocks = Vec::new();
        let mut table = vec![0u8; 4 + 8 * n];
        table[..4].copy_from_slice(&(n as u32).to_le_bytes());
        let mut off = table.len() as u32;
        for i in 0..n {
            let chunk = &data[i * BLOCK..((i + 1) * BLOCK).min(data.len())];
            let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
            z.write_all(chunk)?;
            let mut b = z.finish()?;
            if b.len() >= chunk.len() {
                // Doesn't shrink: stored as it is, marked by a leading 0xff.
                b = Vec::with_capacity(chunk.len() + 1);
                b.push(0xff);
                b.extend_from_slice(chunk);
            }
            table[4 + 8 * i..8 + 8 * i].copy_from_slice(&off.to_le_bytes());
            table[8 + 8 * i..12 + 8 * i].copy_from_slice(&(b.len() as u32).to_le_bytes());
            off += b.len() as u32;
            blocks.extend_from_slice(&b);
        }
        let mut res = table;
        res.extend_from_slice(&blocks);
        let data_len = 4 + res.len();
        let mut fork = vec![0u8; 0x100];
        fork.reserve(data_len + FORK_MAP.len());
        fork[0..4].copy_from_slice(&0x100u32.to_be_bytes());
        fork[4..8].copy_from_slice(&((0x100 + data_len) as u32).to_be_bytes());
        fork[8..12].copy_from_slice(&(data_len as u32).to_be_bytes());
        fork[12..16].copy_from_slice(&(FORK_MAP.len() as u32).to_be_bytes());
        fork.extend_from_slice(&(res.len() as u32).to_be_bytes());
        fork.extend_from_slice(&res);
        fork.extend_from_slice(&FORK_MAP);
        let mut header = vec![0u8; 16];
        header[..4].copy_from_slice(b"fpmc");
        header[4..8].copy_from_slice(&TYPE_ZLIB_FORK.to_le_bytes());
        header[8..16].copy_from_slice(&(data.len() as u64).to_le_bytes());
        Ok((header, fork))
    }

    fn cpath(p: &Path) -> Result<CString, String> {
        CString::new(p.as_os_str().as_bytes()).map_err(|e| e.to_string())
    }

    fn os_err() -> String {
        std::io::Error::last_os_error().to_string()
    }

    fn setxattr(p: &CString, name: &std::ffi::CStr, v: &[u8]) -> Result<(), String> {
        // SAFETY: p and name are NUL-terminated; v is valid for its length.
        let r =
            unsafe { libc::setxattr(p.as_ptr(), name.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0) };
        if r == 0 { Ok(()) } else { Err(os_err()) }
    }

    fn chflags(p: &CString, flags: u32) -> Result<(), String> {
        // SAFETY: p is NUL-terminated.
        if unsafe { libc::chflags(p.as_ptr(), flags) } == 0 {
            Ok(())
        } else {
            Err(os_err())
        }
    }

    /// Sets a file's access and modification times to t, without opening
    /// it (opening a compressed file to write would store it plainly).
    fn set_times(p: &CString, t: std::time::SystemTime) {
        let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let ts = libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        };
        // SAFETY: p is NUL-terminated; times holds two timespecs.
        unsafe { libc::utimensat(libc::AT_FDCWD, p.as_ptr(), [ts, ts].as_ptr(), 0) };
    }

    /// Compresses one file in place. It writes a compressed copy beside it,
    /// reads that back through the system and compares every byte, and only
    /// then puts it in the file's place, and only if the file hasn't changed
    /// meanwhile. Any failure leaves the file as it was.
    pub fn file(path: &Path) -> Result<(), String> {
        let before = std::fs::metadata(path).map_err(|e| e.to_string())?;
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let (header, fork) = encode(&data).map_err(|e| e.to_string())?;
        if fork.len() >= data.len() {
            return Err("doesn't compress".into());
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        let tmp = path.with_file_name(format!(".{name}.squeeze"));
        let _ = std::fs::remove_file(&tmp);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(before.permissions().mode() & 0o777)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        let ctmp = cpath(&tmp)?;
        let done = (|| {
            setxattr(&ctmp, c"com.apple.ResourceFork", &fork)
                .map_err(|e| format!("resource fork: {e}"))?;
            setxattr(&ctmp, c"com.apple.decmpfs", &header).map_err(|e| format!("decmpfs: {e}"))?;
            chflags(&ctmp, UF_COMPRESSED).map_err(|e| format!("chflags: {e}"))?;
            match std::fs::read(&tmp) {
                Ok(back) if back == data => {}
                _ => return Err("the compressed copy doesn't read back the same".to_string()),
            }
            let mtime = before.modified().map_err(|e| e.to_string())?;
            set_times(&ctmp, mtime);
            match std::fs::metadata(path) {
                Ok(now) if now.len() == before.len() && now.modified().ok() == Some(mtime) => {}
                _ => return Err("written to meanwhile".to_string()),
            }
            std::fs::rename(&tmp, path).map_err(|e| e.to_string())
        })();
        if done.is_err() {
            let _ = chflags(&ctmp, 0);
            let _ = std::fs::remove_file(&tmp);
        }
        done
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::io::Write;

    fn set_mtime(p: &Path, t: SystemTime) {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_times(std::fs::FileTimes::new().set_accessed(t).set_modified(t))
            .unwrap();
    }

    #[test]
    fn compresses_idle_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let mut b = String::new();
        let mut i = 0;
        while b.len() < (3 << 20) + 12345 {
            b.push_str(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"line "#);
            b.push_str(&"words ".repeat(i % 40));
            b.push_str("\"}]},\"cwd\":\"/work\"}\n");
            i += 1;
        }
        let want = b.into_bytes();
        std::fs::write(&p, &want).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        set_mtime(&p, SystemTime::now() - Duration::from_secs(72 * 3600));
        // A small one and a recent one are left alone.
        std::fs::write(dir.path().join("small.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("recent.jsonl"), &want).unwrap();
        let before = std::fs::metadata(&p).unwrap();

        let r = transcripts(&[dir.path()], Duration::from_secs(48 * 3600));
        assert!(
            r.files == 1 && r.failed == 0 && r.after < r.before / 3,
            "{r:?}"
        );
        let after = std::fs::metadata(&p).unwrap();
        assert!(compressed(&after));
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        assert_eq!(after.permissions().mode(), before.permissions().mode());
        assert_eq!(std::fs::read(&p).unwrap(), want, "reads back different");
        let out = std::process::Command::new("grep")
            .args(["-c", "line words words"])
            .arg(&p)
            .output()
            .unwrap();
        assert_ne!(String::from_utf8_lossy(&out.stdout).trim(), "0");
        // Appending, as Claude Code does, works and keeps everything.
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"new\":1}\n").unwrap();
        drop(f);
        let mut want2 = want.clone();
        want2.extend_from_slice(b"{\"new\":1}\n");
        assert_eq!(std::fs::read(&p).unwrap(), want2, "append lost data");
        // A second pass leaves compressed and recent files alone (the
        // appended one is recent now, and stored plainly again).
        assert!(!compressed(&std::fs::metadata(&p).unwrap()));
        let r = transcripts(&[dir.path()], Duration::from_secs(48 * 3600));
        assert_eq!(r.files, 0, "second pass {r:?}");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".squeeze"))
            .collect();
        assert!(left.is_empty(), "left a temp file");
    }

    #[test]
    fn incompressible_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        // Bytes no compressor shrinks.
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let data: Vec<u8> = (0..100_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        std::fs::write(&p, &data).unwrap();
        assert_eq!(file(&p), Err("doesn't compress".into()));
        assert_eq!(std::fs::read(&p).unwrap(), data);
        assert!(!compressed(&std::fs::metadata(&p).unwrap()));
    }

    #[test]
    fn encode_layout() {
        let data = vec![b'a'; BLOCK_TEST + 10];
        let (header, fork) = encode(&data).unwrap();
        assert_eq!(&header[..4], b"fpmc");
        assert_eq!(u32::from_le_bytes(header[4..8].try_into().unwrap()), 4);
        assert_eq!(
            u64::from_le_bytes(header[8..16].try_into().unwrap()),
            data.len() as u64
        );
        assert_eq!(u32::from_be_bytes(fork[0..4].try_into().unwrap()), 0x100);
        let res_len = u32::from_be_bytes(fork[0x100..0x104].try_into().unwrap()) as usize;
        let res = &fork[0x104..0x104 + res_len];
        assert_eq!(
            u32::from_le_bytes(res[..4].try_into().unwrap()),
            2,
            "two 64 KB blocks"
        );
        assert_eq!(u32::from_le_bytes(res[4..8].try_into().unwrap()), 4 + 8 * 2);
        assert_eq!(&fork[fork.len() - 20..fork.len() - 16], b"cmpf");
    }

    const BLOCK_TEST: usize = 64 << 10;
}
