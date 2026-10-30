//! Kimi: what ACP gives it, with Kimi's configured models, its saved
//! sessions (Kimi Code 2's wire journals, the older CLI's context logs),
//! its sign-in, its plan's limits and its subagents' own runs. Port of
//! internal/adapters/acp/kimi.go:1-210.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use md5::{Digest, Md5};
use rush_core::adapter::{
    self, Adapter, Authenticator, Choice, Choices, Chooser, Conn, Discoverer, Driver, Feature,
    Features, Follow, HistoryFollower, HistoryReader, KeyChecker, Kind, Level, LoginHinter, Made,
    ModelLister, Once, OnceReader, Profile, Programmer, Published, Publisher, Session, Sink,
    Spawnable, StartOptions, Support, TailReader,
};
use rush_core::event::Event;
use rush_core::time::Ms;
use serde::Deserialize;

use crate::agent::{Agent, installed, kimi_agent};
use crate::history::{Follower, MessageLines, messages_each, read_each, read_messages_tail};
use crate::kimi_account::{kimi_credential, read_kimi_config};
use crate::kimi_wire::{code_past, read_kimi_wire_file, wire_boxed, wire_lines};
use crate::rpc::Error;
use crate::util::{base, env, glob, home_dir, mod_ms, path_str};

/// Kimi: ACP, with Kimi's models and sessions.
#[derive(Clone)]
pub struct Kimi {
    pub agent: Agent,
}

impl Default for Kimi {
    fn default() -> Self {
        Kimi::new()
    }
}

impl Kimi {
    pub fn new() -> Kimi {
        Kimi {
            agent: kimi_agent(),
        }
    }

    /// p's folder, or Kimi's home when p has none.
    fn dir(p: &Profile) -> String {
        if p.dir.is_empty() {
            kimi_home()
        } else {
            p.dir.clone()
        }
    }

