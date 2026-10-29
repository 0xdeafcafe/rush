//! The Session's tests, after Go's internal/ui/sending_test.go,
//! sendmode_test.go, subqueue_test.go (TestSubQueue) and hosted_test.go:
//! the box's keys against a Fake host's recorded requests, sending's
//! arrival rules, the local queue's timings, and the dock's frames. No
//! sleeps: a model with no loop runs handed-off work at once.

use std::sync::Arc;

use rush_convo::Session;
use rush_core::adapter::{
    self, Adapter, Feature, Features, Kind, Level, Profile, Registry, Support,
};
use rush_core::agent::{Agent, State};
use rush_core::event::{Approval, Event, TurnEnd};
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_core::tool::Call;

use super::host::{Fake, Info, Op, Said, set_hosts};
use super::queue::LocalQueue;
use super::sending::Outbox;
use super::sendmode::{self, BoxState, SendMode};
use super::subqueue::{self, Due};
use super::{conn, core, live};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::session::{SessionView, Source};

pub(super) const NOW: Ms = 1_793_190_896_789;
pub(super) const KEY: &str = "default/k1";

/// An agent kind that can be guided and stopped mid-turn and rewound, as
/// Go's Claude Code can.
struct Steer;

impl Adapter for Steer {
    fn kind(&self) -> Kind {
        Kind::from("steer")
    }
    fn name(&self) -> String {
        "Steer Code".into()
    }
    fn features(&self) -> Features {
        [Feature::Guide, Feature::Interrupt, Feature::Rewind]
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

/// One that can't: only queue and discuss.
struct Plain;

impl Adapter for Plain {
    fn kind(&self) -> Kind {
        Kind::from("plain")
    }
    fn name(&self) -> String {
        "Plain Code".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
}

fn registry() -> Registry {
    let r = Registry::new();
    r.register(Steer);
    r.register(Plain);
    r
}

pub(super) fn agent(kind: &str, state: State) -> Agent {
    let mut a = Agent {
        key: KEY.into(),
        display_name: "tidy the parser".into(),
        repo: "/src/checkout".into(),
        branch: "main".into(),
        pid: 4242,
        kind: kind.into(),
        ..Agent::default()
    };
    a.job.id = "k1".into();
    a.job.state = state;
    a.job.cwd = "/src/checkout".into();
    a
}

/// A model with a's conversation open, w×h, sending through fake.
pub(super) fn open(a: Agent, fake: &Arc<Fake>, w: usize, h: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.now = NOW;
    m.reg = registry();
    m.w = w;
    m.h = h;
    let src = Source::History {
        adapter: m.reg.get(a.kind.as_str()).expect("registered"),
        session: Box::new(adapter::Session::default()),
    };
    let mut v = SessionView::new(a.key.clone(), src, 1);
    v.read = true;
    m.sel = a.key.clone();
    m.agents = vec![a];
    m.session = Some(v);
    set_hosts(&mut m, Arc::new(fake.clone()));
    m
}

/// A session rush hosts, connected, with lines from its host taken in.
pub(super) fn hosted(kind: &str, lines: Vec<Said>) -> (Model, Arc<Fake>) {
    let fake = Fake::new();
    fake.host(KEY);
    *fake.lines.lock().unwrap() = lines;
    let mut m = open(agent(kind, State::Working), &fake, 120, 40);
    live::sync(&mut m);
    assert!(conn(&mut m, KEY).hosted(), "the link is open");
    (m, fake)
}

/// One that rush doesn't run, reached through its agent's daemon.
fn outside(state: State) -> (Model, Arc<Fake>) {
    let fake = Fake::new();
    let mut a = agent("plain", state);
    a.interactive = false;
    let m = open(a, &fake, 120, 40);
    (m, fake)
}

pub(super) fn sess(m: &mut Model) -> &mut Session {
    &mut m.session.as_mut().unwrap().sess
}

/// A turn under way: you said text and it's working on it.
fn working(m: &mut Model, text: &str) {
    let now = m.now;
    sess(m).sent(text.into(), Vec::new(), now);
    assert!(sess(m).live().is_some());
}

pub(super) fn press(m: &mut Model, s: &str) -> Out {
    let p = Press::of(s).unwrap_or_else(|| panic!("no key {s:?}"));
    m.press_key(&p)
}

pub(super) fn typed(m: &mut Model, text: &str) {
    for c in text.chars() {
        let k = if c == ' ' {
            "space".to_string()
        } else {
            c.to_string()
        };
        press(m, &k);
    }
}

fn input(m: &mut Model) -> String {
    conn(m, KEY).input.string()
}

pub(super) fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

/// What rush said last, and of what kind: news never takes the line
/// under the box.
pub(super) fn said(m: &Model) -> (crate::frame::messages::SayKind, String) {
    let l = m.said.last().expect("rush said something");
    (l.kind, l.text.clone())
}

fn send(text: &str, now: bool) -> Op {
    Op::Send {
        text: text.into(),
        images: Vec::new(),
        now,
    }
}

pub(super) fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

// ---- sendmode.go, sendmode_test.go ----

/// ctrl+t goes round queue, guide, stop & send and discuss, each session
/// keeping its own, and the box's border says what enter does
/// (TestSendModeCycles).
#[test]
fn send_mode_cycles() {
    let (mut m, _) = hosted("steer", Vec::new());
    let mut saw = Vec::new();
    for _ in 0..4 {
        press(&mut m, "ctrl+t");
        saw.push(super::keys::mode_of(&mut m, KEY).name());
    }
    assert_eq!(saw.join(","), "guide,stop & send,discuss,queue");
    press(&mut m, "ctrl+t");
    assert_eq!(
        flash(&m),
        "enter guides: it reads it at its next step while it works"
    );
    working(&mut m, "go");
    typed(&mut m, "hi");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("coalescing · ctrl+t stop & send"), "{f}");
    // Another session keeps its own.
    assert!(!core(&mut m).send_modes.contains_key("default/other"));
}

#[test]
fn send_modes_follow_what_the_agent_can() {
    let reg = registry();
    let all = [
        SendMode::Queue,
        SendMode::Guide,
        SendMode::Stop,
        SendMode::Discuss,
    ];
    assert_eq!(sendmode::modes(&reg, "steer"), all);
    assert_eq!(
        sendmode::modes(&reg, "plain"),
        [SendMode::Queue, SendMode::Discuss]
    );
    let few = sendmode::modes(&reg, "plain");
    // A pick the agent can't do is queue.
    assert_eq!(
        sendmode::mode_of(Some(SendMode::Guide), &few),
        SendMode::Queue
    );
    assert_eq!(
        sendmode::next_mode(Some(SendMode::Guide), &few),
        SendMode::Discuss
    );
    assert_eq!(sendmode::next_mode(None, &few), SendMode::Discuss);
    assert_eq!(
        sendmode::next_mode(Some(SendMode::Discuss), &few),
        SendMode::Queue
    );
    assert_eq!(
        sendmode::cycled(SendMode::Discuss),
        "enter opens a room on what's typed: its verdict comes back here"
    );
    assert_eq!(
        sendmode::cycled(SendMode::Stop),
        "enter stops it and sends while it works"
    );
}

/// boxKeys: what the hint row offers, state by state.
#[test]
fn box_keys_table() {
    let st = |live, working, typed, queued, picked, mode, cleared| BoxState {
        live,
        working,
        typed,
        queued,
        picked,
        mode,
        cleared,
    };
    use SendMode::*;
    let cases: &[(BoxState, &[(&str, &str)])] = &[
        (st(false, false, false, false, None, Queue, false), &[]),
        (
            st(false, false, true, false, None, Queue, false),
            &[("enter", "send")],
        ),
        (
            st(false, false, false, true, None, Queue, false),
            &[("ctrl+enter", "send the queue now")],
        ),
        (
            st(false, true, true, false, None, Queue, false),
            &[
                ("enter", "queue it"),
                ("ctrl+enter", "send now"),
                ("ctrl+x", "stop it"),
            ],
        ),
        (
            st(true, true, true, false, Some(Guide), Guide, false),
            &[
                ("enter", "guide it"),
                ("ctrl+enter", "send now"),
                ("ctrl+t", "queue · guide · stop · discuss"),
                ("ctrl+x", "stop it"),
            ],
        ),
        (
            st(true, true, true, false, Some(Stop), Stop, false),
            &[
                ("enter", "stop & send"),
                ("ctrl+enter", "send now"),
                ("ctrl+t", "queue · guide · stop · discuss"),
                ("ctrl+x", "stop it"),
            ],
        ),
        (
            st(true, true, true, false, Some(Discuss), Discuss, false),
            &[
                ("enter", "discuss it in a room"),
                ("ctrl+t", "queue · guide · stop · discuss"),
                ("ctrl+x", "stop it"),
            ],
        ),
        (
            st(false, false, false, false, None, Queue, true),
            &[("cmd+z or ctrl+/", "bring back what you cleared")],
        ),
    ];
    for (i, (s, want)) in cases.iter().enumerate() {
        assert_eq!(sendmode::box_keys(s), *want, "case {i}");
    }
}

// ---- the box's keys (paneKey, sendPane) against the host's requests ----

/// enter sends; while it works it queues, guides or stops it and sends,
/// as picked; ctrl+enter always sends now.
#[test]
fn the_ways_enter_sends() {
    let (mut m, fake) = hosted("steer", Vec::new());
    // Idle: enter sends.
    typed(&mut m, "hello there");
    press(&mut m, "enter");
    assert_eq!(input(&mut m), "");
    assert_eq!(flash(&m), "sent");
    assert_eq!(fake.ops(), [send("hello there", false)]);

    working(&mut m, "hello there");
    let cases: &[(Option<&str>, &str, Op)] = &[
        (None, "enter", send("one", false)),
        (None, "ctrl+enter", send("one", true)),
        (None, "ctrl+s", send("one", true)),
        (
            Some("ctrl+t"),
            "enter",
            Op::Guide {
                text: "one".into(),
                images: Vec::new(),
            },
        ),
        (Some("ctrl+t"), "enter", send("one", true)),
    ];
    for (i, (mode, key, want)) in cases.iter().enumerate() {
        if let Some(k) = mode {
            press(&mut m, k);
        }
        fake.ops.lock().unwrap().clear();
        typed(&mut m, "one");
        press(&mut m, key);
        assert_eq!(fake.ops(), std::slice::from_ref(want), "case {i}");
        assert_eq!(input(&mut m), "", "case {i}");
    }
    // Discuss: enter doesn't send.
    press(&mut m, "ctrl+t");
    fake.ops.lock().unwrap().clear();
    typed(&mut m, "one");
    press(&mut m, "enter");
    assert!(fake.ops().is_empty());
    assert_eq!(input(&mut m), "one");
    // An empty box: enter does nothing.
    let (mut m, fake) = hosted("steer", Vec::new());
    press(&mut m, "enter");
    assert!(fake.ops().is_empty());
}

/// A trailing backslash goes on to a new line instead of sending.
#[test]
fn a_backslash_goes_on_to_a_new_line() {
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "one\\");
    press(&mut m, "enter");
    assert_eq!(input(&mut m), "one\n");
    assert!(fake.ops().is_empty());
    typed(&mut m, "two");
    press(&mut m, "enter");
    assert_eq!(fake.ops(), [send("one\ntwo", false)]);
}

/// esc: the box's text first (kept to undo), then the turn under way,
/// then back to the list.
#[test]
fn esc_order() {
    let (mut m, fake) = hosted("steer", Vec::new());
    working(&mut m, "go");
    typed(&mut m, "draft");
    press(&mut m, "esc");
    assert_eq!(input(&mut m), "");
    assert_eq!(flash(&m), "cleared · cmd+z or ctrl+/ brings it back");
    assert!(fake.ops().is_empty());
    assert!(m.session.is_some());
    press(&mut m, "esc");
    assert_eq!(fake.ops(), [Op::Interrupt]);
    assert_eq!(
        flash(&m),
        "stopping the turn · esc again to close, restart or switch it"
    );
    // Asked once: esc again doesn't ask again, it leaves.
    press(&mut m, "esc");
    assert_eq!(fake.ops(), [Op::Interrupt]);
    assert!(m.session.is_none(), "back to the list");
}

/// An agent that can't be stopped mid-turn: esc on an empty box leaves.
#[test]
fn esc_leaves_when_it_cant_stop() {
    let (mut m, fake) = hosted("plain", Vec::new());
    working(&mut m, "go");
    press(&mut m, "esc");
    assert!(fake.ops().is_empty());
    assert!(m.session.is_none());
}

/// ctrl+c wipes the box like esc; with nothing typed it's the frame's.
#[test]
fn ctrl_c_wipes_the_box() {
    let (mut m, _) = hosted("steer", Vec::new());
    typed(&mut m, "draft");
    press(&mut m, "ctrl+c");
    assert_eq!(input(&mut m), "");
    assert!(conn(&mut m, KEY).cleared_at > 0);
}

fn approval(id: &str) -> Event {
    Event::Approval(Box::new(Approval {
        id: id.into(),
        call: Call {
            id: "c1".into(),
            name: "Bash".into(),
            ..Call::default()
        },
        ..Approval::default()
    }))
}

/// y · a · n answer an approval with the box empty; typed words and
/// enter deny with them; a picked row's keys aren't answers.
#[test]
fn approval_keys() {
    let allow = |always| Op::Allow {
        id: "r1".into(),
        always,
    };
    let deny = |why: &str| Op::Deny {
        id: "r1".into(),
        message: why.into(),
    };
    let cases: &[(&str, Op)] = &[
        ("y", allow(false)),
        ("a", allow(true)),
        ("n", deny("")),
        ("alt+y", allow(false)),
        ("alt+a", allow(true)),
        ("alt+n", deny("")),
    ];
    for (k, want) in cases {
        let (mut m, fake) = hosted("steer", vec![Said::Event(approval("r1"))]);
        assert_eq!(sess(&mut m).pending().len(), 1);
        press(&mut m, k);
        assert_eq!(fake.ops(), std::slice::from_ref(want), "{k}");
        assert_eq!(input(&mut m), "", "{k}");
    }
    // Typed: the letters after the first are text, enter denies with
    // them.
    let (mut m, fake) = hosted("steer", vec![Said::Event(approval("r1"))]);
    typed(&mut m, "wait any");
    assert!(fake.ops().is_empty());
    press(&mut m, "enter");
    assert_eq!(fake.ops(), [deny("wait any")]);
    assert_eq!(input(&mut m), "");
    // A row picked: y returns to the box and types.
    let (mut m, fake) = hosted("steer", vec![Said::Event(approval("r1"))]);
    m.session.as_mut().unwrap().sel = "t0".into();
    press(&mut m, "y");
    assert!(fake.ops().is_empty());
    assert_eq!(input(&mut m), "y");
    // Answered: the keys are text again.
    let (mut m, fake) = hosted(
        "steer",
        vec![Said::Event(approval("r1")), Said::Answered("r1".into())],
    );
    press(&mut m, "y");
    assert!(fake.ops().is_empty());
    assert_eq!(input(&mut m), "y");
}

/// The box says how to answer while an approval waits.
#[test]
fn the_box_says_how_to_answer() {
    let (mut m, _) = hosted("steer", vec![Said::Event(approval("r1"))]);
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("y · a · n answer the card · or type why not, enter denies with it"),
        "{f}"
    );
}

