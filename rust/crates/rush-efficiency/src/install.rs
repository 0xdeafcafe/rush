//! Setting a saver up or taking it away: shown first, the files it may
//! change backed up, then logged as rush's doing. Port of
//! internal/efficiency/install.go.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use rush_state::Time;

use crate::catalog::{Saver, Setting, SettingValue};
use crate::detect::{Env, look_path, pick, search_path};
use crate::events::{Event, add_event, remember};
use crate::when::local;

/// What installing or removing a saver will do, shown before it's done:
/// the commands in order and the settings it changes.
#[derive(Clone, Debug)]
pub struct Plan {
    pub saver: &'static Saver,
    pub remove: bool,
    pub cmds: Vec<Vec<String>>,
    pub changes: Vec<String>,
    pub undo: String,
    /// Why it can't be done here: a package manager missing, or a saver
    /// that's only ever set up by hand.
    pub blocked: String,
}

fn tilde(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match p.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => p.to_string(),
    }
}

impl Plan {
    /// What installing (or removing) s would do in env.
    pub fn new(env: &Env, s: &'static Saver, remove: bool) -> Plan {
        let mut p = Plan {
            saver: s,
            remove,
            cmds: Vec::new(),
            changes: Vec::new(),
            undo: String::new(),
            blocked: String::new(),
        };
        if let Some(st) = &s.setting {
            let where_ = tilde(&env.settings_path());
            let name = if st.env {
                format!("env.{}", st.key)
            } else {
                st.key.to_string()
            };
            if remove {
                p.changes = vec![format!("{where_}   − {name}   (back to {})", st.default)];
                p.undo = format!("set {name} again here");
            } else {
                p.changes = vec![format!("{where_}   + {name}: {}", st.value)];
                p.undo = format!("x here removes it; Claude Code then uses {}", st.default);
            }
            return p;
        }
        if !s.manual.is_empty() && s.install.is_empty() {
            p.blocked = format!("set up by hand: {}", s.manual.join("  ·  "));
            return p;
        }
        let rs = if remove { &s.remove } else { &s.install };
        let r = match pick(rs) {
            Ok(r) => r,
            Err(why) => {
                p.blocked = why;
                return p;
            }
        };
        p.cmds = r.plan();
        if !remove && let Ok(u) = pick(&s.remove) {
            p.undo = u
                .steps
                .iter()
                .map(|st| st.argv.join(" "))
                .collect::<Vec<_>>()
                .join(" && ");
        }
        p
    }

    /// Carries the plan out, logging it as rush's doing: the commands'
    /// output, last lines last, or that and why it failed.
    pub fn run(&self, env: &mut Env) -> Result<Vec<String>, (Vec<String>, String)> {
        if !self.blocked.is_empty() {
            return Err((Vec::new(), self.blocked.clone()));
        }
        let mut log = Vec::new();
        let dir = backup(env).map_err(|e| (Vec::new(), format!("backing up first: {e}")))?;
        log.push(format!("backed up to {}", tilde(&dir.to_string_lossy())));
        let mut err = None;
        if let Some(st) = &self.saver.setting {
            if let Err(e) = apply_setting(env, st, self.remove) {
                err = Some(e.to_string());
            }
        } else {
            for argv in &self.cmds {
                log.push(format!("$ {}", argv.join(" ")));
                let (out, res) = run(env, argv);
                log.extend(out);
                if let Err(e) = res {
                    err = Some(format!("{}: {e}", argv.first().map_or("", String::as_str)));
                    break;
                }
            }
        }
        if let Some(e) = err {
            return Err((log, e));
        }
        let fresh = Env::load(&env.profile);
        let f = fresh.detect(self.saver);
        let (kind, detail) = match &self.saver.setting {
            Some(st) if self.remove => ("setting", format!("{} unset", st.key)),
            Some(st) => ("setting", format!("{} = {}", st.key, f.value)),
            None if self.remove => ("remove", String::new()),
            None => ("install", f.parts.join(", ")),
        };
        let _ = add_event(Event {
            kind: kind.into(),
            saver: self.saver.id.into(),
            account: env.profile.dir.clone(),
            source: "rush".into(),
            detail,
            ..Event::default()
        });
        remember(&fresh, self.saver.id, &f);
        Ok(log)
    }
}

impl Env {
    /// Where the account's settings are.
    pub fn settings_path(&self) -> String {
        self.backup.first().cloned().unwrap_or_else(|| {
            PathBuf::from(&self.profile.dir)
                .join("settings.json")
                .to_string_lossy()
                .into_owned()
        })
    }
}

/// Copies the files an install may change into a folder of its own, and
/// returns it.
pub fn backup(env: &Env) -> std::io::Result<PathBuf> {
    let dir = crate::dir()
        .join("backups")
        .join(local(Time::now(), "2006-01-02T150405"));
    std::fs::create_dir_all(&dir)?;
    for p in &env.backup {
        let p = PathBuf::from(p);
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name() else { continue };
        std::fs::copy(&p, dir.join(name))?;
    }
    Ok(dir)
}

