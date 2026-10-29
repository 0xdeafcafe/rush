//! Antigravity: Google's agy CLI as a rush adapter, run in its persistent
//! NDJSON mode (`conn`, `stream`), with the history rush keeps of each
//! conversation (`history`) and the models it last listed (`catalog`).
//! Port of internal/adapters/antigravity/antigravity.go:1-121 and the
//! files beside it.

mod catalog;
mod conn;
mod history;
mod once;
mod stream;

use std::collections::HashMap;
use std::path::Path;
use std::process::Command as Cmd;
use std::sync::Arc;

use rush_core::adapter::{
    self, AccountSummary, AccountSummaryReader, Authenticator, Choice, Choices, Chooser, Conn,
    Driver, Feature, Features, Follow, HistoryFollower, HistoryReader, KeyChecker, Kind, Level,
    Made, ModelLister, Once, OnceReader, Profile, Programmer, Published, Publisher, Registry,
    RunnerSnapshot, Session, Sink, Spawnable, StartOptions, Support, TailReader, TranscriptLocator,
};
use rush_core::event::Event;
use rush_core::time::Ms;

pub use conn::AgyConn;

/// Antigravity's kind.
pub const KIND: &str = "antigravity";

/// An error in words.
#[derive(Debug)]
pub(crate) struct Says(pub String);

impl std::fmt::Display for Says {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Says {}

pub(crate) fn says(s: impl Into<String>) -> adapter::Error {
    Box::new(Says(s.into()))
}

/// Antigravity.
#[derive(Default)]
pub struct Antigravity {
    /// The registry asked whether agy is installed and where: rush's own
    /// unless a test gives one.
    reg: Option<Registry>,
}

impl Antigravity {
    pub fn new() -> Antigravity {
        Antigravity::default()
    }

    /// Antigravity asking reg, not rush's own registry, what's installed.
    pub fn with_registry(reg: Registry) -> Antigravity {
        Antigravity { reg: Some(reg) }
    }

    fn reg(&self) -> &Registry {
        self.reg.as_ref().unwrap_or_else(|| adapter::global())
    }

    fn bin(&self) -> String {
        self.reg().path(KIND)
    }
}

fn features() -> Features {
    use Feature::*;
    let (y, n) = (Support::YES, Support::NO);
    HashMap::from([
        (Run, y.clone()),
        (Resume, y.clone()),
        (Interrupt, y.clone()),
        (
            SignIn,
            y.clone().with("Antigravity's native Google sign-in"),
        ),
        (
            Model,
            n.clone()
                .with("choose before starting; streaming sessions cannot switch models"),
        ),
        (Effort, y.clone().with("selected at startup")),
        (
            Modes,
            y.clone()
                .with("selected at startup; headless tools follow native policy"),
        ),
        (Plan, y.clone().with("selected at startup")),
        (
            Images,
            n.clone().with("agy's streaming input accepts text only"),
        ),
        (
            Questions,
            n.clone()
                .with("native headless policy; no interactive approval protocol"),
        ),
        (
            Quota,
            n.with("use /usage in Antigravity; Gemini Code Assist limits do not apply"),
        ),
        (
            History,
            y.with("conversations started in Rush; native imports are not available"),
        ),
    ])
}

impl adapter::Adapter for Antigravity {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "Antigravity CLI".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg().installed(KIND) {
            return Vec::new();
        }
        let home = std::env::var("HOME").unwrap_or_default();
        vec![Profile {
            kind: Kind::from(KIND),
            name: "Antigravity".into(),
            dir: Path::new(&home)
                .join(".gemini")
                .join("antigravity-cli")
                .to_string_lossy()
                .into_owned(),
        }]
    }

    rush_core::has!(
        driver: Driver,
        history_reader: HistoryReader,
        history_follower: HistoryFollower,
        tail_reader: TailReader,
        transcript_locator: TranscriptLocator,
        programmer: Programmer,
        publisher: Publisher,
        made: Made,
        chooser: Chooser,
        spawnable: Spawnable,
        once_reader: OnceReader,
        authenticator: Authenticator,
        key_checker: KeyChecker,
        account_summary_reader: AccountSummaryReader,
        model_lister: ModelLister,
        runner_snapshot: RunnerSnapshot,
    );
}

impl Made for Antigravity {
    fn maker(&self) -> String {
        "Google".into()
    }
}

impl Programmer for Antigravity {
    fn program(&self) -> (String, Vec<String>) {
        ("agy".into(), vec![".local/bin".into()])
    }
}

