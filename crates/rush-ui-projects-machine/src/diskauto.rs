//! The disk engine on its own (Rust-only; Go had no engine): with the
//! switch on (config.json's disk.auto, "Clean on its own when disk is
//! low"), the rush that leads runs rush-fleet's disk engine every
//! AUTO_EVERY, every AUTO_EVERY_LOW while free is under disk::LOW_UNDER,
//! off the UI and never two at once (engine_lock, which the dire clean
//! holds too). It looks where rush's sessions work (their repos and
//! worktrees, the scratch roots, the processes' folders), takes what the
//! plugins that answer disk.candidates report within CANDIDATES_WAIT, and
//! says what it freed in the news; nothing when nothing. Every run is the
//! engine's disk-clean event.
//!
//! Once, before the first run: the clean plugins' own auto mode is taken
//! in. If rust-clean's or git-clean's auto-clean setting was on, the
//! switch is turned on; git-clean's worktree-idle-days, when set, becomes
//! the worktree rule's idle (disk::update_rules). Both are read straight
//! from plugins/settings.json: the plugins no longer declare auto-clean.
//! Ported from rush-bundled's clean auto mode as of dbaaff71, before the
//! plugins lost it (clean/mod.rs's auto tick: its intervals; auto_on and
//! reap.rs reap_after: the settings it read).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rush_core::time::Ms;
use rush_fleet::disk::{self, Candidate, Kind, Outcome, RulesFile, Scan, Session};
use rush_plugin::{Ctx, SERVICE_DISK_CANDIDATES};

use super::data::*;
use crate::frame::Out;
use crate::model::Model;
use crate::style::disk as size;

/// How often the engine runs with the switch on.
pub const AUTO_EVERY: Ms = 15 * 60_000;
/// How often while free is under disk::LOW_UNDER.
pub const AUTO_EVERY_LOW: Ms = 3 * 60_000;
/// How long the plugins have to say what they found; one that hasn't by
/// then is left out of this run.
pub const CANDIDATES_WAIT: Duration = Duration::from_secs(10);
/// The clean plugins whose auto mode the switch replaces, and its setting.
pub const AUTO_PLUGINS: &[&str] = &["rust-clean", "git-clean"];
pub const AUTO_KEY: &str = "auto-clean";
/// git-clean's: days a worktree sits idle before it may go.
pub const IDLE_DAYS_PLUGIN: &str = "git-clean";
pub const IDLE_DAYS_KEY: &str = "worktree-idle-days";

const DAY_SECS: u64 = 24 * 3600;

static ENGINE: Mutex<()> = Mutex::new(());

/// One engine run at a time in this rush: held for the whole of one, by
/// this and the dire clean alike.
pub fn engine_lock() -> MutexGuard<'static, ()> {
    ENGINE.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long between runs at free bytes free.
pub fn auto_every(free: u64) -> Ms {
    if free > 0 && free < disk::LOW_UNDER {
        AUTO_EVERY_LOW
    } else {
        AUTO_EVERY
    }
}

/// Whether a run is due: never one before (last 0), or auto_every(free)
/// since the last.
pub fn auto_due(last: Ms, free: u64, now: Ms) -> bool {
    last == 0 || now - last >= auto_every(free)
}

/// What a run did, in a few words; "" when it did nothing, which isn't
/// said.
pub fn auto_words(o: &Outcome) -> String {
    let n = o.did.len();
    if n == 0 {
        return String::new();
    }
    format!(
        "disk: rush freed {} on its own ({n} thing{}, by its rules)",
        size(o.freed as i64),
        if n == 1 { "" } else { "s" }
    )
}

/// The switch's state between frames.
#[derive(Default)]
pub struct DiskAuto {
    /// When the last run started; 0 before one.
    pub at: Ms,
    pub running: bool,
    /// The plugins' settings are being taken in, or were.
    pub taking_in: bool,
    pub taken_in: bool,
    /// The last error said, so it's said once, not every run.
    pub said_err: String,
}

/// What the clean plugins' settings said, to take in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginsSaid {
    /// Either plugin's auto-clean was on.
    pub auto: bool,
    /// git-clean's worktree-idle-days, when set.
    pub idle_days: Option<u64>,
}

/// What the plugins' stored settings say, by plugin then key: auto-clean
/// "true" in either AUTO_PLUGINS, and git-clean's idle days when it's a
/// whole number of at least 1.
pub fn plugins_said(stored: &BTreeMap<String, BTreeMap<String, String>>) -> PluginsSaid {
    let get = |p: &str, k: &str| stored.get(p).and_then(|v| v.get(k)).map(String::as_str);
    PluginsSaid {
        auto: AUTO_PLUGINS
            .iter()
            .any(|p| get(p, AUTO_KEY) == Some("true")),
        idle_days: get(IDLE_DAYS_PLUGIN, IDLE_DAYS_KEY)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&n| n >= 1),
    }
}

