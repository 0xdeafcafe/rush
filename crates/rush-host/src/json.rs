//! JSON as Go's jsonx writes it: Marshal on one line, MarshalIndent with
//! two spaces. serde_json writes neither <, > nor & escaped, as Go's v2.

use serde::Serialize;

/// v as one line.
pub fn line<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

/// v indented by two spaces, as jsonx.MarshalIndent writes it.
pub fn indent<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    let f = serde_json::ser::PrettyFormatter::with_indent(b"  ");
    let mut s = serde_json::Serializer::with_formatter(&mut out, f);
    if v.serialize(&mut s).is_err() {
        return Vec::new();
    }
    out
}

/// Writes b to path by way of path.tmp, so no reader sees half of it.
pub fn write_atomic(path: &std::path::Path, b: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    write_private(&tmp, b)?;
    std::fs::rename(&tmp, path)
}

/// Writes b to path, readable by its owner only, as Go's 0o600.
pub fn write_private(path: &std::path::Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(b)
}
