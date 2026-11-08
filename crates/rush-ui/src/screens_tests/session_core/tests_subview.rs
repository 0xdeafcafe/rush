//! The subagents view, a run picked with the list (beside it or under it)
//! and the dock's runs under ↑: after Go's internal/ui workflow_test.go
//! (TestWorkflowRows' list), subview_test.go (TestSubagentsView,
//! TestWatchFromDock, TestCycleSubagents), spawns_test.go
//! (TestWatchedHostTakesTheBox) and a88d13b3. No sleeps: a model with no
//! loop reads at once. Models are named as the registry rush runs with
//! names them (run_tag reads the global one, left empty here): the
//! parity check against Go's frames names them as Go does.

use std::sync::Arc;

use rush_claude::headless::workflow::Workflow;
use rush_convo::Subagent;
use rush_core::adapter::Registry;
use rush_core::event::WorkflowAgent;
use rush_core::text::Line;

use super::host::{Fake, Spawned, set_hosts};
use super::subs::FlowSeen;
use super::tests_subs::{NOW, workflow_session};
use super::{conn, subview, views};
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
    // Its runs' numbers, as the subagents view reads them.
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

/// The list: a count, then under their headings the workflow running,
/// its phases and its agents' runs a line each, an agent's earlier try
/// folded under the try that replaced it; then the runs finished, the
/// one picked lit, its facts with it under the list (TestWorkflowRows'
/// list, Go's subagentList at 120, made one line a run).
#[test]
fn the_list() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let rows: Vec<Line> = subview::list(&mut m, KEY, 120, false)
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    let got = text(&rows);
    // The workflow's start, in local time.
    let start = m.session.as_ref().unwrap().sess.jobs()[0].start;
    let at = subview::started(start, NOW);
    let want: Vec<String> = vec![
        "  1 workflow running · 3 runs in all  ·  click one to see it below".into(),
        "".into(),
        "  Running  1".into(),
        format!("  · ⋔ Port rush to Rust · {at}   · Port 1/3  ›  · Check                                  running                   5m"),
        "    · Port  1 of 3 done".into(),
        "      ✓ port:hl                                                                           done".into(),
        "        ▸ 1 earlier try · click to show".into(),
        "      ✢ port:claude                                                                       · working…".into(),
        "      ◌ port:draw    waiting to start".into(),
        "    · Check  not started".into(),
        "".into(),
        "  Finished  2".into(),
        "  ✓ plan the next piece of the port with care                                             done".into(),
        "▍ ✓ review the diff                                                                       done                      9s".into(),
    ];
    let want = want.join("\n");
    assert_eq!(got, want);
}

/// A session with runs has a Subagents tab, saying how many run.
#[test]
fn the_tab() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    assert_eq!(
        views::views(&m, KEY),
        // Its workflow runs in the background: the background view too.
        [
            "conversation",
            "overview",
            "changes",
            "subagents",
            "background",
            "memory"
        ]
    );
    assert_eq!(subview::tab_stat(&m, KEY).plain(), "1 running");
    assert!(views::show_view(&mut m, KEY, "subagents"));
}

/// There's no view of one run alone: enter or space on a run in the list
/// leaves it picked in the list, its conversation with it (Go's openSub
/// opened it whole under a banner).
#[test]
fn a_run_never_opens_alone() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let _ = views::body(&mut m, 120, 20);
    for k in ["enter", "space"] {
        press(&mut m, k);
        assert_eq!(views::view_name(&m, KEY), "subagents", "{k}");
        assert_eq!(sel(&m), "sub:x", "{k}");
    }
    let body = text(&views::body(&mut m, 120, 20).unwrap());
    assert!(!body.contains("‹ subagents"), "{body}");
    assert!(
        body.contains("The diff is fine: one nit in view.go."),
        "{body}"
    );
}

/// A run known only by its task has no conversation to show: it says
/// where its step is.
#[test]
fn a_run_known_by_its_task() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    subview::pick_run(&mut m, KEY, "job:t9");
    assert_eq!(views::view_name(&m, KEY), "subagents");
    let body = text(&views::body(&mut m, 170, 20).unwrap());
    assert!(
        body.contains("its conversation isn't found yet; its step is in the main conversation"),
        "{body}"
    );
}