impl Publisher for Antigravity {
    fn published(&self) -> Published {
        Published {
            update: vec!["update".into()],
            ..Published::default()
        }
    }
}

impl Chooser for Antigravity {
    fn choices(&self) -> Choices {
        let c = |id: &str, note: &str| Choice {
            id: id.into(),
            note: note.into(),
            ..Choice::default()
        };
        Choices {
            efforts: vec![c("low", ""), c("medium", ""), c("high", ""), c("max", "")],
            modes: vec![
                c("accept-edits", "native edit policy"),
                c("plan", "planning mode"),
                c(
                    "always-proceed",
                    "approve every tool; skips permission prompts",
                ),
            ],
            ..Choices::default()
        }
    }
}

impl Spawnable for Antigravity {
    fn spawn_command(&self) -> (String, String) {
        (r#"agy -p "<task>""#.into(), "--model".into())
    }
}

impl OnceReader for Antigravity {
    fn read_once(&self, args: &[String]) -> Option<Once> {
        let flags = HashMap::from([
            ("-p", "prompt"),
            ("--print", "prompt"),
            ("--prompt", "prompt"),
            ("--model", "model"),
            ("--output-format", "=text"),
            ("--dangerously-skip-permissions", "mode=always-proceed"),
        ]);
        once::OnceSpec {
            once: r#"agy -p "<task>""#,
            flags: &flags,
        }
        .read(args)
    }
}

impl Authenticator for Antigravity {
    fn sign_in_command(&self, _: &Profile) -> adapter::Result<Cmd> {
        let bin = self.bin();
        if bin.is_empty() {
            return Err(says(
                "Antigravity CLI is not installed: install agy from antigravity.google",
            ));
        }
        Ok(Cmd::new(bin))
    }
}

impl KeyChecker for Antigravity {
    fn check_key(&self, _: &Profile) -> adapter::Result<()> {
        catalog::models_output(&self.bin())
            .map(|_| ())
            .map_err(says)
    }
}

impl AccountSummaryReader for Antigravity {
    fn account_summary(&self, p: &Profile) -> AccountSummary {
        if let Err(e) = self.check_key(p) {
            return AccountSummary {
                status: e.to_string(),
                ..AccountSummary::default()
            };
        }
        AccountSummary {
            signed_in: true,
            status: "Antigravity account available".into(),
            method: "native sign-in".into(),
            note: "Identity and plan are managed by Antigravity".into(),
            ..AccountSummary::default()
        }
    }
}

impl ModelLister for Antigravity {
    fn list_models(&self, _: &Profile) -> Vec<Choice> {
        match catalog::models_output(&self.bin()) {
            Ok(b) => catalog::parse_models(&String::from_utf8_lossy(&b)),
            Err(_) => Vec::new(),
        }
    }
}

impl RunnerSnapshot for Antigravity {
    fn runner_snapshot(&self) -> (Vec<Choice>, bool) {
        catalog::runner_snapshot()
    }
}

impl Driver for Antigravity {
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        let bin = if o.binary.is_empty() {
            self.bin()
        } else {
            o.binary.clone()
        };
        Ok(Box::new(conn::start(&bin, &o, rush_state::dir(), sink)?))
    }
}

/// Where session s's history is, or why there's none.
fn path_of(s: &Session) -> adapter::Result<std::path::PathBuf> {
    history::history_path(&rush_state::dir(), &s.id)
        .ok_or_else(|| says("invalid Antigravity conversation ID"))
}

impl HistoryReader for Antigravity {
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> adapter::Result<()> {
        history::read(&path_of(s)?, 0, before, most, each)
    }
}

impl TailReader for Antigravity {
    fn history_tail(&self, s: &Session, most: i64) -> adapter::Result<(Vec<Event>, bool)> {
        let path = path_of(s)?;
        let size = std::fs::metadata(&path)?.len() as i64;
        let offset = (size - most.max(1)).max(0);
        let mut out = Vec::new();
        history::read(&path, offset as u64, 0, adapter::SLICE, &mut |evs| {
            out.extend(evs);
            true
        })?;
        Ok((out, offset > 0))
    }
}

impl HistoryFollower for Antigravity {
    fn follow_history(&self, s: &Session) -> adapter::Result<Box<dyn Follow>> {
        Ok(Box::new(history::Follower {
            path: path_of(s)?,
            off: 0,
        }))
    }
}

impl TranscriptLocator for Antigravity {
    fn transcript_path(&self, _: &Profile, _: &str, sid: &str) -> String {
        history::history_path(&rush_state::dir(), sid)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
