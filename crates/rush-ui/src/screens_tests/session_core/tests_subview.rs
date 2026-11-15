//! Runs opened on the conversation's page (openrun.rs): beside it where
//! the column fits, else (or as Settings asks) framed under the step that
//! started it; tab moves the keys between it and the conversation, esc or
//! its ✕ closes it; a workflow opens there too, its agents rows that open
//! them. And the dock's runs under the conversation, a run's box and its
//! queue: after Go's internal/ui subview_test.go (TestWatchFromDock,
//! TestCycleSubagents), spawns_test.go (TestWatchedHostTakesTheBox) and
//! a88d13b3; where it shows is Rust-only. No sleeps: a model with no loop
//! reads at once. Models are named as the registry rush runs with names
//! them (run_tag reads the global one, left empty here).

use std::sync::Arc;

use rush_claude::headless::workflow::Workflow;
use rush_convo::Subagent;
use rush_core::adapter::Registry;
use rush_core::event::WorkflowAgent;
use rush_core::text::Line;

use super::host::{Fake, Spawned, set_hosts};
use super::subs::FlowSeen;
use super::tests_subs::{NOW, workflow_session};
use super::{conn, openrun, subview, views};
use crate::model::Model;
use crate::session::{SessionView, Source};

pub(super) const KEY: &str = "default/k1";

/// A run's transcript: it searched, then answered, with what it used.
pub(super) const X_LINES: [&str; 4] = [
    r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"review the diff"}}"#,
    r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","model":"claude-opus-4-1","role":"assistant","usage":{"input_tokens":1200,"output_tokens":300,"cache_read_input_tokens":5000},"content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"previewLines"}}]}}"#,
    r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"view.go"}]}}"#,
    r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:09Z","message":{"id":"m2","model":"claude-opus-4-1","role":"assistant","usage":{"input_tokens":10,"output_tokens":40},"content":[{"type":"text","text":"The diff is fine: one nit in view.go."}]}}"#,
];

/// A Claude Code session running a workflow, with five runs: a workflow
/// agent's failed first try, its two agents, and two runs of its own,
/// "x" read from a transcript.
pub(super) fn model(d: &tempfile::TempDir) -> Model {
    let x = d.path().join("agent-x.jsonl");
    std::fs::write(&x, X_LINES.join("\n") + "\n").unwrap();
    let reg = Registry::new();
    reg.register(rush_claude::Claude::new());
    let a = rush_core::agent::Agent {
        key: KEY.into(),
        kind: "claude".into(),
        display_name: "port it".into(),
        ..Default::default()
    };
    let mut m = crate::new_model("0.1.0");
    m.reg = reg;
    m.w = 120;
    m.h = 40;
    m.clock = Some(NOW);
    m.now = NOW;
    let mut v = SessionView::new(KEY.into(), Source::Hosted, 1);
    v.sess = workflow_session();
    v.read = true;
    m.sel = KEY.into();
    m.agents = vec![a];
    m.session = Some(v);
    m.full = true; // the Session alone
    let sub = |id: &str, typ: &str, d: &str, wf: &str, path: String, born: i64| Subagent {
        id: id.into(),
        typ: typ.into(),
        description: d.into(),
        workflow: wf.into(),
        path,
        born,
        ..Subagent::default()
    };
    let wf = |id: &str| format!("/s/subagents/workflows/wf_1/agent-{id}.jsonl");
    let subs = vec![
        sub("a0", "", "port:hl", "wf_1", wf("a0"), 0),
        sub("a1", "", "port:hl", "wf_1", wf("a1"), 1),
        sub("a2", "", "port:claude", "wf_1", wf("a2"), 2),
        sub(
            "x",
            "Explore",
            "review the diff",
            "",
            x.to_string_lossy().into_owned(),
            3,
        ),
        sub(
            "y",
            "Plan",
            "plan the next piece of the port with care",
            "",
            "/s/subagents/agent-y.jsonl".into(),
            4,
        ),
    ];
    let r = &mut conn(&mut m, KEY).runs;
    r.listed = subs.clone();
    r.subs = subs;
    r.flow_dirs
        .insert("w1".into(), "/s/subagents/workflows/wf_1".into());
    r.flows.insert(
        "/s/subagents/workflows/wf_1".into(),
        FlowSeen {
            at: NOW,
            live: NOW,
            w: Workflow {
                agents: vec![WorkflowAgent {
                    id: "a0".into(),
                    label: "port:hl".into(),
                    phase: "Port".into(),
                    state: "failed".into(),
                    ..WorkflowAgent::default()
                }],
                ..Workflow::default()
            },
        },
    );
    // Its runs' numbers, as an open run reads them.
    r.follow_all = true;
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    m
}