/// A failed send stops showing as sending, and says why.
#[test]
fn a_failed_send_stops_showing() {
    let (mut m, fake) = hosted("steer", Vec::new());
    *fake.fail.lock().unwrap() = Some("host gone".into());
    typed(&mut m, "hi");
    press(&mut m, "enter");
    assert!(conn(&mut m, KEY).out.sending.is_empty());
    assert_eq!(flash(&m), "host gone");
}

// ---- sending.go, sending_test.go ----

fn lines(o: &Outbox, s: &Session, now: Ms) -> Vec<String> {
    let mut o2 = o.clone();
    o2.settle(s, 0, now);
    o2.lines(0, 80).iter().map(Line::plain).collect()
}

/// A message sent shows as sending until the agent has it, each of
/// several in turn; a send that fails stops showing
/// (TestSendingUntilItArrives).
#[test]
fn sending_until_it_arrives() {
    let mut s = Session::new();
    let mut o = Outbox::default();
    o.mark(&s, 0, "first", NOW);
    o.mark(&s, 0, "second", NOW);
    let got = lines(&o, &s, NOW).join("\n");
    assert!(
        got.contains("sending  first") && got.contains("second"),
        "{got}"
    );
    s.sent("first".into(), Vec::new(), NOW);
    o.settle(&s, 0, NOW);
    let l = lines(&o, &s, NOW);
    assert!(l.len() == 1 && l[0].contains("second"), "{l:?}");
    // Mid-turn, it arrives as an interjection.
    s.sent("second".into(), Vec::new(), NOW);
    o.settle(&s, 0, NOW);
    assert!(o.sending.is_empty());
    o.mark(&s, 0, "third", NOW);
    o.failed();
    assert!(o.sending.is_empty(), "a failed send still shows as sending");
}

