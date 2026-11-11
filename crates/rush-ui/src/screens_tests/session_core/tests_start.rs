//! Starting, waking and the spawned agent's messages, against a Fake
//! host's records (rush-host's fake host is the binary's, hosted.rs):
//! after Go's hosted_test.go, sleep_host_test.go and spawns_test.go's
//! send-now case. No sleeps: a model with no loop runs handed-off work at
//! once.

use super::core;
use crate::accounts::accounts;
use crate::frame::Out;
use crate::model::Model;
use std::sync::Arc;

use rush_core::adapter::{Adapter, Feature, Features, Kind, Level, Profile, Registry, Support};
use rush_core::agent::{Agent, State};
use rush_state::Store;

use crate::accounts::StartOver;
use crate::accounts::set_store;
use crate::screens::accounts::askrush::ask_rush;
use crate::screens::session_cards::ModelCards;
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
        [
            Feature::Run,
            Feature::Resume,
            Feature::Interrupt,
            Feature::Images,
        ]
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
    let mut m = crate::new_model("0.1.0");
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
    let mut m = crate::new_model("0.1.0");
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

// A host that let this rush go for falling behind, and runs on, is dialled
// again: at the next reading, and with a message sent meanwhile, never a
// second host started over it ("<id> is already running").
#[test]
fn a_host_that_let_go_and_runs_on_is_dialled_again() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    fake.running.lock().unwrap().push(key.into());
    let r#gen = core(&mut m).link_gen;
    live::take(&mut m, key, r#gen, Vec::new(), true);
    assert!(conn(&mut m, key).hosted(), "dialled again at once");
    // Before that dial is in, a message wakes it over a new connection.
    conn(&mut m, key).link = None;
    conn(&mut m, key).input.insert_str("still there?");
    crate::screens::session_core::keys::send(&mut m, key, false);
    assert_eq!(
        fake.wakes.lock().unwrap().clone(),
        vec![(key.to_string(), "still there?".to_string(), false)]
    );
    assert!(fake.resumed.lock().unwrap().is_empty(), "not resumed");
    assert!(conn(&mut m, key).hosted(), "connected again");
}

// A host that stopped, not asleep, is resumed with the message instead.
#[test]
fn a_stopped_host_resumes() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    let r#gen = core(&mut m).link_gen;
    *fake.down.lock().unwrap() = true;
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