/// f with the worktree rules' idle at both levels days long.
pub fn with_worktree_idle(f: &mut RulesFile, days: u64) {
    for r in f.rules.iter_mut().filter(|r| r.kind == Kind::Worktree) {
        for p in [r.normal.as_mut(), r.low.as_mut()].into_iter().flatten() {
            p.idle_secs = Some(days * DAY_SECS);
        }
    }
}

/// The settings rush keeps for plugins (plugins/settings.json, by plugin
/// then key), read as stored: not through a plugin's manifest, which no
/// longer offers auto-clean. Values that aren't text are left out; no
/// file, or one that can't be read, is none.
pub fn stored_settings() -> BTreeMap<String, BTreeMap<String, String>> {
    let Ok(b) = std::fs::read(rush_plugin::root().join("settings.json")) else {
        return BTreeMap::new();
    };
    let Ok(all) =
        serde_json::from_slice::<BTreeMap<String, Option<BTreeMap<String, serde_json::Value>>>>(&b)
    else {
        return BTreeMap::new();
    };
    all.into_iter()
        .map(|(p, kv)| {
            let kv = kv
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_string())))
                .collect();
            (p, kv)
        })
        .collect()
}

/// Takes the clean plugins' auto mode in, once: None when it was taken in
/// before; else the idle days made the worktree rule's (update_rules) and
/// whether the switch should go on. The caller keeps that in config.json,
/// with pluginsTakenIn. Off the UI.
pub fn take_plugins_in() -> Result<Option<bool>, String> {
    if rush_state::Store::load().config.disk.plugins_taken_in {
        return Ok(None);
    }
    let said = plugins_said(&stored_settings());
    if let Some(days) = said.idle_days {
        disk::update_rules(|f| with_worktree_idle(f, days))
            .map_err(|e| format!("couldn't keep git-clean's worktree idle days: {e}"))?;
    }
    Ok(Some(said.auto))
}

/// What the plugins that answer disk.candidates report, within
/// CANDIDATES_WAIT; one that doesn't answer, or answers wrong, is left
/// out. Off the UI.
pub fn plugin_candidates() -> Vec<Candidate> {
    if rush_plugin::providers(SERVICE_DISK_CANDIDATES).is_empty() {
        return Vec::new();
    }
    let ctx = Ctx::background().with_timeout(CANDIDATES_WAIT);
    let answers = rush_plugin::ask_service(&ctx, SERVICE_DISK_CANDIDATES).unwrap_or_default();
    answers
        .into_iter()
        .filter(|a| a.error.is_empty())
        .filter_map(|a| {
            let v: serde_json::Value = serde_json::from_str(a.result.as_ref()?.get()).ok()?;
            disk::from_answer(&a.plugin, &v).ok()
        })
        .flatten()
        .collect()
}

/// Where the engine looks, as rush knows it on the UI: the repos its
/// recent sessions work in, every session with a folder (its id, where,
/// whether it's mid-turn), HOME. The scratch roots and the processes'
/// folders are read off the UI (complete_scan).
pub fn scan_of(m: &Model) -> Scan {
    let mut repos: Vec<PathBuf> = Vec::new();
    for g in work_agents(m) {
        let root = if g.root.is_empty() {
            g.a.job.cwd.as_str()
        } else {
            g.root
        };
        let p = PathBuf::from(root);
        if p.is_absolute() && !repos.contains(&p) {
            repos.push(p);
        }
    }
    let sessions = agents(m)
        .filter_map(|g| {
            let cwd = if g.a.job.cwd.is_empty() {
                g.a.repo.as_str()
            } else {
                g.a.job.cwd.as_str()
            };
            let cwd = PathBuf::from(cwd);
            if !cwd.is_absolute() {
                return None;
            }
            let id = if g.a.job.id.is_empty() {
                g.a.key.clone()
            } else {
                g.a.job.id.clone()
            };
            Some(Session {
                id,
                cwd,
                working: g.in_turn(),
            })
        })
        .collect();
    Scan {
        repos,
        scratch: Vec::new(),
        sessions,
        home: std::env::var("HOME").unwrap_or_default(),
        cwds: None,
    }
}

/// s with what's read off the UI: the scratch roots and the folders
/// processes work in.
pub fn complete_scan(mut s: Scan) -> Scan {
    s.scratch = rush_fleet::temp_roots();
    s.cwds = Some(rush_fleet::process_cwds());
    s
}

