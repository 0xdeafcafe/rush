//! Moving a host from before homes onto the login in use: such a host
//! can't be told it runs as another login, so it's stopped and a new one
//! started on the same conversation, which runs as the login in use; what
//! it had queued, or a continue if a limit stopped it, goes to the new
//! one. A busy one is replaced once its turn is done. After Go's
//! internal/ui/logins.go (hostBusy :600, replaceHost :606, replaceWhenIdle
//! :643).
//!
//! How the new host starts is the caller's: rush's own `rush host run`
//! ([`crate::spawn`]), or a test's host in its own process
//! ([`crate::fake::spawn`]).

use std::time::Duration;

use serde_json::json;

use crate::client::{Client, read_config, read_info};
use crate::events::log_event;
use crate::server::LIMIT_CONTINUE;
use crate::wire::{Config, Info};

/// Starts a host for a session's config, returning once it listens.
pub type Spawn<'a> = &'a (dyn Fn(Config) -> Result<Config, String> + Sync);

/// How often a busy host is looked at to see if its turn is done
/// (replaceWhenIdle's 2 s).
pub const IDLE_EVERY: Duration = Duration::from_secs(2);

/// Whether a host's agent is in a turn (hostBusy).
pub fn busy(i: &Info) -> bool {
    i.claude_pid != 0 && matches!(i.state.as_str(), "working" | "blocked" | "starting")
}

/// Stops a host from before homes and starts a new one on the same
/// conversation, which runs as the login in use. What it had queued, or
/// a continue if a limit stopped it and carry_on, is sent to the new one
/// (replaceHost).
pub fn replace_host(info: &Info, carry_on: bool, spawn: Spawn) -> Result<(), String> {
    let r = replace(info, carry_on, spawn);
    log_event(
        &info.id,
        "host-replace",
        json!({
            "why": "a host from before homes moves to the account in use",
            "carryOn": carry_on,
            "queued": info.queue.len(),
            "err": r.clone().err().unwrap_or_default(),
        }),
    );
    r
}