fn apply_setting(env: &mut Env, st: &Setting, remove: bool) -> std::io::Result<()> {
    if st.env {
        let v = if remove {
            String::new()
        } else {
            st.value.to_string()
        };
        env.settings.set_env(st.key, &v)?;
    } else if remove {
        env.settings.delete(st.key);
    } else {
        match st.value {
            SettingValue::Int(n) => env.settings.set(st.key, &n)?,
            SettingValue::Str(s) => env.settings.set(st.key, s)?,
            SettingValue::Bool(b) => env.settings.set(st.key, &b)?,
        }
    }
    env.settings.save()
}

/// Runs one command as the account, with package managers' folders on the
/// PATH, and keeps the last lines it printed.
fn run(env: &Env, argv: &[String]) -> (Vec<String>, Result<(), String>) {
    let Some(mut bin) = argv.first().cloned() else {
        return (Vec::new(), Err("no command".into()));
    };
    let p = look_path(&bin);
    if !p.is_empty() {
        bin = p;
    }
    let path = std::env::join_paths(search_path())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut c = Command::new(&bin);
    c.args(&argv[1..])
        .env_clear()
        .envs(env.env.iter().filter_map(|e| e.split_once('=')))
        .env("PATH", path)
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("CI", "1")
        .current_dir(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
        .stdin(Stdio::null());
    let out = match run_for(c, Duration::from_secs(600)) {
        Ok(o) => o,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let text = String::from_utf8_lossy(&out.0);
    let lines: Vec<&str> = text.trim().split('\n').collect();
    let lines = &lines[lines.len().saturating_sub(12)..];
    let keep = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| format!("  {l}"))
        .collect();
    (keep, out.1)
}

/// Runs c with its output and errors in one, for at most d.
fn run_for(mut c: Command, d: Duration) -> Result<(Vec<u8>, Result<(), String>), String> {
    use std::io::Read as _;
    let (mut r, w) = std::io::pipe().map_err(|e| e.to_string())?;
    let w2 = w.try_clone().map_err(|e| e.to_string())?;
    c.stdout(w).stderr(w2);
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    drop(c);
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = r.read_to_end(&mut b);
        b
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) if start.elapsed() > d => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("timed out".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(e.to_string()),
        }
    };
    let out = reader.join().unwrap_or_default();
    let res = match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(s.to_string()),
        Err(e) => Err(e),
    };
    Ok((out, res))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::find;
    use crate::detect::tests::test_account;
    use crate::events::{load_events, observe};

    // Settings savers are changed in place, keeping the rest of the file,
    // and logged as rush's doing.
    #[test]
    fn setting_plan() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let tmp = tempfile::tempdir().unwrap();
        let a = test_account(tmp.path(), r#"{"model":"opus","env":{"KEEP":"1"}}"#);
        let sv = find("autocompact").unwrap();
        let p = Plan::new(&Env::load(&a), sv, false);
        assert!(
            p.blocked.is_empty()
                && p.changes.len() == 1
                && p.changes[0].contains("autoCompactWindow: 400000"),
            "{p:?}"
        );
        p.run(&mut Env::load(&a)).unwrap();
        let b = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
        assert!(
            b.contains(r#""autoCompactWindow": 400000"#)
                && b.contains(r#""KEEP": "1""#)
                && b.contains(r#""model": "opus""#),
            "{b}"
        );
        let evs = load_events();
        assert!(
            evs.len() == 1 && evs[0].kind == "setting" && evs[0].source == "rush",
            "{evs:?}"
        );
        let backups: Vec<_> = std::fs::read_dir(crate::dir().join("backups"))
            .unwrap()
            .flatten()
            .filter(|d| d.path().join("settings.json").exists())
            .collect();
        assert_eq!(backups.len(), 1, "settings.json wasn't backed up first");
        // Observing afterwards doesn't log it again.
        observe(&Env::load(&a));
        assert_eq!(
            load_events().len(),
            1,
            "observing logged rush's own change again"
        );

        Plan::new(&Env::load(&a), sv, true)
            .run(&mut Env::load(&a))
            .unwrap();
        let b = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
        assert!(!b.contains("autoCompactWindow"), "removing left it: {b}");
        rush_state::set_thread_dir(None);
    }

    // A change made outside rush is noticed and logged; the first look at
    // an account logs nothing without an install date.
    #[test]
    fn observes() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let tmp = tempfile::tempdir().unwrap();
        let a = test_account(tmp.path(), "{}");
        observe(&Env::load(&a));
        assert!(load_events().is_empty(), "the first look logged events");
        std::fs::write(
            tmp.path().join("settings.json"),
            r#"{"cleanupPeriodDays":90}"#,
        )
        .unwrap();
        observe(&Env::load(&a));
        let evs = load_events();
        assert!(
            evs.len() == 1 && evs[0].saver == "history" && evs[0].source == "noticed",
            "{evs:?}"
        );
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn manual_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let a = test_account(tmp.path(), "");
        let p = Plan::new(&Env::load(&a), find("headroom").unwrap(), false);
        assert!(
            !p.blocked.is_empty(),
            "headroom is set up by hand; rush shouldn't run anything"
        );
    }
}
