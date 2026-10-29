//! Pi, Mario Zechner's coding agent, as a rush adapter. It drives `pi
//! --mode rpc`, Pi's own protocol of JSON lines over stdin and stdout
//! (`rpc`, `conn`, `handle`), and reads Pi's session files back for past
//! sessions (`discover`, `history`, `messages`). Port of
//! internal/adapters/pi/pi.go:1-117 and the files beside it.

mod account;
mod conn;
mod discover;
mod extras;
mod handle;
mod history;
mod json;
mod messages;
mod rpc;

use std::collections::HashMap;
use std::process::Command as Cmd;
use std::sync::Arc;

use rush_core::adapter::{
    self, AccountSummary, AccountSummaryReader, Authenticator, Command, Commander, Conn,
    Discoverer, Driver, Feature, Features, Follow, HistoryFollower, HistoryReader, Kind, Level,
    Memory, MemoryReader, Orphans, Profile, Programmer, Published, Publisher, Registry, Session,
    SettingsFile, SettingsFiler, Sink, StartOptions, Support,
};
use rush_core::event::Event;
use rush_core::time::Ms;

pub use conn::PiConn;
pub use discover::{home, id_of_file};
pub use rpc::CallError;

/// Pi's kind.
pub const KIND: &str = "pi";

/// Pi.
#[derive(Default)]
pub struct Pi {
    /// The registry asked whether pi is installed and where: rush's own
    /// unless a test gives one.
    reg: Option<Registry>,
}

impl Pi {
    pub fn new() -> Pi {
        Pi::default()
    }

    /// Pi asking reg, not rush's own registry, what's installed: for tests.
    pub fn with_registry(reg: Registry) -> Pi {
        Pi { reg: Some(reg) }
    }

    fn reg(&self) -> &Registry {
        self.reg.as_ref().unwrap_or_else(|| adapter::global())
    }
}

/// What RPC mode gives rush. Pi runs every tool without asking and keeps
/// no modes, plans, subagents or MCP by design; those come as extensions,
/// of which rush knows the subagent one's tool.
fn features() -> Features {
    use Feature::*;
    let y = Support::YES;
    let n = Support::NO;
    HashMap::from([
        (Run, y.clone()),
        (Prompt, y.clone()),
        (Resume, y.clone()),
        (
            Fork,
            y.clone().with("the whole session, not from a message"),
        ),
        (Interrupt, y.clone()),
        (Model, y.clone().with("any model Pi has, at once")),
        (Effort, y.clone().with("its thinking level, from the start")),
        (Images, y.clone()),
        (Compact, y.clone()),
        (HandoffIn, y.clone()),
        (
            Questions,
            y.clone().with("an extension's select and confirm"),
        ),
        (Context, y.clone().with("how full it is, not what fills it")),
        (
            Live,
            y.clone()
                .with("sessions written to in the last two minutes"),
        ),
        (History, y.clone()),
        (
            Modes,
            n.clone().with("Pi asks before nothing: every tool runs"),
        ),
        (Plan, n.clone().with("only as an extension")),
        (
            Subagents,
            y.clone()
                .with("the subagent extension's, when it's installed"),
        ),
        (Mcp, n.clone().with("only as an extension")),
        (Rewind, n.clone().with("its /tree is its own screen's")),
        (
            Commands,
            y.clone()
                .with("prompt templates and skills; not packages' or extensions'"),
        ),
        (Pricing, n.clone().with("Pi prices each turn itself")),
        (
            SignIn,
            y.clone().with("its own /login; no account swapping"),
        ),
        (Quota, n.clone().with("each provider keeps its own limits")),
        (Memory, y.clone().with("its AGENTS.md files")),
        (Settings, y.with("settings.json, yours and the project's")),
    ])
}

