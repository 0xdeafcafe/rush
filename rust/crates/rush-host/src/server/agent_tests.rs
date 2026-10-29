//! The per-agent watching, driven by hand: sleep_test.go, decisions_test.go
//! (on the session), away_test.go, stall_test.go, subtool_test.go
//! (TestAnswerLifecycle), heard_test.go, startctx_test.go, longtask_test.go
//! (askable).

use std::sync::{Arc, Mutex};

use rush_core::adapter::{self, Conn, Hearer, Input};
use rush_core::event::{self, Event, Part, PartKind};
use rush_core::tool;
use rush_state::Time;

use super::super::Hooks;
use super::super::tests::{Kept, host, with_conn};
use super::*;
use crate::subtool::{finished, report, report_path};
use crate::testing::Home;
use crate::wire::{Away, Config, Held, Limit, Retry, Task};

fn cfg(id: &str) -> Config {
    Config {
        id: id.into(),
        kind: "claude".into(),
        ..Config::default()
    }
}

/// A host between turns, as rest sees it.
fn idle_host(id: &str, spawned: bool) -> Host {
    let mut c = cfg(id);
    if spawned {
        c.meta.insert("spawnedBy".into(), "parent".into());
    }
    let h = host(c);
    h.lock().info.state = "idle".into();
    h
}

/// rest as the idle wait running out would call it.
fn rest_now(h: &Host) {
    let g = {
        let mut st = h.lock();
        h.arm_idle(&mut st);
        st.w.idle_gen
    };
    h.rest(g);
}

// sleep_test.go:181: whatever is left to do keeps the host up; without
// any of it, it sleeps, saved to resume.
#[test]
fn idle_sleep_preserves_pending_work() {
    let _home = Home::new();
    type Change = fn(&mut State);
    let cases: Vec<(&str, bool, Change)> = vec![
        ("queue", false, |s| {
            s.info.queue = vec!["keep".into()];
            s.info.queue_held = true;
        }),
        ("approval", false, |s| {
            s.pending.insert("a".into(), super::super::Asked::default());
        }),
        ("needs", false, |s| s.info.needs = "Bash ls".into()),
        ("question", false, |s| s.asking = 1),
        ("background", false, |s| {
            s.info.background = vec![Task {
                id: "running".into(),
                ..Task::default()
            }]
        }),
        ("tasks", false, |s| {
            s.info.tasks = 1;
            s.task_start.insert("t1".into(), Time::now());
        }),
        ("retry", false, |s| s.info.retry = Some(Retry::default())),
        ("limit", false, |s| {
            s.info.limit = Some(Limit {
                continue_: true,
                ..Limit::default()
            })
        }),
        ("unanswered", false, |s| s.waiting = Some(Instant::now())),
        ("relogin", false, |s| s.info.relogin = true),
        ("away", false, |s| {
            s.info.away = Some(Away {
                until: Time::now() + Duration::from_secs(3600),
                ..Away::default()
            })
        }),
        ("owed answer", true, |s| {
            s.w.owed = "the first turn's answer".into()
        }),
        ("sending", true, |s| s.w.reporting = 1),
        ("asked for it", true, |s| {
            report(&s.cfg.id, "parent").unwrap();
        }),
    ];
    for (name, spawned, change) in cases {
        let h = idle_host("pending", spawned);
        change(&mut h.lock());
        rest_now(&h);
        let st = h.lock();
        assert!(
            !st.info.sleeping && !st.quit,
            "{name}: slept with work left"
        );
        drop(st);
        let _ = report("pending", "");
    }
    // Without any of it, both sleep: the cases above aren't vacuous.
    for spawned in [false, true] {
        let h = idle_host("clean", spawned);
        {
            let mut st = h.lock();
            st.began = true;
            st.cfg.session_id = "sid".into();
        }
        rest_now(&h);
        let st = h.lock();
        assert!(
            st.info.sleeping && st.quit,
            "idle host (spawned {spawned}) didn't sleep"
        );
        let saved = crate::client::read_config("clean").unwrap();
        assert!(saved.resume, "slept without saving it to resume");
        let ev = std::fs::read_to_string(crate::paths::events_path("clean")).unwrap_or_default();
        crate::events::flush();
        let ev = if ev.is_empty() {
            std::fs::read_to_string(crate::paths::events_path("clean")).unwrap_or_default()
        } else {
            ev
        };
        assert!(ev.contains("\"sleep\""), "{ev}");
    }
}

