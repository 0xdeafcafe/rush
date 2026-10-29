//! Claude Code as a rush adapter: rush_core::adapter::Adapter and the parts
//! of it this crate can do today. Port of internal/adapters/claude:
//! claude.go (Kind, Name, Features, Level, Program, Home, Cost, KeyEnv,
//! Maker, Speaks, Words, LoginHint, ProjectFolder), discover.go (Live,
//! Past), jobs.go (Workers, Watched, FindTranscript), efficiency.go
//! (TranscriptsDir, CostFast), subagents.go (SubagentsDir,
//! CountSubagents), spend.go (SpendScanner), history.go (History),
//! native.go (Doing, KindOf, Call, Output), choices.go (Choices),
//! branch.go (TranscriptPath, Branch, DefaultModel, ContextWindow, Reads,
//! ModelName, Stats), claude.go:84-150 (Config, runAs, Commands,
//! BuiltinDef, Published), scratch.go, subagents.go (SubagentRuns),
//! timeline.go, lastquota.go, summary.go, memory.go, settings.go,
//! settingspage.go, attribution.go, plugins.go, and accounts.go and
//! logins.go through accounts.rs.
//!
//! Running Claude Code is beside it: conn.rs (Driver, Orphans), native.rs
//! (NativeStream, Rewinder), spawn.rs (SpawnFinder), query.rs (Querier),
//! screen.rs (Joiner, Pinner, SessionsViewer) and actions.rs (Dispatcher,
//! Stopper, Remover, Replier, Attacher, Loginer, Screener, Mover).
//!
//! Compacting, side questions (btw), cd and add-dir, and taking over
//! another agent's conversation (Go's Compact, SideQuestion, Dirs,
//! HandoffIn, claude.go:62-74) are what a session does through its host:
//! a control request (Asker) for compact and btw, the host's cd op, and a
//! history carried in (Config's carry) or told for a hand-off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    self, ContextWindower, Describer, Discoverer, FastPricer, Feature, Features, HistoryFollower,
    HistoryReader, Homer, JobKeeper, Keyed, Kind, Level, LoginHinter, Made, Native, Previewer,
    Pricer, Profile, Programmer, ProjectFolderer, RunFollower, Session, Speaker, SpendReader,
    SpendScanner, SpendTarget, SubagentStats, Support, TranscriptLocator, Transcripts, Worded,
};
use rush_core::agent::{Preview, Spend, TokenUsage};
use rush_core::time::{Ms, now};
use rush_core::tool;
use serde_json::value::RawValue;

use crate::account::{Account, path_str};
use crate::discover::Discovery;
use crate::sessions::read_workers;
use crate::spend::{Scanner, Target};
use crate::{history, pricing, tools, transcript};

/// Claude Code's kind.
pub const KIND: &str = "claude";

/// Claude Code.
pub struct Claude {
    /// The home rush lists: CLAUDE_CONFIG_DIR's, else ~/.claude.
    home: Account,
    /// What was read of each profile's folders, so a reading every second
    /// reads only what changed.
    found: Mutex<Discovery>,
    spend: Arc<ClaudeSpend>,
    /// rush's config, for the logins and folders it knows; None loads it.
    config: Option<ConfigFn>,
}

/// Gives rush's config: tests give their own.
pub type ConfigFn = Arc<dyn Fn() -> rush_state::Config + Send + Sync>;

impl Default for Claude {
    fn default() -> Self {
        Claude::new()
    }
}

impl Claude {
    /// Claude Code in the folder it runs in from this environment.
    pub fn new() -> Claude {
        Claude::at(Account::from_env())
    }

    /// Claude Code at home in acct's folder: tests use a temp dir.
    pub fn at(acct: Account) -> Claude {
        Claude {
            home: acct,
            found: Mutex::new(Discovery::new()),
            spend: Arc::new(ClaudeSpend(Mutex::new(Scanner::kept()))),
            config: None,
        }
    }