/// Seen in a conversation holding fewer of yours than when it was sent
/// (TestSendingSeenInShorterConversation).
#[test]
fn sending_seen_in_shorter_conversation() {
    let mut s = Session::new();
    let mut o = Outbox::default();
    s.sent("old".into(), Vec::new(), NOW);
    s.sent("older".into(), Vec::new(), NOW);
    o.mark(&s, 0, "does exist??", NOW);
    let mut s = Session::new();
    s.sent("does exist??".into(), Vec::new(), NOW);
    o.settle(&s, 0, NOW);
    assert!(o.sending.is_empty());
}

/// The same text sent twice waits for both (TestSendingSameTextTwice).
#[test]
fn sending_same_text_twice() {
    let mut s = Session::new();
    let mut o = Outbox::default();
    s.sent("old".into(), Vec::new(), NOW);
    o.mark(&s, 0, "yes", NOW);
    o.mark(&s, 0, "yes", NOW);
    let mut s = Session::new();
    s.sent("yes".into(), Vec::new(), NOW);
    o.settle(&s, 0, NOW);
    assert_eq!(o.sending.len(), 1, "after one of two arrived");
    s.sent("yes".into(), Vec::new(), NOW);
    o.settle(&s, 0, NOW);
    assert!(o.sending.is_empty(), "after both arrived");
}

