//! Paths, quoting and errors as Go's standard library has them, so what
//! rush-plugin hashes, checks and says is what Go rush does: filepath's
//! Clean, Join, Rel, Base and Dir; strconv.Quote (fmt's %q); FileMode's
//! bits (fmt's %o of one); and os errors' text ("open PATH: permission
//! denied").

use std::fs::Metadata;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;

/// filepath.Clean: the shortest path naming the same file, by text alone.
pub fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            _ => out.push(part),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// filepath.Join: the non-empty elements joined by / and cleaned; "" when
/// all are empty.
pub fn join(parts: &[&str]) -> String {
    let kept: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if kept.is_empty() {
        return String::new();
    }
    clean(&kept.join("/"))
}

/// filepath.IsAbs.
pub fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// filepath.Base: the last element, "." for "", "/" for only slashes.
pub fn base(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/".into();
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// filepath.Dir: all but the last element, cleaned.
pub fn dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => clean(&p[..=i]),
        None => ".".into(),
    }
}

/// filepath.Rel: targ relative to base, or an error when it can't be.
pub fn rel(base: &str, targ: &str) -> Result<String, String> {
    let (b, t) = (clean(base), clean(targ));
    if b == t {
        return Ok(".".into());
    }
    let b = if b == "." { String::new() } else { b };
    if is_abs(&b) != is_abs(&t) {
        return Err(format!("Rel: can't make {targ} relative to {base}"));
    }
    let bs: Vec<&str> = b.split('/').filter(|s| !s.is_empty()).collect();
    let ts: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    let common = bs.iter().zip(&ts).take_while(|(x, y)| x == y).count();
    if bs[common..].contains(&"..") {
        return Err(format!("Rel: can't make {targ} relative to {base}"));
    }
    let mut out: Vec<&str> = vec![".."; bs.len() - common];
    out.extend(&ts[common..]);
    if out.is_empty() {
        return Ok(".".into());
    }
    Ok(out.join("/"))
}

/// The path as text, as Go holds it.
pub fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// strconv.Quote, as fmt's %q writes a string: printable characters as
/// they are, others escaped. Printable is Go's unicode.IsPrint as near as
/// the standard library lets us tell: no control, format or private-use
/// characters, and no space but U+0020.
pub fn quote(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() + 2);
    out.push('"');
    let mut i = 0;
    while i < b.len() {
        let (ch, n) = decode_rune(&b[i..]);
        match ch {
            None => {
                out.push_str(&format!("\\x{:02x}", b[i]));
                i += 1;
                continue;
            }
            Some(c) => {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\x07' => out.push_str("\\a"),
                    '\x08' => out.push_str("\\b"),
                    '\x0c' => out.push_str("\\f"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\x0b' => out.push_str("\\v"),
                    c if is_print(c) => out.push(c),
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                        out.push_str(&format!("\\x{:02x}", c as u32))
                    }
                    c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push_str(&format!("\\U{:08x}", c as u32)),
                }
                i += n;
            }
        }
    }
    out.push('"');
    out
}

fn decode_rune(b: &[u8]) -> (Option<char>, usize) {
    let n = match b[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    if b.len() < n {
        return (None, 1);
    }
    match std::str::from_utf8(&b[..n]) {
        Ok(s) => (s.chars().next(), n),
        Err(_) => (None, 1),
    }
}

fn is_print(c: char) -> bool {
    let u = c as u32;
    if u < 0x80 {
        return (0x20..0x7f).contains(&u);
    }
    !(c.is_control()
        || (c.is_whitespace())
        || matches!(u, 0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e
            | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f | 0xfeff
            | 0xfff9..=0xfffb | 0xe000..=0xf8ff | 0xf0000..=0x10ffff | 0xd800..=0xdfff
            | 0xfffe | 0xffff))
}

// Go's fs.FileMode bits.
const MODE_DIR: u32 = 1 << 31;
const MODE_SYMLINK: u32 = 1 << 27;
const MODE_DEVICE: u32 = 1 << 26;
const MODE_NAMED_PIPE: u32 = 1 << 25;
const MODE_SOCKET: u32 = 1 << 24;
const MODE_SETUID: u32 = 1 << 23;
const MODE_SETGID: u32 = 1 << 22;
const MODE_CHAR_DEVICE: u32 = 1 << 21;
const MODE_STICKY: u32 = 1 << 20;

/// The file's mode as Go's fs.FileMode has it: permission bits, then a
/// bit of its own for each kind of file and for setuid, setgid and sticky.
pub fn file_mode(md: &Metadata) -> u32 {
    let raw = md.permissions().mode();
    let mut m = raw & 0o777;
    let ft = md.file_type();
    if ft.is_dir() {
        m |= MODE_DIR;
    } else if ft.is_symlink() {
        m |= MODE_SYMLINK;
    } else if ft.is_block_device() {
        m |= MODE_DEVICE;
    } else if ft.is_char_device() {
        m |= MODE_DEVICE | MODE_CHAR_DEVICE;
    } else if ft.is_fifo() {
        m |= MODE_NAMED_PIPE;
    } else if ft.is_socket() {
        m |= MODE_SOCKET;
    }
    if raw & libc::S_ISGID as u32 != 0 {
        m |= MODE_SETGID;
    }
    if raw & libc::S_ISUID as u32 != 0 {
        m |= MODE_SETUID;
    }
    if raw & libc::S_ISVTX as u32 != 0 {
        m |= MODE_STICKY;
    }
    m
}

/// The file's modification time in Unix nanoseconds: Go's
/// ModTime().UnixNano().
pub fn mod_nanos(md: &Metadata) -> i64 {
    md.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(md.mtime_nsec())
}

/// An os error as Go words it: "open /x: no such file or directory".
pub fn os_err(op: &str, p: &Path, e: &io::Error) -> String {
    format!("{op} {}: {}", p.display(), errno_text(e))
}

/// An error's own words, as Go's syscall.Errno says them: lower case,
/// without Rust's "(os error N)".
pub fn errno_text(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            // SAFETY: strerror returns a pointer to a static string.
            let s = unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }
                .to_string_lossy()
                .into_owned();
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
                None => s,
            }
        }
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_join_rel() {
        for (i, w) in [
            ("", "."),
            ("/", "/"),
            ("a/../..", ".."),
            ("/a/../..", "/"),
            ("a//b/./c/", "a/b/c"),
            ("../../bin/sh", "../../bin/sh"),
        ] {
            assert_eq!(clean(i), w, "{i}");
        }
        assert_eq!(join(&["/home", "/x"]), "/home/x");
        assert_eq!(join(&["", ""]), "");
        assert_eq!(rel("/r", "/r/.rush/plugins/d").unwrap(), ".rush/plugins/d");
        assert_eq!(rel("/r/a", "/r/b").unwrap(), "../b");
        assert_eq!(rel("/r", "/r").unwrap(), ".");
        assert!(rel("/r", "x").is_err());
        assert_eq!(base("/x/demo/"), "demo");
        assert_eq!(dir("/x/demo"), "/x");
    }

    #[test]
    fn quote_as_go() {
        assert_eq!(quote(b"plugin.json"), "\"plugin.json\"");
        assert_eq!(quote(b"a\"b\\c\n\x01"), "\"a\\\"b\\\\c\\n\\x01\"");
        assert_eq!(quote("héllo".as_bytes()), "\"héllo\"");
        assert_eq!(quote(b"\xff"), "\"\\xff\"");
        assert_eq!(quote("\u{200e}".as_bytes()), "\"\\u200e\"");
    }
}
