//! What accounts hands Settings › Providers and Harnesses (forsettings):
//! who each agent is signed in as and why not, the order the logins are
//! spent in, r reading every limit again and u spending an earned reset.
//! After Go's settings_providers.go (r :271, chainRow :636, useReset
//! :946) and settings_auth.go:61 (nativeAccountRows). A fake vault and a
//! fake agent stand in: nothing reads the keychain, ~/.claude or the
//! network, and every test has a rush folder of its own.

use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    Account, AccountSummary, Accounts, Adapter, Features, Kind, Level, Profile, Programmer,
    QuotaSource, Registry, ResetSpender, SignInDone,
};
use rush_core::usage::{Quota, Resets, Window};
use rush_state::vault::{FoundLogins, LoginKeeper};
use rush_state::{Config, Login, SignIn, Store};

use super::keys::{SignIns, took_sign_ins};
use super::logins::use_keeper;
use super::state::{LoginView, accounts, cfg_mut, set_store};
use crate::frame::messages::SayKind;
use crate::model::Model;
use crate::screens::settings::acct::{self as sacct, AcctRow as SRow};

const NOW: i64 = 1_793_190_896_789;
const HOUR: i64 = 3_600_000;

/// A vault that notes what it's asked.
#[derive(Default)]
struct Vault(Mutex<Vec<String>>);

impl LoginKeeper for Vault {
    fn find_logins(&self, _: &Config) -> FoundLogins {
        FoundLogins::default()
    }
    fn refresh_login(&self, _: &str, _: &Config, l: &Login, _: bool) -> rush_core::usage::Reading {
        self.0.lock().unwrap().push(format!("refresh {}", l.id));
        Default::default()
    }
    fn renew_login(&self, _: &Config, _: &Login) -> Result<(), String> {
        Ok(())
    }
    fn using_login(&self, _: &Config) -> String {
        String::new()
    }
    fn use_login(&self, _: &Config, _: &Login) -> std::io::Result<()> {
        Ok(())
    }
    fn adopt_login(&self, _: &Profile) -> std::io::Result<Login> {
        Ok(Login::default())
    }
    fn forget_login(&self, _: &str) -> std::io::Result<()> {
        Ok(())
    }
}

fn model() -> (Model, Arc<Vault>, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.reg = Registry::new();
    m.clock = Some(NOW);
    m.now = NOW;
    m.follows = false;
    set_store(&mut m, Store::default());
    let v = Arc::new(Vault::default());
    use_keeper(&mut m, v.clone());
    (m, v, d)
}

fn said(m: &Model) -> (SayKind, String) {
    let l = m.said.last().expect("rush said something");
    (l.kind, l.text.clone())
}

fn week(percent: f64, resets_in: i64) -> Window {
    Window {
        id: "seven_day".into(),
        span: std::time::Duration::from_secs(7 * 24 * 3600),
        percent,
        resets_at: NOW + resets_in,
        ..Window::default()
    }
}

fn login(id: &str, current: bool, percent: f64, resets_in: i64) -> LoginView {
    LoginView {
        login: Login {
            id: id.into(),
            name: id.into(),
            ..Login::default()
        },
        quota: Quota {
            fetched_at: NOW,
            windows: vec![week(percent, resets_in)],
            ..Quota::default()
        },
        current,
        ..LoginView::default()
    }
}

/// Who each agent is signed in as, as its own sign-in said, and why not
/// (accountsState's why, summary and now); Harnesses' Status and who
/// rows read them (nativeAccountRows).
#[test]
fn who_each_agent_is_signed_in_as() {
    let (mut m, _, _d) = model();
    let a = sacct::accounts();
    assert_eq!((a.why)(&m, "pi"), "");
    assert!((a.summary)(&m, "pi").is_none());
    assert!(!(a.signed_in)(&m, "pi"));
    let rows = crate::screens::settings::harnesses::native_account_rows(&m, "pi");
    assert_eq!(
        (rows[0].label.as_str(), rows[0].value.as_str()),
        ("Status", "not checked")
    );

    took_sign_ins(
        &mut m,
        SignIns {
            kind: "pi".into(),
            summary: Some(AccountSummary {
                signed_in: true,
                method: "OAuth".into(),
                email: "me@example.com".into(),
                plan: "Pro".into(),
                ..AccountSummary::default()
            }),
            ..SignIns::default()
        },
    );
    assert!((a.signed_in)(&m, "pi"));
    assert_eq!((a.summary)(&m, "pi").unwrap().email, "me@example.com");
    let rows = crate::screens::settings::harnesses::native_account_rows(&m, "pi");
    let got: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.label.clone(), r.value.clone()))
        .collect();
    assert_eq!(
        got,
        [
            ("Status", "credentials available"),
            ("Email", "me@example.com"),
            ("Plan", "Pro"),
            ("Sign-in method", "OAuth"),
            ("Refresh account and models", ""),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );

    // Signed out: why, and nothing it's signed in as.
    took_sign_ins(
        &mut m,
        SignIns {
            kind: "pi".into(),
            err: Some("not signed in".into()),
            ..SignIns::default()
        },
    );
    assert_eq!((a.why)(&m, "pi"), "not signed in");
    assert!(!(a.signed_in)(&m, "pi"));
}

