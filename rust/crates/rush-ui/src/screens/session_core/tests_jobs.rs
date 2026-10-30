//! The background view against Go's: every task but subagents, running
//! first, a workflow with its phases under it, a task opened to its
//! output and its launch script, picked and edged as Go draws them
//! (jobs.go jobLines; the rows are Go's, dumped from its jobLines for the
//! same session at 80 and 120 columns). Its keys and clicks, and a
//! workflow's agent watched from it with esc coming back (jobs_test.go
//! TestJobDock, workflow_test.go TestWorkflowRows, rushmode.go watchRun).

use rush_core::event::{Event, Message, Part, PartKind, TaskDone, TaskKind, TaskStarted};
use rush_core::tool::{Call, Input, Kind as ToolKind};
use rush_draw::{Button, MouseKind};

use super::tests_subs::NOW;
use super::tests_subview::{KEY, model};
use super::{conn, running, subview, views};
use crate::frame::Mouse;
use crate::model::Model;
use crate::screens::list_extras::jobs::read_tail;

/// The subagents view's session, with a dev server running in the
/// background (its output last written 5s ago) and a test run that
/// failed.
pub(super) fn background(d: &tempfile::TempDir) -> Model {
    let mut m = model(d);
    let out = d.path().join("b2.output");
    std::fs::write(
        &out,
        "first line\nnpm warn\nbuilding\r50%\r100%\nlistening on :3000\n\n",
    )
    .unwrap();
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis((NOW - 5_000) as u64);
    std::fs::File::options()
        .write(true)
        .open(&out)
        .unwrap()
        .set_modified(at)
        .unwrap();
    let s = &mut m.session.as_mut().unwrap().sess;
    let call = Call {
        id: "t2".into(),
        name: "Bash".into(),
        kind: ToolKind::Shell,
        input: Input {
            command: "npm run dev -- --port 3000".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "m2".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        NOW - 60_000,
    );
    let start = |id: &str, call: &str, label: &str| {
        Event::TaskStarted(TaskStarted {
            id: id.into(),
            call_id: call.into(),
            kind: TaskKind::Shell,
            label: label.into(),
            background: true,
            ..TaskStarted::default()
        })
    };
    s.apply(&start("b2", "t2", "npm run dev"), NOW - 60_000);
    s.apply(&start("b1", "t1x", "go test ./..."), NOW - 120_000);
    s.apply(
        &Event::TaskDone(TaskDone {
            id: "b1".into(),
            status: "failed".into(),
            summary: "exit status 1".into(),
            ..TaskDone::default()
        }),
        NOW - 90_000,
    );
    let p = out.to_string_lossy().into_owned();
    let r = &mut conn(&mut m, KEY).runs;
    r.task_dir = d.path().to_string_lossy().into_owned();
    r.outs.insert(p.clone(), read_tail(&p, None).unwrap());
    m
}

/// The view's rows as Go's test dumps them: each one's ref, then its text.
pub(super) fn dump(m: &mut Model, w: usize) -> String {
    running::background_lines(m, KEY, w)
        .into_iter()
        .map(|(l, r)| format!("{r}|{}\n", l.plain().trim_end()))
        .collect()
}

pub(super) fn pick(m: &mut Model, r: &str) {
    m.session.as_mut().unwrap().sel = r.into();
}

pub(super) fn open(m: &mut Model, r: &str) {
    conn(m, KEY).vs.open.insert(r.into(), true);
}

const HEAD: &str = "|  2 running · 3 tasks · space or click: output · d: command details · x: stop
|
|  Running
";

/// Closed, each task's row, what it has used and its latest output; the
/// workflow's phases under it; the failed test run picked (Go's jobLines
/// at 80 and 120).
#[test]
fn the_background_view() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    pick(&mut m, "job:b1");
    let want120 = HEAD.to_string()
        + "job:w1|  ▸ ⋔ workflow Port rush to Rust                                                                    in background · 5m
job:w1|      235 steps · 980k tokens
job:w1|      · Port 1/3  ›  · Check
job:b2|  ▸ $ shell    npm run dev                                                                          in background · 1m
job:b2|      output active · updated 5s ago
job:b2|      │ npm warn
job:b2|      │ 100%
job:b2|      │ listening on :3000
|
|  Finished
job:b1|▍ ▸ $ shell    go test ./...                                                                            ✗ failed · 30s
job:b1|▍     exit status 1
|
";
    assert_eq!(dump(&mut m, 120), want120);
    let want80 = HEAD.to_string()
        + "job:w1|  ▸ ⋔ workflow Port rush to Rust                            in background · 5m
job:w1|      235 steps · 980k tokens
job:w1|      · Port 1/3  ›  · Check
job:b2|  ▸ $ shell    npm run dev                                  in background · 1m
job:b2|      output active · updated 5s ago
job:b2|      │ npm warn
job:b2|      │ 100%
job:b2|      │ listening on :3000
|
|  Finished
job:b1|▍ ▸ $ shell    go test ./...                                    ✗ failed · 30s
job:b1|▍     exit status 1
|
";
    assert_eq!(dump(&mut m, 80), want80);
    let want200 = r#"|  2 running · 3 tasks · space or click: output · d: command details · x: stop
|
|  Running
job:w1|  ▸ ⋔ workflow Port rush to Rust                                                                                                                                                    in background · 5m
job:w1|      235 steps · 980k tokens
job:w1|      · Port 1/3  ›  · Check
job:b2|  ▸ $ shell    npm run dev                                                                                                                                                          in background · 1m
job:b2|      output active · updated 5s ago
job:b2|      │ npm warn
job:b2|      │ 100%
job:b2|      │ listening on :3000
|
|  Finished
job:b1|▍ ▸ $ shell    go test ./...                                                                                                                                                            ✗ failed · 30s
job:b1|▍     exit status 1
|
"#;
    assert_eq!(dump(&mut m, 200), want200);
}

