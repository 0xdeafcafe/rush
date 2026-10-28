//! An agent as rush knows it: the adapter interface every agent (Claude
//! Code, Codex, Copilot, Kimi, …) fills in, the registry of them, and what
//! rush asks of them without ever naming one. After Go's internal/agent:
//! adapter.go (Kind, Profile, Account, Adapter, the registry) and the
//! interfaces beside it.
//!
//! Go finds an adapter's optional parts with a type assertion
//! (`a.(Previewer)`); here each is a trait, and [`Adapter`] has a method
//! per trait that hands it out, None by default: an adapter overrides
//! those it can do, returning `Some(self)`. Marker interfaces (Go's
//! `ClaudeTranscripts()`, `Warm()`) are methods returning a bool.

mod caps;
mod conn;
mod feature;
mod handoff;
mod installed;
mod registry;
mod types;
mod window;

pub use caps::*;
pub use conn::*;
pub use feature::*;
pub use handoff::*;
pub use installed::find;
pub use registry::*;
pub use types::*;
pub use window::*;

use serde::{Deserialize, Serialize};

/// Names an agent: "claude", "codex", "copilot", "kimi".
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Kind(pub String);

impl Kind {
    pub fn new(s: impl Into<String>) -> Kind {
        Kind(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for Kind {
    fn from(s: &str) -> Kind {
        Kind(s.to_string())
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The agent an empty kind meant: rush ran only Claude Code before it ran
/// others, and wrote no kind for it.
pub const LEGACY_KIND: &str = "claude"; // migration: what older state meant by no kind

/// A kind read from state, a host's info or config an older rush wrote, as
/// it's kept now: an empty one is LEGACY_KIND. Call it only where such a
/// kind is read in; nothing past there sees an empty kind.
pub fn migrated(kind: &str) -> Kind {
    if kind.is_empty() {
        return Kind::from(LEGACY_KIND); // migration: older rushes wrote no kind for Claude Code
    }
    Kind::from(kind)
}

/// One config home of one agent: where its settings, transcripts and
/// sessions live.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub kind: Kind,
    pub name: String,
    pub dir: String,
}

/// Who pays for a session and whose limits it spends: a sign-in with a
/// plan, or an API key. Any number of profiles and agents can use one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Account {
    /// The adapter that knows how to sign it in.
    pub kind: Kind,
    /// The agent's own id for it.
    #[serde(rename = "ID")]
    pub id: String,
    /// Unique across adapters, and where its Quota is kept:
    /// "claude:login:<uuid>".
    pub key: String,
    pub name: String,
    pub email: String,
    pub org: String,
    pub plan: String,
}

/// What an adapter's calls fail with. Callers look for [`Gone`] and
/// [`Elsewhere`] with `downcast_ref`.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// A session its agent has let go of: its conversation is kept, and
/// carries on in rush mode (Go's ErrGone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the agent has let the session go")
    }
}

impl std::error::Error for Gone {}

/// A session's screen opened in another window, which takes it from this
/// one (Go's ErrElsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elsewhere;

impl std::fmt::Display for Elsewhere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the session was opened in another window")
    }
}

impl std::error::Error for Elsewhere {}

