//! The runs a session started, as the Session follows them: after Go's
//! internal/ui subdock_test.go (TestSubagentDock, TestSubagentDockOrder,
//! TestBackgroundSubagentIsASubagent, TestSubagentsTasksUnderIt,
//! TestSubagentDockDone), jobsubs_test.go (TestSubagentFromItsTask),
//! workflow_test.go (TestWorkflowRows, TestWorkflowAgentState) and
//! subdockviews_test.go (TestSubagentsInPanel). No sleeps: a model with
//! no loop reads at once.

use std::sync::Arc;

use rush_convo::{Session, Subagent, Tail};
use rush_core::adapter::Registry;
use rush_core::event::{Event, TaskDone, TaskKind, TaskProgress, TaskStarted, WorkflowAgent};
use rush_core::text::Line;
use rush_core::time::Ms;

use super::running::{Ctx, keep_run_pick, running_preview};
use super::subs::{DONE_FOR, RunStat, Runs, loose_jobs, with_job_subs};
use super::workflow::dock_flows;

pub(super) const NOW: Ms = 1_793_190_896_789;

fn text(ls: &[Line]) -> String {
    ls.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

/// A run's numbers, read from its transcript as the reader does.
fn stat_of(lines: &[&str], calls: &[String]) -> RunStat {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("agent-a1.jsonl");
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    let mut t = Tail::subagent(&p, rush_convo::decode(rush_claude::messages));
    let mut s = Session::new();
    t.read(&mut s).unwrap();
    RunStat::of(&s, calls)
}

fn follower() -> Option<Box<dyn rush_core::adapter::SubagentRuns>> {
    Some(Box::new(rush_claude::subruns::SubagentRuns::new()))
}

/// A Ctx over s and r, the box empty and the pane with the keys.
fn ctx<'a>(s: &'a Session, r: &'a Runs, reg: &'a Registry, sel: &'a str) -> Ctx<'a> {
    Ctx {
        s,
        r,
        reg,
        kind: "claude",
        sel,
        focus: true,
        tick: 0,
        now: NOW,
        idle_box: true,
        queued: &|_| 0,
    }
}

fn preview(s: &Session, r: &mut Runs, sel: &str, w: usize) -> (String, Vec<String>) {
    let running = r.running(s, NOW);
    let dock = r.dock_subs(s, &running, NOW);
    let jobs = s.subagent_jobs();
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, NOW).1;
    let flows = dock_flows(r, s, &running, &live_of);
    let done = r.done_shown().to_vec();
    let reg = Registry::new();
    let (rows, refs) = running_preview(&ctx(s, r, &reg, sel), &dock, &flows, &done, r.run_pick, w);
    for l in &rows {
        assert!(l.width() <= w, "{}", l.plain());
    }
    (text(&rows), refs)
}

const DOCK_LINES: [&str; 4] = [
    r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find the pane"}}"#,
    r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"previewLines"}}]}}"#,
    r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"view.go"}]}}"#,
    r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:03Z","message":{"id":"m2","role":"assistant","content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/w/view.go"}}]}}"#,
];

/// A running subagent shows in the dock with what it's doing and what it
/// just did, and ↑ picks one to watch (TestSubagentDock's dock).
#[test]
fn subagent_dock() {
    let s = Session::new();
    let mut r = Runs {
        subs: vec![Subagent {
            id: "a1".into(),
            typ: "Explore".into(),
            description: "find the pane".into(),
            path: "/x/agent-a1.jsonl".into(),
            modified: NOW,
            ..Subagent::default()
        }],
        follow: follower(),
        ..Runs::default()
    };
    r.stats.insert("a1".into(), stat_of(&DOCK_LINES, &[]));
    let (out, refs) = preview(&s, &mut r, "", 100);
    for w in [
        "Subagents  1 working",
        "Explore",
        "find the pane",
        "› reading view.go",
        "‹ searching for",
        "↑ pick one to watch",
        "2 steps",
    ] {
        assert!(out.contains(w), "missing {w:?} in\n{out}");
    }
    assert_eq!(refs, ["", "run:a1"]);
    // Picked, space watches it.
    let (out, _) = preview(&s, &mut r, "run:a1", 100);
    assert!(out.contains("space watch it"), "{out}");
}

