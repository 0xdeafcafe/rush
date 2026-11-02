//! The tests of rush-ui-social's services.rs that need the whole UI.

use crate::frame::Out;
use crate::model::Model;
use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_ui_social::services::*;
use rush_update::Release;

use rush_core::agent::{Halt, State};

#[test]
fn backs_off() {
    assert_eq!(online_backoff(0), 0);
    assert_eq!(online_backoff(1), 15_000);
    assert_eq!(online_backoff(3), 60_000);
}

#[test]
fn sha256_known_answers() {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(
        hex(&sha2_lite::sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&sha2_lite::sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

fn cut(key: &str, now: Ms, ago: Ms, text: &str) -> Agent {
    Agent {
        key: key.into(),
        display_name: key.into(),
        pid: 7,
        job: rush_core::agent::Job {
            state: State::Done,
            updated_at: now - ago,
            ..Default::default()
        },
        spend: rush_core::agent::Spend {
            halt: Some(Halt {
                kind: "server_error".into(),
                text: text.into(),
                at: now - ago,
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

// Once the API can be reached, each session an API error stopped is
// told to continue once per halt, backing off, up to ONLINE_MOST.
#[test]
fn network_back_continues_what_it_stopped() {
    let mut m = crate::kit::model(100, 30);
    let now = m.clock.unwrap();
    m.now = now;
    let offline = "API Error: Unable to connect to API (ENOTFOUND)";
    m.agents = vec![cut("a", now, 60_000, offline)];
    let keys = |m: &mut Model| -> Vec<String> {
        m.continue_waiting()
            .into_iter()
            .map(|i| m.agents[i].key.clone())
            .collect()
    };
    assert_eq!(keys(&mut m), ["a"]);
    let at = halt_at(&m.agents[0]);
    m.kept::<Online>().sent.insert("a".into(), at);
    m.kept::<Online>().tries.insert("a".into(), 1);
    assert!(keys(&mut m).is_empty(), "told again for the same halt");
    m.agents[0] = cut(
        "a",
        now,
        10_000,
        "API Error: Connection dropped (ECONNRESET)",
    );
    assert!(keys(&mut m).is_empty(), "told again before its backoff");
    m.agents[0] = cut(
        "a",
        now,
        20_000,
        "API Error: Connection dropped (ECONNRESET)",
    );
    assert_eq!(keys(&mut m).len(), 1, "a new halt past its backoff");
    m.kept::<Online>().tries.insert("a".into(), ONLINE_MOST);
    assert!(keys(&mut m).is_empty(), "told past ONLINE_MOST");
    m.agents[0].spend.halt = None;
    let l = m.look();
    assert!(l.answered && !m.kept::<Online>().tries.contains_key("a"));
}

// A look shares each new halt once, and says whether each waiting
// session is still warm: its cache dates from its first halt in a row.
#[test]
fn look_shares_halts_and_warmth() {
    let mut m = crate::kit::model(100, 30);
    let now = m.clock.unwrap();
    m.now = now;
    let dropped = "API Error: Connection dropped (ECONNRESET)";
    let mut cold = cut("cold", now, 2 * 60_000, dropped);
    cold.job.updated_at = now;
    m.agents = vec![cut("warm", now, 60_000, dropped), cold];
    m.kept::<Online>()
        .first
        .insert("cold".into(), now - 2 * 3_600_000);
    let l = m.look();
    assert!(
        l.failed.len() == 2 && l.waiting["warm"] && !l.waiting["cold"],
        "{l:?}"
    );
    assert!(m.look().failed.is_empty(), "halts shared twice");
}

#[test]
fn continue_is_claimed_once() {
    let home = tempfile::tempdir().unwrap();
    let at = 1_790_000_000_000;
    assert!(claim_continue(home.path(), "k", at));
    assert!(
        !claim_continue(home.path(), "k", at),
        "the second rush finds it claimed"
    );
    assert!(
        claim_continue(home.path(), "k", at + 1000),
        "a new halt is a new claim"
    );
}

// #reload hands the next rush the agent selected and where the keys
// were, and that rush picks them up once the agent is listed.
#[test]
fn reload_keeps_the_place() {
    let mut m = crate::kit::model(100, 30);
    m.sel = "claude:x:a".into();
    assert!(m.reload_env().is_none(), "no reload asked for");
    m.kept::<Reload>().installed = Some(|| Ok(()));
    assert_eq!(m.reload(false), Out::Draw);
    let env = m.reload_env().expect("reloading");
    let (k, v) = env.split_once('=').unwrap();
    assert_eq!((k, v), (RESTORE_ENV, "0claude:x:a"));
    let mut n = crate::kit::model(100, 30);
    n.take_restore(Some("1claude:x:a".into()));
    let mut a = Agent {
        key: "claude:x:a".into(),
        pid: 7,
        ..Default::default()
    };
    a.job.state = rush_core::agent::State::Working;
    n.set_fleet(crate::worker::Fleet {
        agents: vec![a],
        roots: vec![String::new()],
        ..Default::default()
    });
    n.apply_restore();
    assert_eq!(n.sel, "claude:x:a");
}

// The agent the last rush had, folded away in Earlier now, isn't
// picked out of sight: the list's first row stays picked.
#[test]
fn restore_skips_a_folded_row() {
    let mut n = crate::kit::model(100, 30);
    n.take_restore(Some("0claude:x:old".into()));
    let mut old = Agent {
        key: "claude:x:old".into(),
        display_name: "old".into(),
        done: true,
        ..Default::default()
    };
    old.job.state = rush_core::agent::State::Idle;
    let mut live = Agent {
        key: "claude:x:live".into(),
        display_name: "live".into(),
        pid: 7,
        ..Default::default()
    };
    live.job.state = rush_core::agent::State::Working;
    n.set_fleet(crate::worker::Fleet {
        agents: vec![old, live],
        roots: vec![String::new(), String::new()],
        ..Default::default()
    });
    n.apply_restore();
    assert!(n.folded("Earlier"));
    assert_eq!(n.sel, "claude:x:live");
}

// An installed rush that doesn't start leaves the view open, saying so.
#[test]
fn reload_stays_when_the_installed_rush_fails() {
    let mut m = crate::kit::model(100, 30);
    m.kept::<Reload>().installed = Some(|| Err("exit status 2: dyld: missing symbol".into()));
    m.reload(false);
    assert!(m.reload_env().is_none());
    let (said, _, err) = m.flash.clone().unwrap();
    assert!(
        err && said.contains("doesn't start") && said.contains("missing symbol"),
        "{said}"
    );
}

#[test]
fn version_tags() {
    let mut m = crate::kit::model(100, 30);
    assert_eq!(m.version_tag().plain(), "rush test");
    m.kept::<Update>().newer = Some(Release {
        version: "0.9.0".into(),
        ..Release::default()
    });
    assert_eq!(
        m.version_tag().plain(),
        "rush test · v0.9.0 is out · #update"
    );
    m.kept::<Update>().rebuilt = true;
    assert!(
        m.version_tag()
            .plain()
            .ends_with("older than installed · #reload")
    );
}

/// The header's row with the notice of a newer rush on it, as text.
fn notice_row(m: &mut Model) -> String {
    m.header()
        .iter()
        .map(|l| l.plain())
        .find(|t| t.contains("↑ "))
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

fn beta5() -> Release {
    Release {
        version: "0.1.0-beta.5".into(),
        ..Release::default()
    }
}

/// An install that runs rush-update's fake brew in prefix, as an update
/// to beta.5 under Homebrew does.
fn brew_install(prefix: &std::path::Path) -> rush_ui_base::update::InstallFn {
    let brew = prefix.join("bin/brew");
    std::sync::Arc::new(
        move |h: &dyn rush_update::Host| -> Result<Release, rush_update::Error> {
            rush_update::brew_upgrade(&brew, "0.1.0-beta.5", h)?;
            Ok(beta5())
        },
    )
}

// A newer rush out shows in the header, with the key that updates; one
// installed and not running, with the key that restarts onto it; a click
// on it does what its key does.
#[test]
fn header_says_a_newer_rush() {
    let mut m = crate::kit::model(120, 30);
    assert_eq!(notice_row(&mut m), "", "nothing to say");
    m.kept::<Update>().newer = Some(beta5());
    let row = notice_row(&mut m);
    assert!(
        row.ends_with("↑ v0.1.0-beta.5 ready · alt+u update"),
        "{row:?}"
    );
    {
        let u = m.kept::<Update>();
        u.rebuilt = true;
        u.installed_version = "0.1.0-beta.5".into();
    }
    let row = notice_row(&mut m);
    assert!(
        row.ends_with("↑ v0.1.0-beta.5 installed · alt+u restart"),
        "{row:?}"
    );
    m.kept::<Reload>().installed = Some(|| Ok(()));
    let (y, from, to) = m.kept::<Update>().notice_at.expect("drawn");
    assert!(!m.click_update(from - 1, y), "beside it");
    assert!(m.reload_env().is_none());
    assert!(m.click_update(to - 1, y));
    assert!(m.reload_env().is_some(), "restarted onto it");
}

// The key runs brew (rush-update's fake one: its tap fetched, then the
// upgrade), then this rush reloads onto what it installed.
#[test]
fn update_key_runs_brew_then_reloads() {
    let mut m = crate::kit::model(120, 30);
    let prefix = tempfile::tempdir().unwrap();
    rush_update::testing::fake_brew(prefix.path(), 0, Some("0.1.0-beta.5"));
    m.kept::<Update>().newer = Some(beta5());
    m.kept::<Update>().install = Some(brew_install(prefix.path()));
    m.kept::<Reload>().installed = Some(|| Ok(()));
    m.press_key(&crate::frame::Press::of("alt+u").unwrap());
    assert_eq!(
        std::fs::read_to_string(prefix.path().join("ran")).unwrap(),
        "update --quiet noauto=\nupgrade 0xdeafcafe/tap/rush noauto=1\n"
    );
    let u = m.kept::<Update>();
    assert!(!u.updating && u.failed.is_empty(), "{}", u.failed);
    assert!(u.rebuilt && u.newer.is_none());
    assert!(m.reload_env().is_some(), "reloaded onto it");
}

// brew failing shows brew's error, under the box and in the header, and
// changes nothing: no reload, the newer rush still offered.
#[test]
fn brew_failing_changes_nothing() {
    let mut m = crate::kit::model(120, 30);
    let prefix = tempfile::tempdir().unwrap();
    rush_update::testing::fake_brew(prefix.path(), 1, None);
    m.kept::<Update>().newer = Some(beta5());
    m.kept::<Update>().install = Some(brew_install(prefix.path()));
    m.kept::<Reload>().installed = Some(|| Ok(()));
    m.press_key(&crate::frame::Press::of("alt+u").unwrap());
    let (said, _, err) = m.flash.clone().unwrap();
    assert!(err, "{said}");
    assert_eq!(said, "update failed, nothing changed: Error: no bottle");
    let u = m.kept::<Update>();
    assert!(!u.updating && !u.rebuilt);
    assert_eq!(u.newer, Some(beta5()));
    assert!(m.reload_env().is_none(), "reloaded");
    let row = notice_row(&mut m);
    assert!(
        row.ends_with("↑ update failed: Error: no bottle · alt+u again"),
        "{row:?}"
    );
}