/// Writes an optional part's accessor: None unless the adapter has it.
macro_rules! parts {
    ($($(#[$m:meta])* $f:ident: $t:ident;)*) => {
        $( $(#[$m])* fn $f(&self) -> Option<&dyn $t> { None } )*
    };
}

/// One agent as rush knows it. The optional parts after the first five are
/// found through their methods; an adapter has only those it can do.
pub trait Adapter: Send + Sync {
    fn kind(&self) -> Kind;
    /// "Claude Code"
    fn name(&self) -> String;
    /// What rush can do with it, feature by feature. A feature it leaves
    /// out is No.
    fn features(&self) -> Features;
    /// How far rush's support for it has been tried.
    fn level(&self) -> Level;
    /// Its config homes on this machine.
    fn profiles(&self) -> Vec<Profile>;

    // Markers: Go interfaces with nothing to call.

    /// Its sessions write Claude Code's transcripts, which the conversation
    /// pane reads as they grow (Go's ClaudeTranscripts).
    fn claude_transcripts(&self) -> bool {
        false
    }
    /// Its models stay warm in the cloud, so a session done with its turn
    /// rests at once: a wake costs it nothing (Go's Warm).
    fn warm(&self) -> bool {
        false
    }
    /// Its running subagents can be sent a message straight, with
    /// StartOptions.inbox, rather than through the main session (Go's
    /// SubagentInbox).
    fn takes_subagent_messages(&self) -> bool {
        false
    }
    /// Its QuotaSource reads any account's limits, not only those of the
    /// account a profile is signed in to (Go's AnyAccountQuota).
    fn reads_any_account(&self) -> bool {
        false
    }

    parts! {
        /// A home of its own (Go's Homer).
        homer: Homer;
        /// Finds its sessions outside rush.
        discoverer: Discoverer;
        /// Runs a session headless.
        driver: Driver;
        orphans: Orphans;
        history_reader: HistoryReader;
        history_follower: HistoryFollower;
        tail_reader: TailReader;
        history_tail_before_reader: HistoryTailBeforeReader;
        quota_source: QuotaSource;
        reset_spender: ResetSpender;
        plan_reader: PlanReader;
        accounts: Accounts;
        sign_in_reader: SignInReader;
        known: Known;
        pricer: Pricer;
        fast_pricer: FastPricer;
        default_modeler: DefaultModeler;
        commander: Commander;
        previewer: Previewer;
        codenamer: Codenamer;
        spend_reader: SpendReader;
        chooser: Chooser;
        model_lister: ModelLister;
        skill_rooter: SkillRooter;
        runner_snapshot: RunnerSnapshot;
        transcript_locator: TranscriptLocator;
        brancher: Brancher;
        transcripts: Transcripts;
        model_namer: ModelNamer;
        attributor: Attributor;
        /// Words of its own for what its tools do.
        describer: Describer;
        cwd_reader: CwdReader;
        authenticator: Authenticator;
        login_hinter: LoginHinter;
        account_summary_reader: AccountSummaryReader;
        programmer: Programmer;
        lesser: Lesser;
        rider: Rider;
        provided: Provided;
        made: Made;
        keyed: Keyed;
        plan_rider: PlanRider;
        speaker: Speaker;
        worded: Worded;
        replaced: Replaced;
        spawn_finder: SpawnFinder;
        child_finder: ChildFinder;
        spawnable: Spawnable;
        once_reader: OnceReader;
        key_checker: KeyChecker;
        media_reader: MediaReader;
        context_windower: ContextWindower;
        compacter: Compacter;
        project_folderer: ProjectFolderer;
        publisher: Publisher;
        last_quota_reader: LastQuotaReader;
        querier: Querier;
        rewinder: Rewinder;
        summarizer: Summarizer;
        stats_reader: StatsReader;
        settings_filer: SettingsFiler;
        status_liner: StatusLiner;
        status_line_profiler: StatusLineProfiler;
        settings_pager: SettingsPager;
        builtin_def_namer: BuiltinDefNamer;
        definer: Definer;
        plugger: Plugger;
        memory_reader: MemoryReader;
        timeliner: Timeliner;
        run_follower: RunFollower;
        job_keeper: JobKeeper;
        scratcher: Scratcher;
        joiner: Joiner;
        dispatcher: Dispatcher;
        stopper: Stopper;
        remover: Remover;
        replier: Replier;
        attacher: Attacher;
        pinner: Pinner;
        sessions_viewer: SessionsViewer;
        loginer: Loginer;
        screener: Screener;
        mover: Mover;
        /// The agent whose own words rush still reads apart from its Driver;
        /// one adapter at most is.
        native: Native;
    }
}
