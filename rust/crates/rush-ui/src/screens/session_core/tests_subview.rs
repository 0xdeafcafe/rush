//! The subagents view, watching a run and the dock's runs under ↑: after
//! Go's internal/ui workflow_test.go (TestWorkflowRows' list),
//! subview_test.go (TestSubagentsView, TestWatchFromDock,
//! TestCycleSubagents), subbanner_test.go (TestSubBanner), spawns_test.go
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
    let mut m = Model::new("0.1.0");
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

fn press(m: &mut Model, k: &str) {
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
        "space" => Key::Char(' '),
        c => Key::Char(c.chars().next().unwrap()),
    };
    m.key(code, mods);
}

/// The list: a count, the workflow with its phases and its agents' runs
/// under them, its agent's earlier try apart, then the runs finished,
/// each two rows: what it was asked and how it stands, then its model,
/// steps, tokens, cost and last words (TestWorkflowRows' list, Go's
/// subagentList at 120).
#[test]
fn the_list() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let rows: Vec<Line> = subview::list(&mut m, KEY, 120)
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    let got = text(&rows);
    let want = [
        "  1 running · 5 runs · space opens one · → opens its conversation",
        "",
        "  Running",
        "  · Port rush to Rust   · Port 1/3  ›  · Check                                                            running   5m",
        "      235 steps · 980k tokens",
        "",
        "    · Port · 1 of 3 done",
        "      ✓ port:hl                                                                                                done",
        "",
        "      ✢ port:claude                                                                                         running",
        "",
        "      ◌ port:draw    waiting to start",
        "",
        "    · Check · not started",
        "",
        "    Earlier tries · tried again above",
        "      ✗ port:hl                                                                                              failed",
        "",
        "",
        "  Finished",
        "  ✓ plan the next piece of the port with care                                                                  done",
        "      Plan",
        "▍ ✓ review the diff                                                                                          done   9s",
        "      claude-opus-4-1 · 1 step · in 6k · out 340 · $0.05  ·  The diff is fine: one nit in view.go.",
        "",
    ]
    .join("\n");
    assert_eq!(got, want);
    // Narrow, its tokens give way.
    let narrow = text(
        &subview::list(&mut m, KEY, 80)
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>(),
    );
    assert!(
        narrow.contains("      claude-opus-4-1 · 1 step · $0.05  ·  The diff is fine"),
        "{narrow}"
    );
}

/// A session with runs has a Subagents tab, saying how many run.
#[test]
fn the_tab() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    assert_eq!(
        views::views(&m, KEY),
        ["conversation", "overview", "changes", "subagents", "memory"]
    );
    assert_eq!(subview::tab_stat(&m, KEY).plain(), "1 running");
    assert!(views::show_view(&mut m, KEY, "subagents"));
}

/// space on a run opens its conversation under a banner that says whose
/// it is and how it stands, with the session's other runs on a line
/// beside it; esc goes back to the list, where it was (TestSubBanner,
/// openSub, closeSub).
#[test]
fn watching_a_run() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    press(&mut m, "space");
    assert!(subview::watching(&mut m, KEY), "space didn't open it");
    let b = text(&subview::banner(&mut m, KEY, 120));
    assert_eq!(
        b,
        [
            "▍‹ subagents   review the diff · claude-opus-4-1                                                     ✓ done 9s · 1 step",
            "  ✗ port:hl   ✓ port:hl   · port:claude  ▍✓ review the di… · claude-opu…   ✓ plan the next … · Plan",
        ]
        .join("\n")
    );
    let body = text(&views::body(&mut m, 120, 20).unwrap());
    assert!(
        body.contains("The diff is fine: one nit in view.go."),
        "{body}"
    );
    // The box says it goes to the main session; the keys, how to go back.
    let dock = text(&super::dock::dock(&mut m, 120));
    assert!(
        dock.contains("to the main session, port it, not the subagent"),
        "{dock}"
    );
    let hint = super::core(&mut m).hint.clone().unwrap().plain();
    assert!(
        hint.starts_with("enter send to the main session  ·  esc · ← back to the list"),
        "{hint}"
    );
    // Its rows are picked and opened as the conversation's are.
    press(&mut m, "up");
    let picked = conn(&mut m, KEY)
        .runs
        .detail
        .as_ref()
        .unwrap()
        .view
        .sel
        .clone();
    assert!(!picked.is_empty(), "↑ picked nothing in the run");
    press(&mut m, "esc");
    assert!(
        subview::watching(&mut m, KEY),
        "the first esc drops the pick"
    );
    press(&mut m, "esc");
    assert!(!subview::watching(&mut m, KEY));
    assert_eq!(views::view_name(&mut m, KEY), "subagents");
    assert!(conn(&mut m, KEY).watch.open.is_empty());
    assert!(conn(&mut m, KEY).runs.detail.is_none());
}

