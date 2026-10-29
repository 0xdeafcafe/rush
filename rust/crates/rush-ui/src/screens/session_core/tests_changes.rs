//! What a session changed: its shell commands' files, its subagents'
//! edits, and the files outside its project kept out of Changes. After
//! Go's convo_test.go (TestChangesByShellAndSubagents,
//! TestChangesByChildSteps, TestChangesShellFolderAndScratch,
//! TestProjectChanges) and helpers_changes_test.go
//! (TestSpawnedEditsInChanges).

use std::sync::Arc;

use rush_convo::{Helper, Session};
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::tool::{Call, Input, Kind, Output, Patch};

use crate::screens::session_core::changes::{FileChange, changes, inside, shell_only};

const T0: i64 = 1_800_000_000_000;

fn at(s: i64) -> i64 {
    T0 + s * 1000
}

/// A call made and its result, under parent (a subagent's call) when set.
fn step(s: &mut Session, parent: &str, call: Call, out: Output, t: i64) {
    let id = call.id.clone();
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: format!("m-{id}"),
            parent: parent.into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        at(t),
    );
    s.apply(
        &Event::Message(Message {
            role: "user".into(),
            id: format!("r-{id}"),
            parent: parent.into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output { call_id: id, ..out })),
                ..Part::default()
            }],
            ..Message::default()
        }),
        at(t + 1),
    );
}

fn bash(id: &str, cmd: &str) -> Call {
    Call {
        id: id.into(),
        name: "Bash".into(),
        kind: Kind::Shell,
        input: Input {
            command: cmd.into(),
            ..Input::default()
        },
        ..Call::default()
    }
}

fn edit(id: &str, path: &str) -> Call {
    Call {
        id: id.into(),
        name: "Edit".into(),
        kind: Kind::Edit,
        input: Input {
            path: path.into(),
            ..Input::default()
        },
        ..Call::default()
    }
}

/// An edit's result: one line added after a.
fn added() -> Output {
    Output {
        text: "ok".into(),
        patches: vec![Patch {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            lines: vec![" a".into(), "+b".into()],
            ..Patch::default()
        }],
        ..Output::default()
    }
}

fn end(s: &mut Session) {
    s.apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }),
        at(90),
    );
}

fn by_path<'a>(ch: &'a [FileChange], p: &str) -> Option<&'a FileChange> {
    ch.iter().find(|f| f.path == p)
}

/// A file a shell command writes (a script on stdin) is among the
/// session's changes, marked as by shell since its lines aren't known; a
/// subagent's edits are there too, marked as its.
#[test]
fn changes_by_shell_and_subagents() {
    let mut s = Session::new();
    s.info.cwd = "/work/rush".into();
    s.sent("and the queue".into(), Vec::new(), at(60));
    step(
        &mut s,
        "",
        bash(
            "b9",
            "cd internal/host && python3 -I - queue.go <<'EOF'\nopen('queue.go','w').write('x')\nEOF\ngo build ./...",
        ),
        Output::default(),
        61,
    );
    end(&mut s);
    let mut helper = Session::new();
    helper.info.cwd = "/work/rush".into();
    helper.sent("add start_agent".into(), Vec::new(), at(0));
    step(
        &mut helper,
        "",
        edit("e7", "/work/rush/internal/agtools/agents.go"),
        added(),
        1,
    );
    s.helpers = vec![Helper {
        name: "start_agent subagent".into(),
        sess: Arc::new(helper),
    }];
    let ch = changes(&s);
    let q = by_path(&ch, "/work/rush/internal/host/queue.go").expect("the shell's file");
    assert_eq!((q.shell.len(), q.add + q.del), (1, 0), "{ch:?}");
    assert_eq!(
        q.cmds[0],
        "cd internal/host && python3 -I - queue.go <<'EOF'"
    );
    let a = by_path(&ch, "/work/rush/internal/agtools/agents.go").expect("the subagent's");
    assert_eq!(
        (a.add, a.by.clone()),
        (1, vec!["start_agent subagent".to_string()])
    );
    assert_eq!(
        a.from,
        vec![String::new()],
        "its step is in its own conversation"
    );
    assert_eq!(shell_only(&ch), 1);
}