/// Opened, a task's whole recent output inside an edge down its left,
/// and its launch script a d away; picked, the edge is the pick's bar.
#[test]
fn a_task_opened() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    pick(&mut m, "job:b1");
    open(&mut m, "job:b2");
    let got = dump(&mut m, 120);
    let want = "job:b2|│ ▾ $ shell    npm run dev                                                                          in background · 1m
job:b2|│     output active · updated 5s ago
job:b2|│     Recent output · updated 5s ago
job:b2|│     │ first line
job:b2|│     │ npm warn
job:b2|│     │ 100%
job:b2|│     │ listening on :3000
job:b2|│     ▸ command details · d to show
|
";
    assert!(got.contains(want), "{got}");
    // d on it shows its launch script.
    views::show_view(&mut m, KEY, "background");
    pick(&mut m, "job:b2");
    super::tests_subview::press(&mut m, "d");
    let got = dump(&mut m, 120);
    let want = "job:b2|▍     │ listening on :3000
job:b2|▍     ▾ command details · d to hide
job:b2|▍     $ npm run dev -- --port 3000
|
|  Finished
job:b1|  ▸ $ shell    go test ./...                                                                            ✗ failed · 30s
";
    assert!(got.contains(want), "{got}");
}

/// The view is there while the session has run anything in the
/// background, its tab saying how many run; space opens and closes a
/// task, a click too; on the workflow it shows its agents among the
/// subagents (TestJobDock, TestWorkflowRows).
#[test]
fn its_tab_keys_and_clicks() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    assert_eq!(
        views::views(&m, KEY),
        [
            "conversation",
            "overview",
            "changes",
            "subagents",
            "background",
            "memory"
        ]
    );
    assert_eq!(running::background_stat(&m, KEY).plain(), "2 running");
    assert!(views::show_view(&mut m, KEY, "background"));
    pick(&mut m, "job:b2");
    super::tests_subview::press(&mut m, "space");
    assert_eq!(conn(&mut m, KEY).vs.open.get("job:b2"), Some(&true));
    super::tests_subview::press(&mut m, "space");
    assert_eq!(conn(&mut m, KEY).vs.open.get("job:b2"), Some(&false));
    // A click on a task's row opens it at once.
    let y = m
        .frame()
        .iter()
        .position(|l| l.plain().contains("go test ./..."))
        .expect("its row on screen");
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: 10,
        y: y as u16,
        mods: Default::default(),
    });
    assert_eq!(conn(&mut m, KEY).vs.open.get("job:b1"), Some(&true));
    assert_eq!(m.session.as_ref().unwrap().sel, "job:b1");
    // space on the workflow: its agents, among the subagents.
    pick(&mut m, "job:w1");
    super::tests_subview::press(&mut m, "space");
    assert_eq!(views::view_name(&m, KEY), "subagents");
    assert_eq!(m.session.as_ref().unwrap().sel, "job:w1");
}

