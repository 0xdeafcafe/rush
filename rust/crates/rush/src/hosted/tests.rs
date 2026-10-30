//! The TUI's hosts against rush-host's fake host, run in this process:
//! a session started, dialled and followed, sent to, answered and sent to
//! without a view; the cards' ops, a rewind, a compaction, a fork and a
//! hand-off. Never `rush host run`, never a real agent; the rush folder is
//! a temp one of this thread's.

use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use rush_core::event::Event;
use rush_host::fake;

use super::*;

/// A rush folder for this thread under /tmp, where a socket's path fits.
struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Home {
        let d = tempfile::Builder::new()
            .prefix("rh-")
            .tempdir_in("/tmp")
            .unwrap();
        let p = std::fs::canonicalize(d.path()).unwrap();
        rush_state::set_thread_dir(Some(p.clone()));
        rush_state::set_thread_cache_dir(Some(p.join("cache")));
        Home(d)
    }
    fn path(&self) -> std::path::PathBuf {
        std::fs::canonicalize(self.0.path()).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
        rush_state::set_thread_cache_dir(None);
    }
}

fn spawn_fake(reg: &Registry, cfg: Config) -> Result<Config, String> {
    fake::spawn(reg.clone(), cfg).map_err(|e| e.to_string())
}

/// The fake host never sleeps here: it's up, so there's nothing to wake.
fn ensure_fake(_: &Registry, id: &str) -> Result<(), String> {
    rush_host::read_info(id)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn hosts() -> RushHosts {
    RushHosts::with(
        fake::registry(),
        Starts {
            spawn: spawn_fake,
            ensure: ensure_fake,
            ..Starts::default()
        },
    )
}

fn start(h: &RushHosts, home: &Home, prompt: &str) -> Started {
    let acct = home.path().join("acct");
    std::fs::create_dir_all(&acct).unwrap();
    h.start(&Start {
        kind: "fake".into(),
        cwd: home.path().to_string_lossy().into_owned(),
        prompt: prompt.into(),
        account: Some(("default".into(), acct.to_string_lossy().into_owned())),
        ..Start::default()
    })
    .expect("started")
}

fn agent(id: &str) -> Agent {
    let mut a = Agent {
        rush: true,
        kind: "fake".into(),
        display_name: "the fake".into(),
        ..Agent::default()
    };
    a.job.id = id.into();
    a
}

/// Follows link on a thread of its own, handing each line over.
fn follow(link: Arc<dyn Link>) -> Receiver<Said> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        link.follow(&mut |batch| batch.into_iter().all(|s| tx.send(s).is_ok()));
    });
    rx
}

/// Reads rx until want takes a line; panics after ten seconds.
fn until(rx: &Receiver<Said>, mut want: impl FnMut(&Said) -> bool) -> Vec<Said> {
    let mut seen = Vec::new();
    loop {
        let s = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("never came; saw {seen:?}"));
        let done = want(&s);
        seen.push(s);
        if done {
            return seen;
        }
    }
}

// A session started on the fake agent is named from its task, runs it,
// and its host's config says what it was started with (startHosted).
#[test]
fn starts_a_session() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "fix the flaky test");
    assert!(!st.id.is_empty());
    assert_eq!(st.name, "fix the flaky test");
    assert_eq!(st.acct, "default");
    let cfg = rush_host::read_config(&st.id).unwrap();
    assert_eq!(cfg.kind, "fake");
    assert_eq!(cfg.prompt, "fix the flaky test");
    assert!(!cfg.name_first);
    // An agent rush doesn't know is refused before anything starts.
    let e = h
        .start(&Start {
            kind: "nosuch".into(),
            ..Start::default()
        })
        .unwrap_err();
    assert!(
        e.starts_with("rush doesn't know the agent \"nosuch\""),
        "{e}"
    );
}

// Dialled, the host replays what the session said up to its info; a
// message runs the fake's command, asks to run it, and an answer ends the
// turn (onHostLines, answerHost).
#[test]
fn follows_and_answers_a_session() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let a = agent(&st.id);
    assert!(h.hosts(&a) && !h.sleeping(&a));
    let link = h.dial(&a).expect("dialled");
    let rx = follow(link.clone());
    until(&rx, |s| matches!(s, Said::Info(_)));
    link.op(Op::Send {
        text: "hi".into(),
        images: Vec::new(),
        now: false,
    })
    .unwrap();
    let seen = until(&rx, |s| matches!(s, Said::Event(Event::Approval(_))));
    assert!(
        seen.iter()
            .any(|s| matches!(s, Said::Sent { text, .. } if text == "hi")),
        "{seen:?}"
    );
    link.op(Op::Allow {
        id: "ap1".into(),
        always: false,
    })
    .unwrap();
    let seen = until(&rx, |s| matches!(s, Said::Event(Event::TurnEnd(_))));
    assert!(
        seen.iter()
            .any(|s| matches!(s, Said::Answered(id) if id == "ap1")),
        "{seen:?}"
    );
    link.close();
}