/// The order the logins are spent in: the one in use, then the rest by
/// how much room they'd lose soonest, a full one last with its wait; a
/// borrowed one not in use is left out (fleet.Chain).
#[test]
fn the_switch_order() {
    let (mut m, _, _d) = model();
    let a = sacct::accounts();
    assert!((a.chain)(&m).is_empty());
    let mut borrowed = login("lent", false, 0.0, HOUR);
    borrowed.login.borrowed = true;
    accounts(&mut m).logins = vec![
        login("alsome", false, 60.0, 48 * HOUR),
        login("personal", false, 100.0, 7 * 24 * HOUR),
        login("alex", true, 68.0, 31 * HOUR),
        login("soon", false, 10.0, 2 * HOUR),
        borrowed,
    ];
    let chain = (a.chain)(&m);
    let got: Vec<(&str, bool, f64, i64, i64)> = chain
        .iter()
        .map(|k| (k.name.as_str(), k.current, k.week_percent, k.left, k.wait))
        .collect();
    assert_eq!(
        got,
        [
            ("alex", true, 68.0, 31 * HOUR, 0),
            ("soon", false, 10.0, 2 * HOUR, 0),
            ("alsome", false, 60.0, 48 * HOUR, 0),
            ("personal", false, 100.0, 7 * 24 * HOUR, 7 * 24 * HOUR),
        ]
    );
}

/// r on Providers reads every limit again: rush's logins through the
/// vault, then who each agent is signed in as; it no longer says
/// accounts aren't switched here (settings_providers.go:271).
#[test]
fn r_reads_every_limit_again() {
    let (mut m, v, _d) = model();
    for id in ["a", "b"] {
        cfg_mut(&mut m).logins.push(Login {
            id: id.into(),
            name: id.into(),
            ..Login::default()
        });
    }
    accounts(&mut m).logins = vec![login("a", true, 10.0, HOUR), login("b", false, 10.0, HOUR)];
    (sacct::accounts().fetch)(&mut m);
    assert_eq!(
        *v.0.lock().unwrap(),
        ["refresh b"],
        "all but the one in use"
    );
    assert!(
        m.said.iter().all(|s| s.kind != SayKind::Refused),
        "{:?}",
        m.said.iter().map(|s| s.text.clone()).collect::<Vec<_>>()
    );
}

/// Credits: an agent with accounts that earn limit resets, its program
/// in bin; what it was asked to spend.
struct Credits(Arc<Mutex<Vec<String>>>);

impl Adapter for Credits {
    fn kind(&self) -> Kind {
        "credits".into()
    }
    fn name(&self) -> String {
        "Credits".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        vec![Profile {
            kind: "credits".into(),
            name: "default".into(),
            dir: "/credits".into(),
        }]
    }
    fn programmer(&self) -> Option<&dyn Programmer> {
        Some(self)
    }
    fn accounts(&self) -> Option<&dyn Accounts> {
        Some(self)
    }
    fn quota_source(&self) -> Option<&dyn QuotaSource> {
        Some(self)
    }
    fn reset_spender(&self) -> Option<&dyn ResetSpender> {
        Some(self)
    }
}

impl Programmer for Credits {
    fn program(&self) -> (String, Vec<String>) {
        ("credits".into(), Vec::new())
    }
}

