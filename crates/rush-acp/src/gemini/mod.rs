//! Google's Gemini CLI as a rush adapter: gemini over ACP, in ~/.gemini.
//! Signed in with Google it runs on Code Assist, free or paid, whose daily
//! allowance rush reads as Gemini itself does; on an API key it's paid by
//! use, with no allowance to read. It's kept for saved sessions: new ones
//! choose Antigravity. Port of internal/adapters/gemini/gemini.go:1-38,
//! choices.go:1-58 and login.go:1-45.

mod history;
mod quota;

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    self, Adapter, Authenticator, Choice, Choices, Chooser, Conn, Driver, Feature, Features,
    KeyChecker, Kind, Level, Made, ModelLister, Once, OnceReader, Profile, Programmer, Published,
    Publisher, Replaced, Sink, Spawnable, StartOptions, Support,
};
use serde::Deserialize;

use crate::agent::{Agent, installed};
use crate::rpc::Error;
use crate::util::{env, home_dir, path_str};

/// Gemini's kind.
pub const KIND: &str = "gemini";

/// Where Gemini keeps its Google sign-in, in its home.
const CREDS_FILE: &str = "oauth_creds.json";

/// Code Assist, where Gemini signed in with Google reads its own limits.
pub const ENDPOINT: &str = "https://cloudcode-pa.googleapis.com/v1internal";

/// Gemini CLI: what ACP gives it, and its allowance.
pub struct Gemini {
    pub agent: Agent,
    /// Code Assist's address (a test's own server in tests).
    endpoint: String,
    /// What was read of each saved session, kept while its file is the
    /// same: the list reads them again and again.
    cache: Mutex<HashMap<String, history::Cached>>,
}

impl Default for Gemini {
    fn default() -> Self {
        Gemini::new()
    }
}