/// What a run came back with.
pub struct AutoRun {
    pub outcome: Outcome,
    /// Why it couldn't run; "" when it did.
    pub err: String,
}

/// One run of the engine on the rules as kept, with the plugins'
/// candidates, holding engine_lock. Off the UI.
pub fn auto_run(scan: Scan, now: Ms) -> AutoRun {
    let _one = engine_lock();
    let free = rush_sys::sysinfo::read_disk().free;
    let rules = match disk::load_rules() {
        Ok(f) => f.rules,
        Err(e) => {
            let err = format!("couldn't read the disk rules, so rush cleaned nothing: {e}");
            let mut f = serde_json::Map::new();
            f.insert("free".into(), free.into());
            f.insert("why".into(), err.clone().into());
            rush_events::log("disk-clean", f);
            return AutoRun {
                outcome: Outcome::default(),
                err,
            };
        }
    };
    let handed = if disk::level(free).is_some() {
        plugin_candidates()
    } else {
        Vec::new()
    };
    let outcome = disk::run(&complete_scan(scan), handed, &rules, free, now);
    AutoRun {
        outcome,
        err: String::new(),
    }
}

/// With each reading of the disk (watch_machine's, the rush that leads
/// only): takes the plugins' auto mode in once, then with the switch on
/// runs the engine when it's due, off the UI.
pub fn watch_disk(m: &mut Model, free: u64) {
    let st = m.kept::<DiskAuto>();
    if !st.taken_in {
        if !st.taking_in {
            st.taking_in = true;
            m.later(take_plugins_in, on_taken_in);
        }
        return;
    }
    let now = m.now;
    let on = data(m).cfg_read && data(m).cfg.disk_auto;
    let st = m.kept::<DiskAuto>();
    if !on || st.running || !auto_due(st.at, free, now) {
        return;
    }
    st.running = true;
    st.at = now;
    let scan = scan_of(m);
    m.later(move || auto_run(scan, now), on_run);
}

/// Keeps what was taken in: the switch on if a plugin had its auto mode
/// on, and that it was taken in, in config.json.
fn on_taken_in(m: &mut Model, r: Result<Option<bool>, String>) -> Out {
    let st = m.kept::<DiskAuto>();
    st.taking_in = false;
    st.taken_in = true;
    match r {
        Ok(None) => Out::Idle,
        Ok(Some(on)) => {
            rush_ui_base::config::save(m, "couldn't keep the disk switch", move |c| {
                c.disk.plugins_taken_in = true;
                if on {
                    c.disk.auto = true;
                }
            });
            if on {
                m.kept::<Data>().cfg.disk_auto = true;
                m.news(
                    "disk: rush cleans on its own when disk is low now, as rust-clean or git-clean did",
                    "",
                );
                return Out::Draw;
            }
            Out::Idle
        }
        Err(e) => {
            m.news(&format!("disk: {e}"), "");
            Out::Draw
        }
    }
}

