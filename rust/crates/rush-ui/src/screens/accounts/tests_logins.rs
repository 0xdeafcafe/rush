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
    find_logins, forget_login, limit_stopped, login_name, on_added_login, on_logins, renew_login,
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
    /// What refresh_login reads.
    reading: Mutex<Reading>,
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
        self.reading.lock().unwrap().clone()
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
    // Then every login's usage read again but the one in use's
    // (onSwitched's fetchUsage).
    assert_eq!(k.asked(), ["use b", "refresh b"]);
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
    assert_eq!(k.asked(), ["use b", "refresh b"]);
}

/// A switch you pick is an answer, not news; one that fails says so;
/// while one is under way another isn't started (switchLogin, onSwitched).
#[test]
fn switching_a_login_you_picked() {
    let (mut m, k, _d) = model(Fake::default());
    nearly_out(&mut m);
    let ops = login_ops(&m).expect("the vault's operations");
    (ops.switch)(&mut m, login("b"), "", false);
    assert_eq!(k.asked(), ["use b", "refresh b"]);
    assert_eq!(
        flash(&m),
        "new sessions run on b · running ones follow once their cache cools"
    );
    accounts(&mut m).switching = true;
    switch_login(&mut m, login("a"), "", true);
    assert_eq!(k.asked(), ["use b", "refresh b"], "one at a time");

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

// ---- After a look, after a switch (logins.go:102-160, :437-466) ----

/// A login a Claude Code put back has every login's usage read again
/// (onLogins' fetchUsage).
#[test]
fn a_put_back_login_reads_usage_again() {
    let found = FoundLogins {
        restored: Some(Restored {
            was: "id-2".into(),
            now: "id-1".into(),
            err: None,
        }),
        ..FoundLogins::default()
    };
    let (mut m, k, _d) = model(Fake {
        found: Mutex::new(found),
        ..Fake::default()
    });
    cfg_mut(&mut m).logins = vec![login("id-1"), login("id-2")];
    find_logins(&mut m);
    // None is in use as this rush folder says: each is read.
    assert_eq!(k.asked(), ["find", "refresh id-1", "refresh id-2"]);
    // A look that put nothing back reads nothing.
    k.found.lock().unwrap().restored = None;
    find_logins(&mut m);
    assert_eq!(k.asked(), ["find", "refresh id-1", "refresh id-2", "find"]);
}

/// An agent whose attribution can't be set: says so when asked.
struct Credits;

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
    rush_core::has!(attributor: Attributor);
}

impl rush_core::adapter::Attributor for Credits {
    fn attribution(&self, _: &Profile) -> (bool, bool) {
        (false, false)
    }
    fn set_attribution(&self, _: &Profile, _: bool) -> rush_core::adapter::Result<()> {
        Err("read-only".into())
    }
}

/// After a look, what General's Every agent says is written into the
/// agents, in the rush that leads only (model.go:1172 applyEverywhere).
#[test]
fn a_look_applies_attribution_only_where_leading() {
    let (mut m, _k, _d) = model(Fake::default());
    m.reg.register(Credits);
    cfg_mut(&mut m).everywhere.attribution = "on".into();
    m.follows = true;
    on_logins(&mut m, FoundLogins::default());
    assert!(m.said.is_empty(), "a rush that follows writes nothing");
    m.follows = false;
    on_logins(&mut m, FoundLogins::default());
    assert_eq!(
        said(&m),
        (
            SayKind::Failed,
            "couldn't set attribution in credits (default): read-only".into()
        )
    );
}