    /// The older Python kimi-cli's sessions, listed in its kimi.json by
    /// the folders they ran in.
    fn legacy_past(&self, p: &Profile) -> Vec<Session> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Dir {
            path: String,
            kaos: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Meta {
            work_dirs: Vec<Dir>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct State {
            custom_title: String,
        }
        let dir = Path::new(&p.dir);
        let Ok(b) = std::fs::read(dir.join("kimi.json")) else {
            return Vec::new();
        };
        let Ok(meta) = serde_json::from_slice::<Meta>(&b) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for d in meta.work_dirs {
            if !d.kaos.is_empty() && d.kaos != "local" {
                continue;
            }
            let hash: String = Md5::digest(d.path.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let root = dir.join("sessions").join(hash);
            for path in glob(&root, &["*", "context.jsonl"]) {
                let Ok(st) = std::fs::metadata(&path) else {
                    continue;
                };
                if st.len() == 0 {
                    continue;
                }
                let sdir = path.parent().unwrap_or(Path::new(""));
                let id = sdir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut name = id.clone();
                if let Ok(b) = std::fs::read(sdir.join("state.json"))
                    && let Ok(s) = serde_json::from_slice::<State>(&b)
                    && !s.custom_title.is_empty()
                {
                    name = s.custom_title;
                }
                out.push(Session {
                    kind: self.agent.id.clone(),
                    profile: p.clone(),
                    id,
                    name,
                    cwd: d.path.clone(),
                    transcript: path_str(&path),
                    state: "done".into(),
                    updated_at: mod_ms(&st),
                    ..Session::default()
                });
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        out
    }
}

/// Where Kimi keeps its config and sign-in: Kimi Code's ~/.kimi-code
/// (KIMI_CODE_HOME), else the older Python kimi-cli's ~/.kimi
/// (KIMI_SHARE_DIR).
pub fn kimi_home() -> String {
    for v in ["KIMI_CODE_HOME", "KIMI_SHARE_DIR"] {
        let d = env(v);
        if !d.is_empty() {
            return d;
        }
    }
    let h = Path::new(&home_dir()).to_path_buf();
    if std::fs::metadata(h.join(".kimi-code").join("config.toml")).is_ok() {
        return path_str(&h.join(".kimi-code"));
    }
    path_str(&h.join(".kimi"))
}

/// Points either Kimi at dir.
fn kimi_env(env: &[String], dir: &str) -> Vec<String> {
    let mut out = env.to_vec();
    out.push(format!("KIMI_CODE_HOME={dir}"));
    out.push(format!("KIMI_SHARE_DIR={dir}"));
    out
}

/// A session with no transcript to read.
fn no_transcript() -> adapter::Error {
    Box::new(Error::other("kimi: this session has no transcript"))
}

impl Adapter for Kimi {
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
        out.insert(
            Feature::Quota,
            y("Kimi Code plans only, as its /usage reads them"),
        );
        out.insert(Feature::Compact, y("its own /compact"));
        out.insert(
            Feature::Background,
            y("background shells and subagents, ended as Kimi says"),
        );
        out
    }
    fn level(&self) -> Level {
        Level::Tested
    }
    fn profiles(&self) -> Vec<Profile> {
        if !installed(&self.agent.id) {
            return Vec::new();
        }
        vec![Profile {
            kind: self.agent.id.clone(),
            name: self.agent.title.clone(),
            dir: kimi_home(),
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
        login_hinter: LoginHinter,
        chooser: Chooser,
        model_lister: ModelLister,
        discoverer: Discoverer,
        history_reader: HistoryReader,
        history_follower: HistoryFollower,
        tail_reader: TailReader,
        authenticator: Authenticator,
        quota_source: QuotaSource,
        account_summary_reader: AccountSummaryReader,
        child_finder: ChildFinder,
    );
}

impl Driver for Kimi {
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        if !o.profile.dir.is_empty() {
            o.env = kimi_env(&o.env, &o.profile.dir);
        }
        Ok(Box::new(self.agent.run(o, sink, true)?))
    }
}

impl OnceReader for Kimi {
    fn read_once(&self, args: &[String]) -> Option<Once> {
        self.agent.once_of(args)
    }
}

impl Spawnable for Kimi {
    fn spawn_command(&self) -> (String, String) {
        (self.agent.once.clone(), self.agent.model.clone())
    }
}

impl Programmer for Kimi {
    fn program(&self) -> (String, Vec<String>) {
        (
            self.agent.command.clone(),
            vec![".kimi-code/bin".into(), ".local/bin".into()],
        )
    }
}

impl Made for Kimi {
    fn maker(&self) -> String {
        self.agent.company.clone()
    }
}

impl Publisher for Kimi {
    fn published(&self) -> Published {
        Published {
            update: vec!["upgrade".into()],
            ..Published::default()
        }
    }
}

impl LoginHinter for Kimi {
    fn login_hint(&self) -> String {
        "run `kimi login` in a terminal".into()
    }
}

impl Chooser for Kimi {
    // ponytail: Kimi's thinking levels and modes as its ACP session/new
    // advertised them (2026-10); a running session's own list wins.
    fn choices(&self) -> Choices {
        let c = |id: &str, note: &str| Choice {
            id: id.into(),
            note: note.into(),
            context: 0,
        };
        Choices {
            efforts: vec![
                c("low", "thinks briefly; quick for simple tasks."),
                c("high", "thinks carefully."),
                c("max", "thinks as much as it can; Kimi's default."),
            ],
            modes: vec![
                c("default", "asks before tools run."),
                c("plan", "reads and plans; runs no tools."),
                c("auto", "approves safe operations itself."),
                c("yolo", "approves everything; never asks."),
            ],
            ..Choices::default()
        }
    }
}

impl ModelLister for Kimi {
    fn list_models(&self, p: &Profile) -> Vec<Choice> {
        let Ok(cfg) = read_kimi_config(&Kimi::dir(p)) else {
            return Vec::new();
        };
        let mut out: Vec<Choice> = cfg
            .models
            .into_iter()
            .map(|(id, m)| Choice {
                id,
                note: if m.display_name.is_empty() {
                    m.model
                } else {
                    m.display_name
                },
                context: m.max_context_size,
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

impl Discoverer for Kimi {
    fn live(&self, _: &Profile) -> Vec<Session> {
        Vec::new()
    }
    fn past(&self, p: &Profile) -> Vec<Session> {
        let mut p = p.clone();
        p.dir = Kimi::dir(&p);
        let mut out = code_past(&self.agent.id, &p);
        out.extend(self.legacy_past(&p));
        out.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        out
    }
}

impl HistoryReader for Kimi {
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> adapter::Result<()> {
        if s.transcript.is_empty() {
            return Err(no_transcript());
        }
        if base(&s.transcript) == "wire.jsonl" {
            let f = std::fs::File::open(&s.transcript)?;
            let mut lines = wire_lines(Path::new(&s.transcript), before);
            return Ok(read_each(f, &mut lines, most, each)?);
        }
        // Legacy context records have no timestamps.
        Ok(messages_each(&s.transcript, before, most, each)?)
    }
}

impl HistoryFollower for Kimi {
    fn follow_history(&self, s: &Session) -> adapter::Result<Box<dyn Follow>> {
        if s.transcript.is_empty() {
            return Err(no_transcript());
        }
        let fresh = if base(&s.transcript) == "wire.jsonl" {
            wire_boxed
        } else {
            MessageLines::boxed
        };
        Ok(Box::new(Follower::new(&s.transcript, fresh)))
    }
}

impl TailReader for Kimi {
    fn history_tail(&self, s: &Session, most: i64) -> adapter::Result<(Vec<Event>, bool)> {
        if base(&s.transcript) == "wire.jsonl" {
            return Ok(read_kimi_wire_file(&s.transcript, most, 0)?);
        }
        Ok(read_messages_tail(&s.transcript, most)?)
    }
}

impl KeyChecker for Kimi {
    /// A configured model has stored provider credentials. The CLI stays
    /// responsible for checking and refreshing them with its server.
    fn check_key(&self, p: &Profile) -> adapter::Result<()> {
        let cred = kimi_credential(&Kimi::dir(p));
        if !cred.token.is_empty() {
            if cred.expired && !cred.refreshable {
                return Err(Box::new(Error::other(
                    "Kimi access token expired and no refresh credential was found",
                )));
            }
            return Ok(());
        }
        Err(Box::new(Error::other(
            "Rush could not verify local Kimi credentials; check the account in Kimi",
        )))
    }
}

impl Authenticator for Kimi {
    fn sign_in_command(&self, p: &Profile) -> adapter::Result<Command> {
        let bin = adapter::global().path(self.agent.id.as_str());
        if bin.is_empty() {
            return Err(Box::new(Error::other("Kimi CLI is not installed")));
        }
        let mut cmd = Command::new(bin);
        cmd.arg("login");
        if !p.dir.is_empty() {
            cmd.env("KIMI_CODE_HOME", &p.dir)
                .env("KIMI_SHARE_DIR", &p.dir);
        }
        Ok(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::read_messages;
    use rush_core::event::PartKind;

    fn profile(dir: &Path) -> Profile {
        Profile {
            kind: Kind::from("kimi"),
            dir: path_str(dir),
            ..Profile::default()
        }
    }

    // history_test.go: TestKimiConfiguredModelsAndPast
    #[test]
    fn configured_models_and_past() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(
            dir.join("config.toml"),
            "[models.\"custom.model\"]\nmodel = \"kimi-k2\"\nmax_context_size = 262144\ndisplay_name = \"My Kimi\"\n",
        )
        .unwrap();
        let a = Kimi::new();
        let models = a.list_models(&profile(dir));
        assert_eq!(models.len(), 1);
        assert_eq!(
            (
                models[0].id.as_str(),
                models[0].context,
                models[0].note.as_str()
            ),
            ("custom.model", 262144, "My Kimi")
        );
        let cwd = "/work/project";
        let hash: String = Md5::digest(cwd.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let root = dir.join("sessions").join(hash).join("session-1");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            dir.join("kimi.json"),
            r#"{"work_dirs":[{"path":"/work/project","kaos":"local"}]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("context.jsonl"),
            r#"{"role":"_checkpoint","id":0}
{"role":"user","content":[{"type":"text","text":"full pasted text"},{"type":"image_url","image_url":{"url":"data:image/png;base64,aGk="}}]}
{"role":"assistant","content":"answer","tool_calls":[{"id":"c1","function":{"name":"ReadFile","arguments":"{\"path\":\"a.go\"}"}}]}
{"role":"tool","tool_call_id":"c1","content":"file contents"}
"#,
        )
        .unwrap();
        let past = a.past(&profile(dir));
        assert_eq!(past.len(), 1);
        assert_eq!(
            (past[0].cwd.as_str(), past[0].id.as_str()),
            (cwd, "session-1")
        );
        let evs = read_messages(&past[0].transcript).unwrap();
        assert_eq!(evs.len(), 4, "{evs:?}");
        let Event::Message(user) = &evs[0] else {
            panic!()
        };
        assert_eq!(user.parts.len(), 2);
        assert_eq!(user.parts[0].text, "full pasted text");
        assert_eq!(user.parts[1].image.as_ref().unwrap().data, b"hi");
        let Event::Message(res) = &evs[2] else {
            panic!()
        };
        assert_eq!(res.parts[0].output.as_ref().unwrap().call_id, "c1");
    }

    // kimi_wire_test.go: TestKimiCodeDiscoveryAndTail
    #[test]
    fn code_discovery_and_tail() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("sessions/wd_project_123/session_one");
        let wire = root.join("agents/main/wire.jsonl");
        std::fs::create_dir_all(wire.parent().unwrap()).unwrap();
        std::fs::write(
            root.join("state.json"),
            r#"{"id":"session_one","cwd":"/work/project","title":"Saved Kimi conversation","updatedAt":4000}"#,
        )
        .unwrap();
        std::fs::write(&wire, crate::kimi_wire::tests::FIXTURE).unwrap();
        let a = Kimi::new();
        let past = a.past(&profile(d.path()));
        assert_eq!(past.len(), 1);
        let s = &past[0];
        assert_eq!(
            (s.id.as_str(), s.cwd.as_str(), s.name.as_str(), s.updated_at),
            (
                "session_one",
                "/work/project",
                "Saved Kimi conversation",
                4000
            )
        );
        let evs = adapter::read_history(&a, s, 3000).unwrap();
        assert!(!evs.is_empty());
        let (evs, cut) = a.history_tail(s, 300).unwrap();
        assert!(cut && !evs.is_empty());
        std::fs::remove_file(&wire).unwrap();
        assert!(
            a.past(&profile(d.path())).is_empty(),
            "a missing transcript isn't resumable history"
        );
    }

    #[test]
    fn follows_a_wire() {
        let d = tempfile::tempdir().unwrap();
        let wire = d.path().join("agents/main/wire.jsonl");
        std::fs::create_dir_all(wire.parent().unwrap()).unwrap();
        std::fs::write(&wire, crate::kimi_wire::tests::FIXTURE).unwrap();
        let s = Session {
            transcript: path_str(&wire),
            ..Session::default()
        };
        let mut f = Kimi::new().follow_history(&s).unwrap();
        assert_eq!(f.wait_on().paths, std::slice::from_ref(&wire));
        let sl = f.next(1 << 20).unwrap();
        let texts: Vec<&str> = sl
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Message(m) if m.parts[0].kind == PartKind::Text => {
                    Some(m.parts[0].text.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            ["question", "background report", "answer", "later question"]
        );
        assert!(f.next(1 << 20).unwrap().events.is_empty());
    }

    // kimi_account_test.go: TestKimiHome. HOME is the process's, so this
    // reads it through the variables Kimi looks at first.
    #[test]
    fn home_from_its_variables() {
        // SAFETY: the only test that sets these.
        unsafe {
            std::env::set_var("KIMI_CODE_HOME", "/x");
        }
        assert_eq!(kimi_home(), "/x");
        unsafe {
            std::env::remove_var("KIMI_CODE_HOME");
        }
    }
}
