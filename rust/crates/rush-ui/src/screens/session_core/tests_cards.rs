//! What the cards ask of a hosted session, done on its host: the host's
//! info as the cards read it, a decisions card set aside, #slim's
//! without, a control request and its answer, a rewind and its message
//! put back, a compaction by another model, a fork, a hand-off, a reply
//! to another agent, images into the box, enter's first answer, and a
//! stop with a message as one ordered job. After Go's
//! internal/ui/rewind_test.go (TestRewindPutsMessageBack), asksheets.go
//! (askClaude, onReply), compactsheet.go (compactBy), forksheet.go
//! (start), handoff.go (handoffTo), keys.go (replyTo), clipimage.go
//! (attachImages), entersheet.go, slimsheet.go and convo/slim.go
//! (Unused), compactby.go and handoff.go (the conversation told).

use std::sync::{Arc, Mutex};

use rush_core::event::{Event, Message, Part, PartKind, Question, TurnEnd};
use rush_core::tool::{Call, Input, Kind as ToolKind};
use rush_core::usage::{Context, ContextAgent, ContextMcpTool, ContextSkill};

use super::host::{Fake, Hosts, Op, Said, set_hosts};
use super::tests::flash;
use super::tests_subs::NOW;
use super::tests_subview::KEY;
use super::{conn, core};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::session_cards::compact::Summ;
use crate::screens::session_cards::fork::Fork;
use crate::screens::session_cards::rewind::{Branch, Rewound};
use crate::screens::session_cards::{Act, Mode};

/// The session KEY, hosted by fake and linked, its host speaking proto.
fn hosted(d: &tempfile::TempDir, fake: &Arc<Fake>, proto: i64) -> Model {
    let mut m = super::tests_subview::model(d);
    m.agents[0].job.id = "h1".into();
    m.agents[0].pid = 4242;
    set_hosts(&mut m, Arc::new(fake.clone()));
    fake.host(KEY);
    let a = m.agents[0].clone();
    let c = conn(&mut m, KEY);
    c.link = Some(fake.dial(&a).unwrap());
    c.info.proto = proto;
    m
}

fn act(m: &mut Model, a: Act) -> Out {
    (super::cards_host::host().act)(m, KEY, a)
}

fn info(m: &Model) -> crate::screens::session_cards::HostInfo {
    (super::cards_host::host().info)(m, KEY).expect("hosted")
}

fn ops(fake: &Fake) -> Vec<Op> {
    std::mem::take(&mut *fake.ops.lock().unwrap())
}

/// A conversation of n turns, each your message, a Bash call and an
/// answer.
fn turns(m: &mut Model, n: usize) {
    let s = &mut m.session.as_mut().unwrap().sess;
    *s = rush_convo::Session::new();
    for t in 1..=n {
        let at = NOW - 100_000 + t as i64 * 1_000;
        s.sent(format!("ask {t}"), Vec::new(), at);
        let call = Call {
            id: format!("c{t}"),
            name: "Bash".into(),
            kind: ToolKind::Shell,
            input: Input {
                command: format!("make step{t}"),
                ..Input::default()
            },
            ..Call::default()
        };
        let parts = vec![
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            },
            Part {
                kind: PartKind::Text,
                text: format!("did {t}"),
                ..Part::default()
            },
        ];
        s.apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: format!("m{t}"),
                parts,
                ..Message::default()
            }),
            at + 10,
        );
        s.apply(
            &Event::TurnEnd(TurnEnd {
                reason: "done".into(),
                ..TurnEnd::default()
            }),
            at + 20,
        );
    }
}