impl adapter::Adapter for Pi {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "Pi".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Tested
    }

    /// Pi's config folder, when it exists and pi is installed: a folder
    /// left behind by an uninstalled pi lists nothing.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg().installed(KIND) {
            return Vec::new();
        }
        let dir = home();
        if dir.is_empty() || !std::fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(KIND),
            name: "Pi".into(),
            dir,
        }]
    }

    rush_core::has!(
        discoverer: Discoverer,
        driver: Driver,
        orphans: Orphans,
        history_reader: HistoryReader,
        history_follower: HistoryFollower,
        programmer: Programmer,
        publisher: Publisher,
        account_summary_reader: AccountSummaryReader,
        authenticator: Authenticator,
        commander: Commander,
        settings_filer: SettingsFiler,
        memory_reader: MemoryReader,
    );
}

impl Programmer for Pi {
    /// pi, as npm, bun or pnpm put it.
    fn program(&self) -> (String, Vec<String>) {
        (
            "pi".into(),
            vec!["Library/pnpm".into(), ".local/share/pnpm".into()],
        )
    }
}

impl Publisher for Pi {
    fn published(&self) -> Published {
        Published {
            npm: "@earendil-works/pi-coding-agent".into(),
            ..Published::default()
        }
    }
}

impl Discoverer for Pi {
    fn live(&self, p: &Profile) -> Vec<Session> {
        discover::live(p)
    }
    fn past(&self, p: &Profile) -> Vec<Session> {
        discover::past(p)
    }
}

impl Driver for Pi {
    /// Runs a session in its own pi.
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        let path = if o.binary.is_empty() && adapter::find("pi", &[]).is_none() {
            self.reg().path(KIND)
        } else {
            String::new()
        };
        Ok(Box::new(conn::start(&o, &path, sink)?))
    }
}

impl Orphans for Pi {
    /// Whether a command line is a pi in RPC mode on session session_id: one
    /// rush started, resumed or forked it with.
    fn runs_session(&self, args: &[String], session_id: &str) -> bool {
        if session_id.is_empty() || !args.iter().any(|a| a == "rpc") {
            return false;
        }
        args.windows(2).any(|w| {
            matches!(w[0].as_str(), "--session" | "--session-id" | "--fork")
                && (w[1] == session_id || id_of_file(&w[1]) == session_id)
        })
    }
}

impl HistoryReader for Pi {
    /// The session file at s.transcript, or else the one of session s.id in
    /// s.profile, as the events a live session sends: the branch of its
    /// tree it's on now. When before isn't 0, it stops at the first entry
    /// written at or after it.
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> adapter::Result<()> {
        history::read(&history::path_of(s)?, before, most, each)
    }
}

impl HistoryFollower for Pi {
    fn follow_history(&self, s: &Session) -> adapter::Result<Box<dyn Follow>> {
        Ok(history::follow(history::path_of(s)?))
    }
}

impl AccountSummaryReader for Pi {
    fn account_summary(&self, p: &Profile) -> AccountSummary {
        account::summary(p)
    }
}

impl Authenticator for Pi {
    fn sign_in_command(&self, p: &Profile) -> adapter::Result<Cmd> {
        account::sign_in_command(&self.reg().path(KIND), p)
    }
}

impl Commander for Pi {
    fn commands(&self, p: &Profile, cwd: &str) -> Vec<Command> {
        extras::commands(p, cwd)
    }
}

impl SettingsFiler for Pi {
    fn settings_files(&self, p: &Profile, cwd: &str) -> Vec<SettingsFile> {
        extras::settings_files(p, cwd)
    }
}

impl MemoryReader for Pi {
    fn memory(&self, p: &Profile, cwd: &str, _transcript: &str) -> Memory {
        let root = if cwd.is_empty() {
            String::new()
        } else {
            rush_sys::actions::repo_root(cwd)
        };
        extras::memory(p, cwd, &root)
    }
    fn forget_file(&self, path: &str) -> adapter::Result<()> {
        Ok(std::fs::remove_file(path)?)
    }
}

#[cfg(test)]
mod tests;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
