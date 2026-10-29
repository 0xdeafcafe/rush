//! The host, driven through a fake agent: host_test.go, agent_test.go,
//! events_test.go, workspace_test.go and the queue's and rewind's tests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rush_core::adapter::{Adapter, Driver, Features, Has, Kind, Level, Support};
use rush_core::tool;

use super::*;
use crate::client::Client;
use crate::lines::Line;
use crate::testing::Home;

/// An agent other than Claude Code: each message runs a command it asks to
/// run, then ends the turn. Its starts are logged to starts.log beside its
/// profile, with the session it resumed (agent_test.go's fakeAgent).
#[derive(Clone)]
pub(crate) struct Fake {
    pub kind: &'static str,
    pub features: Vec<Feature>,
}

impl Default for Fake {
    fn default() -> Fake {
        Fake {
            kind: "fake",
            features: vec![
                Feature::Run,
                Feature::Prompt,
                Feature::Resume,
                Feature::Rewind,
            ],
        }
    }
}

impl Adapter for Fake {
    fn kind(&self) -> Kind {
        Kind::from(self.kind)
    }
    fn name(&self) -> String {
        "Fake".into()
    }
    fn features(&self) -> Features {
        self.features.iter().map(|f| (*f, Support::YES)).collect()
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(driver: Driver);
}

impl Driver for Fake {
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        let resumed = if o.resume {
            o.session_id.clone()
        } else {
            String::new()
        };
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(&o.profile.dir).join("starts.log"))?;
        writeln!(f, "{}", format!("start {resumed}").trim())?;
        sink.event(Event::Init(Box::new(event::Init {
            session_id: "fake-session".into(),
            model: "fake-1".into(),
            ..Default::default()
        })));
        Ok(Box::new(FakeConn {
            sink,
            closed: AtomicUsize::new(0),
        }))
    }
}

struct FakeConn {
    sink: Arc<dyn Sink>,
    closed: AtomicUsize,
}