// Sent to without a view: queued, as from the box or a spawned agent's
// parent (sendHostedID).
#[test]
fn sends_without_a_view() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let link = h.dial(&agent(&st.id)).unwrap();
    let rx = follow(link.clone());
    until(&rx, |s| matches!(s, Said::Info(_)));
    h.send_to(&st.id, "from the parent", false).unwrap();
    until(
        &rx,
        |s| matches!(s, Said::Sent { text, .. } if text == "from the parent"),
    );
    // A reply to a rush session goes through its host too: queued, as
    // the session waits on its approval.
    h.reply(&agent(&st.id), "and again").unwrap();
    until(
        &rx,
        |s| matches!(s, Said::Info(i) if i.queue == ["and again"]),
    );
    link.close();
}

// Waking a host that's up sends what was typed over a new connection.
#[test]
fn wakes_with_a_message() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let link = h
        .wake(&agent(&st.id), "carry on", &[], false)
        .expect("woken");
    let rx = follow(link.clone());
    until(
        &rx,
        |s| matches!(s, Said::Sent { text, .. } if text == "carry on"),
    );
    link.close();
}

// The host's info as the Session keeps it.
#[test]
fn info_as_the_session_keeps_it() {
    let i = rush_host::Info {
        queue: vec!["a".into()],
        queue_held: true,
        state: "working".into(),
        model: "fake-2".into(),
        claude_pid: 7,
        sleeping: true,
        ..rush_host::Info::default()
    };
    let got = info(&i);
    assert_eq!(got.queue, ["a"]);
    assert!(got.queue_held && got.sleeping);
    assert_eq!(
        (got.state.as_str(), got.model.as_str()),
        ("working", "fake-2")
    );
    assert_eq!((got.agent_pid, got.queue_stuck), (7, 0));
}

// The sessions rush hosted for a parent's shell: only that parent's, the
// prompt and start its request exchange says, its transcript where its
// agent keeps it (spawns.go hostedRuns).
#[test]
fn hosted_runs_are_the_parents_own() {
    let home = Home::new();
    let reg = Registry::new();
    let claude_dir = home.path().join("claude");
    reg.register(rush_claude::Claude::at(rush_claude::Account::at(
        "",
        &claude_dir,
    )));
    let account = Profile {
        kind: "claude".into(),
        name: String::new(),
        dir: claude_dir.to_string_lossy().into_owned(),
    };
    let write = |id: &str, prompt: &str| {
        let d = rush_host::dir(id);
        std::fs::create_dir_all(&d).unwrap();
        let cfg = Config {
            id: id.into(),
            kind: "claude".into(),
            cwd: "/w".into(),
            prompt: prompt.into(),
            account: account.clone(),
            ..Config::default()
        };
        std::fs::write(d.join("config.json"), cfg.to_json_indent()).unwrap();
    };
    write("h1", "first message");
    write("h2", "other's");
    rush_host::exchange_journal::append_exchange(
        "h1",
        &rush_core::event::Exchange {
            id: "x1".into(),
            at: 5_000,
            phase: "request".into(),
            text: "what the call asked".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let info = |id: &str, parent: &str, state: &str| rush_host::Info {
        id: id.into(),
        session_id: format!("s-{id}"),
        state: state.into(),
        model: "opus".into(),
        started_at: rush_state::Time::from_ms(9_000),
        meta: [
            ("spawnedBy".to_string(), parent.to_string()),
            ("spawnStep".to_string(), "toolu_1".to_string()),
        ]
        .into(),
        ..rush_host::Info::default()
    };
    let infos = [
        info("h1", "p", "working"),
        info("h2", "q", "working"),
        info("h3", "p", "idle"), // no config: left out
    ];
    let got = hosted_runs(&reg, &infos, "p");
    assert_eq!(got.len(), 1, "{got:?}");
    let s = &got[0];
    assert_eq!(
        (s.id.as_str(), s.step.as_str(), s.kind.as_str()),
        ("h1", "toolu_1", "claude")
    );
    assert_eq!((s.prompt.as_str(), s.born), ("what the call asked", 5_000));
    assert_eq!((s.model.as_str(), s.cwd.as_str()), ("opus", "/w"));
    assert!(s.live);
    assert!(s.transcript.ends_with("s-h1.jsonl"), "{}", s.transcript);
}

/// Reads session id's host info until want takes it; panics after ten
/// seconds.
fn info_until(id: &str, want: impl Fn(&rush_host::Info) -> bool) -> rush_host::Info {
    let until = Instant::now() + Duration::from_secs(10);
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[&rush_host::dir(id)]);
    loop {
        if let Ok(i) = rush_host::read_info(id)
            && want(&i)
        {
            return i;
        }
        let now = Instant::now();
        assert!(now < until, "the host's info never came round");
        w.wait(None, Some((until - now).min(Duration::from_millis(50))));
    }
}

// The cards' ops go over the socket: a decisions card set aside, #slim's
// without, and a control request whose answer (here, that the fake can't
// be asked) comes back by its id (client.go DismissDecisions, SetWithout,
// Ask; applyHostLine's Reply).
#[test]
fn the_cards_ops_and_a_reply() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let link = h.dial(&agent(&st.id)).unwrap();
    let rx = follow(link.clone());
    until(&rx, |s| matches!(s, Said::Info(_)));
    link.op(Op::DismissDecisions("d1".into())).unwrap();
    link.op(Op::Without(vec!["Skill(pdf)".into()])).unwrap();
    until(
        &rx,
        |s| matches!(s, Said::Info(i) if i.now.without == ["Skill(pdf)"]),
    );
    link.op(Op::Ask {
        id: "ui-1".into(),
        req: r#"{"subtype":"export"}"#.into(),
    })
    .unwrap();
    let seen = until(&rx, |s| matches!(s, Said::Reply { .. } | Said::Error(_)));
    assert!(
        matches!(seen.last(), Some(Said::Reply { id, body: Err(_) }) if id == "ui-1")
            || matches!(seen.last(), Some(Said::Error(e)) if e.contains("ask")),
        "{seen:?}"
    );
    link.close();
}