impl Gemini {
    pub fn new() -> Gemini {
        let flags: HashMap<String, String> = [
            ("-p", "prompt"),
            ("--prompt", "prompt"),
            ("-m", "model"),
            ("--model", "model"),
            ("-o", "=text"),
            ("--output-format", "=text"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Gemini {
            agent: Agent {
                id: Kind::from(KIND),
                title: "Gemini CLI".into(),
                company: "Google".into(),
                command: "gemini".into(),
                args: vec!["--experimental-acp".into()],
                home: ".gemini".into(),
                publ: Published {
                    npm: "@google/gemini-cli".into(),
                    ..Published::default()
                },
                creds: vec![CREDS_FILE.into()],
                keys: vec!["GEMINI_API_KEY".into(), "GOOGLE_API_KEY".into()],
                sign_in:
                    "run gemini once and pick Sign in with Google (free), or set GEMINI_API_KEY"
                        .into(),
                more: HashMap::from([(
                    Feature::Quota,
                    Support::YES.with("legacy Code Assist quota"),
                )]),
                once: r#"gemini -p "<task>""#.into(),
                model: "-m".into(),
                flags,
                ..Agent::default()
            },
            endpoint: ENDPOINT.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// A Gemini that asks Code Assist at endpoint: for tests.
    pub fn at(endpoint: &str) -> Gemini {
        Gemini {
            endpoint: endpoint.into(),
            ..Gemini::new()
        }
    }

    fn dir(p: &Profile) -> String {
        if p.dir.is_empty() {
            gemini_home()
        } else {
            p.dir.clone()
        }
    }
}

/// GEMINI_CLI_HOME's .gemini, else the home folder's.
fn gemini_home() -> String {
    let mut h = env("GEMINI_CLI_HOME");
    if h.is_empty() {
        h = home_dir();
    }
    path_str(&Path::new(&h).join(".gemini"))
}

/// The folder a profile's .gemini is in, for GEMINI_CLI_HOME; None for a
/// profile that isn't a .gemini.
fn cli_home(dir: &str) -> Option<String> {
    let p = Path::new(dir);
    (!dir.is_empty() && p.file_name().is_some_and(|n| n == ".gemini"))
        .then(|| path_str(p.parent().unwrap_or(Path::new(""))))
}

impl Adapter for Gemini {
    fn kind(&self) -> Kind {
        self.agent.id.clone()
    }
    fn name(&self) -> String {
        self.agent.title.clone()
    }
    fn features(&self) -> Features {
        let y = |n: &str| Support::YES.with(n);
        let mut out = self.agent.all_features();
        out.insert(
            Feature::SignIn,
            y("native terminal login; no account swapping"),
        );
        out.insert(Feature::History, y("saved local conversations"));
        out
    }
    fn level(&self) -> Level {
        self.agent.tried
    }
    fn profiles(&self) -> Vec<Profile> {
        if !installed(&self.agent.id) {
            return Vec::new();
        }
        vec![Profile {
            kind: self.agent.id.clone(),
            name: self.agent.title.clone(),
            dir: gemini_home(),
        }]
    }
    rush_core::has!(
        driver: Driver,
        once_reader: OnceReader,
        spawnable: Spawnable,
        key_checker: KeyChecker,
        programmer: Programmer,
        made: Made,
        publisher: Publisher,
        quota_source: QuotaSource,
        replaced: Replaced,
        chooser: Chooser,
        model_lister: ModelLister,
        authenticator: Authenticator,
        discoverer: Discoverer,
        history_reader: HistoryReader,
    );
}

impl Driver for Gemini {
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        if let Some(h) = cli_home(&o.profile.dir) {
            o.env.push(format!("GEMINI_CLI_HOME={h}"));
        }
        Ok(Box::new(self.agent.run(o, sink, false)?))
    }
}

impl OnceReader for Gemini {
    fn read_once(&self, args: &[String]) -> Option<Once> {
        self.agent.once_of(args)
    }
}

impl Spawnable for Gemini {
    fn spawn_command(&self) -> (String, String) {
        (self.agent.once.clone(), self.agent.model.clone())
    }
}

impl Programmer for Gemini {
    fn program(&self) -> (String, Vec<String>) {
        self.agent.program()
    }
}

impl Made for Gemini {
    fn maker(&self) -> String {
        self.agent.company.clone()
    }
}

impl Publisher for Gemini {
    fn published(&self) -> Published {
        self.agent.publ.clone()
    }
}

impl Replaced for Gemini {
    fn replacement(&self) -> Kind {
        Kind::from("antigravity")
    }
}

impl Chooser for Gemini {
    /// Gemini resolves these aliases by what the signed-in account may use.
    fn choices(&self) -> Choices {
        let c = |id: &str, note: &str| Choice {
            id: id.into(),
            note: note.into(),
            context: 0,
        };
        Choices {
            modes: vec![
                c("default", "ask before tools"),
                c("autoEdit", "approve edits automatically"),
                c("yolo", "approve all tools"),
                c("plan", "read-only planning, when enabled"),
            ],
            ..Choices::default()
        }
    }
}

impl ModelLister for Gemini {
    fn list_models(&self, p: &Profile) -> Vec<Choice> {
        let c = |id: &str, note: &str| Choice {
            id: id.into(),
            note: note.into(),
            context: 0,
        };
        let mut out = vec![
            c("auto", "let Gemini select the model"),
            c("pro", "account's Pro model"),
            c("flash", "account's fast model"),
        ];
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct M {
            name: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Settings {
            model: M,
        }
        let Ok(b) = std::fs::read(Path::new(&Gemini::dir(p)).join("settings.json")) else {
            return out;
        };
        let Ok(s) = serde_json::from_slice::<Settings>(&b) else {
            return out;
        };
        let id = s.model.name;
        if !id.is_empty() && !["auto", "pro", "flash"].contains(&id.as_str()) {
            out.push(c(&id, "configured model"));
        }
        out
    }
}

impl Authenticator for Gemini {
    /// Gemini's own sign-in picker. Leaving Gemini after signing in comes
    /// back to rush; no model request is made by this command.
    fn sign_in_command(&self, p: &Profile) -> adapter::Result<Command> {
        let bin = adapter::global().path(KIND);
        if bin.is_empty() {
            return Err(Box::new(Error::other("Gemini CLI is not installed")));
        }
        let mut cmd = Command::new(bin);
        if let Some(h) = cli_home(&p.dir) {
            cmd.env("GEMINI_CLI_HOME", h);
        }
        Ok(cmd)
    }
}

impl KeyChecker for Gemini {
    /// Checks credentials without starting Gemini or asking anyone.
    fn check_key(&self, p: &Profile) -> adapter::Result<()> {
        if !env("GEMINI_API_KEY").is_empty() || !env("GOOGLE_API_KEY").is_empty() {
            return Ok(());
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Token {
            access_token: String,
            refresh_token: String,
        }
        if let Ok(b) = std::fs::read(Path::new(&Gemini::dir(p)).join(CREDS_FILE))
            && let Ok(t) = serde_json::from_slice::<Token>(&b)
            && (!t.access_token.is_empty() || !t.refresh_token.is_empty())
        {
            return Ok(());
        }
        Err(Box::new(Error::other(
            "Gemini isn't signed in: open Sign in to use Google, or set GEMINI_API_KEY",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // history_test.go: TestCheckKeyNeedsUsableCredentialRecord
    #[test]
    fn check_key_needs_a_usable_credential() {
        if !env("GEMINI_API_KEY").is_empty() || !env("GOOGLE_API_KEY").is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let p = Profile {
            dir: path_str(d.path()),
            ..Profile::default()
        };
        let a = Gemini::new();
        std::fs::write(d.path().join(CREDS_FILE), "{}").unwrap();
        assert!(
            a.check_key(&p).is_err(),
            "empty credentials read as signed in"
        );
        std::fs::write(
            d.path().join(CREDS_FILE),
            r#"{"refresh_token":"test-refresh"}"#,
        )
        .unwrap();
        a.check_key(&p).unwrap();
    }

    #[test]
    fn its_home_for_gemini() {
        assert_eq!(cli_home("/u/me/.gemini").as_deref(), Some("/u/me"));
        assert_eq!(cli_home("/u/me/other"), None);
        assert_eq!(Gemini::new().replacement().as_str(), "antigravity");
    }
}
