//! A fake host, for tests here and in the crates that drive a host (the
//! UI's): an agent that runs a command for each message, asks to run it,
//! then ends its turn once answered (Go's agent_test.go fakeAgent), and a
//! host for it run in this process rather than as `rush host run`.
//! Built for this crate's tests, and for others' with the `fake` feature.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rush_core::adapter::{
    self, Adapter, Conn, Driver, Feature, Features, Input, Kind, Level, Profile, Registry, Sink,
    StartOptions, Support,
};
use rush_core::event::{self, ApprovalOption, Event, OptionKind};
use rush_core::tool;

use crate::client::{Client, Result};
use crate::paths::dir;
use crate::server::{Hooks, run_with};
use crate::wire::Config;

/// An agent other than Claude Code: each message runs a command it asks to
/// run, then ends the turn. Its starts are logged to starts.log beside its
/// profile, with the session it resumed (agent_test.go's fakeAgent).
#[derive(Clone)]
pub struct Fake {
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
        // Never beside the process's own folder: a start with no profile
        // folder logs nothing.
        if !o.profile.dir.is_empty() {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(Path::new(&o.profile.dir).join("starts.log"))?;
            writeln!(f, "{}", format!("start {resumed}").trim())?;
        }
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

/// A registry with the fake agent, as kind fake, and as claude with what
/// Claude Code can do mid-turn.
pub fn registry() -> Registry {
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

/// Starts cfg's host on a thread of this process, on reg's agents, as
/// [`crate::spawn`] starts `rush host run`: its config written, returning
/// once it takes connections. The thread keeps this thread's rush folder.
pub fn spawn(reg: Registry, mut cfg: Config) -> Result<Config> {
    cfg.fill_ids();
    let d = dir(&cfg.id);
    std::fs::create_dir_all(&d)?;
    std::fs::write(d.join("config.json"), cfg.to_json_indent())?;
    let home = rush_state::dir();
    let cache = rush_state::cache_dir();
    let id = cfg.id.clone();
    std::thread::Builder::new()
        .name("fake-host".into())
        .spawn(move || {
            rush_state::set_thread_dir(Some(home));
            rush_state::set_thread_cache_dir(Some(cache));
            let _ = run_with(reg, Hooks::quiet(), &id);
        })?;
    // Up once its socket answers: its folder changes then.
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[&d]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(c) = Client::dial(&cfg.id) {
            c.close();
            return Ok(cfg);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!("the fake host for {} never listened", cfg.id).into());
        }
        w.wait(None, Some((deadline - now).min(Duration::from_millis(50))));
    }
}