/// A workflow's agent watched from the background view: esc comes back
/// to the background view, on its row (watchRun, closeSub).
#[test]
fn watched_from_the_background_view() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    views::show_view(&mut m, KEY, "background");
    pick(&mut m, "run:x");
    super::tests_subview::press(&mut m, "space");
    assert!(subview::watching(&m, KEY), "space didn't watch it");
    let w = &conn(&mut m, KEY).watch;
    assert_eq!(
        (w.from.as_str(), w.from_view.as_str()),
        ("run:x", "background")
    );
    super::tests_subview::press(&mut m, "esc");
    assert_eq!(views::view_name(&m, KEY), "background");
    assert_eq!(m.session.as_ref().unwrap().sel, "run:x");
}

/// Wide, with no run open, the pointer right of the list is over the
/// run's conversation beside it: it lights no row of the list behind
/// (subHoverAt :1007-1011, 3a4a7e4b).
#[test]
fn the_wide_list_lights_only_its_own_rows() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    m.w = 180;
    views::show_view(&mut m, KEY, "subagents");
    let frame = m.frame();
    let y = frame
        .iter()
        .position(|l| l.plain().contains("review the diff"))
        .expect("its row on screen");
    let over = |m: &mut Model, x: u16| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Moved,
            x,
            y: y as u16,
            mods: Default::default(),
        });
        conn(m, KEY).vs.hover_ref.clone()
    };
    assert_eq!(over(&mut m, 10), "sub:x");
    let lw = subview::list_width(conn(&mut m, KEY).watch.pane_w);
    assert_eq!(over(&mut m, (2 + lw + 1) as u16), "");
}

/// In the views panel beside the conversation, a run's row picks it
/// and a workflow's shows its agents (agentviews.go:908-916, d0df4ab4).
#[test]
fn the_panels_runs_open_on_a_click() {
    use crate::screens::plugin_ui::views::beside::on_click;
    use crate::screens::plugin_ui::views::panel::Click;
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    // A run is picked in the subagents view, beside its list, not
    // opened across it.
    on_click(&mut m, KEY, Click::Run("run:x".into()));
    assert!(!subview::watching(&m, KEY), "the run opened whole");
    assert_eq!(views::view_name(&m, KEY), "subagents");
    assert_eq!(m.session.as_ref().unwrap().sel, "sub:x");
    views::show_view(&mut m, KEY, "conversation");
    on_click(&mut m, KEY, Click::Run("job:w1".into()));
    assert_eq!(views::view_name(&m, KEY), "subagents");
    assert_eq!(m.session.as_ref().unwrap().sel, "job:w1");
    views::show_view(&mut m, KEY, "conversation");
    on_click(&mut m, KEY, Click::Run("job:b2".into()));
    assert_eq!(views::view_name(&m, KEY), "conversation");
    assert_eq!(m.session.as_ref().unwrap().sel, "job:b2");
}

/// The Overview's Subagents count the agents the session ran on any
/// harness: through a spawn_agent call, and through a command once its
/// agent is found, each with the model it ran on (overview.go:596-622).
#[test]
fn the_overviews_subagents_include_other_harnesses() {
    use super::overview::{Agentry, Ran, overview};
    let d = tempfile::tempdir().unwrap();
    let m = background(&d);
    let mut s = rush_convo::Session::new();
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
    s.apply(
        &call(
            "s1",
            "mcp__rush__spawn_agent",
            r#"{"agent":"sol","prompt":"check the docs","model":"gpt-6.1-sol"}"#,
        ),
        NOW - 10_000,
    );
    s.apply(
        &call("b1", "Bash", r#"{"command":"codex exec 'review it'"}"#),
        NOW - 5_000,
    );
    let mut ran = Ran::new();
    ran.insert(
        "b1".into(),
        (
            rush_convo::render::Spawn {
                name: "Codex".into(),
                model: "gpt-6".into(),
                ..Default::default()
            },
            4,
        ),
    );
    let ag = Agentry {
        reg: &m.reg,
        kind: "claude",
    };
    let got: Vec<String> = overview(&s, 100, NOW, false, &ag, &ran, &Default::default())
        .iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect();
    let at = got
        .iter()
        .position(|l| l.contains("Subagents"))
        .unwrap_or_else(|| panic!("no Subagents:\n{}", got.join("\n")));
    let rows = got[at + 1..].join("\n");
    assert!(
        rows.contains("Codex     ×1    gpt 6") && rows.contains("4 steps"),
        "{rows}"
    );
    assert!(rows.contains("sol       ×1    gpt 6.1.sol"), "{rows}");
}
