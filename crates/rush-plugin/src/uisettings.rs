//! A plugin's settings, kept by rush in root(), where the plugin can't
//! read or write: it is told them at initialize and when one changes.
//! Ported from internal/plugin/uisettings.go.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use crate::bundled::bundle_named;
use crate::gopath;
use crate::manifest::Manifest;
use crate::plugin::{Approval, approvals};
use crate::text::printable;
use crate::ui::SettingSpec;

/// The longest a text setting's value may be, in bytes.
pub const MAX_SETTING_TEXT: usize = 1000;

fn settings_path() -> PathBuf {
    crate::root().join("settings.json")
}

fn settings_lock() -> PathBuf {
    crate::root().join("settings.lock")
}

/// Every plugin's stored values, by plugin then key.
fn read_settings() -> BTreeMap<String, BTreeMap<String, String>> {
    std::fs::read(settings_path())
        .ok()
        .and_then(|b| rush_state::json::from_slice::<Option<BTreeMap<String, Option<BTreeMap<String, String>>>>>(&b).ok())
        .flatten()
        .map(|m| m.into_iter().map(|(k, v)| (k, v.unwrap_or_default())).collect())
        .unwrap_or_default()
}

/// The approved plugin's settings: what you set, or each one's default. A
/// plugin that isn't approved has none.
pub fn setting_values(name: &str) -> BTreeMap<String, String> {
    match approvals().remove(name) {
        Some(a) => setting_values_of(&a.manifest),
        None => BTreeMap::new(),
    }
}

/// setting_values for a manifest already in hand. A stored value its
/// settings no longer allow is taken as unset.
pub fn setting_values_of(m: &Manifest) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if m.settings.is_empty() {
        return out;
    }
    let stored = read_settings().remove(&m.name).unwrap_or_default();
    for s in &m.settings {
        let v = match stored.get(&s.key) {
            Some(v) if check_setting(s, v).is_ok() => v.clone(),
            _ => default_setting(s),
        };
        out.insert(s.key.clone(), v);
    }
    out
}

fn default_setting(s: &SettingSpec) -> String {
    if !s.default.is_empty() {
        return s.default.clone();
    }
    match s.kind.as_str() {
        "bool" => "false".into(),
        "choice" if !s.choices.is_empty() => s.choices[0].clone(),
        _ => String::new(),
    }
}

/// Whether v is a value setting s allows.
fn check_setting(s: &SettingSpec, v: &str) -> Result<(), String> {
    match s.kind.as_str() {
        "bool" => {
            if v != "true" && v != "false" {
                return Err(format!("{} is true or false", s.key));
            }
        }
        "choice" => {
            if !s.choices.iter().any(|c| c == v) {
                // %v of a []string: [a b]
                return Err(format!("{} is one of [{}]", s.key, s.choices.join(" ")));
            }
        }
        "text" => {
            if v.len() > MAX_SETTING_TEXT {
                return Err(format!("{} is at most {MAX_SETTING_TEXT} bytes", s.key));
            }
            if printable(v) != v {
                return Err(format!("{} can't hold control characters", s.key));
            }
        }
        _ => return Err(format!("{} has no type rush knows", s.key)),
    }
    Ok(())
}

/// Keeps a value for one of an approved plugin's settings, if the
/// manifest you approved offers it and allows the value. value is bytes so
/// a value that isn't UTF-8 is refused as Go refuses it.
pub fn set_setting(name: &str, key: &str, value: &str) -> Result<(), String> {
    set_setting_bytes(name, key, value.as_bytes())
}