fn text(ls: &[Line]) -> String {
    ls.iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn sel(m: &Model) -> String {
    m.session.as_ref().unwrap().sel.clone()
}

pub(super) fn press(m: &mut Model, k: &str) {
    use rush_draw::{Key, KeyMods};
    let (mods, base) = [
        ("shift+", KeyMods::SHIFT),
        ("ctrl+", KeyMods::CTRL),
        ("alt+", KeyMods::ALT),
    ]
    .into_iter()
    .find_map(|(p, md)| k.strip_prefix(p).map(|b| (md, b)))
    .unwrap_or((KeyMods::NONE, k));
    let code = match base {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "enter" => Key::Enter,
        "esc" => Key::Esc,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "space" => Key::Char(' '),
        c => Key::Char(c.chars().next().unwrap()),
    };
    m.key(code, mods);
}

/// What's open in session key, as drawn last: its id, and its frame's
/// first cell (x, y) and size.
fn open_of(m: &mut Model) -> (Option<String>, [usize; 4]) {
    (openrun::open_id(m, KEY), conn(m, KEY).watch.frame)
}

/// Gives run x's call a step, t2's.
fn x_has_a_step(m: &mut Model) {
    for sa in conn(m, KEY).runs.subs.iter_mut() {
        if sa.id == "x" {
            sa.tool_use_id = "toolu_x".into();
        }
    }
}

/// A session's runs have no tab of their own, nor its background tasks.
#[test]
fn no_tabs_of_their_own() {
    let d = tempfile::tempdir().unwrap();
    let m = model(&d);
    assert_eq!(
        views::views(&m, KEY),
        ["conversation", "overview", "changes", "memory"]
    );
}

/// Wide (from 149, the conversation keeping 90 to 124), a run opened
/// shows beside the conversation, in the column right
/// of it, the conversation still there; it takes the keys as it shows;
/// tab gives them back and takes them again; esc closes it, back on the
/// row it was opened from.
#[test]
fn a_run_opens_beside_the_conversation() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    x_has_a_step(&mut m);
    let step = "t2:s:toolu_x";
    m.session.as_mut().unwrap().sel = step.into();
    press(&mut m, "enter");
    assert_eq!(views::view_name(&m, KEY), "conversation");
    let all = text(&m.frame());
    assert!(all.contains("The diff is fine"), "{all}");
    assert!(all.contains("tab back · esc closes"), "{all}");
    let (id, [x, y, w, h]) = open_of(&mut m);
    assert_eq!(id.as_deref(), Some("x"));
    assert!(x > 60 && x + w <= 180, "not beside it: {x} {w}\n{all}");
    // The conversation is laid out left of it: none of its rows runs on
    // under the run's edge.
    let frame = m.frame();
    for l in frame.iter().skip(y).take(h) {
        let t = l.plain();
        let cells: Vec<char> = t.chars().collect();
        if let Some(&c) = cells.get(x) {
            assert!(c == '│' || c == ' ', "a row runs under the run at {x}: {t}");
        }
    }
    assert!(conn(&mut m, KEY).watch.msg_focus, "it didn't take the keys");
    press(&mut m, "tab");
    assert!(!conn(&mut m, KEY).watch.msg_focus, "tab kept the keys");
    let all = text(&m.frame());
    assert!(all.contains("tab to it · esc closes"), "{all}");
    press(&mut m, "tab");
    assert!(conn(&mut m, KEY).watch.msg_focus, "tab took no keys");
    press(&mut m, "esc");
    assert_eq!(open_of(&mut m).0, None, "esc didn't close it");
    assert_eq!(sel(&m), step);
    // Closed from the conversation's keys too.
    press(&mut m, "enter");
    let _ = m.frame();
    press(&mut m, "tab");
    press(&mut m, "esc");
    assert_eq!(open_of(&mut m).0, None, "esc from the conversation");
}

/// How wide a run beside the conversation is, by the screen's width: the
/// conversation keeps 90 to 124, the run asks for 80 and has 56 at least,
/// else it's a peek.
#[test]
fn a_run_beside_is_as_wide_as_the_room() {
    for (w, want) in [
        (120, 0),
        (148, 0),
        (149, 56),
        (150, 57),
        (180, 80),
        (220, 93),
        (260, 133),
    ] {
        assert_eq!(openrun::run_width(w), want, "{w}");
    }
}

/// Narrow, it's a peek: framed under its step in the conversation, at
/// the conversation's width, the rest going on under it.
#[test]
fn narrow_a_run_peeks_under_its_step() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 90;
    x_has_a_step(&mut m);
    m.session.as_mut().unwrap().sel = "t2:s:toolu_x".into();
    press(&mut m, "enter");
    let frame = m.frame();
    let all = text(&frame);
    assert!(all.contains("The diff is fine"), "{all}");
    let (id, [x, y, w, h]) = open_of(&mut m);
    assert_eq!(id.as_deref(), Some("x"));
    assert_eq!(x, 2, "not in the conversation:\n{all}");
    assert!(w > 60 && h >= 10, "{w}×{h}:\n{all}");
    let top = frame[y].plain();
    assert!(top.contains('╭') && top.contains('✕'), "its edge: {top}");
}

/// Settings can ask for the peek, however wide.
#[test]
fn the_setting_asks_for_the_peek() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 220;
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::config_mut(&mut m).runs_open = openrun::PEEK.into();
    subview::pick_run(&mut m, KEY, "x");
    let all = text(&m.frame());
    assert!(all.contains("The diff is fine"), "{all}");
    assert_eq!(open_of(&mut m).1[0], 2, "beside it:\n{all}");
}