/// A subagent's edit that came in the session's own conversation, under
/// its Task call, counts once, as the subagent's.
#[test]
fn changes_by_child_steps() {
    let mut s = Session::new();
    s.info.cwd = "/work/rush".into();
    s.sent("fix it".into(), Vec::new(), at(0));
    let task = Call {
        id: "a2".into(),
        name: "Task".into(),
        kind: Kind::Subagent,
        input: Input {
            agent: "general-purpose".into(),
            description: "fix the queue".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "m-a2".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(task)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        at(1),
    );
    step(
        &mut s,
        "a2",
        edit("c1", "/work/rush/internal/host/queue.go"),
        added(),
        2,
    );
    let ch = changes(&s);
    assert_eq!(ch.len(), 1, "{ch:?}");
    assert_eq!(ch[0].add, 1);
    assert_eq!(ch[0].by, vec!["fix the queue".to_string()]);
}

/// A scratch file a command writes and then removes changed nothing; a
/// shell that stays in a folder after a cd runs the next command there.
#[test]
fn changes_shell_folder_and_scratch() {
    let mut s = Session::new();
    s.info.cwd = "/work/rush".into();
    s.sent("go".into(), Vec::new(), at(0));
    for (i, cmd) in [
        "cd internal/host && go test ./...",
        "sed -i '' 's/a/b/' queue.go",
        "cat > zz_test.go <<'EOF'\npackage x\nEOF\ngo test -run ZZ; rm zz_test.go",
        "cd /elsewhere; touch x.go",
    ]
    .into_iter()
    .enumerate()
    {
        let t = 1 + 2 * i as i64;
        step(
            &mut s,
            "",
            bash(&format!("b{i}"), cmd),
            Output::default(),
            t,
        );
    }
    let got: Vec<String> = changes(&s).into_iter().map(|f| f.path).collect();
    assert_eq!(
        got,
        ["/work/rush/internal/host/queue.go", "/elsewhere/x.go"]
    );
}

/// Files edited outside the project are the session's own, not its
/// changes; a relative path is the project's.
#[test]
fn inside_the_project() {
    assert!(inside("/work/rush", "/work/rush/a.go"));
    assert!(inside("/work/rush", "/work/rush"));
    assert!(inside("/work/rush/", "/work/rush/x/a.go"));
    assert!(!inside("/work/rush", "/work/rusher/a.go"));
    assert!(!inside("/work/rush", "/u/.claude/plans/p.md"));
    assert!(inside("/work/rush", "a.go"));
}

/// A file an agent the session's shell ran made, its conversation found:
/// Go's Changes counts its lines under the step that ran it (the agent's
/// steps are that step's children, "a subagent") and again as the helper
/// its conversation is, so +6 for a 3-line file, as Go's header said
/// +613 where it had been +249 (changes.go:96, spawn.go:573,
/// ownfiles.go:81). Several agents from one step are each a row of their
/// own, their steps under those.
#[test]
fn changes_by_agents_a_step_ran() {
    use crate::screens::session_core::changes::{Kids, changes_with};
    use rush_convo::render::{Child, Spawn};

    let kid = |path: &str| {
        let mut k = Session::new();
        k.sent("write it".into(), Vec::new(), at(0));
        let call = Call {
            id: format!("w-{path}"),
            name: "Write".into(),
            kind: Kind::Write,
            input: Input {
                path: path.into(),
                content: "a\nb\nc\n".into(),
                ..Input::default()
            },
            ..Call::default()
        };
        let out = Output {
            created: true,
            ..Output::default()
        };
        step(&mut k, "", call, out, 1);
        end(&mut k);
        Arc::new(k)
    };
    let child = |id: &str, sess: Arc<Session>| Child {
        id: id.into(),
        spawn: Spawn {
            name: "Codex".into(),
            ..Spawn::default()
        },
        sess: Some(sess),
        live: false,
        start: at(1),
    };
    let mut s = Session::new();
    s.info.cwd = "/work/rush".into();
    s.sent("go".into(), Vec::new(), at(0));
    step(
        &mut s,
        "",
        bash("b1", "rush spawn codex"),
        Output::default(),
        1,
    );
    step(
        &mut s,
        "",
        bash("b2", "rush spawn kimi"),
        Output::default(),
        3,
    );
    end(&mut s);
    let one = kid("/work/rush/prompts.json");
    let (two, three) = (kid("/work/rush/a.md"), kid("/work/rush/b.md"));
    s.helpers = vec![Helper {
        name: "Codex".into(),
        sess: one.clone(),
    }];
    let mut kids = Kids::new();
    kids.insert("b1".into(), vec![child("h1", one)]);
    kids.insert("b2".into(), vec![child("h2", two), child("h3", three)]);
    let ch = changes_with(&s, &kids);
    let p = by_path(&ch, "/work/rush/prompts.json").expect("the agent's file");
    assert_eq!(
        (p.add, p.new, p.by.clone()),
        (6, true, vec!["a subagent".to_string(), "Codex".to_string()]),
        "{ch:?}"
    );
    for f in ["/work/rush/a.md", "/work/rush/b.md"] {
        let c = by_path(&ch, f).expect("a fanned agent's file");
        assert_eq!((c.add, c.by.clone()), (3, vec!["a subagent".to_string()]));
    }
    // With no agents found, only the helper's count.
    let ch = changes(&s);
    assert_eq!(by_path(&ch, "/work/rush/prompts.json").unwrap().add, 3);
}