/// Runs show newest first, five at a time, and the pick stays put as
/// runs write, moving to the run in its place when it finishes
/// (TestSubagentDockOrder).
#[test]
fn subagent_dock_order() {
    let mut s = Session::new();
    let subs: Vec<Subagent> = (0..7)
        .map(|i| Subagent {
            id: format!("a{i}"),
            typ: "Explore".into(),
            description: format!("run {i}"),
            born: NOW + i * 1000,
            modified: NOW - i * 1000,
            ..Subagent::default()
        })
        .collect();
    let mut r = Runs {
        subs,
        follow: follower(),
        ..Runs::default()
    };
    let ids: Vec<String> = r.running(&s, NOW).into_iter().map(|sa| sa.id).collect();
    assert_eq!(ids.join(" "), "a6 a5 a4 a3 a2 a1 a0");
    let (out, _) = preview(&s, &mut r, "", 100);
    assert!(
        out.contains("run 2") && !out.contains("run 1") && out.contains("+ 2 older"),
        "first five:\n{out}"
    );
    // The oldest picked: the window follows it down.
    let run = r.running(&s, NOW);
    let sel = keep_run_pick("run:a0", &mut r, &run, &[]);
    assert_eq!((sel.as_str(), r.run_pick), ("run:a0", 6));
    let (out, _) = preview(&s, &mut r, &sel, 100);
    assert!(
        out.contains("run 0") && !out.contains("run 5") && out.contains("… 2 newer"),
        "scrolled:\n{out}"
    );
    // a2 picked, then it finishes: the pick takes the run in its place.
    let sel = keep_run_pick("run:a2", &mut r, &run, &[]);
    assert_eq!(sel, "run:a2");
    s.task_status.insert("a2".into(), "completed".into());
    let run = r.running(&s, NOW);
    let sel = keep_run_pick(&sel, &mut r, &run, &[]);
    assert_eq!(sel, "run:a1", "after it finished");
}

