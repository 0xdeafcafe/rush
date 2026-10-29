//! An adapter's optional parts: each is a Go interface an adapter has only
//! when its agent can, found through its method on [`super::Adapter`].
//! After Go's internal/agent: adapter.go, agent.go, actions.go,
//! choices.go, branch.go, attribution.go, conn.go (Describer, CwdReader),
//! auth.go, harness.go, installed.go, kinds.go, lastquota.go, query.go,
//! media.go, memory.go, screen.go, route.go, settings.go, pricing.go,
//! settingspage.go, rows.go, stats.go, subagents.go, timeline.go,
//! summary.go, window.go, updater.go, rewind.go, plugins.go, spend.go,
//! native.go.
//!
//! Go's context.Context arguments are left out: an adapter bounds its own
//! calls.

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command as Cmd;
use std::sync::Arc;

use serde_json::value::RawValue;

use super::*;
use crate::agent::{Preview, Spend, TokenUsage};
use crate::event::Event;
use crate::time::Ms;
use crate::tool;
use crate::usage;

/// An agent with a home of its own, where it keeps everything when nothing
/// says otherwise (~/.claude). home reads nothing.
pub trait Homer {
    fn home(&self) -> Profile;
}

/// Finds an agent's sessions outside rush: those running in a terminal or
/// its own daemon, and past ones with a transcript.
pub trait Discoverer {
    fn live(&self, p: &Profile) -> Vec<Session>;
    fn past(&self, p: &Profile) -> Vec<Session>;
    /// What the list watches changed (None: it can't tell what): whatever
    /// the adapter keeps from earlier readings of those paths is read again
    /// at the next, however lately it was read.
    fn changed(&self, _p: &Profile, _paths: Option<&[std::path::PathBuf]>) {}
}

/// Runs a session headless, for rush to draw.
pub trait Driver {
    /// Starts (or resumes) a session. What it says goes to sink, from the
    /// adapter's own reader thread, in order, until sink.ended: the
    /// caller keeps no thread or channel per session.
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>>;
}

/// A Driver that knows its own sessions' processes by their command line. A
/// host that starts a session ends any process still running it for a host
/// that's gone, so two never write one transcript.
pub trait Orphans {
    fn runs_session(&self, args: &[String], session_id: &str) -> bool;
}

/// How much of a transcript a history read takes in one slice by default:
/// a flood arrives a slice at a time, never all at once.
pub const SLICE: usize = 1 << 20;

/// Reads a session's transcript back as the events a live session sends.
pub trait HistoryReader {
    /// Those written before `before` (0: all of them), handed to `each` a
    /// slice at a time, each slice read from at most `most` bytes of the
    /// transcript (a longer line is a slice of its own). `each` returns
    /// false to stop the read there. Nothing is kept between slices but
    /// what reading on needs (the calls still waiting for their results).
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> Result<()>;
}

/// All of a session's history at once, for callers that want the whole of
/// it (a hand-off, a summary): r's slices, joined.
pub fn read_history(r: &dyn HistoryReader, s: &Session, before: Ms) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    r.history(s, before, SLICE, &mut |evs| {
        out.extend(evs);
        true
    })?;
    Ok(out)
}

/// Reads a session's history as it grows.
pub trait HistoryFollower {
    /// Starts following s's history from its start.
    fn follow_history(&self, s: &Session) -> Result<Box<dyn Follow>>;
}

/// One session's history being followed. Nothing in it polls: the caller
/// owns the waiting, and stops by not calling. In this order, so that a
/// write landing after the last read is never missed:
///
/// 1. register a watch on what `wait_on` names (`fswait::Watcher::watch`
///    for paths) — before reading, so a write from then on wakes the wait;
/// 2. call `next` until a slice's `more` is false;
/// 3. wait on the watch, then go back to 1 (a path replaced or removed is
///    watched again there).
///
/// `rush_sys::fswait::follow` does exactly this; use it rather than a loop
/// of your own. A follower notices a rewrite by the file's identity as
/// well as its size ([`rewritten`]): a transcript renamed over by an
/// equal or longer one starts over too.
pub trait Follow: Send {
    /// What's been written since the last call, read from at most `most`
    /// bytes of it. A line still being written waits for the next call.
    fn next(&mut self, most: usize) -> Result<Slice>;
    /// What changes when there's more to read.
    fn wait_on(&self) -> WaitOn;
}

