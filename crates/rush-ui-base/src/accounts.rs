//! What Accounts knows beyond the list, as every screen reads it: rush's
//! config (the store, Settings' and the accounts' one copy), each login
//! with its plan's reading, every other account's limits, who each agent
//! is signed in as, and what the next session starts as. Kept with
//! `Model::kept`; read off the UI thread and handed back
//! (screens/accounts/state.rs reads it after each reading of the list).
//! After Go's internal/ui/accounts.go (accountsState :24, acctRow :76),
//! fleet/logins.go (LoginView :14, logins :25), claude/homes.go:40
//! (Using), agentquota.go:72 (quotaMsg.applyTo), providers.go (startOver,
//! setupLabel) and askrush.go:15 (askRush's start).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use rush_core::adapter::{Account, AccountSummary, Choice, Profile, Registry};
use rush_core::text::Color;
use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Reading};
use rush_state::profiles::BILLING_KEY;
use rush_state::{Config, Login, Store};

use crate::frame::Out;
use crate::look::agent_name;
use crate::model::Model;

/// A provider id (claude, claude-key, ollama) and the agents here that
/// run it (route).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub id: String,
    pub kinds: Vec<String>,
}

/// How agent k is paid for as provider id: by key when id is a key, or k
/// runs only on one (billingOf).
pub fn billing_of(reg: &Registry, id: &str, k: &str) -> String {
    if reg.billed(id).1 || reg.key_only(k) {
        BILLING_KEY.into()
    } else {
        String::new()
    }
}

/// What a session starts as, over what its folder's profile and Settings
/// say (startOver).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartOver {
    pub kind: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub mode_set: bool,
    /// BILLING_KEY pays with the provider's API key; "" as it's signed in.
    pub billing: String,
    /// The account's name; "" is the one in use.
    pub account: String,
    /// The profile of yours it is; "" none.
    pub profile: String,
}

/// A setup as it's shown: OpenAI (Codex) · alex, Anthropic (Pi) · API key
/// (setupLabel).
pub fn setup_label(reg: &Registry, k: &str, billing: &str, account: &str) -> String {
    if billing == BILLING_KEY || reg.key_only(k) {
        return format!("{} · API key", reg.label(k));
    }
    if !account.is_empty() && reg.harness_of(k).as_str() == k {
        return format!("{} · {account}", reg.label(k));
    }
    reg.label(k)
}

/// A session rush hosts, started for rush's own ends (#ask): what the
/// screens that host sessions are handed (startHosted with its config).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostedStart {
    /// The first message; "" starts it waiting.
    pub task: String,
    pub dir: String,
    /// Its name; "" lets the task name it.
    pub name: String,
    /// In a worktree of its own (false in rush's folder: no project's
    /// work goes on there).
    pub own_worktree: bool,
    pub system_prompt: String,
}

/// Starts a HostedStart: set by the screens that host sessions.
pub type HostedStarter = fn(&mut Model, HostedStart) -> Out;

/// What's done with Claude Code's logins through rush's vault: set once
/// the vault is ported (switchLogin, renewLogin, addLogin, and forgetting
/// a login's keychain entry).
#[derive(Clone, Copy)]
pub struct LoginOps {
    /// Makes to the login new sessions run as; with move, idle sessions
    /// follow at once.
    pub switch: fn(&mut Model, Login, &str, bool) -> Out,
    /// switch, then the last argument once the login is written, whether
    /// it went or not (accountSwitch in a tea.Sequence).
    pub switch_then: fn(&mut Model, Login, &str, bool, AfterSwitch) -> Out,
    pub renew: fn(&mut Model, &LoginView) -> Out,
    /// Signs in to a login named so.
    pub add: fn(&mut Model, &str) -> Out,
    pub forget: fn(&mut Model, &str),
}

/// What runs once a switch is done.
pub type AfterSwitch = Box<dyn FnOnce(&mut Model) -> Out + Send>;

