//! Claude Code's logins through the vault, and switching on rush's own:
//! after Go's internal/ui logins_test.go, logins_roomy_test.go and
//! agentquota_test.go. A fake keeper stands in for the vault, so nothing
//! reads the keychain or ~/.claude; every test has a rush folder of its
//! own, and a model with no loop runs handed-off work at once.

use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    Account, Accounts, Adapter, Features, Kind, Level, Profile, Programmer, Registry, SignInDone,
};
use rush_core::agent::{Agent, State};
use rush_core::usage::{Quota, Reading, Window};
use rush_state::vault::{FoundLogin, FoundLogins, LoginKeeper, Restored};
use rush_state::{Config, Login, SignIn, Store};

use super::autoswitch::{auto_switch, check_limits};
use super::keys::login_ops;
use super::logins::{
    find_logins, forget_login, limit_stopped, login_name, on_added_login, renew_login,
    switch_login, use_keeper,
};
use super::state::{LoginView, accounts, cfg, cfg_mut, peek, set_store};
use crate::frame::messages::SayKind;
use crate::model::Model;

const NOW: i64 = 1_793_190_896_789;

/// What the vault was asked, in order, and what it answers.
#[derive(Default)]
struct Fake {
    asked: Mutex<Vec<String>>,
    found: Mutex<FoundLogins>,
    fail: Option<String>,
}

impl Fake {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
    fn note(&self, s: String) {
        self.asked.lock().unwrap().push(s);
    }
    fn err(&self) -> std::io::Result<()> {
        match &self.fail {
            Some(e) => Err(std::io::Error::other(e.clone())),
            None => Ok(()),
        }
    }
}

impl LoginKeeper for Fake {
    fn find_logins(&self, _: &Config) -> FoundLogins {
        self.note("find".into());
        self.found.lock().unwrap().clone()
    }
    fn refresh_login(&self, _: &str, _: &Config, l: &Login, _: bool) -> Reading {
        self.note(format!("refresh {}", l.id));
        Reading::default()
    }
    fn renew_login(&self, _: &Config, l: &Login) -> Result<(), String> {
        self.note(format!("renew {}", l.id));
        self.err().map_err(|e| e.to_string())
    }
    fn using_login(&self, _: &Config) -> String {
        String::new()
    }
    fn use_login(&self, _: &Config, l: &Login) -> std::io::Result<()> {
        self.note(format!("use {}", l.id));
        self.err()
    }
    fn adopt_login(&self, p: &Profile) -> std::io::Result<Login> {
        self.note(format!("adopt {}", p.name));
        Ok(Login::default())
    }
    fn forget_login(&self, id: &str) -> std::io::Result<()> {
        self.note(format!("forget {id}"));
        Ok(())
    }
}

/// A model on a fresh config in a rush folder of its own, the vault a
/// fake one.
fn model(fake: Fake) -> (Model, Arc<Fake>, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.reg = Registry::new();
    m.clock = Some(NOW);
    m.follows = false;
    set_store(&mut m, Store::default());
    let k = Arc::new(fake);
    use_keeper(&mut m, k.clone());
    (m, k, d)
}

fn login(id: &str) -> Login {
    Login {
        id: id.into(),
        name: id.into(),
        email: format!("{id}@example.com"),
        ..Login::default()
    }
}

fn window(id: &str, hours: u64, percent: f64) -> Window {
    Window {
        id: id.into(),
        span: std::time::Duration::from_secs(hours * 3600),
        percent,
        ..Window::default()
    }
}

fn quota(five_hour: f64, seven_day: f64) -> Quota {
    Quota {
        fetched_at: NOW,
        windows: vec![
            window("five_hour", 5, five_hour),
            window("seven_day", 7 * 24, seven_day),
        ],
        ..Quota::default()
    }
}

