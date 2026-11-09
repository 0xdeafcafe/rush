//! Emeritus without a running host: the words, the successor, the
//! terminal check at every way in, moving what was the retiring
//! session's, following a successor, and settling a handover a host was
//! cut short in.

use std::time::Duration;

use rush_core::event::Peer;

use super::*;
use crate::subtool::{Answer, answer_path, await_answer};
use crate::testing::Home;
use crate::wire::{Limit, Retry, Workspace};

fn baton() -> String {
    SECTIONS
        .iter()
        .map(|s| format!("## {s}\n{s} of the task."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn session(id: &str, f: impl FnOnce(&mut Config)) -> Config {
    let mut c = Config {
        id: id.into(),
        session_id: format!("s-{id}"),
        kind: "fake".into(),
        cwd: "/tmp".into(),
        ..Config::default()
    };
    f(&mut c);
    std::fs::create_dir_all(dir(id)).unwrap();
    write_config(&c).unwrap();
    c
}

fn info(id: &str, f: impl FnOnce(&mut Info)) {
    let mut i = Info {
        id: id.into(),
        state: "idle".into(),
        ..Info::default()
    };
    f(&mut i);
    std::fs::create_dir_all(dir(id)).unwrap();
    std::fs::write(dir(id).join("info.json"), i.to_json()).unwrap();
}

fn retired_to(id: &str, to: &str) {
    let mut c = read_config(id).unwrap();
    c.emeritus = Some(Emeritus {
        retired_at: Time::now(),
        successor: to.into(),
        ..Emeritus::default()
    });
    write_config(&c).unwrap();
}

// The nudge and the retire tool give the whole template: every section,
// the two the 300k lab added among them, and the rule that a turn's
// scope isn't a limit.
#[test]
fn the_template_has_every_section() {
    let n = nudge(312_000, 300_000);
    assert!(
        n.starts_with(NUDGE_START) && n.contains("312k") && n.contains("300k"),
        "{n}"
    );
    for s in SECTIONS {
        assert!(n.contains(&format!("## {s}\n")), "{s} in {n}");
    }
    assert!(n.contains("are not limits for your successor"), "{n}");
    let whole = baton();
    let cases: &[(&str, &str, &[&str])] = &[
        ("whole", whole.as_str(), &[]),
        ("empty", "", SECTIONS),
        (
            "no tests or map",
            "## Destination\n## Core ideas\n## Methods\n## Journey\n## Limits\n## Next action",
            &["Tests that pin text", "Key files"],
        ),
    ];
    for (name, b, want) in cases {
        assert_eq!(baton_lacks(b), want.to_vec(), "{name}");
    }
}

// A successor has started once its first request came back, or its
// first turn ended clean; a limit, a retried error, an error or its host
// gone are a failed start.
#[test]
fn a_successors_start() {
    let base = Info {
        state: "working".into(),
        ..Info::default()
    };
    let with = |f: fn(&mut Info)| {
        let mut i = base.clone();
        f(&mut i);
        i
    };
    let cases: Vec<(&str, Info, bool, Start)> = vec![
        ("waiting", base.clone(), true, Start::Waiting),
        (
            "first request back",
            with(|i| i.context_tokens = 22_000),
            true,
            Start::Started(22_000),
        ),
        (
            "first turn ended, context unsaid",
            with(|i| {
                i.state = "idle".into();
                i.turn = 1;
            }),
            true,
            Start::Started(0),
        ),
        (
            "limited",
            with(|i| {
                i.context_tokens = 5;
                i.limit = Some(Limit::default());
            }),
            true,
            Start::Failed("hit a usage limit".into()),
        ),
        (
            "retrying",
            with(|i| {
                i.retry = Some(Retry {
                    reason: "overloaded".into(),
                    ..Retry::default()
                })
            }),
            true,
            Start::Failed("hit a provider error: overloaded".into()),
        ),
        (
            "error",
            with(|i| i.error = "not signed in".into()),
            true,
            Start::Failed("couldn't start: not signed in".into()),
        ),
        (
            "host gone",
            base.clone(),
            false,
            Start::Failed("stopped before it answered".into()),
        ),
    ];
    for (name, i, alive, want) in cases {
        assert_eq!(start_of(&i, alive), want, "{name}");
    }
}

// The successor works where its predecessor did, as it did: its folder,
// agent, model, effort, mode, account, workspace and parent, tagged with
// its predecessor and the chain's length, on a conversation of its own.
#[test]
fn the_successor_carries_on_as_its_predecessor() {
    let a = Config {
        id: "a1".into(),
        session_id: "s-a1".into(),
        resume: true,
        kind: "claude".into(),
        cwd: "/old".into(),
        model: "sonnet".into(),
        name: "fix the cache".into(),
        meta: [
            ("spawnedBy".to_string(), "p1".to_string()),
            ("lineage".to_string(), "2".to_string()),
        ]
        .into(),
        account: rush_core::adapter::Profile {
            name: "work".into(),
            dir: "/acct".into(),
            ..Default::default()
        },
        emeritus: Some(Emeritus {
            nudged_at: Time::now(),
            ..Emeritus::default()
        }),
        ..Config::default()
    };
    let i = Info {
        cwd: "/new".into(),
        model: "opus".into(),
        effort: "high".into(),
        permission_mode: "auto".into(),
        workspace: Some(Workspace {
            path: "/new".into(),
            branch: "fix/x".into(),
            ..Workspace::default()
        }),
        ..Info::default()
    };
    let b = successor_config(&a, &i, "go".into());
    let got = (
        b.id.as_str(),
        b.session_id.as_str(),
        b.resume,
        b.kind.as_str(),
        b.cwd.as_str(),
        b.model.as_str(),
        b.effort.as_str(),
        b.permission_mode.as_str(),
        b.account.dir.as_str(),
        b.prompt.as_str(),
        b.name.as_str(),
    );
    assert_eq!(
        got,
        (
            "",
            "",
            false,
            "claude",
            "/new",
            "opus",
            "high",
            "auto",
            "/acct",
            "go",
            "fix the cache"
        )
    );
    assert_eq!(b.meta["spawnedBy"], "p1");
    assert_eq!(b.meta["predecessor"], "a1");
    assert_eq!(b.meta["lineage"], "3");
    assert!(b.emeritus.is_none(), "it isn't nudged or retired");
    assert_eq!(b.workspace.unwrap().branch, "fix/x");
}

// Other agents' messages waiting for the predecessor are its
// successor's, named by sender; what the user typed never is.
#[test]
fn what_waits_goes_on_but_never_what_you_typed() {
    let ex = |id: &str| {
        Some(Box::new(Exchange {
            id: id.into(),
            sender: Peer {
                session_id: "p1".into(),
                name: "Parent".into(),
                ..Peer::default()
            },
            ..Exchange::default()
        }))
    };
    let i = Info {
        queue: vec!["next task".into(), "from you".into()],
        queue_exchanges: vec![ex("x1"), None],
        ..Info::default()
    };
    let f = from_agents(&i);
    assert_eq!(
        f,
        vec![Forwarded {
            id: "x1".into(),
            from: "Parent (session p1)".into(),
            text: "next task".into(),
        }]
    );
    assert!(user_queued(&i));
    let agents_only = Info {
        queue: vec!["next task".into()],
        queue_exchanges: vec![ex("x1")],
        ..Info::default()
    };
    assert!(!user_queued(&agents_only) && !user_queued(&Info::default()));
    let a = Config {
        id: "a1".into(),
        name: "fix it".into(),
        ..Config::default()
    };
    let p = successor_prompt(&a, &baton(), "/t/a1.jsonl", &f);
    for want in [
        "\"fix it\" (rush session a1)",
        "## Destination",
        "/t/a1.jsonl",
        "never read it whole",
        "From Parent (session p1):\nnext task",
    ] {
        assert!(p.contains(want), "{want} in {p}");
    }
    assert!(!p.contains("from you"), "{p}");
}

// The facts are read, not remembered: the branch, HEAD and what changed
// in a repository; that it's none outside one; the transcript and cost
// either way.
#[test]
fn the_facts_are_reads() {
    let h = Home::new();
    let repo = h.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let r = repo.to_string_lossy().into_owned();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&r)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first commit"]);
    std::fs::write(repo.join("new.txt"), "x").unwrap();
    let f = facts(&r, "/t/a.jsonl", 2.5, 307_000);
    for want in [
        "- Branch: main",
        "first commit",
        "?? new.txt",
        "- Transcript: /t/a.jsonl",
        "$2.50, context 307000 tokens",
    ] {
        assert!(f.contains(want), "{want} in {f}");
    }
    let plain = h.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let f = facts(&plain.to_string_lossy(), "", 0.0, 1);
    assert!(
        f.contains("- Not a git repository") && !f.contains("Transcript"),
        "{f}"
    );
}

// Retired is terminal at every way in: a start with its own config or a
// fresh one, a wake, a host's start (which marks it stopped); and what's
// sent there is refused with where its work went, not passed on.
#[test]
fn retired_is_terminal_at_every_way_in() {
    let _h = Home::new();
    let a = session("ret1", |_| {});
    info("ret1", |i| i.state = "idle".into());
    retired_to("ret1", "suc1");
    let why = refused("ret1").expect("refused");
    assert!(
        why.contains("suc1") && why.contains("nothing sent here is passed on"),
        "{why}"
    );
    let fresh = Config {
        id: "ret1".into(),
        kind: "fake".into(),
        ..Config::default()
    };
    let saved = read_config("ret1").unwrap();
    let entries: Vec<(&str, String)> = vec![
        (
            "spawn, its own config",
            crate::client::spawn(saved).unwrap_err().to_string(),
        ),
        (
            "spawn, a fresh config",
            crate::client::spawn(fresh).unwrap_err().to_string(),
        ),
        (
            "ensure",
            crate::client::ensure("ret1", "").unwrap_err().to_string(),
        ),
        (
            "host start",
            crate::server::run_with(
                crate::fake::registry(),
                crate::server::Hooks::quiet(),
                "ret1",
            )
            .unwrap_err(),
        ),
    ];
    for (name, err) in entries {
        assert!(err.contains("is retired"), "{name}: {err}");
    }
    let i = read_info_file("ret1").unwrap();
    assert!(
        i.state == "stopped" && i.emeritus == "retired" && i.successor == "suc1",
        "{i:?}"
    );
    // A successor rolled back is retired too, with why.
    session("void1", |c| {
        c.emeritus = Some(Emeritus {
            retired_at: Time::now(),
            why: "its start was rolled back".into(),
            ..Emeritus::default()
        })
    });
    assert_eq!(
        refused("void1").unwrap(),
        "session void1 is retired: its start was rolled back"
    );
    assert!(refused("nosuch").is_none() && refusal(&a).is_none());
}

// A chain of successors is followed to the one that carries on; an
// agent waited on that handed over is answered by its successor, and
// the caller is told it handed over.
#[test]
fn a_successor_answers_for_its_predecessor() {
    let _h = Home::new();
    for id in ["ch1", "ch2", "ch3"] {
        session(id, |_| {});
    }
    retired_to("ch1", "ch2");
    retired_to("ch2", "ch3");
    assert_eq!(live("ch1"), "ch3");
    assert_eq!(live("ch3"), "ch3");
    assert_eq!(successor("ch1").as_deref(), Some("ch2"));
    info("ch3", |_| {});
    let a = Answer {
        at: Time::now(),
        text: "done by the successor".into(),
        ..Answer::default()
    };
    std::fs::write(answer_path("ch3"), serde_json::to_vec(&a).unwrap()).unwrap();
    let reg = crate::fake::registry();
    let got = await_answer(&reg, "ch1", "p1", Time::default(), Duration::from_secs(1)).unwrap();
    assert!(
        got.contains("(ch1) handed over to") && got.contains("(ch2) handed over to"),
        "{got}"
    );
    assert!(got.ends_with("finished:\n\ndone by the successor"), "{got}");
}

// What the predecessor started, what's owed to it and its reminders
// move to its successor; others' stay; done again, nothing more moves.
#[test]
fn what_was_its_moves_to_its_successor() {
    let _h = Home::new();
    rush_remind::no_waiters(true);
    session("mova", |_| {});
    session("movb", |_| {});
    let meta = |k: &str, v: &str| -> std::collections::BTreeMap<String, String> {
        [(k.to_string(), v.to_string())].into()
    };
    session("kid1", |c| c.meta = meta("spawnedBy", "mova"));
    info("kid1", |i| i.meta = meta("spawnedBy", "mova"));
    session("sib1", |c| c.meta = meta("startedFrom", "mova"));
    session("other", |c| c.meta = meta("spawnedBy", "elsewhere"));
    report("kid1", "mova").unwrap();
    report("other", "elsewhere").unwrap();
    rush_remind::add(
        "mova",
        "check the deploy",
        Time::now() + Duration::from_secs(600),
        Time::now(),
    )
    .unwrap();
    let mut moved = move_children("mova", "movb");
    moved.sort();
    assert_eq!(moved, ["kid1", "sib1"]);
    let parent = |id: &str, k: &str| {
        read_config(id)
            .unwrap()
            .meta
            .get(k)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(parent("kid1", "spawnedBy"), "movb");
    assert_eq!(read_info_file("kid1").unwrap().meta["spawnedBy"], "movb");
    assert_eq!(parent("sib1", "startedFrom"), "movb");
    assert_eq!(parent("other", "spawnedBy"), "elsewhere");
    assert_eq!(
        std::fs::read_to_string(report_path("kid1")).unwrap(),
        "movb"
    );
    assert_eq!(
        std::fs::read_to_string(report_path("other")).unwrap(),
        "elsewhere"
    );
    assert!(rush_remind::list("mova").is_empty());
    assert_eq!(rush_remind::list("movb")[0].text, "check the deploy");
    assert!(move_children("mova", "movb").is_empty(), "moved twice");
}

// A handover its host was cut short in is settled at its next start:
// committed when the successor had started (or the predecessor's
// config already says so), rolled back otherwise with the successor
// retired. Never two owners, never none.
#[test]
fn a_handover_cut_short_is_settled() {
    let _h = Home::new();
    struct Case {
        name: &'static str,
        phase: Phase,
        /// The successor's info, when it has one.
        to: Option<fn(&mut Info)>,
        from_retired: bool,
        want: Phase,
    }
    let cases = [
        Case {
            name: "frozen, no successor yet",
            phase: Phase::Frozen,
            to: None,
            from_retired: false,
            want: Phase::RolledBack,
        },
        Case {
            name: "successor answered",
            phase: Phase::Starting,
            to: Some(|i| i.context_tokens = 21_000),
            from_retired: false,
            want: Phase::Committed,
        },
        Case {
            name: "successor never answered, its host gone",
            phase: Phase::Starting,
            to: Some(|i| i.state = "working".into()),
            from_retired: false,
            want: Phase::RolledBack,
        },
        Case {
            name: "successor failed",
            phase: Phase::Starting,
            to: Some(|i| i.error = "not signed in".into()),
            from_retired: false,
            want: Phase::RolledBack,
        },
        Case {
            name: "committed but not journalled",
            phase: Phase::Starting,
            to: Some(|i| i.context_tokens = 21_000),
            from_retired: true,
            want: Phase::Committed,
        },
    ];
    for (n, c) in cases.iter().enumerate() {
        let (a, b) = (format!("rca{n}"), format!("rcb{n}"));
        session(&a, |cfg| {
            cfg.meta.insert("spawnedBy".into(), "par".into());
        });
        let mut j = Journal {
            phase: c.phase,
            from: a.clone(),
            at: Time::now(),
            from_context: 310_000,
            ..Journal::default()
        };
        if let Some(f) = c.to {
            session(&b, |_| {});
            info(&b, f);
            j.to = b.clone();
        }
        if c.from_retired {
            retired_to(&a, &b);
        }
        write_journal(&a, &j).unwrap();
        assert_eq!(recover(&a), Some(c.want), "{}", c.name);
        assert_eq!(read_journal(&a).unwrap().phase, c.want, "{}", c.name);
        let from = read_config(&a).unwrap();
        match c.want {
            Phase::Committed => {
                assert_eq!(successor(&a).as_deref(), Some(b.as_str()), "{}", c.name);
            }
            _ => {
                assert!(!from.retired(), "{}: the predecessor carries on", c.name);
                if c.to.is_some() {
                    let v = read_config(&b).unwrap();
                    assert!(
                        v.retired() && v.emeritus.unwrap().why.contains("rolled back"),
                        "{}",
                        c.name
                    );
                }
            }
        }
        assert_eq!(recover(&a), None, "{}: settled twice", c.name);
    }
    let evs = rush_efficiency::events::load_events();
    let handed: Vec<_> = evs.iter().filter_map(|e| e.handover.as_ref()).collect();
    assert_eq!(handed.len(), 2, "{evs:?}");
    assert!(
        handed
            .iter()
            .all(|h| h.from_context == 310_000 && h.to_context == 21_000 && h.subagent),
        "{handed:?}"
    );
}
