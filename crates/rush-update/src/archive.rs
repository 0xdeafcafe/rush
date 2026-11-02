//! Reads one file out of a release's tar.gz. The release workflow makes it
//! with tar (bsdtar on macOS, GNU tar on Linux): ustar headers, maybe PAX
//! ones before them, which are stepped over like any other entry.

use std::io::Read;

/// The file at path (with or without a leading ./) in the gzipped tar gz,
/// read up to limit bytes of tar.
pub fn find(gz: &[u8], path: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .take(limit)
        .read_to_end(&mut tar)
        .map_err(|e| format!("not a tar.gz: {e}"))?;
    let mut off = 0;
    while off + 512 <= tar.len() {
        let h = &tar[off..off + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size = octal(&h[124..136]).ok_or("a broken tar header")?;
        let body = off + 512;
        let end = body
            .checked_add(size)
            .filter(|&e| e <= tar.len())
            .ok_or("cut short")?;
        let mut name = field(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = field(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        let regular = matches!(h[156], b'0' | 0);
        if regular && name.strip_prefix("./").unwrap_or(&name) == path {
            return Ok(tar[body..end].to_vec());
        }
        off = body + size.div_ceil(512) * 512;
    }
    Err(format!("no {path} in it"))
}

/// A NUL-padded text field.
fn field(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// An octal number padded with spaces or NULs.
fn octal(b: &[u8]) -> Option<usize> {
    let s = field(b);
    let s = s.trim_matches(' ');
    if s.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(s, 8).ok()
}