/// A subagent runs in its parent's process, under its settings: its
/// context is counted against the window the parent compacts within
/// (CLAUDE_CODE_AUTO_COMPACT_WINDOW=400000), not the model's 1M.
#[test]
fn a_runs_context_is_of_its_parents_compact_window() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 220;
    m.agents[0].compaction = rush_core::adapter::Compaction {
        window: 400_000,
        headroom: 33_000,
    };
    subview::pick_run(&mut m, KEY, "x");
    let all = text(&m.frame());
    let ctx = all
        .lines()
        .find(|l| l.contains("context "))
        .unwrap_or_else(|| panic!("no context:\n{all}"));
    assert!(ctx.contains(" of 400k"), "{ctx}");
    assert!(!ctx.contains(" of 1M"), "{ctx}");
}

/// A click on its ✕ closes it; a click elsewhere gives the keys back to
/// the conversation, the run left open.
#[test]
fn its_x_closes_it() {
    use crate::frame::Mouse;
    use rush_draw::{Button, MouseKind};
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    subview::pick_run(&mut m, KEY, "x");
    let _ = m.frame();
    let click = |m: &mut Model, x: usize, y: usize| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Down(Button::Left),
            x: x as u16,
            y: y as u16,
            mods: Default::default(),
        });
    };
    click(&mut m, 4, 6);
    assert!(!conn(&mut m, KEY).watch.msg_focus, "the run kept the keys");
    assert_eq!(open_of(&mut m).0.as_deref(), Some("x"));
    let _ = m.frame();
    let (cx, cy) = conn(&mut m, KEY).watch.close_at;
    click(&mut m, cx, cy);
    assert_eq!(open_of(&mut m).0, None, "✕ didn't close it");
}

/// From the dock: ↑ picks its last run, space opens it, and esc comes back
/// to the conversation on that run's row (watchRun, closeSub, dockRefs).
#[test]
fn from_the_dock() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    // The dock works out what it shows as it's drawn.
    super::running::sync(&mut m, KEY);
    let refs = subview::dock_refs(&mut m, KEY);
    assert_eq!(refs, ["job:w1", "run:a2"], "the dock's rows");
    press(&mut m, "up");
    assert_eq!(sel(&m), "run:a2");
    press(&mut m, "up");
    assert_eq!(sel(&m), "job:w1");
    press(&mut m, "down");
    assert_eq!(sel(&m), "run:a2");
    m.session.as_mut().unwrap().sel = "run:x".into();
    press(&mut m, "space");
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("x"));
    let _ = m.frame();
    press(&mut m, "esc");
    assert!(!openrun::is_open(&m, KEY));
    assert_eq!(sel(&m), "run:x");
    // ↓ past the dock's last goes back to the box.
    m.session.as_mut().unwrap().sel = "run:a2".into();
    press(&mut m, "down");
    assert_eq!(sel(&m), "");
}

/// enter or → on a step a subagent made opens that run, its pane with the
/// keys, and esc comes back to the step (rushmode.go:4270 subOfStep,
/// closeSub).
#[test]
fn enter_on_a_subagents_step_opens_it() {
    for k in ["enter", "right"] {
        let d = tempfile::tempdir().unwrap();
        let mut m = model(&d);
        for sa in conn(&mut m, KEY).runs.subs.iter_mut() {
            if sa.id == "x" {
                sa.tool_use_id = "toolu_x".into();
            }
        }
        m.session.as_mut().unwrap().sel = "t2:s:toolu_x".into();
        press(&mut m, k);
        assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("x"), "{k}");
        let body = text(&m.frame());
        assert!(body.contains("The diff is fine"), "{k}:\n{body}");
        assert!(conn(&mut m, KEY).watch.msg_focus, "{k}: its pane");
        press(&mut m, "esc");
        assert!(!openrun::is_open(&m, KEY), "{k}");
        assert_eq!(sel(&m), "t2:s:toolu_x", "{k}");
    }
}

