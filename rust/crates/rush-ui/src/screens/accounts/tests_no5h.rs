//! Accounts without a 5-hour limit, as Anthropic's top plan now is, beside
//! ones with it: the readings as rush keeps them in usage.json, read the
//! way every screen gets them (rush-claude's readings, then the logins),
//! and what the screens and the switching make of them. A temp rush
//! folder and a fake vault: nothing reads the keychain, ~/.claude or the
//! network.

use std::sync::Arc;

use rush_core::adapter::Registry;
use rush_core::time::{Ms, format_rfc3339};
use rush_core::usage::{NO_FIVE_HOUR, Quota};
use rush_state::vault::{FoundLogins, LoginKeeper};
use rush_state::{Config, Login, Store};

use super::autoswitch::auto_switch;
use super::logins::use_keeper;
use super::state::{LoginView, accounts, cfg_mut, logins, peek, set_store};
use crate::model::Model;
use crate::screens::list_extras::meters::switch_note;
use crate::screens::list_extras::plan::{InQueue, Queue, plan_summary, queue_line};
use crate::screens::settings::acct::{self as sacct, AcctRow as SRow};
use crate::style::line;

const NOW: Ms = 1_793_190_896_789;
const MIN: Ms = 60_000;
const HOUR: Ms = 60 * MIN;

/// A vault that switches and notes it.
#[derive(Default)]
struct Vault(std::sync::Mutex<Vec<String>>);

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
    fn use_login(&self, _: &Config, l: &Login) -> std::io::Result<()> {
        self.0.lock().unwrap().push(format!("use {}", l.id));
        Ok(())
    }
    fn adopt_login(&self, _: &rush_core::adapter::Profile) -> std::io::Result<Login> {
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

fn t(at: Ms) -> String {
    format_rfc3339(at)
}

/// One login's reading as rush keeps it in usage.json, Go's names and all.
fn entry(id: &str, five: Option<(bool, f64, Ms)>, week: f64, week_in: Ms) -> String {
    let (present, p, reset) = five.unwrap_or((false, 0.0, 0));
    let reset = if reset == 0 {
        "0001-01-01T00:00:00Z".to_string()
    } else {
        t(NOW + reset)
    };
    format!(
        r#""login:{id}":{{"usage":{{"AccountID":"{id}","Plan":"","FiveHour":{{"Present":{present},"Percent":{p},"ResetsAt":"{reset}"}},"SevenDay":{{"Present":true,"Percent":{week},"ResetsAt":"{}"}},"FetchedAt":"{}","Fetched":true}}}}"#,
        t(NOW + week_in),
        t(NOW - MIN),
    )
}

/// The logins with their readings, read from a usage.json of entries the
/// way the screens read them; current is the one in use.
fn read(dir: &std::path::Path, entries: &[String], current: &str) -> Vec<LoginView> {
    let path = dir.join("usage.json");
    std::fs::write(&path, format!("{{{}}}", entries.join(","))).unwrap();
    let fetched = rush_claude::usage::readings(&path);
    let mut cfg = Config::default();
    for e in entries {
        let id = e.split('"').nth(1).unwrap().trim_start_matches("login:");
        cfg.logins.push(Login {
            id: id.into(),
            name: id.into(),
            ..Login::default()
        });
    }
    logins(&cfg, current, "", &fetched, NOW)
}

/// The user's three readings: alsome with no 5-hour window at all,
/// rogerio's sent with no reset time, alex's a real one.
fn three(dir: &std::path::Path, current: &str) -> Vec<LoginView> {
    read(
        dir,
        &[
            entry("alsome", None, 66.0, 26 * HOUR),
            entry("rogerio", Some((true, 0.0, 0)), 100.0, 17 * HOUR),
            entry("alex", Some((true, 65.0, 2 * HOUR)), 40.0, 3 * HOUR),
        ],
        current,
    )
}

fn q<'a>(ls: &'a [LoginView], id: &str) -> &'a Quota {
    &ls.iter().find(|l| l.login.id == id).unwrap().quota
}

/// Where pat starts in s, in cells.
fn col(s: &str, pat: &str) -> Option<usize> {
    s.find(pat).map(|i| rush_core::text::width(&s[..i]))
}

fn row(q: &Quota) -> SRow {
    SRow {
        kind: "claude".into(),
        login: "x".into(),
        q: q.clone(),
        ..SRow::default()
    }
}