/// One slice of a history read as it grows.
#[derive(Debug, Default)]
pub struct Slice {
    pub events: Vec<Event>,
    /// The history was rewritten: what came before is void, and events
    /// start it over.
    pub reset: bool,
    /// More is written already: call again without waiting.
    pub more: bool,
}

/// What a follower's caller waits on before reading again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitOn {
    /// Files (and folders) that change when there's more: the transcript.
    pub paths: Vec<PathBuf>,
    /// A descriptor that turns readable when there's more, for a history
    /// that isn't in a file (a pipe, a socket): kqueue can wait on it.
    pub fd: Option<i32>,
    /// Neither can be waited on (a remote session's log): how long to wait
    /// before asking again.
    pub every: Option<std::time::Duration>,
}

/// Which file a path names now: its device and inode, which tell a file
/// replaced (renamed over, atomically) from the one read before, whatever
/// its size. None where the platform can't say.
pub fn file_id(m: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        None
    }
}

/// Whether a file followed `off` bytes in, last seen as `seen`, has been
/// rewritten since: it's shorter than what was read, or the path names
/// another file now (one renamed over it, however long). Notes in `seen`
/// the file `m` is, for next time. A follower calls it with the metadata
/// of the file it has open, before reading.
pub fn rewritten(seen: &mut Option<(u64, u64)>, off: u64, m: &std::fs::Metadata) -> bool {
    let id = file_id(m);
    let other = seen.is_some() && id.is_some() && *seen != id;
    *seen = id;
    other || m.len() < off
}

/// Reads only a session's end, from the first whole line of its
/// transcript's last most bytes, for a long one to show at once; the bool
/// is whether that left some out.
pub trait TailReader {
    fn history_tail(&self, s: &Session, most: i64) -> Result<(Vec<Event>, bool)>;
}

/// Reads a bounded tail ending before a time. It keeps history's cutoff
/// without decoding the whole start.
pub trait HistoryTailBeforeReader {
    fn history_tail_before(&self, s: &Session, most: i64, before: Ms)
    -> Result<(Vec<Event>, bool)>;
}

/// Reads an account's limits. p is the profile to read them through, for
/// agents whose sign-in lives in the profile.
pub trait QuotaSource {
    fn quota(&self, p: &Profile, a: &Account) -> Result<usage::Quota>;
}

/// An agent whose accounts earn limit resets: spends one (id, or the next
/// when ""), on the account profile p is signed in to, and says what came
/// of it.
pub trait ResetSpender {
    fn use_reset(&self, p: &Profile, id: &str) -> Result<String>;
}

/// Reads the plans of an agent's accounts, sharing each reading with every
/// rush process through a file.
pub trait PlanReader {
    /// Who p is signed in as, and its limits, as the agent itself last saw
    /// them. It reads the disk only when that record changed.
    fn plan(&self, p: &Profile) -> usage::Reading;
    /// The readings shared through path, by key.
    fn readings(&self, path: &str) -> BTreeMap<String, usage::Reading>;
    /// p's reading, asked of the provider unless one shared through path is
    /// fresh enough; offline never asks.
    fn refresh_plan(&self, path: &str, p: &Profile, offline: bool) -> usage::Reading;
}

/// What a terminal sign-in leaves to do once its command has run: keep the
/// credential and say whose it is.
pub type SignInDone = Box<dyn FnOnce() -> Result<Account> + Send>;

/// Signs an agent's home in to one of several accounts. Which accounts
/// there are, and what they're called, is rush's to keep; the adapter keeps
/// their credentials.
pub trait Accounts {
    /// Who p is signed in as.
    fn current(&self, p: &Profile) -> Result<Account>;
    /// Signs p in as a, keeping the credential p holds now first.
    fn switch(&self, p: &Profile, a: &Account) -> Result<()>;
    /// Signs in to an account in the terminal, apart from p: the command
    /// runs, then done keeps the credential and says whose it is.
    fn sign_in(&self, p: &Profile) -> Result<(Cmd, SignInDone)>;
    /// Drops the credential rush keeps for a.
    fn forget(&self, a: &Account) -> Result<()>;
}

/// An agent whose profile says which account it's signed in as: the
/// account's own id, empty when none. It reads the disk.
pub trait SignInReader {
    fn signed_in_as(&self, p: &Profile) -> String;
}