/// enter on a call of rush's agent tools opens the agent it's about,
/// whichever harness made the call: the one
/// a spawn_agent call started, the one an agent_result or agent_send call
/// names by its id.
#[test]
fn enter_on_an_agent_tools_step_picks_its_agent() {
    use rush_core::event::{Event, Message, Part, PartKind};
    use rush_core::tool::Call;
    let call = |id: &str, name: &str, input: &str| {
        let c = Call {
            id: id.into(),
            name: name.into(),
            raw: Some(rush_core::wire::RawJson::new(input).unwrap()),
            ..Call::default()
        };
        Event::Message(Message {
            role: "assistant".into(),
            id: format!("m-{id}"),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(c)),
                ..Part::default()
            }],
            ..Message::default()
        })
    };
    for step in ["a1", "a2", "a3"] {
        let d = tempfile::tempdir().unwrap();
        let mut m = model(&d);
        let p = d.path().join("spawned.jsonl");
        std::fs::write(
            &p,
            X_LINES
                .join("\n")
                .replace("\"isSidechain\":true", "\"isSidechain\":false")
                + "\n",
        )
        .unwrap();
        let s = &mut m.session.as_mut().unwrap().sess;
        for (id, name, input) in [
            (
                "a1",
                "mcp__rush__spawn_agent",
                r#"{"agent":"astra","prompt":"review the diff","background":true}"#,
            ),
            (
                "a2",
                "mcp__rush__agent_result",
                r#"{"id":"h9","wait_seconds":600}"#,
            ),
            (
                "a3",
                "mcp__rush__agent_send",
                r#"{"id":"h9","prompt":"and the metrics"}"#,
            ),
        ] {
            s.apply(&call(id, name, input), NOW - 5_000);
        }
        conn(&mut m, KEY).runs.spawns.insert(
            "h9".into(),
            crate::spawns::SpawnRun {
                step: "a1".into(),
                kind: "claude".into(),
                name: "Claude Code".into(),
                prompt: "review the diff".into(),
                path: p.to_string_lossy().into_owned(),
                hosted: "h9".into(),
                live: true,
                ..Default::default()
            },
        );
        super::subs::kick(&mut m, KEY);
        super::subs::refresh(&mut m, KEY);
        let picked = format!("t9:s:{step}");
        m.session.as_mut().unwrap().sel.clone_from(&picked);
        press(&mut m, "enter");
        assert_eq!(
            openrun::open_id(&m, KEY).as_deref(),
            Some("spawn:h9"),
            "{step}"
        );
        let _ = m.frame();
        press(&mut m, "esc");
        assert_eq!(sel(&m), picked, "{step}");
    }
}

/// Rust-only: an Agent call another model answered (rush-claude's
/// others.rs) is one run, the model's that rush hosts ("top · astra"):
/// Claude Code's Haiku stand-in for it, listed beside its transcript
/// under the same call, is no row of its own, and enter on the call
/// opens the model's conversation.
#[test]
fn a_call_another_model_answered_is_one_run() {
    use rush_core::event::{Event, Message, Part, PartKind};
    use rush_core::tool::Call;
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let p = d.path().join("hosted.jsonl");
    std::fs::write(
        &p,
        X_LINES
            .join("\n")
            .replace("\"isSidechain\":true", "\"isSidechain\":false")
            + "\n",
    )
    .unwrap();
    let stand_in = d.path().join("agent-stand.jsonl");
    std::fs::write(&stand_in, X_LINES.join("\n") + "\n").unwrap();
    let c = Call {
        id: "n1".into(),
        name: "Agent".into(),
        raw: Some(
            rush_core::wire::RawJson::new(
                r#"{"subagent_type":"top","description":"Design the cache","prompt":"design it"}"#,
            )
            .unwrap(),
        ),
        ..Call::default()
    };
    m.session.as_mut().unwrap().sess.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "m-n1".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(c)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        NOW - 5_000,
    );
    let r = &mut conn(&mut m, KEY).runs;
    r.listed.push(Subagent {
        id: "stand".into(),
        typ: "top".into(),
        description: "Design the cache".into(),
        model: "claude-haiku-4-5".into(),
        tool_use_id: "n1".into(),
        path: stand_in.to_string_lossy().into_owned(),
        born: NOW - 4_900,
        ..Subagent::default()
    });
    r.spawns.insert(
        "h7".into(),
        crate::spawns::SpawnRun {
            step: "n1".into(),
            kind: "codex".into(),
            name: "top · astra".into(),
            model: "gpt-6-astra".into(),
            prompt: "design it".into(),
            path: p.to_string_lossy().into_owned(),
            hosted: "h7".into(),
            live: true,
            ..Default::default()
        },
    );
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    let rows: Vec<(String, String)> = conn(&mut m, KEY)
        .runs
        .subs
        .iter()
        .filter(|sa| sa.tool_use_id == "n1")
        .map(|sa| (sa.id.clone(), sa.typ.clone()))
        .collect();
    assert_eq!(rows, [("spawn:h7".to_string(), "top · astra".to_string())]);
    // The session's other runs stay.
    assert!(conn(&mut m, KEY).runs.subs.iter().any(|sa| sa.id == "x"));
    let picked = "t9:s:n1".to_string();
    m.session.as_mut().unwrap().sel.clone_from(&picked);
    press(&mut m, "enter");
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("spawn:h7"));
}

/// space on the dock's workflow opens it on the conversation's page: its
/// phases and agents, the keys left with the conversation (openWorkflow).
#[test]
fn a_workflow_from_the_dock() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    super::running::sync(&mut m, KEY);
    m.session.as_mut().unwrap().sel = "job:w1".into();
    press(&mut m, "space");
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("job:w1"));
    assert_eq!(views::view_name(&m, KEY), "conversation");
    let all = text(&m.frame());
    assert!(all.contains("click one of its agents to open it"), "{all}");
    assert!(all.contains("port:claude"), "{all}");
    assert!(!conn(&mut m, KEY).watch.msg_focus);
}