/// From the dock: ↑ picks its last run, space shows it picked in the
/// subagents view, and esc comes back to the conversation on that run's
/// row (watchRun, closeSub, dockRefs).
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
    assert_eq!(views::view_name(&m, KEY), "subagents");
    assert_eq!(sel(&m), "sub:x");
    press(&mut m, "esc");
    assert_eq!(views::view_name(&m, KEY), "conversation");
    assert_eq!(sel(&m), "run:x");
    // ↓ past the dock's last goes back to the box.
    m.session.as_mut().unwrap().sel = "run:a2".into();
    press(&mut m, "down");
    assert_eq!(sel(&m), "");
}

/// enter or → on a step a subagent made shows the subagents view with
/// that run picked, and esc comes back to the step (rushmode.go:4270
/// subOfStep, closeSub).
#[test]
fn enter_on_a_subagents_step_picks_it() {
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
        assert_eq!(views::view_name(&m, KEY), "subagents", "{k}");
        assert_eq!(sel(&m), "sub:x", "{k}");
        let body = text(&views::body(&mut m, 150, 24).unwrap());
        assert!(body.contains("The diff is fine"), "{k}:\n{body}");
        press(&mut m, "esc");
        assert_eq!(views::view_name(&m, KEY), "conversation", "{k}");
        assert_eq!(sel(&m), "t2:s:toolu_x", "{k}");
    }
}

/// enter on a call of rush's agent tools shows the agent it's about
/// picked in the subagents view, whichever harness made the call: the one
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
        assert_eq!(views::view_name(&m, KEY), "subagents", "{step}");
        assert_eq!(sel(&m), "sub:spawn:h9", "{step}");
        press(&mut m, "esc");
        assert_eq!(sel(&m), picked, "{step}");
    }
}

/// space on the dock's workflow shows the subagents view on it
/// (openWorkflow).
#[test]
fn a_workflow_from_the_dock() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    super::running::sync(&mut m, KEY);
    m.session.as_mut().unwrap().sel = "job:w1".into();
    press(&mut m, "space");
    assert_eq!(views::view_name(&m, KEY), "subagents");
    assert_eq!(sel(&m), "job:w1");
}

/// shift+↓ steps from the conversation through the runs, working ones
/// first, then the rest newest first, each picked in the subagents view,
/// and back to the conversation (cycleSub).
#[test]
fn cycling_the_runs() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let mut seen = Vec::new();
    for _ in 0..6 {
        press(&mut m, "shift+down");
        let view = views::view_name(&m, KEY).to_string();
        seen.push(match sel(&m).strip_prefix("sub:") {
            Some(id) if view == "subagents" => id.to_string(),
            _ => view,
        });
    }
    assert_eq!(seen, ["a2", "y", "x", "a1", "a0", "conversation"]);
}

/// Wide, the picked run's conversation shows beside the list, saying
/// where it is in it (drawSubPreview).
#[test]
fn wide_shows_the_run_beside_the_list() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let body = text(&views::body(&mut m, 170, 20).unwrap());
    let first = body.lines().next().unwrap();
    assert!(first.contains("│  ✓ review the diff"), "{body}");
    assert!(body.contains("The diff is fine"), "{body}");
    assert!(conn(&mut m, KEY).watch.peek.visible);
}

/// Wide, nothing shows beside the list until a run is picked: the
/// pointer passing over one moves nothing.
#[test]
fn nothing_beside_the_list_until_a_click() {
    use crate::frame::Mouse;
    use rush_draw::MouseKind;
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    views::show_view(&mut m, KEY, "subagents");
    let frame = m.frame();
    let all = text(&frame);
    assert!(all.contains("Click an agent to see it here"), "{all}");
    let y = frame
        .iter()
        .position(|l| l.plain().contains("review the diff"))
        .expect("its row on screen");
    m.mouse_event(&Mouse {
        kind: MouseKind::Moved,
        x: 10,
        y: y as u16,
        mods: Default::default(),
    });
    let all = text(&m.frame());
    assert!(all.contains("Click an agent to see it here"), "{all}");
    assert!(!all.contains("The diff is fine"), "{all}");
}