    /// Claude Code at home in acct's folder, with rush's config from
    /// config rather than its folder.
    pub fn with_config(acct: Account, config: ConfigFn) -> Claude {
        Claude {
            config: Some(config),
            ..Claude::at(acct)
        }
    }

    /// rush's config, for the logins and folders it knows.
    pub(crate) fn config(&self) -> rush_state::Config {
        match &self.config {
            Some(f) => f(),
            None => rush_state::Store::load().config,
        }
    }

    /// The folder a session started for ~/.claude runs in: the home of the
    /// login in use, ready, or ~/.claude when it can't be readied.
    pub fn run_as(&self) -> Account {
        let cfg = self.config();
        let acct = crate::home::run_account(&cfg);
        if acct.is_default() {
            return acct;
        }
        let root = crate::home::active(&cfg);
        if crate::home::link_home(&acct, &root).is_err() || !crate::home::has_home(&acct) {
            return root;
        }
        acct
    }
}

/// A Claude config folder as a profile of rush's.
pub fn profile(a: &Account) -> Profile {
    Profile {
        kind: Kind::from(KIND),
        name: a.name.clone(),
        dir: path_str(&a.config_dir),
    }
}

/// A profile as the Claude config folder it is.
pub fn account(p: &Profile) -> Account {
    Account::at(&p.name, &p.dir)
}

/// What Claude Code does in rush: everything rush does, bar sessions on
/// Anthropic's servers (claude.go:62 features).
fn features() -> Features {
    use Feature::*;
    let mut f: Features = [
        Live, History, Pricing, Run, Prompt, Port, Resume, Fork, Rewind, Interrupt, Model, Effort,
        Modes, Plan, Images, Questions, Subagents, Background, Context, Compact, Commands,
        SideQuestion, Dirs, Mcp, Hooks, Screen, HandoffIn, Plugins, StatusLine, Settings, Stats,
        Switch, SignIn, Quota, Memory,
    ]
    .into_iter()
    .map(|k| (k, Support::YES))
    .collect();
    f.insert(Guide, Support::YES.with("at its next step"));
    f
}

impl adapter::Adapter for Claude {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "Claude Code".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Full
    }
    fn profiles(&self) -> Vec<Profile> {
        vec![profile(&self.home)]
    }
    fn claude_transcripts(&self) -> bool {
        true
    }
    fn warm(&self) -> bool {
        true
    }

    rush_core::has!(
        homer: Homer,
        discoverer: Discoverer,
        job_keeper: JobKeeper,
        transcripts: Transcripts,
        transcript_locator: TranscriptLocator,
        previewer: Previewer,
        spend_reader: SpendReader,
        run_follower: RunFollower,
        pricer: Pricer,
        fast_pricer: FastPricer,
        context_windower: ContextWindower,
        describer: Describer,
        native: Native,
        history_reader: HistoryReader,
        history_follower: HistoryFollower,
        programmer: Programmer,
        project_folderer: ProjectFolderer,
        login_hinter: LoginHinter,
        made: Made,
        keyed: Keyed,
        speaker: Speaker,
        worded: Worded,
        driver: Driver,
        orphans: Orphans,
        native_stream: NativeStream,
        spawn_finder: SpawnFinder,
        querier: Querier,
        rewinder: Rewinder,
        joiner: Joiner,
        pinner: Pinner,
        sessions_viewer: SessionsViewer,
        dispatcher: Dispatcher,
        stopper: Stopper,
        remover: Remover,
        replier: Replier,
        attacher: Attacher,
        loginer: Loginer,
        screener: Screener,
        mover: Mover,
        accounts: Accounts,
        quota_source: QuotaSource,
        sign_in_reader: SignInReader,
        plan_reader: PlanReader,
        default_modeler: DefaultModeler,
        commander: Commander,
        chooser: Chooser,
        skill_rooter: SkillRooter,
        brancher: Brancher,
        model_namer: ModelNamer,
        attributor: Attributor,
        media_reader: MediaReader,
        compacter: Compacter,
        publisher: Publisher,
        last_quota_reader: LastQuotaReader,
        summarizer: Summarizer,
        stats_reader: StatsReader,
        settings_filer: SettingsFiler,
        status_liner: StatusLiner,
        status_line_profiler: StatusLineProfiler,
        settings_pager: SettingsPager,
        builtin_def_namer: BuiltinDefNamer,
        definer: Definer,
        plugger: Plugger,
        memory_reader: MemoryReader,
        memory_keeper: MemoryKeeper,
        timeliner: Timeliner,
        scratcher: Scratcher,
    );
}

