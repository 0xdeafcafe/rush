//! Codex as a rush adapter: rush_core::adapter::Adapter and the parts of
//! it Go's has. Port of internal/adapters/codex/codex.go:1-165 (Kind,
//! Name, Features, Level, Profiles, Quota, UseReset, Start and its
//! spaceFlags at :128-154, SpawnCommand,
//! KeyFlags, LoginHint, Warm, ProjectFolder, Maker, KeyEnv, Speaks,
//! Program, Published), with discover.go (Live, Past), history.go
//! (History, HistoryTail, HistoryTailBefore, FollowHistory), children.go
//! (FindChild), models.go (ListModels), choices.go (Choices, Reads) and
//! accounts.go (Current, Switch, SignIn, Forget).
//!
//! Go's Codex adapter has no Previewer, SpendReader or Pricer (pricing is
//! Planned), so neither has this one.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rush_core::adapter::{
    self, Account, Accounts, Adapter, ChildFinder, Choice, Choices, Chooser, Conn, Discoverer,
    Driver, Feature, Features, HistoryFollower, HistoryReader, HistoryTailBeforeReader, Keyed,
    Kind, Level, LoginHinter, Made, Media, MediaReader, ModelLister, Profile, Programmer,
    ProjectFolderer, Published, Publisher, QuotaSource, ResetSpender, Session, SignInDone, Sink,
    Spawnable, Speaker, StartOptions, Support, TailReader,
};
use rush_core::event::Event;
use rush_core::time::Ms;
use rush_core::usage;

use crate::discover::Heads;
use crate::{KIND, accounts, children, conn, history, models, quota};

/// Where Codex's home is looked for, and whether its program is here.
type Installed = Arc<dyn Fn() -> bool + Send + Sync>;

/// OpenAI's Codex CLI.
pub struct Codex {
    /// CODEX_HOME and HOME, when a test sets its own; else the process's.
    env: Option<(String, String)>,
    installed: Installed,
    heads: Heads,
}

impl Default for Codex {
    fn default() -> Self {
        Codex::new()
    }
}

impl Codex {
    /// Codex as this machine has it: its home from the environment, and
    /// installed when the registry finds its program.
    pub fn new() -> Codex {
        Codex {
            env: None,
            installed: Arc::new(|| adapter::global().installed(KIND)),
            heads: Heads::default(),
        }
    }