/// Whether a rush session on the login folder named root was stopped by a
/// usage limit, under a profile that moves on from it; set by the screens
/// that host sessions.
pub type LimitStopped = fn(&Model, &str) -> bool;
/// Tells every rush session in the folder named root that it runs as
/// another login now, carrying on those a limit stopped (reloginHosts);
/// set by the screens that host sessions.
pub type Relogin = fn(&mut Model, &str) -> Out;

/// An agent, or one of its accounts (acctRow).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AcctRow {
    pub kind: String,
    /// The agent's own line.
    pub head: bool,
    /// One of Claude Code's logins.
    pub login: Option<LoginView>,
    /// Another agent's account.
    pub acct: Account,
    pub current: bool,
    pub q: Quota,
}

impl AcctRow {
    pub fn name(&self, m: &Model) -> String {
        if self.head {
            return agent_name(&m.reg, &self.kind);
        }
        match &self.login {
            Some(l) => l.name.clone(),
            None => self.acct.name.clone(),
        }
    }

    pub fn email(&self) -> String {
        match &self.login {
            Some(l) => l.email.clone(),
            None if !self.acct.email.is_empty() => self.acct.email.clone(),
            None => self.q.email.clone(),
        }
    }

    /// The account's own id: a login's uuid, or another agent's.
    pub fn id(&self) -> &str {
        match &self.login {
            Some(l) => &l.id,
            None => &self.acct.id,
        }
    }

    /// The account as rush names it: harness:account, or the harness alone
    /// for one with a single sign-in of its own (label).
    pub fn label(&self, m: &Model) -> String {
        if self.head {
            return setup_label(&m.reg, &self.kind, "", "");
        }
        setup_label(&m.reg, &self.kind, "", &self.name(m))
    }
}

/// One Claude Code login with its plan's usage (fleet.LoginView).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoginView {
    pub login: Login,
    /// Who it is and its plan, as last read.
    pub usage: Reading,
    /// The plan's limits, as usage read them.
    pub quota: Quota,
    /// The one new sessions run as.
    pub current: bool,
}

impl std::ops::Deref for LoginView {
    type Target = Login;
    fn deref(&self) -> &Login {
        &self.login
    }
}

/// Why a login's sign-in couldn't be refreshed, and when; why is "" while
/// it's being refreshed (renewNote).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenewNote {
    pub why: String,
    pub at: Ms,
}

/// Starts a session on o with msg in dir: the host's, once it's ported
/// (startHosted). None says rush can't start one here yet.
pub type Starter = fn(&mut Model, &StartOver, &str, &str) -> Out;

/// Switches session key to o, then sends message when there is one: in
/// place on the same agent paid the same way, else handed over
/// (switchSessionMessage). Set by the screens that host sessions.
pub type Switcher = fn(&mut Model, &str, StartOver, &str) -> Out;

