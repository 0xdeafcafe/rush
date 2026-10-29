//! Starting, waking and the spawned agent's messages, against a Fake
//! host's records (rush-host's fake host is the binary's, hosted.rs):
//! after Go's hosted_test.go, sleep_host_test.go and spawns_test.go's
//! send-now case. No sleeps: a model with no loop runs handed-off work at
//! once.

use std::sync::Arc;

use rush_core::adapter::{Adapter, Feature, Features, Kind, Level, Profile, Registry, Support};
use rush_core::agent::{Agent, State};
use rush_state::Store;

use super::*;
use crate::screens::accounts::askrush::ask_rush;
use crate::screens::accounts::setup::StartOver;
use crate::screens::accounts::state::set_store;
use crate::screens::session_core::host::{Fake, Info, Said, set_hosts};
use crate::screens::session_core::{conn, live, spawned};
use crate::worker::Fleet;

const NOW: rush_core::time::Ms = 1_793_190_896_789;

/// An agent rush can run, resume and wake.
struct Runs(&'static str);

impl Adapter for Runs {
    fn kind(&self) -> Kind {
        Kind::from(self.0)
    }
    fn name(&self) -> String {
        "Runs Code".into()
    }
    fn features(&self) -> Features {
        [Feature::Run, Feature::Resume, Feature::Interrupt]
            .into_iter()
            .map(|f| (f, Support::YES))
            .collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
}

fn reg() -> Registry {
    let r = Registry::new();
    r.register(Runs("runs"));
    r
}

/// A model in a rush folder of its own, starting through fake as "runs".
fn model(fake: &Arc<Fake>) -> (Model, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.reg = reg();
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = 120;
    m.h = 40;
    m.launch_dir = "/src/proj".into();
    m.ext.agents_box = true;
    set_store(&mut m, Store::default());
    set_hosts(&mut m, Arc::new(fake.clone()));
    accounts(&mut m).start_over = Some(StartOver {
        kind: "runs".into(),
        model: "big-1".into(),
        effort: "high".into(),
        mode: "plan".into(),
        mode_set: true,
        ..StartOver::default()
    });
    (m, d)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn typed(m: &mut Model, s: &str) -> Out {
    m.input.insert_str(s);
    m.submit()
}

/// The hosted agent the list has once a session's started.
fn hosted_agent(key: &str, id: &str) -> Agent {
    let mut a = Agent {
        key: key.into(),
        display_name: "fix the flaky test".into(),
        kind: "runs".into(),
        rush: true,
        pid: 4242,
        ..Agent::default()
    };
    a.job.id = id.into();
    a.job.state = State::Idle;
    a
}

// A task typed in the box starts a session as the next one starts, in
// the folder new ones start in; once the list has it, it's picked and
// open (keys.go submit, startHosted, hostStartedMsg).
#[test]
fn a_task_in_the_box_starts_a_session() {
    let fake = Fake::new();
    let (mut m, _d) = model(&fake);
    assert_eq!(typed(&mut m, "fix the flaky test"), Out::Draw);
    let starts = fake.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 1);
    let s = &starts[0];
    assert_eq!(
        (s.kind.as_str(), s.cwd.as_str(), s.prompt.as_str()),
        ("runs", "/src/proj", "fix the flaky test")
    );
    assert_eq!((s.model.as_str(), s.effort.as_str()), ("big-1", "high"));
    assert_eq!(s.permission_mode, "plan");
    assert!(s.own_worktree, "a project's session gets its own worktree");
    assert!(s.system_prompt.is_empty());
    assert!(m.input.is_empty(), "the box empties once it's started");
    assert_eq!(flash(&m), "started fix the flaky");
    assert!(
        accounts(&mut m).start_over.is_none(),
        "the sheet's pick is for one session"
    );
    // The list doesn't have it yet: nothing's opened.
    assert!(m.session.is_none());
    let key = "default/a:fake-1";
    assert_eq!(core(&mut m).select_on_load, key);
    fake.host(key);
    m.set_fleet(Fleet {
        agents: vec![hosted_agent(key, "fake-1")],
        roots: vec![String::new()],
        ..Fleet::default()
    });
    assert_eq!(m.sel, key);
    assert_eq!(m.session.as_ref().map(|v| v.key.as_str()), Some(key));
    assert!(core(&mut m).select_on_load.is_empty());
    assert!(conn(&mut m, key).hosted(), "its host's link is open");
}

// A start the host refuses says why, in red, and starts nothing.
#[test]
fn a_start_that_fails_says_why() {
    let fake = Fake::new();
    let (mut m, _d) = model(&fake);
    *fake.fail.lock().unwrap() =
        Some("Runs Code isn't installed: rush can't find its program".into());
    typed(&mut m, "do it");
    let f = m.flash.clone().unwrap();
    assert_eq!(
        f.0,
        "Runs Code isn't installed: rush can't find its program"
    );
    assert!(f.2, "a failure");
    assert!(core(&mut m).select_on_load.is_empty());
}

// With no hosts that start sessions the box says so, as before.
#[test]
fn no_hosts_start_nothing() {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    m.reg = reg();
    m.ext.agents_box = true;
    set_store(&mut m, Store::default());
    typed(&mut m, "fix it");
    assert_eq!(flash(&m), "rush can't start agents from here yet");
    assert_eq!(ask_rush(&mut m, "hi"), Out::Draw);
    assert_eq!(flash(&m), "rush can't start sessions here yet");
}

// #ask starts in rush's folder, never in a worktree, with the guide in
// its system prompt; asked nothing, it's called asking rush.
#[test]
fn ask_starts_in_rushs_folder() {
    let fake = Fake::new();
    let (mut m, d) = model(&fake);
    m.command("#ask");
    let s = fake.starts.lock().unwrap()[0].clone();
    assert_eq!(s.cwd, d.path().to_string_lossy());
    assert_eq!(s.name, "asking rush");
    assert!(!s.own_worktree);
    assert!(s.system_prompt.contains("<rush-guide>"));
    m.command("#ask how do I pin?");
    let s = fake.starts.lock().unwrap()[1].clone();
    assert_eq!((s.name.as_str(), s.prompt.as_str()), ("", "how do I pin?"));
}

// #new with a task starts it once; #with sets the next session.
#[test]
fn new_starts_once() {
    let fake = Fake::new();
    let (mut m, _d) = model(&fake);
    accounts(&mut m).start_over = None;
    crate::screens::accounts::newcmd::skip_catalogs(&mut m);
    let o = m.command("#new runs tidy the imports");
    assert_eq!(o, Out::Draw);
    let starts = fake.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 1, "{}", flash(&m));
    assert_eq!(starts[0].kind, "runs");
    assert_eq!(starts[0].prompt, "tidy the imports");
}

/// A model with the hosted session key open, connected through fake.
fn open_hosted(fake: &Arc<Fake>, key: &str) -> Model {
    let (mut m, d) = model(fake);
    std::mem::forget(d); // the rush folder lives as long as the test's thread
    fake.host(key);
    m.set_fleet(Fleet {
        agents: vec![hosted_agent(key, "h1")],
        roots: vec![String::new()],
        ..Fleet::default()
    });
    m.select(key.into());
    m.open(0);
    live::sync(&mut m);
    assert!(conn(&mut m, key).hosted());
    m
}

// A host that goes to sleep lets its socket go and the conversation stays;
// a message wakes it and goes over its new socket once (parkHost,
// wakeHost).
#[test]
fn a_sleeping_host_wakes_with_the_message() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    let r#gen = core(&mut m).link_gen;
    live::take(
        &mut m,
        key,
        r#gen,
        vec![Said::Info(Info {
            state: "idle".into(),
            sleeping: true,
            ..Info::default()
        })],
        true,
    );
    let c = conn(&mut m, key);
    assert!(!c.hosted() && c.sleeping, "parked");
    assert!(m.session.is_some(), "the conversation stays");
    m.agents[0].pid = 0; // its host has gone
    fake.asleep.lock().unwrap().push(key.into());
    conn(&mut m, key).input.insert_str("carry on");
    crate::screens::session_core::keys::send(&mut m, key, false);
    assert_eq!(
        fake.wakes.lock().unwrap().clone(),
        vec![(key.to_string(), "carry on".to_string(), false)]
    );
    let c = conn(&mut m, key);
    assert!(c.hosted() && !c.sleeping && !c.waking, "connected again");
    assert!(
        fake.resumed.lock().unwrap().is_empty(),
        "woken, not resumed"
    );
    assert_eq!(flash(&m), "waking fix the flaky test…");
}

