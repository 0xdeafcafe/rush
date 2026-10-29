//! Stopping a task and moving one to the background, the processes under
//! an opened one, the files a command writes standing for its output, and
//! the chain's command running now: after Go's internal/ui/jobs_test.go
//! (TestJobDock's keys, TestJobUsage, TestOpenedJobEmptyFile,
//! TestJobOutputOnlyLiveFile) and rushmode.go's pick hints (:3800-3813).

use std::sync::Arc;

use rush_core::event::{
    Event, Message, Part, PartKind, TaskDone, TaskKind, TaskStarted, TaskUpdated,
};
use rush_core::tool::{Call, Input, Kind as ToolKind};

use super::host::{Fake, Hosts, Op, set_hosts};
use super::jobprocs::{JobProcs, ProcRow};
use super::tests::flash;
use super::tests_jobs::{background, dump, open, pick};
use super::tests_subs::NOW;
use super::tests_subview::{KEY, press};
use super::{conn, running, views};
use crate::model::Model;

/// A dev server whose own output file is empty, its output in the file
/// its command sends it to, and another it wrote at its start and left,
/// with what its processes use: the background view's rows against Go's,
/// at 80, 120 and 200 columns, shut and opened (testdata dumped from Go's
/// jobLines for the same session and process table).
#[test]
fn the_files_and_processes_against_gos() {
    let d = tempfile::tempdir().unwrap();
    let mut m = super::tests_subview::model(&d);
    m.session.as_mut().unwrap().sess = rush_convo::Session::new();
    let dir = d.path().to_string_lossy().into_owned();
    m.session.as_mut().unwrap().sess.info.cwd = dir.clone();
    let at = |ms: i64| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    for (name, body, ago) in [
        ("dev.log", "ready on :3000\nGET / 200\n", 5_000),
        ("boot.txt", "", 50_000),
    ] {
        let p = d.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(at(NOW - ago))
            .unwrap();
    }
    bash(
        &mut m,
        "t2",
        "echo go > boot.txt; npm run dev -- --port 3000 > dev.log 2>&1",
        NOW - 60_000,
    );
    task(&mut m, "b2", "t2", "npm run dev", true, NOW - 60_000);
    views::show_view(&mut m, KEY, "background");
    super::subs::kick(&mut m, KEY);
    running::sync(&mut m, KEY);
    let row = |pid, comm: &str, cpu, mb: u64| ProcRow {
        pid,
        comm: comm.into(),
        cmd: comm.into(),
        cpu,
        footprint: mb << 20,
    };
    conn(&mut m, KEY).runs.procs = JobProcs {
        pids: [("b2".to_string(), 12)].into(),
        usage: [("b2".to_string(), (302 << 20, 25.0))].into(),
        trees: [(
            "b2".to_string(),
            vec![row(12, "zsh", 1.0, 2), row(13, "vite", 24.0, 300)],
        )]
        .into(),
        shells: Vec::new(),
    };
    let mut got = String::new();
    for opened in [false, true] {
        conn(&mut m, KEY).vs.open.insert("job:b2".into(), opened);
        for w in [80, 120, 200] {
            got += &format!("== open {opened} width {w}\n");
            got += &dump(&mut m, w);
        }
    }
    let want = include_str!("testdata/go_task_files.txt");
    for (g, w) in got.lines().zip(want.lines()) {
        assert_eq!(g, w, "\n{got}");
    }
    assert_eq!(got.lines().count(), want.lines().count(), "\n{got}");
}

/// The background view's session in a session rush runs through fake,
/// its host new enough to move tasks to the background.
fn hosted(d: &tempfile::TempDir, fake: &Arc<Fake>) -> Model {
    let mut m = background(d);
    set_hosts(&mut m, Arc::new(fake.clone()));
    let a = m.agents[0].clone();
    let c = conn(&mut m, KEY);
    c.link = Some(fake.dial(&a).unwrap());
    c.info.proto = 3;
    m
}

/// A Bash call id running cmd, made at.
fn bash(m: &mut Model, id: &str, cmd: &str, at: i64) {
    let call = Call {
        id: id.into(),
        name: "Bash".into(),
        kind: ToolKind::Shell,
        input: Input {
            command: cmd.into(),
            ..Input::default()
        },
        ..Call::default()
    };
    m.session.as_mut().unwrap().sess.apply(
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
        at,
    );
}

