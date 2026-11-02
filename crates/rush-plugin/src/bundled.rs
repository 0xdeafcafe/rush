//! Plugins that ship inside rush.
//! Ported from internal/plugin/bundled.go.
//!
//! A bundled plugin's code is rush's own, so it's trusted as rush is: it
//! runs without being approved, outside the sandbox, on every system, as
//! `rush plugin run <name>` speaking the same protocol on fd 3 as any
//! other. What it may do in rush's screen is still its manifest's, checked
//! by the broker as for any plugin. Each is on until you turn it off,
//! unless it's optional.

use std::collections::BTreeMap;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::manifest::{Manifest, PROTO_RUSH, name_ok};
use crate::plugin::{Approval, approvals};

/// Its main, given its end of the connection to the broker.
pub type RunFn = Arc<dyn Fn(UnixStream) -> Result<(), String> + Send + Sync>;
/// Run in the process that turns it on or off, after the switch is
/// written: what it keeps running outside the broker (a service, say)
/// starts or stops with it. An error puts the switch back as it was.
pub type SwitchedFn = Arc<dyn Fn(bool) -> Result<(), String> + Send + Sync>;

/// A plugin that ships inside rush.
#[derive(Clone)]
pub struct Bundle {
    pub manifest: Manifest,
    /// Off until you turn it on.
    pub optional: bool,
    pub run: Option<RunFn>,
    pub switched: Option<SwitchedFn>,
}

/// Stands for a bundled plugin's files: they're rush's binary, which the
/// broker runs from.
const BUNDLED_DIGEST: &str = "bundled";

static BUNDLES: RwLock<BTreeMap<String, Bundle>> = RwLock::new(BTreeMap::new());

/// A bundled plugin made known, which forget makes unknown again (for
/// tests).
#[must_use = "registered for good unless forgotten"]
pub struct Registered(String);