    /// Codex with CODEX_HOME and HOME as given, installed when installed
    /// says: for tests.
    pub fn with_env(
        codex_home: &str,
        home: &str,
        installed: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Codex {
        Codex {
            env: Some((codex_home.into(), home.into())),
            installed: Arc::new(installed),
            heads: Heads::default(),
        }
    }

    fn home_dir(&self) -> Option<String> {
        let (codex, home) = match &self.env {
            Some((c, h)) => (c.clone(), h.clone()),
            None => (
                std::env::var("CODEX_HOME").unwrap_or_default(),
                std::env::var("HOME").unwrap_or_default(),
            ),
        };
        if !codex.is_empty() {
            return Some(codex);
        }
        if home.is_empty() {
            return None;
        }
        Some(
            Path::new(&home)
                .join(".codex")
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// What rush integrates from app-server, not the full native Codex
/// capability set. Conversation rollback is distinct from file rewind.
fn features() -> Features {
    use Feature::*;
    let yes = |n: &str| Support::YES.with(n);
    [
        (Run, Support::YES),
        (Prompt, Support::YES),
        (Port, Support::YES),
        (Resume, Support::YES),
        (Fork, Support::YES),
        (Interrupt, Support::YES),
        (Guide, yes("steered into the turn")),
        (Model, yes("from the next turn")),
        (Effort, Support::YES),
        (Modes, yes("read-only, auto, full-access")),
        (Images, Support::YES),
        (Questions, Support::YES),
        (Subagents, yes("its spawn_agent threads")),
        (Context, yes("how full it is, not what fills it")),
        (Compact, Support::YES),
        (Mcp, Support::YES),
        (HandoffIn, Support::YES),
        (
            Background,
            yes("its background terminals and spawned threads"),
        ),
        (Live, Support::YES),
        (History, Support::YES),
        (Switch, Support::YES),
        (SignIn, Support::YES),
        (Quota, Support::YES),
        (Commands, Support::PLANNED),
        (Pricing, Support::PLANNED),
        (Memory, Support::PLANNED.with("its AGENTS.md files")),
        (Settings, Support::PLANNED.with("config.toml")),
    ]
    .into_iter()
    .collect()
}

/// The model provider rush gives Codex to pay with an API key: OpenAI's
/// own, read from OPENAI_API_KEY rather than the ChatGPT sign-in, so the
/// same CODEX_HOME keeps its threads either way.
const KEY_PROVIDER: &str = "rush-openai";

/// app-server's config overrides that pay per token with the key in
/// OPENAI_API_KEY.
pub fn key_flags() -> Vec<String> {
    vec![
        "-c".into(),
        format!("model_provider={KEY_PROVIDER:?}"),
        "-c".into(),
        format!(
            r#"model_providers.{KEY_PROVIDER}={{name="OpenAI",base_url="https://api.openai.com/v1",env_key="OPENAI_API_KEY",wire_api="responses"}}"#
        ),
    ]
}

impl Adapter for Codex {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "Codex".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Tested
    }
    /// CODEX_HOME, or ~/.codex, when it exists and codex is installed: a
    /// folder left behind by an uninstalled codex lists nothing.
    fn profiles(&self) -> Vec<Profile> {
        if !(self.installed)() {
            return Vec::new();
        }
        let Some(dir) = self.home_dir() else {
            return Vec::new();
        };
        if !Path::new(&dir).is_dir() {
            return Vec::new();
        }
        let name = Path::new(&dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        vec![Profile {
            kind: Kind::from(KIND),
            name,
            dir,
        }]
    }
    /// OpenAI keeps its models warm in the cloud.
    fn warm(&self) -> bool {
        true
    }

    rush_core::has!(
        discoverer: Discoverer,
        driver: Driver,
        history_reader: HistoryReader,
        history_follower: HistoryFollower,
        tail_reader: TailReader,
        history_tail_before_reader: HistoryTailBeforeReader,
        quota_source: QuotaSource,
        reset_spender: ResetSpender,
        accounts: Accounts,
        chooser: Chooser,
        model_lister: ModelLister,
        media_reader: MediaReader,
        child_finder: ChildFinder,
        spawnable: Spawnable,
        programmer: Programmer,
        publisher: Publisher,
        login_hinter: LoginHinter,
        project_folderer: ProjectFolderer,
        made: Made,
        keyed: Keyed,
        speaker: Speaker,
    );
}

impl Discoverer for Codex {
    fn live(&self, p: &Profile) -> Vec<Session> {
        self.heads.live(p)
    }
    fn past(&self, p: &Profile) -> Vec<Session> {
        self.heads.past(p)
    }
}

impl Driver for Codex {
    /// Runs a thread in its own app-server.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        if !o.api_key.is_empty() {
            o.env.push(format!("OPENAI_API_KEY={}", o.api_key));
            let mut flags = key_flags();
            flags.append(&mut o.flags);
            o.flags = flags;
        }
        if o.subagents > 0 {
            let mut flags = vec!["-c".into(), format!("agents.max_threads={}", o.subagents)];
            flags.append(&mut o.flags);
            o.flags = flags;
        }
        if cfg!(target_os = "macos") {
            o.env.push(format!("{SPACE_EFFICIENT}=1"));
            let mut flags = space_flags(&o.profile.dir);
            flags.append(&mut o.flags);
            o.flags = flags;
        }
        Ok(Box::new(conn::start(o, sink)?))
    }
}

/// macOS's allocator mode that hands freed memory back. Without it half
/// an app-server's heap is freed memory it keeps: with it a turn of ~670
/// commands peaked at 77 MB rather than 146, for the same CPU.
const SPACE_EFFICIENT: &str = "MallocSpaceEfficient";

/// Keeps SPACE_EFFICIENT from the commands Codex runs, whose speed it was
/// never measured on. The override replaces the exclude list in
/// config.toml (dir's, or ~/.codex's), so that list's own names come along.
fn space_flags(dir: &str) -> Vec<String> {
    let dir = if dir.is_empty() {
        Path::new(&std::env::var("HOME").unwrap_or_default()).join(".codex")
    } else {
        Path::new(dir).to_path_buf()
    };
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Policy {
        exclude: Vec<String>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Cfg {
        shell_environment_policy: Policy,
    }
    let cfg: Cfg = std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|b| toml::from_str(&b).ok())
        .unwrap_or_default();
    let names: Vec<String> = cfg
        .shell_environment_policy
        .exclude
        .iter()
        .map(String::as_str)
        .chain([SPACE_EFFICIENT])
        .map(|n| format!("{n:?}"))
        .collect();
    vec![
        "-c".into(),
        format!("shell_environment_policy.exclude=[{}]", names.join(",")),
    ]
}

impl HistoryReader for Codex {
    /// The rollout at s.transcript, or else the one of thread s.id in
    /// s.profile, as the events a live thread sends.
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> adapter::Result<()> {
        history::read(s, before, most, each)
    }
}

impl HistoryFollower for Codex {
    fn follow_history(&self, s: &Session) -> adapter::Result<Box<dyn adapter::Follow>> {
        history::follow(s)
    }
}

impl TailReader for Codex {
    /// History read from the rollout's first line, the thread's meta, then
    /// the first whole line of its last most bytes on.
    fn history_tail(&self, s: &Session, most: i64) -> adapter::Result<(Vec<Event>, bool)> {
        history::tail_before(s, most, 0)
    }
}

impl HistoryTailBeforeReader for Codex {
    fn history_tail_before(
        &self,
        s: &Session,
        most: i64,
        before: Ms,
    ) -> adapter::Result<(Vec<Event>, bool)> {
        history::tail_before(s, most, before)
    }
}

impl QuotaSource for Codex {
    /// The limits of the account p is signed in to.
    fn quota(&self, p: &Profile, _: &Account) -> adapter::Result<usage::Quota> {
        match quota::quota(p, "") {
            (q, None) => Ok(q),
            (_, Some(e)) => Err(e),
        }
    }
}

impl ResetSpender for Codex {
    /// Spends one of the signed-in account's earned limit resets.
    fn use_reset(&self, p: &Profile, id: &str) -> adapter::Result<String> {
        quota::use_reset(p, "", id)
    }
}

impl Accounts for Codex {
    fn current(&self, p: &Profile) -> adapter::Result<Account> {
        accounts::current(p)
    }
    fn switch(&self, p: &Profile, a: &Account) -> adapter::Result<()> {
        accounts::switch(p, a)
    }
    fn sign_in(&self, _: &Profile) -> adapter::Result<(Command, SignInDone)> {
        accounts::sign_in()
    }
    fn forget(&self, a: &Account) -> adapter::Result<()> {
        accounts::forget(a)
    }
}

impl Chooser for Codex {
    fn choices(&self) -> Choices {
        models::choices()
    }
}

impl ModelLister for Codex {
    fn list_models(&self, p: &Profile) -> Vec<Choice> {
        models::list_models(&p.dir)
    }
}

impl MediaReader for Codex {
    fn reads(&self, model: &str) -> Option<Media> {
        models::reads(model)
    }
}

impl ChildFinder for Codex {
    fn find_child(&self, p: &Profile, parent: &str, child: &str, start: Ms) -> Option<Session> {
        children::find_child(p, parent, child, start)
    }
}

impl Spawnable for Codex {
    /// A run of Codex a shell can hand rush to host.
    fn spawn_command(&self) -> (String, String) {
        (r#"codex exec "<task>""#.into(), "-m".into())
    }
}

impl Programmer for Codex {
    fn program(&self) -> (String, Vec<String>) {
        (KIND.into(), vec![".codex/bin".into()])
    }
}

impl Publisher for Codex {
    fn published(&self) -> Published {
        Published {
            npm: "@openai/codex".into(),
            ..Published::default()
        }
    }
}

impl LoginHinter for Codex {
    fn login_hint(&self) -> String {
        "run `codex login` in a terminal".into()
    }
}

impl ProjectFolderer for Codex {
    fn project_folder(&self) -> String {
        ".codex".into()
    }
}

impl Made for Codex {
    fn maker(&self) -> String {
        "OpenAI".into()
    }
}

impl Keyed for Codex {
    fn key_env(&self) -> String {
        "OPENAI_API_KEY".into()
    }
}

impl Speaker for Codex {
    fn speaks(&self) -> String {
        "OpenAI's Responses API".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::Registry;

    // installed_test.go:TestProfilesNeedCodexInstalled: a ~/.codex left
    // behind by an uninstalled codex isn't a profile; Codex shows only once
    // its program is found.
    #[test]
    fn profiles_need_codex_installed() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let reg = Registry::with_env(
            &bin.path().to_string_lossy(),
            &home.path().to_string_lossy(),
        );
        let r2 = reg.clone();
        reg.register(Codex::with_env(
            "",
            &home.path().to_string_lossy(),
            move || r2.installed(KIND),
        ));
        let c = reg.get(KIND).unwrap();
        if !reg.path(KIND).is_empty() {
            eprintln!("codex is installed where every machine looks");
            return;
        }
        assert!(c.profiles().is_empty(), "codex isn't installed");
        let exe = bin.path().join("codex");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        reg.recheck();
        let ps = c.profiles();
        assert_eq!(ps.len(), 1);
        assert_eq!(
            ps[0].dir,
            home.path().join(".codex").to_string_lossy().into_owned()
        );
        assert_eq!(ps[0].name, ".codex");
        reg.unregister(KIND); // lets the registry and the adapter go
    }

    #[test]
    fn parts_and_words() {
        let c = Codex::with_env("/nowhere", "", || true);
        assert!(c.profiles().is_empty(), "no such folder");
        assert!(c.warm() && c.driver().is_some() && c.history_follower().is_some());
        assert!(c.previewer().is_none() && c.spend_reader().is_none());
        let f = c.features();
        assert_eq!(f[&Feature::Guide].note, "steered into the turn");
        assert_eq!(f[&Feature::Pricing], Support::PLANNED);
        assert_eq!(
            key_flags(),
            vec![
                "-c",
                r#"model_provider="rush-openai""#,
                "-c",
                r#"model_providers.rush-openai={name="OpenAI",base_url="https://api.openai.com/v1",env_key="OPENAI_API_KEY",wire_api="responses"}"#,
            ]
        );
        assert_eq!(c.choices().efforts.len(), 6);
        assert_eq!(c.spawn_command().1, "-m");
    }

    // codex_test.go:619 (TestSpaceFlags)
    #[test]
    fn space_flags_keep_the_users_list() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        assert_eq!(
            space_flags(d),
            [
                "-c",
                r#"shell_environment_policy.exclude=["MallocSpaceEfficient"]"#
            ],
            "no config"
        );
        let cfg = "model = \"x\"\n[shell_environment_policy]\nexclude = [\"AWS_*\", \"*TOKEN*\"]\n";
        std::fs::write(dir.path().join("config.toml"), cfg).unwrap();
        assert_eq!(
            space_flags(d),
            [
                "-c",
                r#"shell_environment_policy.exclude=["AWS_*","*TOKEN*","MallocSpaceEfficient"]"#
            ],
            "the user's list"
        );
    }
}
