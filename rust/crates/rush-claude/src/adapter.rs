//! Claude Code as a rush adapter: rush_core::adapter::Adapter and the parts
//! of it this crate can do today. Port of internal/adapters/claude:
//! claude.go (Kind, Name, Features, Level, Program, Home, Cost, KeyEnv,
//! Maker, Speaks, Words, LoginHint, ProjectFolder), discover.go (Live,
//! Past), jobs.go (Workers, Watched, FindTranscript), efficiency.go
//! (TranscriptsDir, CostFast), subagents.go (SubagentsDir,
//! CountSubagents), spend.go (SpendScanner), history.go (History),
//! native.go (Doing, KindOf, Call, Output) and choices.go
//! (ContextWindow).
//!
//! Not here yet, for wave B: the Driver (headless sessions), the host's
//! half of Native (NativeStream), accounts and sign-in, quotas, commands,
//! and the rest of Go's parts.

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
}

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
            spend: Arc::new(ClaudeSpend(Mutex::new(Scanner::new()))),
        }
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

/// What Claude Code does in rush today; the rest of what Go's says it does
/// (claude.go:57) is Planned until its module is ported.
fn features() -> Features {
    use Feature::*;
    let mut f: Features = [Live, History, Pricing]
        .into_iter()
        .map(|k| (k, Support::YES))
        .collect();
    for k in [
        Run,
        Prompt,
        Port,
        Resume,
        Fork,
        Rewind,
        Interrupt,
        Guide,
        Model,
        Effort,
        Modes,
        Plan,
        Images,
        Questions,
        Subagents,
        Background,
        Context,
        Compact,
        Commands,
        SideQuestion,
        Dirs,
        Mcp,
        Hooks,
        Plugins,
        StatusLine,
        Screen,
        HandoffIn,
        Settings,
        Stats,
        Switch,
        SignIn,
        Quota,
        Memory,
    ] {
        f.insert(k, Support::PLANNED);
    }
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
        let a = account(p);
        let at = a.transcript_path(cwd, sid);
        if at.exists() {
            return path_str(&at);
        }
        let name = format!("{sid}.jsonl");
        std::fs::read_dir(a.projects_dir())
            .ok()
            .and_then(|r| {
                r.flatten()
                    .map(|e| e.path().join(&name))
                    .find(|p| p.exists())
            })
            .map(|p| path_str(&p))
            .unwrap_or_default()
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
    /// Nothing to save: the cache lives for the process.
    fn flush(&self) {}
}

impl SpendReader for Claude {
    fn spend_scanner(&self) -> Arc<dyn SpendScanner> {
        self.spend.clone()
    }
}

impl RunFollower for Claude {
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