// A rewind has the host carry on from another conversation
// and returns once the host has hung up (rewindHost,
// hangUp).
#[test]
fn rewinds_and_keeps_the_path_left() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    info_until(&st.id, |i| i.state == "idle");
    let left = Branch {
        session_id: String::new(),
        left: 1_000_000,
        from: 1,
        turns: 2,
        last: "the last".into(),
    };
    let restarted = h.rewind(&st.id, "fresh-1", false, &left).expect("rewound");
    assert!(!restarted);
    // The fake keeps no transcript, so there's no path to keep, nor any
    // for /rewind to offer.
    let cfg = rush_host::read_config(&st.id).unwrap();
    assert_eq!(cfg.session_id, "fresh-1");
    assert!(h.branches(&agent(&st.id)).is_empty());
}

// A compaction carries the session on from the summary in a fresh
// conversation, only for the idle one it was made from: a stale
// expectation is refused (compactBy's carry, CompactedIfUnchanged).
#[test]
fn compacts_only_the_conversation_expected() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let i = info_until(&st.id, |i| i.state == "idle" && !i.session_id.is_empty());
    let left = Branch::default();
    let mut stale = info(&i);
    stale.now.updated_ns -= 1;
    let e = h
        .compacted(&st.id, "fresh-2", "the summary", &left, &stale)
        .unwrap_err();
    assert!(
        e.starts_with("the session changed while summarising"),
        "{e}"
    );
    h.compacted(&st.id, "fresh-2", "the summary", &left, &info(&i))
        .expect("compacted");
    assert_eq!(
        rush_host::read_config(&st.id).unwrap().session_id,
        "fresh-2"
    );
}

// A fork runs with what was picked, else what the session runs with now,
// resumed; a hand-off to an agent rush doesn't know is refused before
// anything starts (forksheet.go start, handoffTo's UseAgent).
#[test]
fn forks_and_hands_off() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let acct = home.path().join("acct");
    let f = Fork {
        name: "the fork".into(),
        cwd: home.path().to_string_lossy().into_owned(),
        acct: Profile {
            kind: "fake".into(),
            name: "default".into(),
            dir: acct.to_string_lossy().into_owned(),
        },
        effort: "high".into(),
        prompt: "go on".into(),
        ..Fork::default()
    };
    let forked = h.fork(&agent(&st.id), &f).expect("forked");
    let cfg = rush_host::read_config(&forked.id).unwrap();
    assert_eq!(
        (cfg.name.as_str(), cfg.effort.as_str(), cfg.prompt.as_str()),
        ("the fork", "high", "go on")
    );
    assert!(cfg.resume);
    let cwd = home.path().to_string_lossy().into_owned();
    let e = h.handoff("nosuch", &cwd, "x", "told").unwrap_err();
    assert!(e.starts_with("rush doesn't know the agent"), "{e}");
}