/// Task id started by call, in the background or with the turn waiting.
fn task(m: &mut Model, id: &str, call: &str, label: &str, bg: bool, at: i64) {
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskStarted(TaskStarted {
            id: id.into(),
            call_id: call.into(),
            kind: TaskKind::Shell,
            label: label.into(),
            background: bg,
            ..TaskStarted::default()
        }),
        at,
    );
}

/// The keys row under the box, as the dock w wide works it out.
fn hint(m: &mut Model, w: usize) -> String {
    super::dock::dock(m, w);
    super::core(m)
        .hint
        .as_ref()
        .map(|l| l.plain())
        .unwrap_or_default()
}

fn ops(fake: &Fake) -> Vec<Op> {
    std::mem::take(&mut *fake.ops.lock().unwrap())
}

/// b on a task already in the background says so; x stops it; ctrl+b
/// moves what the turn waits on to the background, the one picked if
/// it's that; once nothing waits, ctrl+b is /btw's again; a finished one
/// has nothing to stop (TestJobDock).
#[test]
fn x_stops_and_b_backgrounds() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake);
    task(&mut m, "b3", "t3", "ls -R /", false, NOW - 1_000);
    pick(&mut m, "job:b2");
    press(&mut m, "b");
    assert_eq!(flash(&m), "that's already in the background");
    press(&mut m, "x");
    assert_eq!(flash(&m), "stopping the shell · the rest carries on");
    assert_eq!(ops(&fake), [Op::StopTask { id: "b2".into() }]);
    // Nothing picked: ctrl+b moves everything the turn waits on.
    pick(&mut m, "");
    press(&mut m, "ctrl+b");
    assert_eq!(
        flash(&m),
        "moved what the turn was waiting on to the background"
    );
    assert_eq!(ops(&fake), [Op::Background { id: String::new() }]);
    // The one picked, by its call.
    pick(&mut m, "job:b3");
    press(&mut m, "b");
    assert_eq!(
        flash(&m),
        "moved the shell to the background · the turn carries on"
    );
    assert_eq!(ops(&fake), [Op::Background { id: "t3".into() }]);
    // x on the one the turn waits on: the agent hears it was stopped.
    press(&mut m, "x");
    assert_eq!(
        flash(&m),
        "stopping the shell · Claude hears it was stopped and carries on"
    );
    assert_eq!(ops(&fake), [Op::StopTask { id: "b3".into() }]);
    // In the background now, nothing waits: ctrl+b isn't the dock's.
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskUpdated(TaskUpdated {
            id: "b3".into(),
            background: Some(true),
            ..TaskUpdated::default()
        }),
        NOW,
    );
    pick(&mut m, "");
    assert_eq!(
        super::jobkeys::job_key(&mut m, KEY, "ctrl+b", true),
        crate::frame::Out::Pass
    );
    // A finished one.
    pick(&mut m, "job:b1");
    press(&mut m, "x");
    assert_eq!(flash(&m), "that has already finished");
    assert!(ops(&fake).is_empty());
}

/// A host older than protocol 3 can't move a task; a session rush
/// doesn't run can't stop one (backgroundJob, stopJob).
#[test]
fn what_a_host_cant_do() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake);
    task(&mut m, "b3", "t3", "ls -R /", false, NOW - 1_000);
    conn(&mut m, KEY).info.proto = 2;
    pick(&mut m, "job:b3");
    press(&mut m, "b");
    assert_eq!(
        flash(&m),
        "this session's host is older than this rush · /restart it to move tasks to the background"
    );
    conn(&mut m, KEY).link = None;
    pick(&mut m, "job:b2");
    press(&mut m, "x");
    assert_eq!(
        flash(&m),
        "rush can stop a task only in a session it runs; this one is Claude Code's"
    );
    assert!(ops(&fake).is_empty());
}

/// b and x on a running command picked in the conversation do what they
/// do on its task; its hint says so (shellKey, rushmode.go:3800-3806).
#[test]
fn a_running_command_picked_in_the_conversation() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake);
    bash(&mut m, "t9", "make build", NOW - 1_000);
    let r = m
        .session
        .as_ref()
        .unwrap()
        .sess
        .turns
        .iter()
        .enumerate()
        .find_map(|(i, t)| (!t.items.is_empty()).then_some(i))
        .unwrap_or(0);
    pick(&mut m, &format!("{}:s:t9", r + 1));
    let dock = hint(&mut m, 140);
    assert!(dock.contains("x stop this command"), "{dock}");
    // Not a task yet: the call's id will do.
    press(&mut m, "b");
    assert_eq!(
        flash(&m),
        "moved the shell to the background · the turn carries on"
    );
    assert_eq!(ops(&fake), [Op::Background { id: "t9".into() }]);
    press(&mut m, "x");
    assert_eq!(
        flash(&m),
        "Claude Code hasn't said it's a task yet · try again in a moment"
    );
    task(&mut m, "b9", "t9", "make build", false, NOW);
    press(&mut m, "x");
    assert_eq!(ops(&fake), [Op::StopTask { id: "b9".into() }]);
}

