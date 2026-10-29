//! Mistral Vibe as a rush adapter: vibe-acp, beside vibe, over ACP, in
//! VIBE_HOME (~/.vibe). It runs on a Mistral API key Vibe keeps itself,
//! which rush looks for where Vibe does. It stays out of sight until vibe
//! is installed. Port of internal/adapters/vibe/vibe.go:1-112 and
//! login.go:1-25.

mod account;
mod attribution;
mod children;
mod history;

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rush_core::adapter::{
    self, Adapter, Authenticator, Conn, Driver, Feature, Features, KeyChecker, Kind, Level, Made,
    Once, OnceReader, Profile, Programmer, Published, Publisher, Sink, Spawnable, StartOptions,
    Support,
};

use crate::agent::{Agent, installed};
use crate::rpc::Error;
use crate::util::{env, home_dir, path_str};

/// Vibe's kind.
pub const KIND: &str = "vibe";

/// Where Vibe reads its Mistral key first.
const KEY_ENV: &str = "MISTRAL_API_KEY";

/// The keychain services Vibe keeps its key under, now and before.
const SERVICES: [&str; 2] = ["ai.mistral.vibe", "vibe"];

/// Vibe: what ACP gives it, and the key it runs on.
#[derive(Clone)]
pub struct Vibe {
    pub agent: Agent,
    /// Whether Vibe keeps its key in the keychain (tests have their own).
    keychained: fn() -> bool,
    /// The key Vibe keeps in the keychain, empty when none.
    stored_key: fn() -> Vec<u8>,
}

impl Default for Vibe {
    fn default() -> Self {
        Vibe::new()
    }
}

fn keychained() -> bool {
    SERVICES
        .iter()
        .any(|s| rush_state::vault::keychain::has(s, KEY_ENV))
}

fn stored_key() -> Vec<u8> {
    for s in SERVICES {
        if let Ok(b) = rush_state::vault::keychain::read(s, KEY_ENV)
            && !b.is_empty()
        {
            return b;
        }
    }
    Vec::new()
}