/// A subagent launched in the background runs while its task does,
/// however quiet, until Claude Code says it's done; the background
/// tasks don't count it (TestBackgroundSubagentIsASubagent).
#[test]
fn background_subagent_is_a_subagent() {
    let mut s = Session::new();
    s.apply(
        &Event::TaskStarted(TaskStarted {
            id: "a1".into(),
            call_id: "tA".into(),
            kind: TaskKind::Subagent,
            label: "look around".into(),
            agent: "Explore".into(),
            background: true,
        }),
        NOW - 600_000,
    );
    let mut r = Runs {
        subs: vec![Subagent {
            id: "a1".into(),
            typ: "Explore".into(),
            description: "look around".into(),
            tool_use_id: "tA".into(),
            path: "/x/agent-a1.jsonl".into(),
            modified: NOW - 300_000,
            ..Subagent::default()
        }],
        follow: follower(),
        ..Runs::default()
    };
    assert_eq!(r.running(&s, NOW).len(), 1, "a quiet run whose task runs");
    assert!(
        loose_jobs(&r, &s, &[]).is_empty(),
        "a subagent among the tasks"
    );
    s.apply(
        &Event::TaskDone(TaskDone {
            id: "a1".into(),
            call_id: "tA".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    r.subs[0].modified = NOW;
    assert!(
        r.running(&s, NOW).is_empty(),
        "a finished run still running"
    );
}

/// What a subagent runs in the background hangs under it, not among the
/// session's own tasks, and says whose it is (TestSubagentsTasksUnderIt).
#[test]
fn subagents_tasks_under_it() {
    let lines = [
        r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"merge main"}}"#,
        r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t9","name":"Bash","input":{"command":"cd /r/.worktrees/merge-0930 && git diff --stat","run_in_background":true}}]}}"#,
    ];
    let mut s = Session::new();
    for (id, call, label) in [("b9", "t9", "git diff --stat"), ("b1", "t1", "npm run dev")] {
        s.apply(
            &Event::TaskStarted(TaskStarted {
                id: id.into(),
                call_id: call.into(),
                kind: TaskKind::Shell,
                label: label.into(),
                background: true,
                ..TaskStarted::default()
            }),
            NOW - 60_000,
        );
    }
    let mut r = Runs {
        subs: vec![Subagent {
            id: "a1".into(),
            typ: "lane-opus".into(),
            description: "merge main".into(),
            path: "/x/agent-a1.jsonl".into(),
            modified: NOW,
            ..Subagent::default()
        }],
        follow: follower(),
        ..Runs::default()
    };
    r.stats
        .insert("a1".into(), stat_of(&lines, &["t9".into(), "t1".into()]));
    let run = r.running(&s, NOW);
    let loose: Vec<&str> = loose_jobs(&r, &s, &run)
        .iter()
        .map(|j| j.id.as_str())
        .collect();
    assert_eq!(loose, ["b1"]);
    let (out, refs) = preview(&s, &mut r, "", 110);
    assert!(
        out.contains("↳") && out.contains("git diff --stat"),
        "{out}"
    );
    assert_eq!(refs, ["", "run:a1", "job:b9"]);
    assert_eq!(
        r.job_owner(s.job("b9").unwrap()).map(|sa| sa.id.as_str()),
        Some("a1")
    );
}

/// A run that leaves the dock stays in it a while, ticked off; picked,
/// space reads what it did (TestSubagentDockDone).
#[test]
fn subagent_dock_done() {
    let s = Session::new();
    let sa = Subagent {
        id: "a1".into(),
        typ: "Explore".into(),
        description: "find the pane".into(),
        ..Subagent::default()
    };
    let mut r = Runs {
        subs: vec![sa.clone()],
        ..Runs::default()
    };
    r.dock_subs(&s, std::slice::from_ref(&sa), NOW);
    r.dock_subs(&s, &[], NOW);
    assert_eq!(r.done_shown().len(), 1);
    let (out, refs) = preview(&s, &mut r, "", 100);
    assert!(
        out.contains("✓ find the pane · Explore") && out.contains("done"),
        "{out}"
    );
    assert_eq!(refs.last().map(String::as_str), Some("run:a1"));
    let (out, _) = preview(&s, &mut r, "run:a1", 100);
    assert!(out.contains("read what it did"), "{out}");
    r.dock_subs(&s, &[], NOW + DONE_FOR + 1000);
    assert!(r.done_shown().is_empty(), "kept past DONE_FOR");
}

/// A subagent another agent says of only as a task is listed with the
/// subagents, working until its task ends; a row of its own found gives
/// way to it (TestSubagentFromItsTask).
#[test]
fn subagent_from_its_task() {
    let mut s = Session::new();
    s.apply(
        &Event::TaskStarted(TaskStarted {
            id: "kid".into(),
            call_id: "c1".into(),
            kind: TaskKind::Subagent,
            label: "echoer".into(),
            background: true,
            ..TaskStarted::default()
        }),
        NOW,
    );
    let subs = with_job_subs(&s, Vec::new(), false);
    assert_eq!(subs.len(), 1);
    assert_eq!(
        (
            subs[0].id.as_str(),
            subs[0].description.as_str(),
            subs[0].tool_use_id.as_str()
        ),
        ("job:kid", "echoer", "c1")
    );
    let r = Runs {
        subs: subs.clone(),
        ..Runs::default()
    };
    let jobs = s.subagent_jobs();
    assert!(
        r.state(&s, &subs[0], &jobs, NOW).1,
        "running task read as finished"
    );
    s.apply(
        &Event::TaskDone(TaskDone {
            id: "kid".into(),
            call_id: "c1".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW + 1000,
    );
    let jobs = s.subagent_jobs();
    assert_eq!(
        r.state(&s, &subs[0], &jobs, NOW),
        ("completed".into(), false)
    );
    let found = vec![Subagent {
        id: "spawn:c1".into(),
        tool_use_id: "c1".into(),
        ..Subagent::default()
    }];
    let subs = with_job_subs(&s, found, false);
    assert_eq!(subs.len(), 1, "listed twice: {subs:?}");
}

/// A session running a workflow two phases in: one agent of its first
/// done, one running, one waiting (workflowSession).
pub(super) fn workflow_session() -> Session {
    let mut s = Session::new();
    s.apply(
        &Event::TaskStarted(TaskStarted {
            id: "w1".into(),
            call_id: "t1".into(),
            kind: TaskKind::Workflow,
            label: "Port rush to Rust".into(),
            background: true,
            ..TaskStarted::default()
        }),
        NOW - 300_000,
    );
    let a = |i: i64, label: &str, id: &str, state: &str| WorkflowAgent {
        index: i,
        label: label.into(),
        phase: "Port".into(),
        id: id.into(),
        state: state.into(),
        ..WorkflowAgent::default()
    };
    let mut done = a(1, "port:hl", "a1", "done");
    done.result = "Ported photon/hl".into();
    let mut running = a(2, "port:claude", "a2", "running");
    running.last_tool = "Bash".into();
    running.doing = "cargo build".into();
    s.apply(
        &Event::TaskProgress(TaskProgress {
            id: "w1".into(),
            tokens: 980_000,
            tool_uses: 235,
            phases: vec!["Port".into(), "Check".into()],
            agents: vec![done, running, a(3, "port:draw", "", "queued")],
            ..TaskProgress::default()
        }),
        NOW,
    );
    s
}

fn workflow_runs() -> Runs {
    let mut r = Runs {
        subs: vec![
            Subagent {
                id: "a1".into(),
                description: "port:hl".into(),
                workflow: "wf_1".into(),
                path: "/s/subagents/workflows/wf_1/agent-a1.jsonl".into(),
                born: 1,
                ..Subagent::default()
            },
            Subagent {
                id: "a2".into(),
                description: "port:claude".into(),
                workflow: "wf_1".into(),
                path: "/s/subagents/workflows/wf_1/agent-a2.jsonl".into(),
                born: 2,
                ..Subagent::default()
            },
        ],
        follow: follower(),
        ..Runs::default()
    };
    r.flow_dirs
        .insert("w1".into(), "/s/subagents/workflows/wf_1".into());
    r
}

/// A workflow lives with the subagents: in the dock, its row with the
/// phase it's in, its agents working under it, and not among the shells
/// (TestWorkflowRows' dock).
#[test]
fn workflow_rows() {
    let s = workflow_session();
    let mut r = workflow_runs();
    let (out, refs) = preview(&s, &mut r, "", 120);
    assert!(
        out.contains("⋔ Port rush to Rust")
            && out.contains("› ")
            && out.contains("Port 1/3 · port:claude"),
        "no workflow in the dock's subagents:\n{out}"
    );
    assert!(out.contains("1/3 agents done"), "{out}");
    assert_eq!(refs[1], "job:w1", "↑ doesn't reach the workflow");
    assert!(
        out.contains("port:claude"),
        "its agent at work isn't under it:\n{out}"
    );
    assert!(crate::screens::list_extras::jobs::dock_jobs(&s).is_empty());
    // Picked, space opens its agents.
    let (out, _) = preview(&s, &mut r, "job:w1", 120);
    assert!(out.contains("space its agents"), "{out}");
}

/// A workflow's agent stands as its workflow says; once the workflow
/// stops, one it hadn't heard finish stopped with it
/// (TestWorkflowAgentState).
#[test]
fn workflow_agent_state() {
    let mut s = workflow_session();
    let r = workflow_runs();
    let sa = |id: &str| Subagent {
        id: id.into(),
        workflow: "wf_1".into(),
        ..Subagent::default()
    };
    let jobs = s.subagent_jobs();
    assert!(
        r.state(&s, &sa("a2"), &jobs, NOW).1,
        "a running agent isn't"
    );
    assert_eq!(r.state(&s, &sa("a1"), &jobs, NOW), (String::new(), false));
    s.apply(
        &Event::TaskDone(TaskDone {
            id: "w1".into(),
            status: "stopped".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    let jobs = s.subagent_jobs();
    assert_eq!(
        r.state(&s, &sa("a2"), &jobs, NOW),
        ("stopped".into(), false)
    );
}

/// Through the model: a Claude Code session whose transcript has a run
/// beside it shows the run in its dock, read off the UI thread (here at
/// once, with no loop).
#[test]
fn dock_reads_the_runs() {
    let d = tempfile::tempdir().unwrap();
    let t = d.path().join("s1.jsonl");
    std::fs::write(&t, "").unwrap();
    let sub = d.path().join("s1/subagents");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("agent-a1.meta.json"),
        r#"{"agentType":"Explore","description":"find the pane","spawnDepth":1}"#,
    )
    .unwrap();
    std::fs::write(sub.join("agent-a1.jsonl"), DOCK_LINES.join("\n") + "\n").unwrap();
    let reg = Registry::new();
    reg.register(rush_claude::Claude::new());
    let mut a = rush_core::agent::Agent {
        key: "default/k1".into(),
        kind: "claude".into(),
        display_name: "port it".into(),
        ..Default::default()
    };
    a.job.transcript_path = t.to_string_lossy().into_owned();
    a.job.state = rush_core::agent::State::Working;
    let mut m = crate::model::Model::new("0.1.0");
    m.reg = reg;
    m.w = 140;
    m.h = 40;
    // Now: the run's transcript was just written.
    m.clock = Some(rush_core::time::now());
    m.now = m.clock.unwrap();
    let src = crate::session::Source::of(&m.reg, &a).expect("a transcript");
    let mut v = crate::session::SessionView::new(a.key.clone(), src, 1);
    v.read_now();
    m.sel = a.key.clone();
    m.agents = vec![a];
    m.session = Some(v);
    let dock = super::dock::dock(&mut m, 140);
    let panels = m.session.as_ref().unwrap().panels.clone();
    let all = format!("{}\n{}", text(&panels), text(&dock));
    assert!(all.contains("Subagents  1 working"), "{all}");
    assert!(all.contains("find the pane"), "{all}");
    let r = &super::conn(&mut m, "default/k1").runs;
    assert!(r.stats.contains_key("a1"), "its numbers weren't read");
    let _ = Arc::new(());
}

/// A task of the session's own that cds into another worktree says
/// which, after its command (jobWho).
#[test]
fn a_task_says_its_worktree() {
    let mut s = Session::new();
    s.apply(
        &Event::TaskStarted(TaskStarted {
            id: "b1".into(),
            call_id: "t1".into(),
            kind: TaskKind::Shell,
            label: "cd /r/.claude/worktrees/go-ports && make test".into(),
            background: true,
            ..TaskStarted::default()
        }),
        NOW - 60_000,
    );
    let r = Runs::default();
    let reg = Registry::new();
    let c = ctx(&s, &r, &reg, "");
    let jobs: Vec<_> = s.jobs().iter().collect();
    let (rows, _) = super::running::jobs_block(&c, &jobs, 120);
    let out = text(&rows);
    assert!(out.contains("⎇ go-ports"), "{out}");
}