/// set_setting for a value as it came, which may not be UTF-8.
pub fn set_setting_bytes(name: &str, key: &str, value: &[u8]) -> Result<(), String> {
    let mut a = approvals().remove(name);
    if let Some(b) = bundle_named(name) {
        // A bundled plugin's name is its own, as in enabled.
        a = Some(Approval {
            manifest: b.manifest,
            ..Default::default()
        });
    }
    let Some(a) = a else {
        return Err(format!("{name} is not approved"));
    };
    let Some(spec) = a.manifest.settings.iter().find(|s| s.key == key) else {
        return Err(format!("{name} has no setting {}", crate::text::go_q(key)));
    };
    let value = match std::str::from_utf8(value) {
        Ok(v) => v,
        Err(_) if spec.kind == "text" => {
            return Err(format!("{} can't hold control characters", spec.key));
        }
        // Not UTF-8 is no bool and no choice either.
        Err(_) => "\u{fffd}",
    };
    check_setting(spec, value)?;
    let root = crate::root();
    crate::mkdir_all(&root)?;
    // Two writers (two rushes, say) take turns, so neither loses the
    // other's. The lock goes with the file when it's closed.
    let lp = settings_lock();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lp)
        .map_err(|e| gopath::os_err("open", &lp, &e))?;
    crate::sys::flock_ex(&lock).map_err(|e| gopath::errno_text(&e))?;

    let mut all = read_settings();
    all.entry(name.to_string())
        .or_default()
        .insert(key.to_string(), value.to_string());
    let b = rush_state::json::to_vec_indent(&all).map_err(|e| e.to_string())?;
    let tmp = root.join(format!(".settings-{}.tmp", unique()));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&b)?;
        drop(f);
        std::fs::rename(&tmp, settings_path())
    })();
    drop(lock);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("saving settings\n{}", gopath::errno_text(&e)));
    }
    Ok(())
}

/// A name no other temporary has: Go's CreateTemp pattern's random part.
pub(crate) fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{}{}{}",
        std::process::id(),
        t,
        N.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::tests::{Home, install_plugin, manifest};
    use crate::plugin::{approve, load};
    use std::os::unix::fs::PermissionsExt;

    fn spec(key: &str, kind: &str) -> SettingSpec {
        SettingSpec {
            key: key.into(),
            title: key.to_uppercase(),
            kind: kind.into(),
            ..Default::default()
        }
    }

    #[test]
    fn settings() {
        let home = Home::new();
        let mut m = manifest("drafts", "bin");
        m.settings = vec![
            spec("on", "bool"),
            SettingSpec {
                choices: vec!["a".into(), "b".into()],
                default: "b".into(),
                ..spec("mode", "choice")
            },
            SettingSpec {
                default: "hi".into(),
                ..spec("greeting", "text")
            },
        ];
        let p = load(&install_plugin(&m, &[])).unwrap();
        assert!(
            set_setting("drafts", "on", "true").is_err(),
            "set a setting for a plugin that isn't approved"
        );
        approve(&p).unwrap();
        let want = |kv: &[(&str, &str)]| -> BTreeMap<String, String> {
            kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            setting_values("drafts"),
            want(&[("on", "false"), ("mode", "b"), ("greeting", "hi")])
        );
        let long = "x".repeat(MAX_SETTING_TEXT + 1);
        for (k, v) in [
            ("on", b"yes".as_slice()),
            ("mode", b"c"),
            ("greeting", long.as_bytes()),
            ("greeting", b"a\x1b[31mred"),
            ("greeting", b"\xff"),
            ("missing", b"x"),
        ] {
            assert!(
                set_setting_bytes("drafts", k, v).is_err(),
                "{k} = {v:?} was taken"
            );
        }
        set_setting("drafts", "on", "true").unwrap();
        set_setting("drafts", "greeting", "héllo there").unwrap();
        assert_eq!(
            setting_values("drafts"),
            want(&[("on", "true"), ("mode", "b"), ("greeting", "héllo there")])
        );
        let md = std::fs::metadata(crate::root().join("settings.json")).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o600);

        // Writers taking turns lose nothing.
        let mut m2 = manifest("other", "bin");
        m2.settings = vec![spec("n", "text")];
        approve(&load(&install_plugin(&m2, &[])).unwrap()).unwrap();
        let dir = home.path();
        let ts: Vec<_> = (0..20)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    rush_state::set_thread_dir(Some(dir));
                    if i % 2 == 0 {
                        set_setting("other", "n", "v").unwrap();
                    } else {
                        set_setting("drafts", "mode", "a").unwrap();
                    }
                })
            })
            .collect();
        for t in ts {
            t.join().unwrap();
        }
        assert_eq!(setting_values("other")["n"], "v");
        assert_eq!(setting_values("drafts")["mode"], "a");
        assert_eq!(setting_values("drafts")["on"], "true", "lost a write");
        // A stored value the manifest no longer allows reads as the default.
        std::fs::write(
            crate::root().join("settings.json"),
            r#"{"drafts":{"on":"maybe","mode":"z"}}"#,
        )
        .unwrap();
        let got = setting_values("drafts");
        assert_eq!((got["on"].as_str(), got["mode"].as_str()), ("false", "b"));
        assert!(
            setting_values("nobody").is_empty(),
            "an unapproved plugin has values"
        );
    }
}