/// Accounts' own state.
#[derive(Default)]
pub struct Accounts {
    /// rush's config.json and state.json, as loaded (Go's m.store).
    pub store: Option<Store>,
    /// Claude Code's logins, with their readings (snap.Logins).
    pub logins: Vec<LoginView>,
    /// Every other account's limits as last read, by key (m.quotas).
    pub quotas: HashMap<String, Quota>,
    /// What each agent says of its own account, by kind.
    pub summary: HashMap<String, AccountSummary>,
    /// The account each agent other than Claude Code is signed in as, by
    /// kind: its id.
    pub now: HashMap<String, String>,
    /// Why who an agent is signed in as isn't known, by kind.
    pub why: HashMap<String, String>,
    /// When rush last switched an agent's account itself.
    pub switched_at: HashMap<String, Ms>,
    /// The profile picked for the next session started from the box
    /// (#preset): it goes once used.
    pub profile: String,
    /// By login id.
    pub renew: HashMap<String, RenewNote>,
    /// The plan readings this rush fetched itself, by reading key, with
    /// why one couldn't be refreshed (the loader's fetched, SetFetched):
    /// a failed refresh keeps the last numbers and says why, which
    /// usage.json doesn't keep.
    pub fetched: HashMap<String, Reading>,
    /// Every other account's limits as this rush last read them, why one
    /// failed included, by key (quotaMsg in m.quotas): in place of
    /// quotas.json's where they're no older.
    pub quotas_read: HashMap<String, Quota>,
    /// What the next session starts as, picked on the sheet (m.startOver).
    pub start_over: Option<StartOver>,
    /// The models each agent offers as read from its home, by kind
    /// (m.listed).
    pub listed: HashMap<String, Vec<Choice>>,
    /// Starts sessions: set by the screens that host them.
    pub starter: Option<Starter>,
    /// Starts a session for rush's own ends (#ask): set by the screens
    /// that host them.
    pub hosted_starter: Option<HostedStarter>,
    /// config.json is watched for changes made elsewhere.
    #[doc(hidden)]
    pub reloading: bool,
    /// The logins and limits have been read from disk once (Rust-only:
    /// till then the top bar draws what it said last time, listcache.rs).
    pub read_once: bool,
    /// Switches sessions: set by the screens that host them.
    pub switcher: Option<Switcher>,
    /// Hands a session's conversation over to a new one that starts as
    /// the setup, the one it came from stopped once the new one runs
    /// (handOverMessage): set by the screens that host sessions.
    pub hand_over: Option<Switcher>,
    /// What a hosted session runs as now, by its key (sessionStart): set
    /// by the screens that host sessions.
    pub session_start: Option<fn(&Model, &str) -> Option<StartOver>>,
    /// The list's agents newest first, by key (newest).
    pub newest: Vec<String>,
    /// Where each of newest was in the list when sorted: where to look
    /// first for it (recent_setups).
    pub newest_at: Vec<usize>,
    /// Claude Code's logins through rush's vault, once it's ported.
    pub login_ops: Option<LoginOps>,
    /// A view another app hosts: Getting started and the tips are off
    /// (m.onboard, false in NewHosted).
    pub hosted: bool,
    /// A switch of the login in use is under way: set by LoginOps.switch
    /// while it runs.
    pub switching: bool,
    /// When the login in use last changed (m.switchedAt): set by
    /// LoginOps.switch once it has.
    pub login_switched_at: Ms,
    /// When sessions a limit stopped were last told to carry on in the
    /// login in use (m.resumedAt).
    pub resumed_at: Ms,
    /// Whether a rush session is stopped by a limit: set by the screens
    /// that host sessions.
    pub limit_stopped: Option<LimitStopped>,
    /// Carries on the sessions a limit stopped: set by the screens that
    /// host sessions.
    pub relogin: Option<Relogin>,
    /// The agent new sessions run instead of the default profile's first,
    /// while its accounts are all nearly out; "" when they don't
    /// (accts.spill).
    pub spill: String,
    /// The last look for logins to keep failed, said once
    /// (m.keepFailed).
    pub keep_failed: bool,
    /// Sessions a limit stopped that were handed on to another provider,
    /// by key: each is handed on once (accts.handedOff).
    pub handed_off: std::collections::HashSet<String>,
    /// The registries were told of each other, with the programs found
    /// as of then (None: told before any were).
    #[doc(hidden)]
    pub synced: Option<Option<std::time::Instant>>,
    /// A read of the readings is out.
    #[doc(hidden)]
    pub reading: bool,
    /// Each agent was asked who it's signed in as.
    #[doc(hidden)]
    pub asked: bool,
    /// The store is being loaded off the UI thread.
    #[doc(hidden)]
    pub loading: bool,
    /// Nothing is written back (`--render`: one frame of the real config,
    /// which it never changes).
    #[doc(hidden)]
    pub read_only: bool,
}

static EMPTY: LazyLock<Accounts> = LazyLock::new(Accounts::default);
static EMPTY_CFG: LazyLock<Config> = LazyLock::new(Config::default);

/// Accounts' state, to change.
pub fn accounts(m: &mut Model) -> &mut Accounts {
    m.memo.forget();
    m.kept::<Accounts>()
}

/// Accounts' state, to read while drawing.
pub fn peek(m: &Model) -> &Accounts {
    m.kept.peek::<Accounts>().unwrap_or(&EMPTY)
}