/// An Accounts whose agent keeps a list of its own sign-ins (Copilot's are
/// gh's): rush lists them as they are.
pub trait Known {
    fn known(&self) -> Vec<Account>;
}

/// Prices a model's tokens.
pub trait Pricer {
    fn cost(&self, model: &str, u: &TokenUsage) -> Option<f64>;
}

/// A Pricer whose requests can run in a faster mode, at prices of their
/// own.
pub trait FastPricer {
    fn cost_fast(&self, model: &str, u: &TokenUsage) -> Option<f64>;
}

/// An agent that says which model its sessions run when none is picked, to
/// price tokens whose model isn't known.
pub trait DefaultModeler {
    fn default_model(&self) -> String;
}

/// Lists the slash commands and skills a session can run.
pub trait Commander {
    fn commands(&self, p: &Profile, cwd: &str) -> Vec<Command>;
}

/// An agent that can read a Preview from the last window bytes of a
/// session's transcript at path.
pub trait Previewer {
    fn preview(&self, path: &str, window: i64) -> Preview;
}

/// An agent whose sessions have names for each other, the ones their
/// messages to one another use ("rush-8a"), by session id.
pub trait Codenamer {
    fn codenames(&self) -> HashMap<String, String>;
}

/// Prices transcripts, reading each only as far as it grew, and keeps what
/// it read in a cache of its own. run and flush may be called from any
/// thread.
pub trait SpendScanner: Send + Sync {
    /// Prices every target whose files grew, by key.
    fn run(&self, targets: &[SpendTarget]) -> HashMap<String, Spend>;
    /// Saves the cache, unless a run is under way.
    fn flush(&self);
}

/// An agent whose transcripts rush prices itself.
pub trait SpendReader {
    fn spend_scanner(&self) -> Arc<dyn SpendScanner>;
}

/// An adapter that says what a new session can be started with: the
/// models, efforts and permission modes it takes (StartOptions' model,
/// effort and mode). Settings offers them for that agent; an adapter
/// without one takes a typed model, and its own default for the rest.
pub trait Chooser {
    fn choices(&self) -> Choices;
}

/// An adapter that reads the models its account offers now from profile
/// p's home: Codex's, which come and go with the account. It reads the
/// disk, so never on the UI.
pub trait ModelLister {
    fn list_models(&self, p: &Profile) -> Vec<Choice>;
}

/// An adapter whose skills another agent can read: the folders of
/// <name>/SKILL.md skills it has in cwd. It reads the disk.
pub trait SkillRooter {
    fn skill_roots(&self, p: &Profile, cwd: &str) -> Vec<String>;
}

/// Gives runner metadata found before, without launching a CLI or asking a
/// provider. Starting a session must not wait on finding an unrelated
/// harness; Settings refreshes the catalog.
pub trait RunnerSnapshot {
    /// The models, and whether they're ready.
    fn runner_snapshot(&self) -> (Vec<Choice>, bool);
}

/// Finds saved history without promising rewind or fork.
pub trait TranscriptLocator {
    fn transcript_path(&self, p: &Profile, cwd: &str, sid: &str) -> String;
}

/// An agent whose conversation rush can copy for a fork that remembers
/// only part of it, or that runs in another folder: the copy is resumed as
/// a session of its own.
pub trait Brancher {
    /// Where a session of p's in cwd keeps its conversation.
    fn transcript_path(&self, p: &Profile, cwd: &str, sid: &str) -> String;
    /// Copies the conversation at src, up to byte up_to (-1 for all of it),
    /// as session new_id's in cwd, with what it needs to rewind.
    fn branch(
        &self,
        p: &Profile,
        src: &str,
        cwd: &str,
        sid: &str,
        new_id: &str,
        up_to: i64,
    ) -> Result<()>;
}

/// An agent that keeps each session's conversation in a file under a
/// folder of the profile's.
pub trait Transcripts {
    /// Where session sid, started in cwd, keeps its conversation now:
    /// entering a worktree can move it.
    fn find_transcript(&self, p: &Profile, cwd: &str, sid: &str) -> String;
    /// The folder they're all under.
    fn transcripts_dir(&self, p: &Profile) -> String;
}

/// An agent with names of its own for its model ids.
pub trait ModelNamer {
    /// id as people say it: "claude-opus-5-5" is "Opus 5.5".
    fn model_name(&self, id: &str) -> String;
}