/// A click on one of an open workflow's agents opens that agent; esc,
/// closing it, goes back to the workflow.
#[test]
fn a_workflows_agent_opens_from_its_row() {
    use crate::frame::Mouse;
    use rush_draw::{Button, MouseKind};
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    super::running::sync(&mut m, KEY);
    subview::open_workflow(&mut m, KEY, "job:w1");
    let all = text(&m.frame());
    let w = &conn(&mut m, KEY).watch;
    let [fx, fy, _, _] = w.flow_at;
    let i = w
        .flow_refs
        .iter()
        .position(|r| r == "a2")
        .unwrap_or_else(|| panic!("no row for a2: {:?}\n{all}", w.flow_refs));
    let (x, y) = (fx + 4, fy + i);
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: x as u16,
        y: y as u16,
        mods: Default::default(),
    });
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("a2"));
    openrun::close(&mut m, KEY);
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("job:w1"));
}

/// shift+↓ steps from the conversation through the runs, working ones
/// first, then the rest newest first, each opened in turn, and back to the
/// conversation alone (cycleSub).
#[test]
fn cycling_the_runs() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let mut seen = Vec::new();
    for _ in 0..6 {
        press(&mut m, "shift+down");
        seen.push(openrun::open_id(&m, KEY).unwrap_or_else(|| "conversation".into()));
    }
    assert_eq!(seen, ["a2", "y", "x", "a1", "a0", "conversation"]);
}

/// Where a run works is where its own calls reached, not the folder its
/// parent was in when it started it.
#[test]
fn a_runs_branch_is_where_it_worked() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    subview::pick_run(&mut m, KEY, "x");
    // Opening it reads its transcript afresh; where it worked is set after.
    let _ = m.frame();
    let r = &mut conn(&mut m, KEY).runs;
    r.stats.get_mut("x").unwrap().worktree = "perf-memlist".into();
    let got = text(&m.frame());
    assert!(got.contains("⎇ perf-memlist"), "{got}");
    // A subagent's transcript says only its parent's folder: rush doesn't
    // ask it for one.
    assert!(conn(&mut m, KEY).runs.wt.is_empty());
}

/// A workflow run is told apart by when it started.
#[test]
fn a_workflow_run_says_when_it_started() {
    let at = NOW - 3 * 3_600_000;
    let c = crate::screens::settings::paint::civil(at);
    assert_eq!(
        subview::started(at, NOW),
        format!("{:02}:{:02}", c.hour, c.minute)
    );
    let old = NOW - 3 * 24 * 3_600_000;
    assert!(subview::started(old, NOW).contains(' '));
    assert_eq!(subview::started(0, NOW), "");
}

/// A spawned agent rush hosts, picked: what's typed in its box goes to
/// its own host, queued for its turn's end or sent now (watchedHost,
/// a88d13b3).
#[test]
fn a_spawned_agents_box_goes_to_its_host() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = model(&d);
    m.w = 180;
    set_hosts(&mut m, Arc::new(fake.clone()));
    let p = d.path().join("spawned.jsonl");
    std::fs::write(
        &p,
        X_LINES[1..]
            .join("\n")
            .replace("\"isSidechain\":true", "\"isSidechain\":false")
            + "\n",
    )
    .unwrap();
    conn(&mut m, KEY).runs.spawns.insert(
        "s1".into(),
        crate::spawns::SpawnRun {
            step: "t9".into(),
            kind: "claude".into(),
            name: "Claude Code".into(),
            prompt: "look at the logs".into(),
            path: p.to_string_lossy().into_owned(),
            hosted: "h9".into(),
            live: true,
            ..Default::default()
        },
    );
    let _ = Spawned::default();
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    subview::pick_from(&mut m, KEY, "spawn:s1", "run:spawn:s1");
    let _ = m.frame();
    // Opened, its box has the keys as it shows, and keeps
    // them as the run is drawn again (Rust-only).
    assert!(
        conn(&mut m, KEY).watch.msg_focus,
        "picking it didn't give its box the keys"
    );
    let _ = m.frame();
    assert!(
        conn(&mut m, KEY).watch.msg_focus,
        "lost as it was drawn again"
    );
    typed(&mut m, "and the metrics");
    assert!(
        conn(&mut m, KEY).input.is_empty(),
        "it went to the main box"
    );
    press(&mut m, "ctrl+enter");
    assert_eq!(
        fake.sent_to.lock().unwrap().clone(),
        [("h9".to_string(), "and the metrics".to_string(), true)]
    );
    // tab gives the keys back, and takes them again with the main box
    // typed in.
    press(&mut m, "tab");
    assert!(!conn(&mut m, KEY).watch.msg_focus);
    conn(&mut m, KEY).input.insert_str("for the parent");
    press(&mut m, "tab");
    assert!(
        conn(&mut m, KEY).watch.msg_focus,
        "tab with the main box typed in"
    );
    // Its run gone from what rush hosts, the box lets the keys go, and
    // what's typed goes nowhere else.
    conn(&mut m, KEY).input.clear();
    conn(&mut m, KEY).runs.spawns.clear();
    press(&mut m, "x");
    assert!(
        !conn(&mut m, KEY).watch.msg_focus,
        "a box with no route kept the keys"
    );
    assert!(
        conn(&mut m, KEY).input.is_empty(),
        "it went to the main box"
    );
}