/// Whether the accounts have been read since rush started: the config
/// loaded, the logins and limits read once.
pub fn read_settled(m: &Model) -> bool {
    let a = peek(m);
    a.store.is_some() && a.read_once
}

/// rush's config as loaded; the defaults until it is.
pub fn cfg(m: &Model) -> &Config {
    peek(m).store.as_ref().map_or(&*EMPTY_CFG, |s| &s.config)
}

/// rush's config, to change; loaded first if it isn't yet.
pub fn cfg_mut(m: &mut Model) -> &mut Config {
    ensure_store(m);
    &mut accounts(m).store.get_or_insert_with(Store::default).config
}

/// Uses store as rush's config: tests, and a store loaded elsewhere.
pub fn set_store(m: &mut Model, store: Store) {
    crate::bridge::sync(m);
    accounts(m).store = Some(store);
}

/// Whether config.json is never written back (`--render`).
pub fn read_only(m: &Model) -> bool {
    peek(m).read_only
}

/// Writes config.json (SaveConfig): write-behind when that's on.
pub fn save(m: &mut Model) {
    if accounts(m).read_only {
        return;
    }
    if let Some(s) = accounts(m).store.as_ref()
        && let Err(e) = crate::config::write_store(s)
    {
        m.fail(&format!("couldn't save rush's settings: {e}"));
    }
}

/// The store, loaded now when it isn't yet: a key wants it.
pub fn ensure_store(m: &mut Model) {
    crate::bridge::sync(m);
    if accounts(m).store.is_none() {
        let s = Store::load();
        accounts(m).store = Some(s);
    }
}

/// Keeps a plan reading this rush fetched under key (setFetched): one
/// that failed (no time) keeps the last good numbers and notes why
/// they're not refreshing.
pub fn set_fetched(m: &mut Model, key: String, u: Reading) {
    let a = accounts(m);
    if u.fetched_at == 0
        && let Some(old) = a.fetched.get_mut(&key)
    {
        old.problem = u.problem;
        return;
    }
    a.fetched.insert(key, u);
}

/// Keeps the limits this rush read, the newest of each (quotaMsg's
/// applyTo).
pub fn set_quotas(m: &mut Model, got: HashMap<String, Quota>) {
    let a = accounts(m);
    for (k, q) in got {
        if a.quotas_read
            .get(&k)
            .is_some_and(|old| old.fetched_at > q.fetched_at)
        {
            continue;
        }
        a.quotas_read.insert(k, q);
    }
}

/// What a reading of the disk found.
#[derive(Default)]
pub struct Read {
    pub logins: Vec<LoginView>,
    pub quotas: HashMap<String, Quota>,
}

impl Read {
    /// Every saved login with its usage, and every other account's limits
    /// (fleet's logins): dir is rush's folder, claude_json the logins
    /// agent's own state file, saying whom ~/.claude is signed in as.
    pub fn from_disk(cfg: &Config, dir: &Path, claude_json: &Path, now: Ms) -> Read {
        Read::from_disk_with(cfg, dir, claude_json, &HashMap::new(), now)
    }