/// Logins found or a login switched show in Settings at once: it reads
/// the one store the accounts change (loadDialog from onLogins and
/// onSwitched).
#[test]
fn settings_reads_again_after_logins_change() {
    use crate::screens::settings;
    let found = FoundLogins {
        found: vec![FoundLogin {
            login: login("new-1"),
            name: "work".into(),
        }],
        ..FoundLogins::default()
    };
    let (mut m, _k, _d) = model(Fake {
        found: Mutex::new(found),
        ..Fake::default()
    });
    settings::use_store(&mut m, Store::default());
    let logins = |m: &Model| settings::config(m).logins.len();
    find_logins(&mut m);
    assert_eq!(logins(&m), 1, "closed: the one store has them");
    assert_eq!(cfg(&m).logins.len(), 1);
    settings::open_dialog(&mut m, settings::PAGE_GENERAL);
    on_logins(
        &mut m,
        FoundLogins {
            found: vec![FoundLogin {
                login: Login {
                    email: "moved@example.com".into(),
                    ..login("new-1")
                },
                name: "work".into(),
            }],
            ..FoundLogins::default()
        },
    );
    assert_eq!(logins(&m), 1, "open: the logins as saved");
    assert_eq!(settings::config(&m).logins[0].email, "moved@example.com");
    // A switch reads it again too.
    cfg_mut(&mut m).logins.push(login("new-2"));
    super::keys::login_switched(
        &mut m,
        super::keys::LoginSwitched {
            to: "new-2".into(),
            ..Default::default()
        },
    );
    assert_eq!(logins(&m), 2);
}

/// Settings and the accounts share one config (Go's one m.store): a
/// setting changed in Settings is still there after the accounts save
/// (a profile picked, a sign-in found), on screen and in config.json.
#[test]
fn an_accounts_save_keeps_settings_changes() {
    use crate::screens::settings;
    let (mut m, _k, _d) = model(Fake::default());
    settings::use_store(&mut m, Store::load_with_env(&|_| None));
    settings::config_mut(&mut m).quiet = true;
    settings::config_mut(&mut m).at_limit = "wait".into();
    settings::changed(&mut m);
    // The accounts change something of their own and save.
    cfg_mut(&mut m).logins.push(login("new-1"));
    super::state::save(&mut m);
    rush_state::flush().unwrap();
    let disk = Store::load_with_env(&|_| None).config;
    assert!(disk.quiet, "Notify off kept");
    assert_eq!(disk.at_limit, "wait");
    assert_eq!(disk.logins.len(), 1);
    assert!(cfg(&m).quiet && cfg(&m).at_limit == "wait");
}

/// config.json changed outside rush reaches Settings and the list at
/// once (onConfig: m.store.Reload, m.refresh).
#[test]
fn an_outside_change_reaches_settings() {
    use crate::screens::settings;
    let (mut m, _k, _d) = model(Fake::default());
    settings::use_store(&mut m, Store::load_with_env(&|_| None));
    settings::open_dialog(&mut m, settings::PAGE_GENERAL);
    let b = br#"{"groupBy":"agent","hideLogo":true}"#;
    super::askrush::on_config(&mut m, b);
    assert_eq!(settings::config(&m).group_by, "agent");
    assert!(settings::config(&m).hide_logo);
    assert_eq!(m.cfg.group_by, "agent", "the list's own copy too");
    assert!(m.cfg.hide_logo);
}

/// A login whose refresh fails keeps its last numbers and says why, as
/// Go's loader keeps what fetchUsage read (setFetched): an expired
/// sign-in reads "sign-in expired · r refreshes it", and r refreshes it,
/// rather than "not read since".
#[test]
fn a_failed_reading_says_why() {
    let (mut m, k, _d) = model(Fake::default());
    cfg_mut(&mut m).logins.push(login("a"));
    let hour = 3_600_000;
    *k.reading.lock().unwrap() = Reading {
        windows: quota(10.0, 20.0).windows,
        fetched_at: NOW - 5 * hour,
        ..Reading::default()
    };
    super::logins::fetch_usage(&mut m);
    let q = |m: &Model| peek(m).logins[0].quota.clone();
    assert_eq!(q(&m).windows.len(), 2);
    assert!(q(&m).problem.is_empty());
    *k.reading.lock().unwrap() = Reading {
        problem: rush_core::usage::EXPIRED.into(),
        ..Reading::default()
    };
    super::logins::fetch_usage(&mut m);
    assert_eq!(q(&m).windows.len(), 2, "the last numbers kept");
    assert_eq!(q(&m).fetched_at, NOW - 5 * hour);
    assert_eq!(q(&m).problem, rush_core::usage::EXPIRED);
    let lv = peek(&m).logins[0].clone();
    let row = &super::rows::AcctRow {
        kind: super::look::LOGINS_KIND.into(),
        q: lv.quota.clone(),
        login: Some(lv),
        ..Default::default()
    };
    assert_eq!(
        super::rows::problem(&m, row),
        "sign-in expired · r refreshes it"
    );
    let l = super::rows::limits(&m, row, 26, 26).plain();
    assert!(
        l.contains("sign-in expired · r refreshes it · last read"),
        "{l}"
    );
}

