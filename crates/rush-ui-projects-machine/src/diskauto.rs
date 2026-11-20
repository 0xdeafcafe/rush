//! The disk engine on its own (Rust-only; Go had no engine): with the
//! switch on (config.json's disk.auto, "Clean on its own when disk is
//! low"), the rush that leads runs rush-fleet's disk engine at
//! the level's interval, sooner when a reading shows space falling,
//! off the UI and never two at once (engine_lock, which the dire clean
//! holds too). It looks where rush's sessions work (their repos and
//! worktrees, the scratch roots, the processes' folders), takes what the
//! plugins that answer disk.candidates report within CANDIDATES_WAIT, and
//! reports space freed and any remaining shortage in the news. Every run is the
//! engine's disk-clean event.
//!
//! Once, before the first run: the cache rules learned before rules
//! waited for your yes (cleanup-rules.tsv) are made proposals
//! (rush_fleet::take_learned_in), and the clean plugins' own auto mode is
//! taken in. If rust-clean's or git-clean's auto-clean setting was on, the
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
pub const AUTO_EVERY: Ms = 10 * 60_000;
/// How often while free is under disk::LOW_UNDER.
pub const AUTO_EVERY_LOW: Ms = 2 * 60_000;
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

/// Uses watch_machine's existing statfs readings. Crossings bypass the regular
/// interval. A loss of 1 GiB since the last run brings the next scan forward
/// to one minute. A healthy or unknown reading never starts a scan.
pub fn auto_due(last: Ms, previous_free: u64, free: u64, now: Ms) -> bool {
    let Some(level) = disk::level(free) else {
        return false;
    };
    let previous = disk::level(previous_free);
    let crossed = previous.is_none() || (level == disk::Level::Low && previous != Some(level));
    let interval = if crossed || previous_free.saturating_sub(free) >= 1 << 30 {
        60_000
    } else if level == disk::Level::Low {
        AUTO_EVERY_LOW
    } else {
        AUTO_EVERY
    };
    last == 0 || now.saturating_sub(last) >= interval
}

/// What a run did, in a few words; "" when it did nothing, which isn't
/// said.
pub fn auto_words(o: &Outcome) -> String {
    let n = o.did.len();
    if n == 0 || o.freed == 0 {
        return String::new();
    }
    format!(
        "disk: rush freed {} on its own ({n} thing{}, by its rules)",
        size(o.freed as i64),
        if n == 1 { "" } else { "s" }
    )
}

/// The cache rules learned before rules waited for your yes, made
/// proposals (rush_fleet::take_learned_in), in a few words for the news.
pub fn learned_words(n: usize) -> String {
    format!(
        "disk: {n} cache rule{} learned before {} proposed now · {} once you accept in Settings › Disk",
        if n == 1 { "" } else { "s" },
        if n == 1 { "is" } else { "are" },
        if n == 1 { "it runs" } else { "they run" }
    )
}

/// The switch's state between frames.
#[derive(Default)]
pub struct DiskAuto {
    /// When the last run started; 0 before one.
    pub at: Ms,
    pub running: bool,
    /// Baseline for cumulative loss and crossing into cleanup from healthy space.
    pub free_at_run: u64,
    pub warned_low: bool,
    /// The plugins' settings are being taken in, or were.
    pub taking_in: bool,
    pub taken_in: bool,
    /// The last error said, so it's said once, not every run.
    pub said_err: String,
    /// When the last quiet pass started; 0 before one.
    pub quiet_at: Ms,
    pub quieting: bool,
    /// The last quiet pass's error said, so it's said once.
    pub quiet_err: String,
    /// When the scratch volume was last looked at to unlock; 0 before.
    pub unlock_at: Ms,
    pub unlocking: bool,
    /// The last unlock error said, so it's said once.
    pub unlock_err: String,
}

/// How often the rush that leads looks whether the scratch volume is
/// locked (rush-fleet's disk::volume::unlock): cheap, statfs only, when
/// it's mounted.
pub const UNLOCK_EVERY: Ms = 10 * 60_000;

/// Unlocks an encrypted scratch volume, when it starts and every
/// UNLOCK_EVERY after, off the UI: macOS doesn't after a restart. Until
/// it's unlocked new sessions' scratch goes in /tmp. macOS only.
fn watch_volume(m: &mut Model) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let now = m.now;
    let st = m.kept::<DiskAuto>();
    if st.unlocking || (st.unlock_at != 0 && now - st.unlock_at < UNLOCK_EVERY) {
        return;
    }
    st.unlocking = true;
    st.unlock_at = now;
    m.later(disk::volume::unlock, |m, r| {
        let st = m.kept::<DiskAuto>();
        st.unlocking = false;
        match r {
            Ok(disk::volume::Unlocked::Nothing) => Out::Idle,
            Ok(disk::volume::Unlocked::Unlocked) => {
                st.unlock_err.clear();
                m.noted("disk: unlocked the scratch volume; new sessions' scratch goes there");
                Out::Draw
            }
            Err(e) => {
                if e == st.unlock_err {
                    return Out::Idle;
                }
                st.unlock_err.clone_from(&e);
                m.fail(&format!(
                    "couldn't unlock the scratch volume, so scratch goes in /tmp: {e}"
                ));
                Out::Draw
            }
        }
    });
}

/// How often folders are kept quiet (disk::quiet): new worktrees and
/// targets are found within the hour.
pub const QUIET_EVERY: Ms = 60 * 60_000;