/// A click on the dock's run, on the transcript's end, opens it; on its
/// workflow, opens that with its agents (clickRef :5102).
#[test]
fn clicking_the_docks_runs() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    super::running::sync(&mut m, KEY);
    super::dock::dock(&mut m, 120);
    let refs = m.session.as_ref().unwrap().panel_refs.clone();
    assert!(refs.iter().any(|r| r == "run:a2"), "{refs:?}");
    assert!(subview::click_ref(&mut m, KEY, "run:a2"));
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("a2"));
    assert!(subview::click_ref(&mut m, KEY, "job:w1"));
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("job:w1"));
    assert!(!subview::click_ref(&mut m, KEY, "3:s:t1"));
}

/// A live subagent picked: tab gives its box the keys, and what's typed
/// waits in its own queue and goes into its inbox once it has taken a
/// step; ctrl+enter sends it all now, and a note under its conversation
/// says where it went; tab gives the keys back (queueSub, flushSubQueues,
/// sendSubQueueNow, subNoteLines).
#[test]
fn a_live_subagent_takes_the_box() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    press(&mut m, "tab");
    assert!(
        conn(&mut m, KEY).watch.msg_focus,
        "tab didn't give the box the keys"
    );
    let _ = m.frame();
    let hint = crate::sess::state(&mut m).hint.clone().unwrap().plain();
    assert!(hint.starts_with("enter send to review the diff"), "{hint}");
    typed(&mut m, "check the tests too");
    press(&mut m, "enter");
    let flash = m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default();
    assert_eq!(
        flash,
        "queued for Explore · it reads it when the step it's on ends"
    );
    assert!(fake.ops().is_empty(), "sent before its step ended");
    // It takes a step: the message goes into its inbox.
    conn(&mut m, KEY).runs.stats.get_mut("x").unwrap().steps += 1;
    super::relay::flush(&mut m, KEY);
    assert_eq!(
        fake.ops(),
        [super::host::Op::Tell {
            sub: "x".into(),
            text: "check the tests too".into()
        }]
    );
    // ctrl+enter: now.
    typed(&mut m, "and the docs");
    press(&mut m, "ctrl+enter");
    assert_eq!(fake.ops().len(), 2);
    let notes = text(&super::relay::note_lines(&mut m, KEY, "x", 120));
    assert!(notes.contains("↪ and the docs · in its inbox"), "{notes}");
    press(&mut m, "tab");
    assert!(
        !conn(&mut m, KEY).watch.msg_focus,
        "tab didn't give the keys back"
    );
}

/// The subagent's box keeps a Session's queue: what's queued shows above
/// it; ↑ picks it, [ ] move, ⌫ drops, space edits it in the box (the
/// queue held meanwhile) and enter saves it back, s sends it now.
#[test]
fn a_subagents_queue_shows_and_edits_like_a_sessions() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    press(&mut m, "tab");
    for w in ["one", "two", "three"] {
        typed(&mut m, w);
        press(&mut m, "enter");
    }
    let items = |m: &mut Model| super::subbox::queue_of(m, KEY, "x").items;
    assert_eq!(items(&mut m), ["one", "two", "three"]);
    let all = text(&m.frame());
    assert!(all.contains("queue ·3"), "{all}");
    assert!(fake.ops().is_empty(), "sent before its step ended");
    // ↑ picks the last; [ moves it up, the pick with it; ⌫ drops it,
    // and the pick goes to the one after.
    press(&mut m, "up");
    press(&mut m, "[");
    assert_eq!(items(&mut m), ["one", "three", "two"]);
    press(&mut m, "backspace");
    assert_eq!(items(&mut m), ["one", "two"]);
    // space edits it in the box; the queue holds till enter saves it.
    press(&mut m, "space");
    assert!(super::subbox::queue_of(&mut m, KEY, "x").held);
    typed(&mut m, "!");
    press(&mut m, "enter");
    assert_eq!(items(&mut m), ["one", "two!"]);
    assert!(!super::subbox::queue_of(&mut m, KEY, "x").held);
    // s sends the picked one now, ahead of the rest.
    press(&mut m, "up");
    press(&mut m, "up");
    press(&mut m, "s");
    assert_eq!(
        fake.ops(),
        [super::host::Op::Tell {
            sub: "x".into(),
            text: "one".into()
        }]
    );
    assert_eq!(items(&mut m), ["two!"]);
}

/// Each run keeps what's typed for it; one that didn't go comes back to
/// its own box.
#[test]
fn each_run_keeps_its_own_draft() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    press(&mut m, "tab");
    typed(&mut m, "half done");
    let draft = |m: &mut Model, id: &str| super::subbox::draft(m, KEY, id).input.string();
    assert_eq!(draft(&mut m, "x"), "half done");
    assert_eq!(draft(&mut m, "y"), "");
    super::subbox::give_back(&mut m, KEY, "y", "lost");
    assert_eq!(draft(&mut m, "y"), "lost");
    assert_eq!(draft(&mut m, "x"), "half done");
}