impl Conn for FakeConn {
    fn send(&self, input: Input) -> adapter::Result<()> {
        let call = tool::Call {
            id: "c1".into(),
            name: "shell".into(),
            kind: tool::Kind::Shell,
            input: tool::Input {
                command: format!("echo {}", input.text),
                ..Default::default()
            },
            ..Default::default()
        };
        self.sink.event(Event::MessageStart(event::MessageStart {
            id: "m1".into(),
            ..Default::default()
        }));
        self.sink.event(Event::Delta(event::Delta {
            kind: event::PartKind::Text,
            text: "On it".into(),
            ..Default::default()
        }));
        self.sink.event(Event::Message(event::Message {
            role: "assistant".into(),
            id: "m1".into(),
            parts: vec![
                event::Part {
                    kind: event::PartKind::Text,
                    text: "On it".into(),
                    ..Default::default()
                },
                event::Part {
                    kind: event::PartKind::ToolCall,
                    call: Some(Box::new(call.clone())),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }));
        self.sink.event(Event::Approval(Box::new(event::Approval {
            id: "ap1".into(),
            call,
            options: vec![
                ApprovalOption {
                    id: "yes".into(),
                    kind: OptionKind::AllowOnce,
                    ..Default::default()
                },
                ApprovalOption {
                    id: "no".into(),
                    kind: OptionKind::RejectOnce,
                    ..Default::default()
                },
            ],
            ..Default::default()
        })));
        Ok(())
    }
    fn answer(&self, _id: &str, option: &str) -> adapter::Result<()> {
        let exit = if option == "no" { 1 } else { 0 };
        self.sink.event(Event::Message(event::Message {
            role: "user".into(),
            parts: vec![event::Part {
                kind: event::PartKind::ToolResult,
                output: Some(Box::new(tool::Output {
                    call_id: "c1".into(),
                    text: format!("answered {option}"),
                    exit: Some(exit),
                    is_error: exit != 0,
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        }));
        self.sink.event(Event::TurnEnd(event::TurnEnd {
            reason: "done".into(),
            cost: 0.02,
            ..Default::default()
        }));
        Ok(())
    }
    fn interrupt(&self) -> adapter::Result<()> {
        Ok(())
    }
    fn set_model(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn set_mode(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn close(&self) -> adapter::Result<()> {
        if self.closed.fetch_add(1, Ordering::SeqCst) == 0 {
            self.sink.ended(None);
        }
        Ok(())
    }
}

/// A conn that keeps what it's sent and counts the turns it was asked to
/// stop (host_test.go's inputConn and interruptConn).
#[derive(Default)]
struct Kept {
    got: Mutex<Vec<Input>>,
    stops: AtomicUsize,
}

impl Conn for Kept {
    fn send(&self, input: Input) -> adapter::Result<()> {
        self.got.lock().unwrap().push(input);
        Ok(())
    }
    fn answer(&self, _: &str, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn interrupt(&self) -> adapter::Result<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn set_model(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn set_mode(&self, _: &str) -> adapter::Result<()> {
        Ok(())
    }
    fn close(&self) -> adapter::Result<()> {
        Ok(())
    }
}

/// A registry with the fake agent, as kind fake, and as claude with what
/// Claude Code can do mid-turn.
fn registry() -> Registry {
    let reg = Registry::new();
    reg.register(Fake::default());
    reg.register(Fake {
        kind: "claude",
        features: vec![
            Feature::Run,
            Feature::Interrupt,
            Feature::Guide,
            Feature::Rewind,
            Feature::Prompt,
        ],
    });
    reg
}

/// A host with no socket, for session cfg, its folder made.
fn host(cfg: Config) -> Host {
    std::fs::create_dir_all(dir(&cfg.id)).unwrap();
    let (h, _rx) = build(registry(), Hooks::quiet(), cfg);
    h
}

/// h with conn as its running agent.
fn with_conn(h: &Host, conn: Arc<dyn Conn>) {
    let mut st = h.lock();
    st.next_key += 1;
    st.conn_key = st.next_key;
    st.conn = Some(conn);
}

fn queue(h: &Host) -> String {
    h.lock().info.queue.join("|")
}

fn op(name: &str, f: impl FnOnce(&mut Op)) -> Op {
    let mut o = Op::named(name);
    f(&mut o);
    o
}

#[test]
fn turn_cost_takes_the_rise_in_a_running_total() {
    let mut spent = 0.0;
    assert_eq!(turn_cost(&mut spent, 0.5), 0.5);
    assert_eq!(turn_cost(&mut spent, 0.75), 0.25);
    assert_eq!(
        turn_cost(&mut spent, 0.1),
        0.1,
        "a new process counts from zero"
    );
}

// host_test.go:272
#[test]
fn queue_edits() {
    let _home = Home::new();
    let h = host(Config {
        id: "q".into(),
        ..Config::default()
    });
    h.lock().info.queue = ["a", "b", "c", "d"].map(String::from).to_vec();
    for (o, want) in [
        (
            op("queue_edit", |o| {
                o.index = 1;
                o.text = "B".into();
            }),
            "a|B|c|d",
        ),
        (
            op("queue_move", |o| {
                o.index = 3;
                o.to = 0;
            }),
            "d|a|B|c",
        ),
        (op("queue_merge", |o| o.index = 1), "d|a\n\nB|c"),
        (op("queue_remove", |o| o.index = 0), "a\n\nB|c"),
        (
            op("queue_move", |o| {
                o.index = 0;
                o.to = 9;
            }),
            "c|a\n\nB",
        ),
    ] {
        let name = o.op.clone();
        h.edit_queue(&mut h.lock(), o).unwrap();
        assert_eq!(queue(&h), want, "{name}");
    }
    assert!(
        h.edit_queue(&mut h.lock(), op("queue_merge", |o| o.index = 1))
            .is_err(),
        "merging the last item"
    );
    assert!(
        h.edit_queue(&mut h.lock(), op("queue_remove", |o| o.index = 5))
            .is_err(),
        "removing past the end"
    );
    // Named by its text, a message is found where it went.
    h.edit_queue(
        &mut h.lock(),
        op("queue_remove", |o| {
            o.index = 0;
            o.was = "a\n\nB".into();
        }),
    )
    .unwrap();
    assert_eq!(queue(&h), "c");
    let e = h
        .edit_queue(&mut h.lock(), op("queue_remove", |o| o.was = "gone".into()))
        .unwrap_err();
    assert_eq!(e, "that message has already been sent");
}

// host_test.go:310
#[test]
fn queued_images_go_with_their_message() {
    let _home = Home::new();
    let d = tempfile::tempdir().unwrap();
    let pic = d.path().join("shot.jpg").to_string_lossy().into_owned();
    std::fs::write(&pic, "jpeg").unwrap();
    let h = host(Config {
        id: "qi".into(),
        ..Config::default()
    });
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    h.lock().info.state = "working".into();
    h.send("first", &[], false).unwrap();
    h.send("look at this", std::slice::from_ref(&pic), false)
        .unwrap();
    assert!(c.got.lock().unwrap().is_empty());
    assert_eq!(h.lock().info.queue.len(), 2);
    h.send_queue(&mut h.lock());
    let got = c.got.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].images, vec![pic]);
    assert!(got[0].text.contains("look at this"));
    let st = h.lock();
    assert!(st.info.queue.is_empty() && st.info.queue_images.is_empty());
}

// host_test.go:538
#[test]
fn rewind_keeps_branches() {
    let _home = Home::new();
    let h = host(Config {
        id: "r".into(),
        session_id: "old".into(),
        resume: true,
        ..Config::default()
    });
    let mut st = h.lock();
    st.began = true;
    st.ring = vec![Arc::from(&br#"{"type":"assistant"}"#[..])];
    st.ring_n = 20;
    st.info.state = "working".into();
    assert!(
        h.rewind(
            &mut st,
            "cut",
            true,
            Some(&Branch {
                from: 3,
                ..Branch::default()
            })
        )
        .is_err(),
        "rewinding mid-turn"
    );
    st.info.state = "idle".into();
    let left = Branch {
        from: 3,
        turns: 5,
        last: "try the cache".into(),
        ..Branch::default()
    };
    h.rewind(&mut st, "cut", true, Some(&left)).unwrap();
    assert!(
        st.cfg.session_id == "cut"
            && st.began
            && st.ring.is_empty()
            && !st.info.rewound_at.is_zero()
            && st.info.session_id == "cut"
    );
    let b = &st.cfg.branches;
    assert!(
        b.len() == 1 && b[0].session_id == "old" && b[0].from == 3 && b[0].last == "try the cache",
        "{b:?}"
    );
    let saved = crate::client::read_config("r").unwrap();
    assert!(saved.session_id == "cut" && saved.branches.len() == 1);
    // Back down the old path: it stops being a branch, and "cut" becomes one.
    h.rewind(
        &mut st,
        "old",
        true,
        Some(&Branch {
            from: 3,
            turns: 2,
            ..Branch::default()
        }),
    )
    .unwrap();
    assert!(
        st.cfg.session_id == "old"
            && st.cfg.branches.len() == 1
            && st.cfg.branches[0].session_id == "cut"
    );
    // Before the first message (/clear): a fresh conversation, nothing to
    // resume, and no cache of its own to call cold.
    st.info.cache_warm = Time::now();
    st.info.context_tokens = 9000;
    h.rewind(
        &mut st,
        "fresh",
        false,
        Some(&Branch {
            from: 1,
            turns: 5,
            ..Branch::default()
        }),
    )
    .unwrap();
    assert!(
        !st.began
            && !st.cfg.resume
            && st.cfg.branches.len() == 2
            && st.info.cache_warm.is_zero()
            && st.info.context_tokens == 0
    );
}

// host_test.go:612
#[test]
fn ring_trims_whole_turns() {
    let _home = Home::new();
    let h = host(Config {
        id: "tr".into(),
        ..Config::default()
    });
    let big = format!(
        r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#,
        "x".repeat(1 << 20)
    );
    let mut st = h.lock();
    for turn in 0..5 {
        h.record(
            &mut st,
            lines::echo_line(&format!("turn {turn}"), &[], None),
        );
        for _ in 0..3 {
            h.record(&mut st, big.clone().into_bytes());
        }
    }
    assert!(st.ring_n <= RING_MAX, "ring holds {}", st.ring_n);
    let from = turn_start(&st.ring[0], &st.ring[1]).expect("the ring starts at a turn");
    assert_eq!(st.info.replay_from, from);
    assert!(
        String::from_utf8_lossy(&st.ring[1]).contains("turn 3"),
        "the ring keeps the last two turns"
    );
    // A turn bigger than the ring keeps its latest part and still says
    // when it began.
    let mut last = Time::default();
    for i in 0..st.ring.len() - 1 {
        if let Some(t) = turn_start(&st.ring[i], &st.ring[i + 1]) {
            last = t;
        }
    }
    for _ in 0..10 {
        h.record(&mut st, big.clone().into_bytes());
    }
    assert!(st.ring_n <= RING_MAX);
    assert_eq!(st.info.replay_from, last);
    // What's kept comes back out whole.
    let mut out = Vec::new();
    unpack(st.ring.last().unwrap(), &mut out).unwrap();
    assert_eq!(out, big.as_bytes());
}

// host_test.go:670: sent now mid-turn, a message stops the turn and waits
// in the queue after what was there; a queued one sent now goes first.
#[test]
fn send_now_mid_turn_cuts_in() {
    let _home = Home::new();
    let h = host(Config {
        id: "q".into(),
        kind: "claude".into(),
        ..Config::default()
    });
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    {
        let mut st = h.lock();
        st.info.state = "working".into();
        st.info.queue = vec!["a".into(), "b".into()];
        st.info.queue_held = true;
    }
    h.send("now", &[], true).unwrap();
    h.do_op(op("queue_send", |o| {
        o.index = 2;
        o.was = "b".into();
    }))
    .unwrap();
    assert_eq!(queue(&h), "b|a|now");
    assert!(!h.lock().info.queue_held);
    assert_eq!(c.stops.load(Ordering::SeqCst), 2);
}

// host_test.go:688
#[test]
fn steer_queued_does_not_stop() {
    let _home = Home::new();
    let h = host(Config {
        id: "sq".into(),
        kind: "claude".into(),
        ..Config::default()
    });
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    {
        let mut st = h.lock();
        st.info.state = "working".into();
        st.info.queue = vec!["a".into(), "b".into()];
    }
    h.do_op(op("queue_send", |o| {
        o.index = 1;
        o.was = "b".into();
        o.guide = true;
    }))
    .unwrap();
    assert_eq!(queue(&h), "a");
    let got = c.got.lock().unwrap();
    assert!(got.len() == 1 && got[0].text == "b", "{got:?}");
    assert_eq!(c.stops.load(Ordering::SeqCst), 0);
}

// host_test.go:703: each message's images stay its own.
#[test]
fn cut_in_keeps_images() {
    let _home = Home::new();
    let d = tempfile::tempdir().unwrap();
    let pic = d.path().join("shot.jpg").to_string_lossy().into_owned();
    std::fs::write(&pic, "jpeg").unwrap();
    let h = host(Config {
        id: "qc".into(),
        kind: "claude".into(),
        ..Config::default()
    });
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    {
        let mut st = h.lock();
        st.info.state = "working".into();
        st.info.queue_held = true;
    }
    h.send("a", &[], false).unwrap();
    h.send("b", std::slice::from_ref(&pic), false).unwrap();
    h.send("now", std::slice::from_ref(&pic), true).unwrap();
    h.do_op(op("queue_send", |o| {
        o.index = 2;
        o.was = "b".into();
    }))
    .unwrap();
    assert_eq!(queue(&h), "b|a|now");
    assert_eq!(
        h.lock().info.queue_images,
        vec![vec![pic.clone()], vec![], vec![pic]]
    );
    assert_eq!(c.stops.load(Ordering::SeqCst), 2);
}

// host_test.go:732: a message sent while idle, with the queue still
// waiting and not held, goes after what's queued, all of it together.
#[test]
fn idle_send_joins_the_queue() {
    let _home = Home::new();
    let h = host(Config {
        id: "iq".into(),
        kind: "claude".into(),
        ..Config::default()
    });
    let c = Arc::new(Kept::default());
    with_conn(&h, c.clone());
    {
        let mut st = h.lock();
        st.info.state = "idle".into();
        st.info.queue = vec!["a".into(), "b".into()];
    }
    h.send("new", &[], false).unwrap();
    assert!(h.lock().info.queue.is_empty());
    let got = c.got.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].text,
        join_queue(&["a".into(), "b".into(), "new".into()])
    );
}

// host_test.go:748
#[test]
fn named_from_first_message() {
    let _home = Home::new();
    let h = host(Config {
        id: "nf".into(),
        name: "fresh session in app".into(),
        name_first: true,
        ..Config::default()
    });
    with_conn(&h, Arc::new(Kept::default()));
    {
        let mut st = h.lock();
        st.info.state = "working".into();
        st.info.name = st.cfg.name.clone();
    }
    h.send("[Image #1] retry the upload <pasted_content id=\"a\">\nlots\n</pasted_content> when it times out please now", &[], false).unwrap();
    assert_eq!(h.lock().info.name, "retry the upload when it times");
    assert!(!h.lock().cfg.name_first);
    h.send("something else", &[], false).unwrap();
    assert_eq!(h.lock().info.name, "retry the upload when it times");
}

// host_test.go:765: /clear rewinds to a fresh conversation, named again by
// its next message.
#[test]
fn rewind_fresh_renames() {
    let _home = Home::new();
    let h = host(Config {
        id: "cl".into(),
        session_id: "old".into(),
        resume: true,
        cwd: "/work/app".into(),
        name: "fix the upload".into(),
        ..Config::default()
    });
    with_conn(&h, Arc::new(Kept::default()));
    {
        let mut st = h.lock();
        st.began = true;
        st.info.state = "idle".into();
        st.info.name = "fix the upload".into();
        h.rewind(
            &mut st,
            "new",
            false,
            Some(&Branch {
                from: 1,
                turns: 4,
                ..Branch::default()
            }),
        )
        .unwrap();
        assert!(
            st.info.name == "fresh session in app"
                && st.cfg.name_first
                && st.cfg.branches.len() == 1
                && st.cfg.branches[0].session_id == "old"
        );
    }
    // The rewind let its agent go: the next message starts one, a fake.
    h.lock().cfg.kind = "claude".into();
    let home = Home::new();
    h.lock().cfg.account.dir = home.path().to_string_lossy().into_owned();
    h.send("add retries to the upload", &[], false).unwrap();
    assert_eq!(h.lock().info.name, "add retries to the upload");
    let mut st = h.lock();
    st.info.state = "idle".into();
    h.rewind(
        &mut st,
        "old",
        true,
        Some(&Branch {
            from: 1,
            ..Branch::default()
        }),
    )
    .unwrap();
    assert!(!st.cfg.name_first && st.info.name == "add retries to the upload");
}

// host_test.go:789: #compact by another model carries on in a fresh
// conversation that starts with the summary, under the same name.
#[test]
fn compacted_keeps_name() {
    let home = Home::new();
    let h = host(Config {
        id: "cp".into(),
        kind: "fake".into(),
        session_id: "old".into(),
        resume: true,
        cwd: "/work/app".into(),
        name: "fix the upload".into(),
        account: Profile {
            kind: Kind::from("fake"),
            name: "fake".into(),
            dir: home.path().to_string_lossy().into_owned(),
        },
        ..Config::default()
    });
    with_conn(&h, Arc::new(Kept::default()));
    let (sid, at) = {
        let mut st = h.lock();
        st.began = true;
        st.info.state = "idle".into();
        st.info.name = "fix the upload".into();
        st.info.session_id = "old".into();
        h.publish(&mut st);
        (st.info.session_id.clone(), st.info.updated_at)
    };
    // One whose snapshot is out of date is refused.
    let stale = op("compacted_checked", |o| {
        o.text = "new".into();
        o.expected_session = "older".into();
        o.expected_updated_at = at;
    });
    assert!(
        h.do_op(stale)
            .unwrap_err()
            .starts_with("the session changed while summarising")
    );
    h.do_op(op("compacted_checked", |o| {
        o.text = "new".into();
        o.message = "summary: it was all about uploads".into();
        o.branch = Some(Branch {
            from: 1,
            turns: 4,
            ..Branch::default()
        });
        o.expected_session = sid;
        o.expected_updated_at = at;
    }))
    .unwrap();
    let st = h.lock();
    assert_eq!(st.cfg.session_id, "new");
    assert!(
        st.info.name == "fix the upload"
            && !st.cfg.name_first
            && st.cfg.branches.len() == 1
            && st.cfg.branches[0].session_id == "old"
    );
    let starts = std::fs::read_to_string(home.path().join("starts.log")).unwrap();
    assert_eq!(
        starts, "start\n",
        "the summary goes to a fresh conversation"
    );
}

// events_test.go:12
#[test]
fn events_log_state_changes_once() {
    let _home = Home::new();
    let h = host(Config {
        id: "ev".into(),
        ..Config::default()
    });
    let mut st = h.lock();
    st.logged = None;
    std::fs::remove_file(crate::paths::events_path("ev")).ok();
    crate::events::flush();
    std::fs::remove_file(crate::paths::events_path("ev")).ok();
    st.info.state = "working".into();
    h.note_state(&mut st);
    h.note_state(&mut st); // unchanged: not logged again
    st.info.state = "idle".into();
    st.info.background = vec![Task {
        id: "b1".into(),
        ..Task::default()
    }];
    h.note_state(&mut st);
    h.event(&st, "check-in", json!({"why": "long background tasks"}));
    crate::events::flush();
    let b = std::fs::read_to_string(crate::paths::events_path("ev")).unwrap();
    let (mut kinds, mut states) = (Vec::new(), Vec::new());
    for l in b.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        assert!(v["t"].is_string(), "{l}");
        kinds.push(v["kind"].as_str().unwrap().to_string());
        states.push(v["info"]["state"].as_str().unwrap_or("").to_string());
    }
    assert_eq!(kinds, ["state", "state", "check-in"]);
    assert_eq!(&states[..2], ["working", "idle"]);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A git repository with one commit on main, and that commit.
fn main_repo() -> (tempfile::TempDir, PathBuf, String) {
    let d = tempfile::tempdir().unwrap();
    let main = std::fs::canonicalize(d.path()).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(
        &main,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    );
    let head = git(&main, &["rev-parse", "HEAD"]);
    (d, main, head)
}

// workspace_test.go:36: a session asking for its own worktree gets one
// before its agent starts, from the main checkout, and records it.
#[test]
fn own_worktree_recorded_at_start() {
    let _home = Home::new();
    let (_d, main, head) = main_repo();
    let m = main.to_string_lossy().into_owned();
    let h = host(Config {
        id: "abc123".into(),
        cwd: m.clone(),
        own_worktree: true,
        ..Config::default()
    });
    h.make_own_worktree();
    let want = main
        .join(".claude")
        .join("worktrees")
        .join("abc123")
        .to_string_lossy()
        .into_owned();
    let mut st = h.lock();
    let w = st.cfg.workspace.clone().unwrap();
    assert!(
        w.path == want
            && w.branch == "rush/abc123"
            && w.base == head
            && w.target == "main"
            && w.repo == m
            && w.own,
        "{w:?}"
    );
    assert!(
        st.cfg.cwd == want
            && st.info.cwd == want
            && st.info.workspace.as_ref() == Some(&w)
            && !st.cfg.own_worktree
    );
    assert!(crate::workspace::is_own_worktree(&w.path, &w.branch));
    let saved = crate::client::read_config("abc123").unwrap();
    assert_eq!(saved.workspace.unwrap().branch, "rush/abc123");
    let n = crate::workspace::workspace_note(Some(&w));
    assert!(n.contains(&want) && n.contains("rush/abc123"));
    // Its worktree removed, a later start goes back to the main checkout.
    git(&main, &["worktree", "remove", "--force", &want]);
    h.leave_gone_worktree(&mut st);
    assert!(st.cfg.cwd == m && st.cfg.workspace.is_none());
}

// workspace_test.go:74: one that went into a worktree by itself has it
// filled in from git, not as rush's own.
#[test]
fn workspace_found_from_git() {
    let _home = Home::new();
    let (_d, main, head) = main_repo();
    let other = main.join(".claude").join("worktrees").join("agent-x");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            &other.to_string_lossy(),
        ],
    );
    let o = other.to_string_lossy().into_owned();
    let h = host(Config {
        id: "def456".into(),
        cwd: o.clone(),
        ..Config::default()
    });
    h.note_workspace(&o);
    let st = h.lock();
    let w = st.info.workspace.clone().unwrap();
    assert!(
        !w.own && w.branch == "feature" && w.path == o && w.target == "main" && w.base == head,
        "{w:?}"
    );
    assert_eq!(
        crate::workspace::workspace_note(st.cfg.workspace.as_ref()),
        ""
    );
}

/// Reads c's lines until one decodes to something want takes.
fn next(c: &Client, mut want: impl FnMut(&Line) -> bool) -> Line {
    loop {
        let l = c
            .lines
            .recv_timeout(Duration::from_secs(10))
            .expect("a line from the host");
        let ev = lines::decode(&l)
            .unwrap_or_else(|e| panic!("decode {}: {e}", String::from_utf8_lossy(&l)));
        if want(&ev) {
            return ev;
        }
    }
}

fn in_state(s: &'static str) -> impl FnMut(&Line) -> bool {
    move |l| matches!(l, Line::Info(i) if i.state == s)
}

// agent_test.go:76: a host runs any agent through its adapter: the
// approval it asks, answered by a client, the turn's end, and the next
// message to the same agent.
#[test]
fn host_runs_any_agent() {
    let home = Home::new();
    let hp = home.path();
    let id = "fake1";
    std::fs::create_dir_all(dir(id)).unwrap();
    let cfg = Config {
        id: id.into(),
        session_id: "fake-0000".into(),
        kind: "fake".into(),
        cwd: hp.to_string_lossy().into_owned(),
        account: Profile {
            kind: Kind::from("fake"),
            name: "fake".into(),
            dir: hp.to_string_lossy().into_owned(),
        },
        prompt: "hi".into(),
        ..Config::default()
    };
    std::fs::write(dir(id).join("config.json"), cfg.to_json_indent()).unwrap();
    let hp2 = hp.clone();
    let server = std::thread::spawn(move || {
        Home::enter(&hp2);
        run_with(registry(), Hooks::quiet(), id)
    });
    // The socket is there once the host listens: its folder says when.
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[dir(id)]);
    let c = loop {
        if let Ok(c) = Client::dial(id) {
            break c;
        }
        assert!(
            w.wait(None, Some(Duration::from_secs(10))),
            "the host never listened"
        );
    };
    let Line::Event(Event::Approval(ap)) =
        next(&c, |l| matches!(l, Line::Event(Event::Approval(_))))
    else {
        unreachable!()
    };
    assert_eq!(ap.call.input.command, "echo hi");
    let Line::Info(i) = next(&c, in_state("blocked")) else {
        unreachable!()
    };
    assert!(i.kind == "fake" && i.needs.contains("echo hi"), "{i:?}");
    c.allow(&ap.id, None, false).unwrap();
    next(&c, |l| matches!(l, Line::Answered(id) if id == "ap1"));
    let Line::Event(Event::Message(m)) = next(
        &c,
        |l| matches!(l, Line::Event(Event::Message(m)) if m.role == "user"),
    ) else {
        unreachable!()
    };
    assert_eq!(m.parts[0].output.as_ref().unwrap().text, "answered yes");
    let Line::Info(i) = next(&c, in_state("idle")) else {
        unreachable!()
    };
    assert!(
        i.cost_usd == 0.02 && i.session_id == "fake-session",
        "{i:?}"
    );

    // A second client is replayed the conversation without streamed deltas.
    let c2 = Client::dial(id).unwrap();
    let mut replay = Vec::new();
    loop {
        let l = c2.lines.recv_timeout(Duration::from_secs(10)).unwrap();
        let s = String::from_utf8(l).unwrap();
        let end = s.contains(r#""type":"agtop_info""#);
        replay.push(s);
        if end {
            break;
        }
    }
    drop(c2);
    let joined = replay.join("\n");
    assert!(
        !joined.contains(r#""t":"delta""#)
            && joined.contains(r#""agtop_sent":true"#)
            && joined.contains("answered yes"),
        "{joined}"
    );

    // The next message goes to the same agent: an idle one is never stopped.
    c.send("again").unwrap();
    next(
        &c,
        |l| matches!(l, Line::Event(Event::Approval(a)) if a.call.input.command == "echo again"),
    );
    let starts = std::fs::read_to_string(hp.join("starts.log")).unwrap();
    assert_eq!(starts, "start\n");
    // An op the host can't do is answered with why.
    c.allow("nope", None, false).unwrap();
    next(
        &c,
        |l| matches!(l, Line::Error(e) if e == "no pending request nope"),
    );
    c.stop().unwrap();
    server.join().unwrap().unwrap();
    let info = crate::client::read_info_file(id).unwrap();
    assert_eq!(info.state, "stopped");
    assert!(!sock_path(id).exists(), "the socket outlived the host");
    let ev = std::fs::read_to_string(crate::paths::events_path(id)).unwrap_or_default();
    crate::events::flush();
    let ev = if ev.is_empty() {
        std::fs::read_to_string(crate::paths::events_path(id)).unwrap()
    } else {
        ev
    };
    assert!(ev.contains("host-started"), "{ev}");
}

// hello_test.go:17: a client that says nothing (an older rush) has its
// first op carried out, and gets lines as they are.
#[test]
fn a_client_without_hello_is_served() {
    let home = Home::new();
    let hp = home.path();
    let id = "old1";
    std::fs::create_dir_all(dir(id)).unwrap();
    let cfg = Config {
        id: id.into(),
        kind: "fake".into(),
        cwd: hp.to_string_lossy().into_owned(),
        account: Profile {
            kind: Kind::from("fake"),
            name: "fake".into(),
            dir: hp.to_string_lossy().into_owned(),
        },
        ..Config::default()
    };
    std::fs::write(dir(id).join("config.json"), cfg.to_json_indent()).unwrap();
    let hp2 = hp.clone();
    let server = std::thread::spawn(move || {
        Home::enter(&hp2);
        run_with(registry(), Hooks::quiet(), id)
    });
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[dir(id)]);
    let c = loop {
        if let Ok(c) = Client::dial_raw(id) {
            break c;
        }
        assert!(
            w.wait(None, Some(Duration::from_secs(10))),
            "the host never listened"
        );
    };
    c.send("hi").unwrap(); // before any hello wait is over
    next(
        &c,
        |l| matches!(l, Line::Event(Event::Approval(a)) if a.call.input.command == "echo hi"),
    );
    c.stop().unwrap();
    server.join().unwrap().unwrap();
}

#[test]
fn needs_say_what_a_call_works_on() {
    let c = tool::Call {
        name: "Bash".into(),
        input: tool::Input {
            command: "go test\n./...".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(needs_call(&c), "Bash go test");
    let c = tool::Call {
        name: "Ask".into(),
        raw: Some(RawJson::new(r#"{"questions":[{"question":"Which?"}]}"#).unwrap()),
        ..Default::default()
    };
    assert_eq!(needs_call(&c), "Ask Which?");
    assert_eq!(first_line(&"é".repeat(201)), "é".repeat(200) + "…");
    assert_eq!(first_line(&"é".repeat(200)), "é".repeat(200));
    assert!(retry_prompt("API Error: 529").starts_with(RETRY_PROMPT_START));
    let _ = Has::Yes;
}