// A conversation handed over: the new session starts on it told, with
// your next instruction after it, and the quiet one it came from is
// stopped; one at work is left as it is (handOverMessage).
#[test]
fn hands_a_conversation_over() {
    use rush_core::adapter::Conversation;
    use rush_core::agent::PreviewLine;
    let home = Home::new();
    let h = hosts();
    let src = start(&h, &home, "");
    let link = h.dial(&agent(&src.id)).unwrap();
    let rx = follow(link.clone());
    until(&rx, |s| matches!(s, Said::Info(i) if i.state == "idle"));
    let line = |role: &str, text: &str| PreviewLine {
        role: role.into(),
        text: text.into(),
        ..PreviewLine::default()
    };
    let acct = home.path().join("acct");
    let mut ho = HandOver {
        start: Start {
            kind: "fake".into(),
            cwd: home.path().to_string_lossy().into_owned(),
            name: "the fake · on Fake".into(),
            account: Some(("default".into(), acct.to_string_lossy().into_owned())),
            ..Start::default()
        },
        from: src.id.clone(),
        retire: true,
        conv: Conversation {
            from: "claude".into(),
            name: "the fake".into(),
            first: "fix the flaky test".into(),
            recent: vec![
                line("user", "fix the flaky test"),
                line("assistant", "Fixed."),
            ],
            ..Conversation::default()
        },
        message: "and the tests".into(),
    };
    let done = h.hand_over(&ho).expect("handed over");
    assert!(done.retired, "the quiet source is stopped");
    assert_eq!(done.started.name, "the fake · on Fake");
    let cfg = rush_host::read_config(&done.started.id).unwrap();
    assert!(cfg.prompt.contains("fix the flaky test"), "{}", cfg.prompt);
    assert!(
        cfg.prompt
            .ends_with("\n\nThe user's next instruction:\nand the tests"),
        "{}",
        cfg.prompt
    );
    assert!(cfg.carry.is_none(), "the fake doesn't take it whole");
    // The source stops: its last info comes down the link, unless the
    // socket closes first; the UI then reads the info it saved.
    let stopped = |s: &Said| matches!(s, Said::Info(i) if i.state == "stopped");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(s) if stopped(&s) => break,
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let info = rush_host::read_info(&src.id).unwrap();
                assert_eq!(info.state, "stopped", "the source is stopped");
                break;
            }
            Err(e) => panic!("the source never stopped: {e}"),
        }
    }
    link.close();

    // One at work isn't stopped under it.
    let busy = start(&h, &home, "");
    let link = h.dial(&agent(&busy.id)).unwrap();
    let rx = follow(link.clone());
    until(&rx, |s| matches!(s, Said::Info(_)));
    link.op(Op::Send {
        text: "hi".into(),
        images: Vec::new(),
        now: false,
    })
    .unwrap();
    until(&rx, |s| matches!(s, Said::Event(Event::Approval(_))));
    ho.from = busy.id.clone();
    let e = h.hand_over(&ho).unwrap_err();
    assert_eq!(
        e,
        "source session is busy or has queued messages; switch after it finishes"
    );
    link.close();
}

// A session rush doesn't run moved to rush mode: resumed under a host of
// rush's with its name and folder, or for one open in a terminal, a
// copy forked from it (moveToRushWith).
#[test]
fn adopts_a_session() {
    let home = Home::new();
    let h = hosts();
    let mut a = Agent {
        kind: "fake".into(),
        key: "default/s1".into(),
        display_name: "port the parser".into(),
        acct: Profile {
            kind: "fake".into(),
            name: "default".into(),
            dir: home.path().join("acct").to_string_lossy().into_owned(),
        },
        ..Agent::default()
    };
    a.job.session_id = "conv-1".into();
    a.job.cwd = home.path().to_string_lossy().into_owned();
    let st = h.adopt(&a, "carry on").expect("moved");
    assert_eq!(st.name, "port the parser");
    let cfg = rush_host::read_config(&st.id).unwrap();
    assert!(cfg.resume && !cfg.fork);
    assert_eq!(cfg.session_id, "conv-1");
    assert_eq!(cfg.prompt, "carry on");
    a.interactive = true;
    let st = h.adopt(&a, "").expect("copied");
    let cfg = rush_host::read_config(&st.id).unwrap();
    assert!(cfg.fork);
    assert_eq!(cfg.from, "conv-1");
    a.job.session_id.clear();
    assert_eq!(
        h.adopt(&a, "").unwrap_err(),
        "can't find port the parser's conversation to resume"
    );
}

// Moved to another folder, a rush session's host lists it there at once
// (cdHosted).
#[test]
fn cd_moves_a_session() {
    let home = Home::new();
    let h = hosts();
    let st = start(&h, &home, "");
    let to = home.path().join("elsewhere");
    std::fs::create_dir_all(&to).unwrap();
    let to = to.to_string_lossy().into_owned();
    let c = Client::dial(&st.id).unwrap();
    h.cd(&st.id, &to, false).unwrap();
    // The host says so to whoever follows it.
    rush_host::client::await_line(
        &c,
        Duration::from_secs(10),
        |l| matches!(l, Line::Info(i) if i.cwd == to),
    )
    .expect("moved");
    c.close();
    assert_eq!(rush_host::read_config(&st.id).unwrap().cwd, to);
}