// A wake that fails puts the message back in the empty box.
#[test]
fn a_wake_that_fails_keeps_the_message() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    let r#gen = core(&mut m).link_gen;
    let asleep = Info {
        sleeping: true,
        ..Info::default()
    };
    live::take(&mut m, key, r#gen, vec![Said::Info(asleep)], true);
    *fake.fail.lock().unwrap() = Some("session is still asleep; message was not sent".into());
    conn(&mut m, key).input.insert_str("carry on");
    crate::screens::session_core::keys::send(&mut m, key, false);
    let c = conn(&mut m, key);
    assert_eq!(c.input.string(), "carry on");
    assert!(!c.waking && c.out.sending.is_empty());
    assert_eq!(flash(&m), "session is still asleep; message was not sent");
}

// A host that stopped, not asleep, is resumed with the message instead.
#[test]
fn a_stopped_host_resumes() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    let r#gen = core(&mut m).link_gen;
    live::take(&mut m, key, r#gen, Vec::new(), true);
    assert!(!conn(&mut m, key).sleeping);
    m.agents[0].pid = 0;
    conn(&mut m, key).input.insert_str("again");
    crate::screens::session_core::keys::send(&mut m, key, false);
    assert_eq!(
        fake.resumed.lock().unwrap().clone(),
        vec![(key.to_string(), "again".to_string())]
    );
    assert_eq!(flash(&m), "resumed fix the flaky test");
}