/// A message in the agent's queue has arrived; one waited on past 90 s
/// stops showing.
#[test]
fn sending_queued_or_too_long() {
    let s = Session::new();
    let mut o = Outbox::default();
    o.mark(&s, 0, "queued", NOW);
    o.settle(&s, 1, NOW);
    assert!(o.sending.is_empty(), "in its queue");
    o.mark(&s, 0, "lost", NOW);
    o.settle(&s, 0, NOW + 90_000);
    assert_eq!(o.sending.len(), 1);
    o.settle(&s, 0, NOW + 90_001);
    assert!(o.sending.is_empty(), "waited on too long");
}

/// Sending shows under the conversation, and goes once the host says
/// it's sent.
#[test]
fn sending_shows_in_the_dock() {
    let (mut m, _) = hosted("steer", Vec::new());
    typed(&mut m, "look at the parser");
    press(&mut m, "enter");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("sending  look at the parser"), "{f}");
    let r#gen = conn(&mut m, KEY).link_gen;
    live::take(
        &mut m,
        KEY,
        r#gen,
        vec![Said::Sent {
            text: "look at the parser".into(),
            images: Vec::new(),
        }],
        false,
    );
    let f = screen(&mut m).join("\n");
    assert!(!f.contains("sending"), "{f}");
    assert!(f.contains("look at the parser"), "{f}");
}

// ---- localqueue.go: the queue of a session rush doesn't run ----

fn replies(f: &Fake) -> Vec<String> {
    f.replies
        .lock()
        .unwrap()
        .iter()
        .map(|(_, t)| t.clone())
        .collect()
}

fn local(m: &mut Model) -> LocalQueue {
    core(m).local_q.get(KEY).cloned().unwrap_or_default()
}

fn flush_at(m: &mut Model, t: Ms) {
    m.now = t;
    super::queue::flush(m);
}

/// Working, a message waits in rush's queue, and the queue goes as one
/// message 15 s after the first was queued, or as soon as it's idle.
#[test]
fn the_local_queue_waits_15s_while_it_works() {
    let (mut m, fake) = outside(State::Working);
    typed(&mut m, "one");
    press(&mut m, "enter");
    assert_eq!(
        flash(&m),
        "queued · 1 waiting · goes to tidy the parser within 15s"
    );
    m.now = NOW + 5_000;
    typed(&mut m, "two");
    press(&mut m, "enter");
    assert_eq!(local(&mut m).items, ["one", "two"]);
    flush_at(&mut m, NOW + 14_999);
    assert!(replies(&fake).is_empty());
    flush_at(&mut m, NOW + 15_000);
    assert_eq!(
        replies(&fake),
        [super::host::join_queue(&["one".into(), "two".into()])]
    );
    assert!(local(&mut m).items.is_empty());
    let (kind, text) = said(&m);
    assert_eq!(text, "queued messages sent: tidy the parser has them");
    assert_eq!(kind, crate::frame::messages::SayKind::News, "noted");
    // Gone idle: the next goes at once, once 10 s have passed since the last.
    super::queue::queue_local(&mut m, KEY, "three", NOW + 16_000);
    m.agents[0].job.state = State::Idle;
    flush_at(&mut m, NOW + 20_000);
    assert_eq!(replies(&fake).len(), 1, "10 s since the last went");
    flush_at(&mut m, NOW + 25_000);
    assert_eq!(replies(&fake).len(), 2);
    assert_eq!(replies(&fake)[1], "three");
}

