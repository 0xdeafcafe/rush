//! The runaway guard as the leading rush runs it: each look's readings
//! kept, sessions held, let go and moved through a fake set of hosts, new
//! sessions switched to the account picked before a session moves, and
//! what the list, the header and the queue card say. A temp rush folder,
//! fake hosts and a fake vault: nothing reaches a real session, login,
//! the keychain or the network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rush_core::guard::{Acct, Do, Hold, Level, Sample, Settings};
use rush_core::time::Ms;
use rush_host::wire::{Info, Task};
use rush_state::vault::{FoundLogins, LoginKeeper};
use rush_state::{Config, Login, Store, Time};
use serde_json::Value;

use super::guard::{self as g, HostOps, Input, Move};
use super::logins::use_keeper;
use super::look::LOGINS_KIND;
use super::state::{LoginView, accounts, cfg_mut, set_store};
use crate::model::Model;

const NOW: Ms = 1_793_190_896_789;
const MIN: Ms = 60_000;
const HOUR: Ms = 60 * MIN;

/// What happened, in order, to hosts and logins alike.
type Said = Arc<Mutex<Vec<String>>>;

#[derive(Default)]
struct Hosts {
    infos: Mutex<Vec<Info>>,
    said: Said,
    events: Mutex<Vec<(String, String, Value)>>,
    idle: Option<Ms>,
    locked: bool,
}

impl HostOps for Hosts {
    fn list(&self) -> Vec<Info> {
        self.infos.lock().unwrap().clone()
    }
    fn hold(&self, id: &str, on: bool) -> Result<(), String> {
        self.said.lock().unwrap().push(format!("hold {id} {on}"));
        for i in self.infos.lock().unwrap().iter_mut() {
            if i.id == id {
                i.queue_held = on;
            }
        }
        Ok(())
    }
    fn relogin(&self, id: &str) -> Result<(), String> {
        self.said.lock().unwrap().push(format!("relogin {id}"));
        Ok(())
    }
    fn log(&self, id: &str, kind: &str, f: Value) {
        self.events
            .lock()
            .unwrap()
            .push((id.into(), kind.into(), f));
    }
    fn notify(&self, title: &str, body: &str) {
        self.said
            .lock()
            .unwrap()
            .push(format!("notify {title}: {body}"));
    }
    fn idle(&self) -> (Option<Ms>, Option<bool>) {
        (self.idle, Some(self.locked))
    }
}

impl Hosts {
    fn said(&self) -> Vec<String> {
        self.said.lock().unwrap().clone()
    }
    fn events(&self, kind: &str) -> Vec<(String, Value)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.1 == kind)
            .map(|e| (e.0.clone(), e.2.clone()))
            .collect()
    }
}

/// A rush session on login, idle after `turns` turns.
fn info(id: &str, login: &str) -> Info {
    Info {
        id: id.into(),
        kind: LOGINS_KIND.into(),
        login: login.into(),
        state: "idle".into(),
        turn: 4,
        context_tokens: 412_000,
        ..Info::default()
    }
}

fn warm(mut i: Info, until: Ms) -> Info {
    i.cache_warm = Time::from_ms(until);
    i
}

fn working(mut i: Info) -> Info {
    i.state = "working".into();
    i
}

fn key(id: &str) -> String {
    format!("{LOGINS_KIND}:{id}")
}

/// An account with its week read at at: week% used, resetting in.
fn acct(id: &str, current: bool) -> Acct {
    Acct {
        id: key(id),
        name: id.into(),
        current,
        ..Acct::default()
    }
}

fn input(dir: &std::path::Path, s: Settings, accts: Vec<(Acct, Option<Sample>)>, now: Ms) -> Input {
    Input {
        settings: s,
        accts,
        in_use: HashMap::new(),
        others: 0,
        key_idle: 0,
        dir: dir.to_path_buf(),
        now,
        held: HashMap::new(),
        moved_at: HashMap::new(),
        over: Default::default(),
        let_through: HashMap::new(),
    }
}

