//! Claude Code run leaner: its JS engine's optimising compilers off, and
//! garbage collected once a minute. Port of internal/adapters/claude/lean.go
//! (and its use at conn.go:105).
//!
//! Claude Code is JavaScript run by Bun, and two thirds of its heap is its
//! own code: compiled and bytecode. Bun reads its JS engine's options from
//! BUN_JSC_* and runs a --preload before Claude Code starts; with these an
//! idle session holds less. Measured (a turn of 137 file reads, then five
//! idle minutes, physical footprint): 103 MB every time with both, against
//! 108–126 MB without, for the same CPU. Each alone gets ~105; the JIT
//! entirely off stops Claude Code from starting, and discarding unused
//! bytecode saves a little more for twice the CPU.

use std::fs;
use std::path::PathBuf;

/// The engine option that keeps its two optimising compilers off.
pub(crate) const DFG_OFF: &str = "BUN_JSC_useDFGJIT";

/// Runs in Claude Code before it does: it takes rush's variables back out,
/// so the commands, hooks and MCP servers it starts get the environment
/// they would have without rush (the engine read them already), and
/// collects garbage once a minute, so a session gone idle hands back what
/// it freed.
pub(crate) const LEAN_PRELOAD: &str =
    "// rush: Claude Code's engine options are its own, not its children's.
delete process.env.BUN_OPTIONS;
delete process.env.BUN_JSC_useDFGJIT;
// A full collection a minute, so an idle session hands back what it freed.
setInterval(() => { try { Bun.gc(true); } catch {} }, 60_000).unref();
";

/// What rush adds to Claude Code's environment, or nothing when you set
/// BUN_OPTIONS or the option yourself: yours stand.
pub(crate) fn lean_env() -> Vec<String> {
    lean_env_with(|k| std::env::var(k).ok())
}

fn lean_env_with(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let set = |k: &str| get(k).is_some_and(|v| !v.is_empty());
    if set("BUN_OPTIONS") || set(DFG_OFF) {
        return Vec::new();
    }
    let Some(p) = write_lean_preload() else {
        return Vec::new();
    };
    let p = p.to_string_lossy().into_owned();
    // BUN_OPTIONS splits on spaces.
    if p.contains([' ', '\t', '"', '\'', '\\']) {
        return Vec::new();
    }
    vec![format!("BUN_OPTIONS=--preload {p}"), format!("{DFG_OFF}=0")]
}

/// Puts LEAN_PRELOAD in rush's cache, and is its path, or None when it
/// can't.
fn write_lean_preload() -> Option<PathBuf> {
    let d = rush_state::cache_path("claude");
    let path = d.join("lean-preload.js");
    if fs::read(&path).is_ok_and(|b| b == LEAN_PRELOAD.as_bytes()) {
        return Some(path);
    }
    fs::create_dir_all(&d).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&d, fs::Permissions::from_mode(0o700));
    }
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut o = fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    use std::io::Write;
    o.open(&tmp).ok()?.write_all(LEAN_PRELOAD.as_bytes()).ok()?;
    fs::rename(&tmp, &path).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // lean_test.go:11 (TestLeanEnv)
    #[test]
    fn lean_env() {
        let cache = tempfile::tempdir().unwrap();
        rush_state::set_thread_cache_dir(Some(cache.path().to_path_buf()));
        let p = cache.path().join("claude").join("lean-preload.js");
        let none = |_: &str| None;
        assert_eq!(
            lean_env_with(none),
            vec![
                format!("BUN_OPTIONS=--preload {}", p.display()),
                format!("{DFG_OFF}=0")
            ]
        );
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            LEAN_PRELOAD,
            "preload on disk"
        );
        assert!(LEAN_PRELOAD.contains(&format!("delete process.env.{DFG_OFF};")));
        for own in ["BUN_OPTIONS", DFG_OFF] {
            let got = lean_env_with(|k| (k == own).then(|| "x".to_string()));
            assert!(got.is_empty(), "your own {own}: {got:?}, want nothing");
        }
        rush_state::set_thread_cache_dir(None);
    }
}