    /// from_disk, with the readings this rush fetched itself (kept) in
    /// place of the shared file's where they're no older (syncUsage).
    pub fn from_disk_with(
        cfg: &Config,
        dir: &Path,
        claude_json: &Path,
        kept: &HashMap<String, Reading>,
        now: Ms,
    ) -> Read {
        let using = std::fs::read_to_string(dir.join(rush_state::LOGINS_KIND).join("using"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let mut fetched = readings(&dir.join("usage.json"));
        for (k, r) in kept {
            if fetched.get(k).is_none_or(|f| f.fetched_at <= r.fetched_at) {
                fetched.insert(k.clone(), r.clone());
            }
        }
        let root = signed_in_as(claude_json);
        Read {
            logins: logins(cfg, &using, &root, &fetched, now),
            quotas: usage::load(&dir.join("quotas.json")).into_iter().collect(),
        }
    }
}

/// The plan readings every rush and session shares through path, by key,
/// read by the logins agent's adapter, whose format they're in (Go's
/// fleet plans().Readings). With no such adapter, there are none.
pub fn readings(path: &Path) -> BTreeMap<String, Reading> {
    rush_core::adapter::global()
        .get(rush_state::LOGINS_KIND)
        .and_then(|a| {
            a.plan_reader()
                .map(|pr| pr.readings(&path.to_string_lossy()))
        })
        .unwrap_or_default()
}

/// The logins with their readings: the one in use is the one rush runs
/// new sessions as (using, when it's one of them), else the one ~/.claude
/// is signed in as (root).
pub fn logins(
    cfg: &Config,
    using: &str,
    root: &str,
    fetched: &BTreeMap<String, Reading>,
    now: Ms,
) -> Vec<LoginView> {
    let using = if cfg.login(using).is_some() {
        using
    } else {
        ""
    };
    cfg.logins
        .iter()
        .map(|lg| {
            let mut u = fetched.get(&lg.usage_key()).cloned().unwrap_or_default();
            u.email = lg.email.clone();
            u.org = lg.org.clone();
            let u = u.since(now);
            LoginView {
                quota: u.quota(&lg.usage_key()),
                current: if using.is_empty() {
                    !lg.id.is_empty() && lg.id == root
                } else {
                    using == lg.id
                },
                login: lg.clone(),
                usage: u,
            }
        })
        .collect()
}

/// The logins agent's own state file: whom ~/.claude is signed in as.
#[doc(hidden)]
pub fn claude_json() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join(".claude.json");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".claude.json")
}

/// The account uuid a Claude Code state file says it's signed in as.
pub fn signed_in_as(path: &Path) -> String {
    #[derive(serde::Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Oauth {
        account_uuid: String,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct File {
        oauth_account: Oauth,
    }
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<File>(&b).ok())
        .map(|f| f.oauth_account.account_uuid)
        .unwrap_or_default()
}

static NO_CONFIG: LazyLock<Config> = LazyLock::new(Config::default);

/// rush's store, read the first time it's wanted. It is the one store the
/// accounts hold too (Go's one m.store): a change made here and one made
/// there land in the same copy, so neither saves the other's old value
/// back.
pub fn store(m: &mut Model) -> &mut Store {
    ensure_store(m);
    accounts(m).store.get_or_insert_with(Store::load)
}

/// rush's config as loaded; the defaults before it is.
pub fn config(m: &Model) -> &Config {
    loaded_config(m).unwrap_or(&NO_CONFIG)
}

/// rush's config, once it's been read.
pub fn loaded_config(m: &Model) -> Option<&Config> {
    peek(m).store.as_ref().map(|s| &s.config)
}

/// rush's config, to change: save it after with [`changed`].
pub fn config_mut(m: &mut Model) -> &mut Config {
    &mut store(m).config
}

/// Puts a store in place of the one on disk (tests).
pub fn use_store(m: &mut Model, s: Store) {
    set_store(m, s);
}

/// The account to move on to, and why, from the meters of an agent's
/// accounts (list_extras/plan.rs next_account).
pub type NextAccount = fn(&[MeterRow], Ms) -> Option<(MeterRow, String)>;

/// An account as a meter shows it (acctRow, the parts read here).
#[derive(Clone, Debug, Default)]
pub struct MeterRow {
    /// The provider's mark and its colour (lookOf).
    pub glyph: String,
    pub colour: Option<Color>,
    /// harness:account, as rush names it (acctRow.label).
    pub label: String,
    pub q: Quota,
    /// The agent it signs in (acctRow.kind), its own name (acctRow.name)
    /// and whether it is the one in use (acctRow.current).
    pub kind: String,
    pub name: String,
    pub current: bool,
}

/// Where a reading r of profile p's account is kept: by the login it's
/// signed in as, else by its folder (fleet.ReadingKey).
pub fn reading_key(p: &Profile, r: &Reading) -> String {
    if r.account_id.is_empty() {
        return p.dir.clone();
    }
    Login {
        id: r.account_id.clone(),
        ..Login::default()
    }
    .usage_key()
}