fn pace_on() -> Settings {
    rush_state::config::Guard {
        pace: true,
        ..Default::default()
    }
    .settings()
}

/// Feeds the guard two hours of readings, every 10 minutes: a's week
/// going from 30% to 40% (5%/h against a fair 0.6%/h: over), b's and c's
/// standing still at 40%.
fn two_hours(dir: &std::path::Path, hosts: &Hosts, s: &Settings) -> g::Output {
    let reset = NOW + 100 * HOUR;
    let mut over = Default::default();
    let mut out = g::Output::default();
    for k in 0..=12 {
        let t = NOW - 2 * HOUR + k * 10 * MIN;
        let a = 30.0 + 10.0 * k as f64 / 12.0;
        let mut i = input(
            dir,
            s.clone(),
            vec![
                (acct("a", true), Some(Sample(t, a, reset))),
                (acct("b", false), Some(Sample(t, 40.0, reset - 80 * HOUR))),
                (acct("c", false), Some(Sample(t, 40.0, reset))),
            ],
            t,
        );
        i.over = over;
        out = g::tick(i, hosts);
        over = out.over.clone();
    }
    out
}

#[test]
fn off_unless_set() {
    let d = tempfile::tempdir().unwrap();
    let hosts = Hosts::default();
    *hosts.infos.lock().unwrap() = vec![info("s1", "a"), working(info("s2", "a"))];
    let o = two_hours(d.path(), &hosts, &Settings::default());
    assert!(hosts.said().is_empty(), "{:?}", hosts.said());
    assert!(o.plan.seats.iter().all(|(_, d)| *d == Do::Go));
    assert!(o.over.is_empty() && o.moves.is_empty());
    assert!(!d.path().join(g::SPAWNS_FILE).exists());
    // Its pace is worked out all the same, from the readings kept.
    assert!(d.path().join(rush_core::guard::FILE).exists());
    assert_eq!(o.paces[&key("a")].level, Level::Over);
    let mut c = Config::default();
    assert!(!c.guard.settings().on());
    c.guard.cap = 0;
    assert!(!serde_json::to_string(&c).unwrap().contains("guard"));
}

/// a is past its pace: a session with a warm cache is held, its event
/// saying how that was weighed; a cold one and a new one move to b, the
/// account whose room would go unused soonest; a running one finishes its
/// turn, then holds; none is cut.
#[test]
fn past_its_pace_warm_waits_cold_moves() {
    let d = tempfile::tempdir().unwrap();
    let hosts = Hosts::default();
    *hosts.infos.lock().unwrap() = vec![
        warm(info("warm", "a"), NOW + 4 * MIN),
        warm(info("cold", "a"), NOW - MIN),
        Info {
            turn: 0,
            ..info("new", "a")
        },
        working(info("busy", "a")),
        info("onb", "b"),
    ];
    let o = two_hours(d.path(), &hosts, &pace_on());
    assert_eq!(o.over.iter().collect::<Vec<_>>(), [&key("a")]);
    assert_eq!(o.plan.new_on.as_deref(), Some(key("b").as_str()));
    assert_eq!(o.plan.to_why, "b resets in 20h with 60% left");
    let said = hosts.said();
    assert!(said.contains(&"hold warm true".into()), "{said:?}");
    assert!(said.contains(&"hold busy true".into()), "{said:?}");
    assert!(
        !said.iter().any(|s| s.starts_with("relogin")),
        "moves wait for the switch"
    );
    let moved: Vec<&str> = o.moves.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(moved, ["cold", "new"]);
    assert!(o.moves.iter().all(|m| m.to == key("b") && m.cold));
    assert_eq!(o.moves[0].context, 412_000);
    let held = hosts.events("guard-held");
    let (_, warm_ev) = held.iter().find(|e| e.0 == "warm").unwrap();
    assert!(
        warm_ev["weighed"]
            .as_str()
            .unwrap()
            .contains("cache is warm: moving to b would write its 412000 tokens"),
        "{warm_ev}"
    );
    assert!(
        warm_ev["why"]
            .as_str()
            .unwrap()
            .starts_with("held: past your pace on a · ")
    );
    assert_eq!(warm_ev["cacheWarm"], true);
    // Neither held nor moved: the session already on b.
    assert!(!said.iter().any(|s| s.contains("onb")));
}