fn replace(info: &Info, carry_on: bool, spawn: Spawn) -> Result<(), String> {
    let mut cfg = read_config(&info.id).map_err(|e| e.to_string())?;
    let c = Client::dial(&info.id).map_err(|e| e.to_string())?;
    let _ = c.stop();
    c.close();
    for _ in 0..50 {
        if gone(info) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cfg.resume = true;
    cfg.prompt.clear();
    cfg.images.clear();
    spawn(cfg)?;
    let mut text = info.queue.join("\n\n");
    if text.is_empty() && info.limit.is_some() && carry_on {
        text = LIMIT_CONTINUE.to_string();
    }
    if text.is_empty() {
        return Ok(());
    }
    let c = Client::dial(&info.id).map_err(|e| e.to_string())?;
    let r = c.send(&text).map_err(|e| e.to_string());
    c.close();
    r
}

/// Whether a stopped host has gone: its process ended. One run in this
/// very process (a test's) has gone once its socket no longer answers.
fn gone(info: &Info) -> bool {
    if info.host_pid == std::process::id() as i64 {
        return Client::dial(&info.id).map(|c| c.close()).is_err();
    }
    !crate::alive(info.host_pid)
}

/// Replaces a busy host from before homes once its turn is done, unless
/// it stops first, looking every `every` (replaceWhenIdle, with
/// [`IDLE_EVERY`]). Blocks: run it on a thread of its own.
pub fn replace_when_idle(id: &str, every: Duration, spawn: Spawn) {
    loop {
        std::thread::sleep(every);
        let Ok(info) = read_info(id) else { return };
        if info.state == "stopped" || !crate::alive(info.host_pid) {
            return;
        }
        if !busy(&info) {
            let _ = replace_host(&info, true, spawn);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake;
    use crate::testing::Home;
    use crate::wire::Limit;

    fn spawn_fake(cfg: Config) -> Result<Config, String> {
        fake::spawn(fake::registry(), cfg).map_err(|e| e.to_string())
    }

    /// A fake host for session id, its account's folder (where the fake
    /// logs its starts) under home.
    fn start(home: &Home, id: &str) -> Config {
        let acct = home.path().join("acct");
        std::fs::create_dir_all(&acct).unwrap();
        let mut cfg = Config {
            id: id.into(),
            session_id: format!("{id}-conversation"),
            kind: "fake".into(),
            cwd: home.path().to_string_lossy().into_owned(),
            prompt: "first".into(),
            ..Config::default()
        };
        cfg.account.kind = "fake".into();
        cfg.account.dir = acct.to_string_lossy().into_owned();
        spawn_fake(cfg).unwrap()
    }

    /// Waits, up to 10 s, for what an in-process host does on its own
    /// threads to show in its folder or its account's: true once ok says.
    fn until(home: &Home, id: &str, ok: impl Fn() -> bool) -> bool {
        let w = rush_sys::fswait::Watcher::new();
        w.watch(&[home.path().join("acct"), crate::paths::dir(id)]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            crate::events::flush();
            if ok() {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            w.wait(None, Some((deadline - now).min(Duration::from_millis(50))));
        }
    }

    fn starts(home: &Home) -> Vec<String> {
        std::fs::read_to_string(home.path().join("acct").join("starts.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// What the session's host did, from its events log.
    fn events(id: &str) -> String {
        crate::events::flush();
        std::fs::read_to_string(crate::paths::events_path(id)).unwrap_or_default()
    }

    fn stop(id: &str) {
        if let Ok(c) = Client::dial(id) {
            let _ = c.stop();
            c.close();
        }
    }

    #[test]
    fn busy_is_a_turn_under_way() {
        for (pid, state, want) in [
            (7, "working", true),
            (7, "blocked", true),
            (7, "starting", true),
            (7, "idle", false),
            (0, "working", false),
        ] {
            let i = Info {
                claude_pid: pid,
                state: state.into(),
                ..Info::default()
            };
            assert_eq!(busy(&i), want, "{pid} {state}");
        }
    }

    // A host a limit stopped is stopped, started again resuming its
    // conversation without its first prompt, and told to carry on.
    #[test]
    fn replaces_a_host_and_carries_it_on() {
        let home = Home::new();
        let cfg = start(&home, "rp1");
        let mut info = read_info("rp1").unwrap();
        info.limit = Some(Limit::default());
        replace_host(&info, true, &spawn_fake).unwrap();
        let carried = || events("rp1").contains("on another account now");
        assert!(until(&home, "rp1", || starts(&home).len() == 2 && carried()));
        let s = starts(&home);
        assert_eq!(s.len(), 2, "{s:?} {}", events("rp1"));
        assert_eq!(s[1], format!("start {}", cfg.session_id), "resumed");
        let back = read_config("rp1").unwrap();
        assert!(back.resume && back.prompt.is_empty(), "{back:?}");
        let ev = events("rp1");
        assert!(ev.contains("host-replace"), "{ev}");
        assert!(ev.contains("\"carryOn\":true"), "{ev}");
        stop("rp1");
    }

    // What it had queued goes to the new one, in place of a continue;
    // one whose profile waits gets no continue.
    #[test]
    fn sends_the_queue_not_a_continue() {
        let home = Home::new();
        start(&home, "rp2");
        let mut info = read_info("rp2").unwrap();
        info.queue = vec!["a".into(), "b".into()];
        info.limit = Some(Limit::default());
        replace_host(&info, true, &spawn_fake).unwrap();
        assert!(until(&home, "rp2", || events("rp2").contains("host-replace")));
        let ev = events("rp2");
        assert!(ev.contains("\"queued\":2"), "{ev}");
        assert!(!ev.contains("on another account now"), "{ev}");
        stop("rp2");

        let home = Home::new();
        start(&home, "rp3");
        let mut info = read_info("rp3").unwrap();
        info.limit = Some(Limit::default());
        replace_host(&info, false, &spawn_fake).unwrap();
        assert!(until(&home, "rp3", || events("rp3").contains("host-replace")));
        let ev = events("rp3");
        assert!(ev.contains("\"carryOn\":false"), "{ev}");
        assert!(!ev.contains("on another account now"), "{ev}");
        stop("rp3");
    }

    // One with no host to dial isn't started again; the event says why.
    #[test]
    fn a_host_gone_is_left() {
        let _home = Home::new();
        std::fs::create_dir_all(crate::paths::dir("nohost")).unwrap();
        let info = Info {
            id: "nohost".into(),
            ..Info::default()
        };
        assert!(replace_host(&info, true, &spawn_fake).is_err());
        let ev = events("nohost");
        assert!(
            ev.contains("host-replace") && ev.contains("\"err\":\""),
            "{ev}"
        );
    }

    // An idle host is replaced at the first look; one stopped first is
    // left as it is.
    #[test]
    fn replaces_once_idle_unless_stopped() {
        let home = Home::new();
        start(&home, "rw1");
        replace_when_idle("rw1", Duration::from_millis(1), &spawn_fake);
        assert!(until(&home, "rw1", || events("rw1").contains("host-replace")));
        stop("rw1");

        let home = Home::new();
        start(&home, "rw2");
        stop("rw2");
        let stopped = || read_info("rw2").is_ok_and(|i| i.state == "stopped");
        assert!(until(&home, "rw2", stopped));
        replace_when_idle("rw2", Duration::from_millis(1), &spawn_fake);
        assert!(
            !events("rw2").contains("host-replace"),
            "a stopped one stays stopped"
        );
    }
}
