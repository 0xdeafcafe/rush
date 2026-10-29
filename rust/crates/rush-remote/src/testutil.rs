//! What the tests share: one at a time over rush's folder (remote.json,
//! the switches and the sessions are one per process), and serve's
//! plugins known.

use std::sync::{Mutex, MutexGuard, Once};

static LOCK: Mutex<()> = Mutex::new(());

/// Holds rush's folder for one test, with serve's plugins registered and
/// remote.json, the devices and the switches as a fresh home has them.
pub fn home() -> MutexGuard<'static, ()> {
    static REG: Once = Once::new();
    REG.call_once(|| {
        for b in crate::plugins::bundles() {
            rush_plugin::register_bundle(b).forget();
        }
    });
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = crate::config::dir();
    for f in ["remote.json", "remote-devices.json", "remote-push.json"] {
        let _ = std::fs::remove_file(d.join(f));
    }
    for n in crate::plugins::bundles() {
        let _ = rush_plugin::mark_bundled(&n.manifest.name, false);
    }
    let _ = std::fs::remove_dir_all(rush_host::root());
    crate::plugins::switches_changed();
    crate::others::forget();
    g
}