/// Waiting on you, it holds; held by you, it holds.
#[test]
fn the_local_queue_holds() {
    let (mut m, fake) = outside(State::Blocked);
    super::queue::queue_local(&mut m, KEY, "one", NOW);
    flush_at(&mut m, NOW + 60_000);
    assert!(replies(&fake).is_empty(), "waiting on you");
    m.agents[0].job.state = State::Idle;
    press(&mut m, "alt+h");
    assert!(local(&mut m).held);
    flush_at(&mut m, NOW + 60_000);
    assert!(replies(&fake).is_empty(), "held");
    press(&mut m, "alt+h");
    flush_at(&mut m, NOW + 60_000);
    assert_eq!(replies(&fake), ["one"]);
}

/// A send that fails goes back on the front, is tried again in 30 s, and
/// is held once it has failed three times running.
#[test]
fn the_local_queue_tries_three_times() {
    let (mut m, fake) = outside(State::Idle);
    super::queue::queue_local(&mut m, KEY, "one", NOW);
    let fail = |f: &Fake| *f.fail.lock().unwrap() = Some("daemon down".into());
    fail(&fake);
    flush_at(&mut m, NOW + 10_000);
    let q = local(&mut m);
    assert_eq!(q.items, ["one"]);
    assert_eq!((q.fails, q.retry), (1, NOW + 40_000));
    assert_eq!(
        said(&m).1,
        "queued messages for tidy the parser were not sent: daemon down · trying again in 30s"
    );
    // Queued meanwhile: after the one that failed.
    super::queue::queue_local(&mut m, KEY, "two", NOW + 12_000);
    fail(&fake);
    flush_at(&mut m, NOW + 39_999);
    assert_eq!(local(&mut m).fails, 1, "not before 30 s");
    flush_at(&mut m, NOW + 40_000);
    let q = local(&mut m);
    assert_eq!(q.items, ["one", "two"]);
    assert_eq!(q.fails, 2);
    fail(&fake);
    flush_at(&mut m, NOW + 70_000);
    let q = local(&mut m);
    assert!(q.held, "held after three");
    assert_eq!(q.fails, 0);
    assert_eq!(
        said(&m).1,
        rush_convo::render::key_word(
            "queued messages for tidy the parser are held: sending failed three times, daemon down · alt+h releases them"
        )
    );
    flush_at(&mut m, NOW + 200_000);
    assert!(replies(&fake).is_empty());
    // Let go: it goes.
    press(&mut m, "alt+h");
    flush_at(&mut m, NOW + 200_000);
    assert_eq!(
        replies(&fake),
        [super::host::join_queue(&["one".into(), "two".into()])]
    );
}

/// The queue card says when it goes.
#[test]
fn the_local_queue_card() {
    let (mut m, _) = outside(State::Working);
    typed(&mut m, "one");
    press(&mut m, "enter");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("▌ queue ·1  one"), "{f}");
    assert!(
        f.contains("↑ edit, reorder or steer  ·  ctrl+enter send now"),
        "{f}"
    );
    assert!(f.contains("╭─ queuing ─"), "{f}");
    // Something typed: the keys give way to when it goes.
    typed(&mut m, "x");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("sends within 15s, or when it's idle"), "{f}");
    assert!(f.contains("╭─ queuing · ctrl+enter sends it now ─"), "{f}");
}

/// A session open in a terminal can't take messages.
#[test]
fn interactive_sessions_cant_take_messages() {
    let fake = Fake::new();
    let mut a = agent("plain", State::Idle);
    a.interactive = true;
    let mut m = open(a, &fake, 120, 40);
    typed(&mut m, "hi");
    press(&mut m, "enter");
    assert_eq!(
        flash(&m),
        "tidy the parser is open in a terminal; rush can't send to it"
    );
    assert!(replies(&fake).is_empty());
}

/// Idle and not run by rush: it's a reply through its agent's daemon; one
/// the daemon let go moves to rush mode with it.
#[test]
fn replies_outside_rush() {
    let (mut m, fake) = outside(State::Idle);
    typed(&mut m, "hi");
    press(&mut m, "enter");
    assert_eq!(replies(&fake), ["hi"]);
    assert_eq!(flash(&m), "sent to tidy the parser");
    *fake.gone.lock().unwrap() = true;
    typed(&mut m, "again");
    press(&mut m, "enter");
    let adopted = fake.adopted.lock().unwrap().clone();
    assert_eq!(adopted, [(KEY.to_string(), "again".to_string())]);
}

// ---- sessionviews.go: the queue's keys, against the host's requests ----

fn queue_of(items: &[&str]) -> Said {
    Said::Info(Info {
        queue: items.iter().map(|s| s.to_string()).collect(),
        ..Info::default()
    })
}

fn picked(m: &Model) -> String {
    m.session.as_ref().unwrap().sel.clone()
}