// #compact on a stopped session starts its host with no message, and
// does what it was asked once the host has said its info, rather than
// refusing because rush isn't running it.
#[test]
fn compact_on_a_stopped_session_starts_it_first() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    let r#gen = core(&mut m).link_gen;
    m.agents[0].pid = 0;
    live::take(&mut m, key, r#gen, Vec::new(), true);
    assert!(!conn(&mut m, key).hosted());
    m.cards().host = Some(crate::screens::session_core::cards_host::host());
    crate::screens::session_cards::compact::open_compact(&mut m, key);
    assert_eq!(
        fake.resumed.lock().unwrap().clone(),
        vec![(key.to_string(), String::new())],
        "resumed with nothing sent"
    );
    assert_eq!(flash(&m), "starting fix the flaky test…");
    // Its host back, the info that ends its replay runs what was asked.
    let a = m.agents[0].clone();
    let link = crate::screens::session_core::host::Hosts::dial(&fake, &a).unwrap();
    live::opened(&mut m, key, Ok(link));
    let r#gen = core(&mut m).link_gen;
    live::take(&mut m, key, r#gen, vec![Said::Info(Info::default())], false);
    assert!(
        m.sheet_as::<crate::screens::session_cards::compact::CompactSheet>()
            .is_some(),
        "the compact sheet is open: {}",
        flash(&m)
    );
    assert!(conn(&mut m, key).on_up.is_none(), "once");
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
            "in its own queue · it goes into its turn within 2 minutes",
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
    let notes = core(&mut m).sub_notes[&super::subqueue::key(key, "spawn:s1")].clone();
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

// Rust-only: a spawned Codex takes what's sent into its turn at once, and
// says so; one it left waiting in its queue is said plainly, as a warning.
#[test]
fn a_spawned_codex_takes_it_into_its_turn() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    conn(&mut m, key)
        .input
        .insert_str("would opengl be better?");
    spawned::send(&mut m, key, "spawn:s1", "s1", "codex", false);
    assert_eq!(
        flash(&m),
        "sent to the spawned agent · into its turn now · it reads it at its next step"
    );
    *fake.sends_queue.lock().unwrap() = true;
    conn(&mut m, key).input.insert_str("or html in canvas");
    spawned::send(&mut m, key, "spawn:s1", "s1", "codex", false);
    assert_eq!(
        flash(&m),
        format!("sent to the spawned agent · {}", spawned::STEER_WAITS)
    );
    let notes = core(&mut m).sub_notes[&super::subqueue::key(key, "spawn:s1")].clone();
    assert_eq!(notes.last().unwrap().how, spawned::STEER_WAITS);
}

// Rust-only: the images a message to a spawned agent carries go as
// images, for Codex and Claude Code alike, never as words naming them.
#[test]
fn a_spawned_agent_gets_its_images() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = open_hosted(&fake, key);
    for (i, kind) in ["codex", "runs"].into_iter().enumerate() {
        let text = "look at this [Image #1]\n[image: /tmp/shot.png]".to_string();
        spawned::send_text(&mut m, key, ("spawn:s1", "s1", kind), text, false, true);
        let sent = fake.sent_to.lock().unwrap()[i].clone();
        assert_eq!(sent, ("s1".into(), "look at this [Image #1]".into(), false));
        let images = fake.sent_images.lock().unwrap()[i].clone();
        assert_eq!(images, ("s1".into(), vec!["/tmp/shot.png".to_string()]));
    }
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

// Image files dropped on the Agents box (a paste of their paths) are
// [Image #N] at once, where they were dropped; the session starts with
// them, each marker numbered as sent (model.go:1424, rushmode.go:5342).
#[test]
fn images_dropped_on_the_box_start_with_the_session() {
    let fake = Fake::new();
    let (mut m, d) = model(&fake);
    let a = d.path().join("Screen Shot 1.png");
    let b = d.path().join("b.jpg");
    for p in [&a, &b] {
        std::fs::write(p, "x").unwrap();
    }
    let (a, b) = (
        a.to_string_lossy().into_owned(),
        b.to_string_lossy().into_owned(),
    );
    m.input.insert_str("what's ");
    m.paste(&format!("{} '{b}'", a.replace(' ', "\\ ")));
    assert_eq!(m.input.string(), "what's [Image #1] [Image #2]");
    assert_eq!(m.imgs.path.len(), 2);
    assert_eq!(typed(&mut m, " here"), Out::Draw);
    let s = fake.starts.lock().unwrap()[0].clone();
    assert_eq!(s.prompt, "what's [Image #1] [Image #2] here");
    let _ = (a, b);
    assert_eq!(
        super::tests_drops::kept_names(&s.images),
        ["Screen Shot 1.png", "b.jpg"]
    );
    assert!(m.imgs.path.is_empty(), "the box lets its images go");
}

// ---- a message to a stalled session: its conversation stays on screen ----

/// What the host says of a message sent.
fn said(text: &str) -> Said {
    Said::Sent {
        text: text.into(),
        images: Vec::new(),
        exchange: None,
    }
}

/// The open session's messages, in order: each turn's, and those sent
/// into it while it worked.
fn prompts(m: &Model) -> Vec<String> {
    use rush_convo::model::Item;
    let v = m.session.as_ref().expect("the session is open");
    let mut out = Vec::new();
    for t in &v.sess.turns {
        out.push(t.prompt.clone());
        out.extend(t.items.iter().filter_map(|i| match i {
            Item::Interject(x) => Some(x.text.clone()),
            _ => None,
        }));
    }
    out
}

/// open_hosted, its host having replayed a message and gone idle.
fn talked(fake: &Arc<Fake>, key: &str) -> Model {
    let mut m = open_hosted(fake, key);
    let r#gen = core(&mut m).link_gen;
    let idle = Info {
        state: "idle".into(),
        ..Info::default()
    };
    live::take(
        &mut m,
        key,
        r#gen,
        vec![said("earlier words"), Said::Info(idle)],
        false,
    );
    assert_eq!(prompts(&m), ["earlier words"]);
    m
}

fn send(m: &mut Model, key: &str, text: &str) {
    conn(m, key).input.insert_str(text);
    crate::screens::session_core::keys::send(m, key, false);
}

// Asleep while it was watched: the woken host replays only what came since
// it woke, and that goes on after the conversation on screen
// (carryHostView), never in place of it.
#[test]
fn a_message_to_a_sleeping_host_keeps_the_conversation() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = talked(&fake, key);
    let r#gen = core(&mut m).link_gen;
    let asleep = Info {
        state: "idle".into(),
        sleeping: true,
        ..Info::default()
    };
    live::take(&mut m, key, r#gen, vec![Said::Info(asleep)], true);
    m.agents[0].pid = 0;
    fake.asleep.lock().unwrap().push(key.into());
    *fake.lines.lock().unwrap() = vec![said("carry on"), Said::Info(Info::default())];
    send(&mut m, key, "carry on");
    assert!(conn(&mut m, key).hosted(), "woken");
    assert_eq!(prompts(&m), ["earlier words", "carry on"]);
}

// Opened once it slept (its info read from disk): the same.
#[test]
fn a_message_to_a_session_opened_asleep_keeps_the_conversation() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let (mut m, d) = model(&fake);
    std::mem::forget(d);
    fake.host(key);
    fake.asleep.lock().unwrap().push(key.into());
    let asleep = Info {
        state: "idle".into(),
        sleeping: true,
        ..Info::default()
    };
    fake.saved.lock().unwrap().push((key.into(), asleep));
    let mut a = hosted_agent(key, "h1");
    a.pid = 0;
    m.set_fleet(Fleet {
        agents: vec![a],
        roots: vec![String::new()],
        ..Fleet::default()
    });
    m.select(key.into());
    m.open(0);
    live::sync(&mut m);
    assert!(conn(&mut m, key).sleeping, "read as asleep");
    let v = m.session.as_mut().unwrap();
    v.sess.sent("earlier words".into(), Vec::new(), NOW);
    *fake.lines.lock().unwrap() = vec![said("carry on"), Said::Info(Info::default())];
    send(&mut m, key, "carry on");
    assert!(conn(&mut m, key).hosted(), "woken");
    assert_eq!(prompts(&m), ["earlier words", "carry on"]);
}

