//! A background task under the step that started it: the rows the
//! background view drew for it (jobs.go jobLines' block, its own first row
//! the step's), opened with the step's output, its launch script a d
//! away; the dock's and the panel's task rows open that step, a workflow
//! opens on the conversation's page (jobs_test.go TestJobDock,
//! workflow_test.go TestWorkflowRows). Rust-only where it shows: Go had a
//! background view.

use rush_core::event::{Event, Message, Part, PartKind, TaskDone, TaskKind, TaskStarted};
use rush_core::tool::{Call, Input, Kind as ToolKind};

use super::tests_subs::NOW;
use super::tests_subview::{KEY, model, press};
use super::{conn, openrun, running, subview, views};
use crate::model::Model;
use crate::screens::list_extras::jobs::read_tail;

/// tests_subview's session, with a dev server running in the
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

/// The rows each background task draws under its step, w wide, as Go's
/// test dumps a view's: the task's ref, then its text.
pub(super) fn dump(m: &mut Model, w: usize) -> String {
    let rows = running::task_rows(m, KEY, w);
    let s = &m.session.as_ref().unwrap().sess;
    let mut out = String::new();
    for j in s.jobs() {
        for l in rows.get(&j.tool_use_id).into_iter().flatten() {
            out += &format!("job:{}|{}\n", j.id, l.plain().trim_end());
        }
    }
    out
}

pub(super) fn pick(m: &mut Model, r: &str) {
    m.session.as_mut().unwrap().sel = r.into();
}

/// Opens or shuts the output of the step task r ("job:b2") was started
/// by, as its row in the dock does.
pub(super) fn open_as(m: &mut Model, r: &str, on: bool) {
    let id = r.strip_prefix("job:").unwrap_or(r);
    let v = m.session.as_mut().unwrap();
    let call = v.sess.job(id).map(|j| j.tool_use_id.clone()).unwrap();
    v.o.open.insert(format!("t0:s:{call}:live"), on);
}

pub(super) fn open(m: &mut Model, r: &str) {
    open_as(m, r, true);
}

/// Shut, a task's facts, what it's doing and its last lines: its own row
/// is its step's (jobLines' block at 120, but its first row).
#[test]
fn a_tasks_rows_under_its_step() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    let got = dump(&mut m, 120);
    let want = "job:b2|output active · updated 5s ago
job:b2|│ npm warn
job:b2|│ 100%
job:b2|│ listening on :3000
";
    assert!(got.contains(want), "{got}");
    assert!(got.contains("job:b1|exit status 1"), "{got}");
    assert!(!got.contains("npm run dev"), "its own row: {got}");
    // A workflow is drawn as a workflow, not as a task.
    assert!(!got.contains("job:w1|"), "{got}");
}

/// Two running monitors or more go under one row in the dock, which opens
/// them there.
#[test]
fn monitors_go_under_one_row() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    let s = &mut m.session.as_mut().unwrap().sess;
    for (id, label) in [("m1", "tail server.log"), ("m2", "watch the deploy")] {
        s.apply(
            &Event::TaskStarted(TaskStarted {
                id: id.into(),
                call_id: format!("t{id}"),
                kind: TaskKind::Monitor,
                label: label.into(),
                background: true,
                ..TaskStarted::default()
            }),
            NOW - 30_000,
        );
    }
    super::running::sync(&mut m, KEY);
    let refs = subview::dock_refs(&mut m, KEY);
    let mons = crate::screens::list_extras::jobs::MONITORS_REF;
    assert!(refs.iter().any(|r| r == mons), "{refs:?}");
    assert!(!refs.iter().any(|r| r == "job:m1"), "{refs:?}");
    assert!(subview::click_ref(&mut m, KEY, mons));
    assert_eq!(views::view_name(&m, KEY), "conversation");
    assert_eq!(conn(&mut m, KEY).vs.open.get(mons), Some(&true));
    assert_eq!(m.session.as_ref().unwrap().sel, mons);
}

/// Opened with its step's output, a task's whole recent output, and its
/// launch script a d away on the step.
#[test]
fn a_task_opened() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    open(&mut m, "job:b2");
    let got = dump(&mut m, 120);
    let want = "job:b2|output active · updated 5s ago
job:b2|Recent output · updated 5s ago
job:b2|│ first line
job:b2|│ npm warn
job:b2|│ 100%
job:b2|│ listening on :3000
job:b2|▸ command details · d to show
";
    assert!(got.contains(want), "{got}");
    // d on its step shows its launch script.
    let step = super::presets::step_ref(&m.session.as_ref().unwrap().sess, "t2").expect("its step");
    pick(&mut m, &step);
    press(&mut m, "d");
    let got = dump(&mut m, 120);
    let want = "job:b2|│ listening on :3000
job:b2|▾ command details · d to hide
job:b2|$ npm run dev -- --port 3000
";
    assert!(got.contains(want), "{got}");
}

/// No view of its own: the session's views are the conversation's and
/// the rest; space on a task's row in the dock opens its step where it
/// is, its output with it; on the workflow, it opens on the
/// conversation's page (TestJobDock, TestWorkflowRows).
#[test]
fn a_tasks_row_opens_its_step() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    assert_eq!(
        views::views(&m, KEY),
        ["conversation", "overview", "changes", "memory"]
    );
    running::sync(&mut m, KEY);
    pick(&mut m, "job:b2");
    press(&mut m, "space");
    let sel = m.session.as_ref().unwrap().sel.clone();
    assert!(sel.ends_with(":s:t2"), "{sel}");
    let live = format!("{sel}:live");
    assert_eq!(m.session.as_ref().unwrap().o.open.get(&live), Some(&true));
    assert!(dump(&mut m, 120).contains("job:b2|Recent output"));
    pick(&mut m, "job:w1");
    press(&mut m, "space");
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("job:w1"));
    assert_eq!(views::view_name(&m, KEY), "conversation");
}

/// A finished task's step opened shows how it ended.
#[test]
fn a_finished_tasks_step() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    let s = &mut m.session.as_mut().unwrap().sess;
    s.apply(
        &Event::TaskDone(TaskDone {
            id: "b2".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    let got = dump(&mut m, 120);
    assert!(got.contains("job:b2|│ listening on :3000"), "{got}");
}

/// In the views panel beside the conversation, a run's row opens it, a
/// workflow's opens with its agents, a task's opens its step
/// (agentviews.go:908-916, d0df4ab4).
#[test]
fn the_panels_runs_open_on_a_click() {
    use crate::screens::plugin_ui::views::beside::on_click;
    use crate::screens::plugin_ui::views::panel::Click;
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    on_click(&mut m, KEY, Click::Run("run:x".into()));
    assert_eq!(views::view_name(&m, KEY), "conversation");
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("x"));
    on_click(&mut m, KEY, Click::Run("job:w1".into()));
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("job:w1"));
    openrun::close(&mut m, KEY);
    on_click(&mut m, KEY, Click::Run("job:b2".into()));
    assert_eq!(views::view_name(&m, KEY), "conversation");
    let sel = m.session.as_ref().unwrap().sel.clone();
    assert!(sel.ends_with(":s:t2"), "{sel}");
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