/// New sessions switch to the account picked first, then the sessions
/// move, each move in its events with what it cost.
#[test]
fn new_sessions_switch_then_sessions_move() {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.reg = rush_core::adapter::Registry::new();
    m.clock = Some(NOW);
    m.now = NOW;
    set_store(&mut m, Store::default());
    let said: Said = Default::default();
    let v = Arc::new(Vault(said.clone(), false));
    use_keeper(&mut m, v);
    for id in ["a", "b"] {
        let l = Login {
            id: id.into(),
            name: id.into(),
            ..Login::default()
        };
        cfg_mut(&mut m).logins.push(l.clone());
        accounts(&mut m).logins.push(LoginView {
            login: l,
            current: id == "a",
            ..LoginView::default()
        });
    }
    let hosts = Arc::new(Hosts {
        said: said.clone(),
        ..Hosts::default()
    });
    let mut o = g::Output::default();
    o.plan.new_on = Some(key("b"));
    o.plan.to_why = "b resets in 20h with 60% left".into();
    o.moves = vec![Move {
        id: "cold".into(),
        from: key("a"),
        to: key("b"),
        cold: true,
        context: 412_000,
        why: "its cache is cold".into(),
    }];
    g::applied(&mut m, o, hosts.clone(), NOW);
    let s = said.lock().unwrap().clone();
    let at = |p: &str| {
        s.iter()
            .position(|x| x == p)
            .unwrap_or_else(|| panic!("{p}: {s:?}"))
    };
    assert!(at("use b") < at("relogin cold"), "{s:?}");
    let ev = hosts.events("guard-moved");
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].1["context"], 412_000);
    assert_eq!(ev[0].1["cacheCold"], true);
    assert!(g::peek_state(&m).unwrap().moved_at.contains_key("cold"));
}

/// A switch that fails moves nothing.
#[test]
fn no_move_when_the_switch_fails() {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.reg = rush_core::adapter::Registry::new();
    m.clock = Some(NOW);
    m.now = NOW;
    set_store(&mut m, Store::default());
    let said: Said = Default::default();
    use_keeper(&mut m, Arc::new(Vault(said.clone(), true)));
    for id in ["a", "b"] {
        accounts(&mut m).logins.push(LoginView {
            login: Login {
                id: id.into(),
                name: id.into(),
                ..Login::default()
            },
            current: id == "a",
            ..LoginView::default()
        });
    }
    let hosts = Arc::new(Hosts {
        said: said.clone(),
        ..Hosts::default()
    });
    let mut o = g::Output::default();
    o.plan.new_on = Some(key("b"));
    o.moves = vec![Move {
        id: "cold".into(),
        from: key("a"),
        to: key("b"),
        cold: true,
        context: 1,
        why: String::new(),
    }];
    g::applied(&mut m, o, hosts.clone(), NOW);
    let s = said.lock().unwrap().clone();
    assert!(!s.iter().any(|x| x.starts_with("relogin")), "{s:?}");
    assert_eq!(hosts.events("guard-move-skipped").len(), 1);
}

#[derive(Default)]
struct Vault(Said, bool);

