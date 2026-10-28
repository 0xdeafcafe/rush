//! Files Go rush wrote (tests/fixtures, made by gofixtures/gen.sh) read
//! and write back byte for byte, and the same changes made here write the
//! same bytes Go wrote.

use rush_state::agents::{self, Adapter, Program};
use rush_state::config::{Config, Dispatch};
use rush_state::health;
use rush_state::settingsfile;
use rush_state::store::{Overlay, Store};
use rush_state::{CostCache, Time, json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Once;

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The agents Go's state tests register, as the fixtures were made with.
fn register_fakes() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for k in ["claude", "pa", "pb", "pc"] {
            agents::register(Adapter::new(k));
        }
        agents::register(Adapter {
            program: Program::Missing,
            ..Adapter::new("gone")
        });
        agents::register(Adapter {
            provider: Some("pa".into()),
            rides: Some("pb".into()),
            ..Adapter::new("pa-pb")
        });
        agents::register(Adapter {
            key_env: Some("PS_KEY".into()),
            sign_in: true,
            ..Adapter::new("ps")
        });
        agents::register(Adapter {
            provider: Some("ps".into()),
            rides: Some("pb".into()),
            ..Adapter::new("ps-pb")
        });
    });
}

fn round_trip<T: Serialize + serde::de::DeserializeOwned>(name: &str) -> T {
    let b = fixture(name);
    let v: T = json::from_slice(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(text(&json::to_vec_indent(&v).unwrap()), text(&b), "{name}");
    v
}

#[test]
fn configs_round_trip() {
    let full: Config = round_trip("config_full.json");
    // The profile is kept as written: its number too.
    assert!(
        full.logins[0]
            .profile
            .as_ref()
            .unwrap()
            .get()
            .contains(r#""n": 1.50"#)
    );
    assert!(full.copies_on_select() && !full.dispatch.quick_start());
    let s: Config = round_trip("config_strings.json");
    assert!(s.group_by.starts_with("a && b > c <d>"));
    round_trip::<Config>("config_empty.json");
    round_trip::<Overlay>("overlay_empty.json");
    round_trip::<Dispatch>("dispatch_empty.json");
    // What a fresh one writes is what Go's zero value writes.
    assert_eq!(
        text(&json::to_vec_indent(&Config::default()).unwrap()),
        text(&fixture("config_empty.json"))
    );
}

#[test]
fn floats_as_go_writes_them() {
    round_trip::<BTreeMap<String, f64>>("floats.json");
}

#[test]
fn overlay_keeps_times_as_written() {
    let o: Overlay = round_trip("state.json");
    let a = o.done["claude/a"];
    assert_eq!(
        a,
        Time::parse("2026-10-28T13:34:56.123456789+01:00").unwrap()
    );
    assert_eq!(o.seen["z"].format(), "1999-01-02T03:04:05.1Z");
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CostFile {
    offset: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    cost: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    cwd: String,
}

fn is_zero(f: &f64) -> bool {
    *f == 0.0
}

#[test]
fn cost_cache_compact() {
    let b = fixture("costs.json");
    let c: CostCache<CostFile> = json::from_slice(&b).unwrap();
    assert_eq!(c.files["/b.jsonl"].cost, 0.25);
    assert_eq!(text(&json::to_vec(&c).unwrap()), text(&b));

    // Saved and loaded through the cache folder: transcripts gone are dropped.
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_cache_dir(Some(d.path().to_path_buf()));
    let kept = d.path().join("kept.jsonl");
    fs::write(&kept, "").unwrap();
    let mut c: CostCache<CostFile> = CostCache::load();
    c.get_mut(kept.to_str().unwrap()).offset = 7;
    c.get_mut("/nowhere.jsonl").offset = 1;
    c.mark_dirty();
    c.save().unwrap();
    let c: CostCache<CostFile> = CostCache::load();
    assert_eq!(c.files.len(), 1);
    assert_eq!(c.files[kept.to_str().unwrap()].offset, 7);
}

#[test]
fn health_writes_what_go_wrote() {
    let b = fixture("health.json");
    let all: health::All = json::from_slice(&b).unwrap();
    assert_eq!(text(&json::to_vec(&all).unwrap()), text(&b));

    // The same calls Go made, at the same times, write the same file.
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let t0 = Time::parse("2026-10-28T12:00:00.0000005+01:00").unwrap();
    let s = |n: i64| t0.add_nanos(n * 1_000_000_000);
    health::answered("claude/a", t0);
    health::failed("claude/a", "API Error: 401 invalid_grant sk-secret", s(2));
    health::failed("codex/b", "API Error: Connection error.", s(3));
    health::failed("codex/b", "API Error: 503", s(4));
    health::answered("pi/c", s(60));
    assert_eq!(text(&fs::read(health::path()).unwrap()), text(&b));
}

#[test]
fn settings_edits_write_what_go_wrote() {
    let d = tempfile::tempdir().unwrap();
    let work = d.path().join("settings.json");
    fs::write(&work, fixture("settings.in.json")).unwrap();
    let mut s = settingsfile::File::load(&work).unwrap();
    s.set_env("B", "2 <&> é").unwrap();
    s.set_env("DROP", "").unwrap();
    s.set("permissions.defaultMode", "bypassPermissions")
        .unwrap();
    s.set("statusLine.type", "command").unwrap();
    s.set("statusLine.command", "rush status && echo").unwrap();
    s.set(
        "new.deep.list",
        &serde_json::json!([1, 2.5, "x", {"k": null}]),
    )
    .unwrap();
    s.delete("hooks.empty");
    s.save().unwrap();
    assert_eq!(
        text(&fs::read(&work).unwrap()),
        text(&fixture("settings.out.json"))
    );
    assert_eq!(
        s.string("model"),
        "sonnet",
        "a repeated key's last value wins"
    );
    assert_eq!(s.env()["A"], "1");

    let empty = d.path().join("s.json");
    let mut e = settingsfile::File::load(&empty).unwrap();
    e.set("theme", "dark").unwrap();
    e.set("env.X", "1").unwrap();
    e.save().unwrap();
    assert_eq!(
        text(&fs::read(&empty).unwrap()),
        text(&fixture("settings_new.out.json"))
    );
}

// Old configs, loaded and saved, come out as Go's migration wrote them.
#[test]
fn migrations_write_what_go_wrote() {
    register_fakes();
    for name in [
        "migrate_accounts",
        "migrate_profiles",
        "migrate_movein",
        "migrate_plain",
        "migrate_split",
        "migrate_empty",
        "migrate_unknown",
    ] {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        fs::write(
            d.path().join("config.json"),
            fixture(&format!("{name}.in.json")),
        )
        .unwrap();
        let s = Store::load_with_env(&|_| None);
        s.save_config().unwrap();
        let got = fs::read(d.path().join("config.json")).unwrap();
        assert_eq!(
            text(&got),
            text(&fixture(&format!("{name}.out.json"))),
            "{name}"
        );
        // Loaded again, nothing is moved twice.
        let again = Store::load_with_env(&|_| None);
        again.save_config().unwrap();
        assert_eq!(
            text(&fs::read(d.path().join("config.json")).unwrap()),
            text(&got),
            "{name} again"
        );
    }
}

// A config from before accounts were grouped by agent loads with what it
// meant, and saves in a shape an older rush still reads; the copy kept
// before is never overwritten.
#[test]
fn migrate_accounts_keeps_the_old_one_aside() {
    register_fakes();
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let path = d.path().join("config.json");
    let old = fixture("migrate_accounts.in.json");
    fs::write(&path, &old).unwrap();
    let mut s = Store::load_with_env(&|_| None);
    let c = &s.config;
    assert!(
        c.switch_on_limit == "off" && c.group_by == "agent" && c.folders.len() == 2,
        "{c:?}"
    );
    rush_state::keep_before("before-accounts");
    s.config.set_switch_on_limit("agent");
    s.save_config().unwrap();
    rush_state::keep_before("before-accounts"); // never overwritten
    assert_eq!(
        fs::read(d.path().join("config.json.before-accounts")).unwrap(),
        old
    );
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(
        raw.get("accounts").is_some() && raw.get("stayOnAccount").is_none(),
        "{raw}"
    );
}

// An older config's default agent, order and choice when nearly out
// became the Default profile, and that is moved into what replaced
// profiles; the old fields are still written.
#[test]
fn migrate_profiles_then_change() {
    register_fakes();
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let path = d.path().join("config.json");
    fs::write(&path, fixture("migrate_profiles.in.json")).unwrap();
    let mut s = Store::load_with_env(&|_| None);
    let def = s.config.default_profile();
    assert!(s.config.default_profile == "pa" && s.config.at_limit == "next" && def.builtin);
    assert!(
        def.mix == "mix" && def.limit() == "handoff" && def.providers[0] == "pa",
        "{def:?}"
    );
    s.config.at_limit = "wait".into();
    s.config.set_default_provider("pb");
    s.save_config().unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["dispatch"]["kind"], "pb");
    assert_eq!(raw["switchOnLimit"], "off");
    assert_eq!(raw["stayOnAccount"], true);
    assert_eq!(raw["profiles"].as_array().unwrap().len(), 1);
    let s = Store::load_with_env(&|_| None);
    assert!(s.config.default_profile == "pb" && s.config.default_profile().limit() == "wait");
}