/// Keeps rush's scratch, and opted in its projects, out of Spotlight and
/// Time Machine, once an hour, off the UI: whatever the engine's switch
/// says.
fn watch_quiet(m: &mut Model) {
    if !data(m).cfg_read {
        return;
    }
    let q = data(m).cfg.quiet;
    let now = m.now;
    let st = m.kept::<DiskAuto>();
    if !q.any() || st.quieting || (st.quiet_at != 0 && now - st.quiet_at < QUIET_EVERY) {
        return;
    }
    st.quieting = true;
    st.quiet_at = now;
    let repos = scan_of(m).repos;
    m.later(
        move || {
            let repos = disk::quiet::repos_of(&repos);
            let scratch = rush_fleet::temp_roots();
            disk::quiet::quiet(&disk::quiet::folders(q, &repos, &scratch))
        },
        |m, did| {
            let st = m.kept::<DiskAuto>();
            st.quieting = false;
            let err = did.errs.first().cloned().unwrap_or_default();
            if err.is_empty() || err == st.quiet_err {
                st.quiet_err = err;
                return Out::Idle;
            }
            st.quiet_err.clone_from(&err);
            m.fail(&format!(
                "couldn't keep a folder out of Spotlight or Time Machine: {err}"
            ));
            Out::Draw
        },
    );
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
    pub free_after: u64,
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
                free_after: free,
                err,
            };
        }
    };
    let outcome = if disk::level(free).is_some() {
        disk::run(&complete_scan(scan), plugin_candidates(), &rules, free, now)
    } else {
        disk::run(&scan, Vec::new(), &rules, free, now)
    };
    AutoRun {
        outcome,
        free_after: rush_sys::sysinfo::read_disk().free,
        err: String::new(),
    }
}

/// With each reading of the disk (watch_machine's, the rush that leads
/// only): takes the plugins' auto mode in once, then with the switch on
/// runs the engine when it's due, off the UI.
pub fn watch_disk(m: &mut Model, free: u64) {
    watch_volume(m);
    watch_quiet(m);
    let st = m.kept::<DiskAuto>();
    if !st.taken_in {
        if !st.taking_in {
            st.taking_in = true;
            m.later(
                || (take_plugins_in(), rush_fleet::take_learned_in()),
                |m, (r, learned)| {
                    match learned {
                        Ok(0) => {}
                        Ok(n) => m.news(&learned_words(n), ""),
                        Err(e) => m.fail(&format!(
                            "couldn't propose the cache rules learned before: {e}"
                        )),
                    }
                    on_taken_in(m, r)
                },
            );
        }
        return;
    }
    let now = m.now;
    let on = data(m).cfg_read && data(m).cfg.disk_auto;
    let st = m.kept::<DiskAuto>();
    if !on {
        st.at = 0;
        st.free_at_run = 0;
        st.warned_low = false;
        return;
    }
    if st.running {
        return;
    }
    // Use recoveries as the baseline for the next fall, even between runs.
    st.free_at_run = st.free_at_run.max(free);
    if free >= disk::LOW_UNDER {
        st.warned_low = false;
    }

    if !auto_due(st.at, st.free_at_run, free, now) {
        return;
    }
    st.running = true;
    st.at = now;
    st.free_at_run = free;
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
    if r.free_after != 0 {
        st.free_at_run = r.free_after;
    }
    let said = finish_words(&r.outcome, r.free_after, &mut st.warned_low);
    if said.is_empty() {
        return Out::Idle;
    }
    m.news(&said, "");
    Out::Draw
}

/// News after a run, using measured free space. Say a shortage once per episode.
pub fn finish_words(outcome: &Outcome, free_after: u64, warned_low: &mut bool) -> String {
    let mut said = auto_words(outcome);
    if disk::level(free_after) == Some(disk::Level::Low) && !*warned_low {
        *warned_low = true;
        if !said.is_empty() {
            said.push_str(". ");
        }
        said.push_str(&format!(
            "Only {} free after cleanup. Review what rush kept in Settings › Disk.",
            size(free_after as i64)
        ));
    }
    if free_after >= disk::LOW_UNDER {
        *warned_low = false;
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_fleet::disk::{Done, built_in_rules};

    const G: u64 = 1 << 30;
    const MIN: Ms = 60_000;
    const NOW: Ms = 1_793_190_896_789;

    #[test]
    fn due_by_free() {
        assert!(auto_due(0, 0, 44 * G, NOW));
        for (elapsed, before, free, want) in [
            (0, 0, 0, false),
            (0, 0, 80, false),
            (1, 0, 42, true),
            (1, 50, 49, true),
            (0, 40, 39, false),
            (1, 40, 39, true),
            (1, 45, 44, true),
            (0, 45, 44, false),
            (9, 45, 45, false),
            (10, 45, 45, true),
            (1, 35, 35, false),
            (2, 35, 35, true),
            (15, 30, 50, false),
            (15, 30, 0, false),
        ] {
            assert_eq!(
                auto_due(NOW - elapsed * MIN, before * G, free * G, NOW),
                want,
                "{elapsed} min, {before}G -> {free}G"
            );
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
            (vec![done(0)], 0, ""),
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

    #[test]
    fn warns_once_until_the_disk_recovers() {
        let mut warned = false;
        for (free, warn) in [
            (0, false),
            (35, true),
            (12, false),
            (0, false),
            (40, false),
            (39, true),
        ] {
            let news = finish_words(&Outcome::default(), free * G, &mut warned);
            assert_eq!(news.contains("Settings › Disk"), warn);
            if warn {
                assert!(news.contains(&size((free * G) as i64)));
            }
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
            ..RulesFile::default()
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