fn login_at(id: &str, current: bool, five_hour: f64) -> LoginView {
    LoginView {
        login: login(id),
        current,
        quota: quota(five_hour, 40.0),
        ..LoginView::default()
    }
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn said(m: &Model) -> (SayKind, String) {
    let l = m.said.last().expect("rush said something");
    (l.kind, l.text.clone())
}

/// a nearly out, b with room.
fn nearly_out(m: &mut Model) {
    for id in ["a", "b"] {
        cfg_mut(m).logins.push(login(id));
    }
    accounts(m).logins = vec![login_at("a", true, 99.0), login_at("b", false, 10.0)];
}

// ---- Switching on rush's own: only the rush that leads ----

/// Nearly out of the login in use, the rush that leads switches to the
/// next through the vault and says why as news; one that follows never
/// does (logins.go:240, hosted.go:70 leads).
#[test]
fn only_the_lead_switches_logins() {
    let (mut m, k, _d) = model(Fake::default());
    nearly_out(&mut m);
    m.follows = true;
    auto_switch(&mut m);
    assert!(k.asked().is_empty(), "a rush that follows switches nothing");
    assert_eq!(peek(&m).login_switched_at, 0);
    m.follows = false;
    auto_switch(&mut m);
    assert_eq!(k.asked(), ["use b"]);
    assert!(!peek(&m).switching, "the switch is over");
    assert_eq!(peek(&m).login_switched_at, NOW, "onSwitched's switchedAt");
    assert_eq!(
        said(&m),
        (
            SayKind::News,
            "switched to b: a was at 99% · new sessions run on b".into()
        )
    );
    // Not again until the readings catch up (switchGap).
    auto_switch(&mut m);
    assert_eq!(k.asked(), ["use b"]);
}

/// A switch you pick is an answer, not news; one that fails says so;
/// while one is under way another isn't started (switchLogin, onSwitched).
#[test]
fn switching_a_login_you_picked() {
    let (mut m, k, _d) = model(Fake::default());
    nearly_out(&mut m);
    let ops = login_ops(&m).expect("the vault's operations");
    (ops.switch)(&mut m, login("b"), "", false);
    assert_eq!(k.asked(), ["use b"]);
    assert_eq!(
        flash(&m),
        "new sessions run on b · running ones follow once their cache cools"
    );
    accounts(&mut m).switching = true;
    switch_login(&mut m, login("a"), "", true);
    assert_eq!(k.asked(), ["use b"], "one at a time");

    let (mut m, _k, _d) = model(Fake {
        fail: Some("no keychain".into()),
        ..Fake::default()
    });
    switch_login(&mut m, login("b"), "a was at 99%", true);
    assert_eq!(flash(&m), "couldn't switch to b: no keychain");
    assert!(!peek(&m).switching);
    assert_eq!(peek(&m).login_switched_at, 0);
}

/// A session a limit stopped on the folder in use, under a profile that
/// moves on, counts as stopped; one whose profile waits doesn't
/// (logins.go:246-253).
#[test]
fn limit_stopped_sessions() {
    let (mut m, _k, _d) = model(Fake::default());
    let root = cfg(&m).active_account().name;
    let mut a = Agent {
        key: "k".into(),
        rush: true,
        ..Agent::default()
    };
    a.job.account = root.clone();
    a.job.state = State::Blocked;
    a.job.detail = "usage limit · resets 4pm".into();
    m.agents = vec![a.clone()];
    assert!(limit_stopped(&m, &root));
    assert!(!limit_stopped(&m, "elsewhere"));
    m.agents[0].job.state = State::Stopped;
    assert!(
        !limit_stopped(&m, &root),
        "one stopped since waits for nothing"
    );
    m.agents[0] = a;
    cfg_mut(&mut m).at_limit = rush_state::setup::AT_LIMIT_WAIT.into();
    assert!(!limit_stopped(&m, &root), "its profile waits");
}

// ---- Looking for logins ----

/// The rush that leads looks for logins and keeps new ones, named from
/// their email; one that follows leaves it to the lead, as a look can
/// switch ~/.claude back (findLogins, onLogins).
#[test]
fn only_the_lead_looks_for_logins() {
    let found = FoundLogins {
        found: vec![FoundLogin {
            login: Login {
                id: "uuid-1234567890".into(),
                email: "sam@example.com".into(),
                ..Login::default()
            },
            name: String::new(),
        }],
        ..FoundLogins::default()
    };
    let (mut m, k, _d) = model(Fake {
        found: Mutex::new(found),
        ..Fake::default()
    });
    m.follows = true;
    find_logins(&mut m);
    assert!(k.asked().is_empty());
    m.follows = false;
    find_logins(&mut m);
    assert_eq!(k.asked()[0], "find");
    let names: Vec<String> = cfg(&m).logins.iter().map(|l| l.name.clone()).collect();
    assert_eq!(names, ["sam"]);
    assert_eq!(
        &k.asked()[1..],
        ["refresh uuid-1234567890"],
        "a new one's usage is read at once"
    );
    // Found again, with its email changed: kept, renamed nothing.
    k.found.lock().unwrap().found[0].login.email = "sam@new.example.com".into();
    find_logins(&mut m);
    assert_eq!(cfg(&m).logins.len(), 1);
    assert_eq!(cfg(&m).logins[0].email, "sam@new.example.com");
    assert_eq!(cfg(&m).logins[0].name, "sam");
}

/// A look that fails is said once; a login a Claude Code put back,
/// switched again, is news (onLogins' news, logins.go:102-118).
#[test]
fn a_put_back_login_is_news() {
    let found = FoundLogins {
        restored: Some(Restored {
            was: "id-2".into(),
            now: "id-1".into(),
            err: None,
        }),
        failed: Some("keychain locked".into()),
        ..FoundLogins::default()
    };
    let (mut m, _k, _d) = model(Fake {
        found: Mutex::new(found),
        ..Fake::default()
    });
    cfg_mut(&mut m).logins = vec![login("id-1"), login("id-2")];
    find_logins(&mut m);
    let texts: Vec<&str> = m.said.iter().map(|l| l.text.as_str()).collect();
    assert!(texts.contains(&"rush can't switch account: keychain locked"));
    assert_eq!(
        said(&m).1,
        "rush switched back to id-1: a Claude Code started on id-2 had signed ~/.claude back in as it"
    );
}

/// A new login takes its folder's name, or its email's, unless another
/// has it: then its email; with neither, its id's first 8 (loginName).
#[test]
fn new_logins_are_named() {
    let (mut m, _k, _d) = model(Fake::default());
    let f = |name: &str, email: &str| FoundLogin {
        login: Login {
            id: "0123456789abcdef".into(),
            email: email.into(),
            ..Login::default()
        },
        name: name.into(),
    };
    assert_eq!(login_name(&m, &f("work", "sam@x.com")), "work");
    assert_eq!(login_name(&m, &f("", "sam@x.com")), "sam");
    assert_eq!(login_name(&m, &f("", "")), "01234567");
    cfg_mut(&mut m).logins.push(login("sam"));
    assert_eq!(login_name(&m, &f("", "sam@x.com")), "sam@x.com");
}

// ---- Renewing, adding and forgetting ----

/// An expired sign-in refreshed says so; one that can't be keeps why on
/// its row (renewLogin, renewedMsg.applyTo).
#[test]
fn renewing_a_sign_in() {
    let (mut m, k, _d) = model(Fake::default());
    let lv = login_at("b", false, 10.0);
    renew_login(&mut m, &lv);
    assert_eq!(k.asked(), ["renew b", "refresh b"]);
    assert_eq!(flash(&m), "refreshed b's sign-in");
    assert!(peek(&m).renew.is_empty());

    let (mut m, k, _d) = model(Fake {
        fail: Some("refresh token revoked".into()),
        ..Fake::default()
    });
    renew_login(&mut m, &lv);
    assert_eq!(k.asked(), ["renew b"]);
    assert_eq!(peek(&m).renew["b"].why, "refresh token revoked");
}

/// A login signed in to from Accounts is added, or signed in to again
/// keeping its name; forgetting one drops its vault entry (onAddedLogin,
/// accounts.go:650).
#[test]
fn adding_and_forgetting() {
    let (mut m, k, _d) = model(Fake::default());
    on_added_login(&mut m, "work", Ok(login("id-9")));
    assert_eq!(
        flash(&m),
        "added work (id-9@example.com) · enter switches to it"
    );
    cfg_mut(&mut m).logins[0].borrowed = true;
    on_added_login(&mut m, "other", Ok(login("id-9")));
    assert_eq!(flash(&m), "signed in to work again (id-9@example.com)");
    assert!(cfg(&m).logins[0].borrowed);
    on_added_login(&mut m, "x", Err("cancelled".into()));
    assert_eq!(flash(&m), "didn't sign in to x: cancelled");
    forget_login(&mut m, "id-9");
    assert_eq!(k.asked().last().map(String::as_str), Some("forget id-9"));
}

// ---- Other agents' accounts (agentquota.go:90-114) ----

/// Acme: an agent with accounts rush switches, its program in bin.
struct Acme(Arc<Mutex<Vec<String>>>);

impl Adapter for Acme {
    fn kind(&self) -> Kind {
        "acme".into()
    }
    fn name(&self) -> String {
        "Acme".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        vec![Profile {
            kind: "acme".into(),
            name: "default".into(),
            dir: "/acme".into(),
        }]
    }
    fn programmer(&self) -> Option<&dyn Programmer> {
        Some(self)
    }
    fn accounts(&self) -> Option<&dyn Accounts> {
        Some(self)
    }
}

impl Programmer for Acme {
    fn program(&self) -> (String, Vec<String>) {
        ("acme".into(), Vec::new())
    }
}

impl Accounts for Acme {
    fn current(&self, _: &Profile) -> rush_core::adapter::Result<Account> {
        Ok(Account::default())
    }
    fn switch(&self, _: &Profile, a: &Account) -> rush_core::adapter::Result<()> {
        self.0.lock().unwrap().push(a.id.clone());
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

/// The one in use nearly out, the rush that leads switches Acme to the
/// account with the most room, once a switch gap; one that follows never
/// does (checkLimits, nextAccount).
#[test]
fn only_the_lead_switches_accounts() {
    let (mut m, _k, d) = model(Fake::default());
    let bin = tempfile::tempdir().unwrap();
    let p = bin.path().join("acme");
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let reg = Registry::with_env(&bin.path().to_string_lossy(), &d.path().to_string_lossy());
    let switched = Arc::new(Mutex::new(Vec::new()));
    reg.register(Acme(switched.clone()));
    m.reg = reg;
    for (id, name) in [("a1", "work"), ("a2", "home"), ("a3", "spare")] {
        cfg_mut(&mut m).sign_ins.push(SignIn {
            kind: "acme".into(),
            id: id.into(),
            name: name.into(),
            ..SignIn::default()
        });
    }
    let keys: Vec<String> = ["a1", "a2", "a3"]
        .iter()
        .map(|id| super::rows::acct_key("acme", id))
        .collect();
    let a = accounts(&mut m);
    a.now.insert("acme".into(), "a1".into());
    a.quotas.insert(keys[0].clone(), quota(99.0, 50.0));
    a.quotas.insert(keys[1].clone(), quota(60.0, 50.0));
    a.quotas.insert(keys[2].clone(), quota(20.0, 50.0));
    m.follows = true;
    check_limits(&mut m);
    assert!(
        switched.lock().unwrap().is_empty(),
        "a rush that follows switches nothing"
    );
    m.follows = false;
    check_limits(&mut m);
    assert_eq!(*switched.lock().unwrap(), ["a3"], "the most room");
    check_limits(&mut m);
    assert_eq!(
        switched.lock().unwrap().len(),
        1,
        "not again within the gap"
    );
    // With the default profile waiting at a limit, nothing moves.
    accounts(&mut m).switched_at.clear();
    cfg_mut(&mut m).at_limit = rush_state::setup::AT_LIMIT_WAIT.into();
    check_limits(&mut m);
    assert_eq!(
        switched.lock().unwrap().len(),
        1,
        "the default profile waits"
    );
}