#[test]
fn readings_have_no_five_hour_window_where_there_is_no_limit() {
    let d = tempfile::tempdir().unwrap();
    let ls = three(d.path(), "alex");
    for id in ["alsome", "rogerio"] {
        assert!(q(&ls, id).window("five_hour").is_none(), "{id}");
        assert!(q(&ls, id).no_five_hour(), "{id}");
    }
    assert!(q(&ls, "alex").window("five_hour").is_some());
    assert!(!q(&ls, "alex").no_five_hour());
    // rogerio's weekly is full, and that alone makes it out; never 0%.
    assert_eq!(q(&ls, "rogerio").used(""), 100.0);
    assert_eq!(q(&ls, "alsome").used(""), 66.0);
}

/// Settings › Providers: the Account table's 5h column says there's no
/// 5-hour limit and keeps the weekly in its own column; the provider's
/// own rows say it in the 5h row; together averages the 5-hour over the
/// accounts that have one, in its place whichever came first.
#[test]
fn settings_draws_no_five_hour_meter() {
    let (m, _, d) = model();
    let ls = three(d.path(), "alsome");
    let alsome = sacct::limits(&m, &row(q(&ls, "alsome")), 26, 26).plain();
    assert!(alsome.starts_with(NO_FIVE_HOUR), "{alsome:?}");
    assert_eq!(col(&alsome, "7d"), Some(26), "{alsome:?}");
    assert!(!alsome.contains("5h"), "{alsome:?}");
    let alex = sacct::limits(&m, &row(q(&ls, "alex")), 26, 26).plain();
    assert!(
        alex.starts_with("5h") && col(&alex, "7d") == Some(26),
        "{alex:?}"
    );

    let label = |s: &str| line([crate::style::sp(format!("{s:<10}"), crate::style::DIM)]);
    let lines: Vec<String> = sacct::window_lines(q(&ls, "alsome"), NOW, &label)
        .iter()
        .map(|l| l.plain())
        .collect();
    assert_eq!(lines[0].trim_end(), format!("5h        {NO_FIVE_HOUR}"));
    assert!(
        lines[1].starts_with("7d") && lines[1].contains("66%"),
        "{lines:?}"
    );
    assert!(lines.iter().all(|l| !l.contains("0%") || l.contains("66%")));

    // together: alsome first, with no 5-hour window, then alex with one.
    let rows = [row(q(&ls, "alsome")), row(q(&ls, "alex"))];
    let (all, n) = sacct::together(&rows);
    assert_eq!(n, 2);
    let ids: Vec<&str> = all.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["five_hour", "seven_day"]);
    assert_eq!(all.windows[0].percent, 65.0, "the 5-hour of alex alone");
    assert_eq!(all.windows[1].percent, 53.0);
    let tog = sacct::limits(&m, &row(&all), 26, 26).plain();
    assert!(
        tog.starts_with("5h") && col(&tog, "7d") == Some(26),
        "{tog:?}"
    );
    // None of them with a 5-hour limit: together says so too.
    let (none, _) = sacct::together(&[row(q(&ls, "alsome")), row(q(&ls, "rogerio"))]);
    let tog = sacct::limits(&m, &row(&none), 26, 26).plain();
    assert!(tog.starts_with(NO_FIVE_HOUR), "{tog:?}");
}

/// The Accounts screen's rows, the same.
#[test]
fn accounts_screen_draws_no_five_hour_meter() {
    let (m, _, d) = model();
    let ls = three(d.path(), "alsome");
    let lv = ls.iter().find(|l| l.login.id == "alsome").unwrap().clone();
    let r = super::rows::AcctRow {
        kind: "claude".into(),
        q: lv.quota.clone(),
        login: Some(lv),
        ..Default::default()
    };
    let s = super::rows::limits(&m, &r, 26, 26).plain();
    assert!(
        s.starts_with(NO_FIVE_HOUR) && col(&s, "7d") == Some(26),
        "{s:?}"
    );
    let lines: Vec<String> = super::rows::window_lines(&r.q, NOW, &|s| {
        crate::style::sp(format!("{s:<4}"), crate::style::DIM)
    })
    .iter()
    .map(|l| l.plain())
    .collect();
    assert_eq!(lines[0], format!("5h  {NO_FIVE_HOUR}"));
}