/// The host's info is what the cards read: its effort, what it goes
/// without, its modes, its open decisions, its cache, the paths its
/// rewinds left, its subagents, and its context's count with what it
/// never used.
#[test]
fn the_cards_read_the_hosts_info() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    let c = conn(&mut m, KEY);
    c.info.now.effort = "high".into();
    c.info.now.without = vec!["mcp__linear".into()];
    c.info.now.permission_modes = vec![Mode {
        id: "plan".into(),
        name: "Plan".into(),
        description: "looks first".into(),
    }];
    c.info.now.decisions = Some(Question {
        id: "d1".into(),
        ..Question::default()
    });
    c.info.cache_warm = NOW + 60_000;
    c.branches = vec![Branch {
        session_id: "s0".into(),
        turns: 2,
        ..Branch::default()
    }];
    let i = info(&m);
    assert_eq!(i.effort, "high");
    assert_eq!(i.without, ["mcp__linear"]);
    assert_eq!(i.permission_modes[0].id, "plan");
    assert_eq!(i.decisions.map(|q| q.id).as_deref(), Some("d1"));
    assert_eq!(i.cache_warm, NOW + 60_000);
    assert_eq!(i.branches[0].session_id, "s0");
    assert!(!i.subs.is_empty(), "the workflow's runs are its subagents");
    assert_eq!(i.unused, None);
    assert_eq!(i.conv_tokens, None);
    // A count of its context comes at a turn's end.
    let mut u = Context::default();
    u.messages.tool_results = 300;
    u.messages.assistant = 200;
    u.messages.user = 100;
    u.agents = vec![ContextAgent {
        kind: "Explore".into(),
        tokens: 900,
        ..ContextAgent::default()
    }];
    super::live::take(&mut m, KEY, 0, vec![Said::Context(Box::new(u))], false);
    let i = info(&m);
    assert_eq!(i.conv_tokens, Some((600, 300)));
    let unused = i.unused.expect("counted");
    assert_eq!(unused[0].name, "Explore");
    assert_eq!(unused[0].rules, ["Task(Explore)", "Agent(Explore)"]);
}

/// What a session never used: its MCP servers by what their loaded
/// tools cost (rush's own staying), subagents and skills, costliest
/// first; one it called isn't offered (convo/slim.go Unused).
#[test]
fn unused_is_what_it_never_called() {
    let mut s = rush_convo::Session::new();
    let call = |id: &str, name: &str, input: &str| Call {
        id: id.into(),
        name: name.into(),
        raw: rush_core::wire::RawJson::new(input).ok(),
        ..Call::default()
    };
    let parts = [
        call("x1", "mcp__github__search", "{}"),
        call("x2", "Skill", r#"{"skill":"pdf"}"#),
    ]
    .into_iter()
    .map(|c| Part {
        kind: PartKind::ToolCall,
        call: Some(Box::new(c)),
        ..Part::default()
    })
    .collect();
    s.sent("go".into(), Vec::new(), NOW);
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "m".into(),
            parts,
            ..Message::default()
        }),
        NOW,
    );
    let tool = |name: &str, server: &str, tokens, is_loaded| ContextMcpTool {
        name: name.into(),
        server: server.into(),
        tokens,
        is_loaded,
    };
    let mut u = Context {
        mcp_tools: vec![
            tool("search", "github", 500, true),
            tool("issues", "linear.app", 300, true),
            tool("more", "linear.app", 400, false),
            tool("draw", "rush", 900, true),
        ],
        ..Context::default()
    };
    u.skills.each = vec![
        ContextSkill {
            name: "pdf".into(),
            tokens: 50,
            ..ContextSkill::default()
        },
        ContextSkill {
            name: "xlsx".into(),
            tokens: 80,
            ..ContextSkill::default()
        },
    ];
    let got: Vec<(String, String, i64, Vec<String>)> = super::unused::unused(&s, &u)
        .into_iter()
        .map(|x| (x.what, x.name, x.tokens, x.rules))
        .collect();
    assert_eq!(
        got,
        [
            (
                "MCP server".to_string(),
                "linear.app".to_string(),
                300,
                vec!["mcp__linear_app".to_string()]
            ),
            (
                "skill".into(),
                "xlsx".into(),
                80,
                vec!["Skill(xlsx)".to_string()]
            ),
        ]
    );
}

/// A decisions card set aside and #slim's choices go to the host as its
/// ops (client.go DismissDecisions, SetWithout).
#[test]
fn decisions_and_without_go_to_the_host() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    act(&mut m, Act::DismissDecisions("d1".into()));
    act(&mut m, Act::Without(Some(vec!["Skill(pdf)".into()])));
    act(&mut m, Act::Without(None));
    assert_eq!(
        ops(&fake),
        [
            Op::DismissDecisions("d1".into()),
            Op::Without(vec!["Skill(pdf)".into()]),
            Op::Without(Vec::new()),
        ]
    );
}