/// A click on a run picks it and shows it beside the list, with its
/// facts, its conversation and a box to message it; nothing opens it
/// across the whole view.
#[test]
fn a_click_picks_a_run_and_never_opens_it() {
    use crate::frame::Mouse;
    use rush_draw::{Button, MouseKind};
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    views::show_view(&mut m, KEY, "subagents");
    let frame = m.frame();
    let y = frame
        .iter()
        .position(|l| l.plain().contains("review the diff"))
        .expect("its row on screen");
    let click = |m: &mut Model| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Down(Button::Left),
            x: 10,
            y: y as u16,
            mods: Default::default(),
        });
    };
    click(&mut m);
    assert_eq!(sel(&m), "sub:x");
    let all = text(&m.frame());
    assert!(all.contains("The diff is fine"), "{all}");
    assert!(all.contains("claude-opus-4-1 · 1 step"), "{all}");
    assert!(all.contains("to review the diff"), "{all}");
    click(&mut m);
    press(&mut m, "enter");
    assert_eq!(sel(&m), "sub:x");
    let all = text(&m.frame());
    assert!(all.contains("│  ✓ review the diff"), "{all}");
}

/// Narrow, the picked run shows under a short list of the runs, a rule
/// between: ↑↓ move the pick in the list and the run under it follows.
#[test]
fn narrow_the_run_shows_under_the_list() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 90;
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let got = text(&views::body(&mut m, 90, 30).unwrap());
    let rule = got
        .lines()
        .position(|l| l.starts_with("─────"))
        .unwrap_or_else(|| panic!("no rule under the list:\n{got}"));
    let lines: Vec<&str> = got.lines().collect();
    let (list, under) = (lines[..rule].join("\n"), lines[rule..].join("\n"));
    assert!(list.contains("▍ ✓ review the diff"), "{got}");
    assert!(under.contains("claude-opus-4-1 · 1 step"), "{got}");
    assert!(under.contains("The diff is fine"), "{got}");
    assert!(under.contains("to review the diff"), "{got}");
    assert_eq!(conn(&mut m, KEY).watch.split, rule);
    press(&mut m, "up");
    assert_eq!(sel(&m), "sub:y");
    let got = text(&views::body(&mut m, 90, 30).unwrap());
    let under: String = got.lines().skip(rule).collect::<Vec<_>>().join("\n");
    assert!(under.contains("plan the next piece"), "{got}");
}

/// A workflow agent tried again: its earlier try folds under the try
/// that replaced it, opened by a click or enter, never a row of its own
/// while the workflow runs.
#[test]
fn earlier_tries_fold_under_the_new_one() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let got = text(
        &subview::list(&mut m, KEY, 120, false)
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>(),
    );
    assert!(got.contains("▸ 1 earlier try"), "{got}");
    assert!(!got.contains("failed"), "{got}");
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "tries:a1".into();
    press(&mut m, "enter");
    let got = text(
        &subview::list(&mut m, KEY, 120, false)
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>(),
    );
    assert!(got.contains("▾ 1 earlier try"), "{got}");
    assert!(got.contains("✗ port:hl"), "{got}");
}

/// Where a run works is where its own calls reached, not the folder its
/// parent was in when it started it.
#[test]
fn a_runs_branch_is_where_it_worked() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    // Picking it reads its transcript afresh; where it worked is set after.
    let _ = views::body(&mut m, 170, 20);
    let r = &mut conn(&mut m, KEY).runs;
    r.stats.get_mut("x").unwrap().worktree = "perf-memlist".into();
    let got = text(&views::body(&mut m, 170, 20).unwrap());
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
    assert_eq!(views::view_name(&m, KEY), "subagents");
    let _ = m.frame();
    // Picked from elsewhere, its box has the keys as it shows, and keeps
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
    // Its run gone from what rush hosts, the box lets the keys go.
    conn(&mut m, KEY).runs.spawns.clear();
    press(&mut m, "x");
    assert!(
        !conn(&mut m, KEY).watch.msg_focus,
        "a box with no route kept the keys"
    );
}