/// An adapter whose agent can credit itself in the commits and pull
/// requests it makes (a Co-Authored-By trailer, a "Generated with" line),
/// and be told not to.
pub trait Attributor {
    /// Whether p's agent credits itself (on, set); set is false when its
    /// settings don't say, and it does as it does by default.
    fn attribution(&self, p: &Profile) -> (bool, bool);
    fn set_attribution(&self, p: &Profile, on: bool) -> Result<()>;
}

/// An adapter with words of its own for what its tools do: a call in a few
/// words ("reading view.go"). Without one, the call's words are
/// tool::doing's (Go's tool/doing.go, ported beside the Claude adapter).
pub trait Describer {
    fn doing(&self, c: &tool::Call) -> String;
}

/// An agent whose session can move to another folder as it runs, into a
/// worktree, and says where: the session its process pid, of profile p,
/// runs, and the folder it works in now. It reads the disk.
pub trait CwdReader {
    /// (session id, cwd), when it can say.
    fn session_cwd(&self, p: &Profile, pid: i32) -> Option<(String, String)>;
}

/// Offers the harness's own terminal login or setup, without implying rush
/// can keep, switch or delete several accounts for it. The caller runs the
/// command with the terminal and checks the key again.
pub trait Authenticator {
    fn sign_in_command(&self, p: &Profile) -> Result<Cmd>;
}

/// Says how to sign the harness in again from a terminal, for a run whose
/// login stopped working: "run `claude auth login` in a terminal".
pub trait LoginHinter {
    fn login_hint(&self) -> String;
}

/// Reads the agent's own account information, off the UI thread. It
/// implies no account switching or credential keeping.
pub trait AccountSummaryReader {
    fn account_summary(&self, p: &Profile) -> AccountSummary;
}

/// An adapter whose agent is a program on this machine: name is what it's
/// called, dirs are where its installer puts it besides PATH (under the
/// home folder when relative: ".kimi-code/bin").
pub trait Programmer {
    fn program(&self) -> (String, Vec<String>);
}

/// A Programmer whose agent is there, doing less, when only another
/// program is: Copilot's coding agent needs only gh. hint says what
/// installing the agent's own program adds.
pub trait Lesser {
    /// (name, dirs, hint)
    fn lesser(&self) -> (String, Vec<String>, String);
}

/// An adapter that runs on another agent's program: Ollama's runs Claude
/// Code. Its own Program is only what else it needs, so it's installed
/// when both are, and a shell running that program isn't it.
pub trait Rider {
    fn rides(&self) -> Kind;
}

/// A Rider that names the provider it runs, when that isn't its own kind:
/// Ollama in Pi is kind "ollama-pi", provider "ollama".
pub trait Provided {
    fn provider(&self) -> String;
}

/// An agent that names the company behind the models it runs, when that
/// isn't its own name: Claude Code's are Anthropic's.
pub trait Made {
    fn maker(&self) -> String;
}

/// A provider that can be paid for per token with an API key, apart from
/// any subscription: the variable its own programs read the key from.
pub trait Keyed {
    fn key_env(&self) -> String;
}

/// A rider that runs its provider on the plan its harness is signed in to,
/// rather than with the provider's API key: Pi on your Anthropic plan,
/// through Pi's own /login.
pub trait PlanRider {
    fn on_plan(&self) -> bool;
}

/// A harness that speaks one provider's API alone, said for the routes it
/// can't run: "Anthropic's API".
pub trait Speaker {
    fn speaks(&self) -> String;
}

/// An adapter with other words to type it by, beside its kind and its
/// name: cc for Claude Code.
pub trait Worded {
    fn words(&self) -> Vec<String>;
}

/// A legacy harness kept for saved sessions. Choosing a new session uses
/// its replacement, without rewriting old conversations' kinds.
pub trait Replaced {
    fn replacement(&self) -> Kind;
}

/// Whether a found session fits what's looked for.
pub type Fits<'a> = &'a dyn Fn(&Session) -> bool;

/// An adapter that finds a session one of its programs began from
/// another's shell itself, faster than listing every session: one begun in
/// dir after start that fits, looked for in profiles.
pub trait SpawnFinder {
    fn find_spawn(&self, profiles: &[Profile], dir: &str, start: Ms, fits: Fits)
    -> Option<Session>;
}

/// An adapter whose subagents keep sessions of their own, apart from the
/// one that started them (Codex's spawn_agent threads).
pub trait ChildFinder {
    /// The run session parent started as child (its call's input.child),
    /// begun after start, in p.
    fn find_child(&self, p: &Profile, parent: &str, child: &str, start: Ms) -> Option<Session>;
}