/// A control request goes to the host with an id of the UI's, and its
/// answer to whoever asked; another's answer is left be; a host older
/// than protocol 2 can't be asked (askClaude, onReply).
#[test]
fn a_control_request_and_its_answer() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 1);
    let h = super::cards_host::host();
    let got: Arc<Mutex<Vec<Result<serde_json::Value, String>>>> = Arc::default();
    let done = |got: &Arc<Mutex<Vec<_>>>| -> crate::screens::session_cards::Reply {
        let got = got.clone();
        Box::new(move |_m: &mut Model, r| {
            got.lock().unwrap().push(r);
            Out::Draw
        })
    };
    let req = serde_json::json!({"subtype": "export"});
    assert!(!(h.ask)(&mut m, KEY, req.clone(), done(&got)));
    assert_eq!(
        flash(&m),
        "this agent's host is older than that: it can once the host restarts (it does after resting idle)"
    );
    conn(&mut m, KEY).info.proto = 2;
    assert!((h.ask)(&mut m, KEY, req.clone(), done(&got)));
    assert_eq!(
        ops(&fake),
        [Op::Ask {
            id: "ui-1".into(),
            req: r#"{"subtype":"export"}"#.into(),
        }]
    );
    let reply = |id: &str, body| Said::Reply {
        id: id.into(),
        body,
    };
    super::live::take(
        &mut m,
        KEY,
        0,
        vec![
            reply("other-1", Ok(serde_json::json!(1))),
            reply("ui-1", Ok(serde_json::json!({"text": "hi"}))),
        ],
        false,
    );
    assert_eq!(
        *got.lock().unwrap(),
        [Ok(serde_json::json!({"text": "hi"}))]
    );
    assert!(conn(&mut m, KEY).asks.is_empty());
    // Not hosted: it says how to make it so.
    conn(&mut m, KEY).link = None;
    assert!(!(h.ask)(&mut m, KEY, req, done(&got)));
    assert_eq!(
        flash(&m),
        "that works in rush-mode sessions · /rush moves this one over"
    );
}

/// A rewind has the host carry on from the copy, says so (and that an
/// older host restarted), lets the link go, and the message goes back in
/// the box once the host is reached again (TestRewindPutsMessageBack).
#[test]
fn a_rewind_puts_the_message_back() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    *fake.old_host.lock().unwrap() = true;
    let left = Branch {
        from: 3,
        turns: 4,
        last: "the last".into(),
        ..Branch::default()
    };
    act(
        &mut m,
        Act::Rewind(Box::new(Rewound {
            session_id: "s2".into(),
            resume: true,
            left: left.clone(),
            draft: "try it another way".into(),
            text: "rewound to before turn 3 · the path you left is kept (/rewind)".into(),
        })),
    );
    assert_eq!(
        *fake.rewound.lock().unwrap(),
        [("h1".to_string(), "s2".to_string(), true, left)]
    );
    assert_eq!(
        flash(&m),
        "rewound to before turn 3 · the path you left is kept (/rewind) · its host was from an older rush, so it restarted on this one"
    );
    // Let go and reached again: the message is in the box.
    assert!(core(&mut m).rewound.is_empty());
    assert_eq!(conn(&mut m, KEY).input.string(), "try it another way");
    assert!(conn(&mut m, KEY).hosted());
}

/// A rewind of the session beside the list gives its box the keys once
/// the message is back in it (onHostOpen's paneFocus = true): it's
/// opened.
#[test]
fn a_rewind_beside_the_list_gives_the_box_the_keys() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    m.pane = m.session.take();
    act(
        &mut m,
        Act::Rewind(Box::new(Rewound {
            session_id: "s2".into(),
            resume: true,
            left: Branch::default(),
            draft: "try it another way".into(),
            text: "rewound".into(),
        })),
    );
    assert_eq!(conn(&mut m, KEY).input.string(), "try it another way");
    assert_eq!(m.session.as_ref().map(|v| v.key.as_str()), Some(KEY));
    assert!(m.pane.is_none());
}

/// #discuss and #intervene open a room's set-up on the chat, its
/// exchanges as context (chatRoom).
#[test]
fn a_room_from_a_chat_opens_its_set_up() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    act(
        &mut m,
        Act::Room(Box::new(crate::screens::session_cards::discuss::RoomAsk {
            topic: "why is it stuck?".into(),
            from: KEY.into(),
            turns: vec!["User: go\n".into()],
            ctx: 10,
            dir: "/src/chat".into(),
            head: String::new(),
            fill: true,
        })),
    );
    let s = m
        .sheet_as::<crate::screens::social::roomsetup::RoomSetup>()
        .expect("the room's set-up");
    assert_eq!(s.from, KEY);
    assert_eq!((s.ctx, s.fill, s.dir.as_str()), (1, true, "/src/chat"));
    assert_eq!(s.topic.iter().collect::<String>(), "why is it stuck?");
}

/// A failed rewind says why and leaves the box be.
#[test]
fn a_failed_rewind_says_why() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    *fake.fail.lock().unwrap() = Some("the agent's host didn't answer the rewind".into());
    act(
        &mut m,
        Act::Rewind(Box::new(Rewound {
            session_id: "s2".into(),
            draft: "kept?".into(),
            ..Rewound::default()
        })),
    );
    assert_eq!(flash(&m), "the agent's host didn't answer the rewind");
    assert_eq!(conn(&mut m, KEY).input.string(), "");
}