/// A click on the dock's workflow, on the transcript's end, shows its
/// agents; on a run, picks it (clickRef :5102).
#[test]
fn clicking_the_docks_runs() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    super::running::sync(&mut m, KEY);
    super::dock::dock(&mut m, 120);
    let refs = m.session.as_ref().unwrap().panel_refs.clone();
    assert!(refs.iter().any(|r| r == "run:a2"), "{refs:?}");
    assert!(subview::click_ref(&mut m, KEY, "run:a2"));
    assert_eq!(sel(&m), "run:a2");
    assert!(subview::click_ref(&mut m, KEY, "job:w1"));
    assert_eq!(views::view_name(&m, KEY), "subagents");
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
    assert!(hint.starts_with("enter send to the subagent"), "{hint}");
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
/// through fake, picked beside the wide list, not opened.
fn picked_beside(d: &tempfile::TempDir, fake: &Arc<Fake>, inbox: bool) -> Model {
    use super::host::Hosts;
    use rush_core::event::{Event, TaskKind, TaskStarted};
    let mut m = model(d);
    m.w = 180;
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
    m
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        let k = if c == ' ' {
            "space".into()
        } else {
            c.to_string()
        };
        press(m, &k);
    }
}

/// The box under a run beside the list: → gives it the keys, and what's
/// typed goes to the run, not the main session: into its inbox where its
/// host has one, else passed on by the main session with SendMessage,
/// which the box says.
#[test]
fn the_box_beside_the_list_messages_the_run() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    let all = text(&m.frame());
    assert!(all.contains("click here or press → to message it"), "{all}");
    assert!(all.contains("into its inbox"), "{all}");
    press(&mut m, "right");
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
    press(&mut m, "esc");
    assert!(!conn(&mut m, KEY).watch.msg_focus);

    // No inbox: the main session passes it on, and the box says so.
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, false);
    let all = text(&m.frame());
    assert!(all.contains("passes it on with SendMessage"), "{all}");
    press(&mut m, "right");
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

/// A session rush doesn't run can't pass a message on: the box says why
/// and takes no keys.
#[test]
fn the_box_says_why_nothing_can_reach_the_run() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.w = 180;
    subview::pick_run(&mut m, KEY, "x");
    let all = text(&m.frame());
    assert!(all.contains("the session isn't running in rush"), "{all}");
    press(&mut m, "right");
    assert!(!conn(&mut m, KEY).watch.msg_focus);
    // Nothing opens it alone instead: it stays picked with the list.
    assert_eq!(views::view_name(&m, KEY), "subagents");
    assert_eq!(sel(&m), "sub:x");
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

/// The lowest row with a ref in sight, as the view was last drawn.
fn by_the_box(m: &mut Model) -> String {
    conn(m, KEY)
        .vs
        .refs
        .iter()
        .rev()
        .find(|r| !r.is_empty())
        .cloned()
        .expect("a row in sight")
}

/// ↑ from the box picks the row in sight by the box, as the
/// conversation's ↑ does, not the list's last row (the oldest run
/// finished), however far below that is.
#[test]
fn up_from_the_box_picks_the_row_by_it() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    views::show_view(&mut m, KEY, "subagents");
    views::body(&mut m, 120, 8).unwrap();
    assert_eq!(conn(&mut m, KEY).vs.top, 0);
    let by_box = by_the_box(&mut m);
    let last = conn(&mut m, KEY)
        .vs
        .all_refs
        .iter()
        .rev()
        .find(|r| !r.is_empty())
        .cloned()
        .unwrap();
    assert_ne!(by_box, last, "the whole list is in sight");
    press(&mut m, "up");
    assert_eq!(sel(&m), by_box);
    views::body(&mut m, 120, 8).unwrap();
    assert_eq!(conn(&mut m, KEY).vs.top, 0, "the view jumped");
}