/// ↑ from the empty box picks the last queued message, ↑↓ go through the
/// queue, ↓ past its last back to the box.
#[test]
fn up_picks_the_queue() {
    let (mut m, _) = hosted("steer", vec![queue_of(&["a", "b", "c"])]);
    press(&mut m, "up");
    assert_eq!(picked(&m), "q:2");
    press(&mut m, "up");
    assert_eq!(picked(&m), "q:1");
    press(&mut m, "down");
    press(&mut m, "down");
    assert_eq!(picked(&m), "");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("queue ·3"), "{f}");
    press(&mut m, "up");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("space edit"), "the queue's keys: {f}");
}

#[test]
fn queue_edits() {
    let was = |i: usize, w: &str| (i, w.to_string());
    type Case = (&'static str, Vec<Op>, &'static [&'static str], &'static str);
    let (i, b) = was(1, "b");
    let cases: Vec<Case> = vec![
        (
            "]",
            vec![Op::MoveQueued {
                i,
                was: b.clone(),
                to: 2,
            }],
            &["a", "c", "b"],
            "q:2",
        ),
        (
            "[",
            vec![Op::MoveQueued {
                i,
                was: b.clone(),
                to: 0,
            }],
            &["b", "a", "c"],
            "q:0",
        ),
        (
            "shift+down",
            vec![Op::MergeQueued { i, was: b.clone() }],
            &["a", "b\n\nc"],
            "q:1",
        ),
        (
            "shift+up",
            vec![Op::MergeQueued {
                i: 0,
                was: "a".into(),
            }],
            &["a\n\nb", "c"],
            "q:0",
        ),
        (
            "s",
            vec![Op::SendQueued { i, was: b.clone() }],
            &["a", "c"],
            "q:1",
        ),
        (
            "backspace",
            vec![Op::RemoveQueued { i, was: b.clone() }],
            &["a", "c"],
            "q:1",
        ),
        ("h", vec![Op::HoldQueue(true)], &["a", "b", "c"], "q:1"),
        (
            "o",
            vec![Op::QueueSeparately(true)],
            &["a", "b", "c"],
            "q:1",
        ),
        (
            "g",
            vec![Op::SteerQueued { i, was: b.clone() }],
            &["a", "c"],
            "q:1",
        ),
        (
            "G",
            vec![
                Op::MergeQueued {
                    i: 0,
                    was: "a".into(),
                },
                Op::MergeQueued {
                    i: 0,
                    was: "a\n\nb".into(),
                },
                Op::EditQueued {
                    i: 0,
                    was: "a\n\nb\n\nc".into(),
                    text: super::host::join_queue(&["a".into(), "b".into(), "c".into()]),
                },
                Op::SteerQueued {
                    i: 0,
                    was: super::host::join_queue(&["a".into(), "b".into(), "c".into()]),
                },
            ],
            &[],
            "",
        ),
    ];
    for (k, ops, after, sel) in cases {
        let (mut m, fake) = hosted("steer", vec![queue_of(&["a", "b", "c"])]);
        press(&mut m, "up");
        press(&mut m, "up");
        assert_eq!(picked(&m), "q:1");
        press(&mut m, k);
        assert_eq!(fake.ops(), ops, "{k}");
        assert_eq!(conn(&mut m, KEY).info.queue, after, "{k}");
        assert_eq!(picked(&m), sel, "{k}");
    }
}

/// Merging past either end says there's nothing to merge with.
#[test]
fn queue_merge_at_the_ends() {
    let (mut m, fake) = hosted("steer", vec![queue_of(&["a", "b"])]);
    press(&mut m, "up");
    press(&mut m, "shift+down");
    assert_eq!(flash(&m), "nothing after it to merge with");
    press(&mut m, "up");
    press(&mut m, "shift+up");
    assert_eq!(flash(&m), "nothing before it to merge with");
    assert!(fake.ops().is_empty());
}

/// space edits a queued message in the box, the queue held meanwhile;
/// enter saves it back in place and lets the queue go; esc cancels.
#[test]
fn queue_edit_in_the_box() {
    let (mut m, fake) = hosted("steer", vec![queue_of(&["a", "b"])]);
    press(&mut m, "up");
    press(&mut m, "space");
    assert_eq!(input(&mut m), "b");
    assert_eq!(fake.ops(), [Op::HoldQueue(true)]);
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("editing queued message 2 · enter saves it back · esc cancels"),
        "{f}"
    );
    press(&mut m, "backspace");
    typed(&mut m, "bee");
    press(&mut m, "enter");
    assert_eq!(
        fake.ops(),
        [
            Op::HoldQueue(true),
            Op::EditQueued {
                i: 1,
                was: "b".into(),
                text: "bee".into(),
            },
            Op::HoldQueue(false),
        ]
    );
    assert_eq!(conn(&mut m, KEY).info.queue, ["a", "bee"]);
    // esc cancels, and lets go.
    fake.ops.lock().unwrap().clear();
    press(&mut m, "up");
    press(&mut m, "space");
    press(&mut m, "esc");
    assert_eq!(input(&mut m), "");
    assert_eq!(fake.ops(), [Op::HoldQueue(true), Op::HoldQueue(false)]);
    // Something typed: finish it first.
    typed(&mut m, "x");
    m.session.as_mut().unwrap().sel = "q:0".into();
    press(&mut m, "space");
    assert_eq!(
        flash(&m),
        "Finish or stash your message before editing the queue"
    );
}