/// Pruning keeps the steps the decision model says the work needs, cuts
/// the rest to their calls, and carries on from that; nothing to prune
/// says so; every step kept leaves it unchanged (compactBy's prune).
#[test]
fn a_prune_carries_on_from_whats_kept() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    let by = Summ {
        kind: "ollama".into(),
        label: "Ollama prune".into(),
        model: "tev1".into(),
        prune: true,
    };
    turns(&mut m, 2);
    act(
        &mut m,
        Act::Compact {
            by: by.clone(),
            draft: String::new(),
        },
    );
    assert_eq!(
        flash(&m),
        "nothing to prune yet: only the latest turns have tool steps"
    );
    turns(&mut m, 4);
    *fake.scores.lock().unwrap() = vec![0.9, 0.9];
    act(
        &mut m,
        Act::Compact {
            by: by.clone(),
            draft: String::new(),
        },
    );
    assert_eq!(
        flash(&m),
        "Ollama prune tev1 judged all 2 steps still needed; conversation left unchanged"
    );
    *fake.scores.lock().unwrap() = vec![0.9, 0.1];
    act(
        &mut m,
        Act::Compact {
            by,
            draft: "next".into(),
        },
    );
    let c = fake.compacted.lock().unwrap().clone();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0, "h1");
    let p = &c[0].2;
    assert!(
        p.starts_with(
            "This conversation was pruned by Ollama prune tev1 (kept 1 of 2 steps) to make room"
        ),
        "{p}"
    );
    assert!(p.contains("make step1"), "{p}");
    assert!(p.contains("(output dropped)"), "{p}");
    assert_eq!(
        flash(&m),
        "compacted by Ollama prune tev1 (kept 1 of 2 steps) · the conversation it had is kept (/rewind)"
    );
    assert_eq!(conn(&mut m, KEY).input.string(), "next");
    assert_eq!(m.session.as_ref().unwrap().sess.compacting(), None);
}

/// A model that can't summarise says so, and the dock stops saying it's
/// compacting.
#[test]
fn a_summary_needs_a_summariser() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    turns(&mut m, 1);
    m.session.as_mut().unwrap().sess.mark_compacting(NOW);
    act(
        &mut m,
        Act::Compact {
            by: Summ {
                kind: "nosuch".into(),
                label: "Nobody".into(),
                model: "x".into(),
                prune: false,
            },
            draft: String::new(),
        },
    );
    assert_eq!(flash(&m), "Nobody can't summarise");
    assert_eq!(m.session.as_ref().unwrap().sess.compacting(), None);
    assert!(fake.compacted.lock().unwrap().is_empty());
}

/// A fork and a hand-off start through the host and are picked once the
/// list has them; the hand-off opens with the conversation told.
#[test]
fn a_fork_and_a_handoff_start_sessions() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    turns(&mut m, 2);
    act(
        &mut m,
        Act::Fork(Box::new(Fork {
            name: "port it (fork)".into(),
            session_id: "s1".into(),
            fork: true,
            from: "s1".into(),
            ..Fork::default()
        })),
    );
    assert_eq!(fake.forks.lock().unwrap()[0].1.name, "port it (fork)");
    assert_eq!(flash(&m), "started port it (fork)");
    act(
        &mut m,
        Act::Handoff {
            to: "codex".into(),
            name: "port it · on Codex".into(),
        },
    );
    let h = fake.handoffs.lock().unwrap().clone();
    assert_eq!(h[0].0, "codex");
    assert_eq!(h[0].2, "port it · on Codex");
    assert!(
        h[0].3.starts_with("You're taking over a conversation"),
        "{}",
        h[0].3
    );
    assert!(h[0].3.contains("ask 1"), "{}", h[0].3);
    assert!(h[0].3.contains("did 2"), "{}", h[0].3);
    assert_eq!(flash(&m), "started port it · on Codex");
}