// A host that let this rush go and runs on replays all it has: what's on
// screen stays until that's in, then its own conversation, nothing twice.
#[test]
fn a_message_to_a_host_that_let_go_keeps_the_conversation() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = talked(&fake, key);
    fake.running.lock().unwrap().push(key.into());
    conn(&mut m, key).link = None;
    send(&mut m, key, "still there?");
    assert!(conn(&mut m, key).hosted(), "reached again");
    assert_eq!(
        prompts(&m),
        ["earlier words"],
        "on screen till its replay's in"
    );
    let r#gen = core(&mut m).link_gen;
    let replay = vec![
        said("earlier words"),
        said("still there?"),
        Said::Info(Info::default()),
    ];
    live::take(&mut m, key, r#gen, replay, false);
    assert_eq!(prompts(&m), ["earlier words", "still there?"]);
}

/// A host down, then a message: resumed, and its new host reached once the
/// list has it. gone: the list still had its old process when it went.
fn resumed_with_a_message(gone: bool) {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = talked(&fake, key);
    let r#gen = core(&mut m).link_gen;
    *fake.down.lock().unwrap() = true;
    if !gone {
        m.agents[0].pid = 0;
    }
    live::take(&mut m, key, r#gen, Vec::new(), true);
    assert!(!conn(&mut m, key).hosted());
    m.agents[0].pid = 0;
    // Read again as the list has it: from its transcript.
    let v = m.session.as_mut().unwrap();
    v.sess.sent("earlier words".into(), Vec::new(), NOW);
    send(&mut m, key, "again");
    assert_eq!(
        fake.resumed.lock().unwrap().clone(),
        vec![(key.to_string(), "again".to_string())]
    );
    *fake.down.lock().unwrap() = false;
    m.agents[0].pid = 5151;
    live::sync(&mut m);
    assert!(conn(&mut m, key).hosted(), "its new host reached");
    assert_eq!(
        prompts(&m),
        ["earlier words"],
        "on screen till its replay's in"
    );
}

#[test]
fn a_message_to_a_stopped_host_keeps_the_conversation() {
    resumed_with_a_message(false);
}

#[test]
fn a_message_to_a_host_that_died_keeps_the_conversation() {
    resumed_with_a_message(true);
}

// One that's connected and waiting (stalled mid-turn, or idle) takes the
// message over its link: the conversation isn't touched.
#[test]
fn a_message_to_a_waiting_host_keeps_the_conversation() {
    let fake = Fake::new();
    let key = "default/a:h1";
    let mut m = talked(&fake, key);
    let r#gen = core(&mut m).link_gen;
    let stalled = Info {
        state: "working".into(),
        ..Info::default()
    };
    live::take(&mut m, key, r#gen, vec![Said::Info(stalled)], false);
    send(&mut m, key, "you there?");
    assert!(!fake.ops().is_empty(), "sent over its link");
    assert!(fake.wakes.lock().unwrap().is_empty() && fake.resumed.lock().unwrap().is_empty());
    assert_eq!(prompts(&m), ["earlier words"]);
}

/// Where the menu bar app leaves the agent a click asked this rush to go
/// to (rush-menubar's terminal.rs goto).
fn ask_goto(dir: &std::path::Path, key: &str) {
    let open = dir.join("menubar").join("open");
    std::fs::create_dir_all(&open).unwrap();
    std::fs::write(open.join(format!("{}.goto", std::process::id())), key).unwrap();
}

// A click on a notification opens its agent here, once it's in the list,
// and the ask is taken once; asked again while the list is as it was, the
// window coming back into focus opens it (snapload.go:41 msg.jump).
#[test]
fn a_click_in_the_menu_bar_opens_its_agent() {
    use rush_draw::{App, Event, Msg};
    let fake = Fake::new();
    let (mut m, d) = model(&fake);
    let (a, b) = ("default/a:fake-1", "default/a:fake-2");
    fake.host(a);
    fake.host(b);
    let fleet = Fleet {
        agents: vec![hosted_agent(a, "fake-1"), hosted_agent(b, "fake-2")],
        roots: vec![String::new(); 2],
        ..Fleet::default()
    };
    ask_goto(d.path(), b);
    m.set_fleet(fleet.clone());
    assert_eq!(m.sel, b);
    assert_eq!(m.session.as_ref().map(|v| v.key.as_str()), Some(b));
    m.session = None;
    m.set_fleet(fleet);
    assert!(m.session.is_none(), "asked once, opened once");
    ask_goto(d.path(), a);
    let _ = m.update(Msg::Input(Event::FocusGained));
    assert_eq!(m.sel, a);
    assert_eq!(m.session.as_ref().map(|v| v.key.as_str()), Some(a));
    ask_goto(d.path(), "default/a:gone");
    let _ = m.update(Msg::Input(Event::FocusGained));
    assert_eq!(m.sel, a, "an agent not in the list isn't gone to");
}

/// An agent rush can run that has modes to pick from, as Claude Code's.
struct Moded;

impl Adapter for Moded {
    fn kind(&self) -> Kind {
        Kind::from("moded")
    }
    fn name(&self) -> String {
        "Moded Code".into()
    }
    fn features(&self) -> Features {
        [(Feature::Run, Support::YES)].into_iter().collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(chooser: Chooser);
}

impl rush_core::adapter::Chooser for Moded {
    fn choices(&self) -> rush_core::adapter::Choices {
        let modes = [
            "default",
            "acceptEdits",
            "plan",
            "auto",
            "bypassPermissions",
        ]
        .iter()
        .map(|id| rush_core::adapter::Choice {
            id: (*id).into(),
            ..Default::default()
        })
        .collect();
        rush_core::adapter::Choices {
            modes,
            ..Default::default()
        }
    }
}

// The cleanup agent starts in the mode that asks, whatever new sessions
// start in (bypass, auto), even when told to start in auto: the Start its
// host's Config is made of says "default", and that it's the cleanup's.
#[test]
fn the_cleanup_agent_starts_asking_whatever_new_sessions_start_in() {
    use rush_ui_projects_machine::{CareStart, care_start_with_hosts};
    for next in ["bypassPermissions", "auto", "acceptEdits", ""] {
        for auto in [false, true] {
            let fake = Fake::new();
            let (mut m, _d) = model(&fake);
            m.reg.register(Moded);
            accounts(&mut m).start_over = Some(StartOver {
                kind: "moded".into(),
                mode: next.into(),
                mode_set: true,
                ..StartOver::default()
            });
            care_start_with_hosts(
                &mut m,
                CareStart {
                    prompt: "free some space".into(),
                    cwd: "/Users/me".into(),
                    name: "Cleanup".into(),
                    auto,
                    started_by: "cleanup".into(),
                    why: "19.7G free on disk".into(),
                    allow: vec!["Bash(du:*)".into()],
                },
            );
            let starts = fake.starts.lock().unwrap().clone();
            assert_eq!(starts.len(), 1, "{next} {auto}: {}", flash(&m));
            let s = &starts[0];
            assert_eq!(s.kind, "moded");
            assert_eq!(s.permission_mode, "default", "{next} auto={auto}");
            assert_eq!(s.started_by, "cleanup");
            assert_eq!(s.allow, ["Bash(du:*)"]);
        }
    }
}