impl Accounts for Credits {
    fn current(&self, _: &Profile) -> rush_core::adapter::Result<Account> {
        Ok(Account::default())
    }
    fn switch(&self, _: &Profile, _: &Account) -> rush_core::adapter::Result<()> {
        Ok(())
    }
    fn sign_in(
        &self,
        _: &Profile,
    ) -> rush_core::adapter::Result<(std::process::Command, SignInDone)> {
        Err("no".into())
    }
    fn forget(&self, _: &Account) -> rush_core::adapter::Result<()> {
        Ok(())
    }
}

impl QuotaSource for Credits {
    fn quota(&self, _: &Profile, _: &Account) -> rush_core::adapter::Result<Quota> {
        Ok(Quota {
            account: "credits:w1".into(),
            fetched_at: NOW + 1,
            windows: vec![week(0.0, 7 * 24 * HOUR)],
            ..Quota::default()
        })
    }
}

impl ResetSpender for Credits {
    fn use_reset(&self, p: &Profile, id: &str) -> rush_core::adapter::Result<String> {
        self.0.lock().unwrap().push(format!("{} {id:?}", p.dir));
        Ok("limits reset".into())
    }
}

/// u spends an earned reset on the account in use once you say so, and
/// its limits are read again at once; one not in use, or with none,
/// says why not (useReset).
#[test]
fn u_spends_an_earned_reset() {
    let (mut m, _, d) = model();
    let bin = tempfile::tempdir().unwrap();
    let p = bin.path().join("credits");
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let reg = Registry::with_env(&bin.path().to_string_lossy(), &d.path().to_string_lossy());
    let spent = Arc::new(Mutex::new(Vec::new()));
    reg.register(Credits(spent.clone()));
    m.reg = reg;
    for (id, name) in [("w1", "work"), ("h1", "home")] {
        cfg_mut(&mut m).sign_ins.push(SignIn {
            kind: "credits".into(),
            id: id.into(),
            name: name.into(),
            ..SignIn::default()
        });
    }
    let with_resets = |n: i64| Quota {
        fetched_at: NOW,
        windows: vec![week(90.0, 7 * 24 * HOUR)],
        resets: Some(Resets {
            available: n,
            ..Resets::default()
        }),
        ..Quota::default()
    };
    let a = accounts(&mut m);
    a.now.insert("credits".into(), "w1".into());
    a.quotas.insert("credits:w1".into(), with_resets(2));
    a.quotas.insert("credits:h1".into(), with_resets(1));
    let row = |name: &str| SRow {
        kind: "credits".into(),
        name: name.into(),
        ..SRow::default()
    };
    let use_reset = sacct::accounts().use_reset;

    use_reset(&mut m, &row("home"));
    assert_eq!(
        said(&m),
        (
            SayKind::Said,
            "a reset goes to the account in use: enter switches to home, then u".into()
        )
    );
    use_reset(&mut m, &row("work"));
    assert_eq!(
        m.confirm.as_ref().map(|c| c.question.clone()).as_deref(),
        Some("Use one of work's 2 limit resets now? Its limits go back to empty; 1 left after.")
    );
    m.confirm_key("y");
    assert_eq!(*spent.lock().unwrap(), ["/credits \"\""]);
    assert_eq!(said(&m), (SayKind::Said, "work: limits reset".into()));
    let a = accounts(&mut m);
    assert_eq!(
        a.quotas["/credits"].fetched_at,
        NOW + 1,
        "read again at once"
    );
    assert_eq!(a.quotas["credits:w1"].windows[0].percent, 0.0);
    let kept = rush_core::usage::load(&d.path().join("quotas.json"));
    assert!(kept.contains_key("credits:/credits"), "{:?}", kept.keys());

    // Its resets spent: none left to use.
    use_reset(&mut m, &row("work"));
    assert_eq!(said(&m), (SayKind::Said, "work has no limit resets".into()));
}

/// Spending a reset and reading the limits after it have 30 seconds
/// between them, as Go's one context (settings_providers.go:973): what
/// takes longer is given up on.
#[test]
fn a_reset_gives_up_after_its_time() {
    use super::forsettings::{RESET_WITHIN, within};
    use std::time::{Duration, Instant};
    assert_eq!(RESET_WITHIN, Duration::from_secs(30));
    let by = Instant::now() + Duration::from_millis(50);
    assert_eq!(within(by, || 7), Some(7));
    let slow = within(by, || std::thread::sleep(Duration::from_millis(400)));
    assert_eq!(slow, None, "too slow");
    assert_eq!(within(Instant::now(), || 1), None, "no time left");
}