/// A reply goes to a rush session through its host; to a busy one rush
/// doesn't run, into its queue (replyTo).
#[test]
fn a_reply_to_another_agent() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    let mut other = m.agents[0].clone();
    other.key = "other".into();
    other.display_name = "the other".into();
    other.rush = true;
    other.job.id = "h2".into();
    m.agents.push(other.clone());
    act(
        &mut m,
        Act::ReplyTo {
            to: "other".into(),
            text: "look at this".into(),
            tagged: "look at <paste>this</paste>".into(),
        },
    );
    assert_eq!(
        *fake.sent_to.lock().unwrap(),
        [(
            "h2".to_string(),
            "look at <paste>this</paste>".to_string(),
            false
        )]
    );
    assert_eq!(flash(&m), "sent to the other");
    let o = m.agents.iter_mut().find(|a| a.key == "other").unwrap();
    o.rush = false;
    o.job.state = rush_core::agent::State::Working;
    act(
        &mut m,
        Act::ReplyTo {
            to: "other".into(),
            text: "and this".into(),
            tagged: "and this".into(),
        },
    );
    assert_eq!(flash(&m), "queued for the other · goes within 15s");
    assert_eq!(core(&mut m).local_q["other"].items, ["and this"]);
}

/// Images go into the box at the cursor as [Image #N], and go with the
/// message (attachImages, resolve).
#[test]
fn images_go_into_the_box() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    conn(&mut m, KEY).input.set("look");
    act(
        &mut m,
        Act::AttachImages(vec!["/tmp/a.png".into(), "/tmp/b.png".into()]),
    );
    assert_eq!(
        conn(&mut m, KEY).input.string(),
        "look [Image #1] [Image #2]"
    );
    super::keys::send(&mut m, KEY, false);
    let got = ops(&fake);
    assert!(
        got.iter().any(|o| matches!(o, Op::Send { text, images, .. }
            if text == "look [Image #1] [Image #2]" && images == &["/tmp/a.png", "/tmp/b.png"])),
        "{got:?}"
    );
}

/// enter's first answer, open, opens the agent now; rename puts its
/// name in the list's box to change (enterSheet's startRename).
#[test]
fn enter_on_opens_it_now() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake, 9);
    m.session = None;
    act(&mut m, Act::EnterOn(true));
    assert_eq!(m.session.as_ref().map(|v| v.key.as_str()), Some(KEY));
    m.session = None;
    act(&mut m, Act::EnterOn(false));
    let name = m.agents[0].display_name.clone();
    assert_eq!(m.input.string(), name);
    assert_eq!(crate::screens::list_extras::rename::renaming(&m), Some(KEY));
}

/// What's started in order runs as one job, in that order, its results
/// taken in that order after (tea.Sequence).
#[test]
fn in_order_runs_as_one_job() {
    let d = tempfile::tempdir().unwrap();
    let mut m = super::tests_subview::model(&d);
    let ran: Arc<Mutex<Vec<&str>>> = Arc::default();
    let r = ran.clone();
    super::in_order(&mut m, |m| {
        for w in ["stop", "send"] {
            let (r1, r2) = (r.clone(), r.clone());
            super::later(
                m,
                move || r1.lock().unwrap().push(w),
                move |_, ()| {
                    r2.lock().unwrap().push("taken");
                    Out::Idle
                },
            );
        }
        // Held until it's all started.
        assert!(r.lock().unwrap().is_empty());
        Out::Draw
    });
    assert_eq!(*ran.lock().unwrap(), ["stop", "send", "taken", "taken"]);
    assert!(core(&mut m).seq.is_none());
}

/// The conversation as another model reads it, and its first message
/// after (compactby.go PlainText, CompactedPrompt, PrunedPrompt).
#[test]
fn the_conversation_told_for_a_summary() {
    let d = tempfile::tempdir().unwrap();
    let mut m = super::tests_subview::model(&d);
    turns(&mut m, 3);
    let s = &m.session.as_ref().unwrap().sess;
    let t = super::told::plain_text(s);
    assert!(t.starts_with("## User\nask 1\n- Bash"), "{t}");
    assert!(t.contains("## Assistant\ndid 3"), "{t}");
    let (goal, steps) = super::told::prune_steps(s);
    assert!(
        goal.starts_with("User:\nask 3\n\nAssistant, latest:\ndid 3"),
        "{goal}"
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(
        super::told::compacted_prompt("Claude haiku", "did X, doing Y"),
        "This conversation was compacted by Claude haiku to make room. Here is the summary of everything before this point:\n\ndid X, doing Y\n\nTake this as what you know of the work so far. Reply only \"ready\" and wait for my next message."
    );
    let c = super::switch::conversation(s, &rush_core::adapter::Kind::from("claude"));
    assert_eq!(c.first, "ask 1");
    assert_eq!(c.steps.len(), 3);
    assert_eq!(c.history.first().map(|l| l.role.as_str()), Some("user"));
    assert_eq!(c.recent.len(), 6);
}