/// A run picked in the dock: space watches it and x stops it alone while
/// it works; once done, space reads what it did (rushmode.go:3807-3813,
/// stopSub).
#[test]
fn a_run_picked() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = hosted(&d, &fake);
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
    pick(&mut m, "run:x");
    let h = hint(&mut m, 160);
    assert!(
        h.contains("watch this subagent") && h.contains("…and say why"),
        "{h}"
    );
    press(&mut m, "x");
    assert_eq!(flash(&m), "stopping Explore · the turn carries on");
    assert_eq!(ops(&fake), [Op::StopTask { id: "x".into() }]);
    m.session.as_mut().unwrap().sess.apply(
        &Event::TaskDone(TaskDone {
            id: "x".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    pick(&mut m, "run:x");
    let h = hint(&mut m, 160);
    assert!(h.contains("read what it did"), "{h}");
    press(&mut m, "x");
    assert_eq!(flash(&m), "that subagent has already finished");
}

/// What a running shell's processes use ends its row in the dock and the
/// background view; opened, the processes under it, busiest first, the
/// MCP server beside it not among them (TestJobUsage).
#[test]
fn processes_under_an_opened_task() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    running::sync(&mut m, KEY);
    let row = |pid, comm: &str, cmd: &str, cpu, mb: u64| ProcRow {
        pid,
        comm: comm.into(),
        cmd: cmd.into(),
        cpu,
        footprint: mb << 20,
    };
    conn(&mut m, KEY).runs.procs = JobProcs {
        pids: [("b2".to_string(), 12)].into(),
        usage: [("b2".to_string(), (302 << 20, 25.0))].into(),
        trees: [(
            "b2".to_string(),
            vec![
                row(12, "zsh", "zsh -c npm run dev", 1.0, 2),
                row(13, "vite", "vite --port 3000", 24.0, 300),
            ],
        )]
        .into(),
        shells: Vec::new(),
    };
    let dock: String = running::blocks(&mut m, KEY, 110, false)
        .into_iter()
        .flat_map(|b| b.rows)
        .map(|l| l.plain())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(dock.contains("25% · 302M"), "{dock}");
    open(&mut m, "job:b2");
    let got = dump(&mut m, 110);
    let want = "job:b2|│     Processes · busiest first
job:b2|│     vite                                                                                     24%    300M
job:b2|│     zsh                                                                                       1%      2M
job:b2|│     ▸ command details · d to show
";
    assert!(got.contains("25% · 302M  ·  in background · 1m"), "{got}");
    assert!(got.contains(want), "{got}");
    // Its details shown, each process by its words.
    conn(&mut m, KEY)
        .vs
        .open
        .insert("job-details:b2".into(), true);
    let got = dump(&mut m, 110);
    assert!(got.contains("│     vite --port 3000 "), "{got}");
}

/// A shell whose own output is empty reads the files its command writes:
/// the one still being written stands for its output, named; opened, a
/// file that's there but empty says so; one written at its start and left
/// doesn't stand for it (TestOpenedJobEmptyFile, TestJobOutputOnlyLiveFile).
#[test]
fn the_files_a_command_writes() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    let dir = d.path().to_string_lossy().into_owned();
    m.session.as_mut().unwrap().sess.info.cwd = dir.clone();
    let at = |ms: i64| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    let write = |name: &str, body: &str, ms: i64| {
        let p = d.path().join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(at(ms))
            .unwrap();
    };
    write("r6.out", "", NOW - 2_000);
    write("spec.json", "{\n}\n", NOW - 120_000);
    write("check.log", "checking a\nchecking b\n", NOW - 3_000);
    bash(
        &mut m,
        "t4",
        "git show main:spec.json > spec.json; bash run-all.sh > r6.out 2>&1; python3 check.py | tee check.log",
        NOW - 30_000,
    );
    task(&mut m, "b4", "t4", "check it", true, NOW - 30_000);
    views::show_view(&mut m, KEY, "background");
    super::subs::kick(&mut m, KEY);
    running::sync(&mut m, KEY);
    let got = dump(&mut m, 110);
    let want = "job:b4|      from check.log · updated 3s ago
job:b4|      │ checking a
job:b4|      │ checking b
";
    assert!(got.contains(want), "{got}");
    open(&mut m, "job:b4");
    super::subs::kick(&mut m, KEY);
    running::sync(&mut m, KEY);
    let got = dump(&mut m, 110);
    assert!(
        got.contains("job:b4|│     from r6.out · empty so far · updated 2s ago"),
        "{got}"
    );
    // spec.json, left since its start, is named among the other files
    // but doesn't stand for the output.
    assert!(
        got.contains("│     from spec.json · updated 2m ago"),
        "{got}"
    );
    assert!(
        got.contains("job:b4|│     1  {"),
        "a document, numbered:\n{got}"
    );
}