// ---- A host from before homes (logins.go:385-425, :600-657) ----

/// A rush folder under /tmp, so a session's socket path fits.
fn short_home() -> tempfile::TempDir {
    let d = tempfile::Builder::new()
        .prefix("rl-")
        .tempdir_in("/tmp")
        .unwrap();
    let p = std::fs::canonicalize(d.path()).unwrap();
    rush_state::set_thread_dir(Some(p.clone()));
    rush_state::set_thread_cache_dir(Some(p.join("cache")));
    d
}

fn spawn_fake(c: rush_host::Config) -> Result<rush_host::Config, String> {
    rush_host::fake::spawn(rush_host::fake::registry(), c).map_err(|e| e.to_string())
}

/// What a session's host did, once its events say want: up to 10 s for
/// an in-process host's threads.
fn events_once(id: &str, want: &str) -> String {
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[rush_host::dir(id)]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        rush_host::events::flush();
        let ev = std::fs::read_to_string(rush_host::events_path(id)).unwrap_or_default();
        let now = std::time::Instant::now();
        if ev.contains(want) || now >= deadline {
            return ev;
        }
        w.wait(
            None,
            Some((deadline - now).min(std::time::Duration::from_millis(50))),
        );
    }
}

/// A host from before homes can't be told it runs as another login: it's
/// stopped and started again on its conversation, and one a limit stopped
/// carries on (reloginHosts, replaceHost).
#[test]
fn a_host_from_before_homes_is_replaced() {
    let (_m, _k, _d) = model(Fake::default());
    let home = short_home();
    let c = spawn_fake(rush_host::Config {
        id: "old1".into(),
        session_id: "old1-conversation".into(),
        kind: "claude".into(),
        cwd: home.path().to_string_lossy().into_owned(),
        ..rush_host::Config::default()
    })
    .unwrap();
    // As a host from before homes wrote it, stopped by a limit.
    let mut info = rush_host::read_info(&c.id).unwrap();
    info.homes = false;
    info.limit = Some(rush_host::Limit::default());
    std::fs::write(
        rush_host::dir(&c.id).join("info.json"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();
    let got = super::logins::relogin_hosts(
        "",
        &Config::default(),
        &std::collections::BTreeMap::new(),
        spawn_fake,
    );
    assert_eq!(got, (1, 0), "carried on, none waiting");
    let ev = events_once(&c.id, "on another account now");
    assert!(ev.contains("\"kind\":\"host-replace\""), "{ev}");
    assert!(ev.contains("on another account now"), "the continue: {ev}");
    let back = rush_host::read_config(&c.id).unwrap();
    assert!(back.resume, "started again on its conversation");
    if let Ok(cl) = rush_host::Client::dial(&c.id) {
        let _ = cl.stop();
    }
    rush_state::set_thread_cache_dir(None);
    drop(home);
}

// ---- Handing on what a limit stopped (profiles.go:136) ----

/// An agent of kind .0 for hand-off tests, taking another's conversation
/// on when .1.
struct Prov(&'static str, bool);

impl Adapter for Prov {
    fn kind(&self) -> Kind {
        self.0.into()
    }
    fn name(&self) -> String {
        self.0.to_uppercase()
    }
    fn features(&self) -> Features {
        if self.1 {
            [(
                rush_core::adapter::Feature::HandoffIn,
                rush_core::adapter::Support::YES,
            )]
            .into_iter()
            .collect()
        } else {
            Features::default()
        }
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
}

#[derive(Default)]
struct Moved(Vec<String>);

/// hoa, out of room; hob after it, taking conversations on when takes;
/// a session on hoa a limit stopped; moves noted in Moved.
fn hand_off_model(takes: bool) -> (Model, Arc<Fake>, tempfile::TempDir) {
    let (mut m, k, d) = model(Fake::default());
    let reg = Registry::new();
    reg.register(Prov("hoa", false));
    reg.register(Prov("hob", takes));
    super::bridge::tell(&reg);
    m.reg = reg;
    let mut a = Agent {
        key: "s1".into(),
        kind: "hoa".into(),
        display_name: "fix the upload".into(),
        rush: true,
        ..Agent::default()
    };
    a.job.id = "s1".into();
    a.job.detail = "usage limit · resets 4pm".into();
    m.agents = vec![a];
    accounts(&mut m).hand_over = Some(|m, key, o, msg| {
        m.kept::<Moved>()
            .0
            .push(format!("{key} → {} {msg}", o.kind));
        crate::frame::Out::Draw
    });
    (m, k, d)
}

fn hand_profile() -> rush_state::Profile {
    let mut p = rush_state::Profile::default();
    p.name = "ho".into();
    p.providers = vec!["hoa".into(), "hob".into()];
    p.on_limit = rush_state::profiles::LIMIT_HANDOFF.into();
    p
}

fn hoa_out() -> rush_state::profiles::Room {
    rush_state::profiles::Room::from([(
        "hoa".to_string(),
        vec![rush_state::profiles::Seat {
            id: "x".into(),
            current: true,
            out: true,
            used: 99.0,
            ..Default::default()
        }],
    )])
}

/// Out of every account of its own, a conversation goes to the next
/// provider that can take it on; with room left on its own, or none that
/// can, it stays (handOffStopped's takers).
#[test]
fn hand_off_goes_to_a_taker_with_room() {
    let (m, _k, _d) = hand_off_model(true);
    let p = hand_profile();
    let yes = |k: &str| m.reg.supports(k, rush_core::adapter::Feature::HandoffIn);
    let to = super::handoff::hand_off_to(&p, "hoa", &hoa_out(), yes).expect("handed on");
    assert_eq!(to.kind, "hob");
    let room = rush_state::profiles::Room::new();
    assert!(
        super::handoff::hand_off_to(&p, "hoa", &room, yes).is_none(),
        "its own has room: switching comes first"
    );
    assert!(
        super::handoff::hand_off_to(&p, "hoa", &hoa_out(), |_| false).is_none(),
        "no provider can take it on"
    );
}

/// Only the rush that leads hands a conversation on, and once; one whose
/// profile waits stays (handOffStopped, handOff).
#[test]
fn only_the_lead_hands_off_and_once() {
    use super::handoff::hand_off_stopped_in;
    let (mut m, _k, _d) = hand_off_model(true);
    let room = hoa_out();
    m.follows = true;
    hand_off_stopped_in(&mut m, &room, |_, _| hand_profile());
    assert!(
        m.kept::<Moved>().0.is_empty(),
        "a rush that follows hands nothing on"
    );
    m.follows = false;
    hand_off_stopped_in(&mut m, &room, |_, _| hand_profile());
    assert_eq!(m.kept::<Moved>().0, ["s1 → hob "]);
    assert_eq!(flash(&m), "fix the upload is out of HOA · moving it to HOB");
    hand_off_stopped_in(&mut m, &room, |_, _| hand_profile());
    assert_eq!(m.kept::<Moved>().0.len(), 1, "once");

    let (mut m, _k, _d) = hand_off_model(true);
    let mut waits = hand_profile();
    waits.on_limit = rush_state::profiles::LIMIT_WAIT.into();
    hand_off_stopped_in(&mut m, &room, move |_, _| waits.clone());
    assert!(m.kept::<Moved>().0.is_empty(), "its profile waits");
}