/// The wheel moves the subagents view and leaves the pick where it is,
/// out of sight, as the conversation's wheel does; drawn again (the runs
/// read again) it stays put; ↑ then picks the row in sight by the box.
#[test]
fn the_wheel_leaves_the_pick() {
    use crate::frame::{Mouse, Out};
    use rush_draw::MouseKind;
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    views::body(&mut m, 120, 8).unwrap();
    let x = "sub:x".to_string();
    assert!(
        conn(&mut m, KEY).vs.top > 0,
        "the pick isn't brought in sight"
    );
    assert!(conn(&mut m, KEY).vs.refs.contains(&x));
    let wheel = |m: &mut Model, kind: MouseKind| {
        views::mouse(
            m,
            &Mouse {
                kind,
                x: 4,
                y: 10,
                mods: Default::default(),
            },
        )
    };
    for _ in 0..4 {
        wheel(&mut m, MouseKind::ScrollUp);
    }
    assert_eq!(wheel(&mut m, MouseKind::ScrollUp), Out::Idle, "at the top");
    for _ in 0..2 {
        views::body(&mut m, 120, 8).unwrap();
        assert_eq!(conn(&mut m, KEY).vs.top, 0, "the pick pulled it back");
        assert!(!conn(&mut m, KEY).vs.refs.contains(&x));
        assert_eq!(sel(&m), x, "the wheel moved the pick");
    }
    let by_box = by_the_box(&mut m);
    press(&mut m, "up");
    assert_eq!(sel(&m), by_box);
    views::body(&mut m, 120, 8).unwrap();
    assert_eq!(conn(&mut m, KEY).vs.top, 0, "the view jumped");
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

/// While a run works, the subagents view shows the session's working line:
/// docked under its conversation, beside the list and under it, and,
/// short, on its row (the spinner and its step); a finished run shows how
/// it ended. Once it finishes, the line goes and its row says done.
#[test]
fn a_working_run_shows_its_working_line() {
    use rush_core::event::{Event, TaskDone};
    let d = tempfile::tempdir().unwrap();
    let mut m = working(&d);
    m.w = 150;
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:w".into();
    let got = text(&views::body(&mut m, 150, 30).unwrap());
    let row = row_of(&mut m, "w");
    assert!(row.contains("· run the tests…"), "its row: {row}");
    assert!(got.contains("· running Bash…"), "no working line:\n{got}");
    let done = row_of(&mut m, "x");
    assert!(done.contains("done"), "the finished run: {done}");
    let start = m.session.as_ref().unwrap().sess.jobs()[0].start;
    let wf = subview::started(start, NOW);
    let asked = crate::screens::settings::paint::civil(NOW - 56_789);
    let hm = format!("{:02}:{:02}", asked.hour, asked.minute);
    assert_eq!(got, working_view(&wf, &hm), "\n{got}");
    // Narrow, the run goes under a short list, its working line with it.
    let narrow = text(&views::body(&mut m, 90, 30).unwrap());
    assert!(narrow.contains("· running Bash…"), "narrow:\n{narrow}");
    assert_eq!(narrow, working_narrow(&wf, &hm), "\n{narrow}");
    // It finishes: the line goes, and its row says how it ended.
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskDone(TaskDone {
            id: "w".into(),
            call_id: "tw".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    let narrow = text(&views::body(&mut m, 90, 30).unwrap());
    assert!(
        !narrow.contains("running Bash"),
        "narrow, finished:\n{narrow}"
    );
    let got = text(&views::body(&mut m, 150, 30).unwrap());
    assert!(!got.contains("running Bash"), "finished:\n{got}");
    let row = row_of(&mut m, "w");
    assert!(!row.contains("run the tests"), "its row, finished: {row}");
    assert!(row.contains("done"), "its row, finished: {row}");
}

/// Run id's row in the list, 120 wide.
fn row_of(m: &mut Model, id: &str) -> String {
    let rf = format!("sub:{id}");
    subview::list(m, KEY, 120, false)
        .into_iter()
        .find(|(_, r)| *r == rf)
        .map(|(l, _)| l.plain())
        .unwrap_or_default()
}

/// The subagents view 150 wide, w picked and working: the line under its
/// conversation, its short form on its row; wf is when the workflow
/// started and hm when w was asked, in local time.
fn working_view(wf: &str, hm: &str) -> String {
    [
        "  1 agent and 1 workflow running · 4 runs in all  ·  click one to see it h… │  · check the port's tests                                   running  57s",
        "                                                                            │    claude-opus-4-1 · 1 step · $0.02",
        "  Running  2                                                                │ ────────────────────────────────────────────────────────────────────────",
        "  · ⋔ Port rush t… · {wf}   · Port 1/3  ›  · C…  running              5m   │",
        "    · Port  1 of 3 done                                                     │ ▍ You  {hm}",
        "      ✓ port:hl                                   done                      │ ▍ check the port's tests",
        "        ▸ 1 earlier try · click to show                                     │",
        "      ✢ port:claude                               · working…                │ ▍ Agent  {hm}",
        "      ◌ port:draw    waiting to start                                       │ │   ✳ run the tests  cargo test                        36s since {hm}",
        "    · Check  not started                                                    │",
        "▍ · check the port's tests                        · run the test…     57s   │ ▍   · running Bash…  36s  turn 56s",
        "                                                                            │",
        "  Finished  2                                                               │",
        "  ✓ plan the next piece of the port with care     done                      │",
        "  ✓ review the diff                               done                 9s   │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │",
        "                                                                            │ ╭─ to check the port's tests ──────────────────────────────────────────╮",
        "                                                                            │ │ ❯ the session isn't running in rush · resume it to send              │",
        "                                                                            │ ╰──────────────────────────────────────────────────────────────────────╯",
    ]
    .join("\n")
    .replace("{wf}", wf)
    .replace("{hm}", hm)
}

/// The subagents view 90 wide, w picked and working: a short list, then
/// w under it with the line under its conversation.
fn working_narrow(wf: &str, hm: &str) -> String {
    [
        "  1 agent and 1 workflow running · 4 runs in all  ·  click one to see it below",
        "",
        "  Running  2",
        "  · ⋔ Port rush to Rust · {wf}   · Port 1/3  ›  · Check      running                 5m",
        "    · Port  1 of 3 done",
        "      ✓ port:hl                                               done",
        "        ▸ 1 earlier try · click to show",
        "      ✢ port:claude                                           · working…",
        "      ◌ port:draw    waiting to start",
        "    · Check  not started",
        "▍ · check the port's tests                                    · run the tests…       57s",
        "",
        "──────────────────────────────────────────────────────────────────────────────────────────",
        " · check the port's tests                                                     running  57s",
        "   claude-opus-4-1 · 1 step · $0.02",
        "──────────────────────────────────────────────────────────────────────────────────────────",
        "",
        "▍ You  {hm}",
        "▍ check the port's tests",
        "",
        "▍ Agent  {hm}",
        "│   ✳ run the tests  cargo test                                          36s since {hm}",
        "",
        "▍   · running Bash…  36s  turn 56s",
        "",
        "",
        "",
        "╭─ to check the port's tests ────────────────────────────────────────────────────────────╮",
        "│ ❯ the session isn't running in rush · resume it to send                                │",
        "╰────────────────────────────────────────────────────────────────────────────────────────╯",
    ]
    .join("\n")
    .replace("{wf}", wf)
    .replace("{hm}", hm)
}

/// What the picked run follows, by id: None when nothing is.
fn followed(m: &mut Model) -> Option<String> {
    conn(m, KEY).runs.live.as_ref().map(|f| f.id.clone())
}

/// w picked in the Subagents tab, shown 150 wide.
fn picked_working(d: &tempfile::TempDir) -> Model {
    let mut m = working(d);
    m.w = 150;
    subview::pick_run(&mut m, KEY, "w");
    let _ = views::body(&mut m, 150, 30);
    m
}

/// A running subagent picked is followed: what its transcript gains shows
/// after one wake of its watch, its conversation's new step and its row's
/// short working form, with no wait for the next refresh.
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
    let got = text(&views::body(&mut m, 150, 30).unwrap());
    assert!(got.contains("lint it"), "its new step:\n{got}");
    let row = row_of(&mut m, "w");
    assert!(row.contains("lint it"), "its row: {row}");
}

/// The tab not showing, nothing is followed; a wake from a follow left
/// over lets it go.
#[test]
fn a_hidden_tab_follows_nothing() {
    let d = tempfile::tempdir().unwrap();
    let mut m = picked_working(&d);
    assert_eq!(followed(&mut m).as_deref(), Some("w"));
    views::show_view(&mut m, KEY, "conversation");
    let _ = super::subfollow::woke(&mut m, KEY, "w");
    assert_eq!(followed(&mut m), None);
    super::subs::refresh(&mut m, KEY);
    assert_eq!(followed(&mut m), None);
}

/// A finished subagent isn't followed: picked once done, or let go as it
/// finishes.
#[test]
fn a_finished_subagent_is_not_followed() {
    use rush_core::event::{Event, TaskDone};
    let d = tempfile::tempdir().unwrap();
    let mut m = picked_working(&d);
    subview::pick_run(&mut m, KEY, "x");
    let _ = views::body(&mut m, 150, 30);
    assert_eq!(followed(&mut m), None, "x is done");
    subview::pick_run(&mut m, KEY, "w");
    let _ = views::body(&mut m, 150, 30);
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
    let _ = views::body(&mut m, 150, 30);
    assert_eq!(followed(&mut m), None, "w finished");
}