/// A chain seen running says which of its commands runs now
/// (RunningPart in jobLines' facts).
#[test]
fn the_chains_command_running_now() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    let cmd = "go vet ./... && go test ./internal/ui && echo ok";
    bash(&mut m, "t5", cmd, NOW - 10_000);
    task(&mut m, "b5", "t5", "check", true, NOW - 10_000);
    let shell = rush_convo::Shell {
        command: cmd.into(),
        start: NOW - 9_000,
        kids: vec![rush_convo::ShellProc {
            pid: 40,
            args: vec!["go".into(), "test".into(), "./internal/ui".into()],
            start: NOW - 5_000,
            kids: Vec::new(),
        }],
        ..Default::default()
    };
    m.session.as_mut().unwrap().sess.watch_shells(&[shell], NOW);
    let got = dump(&mut m, 110);
    assert!(
        got.contains("job:b5|      ▸ 2/3 go test ./internal/ui"),
        "{got}"
    );
}

/// A finished task's rows are drawn once and kept until what they're
/// drawn from changes (jobRowsKey).
#[test]
fn finished_rows_are_kept() {
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    let first = dump(&mut m, 120);
    let r = &conn(&mut m, KEY).runs;
    {
        let mut memo = r.job_rows.lock().unwrap();
        assert!(memo.contains_key("b1") && !memo.contains_key("b2"));
        // What's kept is what's drawn.
        for e in memo.values_mut() {
            e.mark_for_test();
        }
    }
    let again = dump(&mut m, 120);
    assert_ne!(first, again, "the kept rows weren't used");
    assert!(again.contains("(kept)"), "{again}");
    // Another width draws them again.
    assert!(!dump(&mut m, 100).contains("(kept)"));
}

/// The agents a command ran, found among the sessions rush hosted for
/// it: each read whole, a row each under the step that ran them; one in
/// another format is read by its own adapter (SetChildren, spawnRun.view,
/// TestFanOutFollowed).
#[test]
fn a_command_that_ran_several_agents() {
    use crate::screens::list_extras::spawns::SpawnRun;
    let d = tempfile::tempdir().unwrap();
    let mut m = background(&d);
    bash(&mut m, "t7", "run a & run b & wait", NOW - 20_000);
    let lines = super::tests_subview::X_LINES
        .join("\n")
        .replace("\"isSidechain\":true", "\"isSidechain\":false")
        + "\n";
    for (id, kind) in [("s1", "claude"), ("s2", "claude"), ("s3", "codex")] {
        let p = d.path().join(format!("{id}.jsonl"));
        std::fs::write(&p, &lines).unwrap();
        conn(&mut m, KEY).runs.spawns.insert(
            id.into(),
            SpawnRun {
                step: if kind == "codex" { "t8" } else { "t7" }.into(),
                kind: kind.into(),
                name: "Claude Code".into(),
                prompt: format!("review lens {id}"),
                path: p.to_string_lossy().into_owned(),
                born: NOW - 19_000,
                live: id == "s1",
                ..Default::default()
            },
        );
    }
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    let v = m.session.as_ref().unwrap();
    let kids = &v.o.kids["t7"];
    assert_eq!(kids.len(), 2);
    assert!(
        kids.iter()
            .all(|k| k.sess.as_ref().is_some_and(|s| !s.steps().is_empty())),
        "a kid's session wasn't read"
    );
    let o = rush_convo::Options {
        width: 110,
        now: NOW,
        ..v.o.clone()
    };
    let out = rush_convo::render::plain(&rush_convo::Renderer::new().render(&v.sess, &o));
    assert_eq!(out.matches("⇉ review lens s").count(), 2, "{out}");
    assert!(out.contains("2 agents"), "{out}");
    // The one in another format was asked of its adapter: none here, so
    // it reads as empty, and isn't asked again until it grows.
    assert!(conn(&mut m, KEY).runs.full.contains_key("spawn:s3"));
}