/// Session key's run x working in the background, in a session rush runs
/// through fake, open beside the conversation, the keys given back to the
/// conversation.
pub(super) fn picked_beside(d: &tempfile::TempDir, fake: &Arc<Fake>, inbox: bool) -> Model {
    use super::host::Hosts;
    use rush_core::event::{Event, TaskKind, TaskStarted};
    let mut m = model(d);
    m.w = 220;
    set_hosts(&mut m, Arc::new(fake.clone()));
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskStarted(TaskStarted {
            id: "x".into(),
            call_id: "tx".into(),
            kind: TaskKind::Subagent,
            label: "review the diff".into(),
            background: true,
            ..TaskStarted::default()
        }),
        NOW,
    );
    let a = m.agents[0].clone();
    let c = conn(&mut m, KEY);
    c.link = Some(fake.dial(&a).unwrap());
    c.info.inbox = inbox;
    subview::pick_run(&mut m, KEY, "x");
    let _ = m.frame();
    conn(&mut m, KEY).watch.msg_focus = false;
    let _ = m.frame();
    m
}

pub(super) fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        let k = if c == ' ' {
            "space".into()
        } else {
            c.to_string()
        };
        press(m, &k);
    }
}

/// The box under a run open: tab gives it the keys, and what's
/// typed goes to the run, not the main session: into its inbox where its
/// host has one, else passed on by the main session with SendMessage,
/// which the box says.
#[test]
fn the_box_under_the_run_messages_it() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    let all = text(&m.frame());
    assert!(all.contains("tab or click here to message it"), "{all}");
    assert!(all.contains("into its inbox"), "{all}");
    press(&mut m, "tab");
    assert!(conn(&mut m, KEY).watch.msg_focus);
    typed(&mut m, "check tests");
    assert!(
        conn(&mut m, KEY).input.is_empty(),
        "it went to the main box"
    );
    press(&mut m, "ctrl+enter");
    assert_eq!(
        fake.ops(),
        [super::host::Op::Tell {
            sub: "x".into(),
            text: "check tests".into()
        }]
    );
    press(&mut m, "tab");
    assert!(!conn(&mut m, KEY).watch.msg_focus);

    // No inbox: the main session passes it on, and the box says so.
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, false);
    let all = text(&m.frame());
    assert!(all.contains("passes it on with SendMessage"), "{all}");
    press(&mut m, "tab");
    typed(&mut m, "stop");
    press(&mut m, "enter");
    match &fake.ops()[..] {
        [super::host::Op::Guide { text, .. } | super::host::Op::Send { text, .. }] => assert!(
            text.starts_with("Pass this message on to your running subagent x (Explore)")
                && text.ends_with("The message: stop"),
            "{text}"
        ),
        ops => panic!("{ops:?}"),
    }
}

/// A session rush doesn't run can't pass a message on: the box says why;
/// its pane still takes the keys for its conversation, and what's typed
/// goes nowhere, the main box neither.
#[test]
fn the_box_says_why_nothing_can_reach_the_run() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    subview::pick_run(&mut m, KEY, "x");
    let all = text(&m.frame());
    assert!(all.contains("the session isn't running in rush"), "{all}");
    // Opened, its pane has the keys: it has a conversation to read.
    assert!(conn(&mut m, KEY).watch.msg_focus);
    typed(&mut m, "hi");
    assert!(
        conn(&mut m, KEY).input.is_empty(),
        "it went to the main box"
    );
    let flash = m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default();
    assert!(flash.contains("isn't running in rush"), "{flash}");
    // It stays open.
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("x"));
}

/// A workflow resumed after a restart lists an agent once a try: the
/// phase counts it once, by its latest try (the one running, else the
/// last), and the others are its earlier tries.
#[test]
fn a_phase_counts_an_agent_once_however_often_it_was_tried() {
    use super::workflow::{Phase, workflow_strip};
    let a = |id: &str, label: &str, state: &str| WorkflowAgent {
        id: id.into(),
        label: label.into(),
        phase: "Port".into(),
        state: state.into(),
        ..WorkflowAgent::default()
    };
    let p = Phase {
        title: "Port".into(),
        agents: vec![
            a("k", "port:keys", "done"),
            a("l1", "port:listkeys", "done"),
            a("h", "port:hash", "done"),
            a("l2", "port:listkeys", "running"),
            a("l3", "port:listkeys", "done"),
        ],
    };
    let cur: Vec<&str> = p.current().iter().map(|a| a.id.as_str()).collect();
    assert_eq!(cur, ["k", "l2", "h"]);
    let earlier: Vec<&str> = p
        .earlier(p.current()[1])
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(earlier, ["l1", "l3"]);
    assert!(workflow_strip(&[p], 0).plain().contains("Port 2/3"));
}