/// An adapter whose program a session's shell can run as a run rush hosts:
/// how the run is written, with a "<task>" for its prompt, and the flag
/// that picks its model (a VAR= when a variable set before the command
/// does).
pub trait Spawnable {
    /// (cmd, model flag)
    fn spawn_command(&self) -> (String, String);
}

/// A Spawnable that reads its program's one-shot runs itself: what args ask
/// for; None when rush can't host the run and print it as the program
/// would.
pub trait OnceReader {
    fn read_once(&self, args: &[String]) -> Option<Once>;
}

/// An agent that runs only on an API key its own program keeps where rush
/// can't see it (Vibe's, in the keychain or its .env): says why profile p
/// can't run yet, Ok once it can. It reads the keychain: never on the UI
/// thread.
pub trait KeyChecker {
    fn check_key(&self, p: &Profile) -> Result<()>;
}

/// An agent that knows what its models read besides text.
pub trait MediaReader {
    /// What model takes; None when the agent doesn't know it.
    fn reads(&self, model: &str) -> Option<Media>;
}

/// An agent that knows its models' context windows.
pub trait ContextWindower {
    fn context_window(&self, model: &str) -> i64;
}

/// An agent whose sessions compact within a window their settings and
/// environment can make smaller than the model's.
pub trait Compacter {
    /// Where a session on p in cwd, whose process has env (NAME=value),
    /// compacts.
    fn compaction(&self, p: &Profile, cwd: &str, env: &[String]) -> Compaction;
}

/// An agent that keeps a folder of its own in the projects it works in:
/// Claude Code's .claude, Codex's .codex.
pub trait ProjectFolderer {
    fn project_folder(&self) -> String;
}

/// An adapter whose program says where it's published.
pub trait Publisher {
    fn published(&self) -> Published;
}

/// An agent whose last reading of a profile's limits rush can read at
/// once, without asking anyone: for a status line drawn many times a
/// second.
pub trait LastQuotaReader {
    /// p's limits as last read, with the windows that have reset since
    /// emptied; None if there's no reading.
    fn last_quota(&self, p: &Profile) -> Option<usage::Quota>;
}

/// An agent that answers one prompt headless, read-only and in a JSON
/// shape, keeping no session: what to run for q, and how to read what it
/// printed. The caller runs the program and owns its process.
pub trait Querier {
    /// (args, env)
    fn query_command(&self, p: &Profile, q: &Query) -> (Vec<String>, Vec<String>);
    /// Reads what it printed; None when that can't be read at all, so what
    /// it cost isn't known.
    fn query_answer(&self, stdout: &[u8]) -> Option<Answer>;
    /// Its quick, cheap model and its careful one.
    fn query_models(&self) -> (String, String);
}

/// An agent that can take a conversation back: put back the files it
/// changed, and first say what it learned. Copying the conversation to go
/// back to is Brancher's.
pub trait Rewinder {
    /// Puts the files session sid (p's, in cwd, on model) changed back as
    /// they were just before its message msg_id; dry_run only says what
    /// that would change.
    fn rewind_files(
        &self,
        p: &Profile,
        cwd: &str,
        sid: &str,
        model: &str,
        msg_id: &str,
        dry_run: bool,
    ) -> Result<FileRewind>;
    /// Asks session sid what it learned since its message from.
    fn recap(&self, p: &Profile, cwd: &str, sid: &str, model: &str, from: &str) -> Result<String>;
}

/// Writes a summary with one of its models: how a session is compacted by a
/// model other than its own, a cheaper or a local one.
pub trait Summarizer {
    /// The models it can summarise with, best first.
    fn summary_models(&self) -> Result<Vec<String>>;
    fn summarize(&self, model: &str, system: &str, text: &str) -> Result<String>;
}

/// An agent that keeps a record of a profile's use.
pub trait StatsReader {
    fn stats(&self, p: &Profile) -> Result<Stats>;
}

/// An agent whose settings are JSON files rush edits in place, read in
/// order: a profile's own, then a project's.
pub trait SettingsFiler {
    fn settings_files(&self, p: &Profile, cwd: &str) -> Vec<SettingsFile>;
}

