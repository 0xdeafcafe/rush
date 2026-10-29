//! The session's link to its host and what's beside its conversation:
//! the transcript before a host's replay (twopass.go's hostPre, preFor,
//! takeWhole; rushmode.go:1832-1858), a split pane's error as news
//! (rushmode.go:2096-2104), the send-now key as bound (cmdbar.go:1756),
//! the subagents' edits handed to Changes (ownfiles.go:65-90) and only the
//! project's files counted (bars.go:257,268). No sleeps: a model with no
//! loop runs handed-off work at once; every file is in a temp dir.

use std::collections::BTreeMap;
use std::sync::Arc;

use rush_convo::{Helper, Session, Subagent};
use rush_core::adapter::{Profile, Registry};
use rush_core::agent::State;
use rush_core::text::Line;
use rush_core::time::{Ms, parse_rfc3339};
use rush_state::Time;

use super::host::{Fake, Said};
use super::live::{self, PRE_TAIL, Pre, pre_for};
use super::queue::Queued;
use super::tests::{KEY, agent, hosted, open, said, sess};
use super::{conn, subs};
use crate::frame::messages::SayKind;
use crate::session::SessionView;

fn reg() -> Registry {
    let r = Registry::new();
    r.register(rush_claude::Claude::new());
    r
}

fn ts(s: &str) -> Ms {
    parse_rfc3339(s)
}