/// ctrl+enter with the box empty sends the whole queue now, as one; with
/// text, the text after it.
#[test]
fn send_the_queue_now() {
    let (mut m, fake) = hosted("steer", vec![queue_of(&["a", "b"])]);
    press(&mut m, "ctrl+enter");
    let joined = super::host::join_queue(&["a".into(), "b".into()]);
    assert_eq!(
        fake.ops(),
        [
            Op::MergeQueued {
                i: 0,
                was: "a".into(),
            },
            Op::EditQueued {
                i: 0,
                was: "a\n\nb".into(),
                text: joined.clone(),
            },
            Op::SendQueued { i: 0, was: joined },
        ]
    );
    assert!(conn(&mut m, KEY).info.queue.is_empty());
    let (mut m, fake) = hosted("steer", vec![queue_of(&["a"])]);
    typed(&mut m, "c");
    press(&mut m, "ctrl+enter");
    let joined = super::host::join_queue(&["a".into(), "c".into()]);
    assert_eq!(
        fake.ops(),
        [
            Op::EditQueued {
                i: 0,
                was: "a".into(),
                text: joined.clone(),
            },
            Op::SendQueued { i: 0, was: joined },
        ]
    );
}

/// A rush command queued runs alone; the card's words.
#[test]
fn queue_card_words() {
    let (mut m, _) = hosted("steer", vec![queue_of(&["/compact", "then this"])]);
    let f = screen(&mut m).join("\n");
    assert!(f.contains("⌘ /compact"), "{f}");
    assert!(f.contains("once it's done then this"), "{f}");
    assert!(f.contains("sends when this turn ends"), "{f}");
}

/// Queued behind a turn that can't take a message (Kimi's), the card says
/// so and how to send now (rushmode.go paneDock's queue, Go 5d617d3d).
#[test]
fn queue_card_stuck_behind_a_turn() {
    let (mut m, _) = hosted(
        "steer",
        vec![Said::Info(Info {
            queue: vec!["then this".into(), "and that".into()],
            queue_stuck: NOW - 120_000,
            ..Info::default()
        })],
    );
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains(&format!(
            "· its turn can't take it · {} stops the turn and sends it",
            super::sendmode::SEND_NOW
        )),
        "{f}"
    );
}

// ---- subqueue.go, subqueue_test.go ----

/// One message into a watched subagent's inbox per step it ends; a run
/// that finished hands the rest to the main session (TestSubQueue).
#[test]
fn sub_queue() {
    let mut q = LocalQueue::default();
    let steps = 1;
    subqueue::queue(&mut q, steps, "one", NOW);
    subqueue::queue(&mut q, steps, "two", NOW);
    assert_eq!(
        subqueue::due(&mut q, "lane", true, steps, NOW),
        Due::Tell("one".into()),
        "the first goes in at once, for the end of the step it's on"
    );
    assert_eq!(q.items, ["two"]);
    assert_eq!(
        subqueue::due(&mut q, "lane", true, steps, NOW),
        Due::Nothing
    );
    // It took one more step.
    assert_eq!(
        subqueue::due(&mut q, "lane", true, steps + 1, NOW),
        Due::Tell("two".into())
    );
    subqueue::queue(&mut q, steps + 1, "three", NOW);
    assert_eq!(
        subqueue::due(&mut q, "lane", true, steps + 1, NOW),
        Due::Nothing,
        "one sent on this step already"
    );
    match subqueue::due(&mut q, "lane", false, steps + 1, NOW) {
        Due::Main { mine, text } => {
            assert_eq!(mine, "three");
            assert_eq!(
                text,
                "For your subagent lane, which finished before it got this:\n\nthree"
            );
        }
        d => panic!("a finished run's queue goes to the main session: {d:?}"),
    }
    assert!(q.items.is_empty());
}

#[test]
fn sub_notes_keep_ten() {
    let mut notes = Vec::new();
    for i in 0..12 {
        subqueue::note(
            &mut notes,
            subqueue::SubNote {
                text: i.to_string(),
                ..Default::default()
            },
        );
    }
    assert_eq!(notes.len(), 10);
    assert_eq!(notes[0].text, "2");
    let s = Session::new();
    let l = subqueue::note_lines(&notes, &s, true, 80);
    assert_eq!(l.len(), 3, "the latest three");
    assert!(l[2].plain().contains("↪ 11"), "{:?}", l[2].plain());
}

// ---- host.go, host/queue.go ----

#[test]
fn join_queue_and_commands() {
    assert_eq!(super::host::join_queue(&["one".into()]), "one");
    assert_eq!(
        super::host::join_queue(&["a".into(), "b".into()]),
        "2 messages, queued while you worked. Each is its own message; take them in order.\n\n[Message 1 of 2]\na\n\n[Message 2 of 2]\nb"
    );
    for (t, want) in [
        ("/compact", true),
        ("  /model opus", true),
        ("/usr/bin/env", false),
        ("/etc/x.conf", false),
        ("compact", false),
    ] {
        assert_eq!(super::host::is_command(t), want, "{t}");
    }
}

// ---- the dock's frames, at Go's widths ----

/// The box and its hint row, as Go's paneDock draws them, at the widths
/// Go's tests use.
#[test]
fn dock_frames() {
    for w in [80, 100, 120] {
        let (mut m, _) = hosted("steer", Vec::new());
        m.w = w;
        core(&mut m).hide_map = true; // as Go's tests draw it
        let f = screen(&mut m);
        let n = f.len();
        // The box's three rows, then the keys.
        assert!(f[n - 4].starts_with("  ╭─"), "{w}: {f:?}");
        assert!(f[n - 4].ends_with("╮"), "{w}: {f:?}");
        assert_eq!(
            rush_core::text::width(&f[n - 4]),
            w - 1,
            "{w}: the box is Go's pane, the width less 3, from column 2"
        );
        assert!(
            f[n - 3].contains("❯  a message for this agent"),
            "{w}: {f:?}"
        );
        assert!(f[n - 2].starts_with("  ╰"), "{w}: {f:?}");
        assert_eq!(f[n - 1].trim_end(), "    f1 more  ·  esc back", "{w}");
    }
}