impl Homer for Claude {
    fn home(&self) -> Profile {
        profile(&Account::home())
    }
}

impl Discoverer for Claude {
    /// Its background jobs, then every session whose process lives.
    fn live(&self, p: &Profile) -> Vec<Session> {
        let acct = account(p);
        let now = now();
        let mut d = self.found.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<Session> = d
            .jobs(&acct, now)
            .into_iter()
            .map(|cj| {
                let j = cj.job;
                Session {
                    kind: Kind::from(KIND),
                    profile: p.clone(),
                    id: j.session_id.clone(),
                    name: j.name.clone(),
                    // The job file's own word: "running" is a turn Claude
                    // Code hasn't classed yet, which the list reads apart.
                    state: cj.raw_state,
                    detail: j.detail.clone(),
                    needs: j.needs.clone(),
                    cwd: j.cwd.clone(),
                    transcript: j.transcript_path.clone(),
                    todos: j.todos.clone(),
                    running: j.running.clone(),
                    created_at: j.created_at,
                    updated_at: j.updated_at,
                    job_id: j.id.clone(),
                    job: Some(Box::new(j)),
                    // Claude Code's own record of it, for a move.
                    extra: Some(Arc::new(crate::jobs::ClaudeJob {
                        job: rush_core::agent::Job::default(),
                        raw_state: String::new(),
                        name_source: cj.name_source,
                        tempo: cj.tempo,
                        children: cj.children,
                        todos_done: cj.todos_done,
                        cli_version: cj.cli_version,
                        respawn_flags: cj.respawn_flags,
                    })),
                    ..Session::default()
                }
            })
            .collect();
        for ss in d.sessions(&acct) {
            out.push(Session {
                kind: Kind::from(KIND),
                profile: p.clone(),
                created_at: ss.started_at(),
                updated_at: ss.updated_at(),
                status_at: ss.status_at(),
                id: ss.session_id,
                name: ss.name,
                cwd: ss.cwd,
                pid: ss.pid,
                interactive: ss.kind == "interactive",
                status: ss.status,
                job_id: ss.job_id,
                ..Session::default()
            });
        }
        out
    }

    /// Conversations in its projects folder, listed again at most every
    /// 15s: each one that is a conversation of its own.
    fn past(&self, p: &Profile) -> Vec<Session> {
        let acct = account(p);
        let mut d = self.found.lock().unwrap_or_else(|e| e.into_inner());
        d.past(&acct, now())
            .filter_map(|f| {
                let c = f.convo.as_ref()?;
                Some(Session {
                    kind: Kind::from(KIND),
                    profile: p.clone(),
                    id: c.session_id.clone(),
                    name: c.title.clone(),
                    state: "stopped".into(),
                    cwd: c.cwd.clone(),
                    transcript: path_str(&f.path),
                    created_at: c.started,
                    updated_at: f.modified,
                    ..Session::default()
                })
            })
            .collect()
    }

    /// A change under the projects folder (or one it can't place) lists
    /// it again at once, rather than when PAST_EVERY is up.
    fn changed(&self, p: &Profile, paths: Option<&[PathBuf]>) {
        let acct = account(p);
        let root = acct.projects_dir();
        if paths.is_none_or(|ps| ps.iter().any(|x| x.starts_with(&root))) {
            let mut d = self.found.lock().unwrap_or_else(|e| e.into_inner());
            d.expire_past(&acct);
        }
    }
}