/// A line of a Claude Code transcript: you said text at t.
fn prompt(t: &str, text: &str) -> String {
    format!(r#"{{"type":"user","timestamp":"{t}","message":{{"role":"user","content":"{text}"}}}}"#)
}

/// Its answer at t.
fn answer(t: &str, id: &str, text: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{t}","message":{{"id":"{id}","role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

/// A transcript of two turns, the second after the host started.
fn transcript(dir: &std::path::Path) -> String {
    let p = dir.join("s1.jsonl");
    let lines = [
        prompt("2026-09-23T20:00:00Z", "find the pane"),
        answer("2026-09-23T20:00:01Z", "m1", "It is in view.go."),
        prompt("2026-09-23T20:10:00Z", "now fix it"),
        answer("2026-09-23T20:10:01Z", "m2", "Fixed."),
    ];
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p.to_string_lossy().into_owned()
}

fn info(started: &str) -> rush_host::Info {
    rush_host::Info {
        id: "k1".into(),
        session_id: "s1".into(),
        started_at: Time::from_ms(ts(started)),
        ..rush_host::Info::default()
    }
}

fn resumed() -> rush_host::Config {
    rush_host::Config {
        resume: true,
        ..rush_host::Config::default()
    }
}

fn prompts(s: &Session) -> Vec<String> {
    s.turns.iter().map(|t| t.prompt.clone()).collect()
}

// ---- twopass.go: hostPre, preFor ----

/// A session that resumed a conversation reads its transcript up to when
/// its host started; one that started afresh reads nothing before its
/// replay (preFor).
#[test]
fn pre_reads_up_to_the_host_start() {
    let d = tempfile::tempdir().unwrap();
    let path = transcript(d.path());
    let reg = reg();
    let i = info("2026-09-23T20:05:00Z");
    let fresh = pre_for(&reg, Some(&i), None, path.clone(), Profile::default(), 0);
    assert!(!fresh.any(), "nothing to read for a fresh session");
    let mut pre = pre_for(
        &reg,
        Some(&i),
        Some(&resumed()),
        path.clone(),
        Profile::default(),
        0,
    );
    assert!(pre.any());
    assert_eq!(pre.before, ts("2026-09-23T20:05:00Z"));
    let s = pre.tail();
    assert_eq!(
        prompts(&s),
        ["find the pane"],
        "the turn after its start is the replay's"
    );
    assert!(!s.partial, "small enough to read whole");
    assert!(s.live().is_none(), "its last turn is closed");
    let w = pre.whole(None);
    assert_eq!(prompts(&w), ["find the pane"]);
}

/// The latest of its start, a rewind and where a trimmed replay begins
/// is where its transcript stops; a trimmed replay reads back whatever
/// the config says (preFor's trimmed).
#[test]
fn pre_before_latest_of_start_rewind_and_trim() {
    let reg = reg();
    let mut i = info("2026-09-23T20:00:00Z");
    i.rewound_at = Time::from_ms(ts("2026-09-23T20:02:00Z"));
    i.replay_from = Time::from_ms(ts("2026-09-23T20:03:00Z"));
    let p = pre_for(
        &reg,
        Some(&i),
        Some(&rush_host::Config::default()),
        "/x/s1.jsonl".into(),
        Profile::default(),
        7,
    );
    assert!(p.any(), "a trimmed replay reads what came before it");
    assert_eq!(p.before, ts("2026-09-23T20:03:00Z"));
    let p = pre_for(
        &reg,
        None,
        Some(&resumed()),
        "/x/s1.jsonl".into(),
        Profile::default(),
        7,
    );
    assert_eq!(p.before, 7, "with no info, up to now");
    let p = pre_for(
        &reg,
        Some(&i),
        None,
        "/x/s1.jsonl".into(),
        Profile::default(),
        7,
    );
    assert!(!p.any(), "no config: nothing");
}

/// A fork with nothing of its own yet reads the transcript it forked
/// from (hostPre.fork).
#[test]
fn pre_reads_the_fork_when_its_own_is_empty() {
    let d = tempfile::tempdir().unwrap();
    transcript(d.path()); // s1.jsonl, the original
    let own = d.path().join("s2.jsonl");
    std::fs::write(&own, "").unwrap();
    let cfg = rush_host::Config {
        resume: true,
        from: "s1".into(),
        ..rush_host::Config::default()
    };
    let mut pre = pre_for(
        &reg(),
        Some(&info("2026-09-23T20:05:00Z")),
        Some(&cfg),
        own.to_string_lossy().into_owned(),
        Profile::default(),
        0,
    );
    assert!(pre.fork.ends_with("/s1.jsonl"), "{}", pre.fork);
    assert_eq!(prompts(&pre.tail()), ["find the pane"]);
}

/// A transcript longer than the end read first opens Partial on its last
/// turns; the whole of it reads every turn (tail, whole).
#[test]
fn pre_tail_of_a_long_transcript() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("long.jsonl");
    let pad = "x".repeat(4000);
    let mut lines = Vec::new();
    for i in 0..1200 {
        let t = format!("2026-09-23T10:{:02}:{:02}Z", i / 60, i % 60);
        lines.push(prompt(&t, &format!("turn {i} {pad}")));
    }
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    assert!(std::fs::metadata(&p).unwrap().len() > 2 * PRE_TAIL);
    let mut pre = pre_for(
        &reg(),
        Some(&info("2026-09-23T23:00:00Z")),
        Some(&resumed()),
        p.to_string_lossy().into_owned(),
        Profile::default(),
        0,
    );
    let s = pre.tail();
    assert!(s.partial, "only its end");
    assert!(s.turns.len() < 1200 && !s.turns.is_empty());
    let w = pre.whole(None);
    assert!(!w.partial);
    assert_eq!(w.turns.len(), 1200);
}

/// Connected with the end of what came before the replay, the host's
/// lines go on top; the whole of it, once read, takes the end's place
/// with the lines that came meanwhile on top too (onReplay's keep,
/// onWhole, takeWhole).
#[test]
fn whole_takes_the_lines_kept_meanwhile() {
    let fake = Fake::new();
    fake.host(KEY);
    let mut m = open(agent("steer", State::Working), &fake, 120, 40);
    let mut end = Session::new();
    end.sent("turn two".into(), Vec::new(), 2_000);
    end.apply(&ended(), 2_500);
    end.partial = true;
    let link = rush_ui_link(&fake);
    super::live::opened_with(&mut m, KEY, Ok(link), Some((Pre::default(), end)));
    let r#gen = conn(&mut m, KEY).link_gen;
    assert!(!conn(&mut m, KEY).keep, "nothing to read whole");
    // As though the whole were being read (with no loop it's read at once).
    conn(&mut m, KEY).keep = true;
    live::take(
        &mut m,
        KEY,
        r#gen,
        vec![Said::Sent {
            text: "turn three".into(),
            images: Vec::new(),
        }],
        false,
    );
    assert_eq!(prompts(sess(&mut m)), ["turn two", "turn three"]);
    assert_eq!(conn(&mut m, KEY).since.len(), 1, "kept for the whole");
    let mut whole = Session::new();
    whole.sent("turn one".into(), Vec::new(), 1_000);
    whole.apply(&ended(), 1_500);
    whole.sent("turn two".into(), Vec::new(), 2_000);
    whole.apply(&ended(), 2_500);
    live::took_whole(&mut m, KEY, r#gen, whole);
    assert_eq!(
        prompts(sess(&mut m)),
        ["turn one", "turn two", "turn three"]
    );
    assert!(!sess(&mut m).partial);
    assert!(!conn(&mut m, KEY).keep && conn(&mut m, KEY).since.is_empty());
}

fn ended() -> rush_core::event::Event {
    rush_core::event::Event::TurnEnd(rush_core::event::TurnEnd {
        reason: "done".into(),
        ..Default::default()
    })
}

/// A link that does nothing, for opening one by hand.
fn rush_ui_link(fake: &Arc<Fake>) -> Arc<dyn super::host::Link> {
    use super::host::Hosts;
    fake.dial(&agent("steer", State::Working)).unwrap()
}

// ---- rushmode.go:2096-2104: a session's error ----

/// The session on screen: its error is a failure, yours to see now. One
/// in a pane beside it: news, naming it.
#[test]
fn errors_on_screen_fail_beside_are_news() {
    let (mut m, _) = hosted("steer", Vec::new());
    let r#gen = conn(&mut m, KEY).link_gen;
    live::take(
        &mut m,
        KEY,
        r#gen,
        vec![Said::Error("it broke".into())],
        false,
    );
    assert_eq!(said(&m), (SayKind::Failed, "it broke".into()));
    // Another session's, beside the open one.
    let other = "default/k2";
    let mut a = agent("steer", State::Working);
    a.key = other.into();
    a.display_name = "fix login".into();
    m.agents.push(a.clone());
    let src = m.session.as_ref().unwrap().source.clone();
    m.pane = Some(SessionView::new(other.into(), src, 2));
    conn(&mut m, other).link_gen = 99;
    live::take(
        &mut m,
        other,
        99,
        vec![Said::Error("limit hit".into())],
        false,
    );
    assert_eq!(
        said(&m),
        (
            SayKind::News,
            "fix login reported an error: limit hit".into()
        )
    );
}

// ---- cmdbar.go:1756 sendNowKey ----

/// The held and stuck lines name the send-now key as you bound it, not a
/// fixed ctrl+enter (rushmode.go:3543-3546).
#[test]
fn held_and_stuck_name_your_send_now_key() {
    let (mut m, _) = hosted("steer", Vec::new());
    let plain = |l: Line| l.plain();
    let held = Queued {
        items: vec!["a".into()],
        held: true,
        ..Queued::default()
    };
    let stuck = Queued {
        items: vec!["a".into()],
        stuck: true,
        ..Queued::default()
    };
    assert!(plain(super::queue::when(&mut m, KEY, &held)).contains("ctrl+enter sends it now"));
    crate::screens::cmdbar::keybind::set_keys(
        &mut m,
        rush_keymap::File {
            bindings: BTreeMap::from([("session.send".to_string(), vec!["ctrl+g".to_string()])]),
        },
    );
    let h = plain(super::queue::when(&mut m, KEY, &held));
    assert!(h.contains("held · ctrl+g sends it now"), "{h}");
    let s = plain(super::queue::when(&mut m, KEY, &stuck));
    assert!(
        s.contains("its turn can't take it · ctrl+g stops the turn and sends it"),
        "{s}"
    );
}

// ---- ownfiles.go:65-90 setHelpers ----

/// Each subagent read whole is handed to the session as a helper, named
/// short by its task; one whose calls came in the session's own
/// conversation isn't (setHelpers, helperName).
#[test]
fn helpers_from_runs_read_whole() {
    let (mut m, _) = hosted("steer", Vec::new());
    let c = conn(&mut m, KEY);
    c.runs.subs = vec![
        Subagent {
            id: "a1".into(),
            typ: "Explore".into(),
            description: "find every place the parser is called from".into(),
            ..Subagent::default()
        },
        Subagent {
            id: "a2".into(),
            typ: "general-purpose".into(),
            ..Subagent::default()
        },
        Subagent {
            id: "a3".into(),
            typ: "unread".into(),
            ..Subagent::default()
        },
    ];
    c.runs.full.insert("a1".into(), Arc::new(Session::new()));
    c.runs.full.insert("a2".into(), Arc::new(Session::new()));
    subs::set_helpers(&mut m, KEY);
    let names: Vec<String> = sess(&mut m)
        .helpers
        .iter()
        .map(|h| h.name.clone())
        .collect();
    assert_eq!(names, ["find every place the pa…", "general-purpose"]);
    // The same again changes nothing: Changes isn't worked out afresh.
    let first = sess(&mut m).helpers[0].sess.clone();
    subs::set_helpers(&mut m, KEY);
    assert!(Arc::ptr_eq(&first, &sess(&mut m).helpers[0].sess));
}

// ---- bars.go:257,268: the project's files only ----

/// The status line counts the project's files: not the session's memory
/// or plans outside it (ProjectChanges).
#[test]
fn status_counts_only_the_projects_files() {
    let (mut m, _) = hosted("steer", Vec::new());
    let s = sess(&mut m);
    s.info.cwd = "/src/checkout".into();
    let mut sub = Session::new();
    use rush_core::event::{Event, Message, Part, PartKind};
    use rush_core::tool::{Call, Input, Kind, Output};
    for (id, path) in [
        ("w1", "/src/checkout/main.go"),
        ("w2", "/Users/x/.claude/plans/plan.md"),
    ] {
        let call = Call {
            id: id.into(),
            name: "Write".into(),
            kind: Kind::Write,
            input: Input {
                path: path.into(),
                content: "a\nb\n".into(),
                ..Input::default()
            },
            ..Call::default()
        };
        sub.apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: format!("m-{id}"),
                parts: vec![Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(call)),
                    ..Part::default()
                }],
                ..Message::default()
            }),
            1,
        );
        sub.apply(
            &Event::Message(Message {
                role: "user".into(),
                id: format!("r-{id}"),
                parts: vec![Part {
                    kind: PartKind::ToolResult,
                    output: Some(Box::new(Output {
                        call_id: id.into(),
                        ..Output::default()
                    })),
                    ..Part::default()
                }],
                ..Message::default()
            }),
            2,
        );
    }
    sess(&mut m).helpers = vec![Helper {
        name: "writer".into(),
        sess: Arc::new(sub),
    }];
    let o = crate::screens::bars::agent::Of::read(&m, KEY).unwrap();
    assert_eq!(o.files, 1, "the plan outside the project isn't counted");
}