// sleep_test.go:234: the CLI shim owns its host: its agent rests, but the
// host stays for it.
#[test]
fn idle_owned_host_doesnt_sleep() {
    let _home = Home::new();
    let h = idle_host("owned", false);
    h.lock().cfg.owner = std::process::id() as i64;
    rest_now(&h);
    assert!(!h.lock().info.sleeping);
}

// sleep_test.go:244 and :253: a wait a send stopped does nothing.
#[test]
fn send_disarms_idle() {
    let _home = Home::new();
    let h = idle_host("sent", false);
    let armed = {
        let mut st = h.lock();
        h.arm_idle(&mut st);
        let g = st.w.idle_gen;
        h.disarm_idle(&mut st);
        g
    };
    h.rest(armed);
    assert!(
        !h.lock().info.sleeping,
        "slept on a wait a send had stopped"
    );
}

/// A conn whose close returns at once, its process left running.
struct EarlyClose(i32);

impl Conn for EarlyClose {
    fn send(&self, _: Input) -> adapter::Result<()> {
        Ok(())
    }
    fn answer(&self, _: &str, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn interrupt(&self) -> adapter::Result<()> {
        Ok(())
    }
    fn set_model(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn set_mode(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn close(&self) -> adapter::Result<()> {
        Ok(())
    }
    fn pid(&self) -> Option<i32> {
        Some(self.0)
    }
}

// sleep_test.go:273: the agent's process goes before rest carries on.
#[test]
fn idle_retirement_waits_for_actual_process() {
    let mut p = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = p.id() as i32;
    let reaper = std::thread::spawn(move || p.wait());
    let c: Arc<dyn Conn> = Arc::new(EarlyClose(pid));
    stop_resting_agent(&c);
    reaper.join().unwrap().unwrap();
    assert!(!crate::alive(pid as i64), "runtime PID remains alive");
}

pub(crate) const M1326: &str = "**Branch prompts in boxer's project context:** I read boxer's root (#1) from here.\n\n**My take:** you're right, they don't belong there in full.\n- **Already covered:** each branch's abstract already opens with an \"Ask:\" line.\n\nDecisions (my picks: 1a 2a):\n1. **Branch starts in the context:** (a) one-line intent plus breadcrumb; (b) drop them, rely on abstracts; (c) leave as is.\n2. **Boxer's copied `agents/ultimate.md`:** (a) assess a way for projects to add tools to @ultimate without forking it; (b) leave it.";

const M1335: &str = "**The main cause is my message format, not the card.**\n1. **Stated picks read as decided.** The prompt says to leave out \"anything already decided\".\n2. **Extra questions:** a trailing \"go\" or \"Build it?\" turned into an extra question.\n\nDecisions:\n1. **Scope:** (a) the full recommendation; (b) the easier alternative.\n2. **Keyboard browser test:** (a) add one that answers a 3-question card with the keyboard; (b) fast tests only.\n3. **Answer format:** (a) `1a. Topic: label` lines; (b) keep `Q:/A:`.\n\nMy picks: 1a 2a 3a.";

fn said(text: &str) -> event::Message {
    event::Message {
        role: "assistant".into(),
        parts: vec![Part {
            kind: PartKind::Text,
            text: text.into(),
            ..Part::default()
        }],
        ..event::Message::default()
    }
}

// decisions_test.go:210: a turn that ends with decisions puts the card on
// the session, which may still sleep: they don't need it. What you send
// next takes the card away, and so does dismissing it.
#[test]
fn decisions_on_the_session() {
    let _home = Home::new();
    let h = host(cfg("dec"));
    let c = Arc::new(Kept::default());
    let conn: Arc<dyn Conn> = c.clone();
    with_conn(&h, conn.clone());
    let turn = |text: &str, reason: &str| {
        let mut st = h.lock();
        st.info.state = "working".into();
        if !text.is_empty() {
            h.said(&mut st, &said(text));
        }
        h.on_turn_end(
            &mut st,
            &conn,
            event::TurnEnd {
                reason: reason.into(),
                ..Default::default()
            },
        );
    };
    turn(M1335, "done");
    let d = {
        let st = h.lock();
        let d = st.info.decisions.clone().expect("a card");
        assert!(d.asks.len() == 3 && st.info.needs.is_empty() && st.info.state == "idle");
        assert!(
            h.runtime_idle_safe(&st) && h.sleep_safe(&st) && st.info.quiet(),
            "open decisions keep the agent's program or the host up"
        );
        d
    };
    let answer = crate::decisions::decision_answer(
        &d,
        &HashMap::from([(d.asks[0].text.clone(), d.asks[0].options[0].label.clone())]),
    );
    h.send(&answer, &[], false).unwrap();
    {
        let st = h.lock();
        let got = c.got.lock().unwrap();
        assert!(st.info.decisions.is_none() && got.len() == 1);
        assert_eq!(got[0].text, "1a. Scope: the full recommendation");
    }
    // The next turn said nothing new: last turn's block isn't asked again.
    turn("", "done");
    assert!(
        h.lock().info.decisions.is_none(),
        "asked again from an earlier turn"
    );
    turn(M1326, "interrupted");
    assert!(
        h.lock().info.decisions.is_none(),
        "asked from an interrupted turn"
    );
    turn(M1326, "done");
    let id = {
        let st = h.lock();
        let d = st.info.decisions.as_ref().expect("a card");
        assert_eq!(crate::decisions::needs_decisions(d), "2 decisions");
        d.id.clone()
    };
    let mut o = crate::op::Op::named("decisions_dismiss");
    o.id = "someone else's".into();
    h.do_op(o).unwrap();
    assert!(h.lock().info.decisions.is_some());
    let mut o = crate::op::Op::named("decisions_dismiss");
    o.id = id;
    h.do_op(o).unwrap();
    assert!(h.lock().info.decisions.is_none());
    assert_eq!(c.got.lock().unwrap().len(), 1, "dismissing sent something");
}

// subtool_test.go:107: an answer rush will try again isn't the last; a
// turn that ends with a message queued is answered with the next, its
// error kept though it ended "done"; and it's reported once.
#[test]
fn answer_lifecycle() {
    let _home = Home::new();
    let id = "answer01";
    let mut c = cfg(id);
    c.meta.insert("spawnedBy".into(), "nobody00".into());
    let h = host(c);
    let mut st = h.lock();
    st.info.state = "idle".into();
    st.info.queue = vec!["next".into()];
    st.last_said = "first".into();
    h.turn_done(
        &mut st,
        &event::TurnEnd {
            reason: "done".into(),
            ..Default::default()
        },
    );
    assert!(
        !finished(&h.reg, id, &st.info, Time::default()).1,
        "finished with a message queued"
    );
    report(id, "nobody00").unwrap();
    st.info.queue.clear();
    st.last_said = "second".into();
    h.turn_done(
        &mut st,
        &event::TurnEnd {
            reason: "done".into(),
            err: "is_error".into(),
            ..Default::default()
        },
    );
    let (a, done) = finished(&h.reg, id, &st.info, Time::default());
    assert!(
        done && a.text == "first\n\nsecond" && a.err == "is_error",
        "{a:?}"
    );
    assert!(
        !report_path(id).exists(),
        "reported, but would report again"
    );
    assert_eq!(st.w.reporting, 1, "the answer is being sent");
}

// away_test.go:58: at a check-in an idle session is told to keep going;
// the time's up ends an away but keeps it for you, and ends a loop
// outright.
#[test]
fn check_away() {
    let _home = Home::new();
    let h = host(cfg("aw"));
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    let now = Time::now();
    let mut st = h.lock();
    st.info.state = "idle".into();
    st.info.away = Some(Away {
        until: now + Duration::from_secs(3600),
        every: Duration::from_secs(1800),
        next: now,
        held: vec![Held {
            kind: "question".into(),
            text: "which?".into(),
            ..Held::default()
        }],
        ..Away::default()
    });
    h.check_away(&mut st, now);
    {
        let got = c.got.lock().unwrap();
        let a = st.info.away.as_ref().unwrap();
        assert!(
            got.len() == 1 && got[0].text.contains("Rush check-in"),
            "{got:?}"
        );
        assert!(
            a.nudges == 1 && a.next == now + Duration::from_secs(1800),
            "{a:?}"
        );
    }
    st.info.state = "idle".into();
    h.check_away(&mut st, now + Duration::from_secs(3600));
    let a = st.info.away.clone().expect("kept for you");
    assert!(!a.on() && a.held.len() == 1, "{a:?}");
    assert!(st.info.quiet(), "what an away left keeps its host up");
    st.info.away = Some(Away {
        until: now + Duration::from_secs(3600),
        r#loop: true,
        next: now + Duration::from_secs(60),
        ..Away::default()
    });
    h.check_away(&mut st, now + Duration::from_secs(3600));
    assert!(
        st.info.away.is_none(),
        "a loop's time is up and it's still on"
    );
}

// away_test.go:86: away, what the agent asks is saved for you and refused
// at once; looping, it waits for you as ever.
#[test]
fn away_holds() {
    let _home = Home::new();
    let h = host(cfg("ah"));
    let c = Arc::new(Kept::default());
    let conn: Arc<dyn Conn> = c.clone();
    with_conn(&h, conn.clone());
    let mut st = h.lock();
    st.info.state = "working".into();
    st.info.away = Some(Away {
        until: Time::now() + Duration::from_secs(3600),
        ..Away::default()
    });
    let q = event::Question {
        id: "q".into(),
        asks: vec![event::Ask {
            text: "Which DB?".into(),
            options: vec![
                event::Choice {
                    label: "pg".into(),
                    ..Default::default()
                },
                event::Choice {
                    label: "sqlite".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    h.on_agent_event(&mut st, &conn, Event::Question(q));
    let ap = event::Approval {
        id: "p".into(),
        call: tool::Call {
            name: "Bash".into(),
            input: tool::Input {
                command: "rm -rf build".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    h.on_agent_event(&mut st, &conn, Event::Approval(Box::new(ap)));
    let held = st.info.away.as_ref().unwrap().held.clone();
    assert!(st.info.state != "blocked", "blocked while away");
    assert!(
        held.len() == 2
            && held[0].text == "Which DB? (pg / sqlite)"
            && held[1].kind == "permission",
        "{held:?}"
    );
    st.info.away.as_mut().unwrap().r#loop = true;
    let q2 = event::Question {
        id: "q2".into(),
        asks: vec![event::Ask {
            text: "And?".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    h.on_agent_event(&mut st, &conn, Event::Question(q2));
    assert!(st.info.state == "blocked" && st.info.away.as_ref().unwrap().held.len() == 2);
}

// stall_test.go:14: a turn silent for STALL_AFTER with nothing running is
// marked stalled and never stopped; a tool running, a question asked, or a
// word from the agent keeps it from showing, and the next word clears it.
#[test]
fn stalled_turn_is_shown_never_stopped() {
    let _home = Home::new();
    let h = host(cfg("stall"));
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    let mut st = h.lock();
    st.info.state = "working".into();
    let now = Instant::now();
    let call = tool::Call {
        id: "c1".into(),
        name: "shell".into(),
        ..Default::default()
    };
    let msg = |role: &str, id: &str, p: Part| {
        Event::Message(event::Message {
            role: role.into(),
            id: id.into(),
            parts: vec![p],
            ..Default::default()
        })
    };
    h.heard_from(
        &mut st,
        &msg(
            "assistant",
            "",
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Default::default()
            },
        ),
    );
    h.mark_stall(&mut st, now + Duration::from_secs(3600));
    assert!(st.info.stalled.is_zero(), "stalled while a tool runs");
    h.heard_from(
        &mut st,
        &msg(
            "user",
            "",
            Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(tool::Output {
                    call_id: "c1".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ),
    );
    h.mark_stall(&mut st, Instant::now() + Duration::from_secs(60));
    assert!(
        st.info.stalled.is_zero(),
        "stalled a minute after its tool returned"
    );
    st.pending
        .insert("ap".into(), super::super::Asked::default());
    h.mark_stall(&mut st, Instant::now() + Duration::from_secs(3600));
    assert!(
        st.info.stalled.is_zero(),
        "stalled while it asks you something"
    );
    st.pending.remove("ap");
    h.mark_stall(
        &mut st,
        Instant::now() + STALL_AFTER + Duration::from_secs(1),
    );
    assert!(
        !st.info.stalled.is_zero(),
        "not stalled after STALL_AFTER of silence"
    );
    assert_eq!(
        c.stops.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "rush stopped a stalled turn"
    );
    // A word from the agent ends the stall, as does the turn's end.
    h.heard_from(
        &mut st,
        &msg(
            "assistant",
            "m2",
            Part {
                kind: PartKind::Text,
                text: "back".into(),
                ..Default::default()
            },
        ),
    );
    assert!(
        st.info.stalled.is_zero(),
        "still stalled once the agent spoke"
    );
    h.mark_stall(&mut st, Instant::now() + STALL_AFTER * 2);
    assert!(
        !st.info.stalled.is_zero(),
        "not stalled again after another silence"
    );
    h.quiet_turn(&mut st);
    assert!(st.info.stalled.is_zero(), "a new turn starts stalled");
}

// longtask_test.go:11: a subagent's task, and one the agent said ended,
// aren't asked about.
#[test]
fn askable_skips_subagents_and_ended() {
    let _home = Home::new();
    let h = host(cfg("lt"));
    let mut st = h.lock();
    let task = |id: &str, parent: &str| event::BackgroundTask {
        id: id.into(),
        kind_word: "local_bash".into(),
        parent: parent.into(),
        ..Default::default()
    };
    h.on_task(
        &mut st,
        &Event::Background(event::Background {
            tasks: vec![task("mine", ""), task("theirs", "a1"), task("killed", "")],
        }),
    );
    for (id, status) in [("killed", "killed"), ("mine", "running")] {
        h.on_task(
            &mut st,
            &Event::TaskUpdated(event::TaskUpdated {
                id: id.into(),
                status: status.into(),
                ..Default::default()
            }),
        );
    }
    let got: Vec<String> = h.askable(&st).into_iter().map(|t| t.id).collect();
    assert_eq!(got, ["mine"]);
}

/// A conn that says whether it heard a message, and keeps what it's sent.
#[derive(Default)]
struct Deaf {
    got: Mutex<Vec<Input>>,
}

impl Hearer for Deaf {
    fn heard(&self, _: &str, _: rush_core::time::Ms) -> Option<bool> {
        Some(false)
    }
}

impl Conn for Deaf {
    fn send(&self, input: Input) -> adapter::Result<()> {
        self.got.lock().unwrap().push(input);
        Ok(())
    }
    fn answer(&self, _: &str, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn interrupt(&self) -> adapter::Result<()> {
        Ok(())
    }
    fn set_model(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn set_mode(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn close(&self) -> adapter::Result<()> {
        Ok(())
    }
    fn hearer(&self) -> Option<&dyn Hearer> {
        Some(self)
    }
}

// heard_test.go:22: a turn the agent ended without reading the message
// sends it again, once.
#[test]
fn dropped_message_goes_again() {
    let _home = Home::new();
    let mut c = cfg("heard");
    c.kind = "fake".into();
    std::fs::create_dir_all(dir("heard")).unwrap();
    let (h, _rx) = super::super::build(
        super::super::tests::registry(),
        Hooks {
            heard_wait: Duration::ZERO,
            ..Hooks::quiet()
        },
        c,
    );
    let d = Arc::new(Deaf::default());
    let conn: Arc<dyn Conn> = d.clone();
    with_conn(&h, conn.clone());
    h.send("do the thing", &[], false).unwrap();
    let end = event::TurnEnd {
        reason: "done".into(),
        ..Default::default()
    };
    // Sent again off the lock: its folder's info says when it's working.
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[dir("heard")]);
    {
        let mut st = h.lock();
        h.on_turn_end(&mut st, &conn, end.clone());
    }
    while d.got.lock().unwrap().len() < 2 {
        assert!(
            w.wait(None, Some(Duration::from_secs(10))),
            "never sent again"
        );
    }
    assert_eq!(d.got.lock().unwrap()[1].text, "do the thing");
    // The second time it's dropped, it isn't sent a third.
    {
        let mut st = h.lock();
        assert!(st.w.unheard.as_ref().is_some_and(|u| u.again));
        h.on_turn_end(&mut st, &conn, end);
    }
    assert_eq!(d.got.lock().unwrap().len(), 2);
}

// startctx_test.go:14: a start says what changed from the last.
#[test]
fn restart_says_what_changed() {
    let _home = Home::new();
    let h = host(cfg("ctx"));
    let mut st = h.lock();
    let sec = |n: &str, t: i64| rush_core::usage::Section {
        name: n.into(),
        tokens: t,
    };
    let init = |tools: &[&str]| event::Init {
        tools: tools.iter().map(|t| t.to_string()).collect(),
        ..Default::default()
    };
    h.begin_start(&mut st, vec![sec("rush's views", 100)]);
    h.start_init(&mut st, &init(&["Bash", "Read"]));
    assert!(dir("ctx").join(START_FILE).exists(), "the start isn't kept");
    // The next start has a tool more and a bigger section.
    h.begin_start(&mut st, vec![sec("rush's views", 300)]);
    assert!(st.w.last_start.is_some());
    h.start_init(&mut st, &init(&["Bash", "Read", "Write"]));
    assert!(
        st.info.context_change.is_some(),
        "nothing said of what changed"
    );
}

// host.go:710: a detached agent's shells are no longer waited on, so the
// hung-shell look stops rather than carrying them to the next start.
#[test]
fn detach_forgets_the_shells_waited_on() {
    let _home = Home::new();
    let h = host(cfg("detach-shells"));
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    let mut st = h.lock();
    let call = tool::Call {
        id: "s1".into(),
        kind: tool::Kind::Shell,
        ..Default::default()
    };
    h.shell_calls(
        &mut st,
        &event::Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    assert_eq!(st.w.waits_on.len(), 1);
    h.detach(&mut st);
    assert!(
        st.w.waits_on.is_empty(),
        "still waits on a detached agent's shell"
    );
}