// A model switched mid-session comes only as the host's info.
#[test]
fn the_hosts_info_switches_the_model() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    let r#gen = core(&mut m).link_gen;
    let i = Info {
        model: "big-2".into(),
        ..Info::default()
    };
    live::take(&mut m, key, r#gen, vec![Said::Info(i)], false);
    assert_eq!(m.session.as_ref().unwrap().sess.model, "big-2");
}

// From its parent's view, a spawned agent's message is queued for its
// turn's end, or sent now, stopping its turn; one whose turn can't take a
// message says how to send it now (a88d13b3).
#[test]
fn a_spawned_agent_from_its_parents_view() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    fake.not_mid_turn.lock().unwrap().push("kimi".into());
    let cases = [
        (
            "runs",
            false,
            "in its own queue · it reads it when its turn ends",
        ),
        ("runs", true, "its turn stopped for it · it reads it now"),
        (
            "kimi",
            false,
            "in its own queue · its turn can't take it · ctrl+enter stops the turn and sends it",
        ),
    ];
    for (i, (kind, now, how)) in cases.into_iter().enumerate() {
        conn(&mut m, key).input.insert_str("look at the logs");
        spawned::send(&mut m, key, "spawn:s1", "s1", kind, now);
        let sent = fake.sent_to.lock().unwrap()[i].clone();
        assert_eq!(sent, ("s1".into(), "look at the logs".into(), now));
        assert_eq!(flash(&m), format!("sent to the spawned agent · {how}"));
        assert!(conn(&mut m, key).input.is_empty());
    }
    let notes = core(&mut m).sub_notes[&super::super::subqueue::key(key, "spawn:s1")].clone();
    assert_eq!(notes.len(), 3);
    // One that doesn't go comes back to the box.
    *fake.fail.lock().unwrap() = Some("no host".into());
    conn(&mut m, key).input.insert_str("again");
    spawned::send(&mut m, key, "spawn:s1", "s1", "runs", false);
    assert_eq!(conn(&mut m, key).input.string(), "again");
    assert_eq!(
        flash(&m),
        "couldn't send to the spawned agent: no host · back in the box"
    );
}

// A session started from hosted's list is in rush's Agents; the hosted
// view stays on its own session (model.go:1057 hostStartedMsg).
#[test]
fn hosted_stays_on_its_own_session() {
    let fake = Fake::new();
    let (mut m, _d) = model(&fake);
    crate::hosted::set(&mut m, "mine");
    assert_eq!(typed(&mut m, "fix the flaky test"), Out::Draw);
    assert_eq!(fake.starts.lock().unwrap().len(), 1);
    assert_eq!(flash(&m), "started fix the flaky · it's in rush's Agents");
    assert!(core(&mut m).select_on_load.is_empty(), "nothing to pick");
}