impl JobKeeper for Claude {
    /// The daemon's workers, by their job's short id (the first named).
    fn workers(&self, p: &Profile) -> HashMap<String, i32> {
        let mut out = HashMap::new();
        for (id, pid) in read_workers(&account(p)) {
            out.entry(id).or_insert(pid);
        }
        out
    }
    /// Not read yet: Claude Code's native view's pins.
    fn pins(&self, _: &Profile) -> HashMap<String, i64> {
        HashMap::new()
    }
    /// Not read yet: Claude Code's PR status cache.
    fn linked_prs(&self, _: &Profile) -> HashMap<String, adapter::Pr> {
        HashMap::new()
    }
    /// The folders sessions and jobs register in, the daemon's roster and
    /// the state file, and each job's folder and state (jobs.go:76).
    fn watched(&self, p: &Profile, jobs: &[String]) -> Vec<PathBuf> {
        let a = account(p);
        let mut out = vec![
            a.config_dir.clone(),
            a.sessions_dir(),
            a.jobs_dir(),
            a.config_dir.join("daemon"),
            a.roster_path(),
            a.state_path(),
        ];
        for id in jobs {
            let d = a.jobs_dir().join(id);
            out.push(d.join("state.json"));
            out.push(d);
        }
        // Each session's own file: Claude Code rewrites it in place as a
        // terminal goes busy or idle, which its folder doesn't see.
        if let Ok(rd) = std::fs::read_dir(a.sessions_dir()) {
            out.extend(
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "json")),
            );
        }
        out
    }
}

impl Transcripts for Claude {
    /// Where session sid keeps its conversation now: under the folder it
    /// started in, or, once entering a worktree moved it, under whichever
    /// project has it; "" when none does.
    fn find_transcript(&self, p: &Profile, cwd: &str, sid: &str) -> String {
        let at = account(p).find_transcript(cwd, sid);
        if at.exists() {
            return path_str(&at);
        }
        String::new()
    }
    fn transcripts_dir(&self, p: &Profile) -> String {
        path_str(&account(p).projects_dir())
    }
}

impl TranscriptLocator for Claude {
    /// Where a session started in cwd keeps its conversation, unless it
    /// moved.
    fn transcript_path(&self, p: &Profile, cwd: &str, sid: &str) -> String {
        path_str(&account(p).transcript_path(cwd, sid))
    }
}

impl Previewer for Claude {
    fn preview(&self, path: &str, window: i64) -> Preview {
        transcript::read_preview(Path::new(path), window.max(0) as u64)
    }
}

/// Claude's spend scanner, shared by every caller.
struct ClaudeSpend(Mutex<Scanner>);

impl SpendScanner for ClaudeSpend {
    fn run(&self, targets: &[SpendTarget]) -> HashMap<String, Spend> {
        let t: Vec<Target> = targets
            .iter()
            .map(|t| Target {
                key: t.key.clone(),
                path: PathBuf::from(&t.path),
                live: t.live,
                past: t.past,
            })
            .collect();
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .run(&t, now())
    }
    /// Saves the cache unless a run is under way: a run saves it too,
    /// every so often, so skipping one loses nothing (spend.go:183).
    fn flush(&self) {
        if let Ok(mut s) = self.0.try_lock() {
            s.save();
        }
    }
}

impl SpendReader for Claude {
    fn spend_scanner(&self) -> Arc<dyn SpendScanner> {
        self.spend.clone()
    }
}

impl RunFollower for Claude {
    /// Follows a session's subagent runs from its transcript and theirs
    /// (adapters/claude/subagents.go:14).
    fn subagent_runs(&self) -> Option<Box<dyn rush_core::adapter::SubagentRuns>> {
        Some(Box::new(crate::subruns::SubagentRuns::new()))
    }
    fn subagents_dir(&self, transcript: &str) -> String {
        path_str(&transcript::subagents_dir(Path::new(transcript)))
    }
    fn count_subagents(&self, transcript: &str, now: Ms) -> SubagentStats {
        transcript::read_subagent_stats(Path::new(transcript), now)
    }
}