/// While it works with something typed: what enter does, on the box's
/// edge and in the keys.
#[test]
fn dock_frame_while_it_works() {
    let (mut m, _) = hosted("steer", Vec::new());
    core(&mut m).hide_map = true;
    working(&mut m, "go");
    typed(&mut m, "more");
    let f = screen(&mut m);
    let n = f.len();
    assert!(
        f[n - 4].starts_with("  ╭─ queuing · ctrl+t coalesce ─"),
        "{f:?}"
    );
    assert!(f[n - 3].contains("❯ more"), "{f:?}");
    assert_eq!(
        f[n - 1].trim_end(),
        "    enter queue it  ·  ctrl+enter send now  ·  ctrl+t queue · guide · stop · discuss  ·  f1 more  ·  esc back"
    );
}

/// Picked queued message: its keys on the bottom row; the box says how to
/// get back.
#[test]
fn dock_frame_queue_picked() {
    let (mut m, _) = hosted("steer", vec![queue_of(&["first", "second"])]);
    press(&mut m, "up");
    let f = screen(&mut m);
    let n = f.len();
    assert!(
        f[n - 4].contains("typing returns here · ↓ past the last row or esc"),
        "{f:?}"
    );
    assert!(
        f[n - 1].starts_with("    space edit  ·  shift+↑↓ merge up/down"),
        "{f:?}"
    );
    let card = f
        .iter()
        .position(|l| l.contains("queue ·2"))
        .expect("the card");
    assert!(f[card + 1].contains("1  first"), "{f:?}");
    assert!(f[card + 2].contains("2  second"), "{f:?}");
}

/// The link opens to the session showing, and is let go when it's left.
#[test]
fn the_link_follows_the_session() {
    let (mut m, fake) = hosted("steer", Vec::new());
    assert_eq!(*fake.dials.lock().unwrap(), 1);
    live::sync(&mut m);
    assert_eq!(*fake.dials.lock().unwrap(), 1, "dialled once");
    m.session = None;
    live::sync(&mut m);
    assert!(!conn(&mut m, KEY).hosted());
    assert!(core(&mut m).linked.is_empty());
    // A turn ending clears what was live.
    let (mut m, _) = hosted("steer", Vec::new());
    working(&mut m, "go");
    let now = m.now;
    sess(&mut m).apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }),
        now,
    );
    assert!(sess(&mut m).live().is_none());
}

/// The keys row, as Go's paneDock draws it (minimap hidden: it isn't
/// ported yet), in the pane two columns in, at Go's widths.
#[test]
fn dock_keys_at_go_widths() {
    let want = [
        (
            "typed-live",
            80,
            "enter queue it  ·  ctrl+enter send now  ·  esc back",
        ),
        (
            "typed-live",
            100,
            "enter queue it  ·  ctrl+enter send now  ·  ctrl+t queue · guide · stop · discuss  ·  esc back",
        ),
        (
            "typed-live",
            120,
            "enter queue it  ·  ctrl+enter send now  ·  ctrl+t queue · guide · stop · discuss  ·  f1 more  ·  esc back",
        ),
        (
            "queue-picked",
            80,
            "space edit  ·  shift+↑↓ merge up/down  ·  [ ] move  ·  esc done",
        ),
        (
            "queue-picked",
            100,
            "space edit  ·  shift+↑↓ merge up/down  ·  [ ] move  ·  s send this now  ·  esc done",
        ),
        (
            "queue-picked",
            120,
            "space edit  ·  shift+↑↓ merge up/down  ·  [ ] move  ·  s send this now  ·  g steer with it  ·  esc done",
        ),
        (
            "working",
            80,
            "ctrl+t queue · guide · stop · discuss  ·  ctrl+x stop it  ·  esc back",
        ),
        (
            "working",
            100,
            "ctrl+t queue · guide · stop · discuss  ·  ctrl+x stop it  ·  ↑ rewind  ·  esc back",
        ),
        (
            "working",
            120,
            "ctrl+t queue · guide · stop · discuss  ·  ctrl+x stop it  ·  ↑ rewind  ·  f1 more  ·  esc back",
        ),
    ];
    for (st, w, keys) in want {
        let queued = if st == "queue-picked" {
            vec![queue_of(&["first", "second"])]
        } else {
            Vec::new()
        };
        let (mut m, _) = hosted("steer", queued);
        m.w = w;
        core(&mut m).hide_map = true; // as Go was drawn
        match st {
            "typed-live" => {
                working(&mut m, "go");
                typed(&mut m, "more");
            }
            "queue-picked" => {
                press(&mut m, "up");
            }
            _ => working(&mut m, "go"),
        }
        let f = screen(&mut m);
        let n = f.len();
        assert_eq!(f[n - 1].trim_end(), format!("    {keys}"), "{st} {w}");
        assert_eq!(
            rush_core::text::width(&f[n - 2]),
            w - 1,
            "{st} {w}: the box"
        );
    }
}

#[path = "tests_views.rs"]
mod views_tests;

#[path = "tests_changes.rs"]
mod changes_tests;

#[path = "tests_beside.rs"]
mod beside_tests;