/// An agent that runs a command of yours to draw a line under its prompt.
pub trait StatusLiner {
    /// p's command now; "" is none.
    fn status_line(&self, p: &Profile) -> Result<String>;
    /// Makes command p's; "" takes it away.
    fn set_status_line(&self, p: &Profile, command: &str) -> Result<()>;
}

/// A StatusLiner whose command runs with the profile it's for named in its
/// environment: this reads it, else it's the agent's home.
pub trait StatusLineProfiler {
    fn status_line_profile(&self) -> Profile;
}

/// An agent with a SettingsPage.
pub trait SettingsPager {
    fn settings_page(&self) -> SettingsPage;
}

/// An agent whose own agent, the one that runs when no definition is
/// picked, has a name among its definitions.
pub trait BuiltinDefNamer {
    fn builtin_def(&self) -> String;
}

/// An agent whose sessions can start as a definition of yours.
pub trait Definer {
    /// p's definitions, the built-in one first, then those of every project
    /// under roots.
    fn agent_defs(&self, p: &Profile, roots: &[String]) -> Vec<AgentDef>;
    /// Makes a definition of p's called name, from a template if there
    /// isn't one, and says where it is.
    fn new_agent_def(&self, p: &Profile, name: &str) -> Result<String>;
}

/// An agent with plugins, from marketplaces: Feature::Plugins. dir is the
/// folder whose project plugins count.
pub trait Plugger {
    /// (installed, available)
    fn plugins(&self, p: &Profile, dir: &str) -> Result<(Vec<Plugin>, Vec<Plugin>)>;
    fn marketplaces(&self, p: &Profile, dir: &str) -> Result<Vec<Marketplace>>;
    /// Roughly what a plugin adds to every session: "~1.2k tok".
    fn plugin_cost(&self, p: &Profile, dir: &str, id: &str) -> Result<String>;
    fn change_plugins(&self, p: &Profile, dir: &str, c: &PluginChange) -> Result<()>;
}

/// An agent that says what it reads for a session of p's in cwd, whose
/// transcript (if it has one yet) is at transcript.
pub trait MemoryReader {
    fn memory(&self, p: &Profile, cwd: &str, transcript: &str) -> Memory;
    /// Deletes one of its files, and whatever points to it.
    fn forget_file(&self, path: &str) -> Result<()>;
}

/// An agent whose memory notes rush can write and tidy (memory.go:82).
pub trait MemoryKeeper {
    /// Writes a note into the memory of p's sessions in cwd, and says
    /// where.
    fn remember(&self, p: &Profile, cwd: &str, n: &MemoryNote) -> Result<String>;
    /// Fixes what needs no judgement in the memory folder dir, and says
    /// what it did.
    fn tidy_memory(&self, dir: &str) -> Result<Vec<String>>;
    /// Puts the note at drop into keep, and deletes drop.
    fn merge_notes(&self, keep: &str, drop: &str) -> Result<()>;
    /// Moves a project's note into what every project of p's reads.
    fn make_global(&self, p: &Profile, path: &str) -> Result<()>;
    /// Reads a note: its name, description, type and text.
    fn read_note(&self, path: &str) -> Result<MemoryNote>;
    /// Fills in how often sessions opened each of mem's notes lately: it
    /// reads transcripts, so it's asked for only where shown.
    fn count_opens(&self, p: &Profile, mem: &mut Memory);
}

/// An agent whose sessions rush can follow as a timeline: what a session
/// and its subagents have done, in time order.
pub trait Timeliner {
    fn new_timeline(&self) -> Box<dyn Timeline>;
}

/// Follows one session's transcript, reading only what it gains.
pub trait Timeline: Send {
    /// Reads what the transcript at path, and its subagents', have gained;
    /// subagents last written before since aren't read.
    fn update(&mut self, path: &str, since: Ms);
    /// Those since since, oldest first.
    fn events(&self, since: Ms) -> Vec<Happening>;
}