/// The header: the plan line and the queue show the weekly alone for an
/// account without a 5-hour limit, and the low note speaks of its week.
#[test]
fn header_draws_no_five_hour_meter() {
    let d = tempfile::tempdir().unwrap();
    let ls = three(d.path(), "alsome");
    let s = plan_summary(line([crate::style::plain("✻")]), q(&ls, "alsome"), NOW).plain();
    assert!(!s.contains("5h"), "{s:?}");
    assert!(s.contains("7d 66%"), "{s:?}");
    let qu = Queue {
        kind: "claude".into(),
        glyph: "✻".into(),
        accts: rush_fleet::chain(&fleet(&ls), NOW)
            .into_iter()
            .map(|k| InQueue {
                name: k.login.login.name.clone(),
                cur: k.login.current,
                q: k.login.quota,
            })
            .collect(),
        ..Queue::default()
    };
    let l = queue_line(&qu, NOW, 200).plain();
    assert!(l.starts_with("✻ ▸ alsome ▰▰▰▰▱▱"), "{l:?}");
    // rogerio is out for its week, not for five hours: back when its week
    // resets.
    assert!(l.contains("rogerio") && l.contains("out ↻17h"), "{l:?}");
    let mut low = q(&ls, "alsome").clone();
    low.windows[0].percent = 85.0;
    let note = switch_note(&low, &[], NOW);
    assert_eq!(note, "low: 85% of 7d used");
}

fn fleet(ls: &[LoginView]) -> Vec<rush_fleet::LoginView> {
    ls.iter()
        .map(|l| rush_fleet::LoginView {
            login: l.login.clone(),
            usage: l.usage.clone(),
            quota: l.quota.clone(),
            current: l.current,
        })
        .collect()
}

/// The switch order with mixed accounts: a login without a 5-hour limit
/// never waits on one; the one whose 5-hour window is full waits for it.
#[test]
fn the_switch_order_with_mixed_accounts() {
    let (mut m, _, d) = model();
    let ls = read(
        d.path(),
        &[
            entry("alex", Some((true, 100.0, 2 * HOUR)), 40.0, 30 * HOUR),
            entry("alsome", None, 60.0, 26 * HOUR),
            entry("rogerio", Some((true, 0.0, 0)), 20.0, 100 * HOUR),
        ],
        "alex",
    );
    accounts(&mut m).logins = ls;
    let chain = (sacct::accounts().chain)(&m);
    let got: Vec<(&str, f64, Ms)> = chain
        .iter()
        .map(|k| (k.name.as_str(), k.week_percent, k.wait))
        .collect();
    assert_eq!(
        got,
        [
            ("alex", 40.0, 2 * HOUR),
            ("alsome", 60.0, 0),
            ("rogerio", 20.0, 0),
        ]
    );
}

/// Auto-switch with mixed accounts: off one whose 5-hour window is nearly
/// out to one without that limit; and one without it is never moved off
/// for five hours, nor counted out by a 5-hour window it hasn't.
#[test]
fn auto_switch_with_mixed_accounts() {
    // alex's 5 hours nearly out: to alsome, which has no 5-hour limit.
    let (mut m, v, d) = model();
    let ls = read(
        d.path(),
        &[
            entry("alex", Some((true, 99.0, 2 * HOUR)), 40.0, 30 * HOUR),
            entry("alsome", None, 30.0, 26 * HOUR),
        ],
        "alex",
    );
    for l in &ls {
        cfg_mut(&mut m).logins.push(l.login.clone());
    }
    accounts(&mut m).logins = ls;
    auto_switch(&mut m);
    assert_eq!(v.0.lock().unwrap()[0], "use alsome");

    // On alsome, its week with room: it stays, whatever the other's 5 hours.
    let (mut m, v, d) = model();
    let ls = read(
        d.path(),
        &[
            entry("alsome", None, 50.0, 100 * HOUR),
            entry("alex", Some((true, 10.0, 2 * HOUR)), 40.0, 100 * HOUR),
        ],
        "alsome",
    );
    for l in &ls {
        cfg_mut(&mut m).logins.push(l.login.clone());
    }
    accounts(&mut m).logins = ls;
    auto_switch(&mut m);
    assert!(v.0.lock().unwrap().is_empty(), "{:?}", v.0.lock().unwrap());
    assert_eq!(peek(&m).login_switched_at, 0);

    // On alsome, its week nearly out: to alex, whose 5 hours have room.
    let (mut m, v, d) = model();
    let ls = read(
        d.path(),
        &[
            entry("alsome", None, 99.0, 100 * HOUR),
            entry("alex", Some((true, 10.0, 2 * HOUR)), 40.0, 100 * HOUR),
            // rogerio: no 5-hour limit, its week full: never picked.
            entry("rogerio", Some((true, 0.0, 0)), 100.0, 17 * HOUR),
        ],
        "alsome",
    );
    for l in &ls {
        cfg_mut(&mut m).logins.push(l.login.clone());
    }
    accounts(&mut m).logins = ls;
    auto_switch(&mut m);
    assert_eq!(v.0.lock().unwrap()[0], "use alex");
}