/// A run known only by its task has no conversation to open: it says
/// where its step is (openSub's jobSubPrefix).
#[test]
fn a_run_known_by_its_task() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    subview::open_sub(&mut m, KEY, "job:t9");
    assert!(!subview::watching(&mut m, KEY));
    assert_eq!(
        m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default(),
        "that subagent's conversation isn't found yet; its step is in the conversation"
    );
}

/// From the dock: ↑ picks its last run, space watches it, and esc comes
/// back to the conversation on that run's row (watchRun, closeSub,
/// dockRefs).
#[test]
fn watching_from_the_dock() {
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
    // a2 has no transcript here: watch x, which has.
    m.session.as_mut().unwrap().sel = "run:x".into();
    press(&mut m, "space");
    assert!(subview::watching(&mut m, KEY));
    assert!(text(&subview::banner(&mut m, KEY, 120)).starts_with("▍‹ conversation"));
    press(&mut m, "esc");
    assert_eq!(views::view_name(&mut m, KEY), "conversation");
    assert_eq!(sel(&m), "run:x");
    // ↓ past the dock's last goes back to the box.
    m.session.as_mut().unwrap().sel = "run:a2".into();
    press(&mut m, "down");
    assert_eq!(sel(&m), "");
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
    assert_eq!(views::view_name(&mut m, KEY), "subagents");
    assert_eq!(sel(&m), "job:w1");
    assert!(!subview::watching(&mut m, KEY));
}

/// shift+↓ steps from the conversation through the runs, working ones
/// first, then the rest newest first, and back to the conversation
/// (cycleSub).
#[test]
fn cycling_the_runs() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let mut seen = Vec::new();
    for _ in 0..6 {
        press(&mut m, "shift+down");
        let open = conn(&mut m, KEY).watch.open.clone();
        seen.push(if open.is_empty() {
            views::view_name(&mut m, KEY).to_string()
        } else {
            open
        });
    }
    // The workflow's agents (a2 working) have no transcripts here: only
    // x and y's open, and y's path isn't there either but is listed.
    assert_eq!(seen[0], "a2");
    assert_eq!(seen.last().map(String::as_str), Some("conversation"));
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
    assert!(
        first.contains("│  Explore · scroll here · click to open"),
        "{body}"
    );
    assert!(body.contains("The diff is fine"), "{body}");
    assert!(conn(&mut m, KEY).watch.peek.visible);
}

/// Watching a spawned agent rush hosts, what's typed goes to it, queued
/// for its turn's end or sent now (watchedHost, a88d13b3).
#[test]
fn a_watched_spawned_agent_takes_the_box() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = model(&d);
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
        crate::screens::list_extras::spawns::SpawnRun {
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
    subview::watch_run(&mut m, KEY, "spawn:s1");
    assert!(
        subview::watching(&mut m, KEY),
        "the spawned agent isn't watched"
    );
    assert_eq!(
        subview::watched_host(&mut m, KEY).map(|w| w.1),
        Some("h9".to_string())
    );
    conn(&mut m, KEY).input.insert_str("and the metrics");
    press(&mut m, "ctrl+enter");
    assert_eq!(
        fake.sent_to.lock().unwrap().clone(),
        [("h9".to_string(), "and the metrics".to_string(), true)]
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
    assert_eq!(views::view_name(&mut m, KEY), "subagents");
    assert!(!subview::click_ref(&mut m, KEY, "3:s:t1"));
}