/// Follows a session's transcript, and its subagent runs' own, reading only
/// what each has gained since.
pub trait SubagentRuns: Send {
    /// Says the session's own process is known to have exited: a run the
    /// transcripts left unfinished ended with it. gone says so.
    fn set_gone(&mut self, gone: bool);
    fn gone(&self) -> bool;
    /// Reads what the session's transcript at path, and those of its runs
    /// (as listed from their meta files), have gained.
    fn update_runs(&mut self, path: &str, runs: &[SubagentRun]);
    /// Reads line, the next of the transcript at path, as read by another
    /// from its start: update_runs goes on from after it.
    fn took(&mut self, path: &str, line: &[u8]);
    /// Counts the session's runs, and those still working.
    fn stats(&mut self, path: &str, now: Ms) -> SubagentStats;
    /// The runs stats last found still working.
    fn running(&self) -> Vec<SubagentRun>;
    /// How the transcripts say run id, of call tool_use_id, stands: how it
    /// ended, and when, once it has.
    fn state(&self, id: &str, tool_use_id: &str) -> (RunState, String, Ms);
    /// Whether run id is still working, last written at mod_time, and how
    /// it ended if it has.
    fn going(&self, id: &str, tool_use_id: &str, mod_time: Ms, now: Ms) -> (bool, String);
    /// A copy of what's been read, to read apart from it.
    fn clone_runs(&self) -> Box<dyn SubagentRuns>;
}

/// An agent whose sessions' subagent runs rush follows from their
/// transcripts.
pub trait RunFollower {
    /// A follower of one session's runs; None while the adapter only
    /// counts them.
    fn subagent_runs(&self) -> Option<Box<dyn SubagentRuns>> {
        None
    }
    /// The folder a session's runs keep their transcripts in, beside its
    /// own at transcript.
    fn subagents_dir(&self, transcript: &str) -> String;
    /// Counts a session's runs from what each says of itself, without
    /// reading their transcripts: those still writing are working.
    fn count_subagents(&self, transcript: &str, now: Ms) -> SubagentStats;
}

/// An agent whose background jobs a service of its own runs (Claude Code's
/// daemon), and that keeps more of them than each job's own record. Each
/// is read again only when its file changes.
pub trait JobKeeper {
    /// The processes running p's jobs, by job id.
    fn workers(&self, p: &Profile) -> HashMap<String, i32>;
    /// The jobs pinned in the agent's own list, by job id, each its place
    /// from 1.
    fn pins(&self, p: &Profile) -> HashMap<String, i64>;
    /// The pull requests the agent linked to p's sessions, by URL.
    fn linked_prs(&self, p: &Profile) -> HashMap<String, Pr>;
    /// The files and folders p's sessions are found in, and those of each
    /// of jobs.
    fn watched(&self, p: &Profile, jobs: &[String]) -> Vec<PathBuf>;
}

/// An agent that leaves scratch work of its own behind: what its sessions
/// cloned, built or downloaded.
pub trait Scratcher {
    /// Session sid's folders, for one run in cwd, and job's own when it
    /// runs as the background job with that id.
    fn scratch(&self, p: &Profile, job: &str, sid: &str, cwd: &str) -> Vec<TempDir>;
    /// The folder a session's scratch folder is two below.
    fn scratch_root(&self) -> String;
}

/// A session's own screen, run in rush's terminal until you leave it.
pub trait Terminal: Send {
    fn run(&mut self) -> Result<()>;
    fn set_stdin(&mut self, r: Box<dyn Read + Send>);
    fn set_stdout(&mut self, w: Box<dyn Write + Send>);
    fn set_stderr(&mut self, w: Box<dyn Write + Send>);
}

/// An agent whose background sessions' own screens rush opens.
pub trait Joiner {
    /// Session id's screen in rush's terminal. Its run fails with Gone for
    /// a session the agent has let go, and Elsewhere once it was opened in
    /// another window.
    fn join(&self, p: &Profile, id: &str) -> Box<dyn Terminal>;
    /// Whether p's background service runs. It asks the service, so never
    /// on the UI.
    fn service_up(&self, p: &Profile) -> bool;
    /// Attaches to session id as viewer, cols by rows.
    fn mirror(&self, p: &Profile, id: &str, viewer: &str, cols: u16, rows: u16) -> Result<Mirror>;
    /// Resizes viewer's attachment to session id.
    fn resize(&self, p: &Profile, id: &str, viewer: &str, cols: u16, rows: u16) -> Result<()>;
}

// What rush does to a session or an account that runs outside rush mode,
// through the agent's own program (Go's actions.go).

/// Starts a session in the agent's own background service, and returns its
/// id.
pub trait Dispatcher {
    fn dispatch(&self, p: &Profile, dir: &str, prompt: &str, flags: &[String]) -> Result<String>;
}