impl Pricer for Claude {
    fn cost(&self, model: &str, u: &TokenUsage) -> Option<f64> {
        pricing::price_for(model).map(|_| pricing::cost(model, *u, false))
    }
}

impl FastPricer for Claude {
    fn cost_fast(&self, model: &str, u: &TokenUsage) -> Option<f64> {
        pricing::price_for(model).map(|_| pricing::cost(model, *u, true))
    }
}

impl ContextWindower for Claude {
    fn context_window(&self, model: &str) -> i64 {
        pricing::context_window(model)
    }
}

impl Describer for Claude {
    fn doing(&self, c: &tool::Call) -> String {
        tools::describe(c)
    }
}

impl Native for Claude {
    fn messages(&self) -> Box<dyn adapter::MessageDecoder> {
        crate::messages()
    }
    fn kind_of(&self, name: &str) -> tool::Kind {
        tools::kind_of(name)
    }
    fn call(&self, id: &str, name: &str, input: &RawValue) -> tool::Call {
        tools::call(id, name, Some(input))
    }
    fn output(
        &self,
        c: &tool::Call,
        text: &str,
        is_error: bool,
        result: Option<&RawValue>,
    ) -> tool::Output {
        tools::output(c, text, is_error, result)
    }
}

impl HistoryReader for Claude {
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<rush_core::event::Event>) -> bool,
    ) -> adapter::Result<()> {
        history::read(s, before, most, each)
    }
}

impl HistoryFollower for Claude {
    fn follow_history(&self, s: &Session) -> adapter::Result<Box<dyn adapter::Follow>> {
        history::follow(s)
    }
}

impl Programmer for Claude {
    /// claude, which Claude Code's own installer puts in ~/.claude/local.
    fn program(&self) -> (String, Vec<String>) {
        ("claude".into(), vec![".claude/local".into()])
    }
}

impl ProjectFolderer for Claude {
    fn project_folder(&self) -> String {
        ".claude".into()
    }
}

impl LoginHinter for Claude {
    fn login_hint(&self) -> String {
        "run `claude auth login` in a terminal, or add the login again in rush's Settings → Providers".into()
    }
}

impl Made for Claude {
    fn maker(&self) -> String {
        "Anthropic".into()
    }
}

impl Keyed for Claude {
    fn key_env(&self) -> String {
        "ANTHROPIC_API_KEY".into()
    }
}

impl Speaker for Claude {
    fn speaks(&self) -> String {
        "Anthropic's API".into()
    }
}

impl Worded for Claude {
    fn words(&self) -> Vec<String> {
        vec!["cc".into(), "claudecode".into()]
    }
}

#[cfg(test)]
mod tests {
    use rush_core::adapter::{Adapter, Feature, Support};

    // Claude Code says Yes to everything Go's says it does (claude.go:62),
    // nothing Planned: compacting, btw, cd and hand-offs included.
    #[test]
    fn features_are_gos() {
        let f = super::Claude::new().features();
        use Feature::*;
        for k in [
            Run, Prompt, Port, Resume, Fork, Rewind, Interrupt, Guide, Model, Effort, Modes, Plan,
            Images, Questions, Subagents, Background, Context, Compact, Commands, SideQuestion,
            Dirs, Mcp, Hooks, Plugins, StatusLine, Screen, HandoffIn, Settings, Stats, Live,
            History, Switch, SignIn, Quota, Pricing, Memory,
        ] {
            let s = f.get(&k).unwrap_or_else(|| panic!("{k:?} missing"));
            assert_eq!(s.is, rush_core::adapter::Has::Yes, "{k:?} is {s:?}");
        }
        assert_eq!(f.len(), 36, "{f:?}");
        assert_eq!(f[&Guide], Support::YES.with("at its next step"));
    }
}
