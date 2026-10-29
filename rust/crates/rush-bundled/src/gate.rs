//! The bundled plugin that shares the machine between the intensive
//! programs (tsc, go, cargo…) sessions run: see the gate. Its settings say
//! which programs, the memory and time each may take, and the memory kept
//! free; rush gate run and the shims read them from disk, each its own
//! process.
//! Ported from internal/bundled/gate/plugin.go:1-283.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use rush_plugin::rpc::no_method;
use rush_plugin::{Bundle, Cancel, Ctx, Manifest, Requires, SettingSpec};

use crate::gatecore::{self, GuardLog, Rule, Tell, quote};

/// The plugin's.
pub const NAME: &str = "gate";

/// When set, taken as the per-program limits with the gate on, over the
/// settings: for scripts and tests.
pub const RULES_ENV: &str = "RUSH_GATE_RULES";

/// The gate plugin's.
pub fn manifest() -> Manifest {
    let setting =
        |key: &str, title: &str, kind: &str, choices: &[&str], description: &str| SettingSpec {
            key: key.into(),
            title: title.into(),
            kind: kind.into(),
            choices: choices.iter().map(|c| c.to_string()).collect(),
            default: gatecore::default_setting(key).into(),
            description: description.into(),
        };
    Manifest {
        settings: vec![
            setting(
                "names",
                "Programs",
                "text",
                &[],
                "The programs to gate, by name, separated by commas",
            ),
            setting(
                "limit",
                "Time limit",
                "choice",
                &["10m", "15m", "30m", "1h", "2h"],
                "How long a run may go before it's stopped",
            ),
            setting(
                "reserve",
                "Kept free",
                "choice",
                &["5%", "10%", "15%", "20%", "25%"],
                "The share of memory no run starts into",
            ),
            setting(
                "floor",
                "Last resort",
                "choice",
                &["off", "3%", "5%", "8%", "10%"],
                "The memory left at which the largest program any agent started is stopped, gated or not",
            ),
            setting(
                "limits",
                "Per program",
                "text",
                &[],
                "Memory a run is expected to need and its time limit, either left out: tsc=6G/45m, vitest=4G. \
                 A run holding three times its memory is stopped",
            ),
        ],
        requires: Requires {
            os: vec!["darwin".into(), "linux".into()],
            ..Default::default()
        },
        ..crate::manifest(
            NAME,
            "machine",
            "Shares the machine between the intensive programs (tsc, vitest, go, golangci-lint…) your agents run: \
             each starts once there's memory for it, fairly by agent, and gives way on the CPU as it ages; a run that \
             outlives its agent or its limits is stopped, and the newest paused while memory is critical. Package scripts' \
             runs too, through a node preload. Watchers and dev servers go straight through. rush gate status shows them. \
             Past the gate, when the machine is all but out of memory it stops the largest program any agent started.",
            32,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, run)
}

fn env_rules() -> Option<String> {
    std::env::var(RULES_ENV).ok()
}

/// The rule for each program, or None when the gate is off.
pub fn rules() -> Option<HashMap<String, Rule>> {
    rules_from(env_rules())
}

/// rules, given what RULES_ENV says.
fn rules_from(env: Option<String>) -> Option<HashMap<String, Rule>> {
    if let Some(v) = env {
        return Some(gatecore::parse_rules(&BTreeMap::from([
            ("names".to_string(), String::new()),
            ("limits".to_string(), v),
        ])));
    }
    if !rush_plugin::bundled_on(NAME) {
        return None;
    }
    Some(gatecore::parse_rules(&rush_plugin::setting_values_of(
        &manifest(),
    )))
}

/// The share of memory kept free of gated runs.
pub fn reserve() -> f64 {
    if env_rules().is_some() || !rush_plugin::bundled_on(NAME) {
        return gatecore::parse_reserve(&BTreeMap::new());
    }
    gatecore::parse_reserve(&rush_plugin::setting_values_of(&manifest()))
}

/// The memory left, in percent, at which the guard stops agents'
/// programs; 0 when it's off or the gate is.
pub fn floor() -> i32 {
    if env_rules().is_some() {
        return gatecore::parse_floor(&BTreeMap::new());
    }
    if !rush_plugin::bundled_on(NAME) {
        return 0;
    }
    gatecore::parse_floor(&rush_plugin::setting_values_of(&manifest()))
}

static TELL: RwLock<Option<Tell>> = RwLock::new(None);
static LOG: RwLock<Option<GuardLog>> = RwLock::new(None);

/// Sets how the guard tells a session's agent why its program was
/// stopped: rush's command can reach sessions' inboxes, as this can't.
pub fn set_tell(t: Option<Tell>) {
    *TELL.write().unwrap_or_else(|e| e.into_inner()) = t;
}

/// Sets where the guard records what it did and why.
pub fn set_log(l: Option<GuardLog>) {
    *LOG.write().unwrap_or_else(|e| e.into_inner()) = l;
}

/// Makes bin_dir hold a shim for each program the gate queues, and nothing
/// else: none when it's off. It keeps the node preload in step too.
pub fn write_shims() {
    write_shims_for(rules());
}

fn write_shims_for(rules: Option<HashMap<String, Rule>>) {
    write_bin(rules.as_ref());
    write_node(rules.as_ref());
}

fn write_bin(rules: Option<&HashMap<String, Rule>>) {
    let d = gatecore::bin_dir();
    let exe = exe_for(&d);
    if exe.is_empty() || mkdir_all(&d, 0o700).is_err() {
        return;
    }
    let empty = HashMap::new();
    let rules = rules.unwrap_or(&empty);
    if let Ok(ents) = std::fs::read_dir(&d) {
        for e in ents.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !rules.contains_key(&n) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for name in rules.keys() {
        if name.contains('/') || name.starts_with('.') {
            continue;
        }
        let path = d.join(name);
        let want = format!(
            "#!/bin/sh\nexec {} gate shim {} \"$@\"\n",
            quote(&exe),
            quote(name)
        );
        write_if_changed(&path, want.as_bytes(), 0o700);
    }
}

/// Writes the node preload, always, since a session's NODE_OPTIONS names
/// it for its whole life, and the names it reads: none when the gate is
/// off, so it does nothing.
fn write_node(rules: Option<&HashMap<String, Rule>>) {
    let d = gatecore::node_dir();
    if mkdir_all(&d, 0o700).is_err() {
        return;
    }
    write_if_changed(
        &gatecore::preload_path(),
        gatecore::PRELOAD.as_bytes(),
        0o600,
    );
    let exe = exe_for(&d);
    let Some(rules) = rules.filter(|_| !exe.is_empty()) else {
        let _ = std::fs::remove_file(gatecore::names_path());
        return;
    };
    let mut names: Vec<&String> = rules.keys().collect();
    names.sort();
    let mut body = exe + "\n";
    for n in names {
        body.push_str(n);
        body.push('\n');
    }
    write_if_changed(&gatecore::names_path(), body.as_bytes(), 0o600);
}

/// NODE_OPTIONS with the preload required first, or as it is when it's
/// there already or the preload isn't written.
pub fn node_options(cur: &str) -> String {
    let p = gatecore::preload_path().to_string_lossy().into_owned();
    if cur.contains(&p) || p.contains([' ', '"', '\'', '\\']) {
        return cur.to_string();
    }
    if std::fs::metadata(&p).is_err() {
        return cur.to_string();
    }
    format!("--require {p} {cur}").trim().to_string()
}

fn write_if_changed(path: &Path, want: &[u8], mode: u32) {
    if std::fs::read(path).is_ok_and(|b| b == want) {
        return;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let wrote = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .and_then(|mut f| std::io::Write::write_all(&mut f, want));
    if wrote.is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn mkdir_all(d: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(d)
}

/// The running rush, links resolved, or "".
pub fn exe() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return String::new();
    };
    std::fs::canonicalize(&exe)
        .unwrap_or(exe)
        .to_string_lossy()
        .into_owned()
}

/// The rush a stand-in written into dir runs: the running one, unless
/// that's a scratch build (a test's, one in the temp folder) and dir
/// outlives it; then the rush on PATH, or "" for none.
pub fn exe_for(dir: &Path) -> String {
    let exe = exe();
    if !scratch(&exe) || scratch(&dir.to_string_lossy()) {
        return exe;
    }
    let Some(p) = rush_plugin::manifest::look_path("rush") else {
        return String::new();
    };
    let p = rush_plugin::manifest::eval_symlinks(&p).unwrap_or(p);
    if scratch(&p) {
        return String::new();
    }
    p
}

/// Whether path is somewhere that comes and goes: the temp folder, or a
/// build's own folder (Go's go-build cache; for Rust, cargo's deps folder,
/// where its test binaries are).
fn scratch(path: &str) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    for d in [tmp.as_path(), Path::new("/tmp"), Path::new("/private/tmp")] {
        let mut prefix = d.to_string_lossy().trim_end_matches('/').to_string();
        prefix.push('/');
        if path.starts_with(&prefix) {
            return true;
        }
    }
    path.contains("/go-build") || path.contains("/deps/")
}

/// Claude Code's PreToolUse hook for Bash: it gates what the gate's rules
/// name, and runs a long call as a script rush can follow. "" when there's
/// no rush to run it.
pub fn hook_command() -> String {
    let exe = exe();
    if exe.is_empty() {
        return String::new();
    }
    format!("{} gate hook", quote(&exe))
}

/// The plugin, on its connection to the broker: it keeps the shims in step
/// with the settings, and runs the guard while the floor is set.
fn run(s: UnixStream) -> Result<(), String> {
    let stop_guard: Arc<Mutex<Option<Cancel>>> = Arc::new(Mutex::new(None));
    let sg = stop_guard.clone();
    let guard = move || {
        let mut g = sg.lock().unwrap_or_else(|e| e.into_inner());
        // Started afresh each time, so it guards by the settings as they are.
        if let Some(c) = g.take() {
            c.cancel();
        }
        let f = floor();
        if f <= 0 {
            return;
        }
        let Some(rt) = gatecore::runtime() else {
            return;
        };
        let (ctx, cancel) = Ctx::background().with_cancel();
        *g = Some(cancel);
        let names: HashSet<String> = rules().unwrap_or_default().into_keys().collect();
        let guard = gatecore::Guard {
            floor: f,
            names,
            tell: TELL.read().unwrap_or_else(|e| e.into_inner()).clone(),
            log: LOG.read().unwrap_or_else(|e| e.into_inner()).clone(),
        };
        std::thread::spawn(move || rt.guard(guard, ctx));
    };
    let res = crate::serve(
        s,
        Arc::new(move |_, _, method, _| match method {
            "initialize" | "ui.settings" => {
                write_shims();
                guard();
                crate::empty()
            }
            "tools.list" => crate::no_tools(),
            _ => Err(no_method(method)),
        }),
    );
    if let Some(c) = stop_guard.lock().unwrap_or_else(|e| e.into_inner()).take() {
        c.cancel();
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Home, bundles_lock};

    // plugin_test.go TestShimsFollowTheRules
    #[test]
    fn shims_follow_the_rules() {
        let _h = Home::new();
        write_shims_for(rules_from(Some("tsc=1, go".into())));
        let b = std::fs::read_to_string(gatecore::bin_dir().join("tsc")).unwrap();
        assert!(b.contains(" gate shim 'tsc' \"$@\""), "tsc shim {b:?}");
        assert!(gatecore::bin_dir().join("go").exists());
        let names = std::fs::read_to_string(gatecore::names_path()).unwrap();
        assert!(names.ends_with("\ngo\ntsc\n"), "names {names:?}");
        assert_eq!(
            std::fs::read_to_string(gatecore::preload_path()).unwrap(),
            gatecore::PRELOAD
        );
        write_shims_for(rules_from(Some("go".into())));
        assert!(
            !gatecore::bin_dir().join("tsc").exists(),
            "tsc's shim stayed after it left the rules"
        );
        // Off, no shims and no names; the preload stays.
        write_shims_for(None);
        assert!(!gatecore::bin_dir().join("go").exists());
        assert!(!gatecore::names_path().exists());
        let opts = node_options("--max-old-space-size=4096");
        assert!(
            opts.starts_with("--require ") && opts.ends_with(" --max-old-space-size=4096"),
            "{opts}"
        );
        assert_eq!(node_options(&opts), opts, "required once");
    }

    // plugin_test.go TestOffHasNoRules
    #[test]
    fn off_has_no_rules() {
        let _b = bundles_lock();
        let _h = Home::new();
        let reg = rush_plugin::register_bundle(bundle());
        assert_eq!(rules_from(None), None, "rules while off");
        assert_eq!(floor(), 0);
        rush_plugin::set_bundled(NAME, true).unwrap();
        let r = rules_from(None).expect("on, it has rules");
        assert!(r.contains_key("tsc") && r.contains_key("cargo"));
        assert_eq!(floor(), 5);
        assert_eq!(reserve(), 0.15);
        reg.forget();
    }

    // plugin_test.go TestScratchBuildsKeepToScratch: a test's own build
    // runs the stand-ins it writes into a temp folder, but never the ones
    // a real folder keeps for every agent.
    #[test]
    fn scratch_builds_keep_to_scratch() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(exe_for(t.path()), exe(), "into a temp folder");
        // A home's folder, not the test's own home: that's in /tmp
        // (rush-testenv), which is scratch.
        let real = Path::new("/Users/someone")
            .join("Library")
            .join("Caches")
            .join("agtop")
            .join("shims");
        let got = exe_for(&real);
        assert!(got != exe() && !scratch(&got), "into a real folder: {got}");
        assert!(hook_command().ends_with("' gate hook"));
    }
}