/// Ends a session running outside rush and keeps its conversation. pid,
/// when known (not 0), is ended if the agent's own way doesn't.
pub trait Stopper {
    fn stop(&self, p: &Profile, id: &str, pid: i32) -> Result<()>;
}

/// Deletes a session the agent keeps, and what it made for it.
pub trait Remover {
    fn remove(&self, p: &Profile, id: &str) -> Result<()>;
}

/// Sends a message to a session in the agent's background service. A
/// session it has let go of is Gone.
pub trait Replier {
    fn reply(&self, p: &Profile, id: &str, text: &str) -> Result<()>;
}

/// Opens a background session in the agent's own terminal UI.
pub trait Attacher {
    fn attach(&self, p: &Profile, id: &str) -> Cmd;
}

/// Keeps sessions pinned in the agent's own list of them.
pub trait Pinner {
    /// Pins session id, or unpins it.
    fn toggle_pin(&self, p: &Profile, id: &str) -> Result<()>;
}

/// Opens the agent's own list of its sessions in the terminal. Making the
/// command looks for its program: not on the UI.
pub trait SessionsViewer {
    fn sessions_view(&self, p: &Profile) -> Cmd;
}

/// Signs p in, in the terminal.
pub trait Loginer {
    fn login(&self, p: &Profile) -> Cmd;
}

/// Opens one of the agent's own screens in the terminal: a fresh program on
/// a slash command, in dir, with hint printed above it.
pub trait Screener {
    fn screen(&self, p: &Profile, dir: &str, command: &str, hint: &str) -> Cmd;
}

/// Moves a session running outside rush to another profile or folder,
/// keeping its conversation, and returns its new id.
pub trait Mover {
    fn move_session(&self, m: &Move) -> Result<String>;
}

/// The agent whose own words rush still reads apart from its Driver:
/// its transcripts keep them, and a conversation's steps keep every call
/// in its tools' names and inputs. The conversation reads them through it,
/// never knowing their shape. One adapter at most is.
pub trait Native {
    /// Starts reading one transcript's messages as rush's events: what the
    /// conversation's transcript reader decodes each message line with.
    fn messages(&self) -> Box<dyn MessageDecoder>;
    /// What the agent's tool of this name does.
    fn kind_of(&self, name: &str) -> tool::Kind;
    /// A call of the agent's tool, in its words, as rush's own.
    fn call(&self, id: &str, name: &str, input: &RawValue) -> tool::Call;
    /// How call c came out, from the text and the result the agent gave
    /// back in its words.
    fn output(
        &self,
        c: &tool::Call,
        text: &str,
        is_error: bool,
        result: Option<&RawValue>,
    ) -> tool::Output;
}

/// The host's half of Go's Native: the lines a running session of the
/// native agent writes (hosts an older rush started send them as written),
/// read as rush's events. Apart from Native so an adapter can read its
/// transcripts before its live stream is ported.
pub trait NativeStream {
    /// Reads one line the agent wrote as its own event.
    fn line(&self, line: &[u8]) -> Result<Box<dyn Any + Send>>;
    /// Reads a message a transcript keeps (its type, the message and the
    /// result a tool gave) as the agent's own event.
    fn message(
        &self,
        typ: &str,
        message: &RawValue,
        result: Option<&RawValue>,
    ) -> Result<Box<dyn Any + Send>>;
    /// Starts reading one session's own events as rush's.
    fn neutral(&self) -> Box<dyn Neutral>;
}

/// Reads one session's own events, as its Native read them, as rush's: none
/// for what only the host needs, more than one where the agent says two
/// things at once. None for a value that isn't one of the agent's own
/// events.
pub trait Neutral: Send {
    fn event(&mut self, ev: &dyn Any) -> Option<Vec<Event>>;
}

/// Reads the messages a transcript keeps, in the agent's words, as rush's
/// events: one per transcript, fed its lines in order, since a tool result
/// is read knowing the call it answers. The conversation's transcript
/// reader (rush_convo::Tail) reads each user or assistant line through
/// one; what comes out is what a live session sends (event::Message, its
/// tool calls and results as tool::Call and tool::Output), so a session
/// read back draws as one hosted does.
pub trait MessageDecoder: Send {
    /// typ is the line's type (user, assistant), message its message
    /// object, result what the tool gave back in the agent's words
    /// (Claude Code's toolUseResult). Empty when it isn't a message.
    fn message(&mut self, typ: &str, message: &RawValue, result: Option<&RawValue>) -> Vec<Event>;
}