impl Vibe {
    pub fn new() -> Vibe {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let flags: HashMap<String, String> = [
            ("-p", "prompt"),
            ("--prompt", "prompt"),
            ("--workdir", "cwd"),
            ("--agent", "mode"),
            ("--auto-approve", "mode=auto-approve"),
            ("--yolo", "mode=auto-approve"),
            ("--output", "=text"),
            ("--trust", ""),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Vibe {
            agent: Agent {
                id: Kind::from(KIND),
                title: "Vibe".into(),
                company: "Mistral".into(),
                command: "vibe".into(),
                acp: "vibe-acp".into(),
                home: ".vibe".into(),
                publ: Published {
                    pypi: "mistral-vibe".into(),
                    ..Published::default()
                },
                // Vibe says which plan it's on (its whoami), never how much
                // is left; a limit shows only as the error it stops a turn
                // with.
                more: HashMap::from([(
                    Feature::Quota,
                    Support::NO.with("Vibe says its plan, not what's left of it"),
                )]),
                // vibe takes its model only from its config, which VIBE_*
                // sets.
                once: r#"vibe -p "<task>""#.into(),
                model: "VIBE_ACTIVE_MODEL=".into(),
                flags,
                args: s(&[]),
                ..Agent::default()
            },
            keychained,
            stored_key,
        }
    }

    /// A Vibe that looks in no keychain: for tests.
    #[cfg(test)]
    pub(crate) fn without_keychain() -> Vibe {
        Vibe {
            keychained: || false,
            stored_key: Vec::new,
            ..Vibe::new()
        }
    }

    fn dir(p: &Profile) -> String {
        if p.dir.is_empty() {
            home()
        } else {
            p.dir.clone()
        }
    }
}

/// Vibe's home: VIBE_HOME, else ~/.vibe.
pub(crate) fn home() -> String {
    let d = env("VIBE_HOME");
    if !d.is_empty() {
        return d;
    }
    let h = home_dir();
    if h.is_empty() {
        return String::new();
    }
    path_str(&Path::new(&h).join(".vibe"))
}

/// Whether a .env file sets the key.
fn in_dotenv(path: &Path) -> bool {
    dotenv_key(path).is_some_and(|k| !k.is_empty())
}

/// The key a .env file sets, as written (quotes taken off).
fn dotenv_key(path: &Path) -> Option<String> {
    let b = std::fs::read_to_string(path).ok()?;
    for l in b.split('\n') {
        let l = l.trim();
        let l = l.strip_prefix("export ").unwrap_or(l);
        if let Some(v) = l.strip_prefix(&format!("{KEY_ENV}=")) {
            return Some(v.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

impl Adapter for Vibe {
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
        out.insert(Feature::Effort, y("model-supported thinking levels"));
        out.insert(Feature::History, y("saved local conversations"));
        out
    }
    fn level(&self) -> Level {
        self.agent.tried
    }
    /// Vibe's home, when vibe is installed.
    fn profiles(&self) -> Vec<Profile> {
        if !installed(&self.agent.id) {
            return Vec::new();
        }
        vec![Profile {
            kind: self.agent.id.clone(),
            name: self.agent.title.clone(),
            dir: home(),
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
        account_summary_reader: AccountSummaryReader,
        attributor: Attributor,
        child_finder: ChildFinder,
        authenticator: Authenticator,
        chooser: Chooser,
        model_lister: ModelLister,
        discoverer: Discoverer,
        history_reader: HistoryReader,
        history_follower: HistoryFollower,
        tail_reader: TailReader,
    );
}

impl Driver for Vibe {
    /// Runs vibe-acp in the profile's home.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        if !o.profile.dir.is_empty() && o.profile.dir != home() {
            o.env.push(format!("VIBE_HOME={}", o.profile.dir));
        }
        if !o.model.is_empty() {
            o.env.push(format!("VIBE_ACTIVE_MODEL={}", o.model));
            // Vibe reads its startup model from its config, before ACP
            // opens.
            o.model.clear();
        }
        Ok(Box::new(self.agent.run(o, sink, true)?))
    }
}

impl OnceReader for Vibe {
    fn read_once(&self, args: &[String]) -> Option<Once> {
        self.agent.once_of(args)
    }
}

impl Spawnable for Vibe {
    fn spawn_command(&self) -> (String, String) {
        (self.agent.once.clone(), self.agent.model.clone())
    }
}

impl Programmer for Vibe {
    fn program(&self) -> (String, Vec<String>) {
        self.agent.program()
    }
}

impl Made for Vibe {
    fn maker(&self) -> String {
        self.agent.company.clone()
    }
}

impl Publisher for Vibe {
    fn published(&self) -> Published {
        self.agent.publ.clone()
    }
}

/// A Vibe with no key rush can find.
fn no_key() -> Error {
    Error::other(format!(
        "no Mistral API key: run vibe --setup once, or set {KEY_ENV}"
    ))
}

impl KeyChecker for Vibe {
    /// Finds the key where Vibe looks: the environment, its home's .env,
    /// then the keychain.
    fn check_key(&self, p: &Profile) -> adapter::Result<()> {
        let dir = Vibe::dir(p);
        if !env(KEY_ENV).is_empty()
            || in_dotenv(&Path::new(&dir).join(".env"))
            || (self.keychained)()
        {
            return Ok(());
        }
        Err(Box::new(no_key()))
    }
}

impl Authenticator for Vibe {
    /// Vibe's own onboarding, in your terminal. Vibe owns its API key and
    /// keychain; rush doesn't pretend to swap accounts for it.
    fn sign_in_command(&self, p: &Profile) -> adapter::Result<Command> {
        let bin = adapter::global().path(KIND);
        if bin.is_empty() {
            return Err(Box::new(Error::other("Vibe is not installed")));
        }
        let mut cmd = Command::new(bin);
        cmd.arg("--setup");
        if !p.dir.is_empty() {
            cmd.env("VIBE_HOME", &p.dir);
        }
        Ok(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // vibe_test.go: TestCheckKey (the key from the environment is the
    // process's own, so only the .env and keychain halves run here).
    #[test]
    fn check_key_finds_the_dotenv() {
        if !env(KEY_ENV).is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let p = Profile {
            kind: Kind::from(KIND),
            dir: path_str(d.path()),
            ..Profile::default()
        };
        let v = Vibe::without_keychain();
        let err = v.check_key(&p).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no Mistral API key: run vibe --setup once, or set MISTRAL_API_KEY"
        );
        std::fs::write(
            d.path().join(".env"),
            "OTHER=1\nexport MISTRAL_API_KEY='k'\n",
        )
        .unwrap();
        v.check_key(&p).unwrap();
        let kept = Vibe {
            keychained: || true,
            ..Vibe::without_keychain()
        };
        let d2 = tempfile::tempdir().unwrap();
        kept.check_key(&Profile {
            dir: path_str(d2.path()),
            ..Profile::default()
        })
        .unwrap();
    }

    #[test]
    fn starts_vibe_acp_beside_vibe() {
        let v = Vibe::new();
        assert_eq!(v.agent.acp, "vibe-acp");
        let o = v
            .read_once(&["-p".into(), "go".into(), "--yolo".into()])
            .unwrap();
        assert_eq!((o.prompt.as_str(), o.mode.as_str()), ("go", "auto-approve"));
    }
}