impl Registered {
    pub fn forget(self) {
        BUNDLES
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Makes a bundled plugin known. A manifest that asks for what a bundled
/// plugin can't have is a bug in rush, and panics.
pub fn register_bundle(b: Bundle) -> Registered {
    if let Err(e) = b.manifest.validate_bundled() {
        panic!("bundled plugin {}: {e}", b.manifest.name);
    }
    let name = b.manifest.name.clone();
    BUNDLES
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name.clone(), b);
    Registered(name)
}

impl Manifest {
    fn validate_bundled(&self) -> Result<(), String> {
        if !name_ok(&self.name) {
            return Err("bad name".into());
        }
        if self.proto() != PROTO_RUSH || !self.network.is_empty() || !self.exec.is_empty() {
            return Err("a bundled plugin speaks rush's protocol, with no network or exec".into());
        }
        self.validate_ui()?;
        self.validate_category()?;
        self.validate_services()?;
        self.validate_cli()
    }
}

/// The bundled plugins, by name.
pub fn bundles() -> Vec<Bundle> {
    BUNDLES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect()
}

/// The bundled plugin called name.
pub fn bundle_named(name: &str) -> Option<Bundle> {
    BUNDLES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .cloned()
}

fn off_path() -> PathBuf {
    crate::root().join("bundled-off.json")
}

/// Lists the optional ones you turned on.
fn on_path() -> PathBuf {
    crate::root().join("bundled-on.json")
}

fn read_list(path: &Path) -> Vec<String> {
    read_list_nil(path).unwrap_or_default()
}

/// The list, None where Go's would be nil: no file, or null in it.
fn read_list_nil(path: &Path) -> Option<Vec<String>> {
    std::fs::read(path)
        .ok()
        .and_then(|b| rush_state::json::from_slice::<Option<Vec<String>>>(&b).ok())
        .flatten()
}

fn write_list(path: &Path, list: Option<Vec<String>>) -> Result<(), String> {
    crate::mkdir_all(&crate::root())?;
    // A nil list is written null, as Go writes it.
    let b = match list {
        None => b"null".to_vec(),
        Some(mut list) => {
            list.sort();
            rush_state::json::to_vec(&list).map_err(|e| e.to_string())?
        }
    };
    crate::write_atomic(path, &b)
}

/// Whether you have a bundled plugin on. It runs only if its manifest's
/// requirements are met too: see unmet.
pub fn bundled_on(name: &str) -> bool {
    bundle_named(name).is_some_and(|b| on(&b, &read_list(&off_path()), &read_list(&on_path())))
}

fn on(b: &Bundle, off: &[String], on: &[String]) -> bool {
    if b.optional {
        return on.contains(&b.manifest.name);
    }
    !off.contains(&b.manifest.name)
}

/// Which bundled plugins you have on, by name. It reads the disk, so it's
/// not for the UI thread.
pub fn bundles_on() -> BTreeMap<String, bool> {
    let (off, onl) = (read_list(&off_path()), read_list(&on_path()));
    bundles()
        .iter()
        .map(|b| (b.manifest.name.clone(), on(b, &off, &onl)))
        .collect()
}

/// Turns a bundled plugin on or off, and runs its switched.
pub fn set_bundled(name: &str, turn_on: bool) -> Result<(), String> {
    let Some(b) = bundle_named(name) else {
        return Err(format!("{name} isn't bundled with rush"));
    };
    let was = bundled_on(name);
    mark_bundled(name, turn_on)?;
    let Some(sw) = b.switched else {
        return Ok(());
    };
    if let Err(e) = sw(turn_on) {
        let _ = mark_bundled(name, was);
        return Err(e);
    }
    Ok(())
}

/// Writes a bundled plugin's switch without running its switched: for
/// what's already so, a service found running, say.
pub fn mark_bundled(name: &str, turn_on: bool) -> Result<(), String> {
    let Some(b) = bundle_named(name) else {
        return Err(format!("{name} isn't bundled with rush"));
    };
    let (path, add) = if b.optional {
        (on_path(), turn_on)
    } else {
        (off_path(), !turn_on)
    };
    let list =
        read_list_nil(&path).map(|l| l.into_iter().filter(|n| n != name).collect::<Vec<_>>());
    let list = match (list, add) {
        (l, true) => Some(
            l.unwrap_or_default()
                .into_iter()
                .chain([name.to_string()])
                .collect(),
        ),
        (l, false) => l,
    };
    write_list(&path, list)
}

/// Every plugin that runs, by name: the approved ones, and the bundled
/// ones turned on, less any whose requirements aren't met here. A bundled
/// plugin's name is its own: an installed plugin of the same name doesn't
/// run.
pub fn enabled() -> BTreeMap<String, Approval> {
    let mut out = approvals();
    out.retain(|_, a| a.manifest.unmet().is_empty());
    let (off, onl) = (read_list(&off_path()), read_list(&on_path()));
    for b in bundles() {
        out.remove(&b.manifest.name);
        if on(&b, &off, &onl) && b.manifest.unmet().is_empty() {
            out.insert(
                b.manifest.name.clone(),
                Approval {
                    digest: BUNDLED_DIGEST.into(),
                    manifest: b.manifest.clone(),
                    ..Default::default()
                },
            );
        }
    }
    out
}

/// Runs the bundled plugin name on fd 3, as the broker starts it.
pub fn run_bundled(name: &str) -> Result<(), String> {
    let Some(b) = bundle_named(name) else {
        return Err(format!("{name} isn't bundled with rush"));
    };
    // SAFETY: F_GETFD only asks whether fd 3 is open.
    if unsafe { libc::fcntl(3, libc::F_GETFD) } < 0 {
        return Err("no connection on fd 3".into());
    }
    // SAFETY: fd 3 is open, and is the broker's socket, ours to own.
    let s = unsafe { UnixStream::from_raw_fd(3) };
    match b.run {
        Some(run) => run(s),
        None => Err(format!("{name} has nothing to run")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Requires;
    use crate::plugin::tests::{Home, install_plugin, manifest};
    use crate::plugin::{approve, load, verify};

    fn bundle(name: &str) -> Bundle {
        Bundle {
            manifest: manifest(name, "rush"),
            optional: false,
            run: Some(Arc::new(|_| Ok(()))),
            switched: None,
        }
    }

    #[test]
    fn bundled_on_until_turned_off() {
        let _b = crate::plugin::tests::bundles_lock();
        let _h = Home::new();
        let mut b = bundle("bundle-test");
        b.manifest.ui = vec![crate::ui::UI_NOTIFY.into()];
        let reg = register_bundle(b);
        assert!(
            enabled().contains_key("bundle-test") && bundled_on("bundle-test"),
            "on without approval"
        );
        let p = verify("bundle-test").unwrap();
        assert!(p.bundled && p.dir.is_empty(), "verify: {p:?}");
        set_bundled("bundle-test", false).unwrap();
        assert!(
            !enabled().contains_key("bundle-test") && !bundled_on("bundle-test"),
            "turned off, it shouldn't run"
        );
        assert!(
            verify("bundle-test").is_err(),
            "a bundled plugin turned off shouldn't verify"
        );
        set_bundled("bundle-test", true).unwrap();
        assert!(bundled_on("bundle-test"));
        assert!(
            set_bundled("not-bundled", false).is_err(),
            "only a bundled plugin can be turned off"
        );
        assert_eq!(
            std::fs::read_to_string(crate::root().join("bundled-off.json")).unwrap(),
            "[]"
        );
        reg.forget();
    }

    #[test]
    fn enabled_skips_unmet_requirements() {
        let _b = crate::plugin::tests::bundles_lock();
        let _h = Home::new();
        let mut b = bundle("bundle-never");
        b.manifest.requires = Requires {
            os: vec!["never-an-os".into()],
            ..Default::default()
        };
        let reg = register_bundle(b);
        assert!(
            !enabled().contains_key("bundle-never"),
            "a bundled plugin whose OS isn't this one never runs"
        );
        let mut m = manifest("p", "bin");
        m.requires.bin = vec!["never-a-real-binary-xyz".into()];
        approve(&load(&install_plugin(&m, &[])).unwrap()).unwrap();
        assert!(
            !enabled().contains_key("p"),
            "an approved plugin whose binary is missing never runs"
        );
        reg.forget();
    }

    #[test]
    #[should_panic(expected = "bundled plugin bundle-net")]
    fn bundled_manifest_is_checked() {
        let mut b = bundle("bundle-net");
        b.manifest.network = vec!["example.com:443".into()];
        let _ = register_bundle(b);
    }

    #[test]
    fn optional_bundle() {
        let _b = crate::plugin::tests::bundles_lock();
        let _h = Home::new();
        let mut opt = bundle("bundle-opt");
        opt.optional = true;
        let mut needs = bundle("bundle-needs");
        needs.optional = true;
        needs.manifest.requires.bin = vec!["never-a-real-binary-xyz".into()];
        let (r1, r2) = (register_bundle(opt), register_bundle(needs));
        assert!(
            !enabled().contains_key("bundle-opt")
                && !bundled_on("bundle-opt")
                && !bundles_on()["bundle-opt"],
            "an optional bundle is off until turned on"
        );
        for n in ["bundle-opt", "bundle-needs"] {
            set_bundled(n, true).unwrap();
        }
        assert!(
            enabled().contains_key("bundle-opt") && bundles_on()["bundle-opt"],
            "turned on, it runs"
        );
        assert!(
            !enabled().contains_key("bundle-needs") && bundled_on("bundle-needs"),
            "on, but missing what it requires"
        );
        set_bundled("bundle-opt", false).unwrap();
        assert!(!bundled_on("bundle-opt"));
        r1.forget();
        r2.forget();
    }

    #[test]
    fn a_failed_switch_is_put_back() {
        let _b = crate::plugin::tests::bundles_lock();
        let _h = Home::new();
        let mut b = bundle("bundle-sw");
        b.switched = Some(Arc::new(
            |on| if on { Ok(()) } else { Err("can't stop".into()) },
        ));
        let reg = register_bundle(b);
        assert_eq!(set_bundled("bundle-sw", false), Err("can't stop".into()));
        assert!(bundled_on("bundle-sw"), "the switch went back");
        reg.forget();
    }
}