/// A working run's transcript: asked, it ran a command it hasn't heard
/// back from, NOW less 36.789s.
const W_LINES: [&str; 2] = [
    r#"{"type":"user","isSidechain":true,"timestamp":"2026-10-28T12:34:00Z","message":{"role":"user","content":"check the port's tests"}}"#,
    r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-28T12:34:20Z","message":{"id":"m1","model":"claude-opus-4-1","role":"assistant","usage":{"input_tokens":900,"output_tokens":60},"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test","description":"run the tests"}}]}}"#,
];

/// model's runs and one more, w, working: its task running, its
/// transcript at a command still going.
fn working(d: &tempfile::TempDir) -> Model {
    use rush_core::event::{Event, TaskKind, TaskStarted};
    let mut m = model(d);
    let p = d.path().join("agent-w.jsonl");
    std::fs::write(&p, W_LINES.join("\n") + "\n").unwrap();
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskStarted(TaskStarted {
            id: "w".into(),
            call_id: "tw".into(),
            kind: TaskKind::Subagent,
            label: "check the port's tests".into(),
            background: true,
            ..TaskStarted::default()
        }),
        NOW - 60_000,
    );
    let sa = Subagent {
        id: "w".into(),
        typ: "general-purpose".into(),
        description: "check the port's tests".into(),
        tool_use_id: "tw".into(),
        path: p.to_string_lossy().into_owned(),
        born: 5,
        ..Subagent::default()
    };
    let r = &mut conn(&mut m, KEY).runs;
    r.listed.push(sa.clone());
    r.subs.push(sa);
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    m
}

/// While a run works, it shows the session's working line docked under
/// its conversation; once it finishes, the line goes.
#[test]
fn a_working_run_shows_its_working_line() {
    use rush_core::event::{Event, TaskDone};
    let d = tempfile::tempdir().unwrap();
    let mut m = working(&d);
    m.w = 180;
    subview::pick_run(&mut m, KEY, "w");
    let _ = m.frame();
    let got = text(&m.frame());
    assert!(got.contains("· running Bash…"), "no working line:\n{got}");
    assert!(got.contains("· check the port's tests"), "its head:\n{got}");
    assert!(got.contains("to check the port's tests"), "its box:\n{got}");
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskDone(TaskDone {
            id: "w".into(),
            call_id: "tw".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    let got = text(&m.frame());
    assert!(!got.contains("running Bash"), "finished:\n{got}");
}

/// What the picked run follows, by id: None when nothing is.
fn followed(m: &mut Model) -> Option<String> {
    conn(m, KEY).runs.live.as_ref().map(|f| f.id.clone())
}

/// w open beside the conversation, 180 wide.
fn picked_working(d: &tempfile::TempDir) -> Model {
    let mut m = working(d);
    m.w = 180;
    subview::pick_run(&mut m, KEY, "w");
    let _ = m.frame();
    m
}

/// A running subagent open is followed: what its transcript gains shows
/// after one wake of its watch, its conversation's new step, with no wait
/// for the next refresh.
#[test]
fn a_picked_running_subagent_is_followed() {
    use std::io::Write;
    let d = tempfile::tempdir().unwrap();
    let mut m = picked_working(&d);
    assert_eq!(followed(&mut m).as_deref(), Some("w"));
    let p = d.path().join("agent-w.jsonl");
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    for l in [
        r#"{"type":"user","isSidechain":true,"timestamp":"2026-10-28T12:34:30Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"b1","content":"ok"}]}}"#,
        r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-28T12:34:31Z","message":{"id":"m2","model":"claude-opus-4-1","role":"assistant","usage":{"input_tokens":950,"output_tokens":40},"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"cargo clippy","description":"lint it"}}]}}"#,
    ] {
        writeln!(f, "{l}").unwrap();
    }
    drop(f);
    // One wake, as the watch on its file gives when it grows.
    let _ = super::subfollow::woke(&mut m, KEY, "w");
    let got = text(&m.frame());
    assert!(got.contains("lint it"), "its new step:\n{got}");
}

/// Closed, nothing is followed; a wake from a follow left over lets it
/// go.
#[test]
fn a_closed_run_is_not_followed() {
    let d = tempfile::tempdir().unwrap();
    let mut m = picked_working(&d);
    assert_eq!(followed(&mut m).as_deref(), Some("w"));
    openrun::close(&mut m, KEY);
    let _ = super::subfollow::woke(&mut m, KEY, "w");
    assert_eq!(followed(&mut m), None);
    super::subs::refresh(&mut m, KEY);
    assert_eq!(followed(&mut m), None);
}

/// A finished subagent isn't followed: opened once done, or let go as it
/// finishes.
#[test]
fn a_finished_subagent_is_not_followed() {
    use rush_core::event::{Event, TaskDone};
    let d = tempfile::tempdir().unwrap();
    let mut m = picked_working(&d);
    subview::pick_run(&mut m, KEY, "x");
    let _ = m.frame();
    assert_eq!(followed(&mut m), None, "x is done");
    subview::pick_run(&mut m, KEY, "w");
    let _ = m.frame();
    assert_eq!(followed(&mut m).as_deref(), Some("w"));
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskDone(TaskDone {
            id: "w".into(),
            call_id: "tw".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    let _ = m.frame();
    assert_eq!(followed(&mut m), None, "w finished");
}