/// Says what a run freed, nothing when nothing; a run that couldn't is
/// said once until it can.
fn on_run(m: &mut Model, r: AutoRun) -> Out {
    let st = m.kept::<DiskAuto>();
    st.running = false;
    if !r.err.is_empty() {
        if st.said_err == r.err {
            return Out::Idle;
        }
        st.said_err = r.err.clone();
        m.news(&format!("disk: {}", r.err), "");
        return Out::Draw;
    }
    st.said_err.clear();
    let said = auto_words(&r.outcome);
    if said.is_empty() {
        return Out::Idle;
    }
    m.news(&said, "");
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_fleet::disk::{Done, built_in_rules};

    const G: u64 = 1 << 30;
    const MIN: Ms = 60_000;
    const NOW: Ms = 1_793_190_896_789;

    // Every 15 minutes, every 3 under 20 GB free; the first at once. Free
    // not known (0) is the slower pace.
    #[test]
    fn due_by_free() {
        for (last, free, want, why) in [
            (0, 80 * G, true, "never ran"),
            (NOW - 14 * MIN, 30 * G, false, "14m ago, 30G free"),
            (NOW - 15 * MIN, 30 * G, true, "15m ago, 30G free"),
            (NOW - 2 * MIN, 10 * G, false, "2m ago, low"),
            (NOW - 3 * MIN, 10 * G, true, "3m ago, low"),
            (NOW - 3 * MIN, 20 * G, false, "3m ago, just not low"),
            (NOW - 5 * MIN, 0, false, "free not known"),
        ] {
            assert_eq!(auto_due(last, free, NOW), want, "{why}");
        }
    }

    #[test]
    fn says_what_it_freed() {
        let done = |freed| Done {
            rule: "cargo-target".into(),
            path: "/w/target".into(),
            what: "removed".into(),
            freed,
        };
        for (did, freed, want) in [
            (vec![], 0, ""),
            (
                vec![done(3 * G)],
                3 * G,
                "disk: rush freed 3.0G on its own (1 thing, by its rules)",
            ),
            (
                vec![done(G), done(G / 2)],
                G + G / 2,
                "disk: rush freed 1.5G on its own (2 things, by its rules)",
            ),
        ] {
            let o = Outcome {
                did,
                left: Vec::new(),
                freed,
            };
            assert_eq!(auto_words(&o), want);
        }
    }

    fn stored(pairs: &[(&str, &str, &str)]) -> BTreeMap<String, BTreeMap<String, String>> {
        let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (p, k, v) in pairs {
            out.entry(p.to_string())
                .or_default()
                .insert(k.to_string(), v.to_string());
        }
        out
    }

    // Either clean plugin's auto-clean on turns the switch on; git-clean's
    // idle days, when a whole number from 1, become the worktree rule's.
    #[test]
    fn takes_the_plugins_in() {
        for (s, auto, days, why) in [
            (vec![], false, None, "nothing set"),
            (
                vec![("rust-clean", AUTO_KEY, "true")],
                true,
                None,
                "rust-clean's on",
            ),
            (
                vec![
                    ("git-clean", AUTO_KEY, "true"),
                    ("git-clean", IDLE_DAYS_KEY, "7"),
                ],
                true,
                Some(7),
                "git-clean's on, 7 days",
            ),
            (
                vec![
                    ("rust-clean", AUTO_KEY, "false"),
                    ("git-clean", IDLE_DAYS_KEY, "14"),
                ],
                false,
                Some(14),
                "off, 14 days",
            ),
            (
                vec![
                    ("go-clean", AUTO_KEY, "true"),
                    ("git-clean", IDLE_DAYS_KEY, "0"),
                ],
                false,
                None,
                "another plugin's, and 0 days",
            ),
            (
                vec![("git-clean", IDLE_DAYS_KEY, "x")],
                false,
                None,
                "not a number",
            ),
        ] {
            assert_eq!(
                plugins_said(&stored(&s)),
                PluginsSaid {
                    auto,
                    idle_days: days
                },
                "{why}"
            );
        }
        let mut f = RulesFile {
            rules: built_in_rules(),
            proposed: Vec::new(),
        };
        with_worktree_idle(&mut f, 7);
        let w = f.rules.iter().find(|r| r.kind == Kind::Worktree).unwrap();
        assert_eq!(w.normal.as_ref().unwrap().idle_secs, Some(7 * DAY_SECS));
        assert_eq!(w.low.as_ref().unwrap().idle_secs, Some(7 * DAY_SECS));
        let others: Vec<_> = f
            .rules
            .iter()
            .filter(|r| r.kind != Kind::Worktree)
            .collect();
        let built = built_in_rules();
        assert!(
            others.iter().all(|r| built.contains(r)),
            "only the worktree rule changes"
        );
    }

    /// rush's folder, for this test's thread only.
    struct RushHome(tempfile::TempDir);

    impl RushHome {
        fn new() -> RushHome {
            let d = tempfile::tempdir().unwrap();
            rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
            RushHome(d)
        }
    }

    impl Drop for RushHome {
        fn drop(&mut self) {
            rush_state::set_thread_dir(None);
        }
    }

    // The plugins' settings as kept in plugins/settings.json are taken in
    // once: the idle days into disk-rules.json, the switch to turn on
    // said; with config.json's pluginsTakenIn, never again.
    #[test]
    fn takes_the_plugins_in_once() {
        let home = RushHome::new();
        let dir = home.0.path().canonicalize().unwrap();
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        std::fs::write(
            dir.join("plugins/settings.json"),
            r#"{"js-clean":null,"rust-clean":{"auto-clean":"true","n":3},"git-clean":{"auto-clean":"false","worktree-idle-days":"7"}}"#,
        )
        .unwrap();
        assert_eq!(take_plugins_in(), Ok(Some(true)));
        let rules = disk::load_rules().unwrap().rules;
        let w = rules.iter().find(|r| r.kind == Kind::Worktree).unwrap();
        assert_eq!(w.normal.as_ref().unwrap().idle_secs, Some(7 * DAY_SECS));
        std::fs::write(
            dir.join("config.json"),
            r#"{"disk":{"pluginsTakenIn":true}}"#,
        )
        .unwrap();
        assert_eq!(take_plugins_in(), Ok(None));
    }
}
