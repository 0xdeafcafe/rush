//! macOS's malloc in its space-efficient mode for rush's views. A view
//! keeps tens of thousands of small allocations for as long as it runs,
//! made among as many that don't last: with malloc's defaults the pages
//! they leave half used stay the process's, about 40 MB of an idle list's
//! 85 MB footprint; space-efficient, the list sits at about half that, for
//! the same instructions. malloc reads `MallocSpaceEfficient` once, as the
//! process starts, so a view starts itself again with it set (exec: the
//! same process, its terminal and arguments), and takes it out of its own
//! environment then, so nothing it starts inherits it. Rust-only; DESIGN.md
//! "Hosted sessions' memory" does the same for Codex.

/// What malloc reads.
const VAR: &str = "MallocSpaceEfficient";
/// Says rush set VAR itself, to take out again.
const MARK: &str = "RUSH_MALLOC_SET";

/// Starts this process again in malloc's space-efficient mode, unless it
/// already is; returns only when it is, or it can't be. Call it first in
/// main, before any thread starts.
pub fn space_efficient() {
    if !cfg!(target_os = "macos") {
        return;
    }
    if std::env::var_os(MARK).is_some() {
        // SAFETY: no other thread runs yet (the caller's promise), so none
        // reads the environment while it changes.
        unsafe {
            std::env::remove_var(MARK);
            std::env::remove_var(VAR);
        }
        return;
    }
    if std::env::var_os(VAR).is_some() {
        return; // whoever started rush chose already
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut args = std::env::args_os();
    let Some(arg0) = args.next() else { return };
    use std::os::unix::process::CommandExt;
    // exec returns only when it fails: carry on as malloc is.
    let _ = std::process::Command::new(exe)
        .arg0(arg0)
        .args(args)
        .env(VAR, "1")
        .env(MARK, "1")
        .exec();
}