impl LoginKeeper for Vault {
    fn find_logins(&self, _: &Config) -> FoundLogins {
        FoundLogins::default()
    }
    fn refresh_login(&self, _: &str, _: &Config, _: &Login, _: bool) -> rush_core::usage::Reading {
        Default::default()
    }
    fn renew_login(&self, _: &Config, _: &Login) -> Result<(), String> {
        Ok(())
    }
    fn using_login(&self, _: &Config) -> String {
        String::new()
    }
    fn use_login(&self, _: &Config, l: &Login) -> std::io::Result<()> {
        if self.1 {
            return Err(std::io::Error::other("no keychain here"));
        }
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

/// The cap: past it an idle session is held and new spawns refused;
/// as agents finish, held sessions go in the order held.
#[test]
fn the_cap_queues_and_releases() {
    let d = tempfile::tempdir().unwrap();
    let hosts = Hosts::default();
    let mut busy = working(info("busy", "a"));
    // Its turn, a subagent in it and a workflow in the background: 3.
    busy.tasks = 2;
    busy.background = vec![
        Task {
            id: "w".into(),
            kind: "local_workflow".into(),
            ..Task::default()
        },
        Task {
            id: "sh".into(),
            kind: "local_bash".into(),
            ..Task::default()
        },
    ];
    assert_eq!(
        g::agents_of(&busy),
        2,
        "its turn and the workflow; the shell doesn't count"
    );
    busy.tasks = 3;
    assert_eq!(g::agents_of(&busy), 3, "and a subagent in its turn");
    let mut queued = info("waits", "a");
    queued.queue = vec!["next".into()];
    *hosts.infos.lock().unwrap() = vec![busy.clone(), queued.clone()];
    let s = Settings {
        cap: 3,
        ..Settings::default()
    };
    let o = g::tick(input(d.path(), s.clone(), vec![], NOW), &hosts);
    assert_eq!((o.plan.working, o.plan.cap), (3, 3));
    assert_eq!(hosts.said(), ["hold waits true"]);
    assert!(
        std::fs::read_to_string(d.path().join(g::SPAWNS_FILE))
            .unwrap()
            .contains("3 of 3 agents are working")
    );
    // busy finishes: waits goes, and spawns are allowed again.
    busy.state = "idle".into();
    busy.tasks = 0;
    busy.background.clear();
    *hosts.infos.lock().unwrap() = vec![
        busy,
        Info {
            queue_held: true,
            ..queued
        },
    ];
    let mut i = input(d.path(), s, vec![], NOW + MIN);
    i.held = o.held;
    let o = g::tick(i, &hosts);
    assert!(o.held.is_empty());
    assert_eq!(hosts.said().last().unwrap(), "hold waits false");
    assert!(!d.path().join(g::SPAWNS_FILE).exists());
    let rel = hosts.events("guard-released");
    assert_eq!(rel[0].1["why"], "no longer cap");
}

/// Away: no key in rush nor on the machine for the time set, or the
/// screen locked; each session holds after its turn, a #away one carries
/// on; back, they're let go.
#[test]
fn away_tightens_and_relaxes() {
    let d = tempfile::tempdir().unwrap();
    let s = rush_state::config::Guard {
        away: true,
        ..Default::default()
    }
    .settings();
    let mut looping = info("loop", "a");
    looping.away = Some(Default::default());
    let infos = [working(info("w", "a")), info("idle", "a"), looping];
    let look = |idle: Option<Ms>, locked: bool, key_idle: Ms, held| {
        let hosts = Hosts {
            idle,
            locked,
            ..Hosts::default()
        };
        let held: HashMap<String, g::Held> = held;
        *hosts.infos.lock().unwrap() = infos
            .iter()
            .map(|x| Info {
                queue_held: held.contains_key(&x.id),
                ..x.clone()
            })
            .collect();
        let mut i = input(d.path(), s.clone(), vec![], NOW);
        i.key_idle = key_idle;
        i.held = held;
        let o = g::tick(i, &hosts);
        (o, hosts.said())
    };
    // Typing elsewhere on the Mac: not away.
    let (o, said) = look(Some(MIN), false, 30 * MIN, HashMap::new());
    assert!(!o.away && said.is_empty());
    // Nothing anywhere for 30 minutes: away.
    let (o, said) = look(Some(30 * MIN), false, 30 * MIN, HashMap::new());
    assert!(o.away);
    assert_eq!(said, ["hold w true", "hold idle true"]);
    assert_eq!(o.plan.cap, 2);
    assert!(o.plan.spawns.as_deref().unwrap().contains("away"));
    // The screen locked: away at once.
    let (o2, _) = look(Some(MIN), true, MIN, HashMap::new());
    assert!(o2.away);
    // Back.
    let held = o.held.clone();
    let (o, said) = look(Some(0), false, 0, held);
    assert!(!o.away && o.held.is_empty());
    assert_eq!(said, ["hold w false", "hold idle false"]);
}

/// alt+h on a held session lets its next turn through: the guard notes
/// it, doesn't hold it again until that turn has run.
#[test]
fn a_held_turn_let_through() {
    let d = tempfile::tempdir().unwrap();
    let s = Settings {
        cap: 1,
        ..Settings::default()
    };
    let hosts = Hosts::default();
    *hosts.infos.lock().unwrap() = vec![working(info("w", "a")), info("h", "a")];
    let o = g::tick(input(d.path(), s.clone(), vec![], NOW), &hosts);
    assert!(o.held.contains_key("h"));
    // You let it go (alt+h): its queue isn't held any more.
    hosts.infos.lock().unwrap()[1].queue_held = false;
    let mut i = input(d.path(), s.clone(), vec![], NOW + MIN);
    i.held = o.held;
    let o = g::tick(i, &hosts);
    assert!(!o.held.contains_key("h"));
    assert_eq!(
        hosts.events("guard-released")[0].1["why"],
        "you let it through"
    );
    assert_eq!(hosts.said(), ["hold h true"], "not held again");
    // Its turn runs, then ends: the guard holds it again at the cap.
    hosts.infos.lock().unwrap()[1].state = "working".into();
    let mut i = input(d.path(), s.clone(), vec![], NOW + 2 * MIN);
    i.let_through = o.let_through;
    let o = g::tick(i, &hosts);
    assert_eq!(o.let_through.get("h"), Some(&true));
    hosts.infos.lock().unwrap()[1].state = "idle".into();
    let mut i = input(d.path(), s, vec![], NOW + 3 * MIN);
    i.let_through = o.let_through;
    let o = g::tick(i, &hosts);
    assert!(o.let_through.is_empty());
    assert!(o.held.contains_key("h"));
}

/// The list, the header and the queue card say why a turn is held.
#[test]
fn held_said_in_the_list_and_header() {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.now = NOW;
    set_store(&mut m, Store::default());
    cfg_mut(&mut m).guard.pace = true;
    let st = m.kept::<g::State>();
    st.held.insert(
        "s1".into(),
        g::Held {
            hold: Hold::Pace {
                acct: "alex".into(),
                ratio: 2.1,
                frees_at: 0,
                why: String::new(),
            },
            since: NOW,
        },
    );
    let mut a = rush_core::agent::Agent {
        rush: true,
        ..Default::default()
    };
    a.job.id = "s1".into();
    assert_eq!(
        g::row_note(&m, &a).as_deref(),
        Some("held: past your pace on alex · 2.1×")
    );
    a.job.state = rush_core::agent::State::Working;
    assert_eq!(g::row_note(&m, &a), None, "its turn runs: it's held after");
    let h = g::header_line(&m).unwrap().plain();
    assert_eq!(
        h,
        "⏸ 1 held · past your pace on alex · 2.1× · alt+h in a session lets it through"
    );
    // The guard off: nothing said.
    cfg_mut(&mut m).guard.pace = false;
    assert!(g::header_line(&m).is_none());
}
