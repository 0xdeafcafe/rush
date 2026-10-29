//! The TUI's hosts against rush-host's fake host, run in this process:
//! a session started, dialled and followed, sent to, answered and sent to
//! without a view. Never `rush host run`, never a real agent; the rush
//! folder is a temp one of this thread's.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

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
