//! What Accounts knows beyond the list: rush's config (the store), each
//! login with its plan's reading, every other account's limits, who each
//! agent is signed in as, and what the next session starts as. Kept with
//! `Model::kept`; read off the UI thread and handed back. After Go's
//! internal/ui/accounts.go (accountsState :24, ready :57), fleet/logins.go
//! (LoginView :14, logins :25), claude/homes.go:40 (Using) and
//! agentquota.go:72 (quotaMsg.applyTo).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use rush_core::adapter::{AccountSummary, Choice};
use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Reading};
use rush_state::{Config, Login, Store};

use super::setup::StartOver;
use crate::frame::Out;
use crate::model::Model;

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
    /// What the next session starts as, picked on the sheet (m.startOver).
    pub start_over: Option<StartOver>,
    /// The models each agent offers as read from its home, by kind
    /// (m.listed).
    pub listed: HashMap<String, Vec<Choice>>,
    /// Starts sessions: set by the screens that host them.
    pub starter: Option<Starter>,
    /// Starts a session for rush's own ends (#ask): set by the screens
    /// that host them.
    pub hosted_starter: Option<super::askrush::HostedStarter>,
    /// config.json is watched for changes made elsewhere.
    pub(crate) reloading: bool,
    /// Switches sessions: set by the screens that host them.
    pub switcher: Option<Switcher>,
    /// What a hosted session runs as now, by its key (sessionStart): set
    /// by the screens that host sessions.
    pub session_start: Option<fn(&Model, &str) -> Option<StartOver>>,
    /// The list's agents newest first, by key (newest).
    pub newest: Vec<String>,
    /// Claude Code's logins through rush's vault, once it's ported.
    pub login_ops: Option<super::keys::LoginOps>,
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
    pub limit_stopped: Option<super::autoswitch::LimitStopped>,
    /// Carries on the sessions a limit stopped: set by the screens that
    /// host sessions.
    pub relogin: Option<super::autoswitch::Relogin>,
    /// The registries were told of each other.
    pub(crate) synced: bool,
    /// A read of the readings is out.
    reading: bool,
    /// Each agent was asked who it's signed in as.
    asked: bool,
    /// The store is being loaded off the UI thread.
    loading: bool,
    /// Nothing is written back (`--render`: one frame of the real config,
    /// which it never changes).
    read_only: bool,
}

static EMPTY: LazyLock<Accounts> = LazyLock::new(Accounts::default);
static EMPTY_CFG: LazyLock<Config> = LazyLock::new(Config::default);

/// Accounts' state, to change.
pub fn accounts(m: &mut Model) -> &mut Accounts {
    m.kept::<Accounts>()
}

/// Accounts' state, to read while drawing.
pub fn peek(m: &Model) -> &Accounts {
    m.kept.peek::<Accounts>().unwrap_or(&EMPTY)
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
    super::bridge::sync(m);
    accounts(m).store = Some(store);
}

/// Writes config.json (SaveConfig): write-behind when that's on.
pub fn save(m: &mut Model) {
    if accounts(m).read_only {
        return;
    }
    if let Some(s) = accounts(m).store.as_ref()
        && let Err(e) = s.save_config()
    {
        m.fail(&format!("couldn't save rush's settings: {e}"));
    }
}

/// rush's config as it is, never written back: `--render` draws what Go's
/// does from the same config.json (cmd/rush/main.go:510 state.Load) but
/// leaves it as it was.
pub fn load_read_only(m: &mut Model) {
    accounts(m).read_only = true;
    ensure_store(m);
    read(m); // with no loop, at once
}

/// The store, loaded now when it isn't yet: a key wants it.
pub fn ensure_store(m: &mut Model) {
    super::bridge::sync(m);
    if accounts(m).store.is_none() {
        let s = Store::load();
        accounts(m).store = Some(s);
    }
}

/// After each reading of the list: the store loaded off the UI thread the
/// first time, and the accounts' readings read again. Only with a loop:
/// tests and `--render` load what they need when asked.
pub(crate) fn on_fleet(m: &mut Model) {
    // The header names the account new sessions start on from the first
    // frame: state's list knows Claude Code's home.
    super::bridge::sync(m);
    if m.sender().is_none() {
        return;
    }
    let a = accounts(m);
    if a.store.is_none() && !a.loading {
        a.loading = true;
        m.later(Store::load, |m, s| {
            let a = accounts(m);
            a.loading = false;
            if a.store.is_none() {
                a.store = Some(s);
            }
            read(m);
            Out::Draw
        });
        return;
    }
    read(m);
    if !accounts(m).asked {
        accounts(m).asked = true;
        super::keys::find_sign_ins(m);
    }
}

/// Reads the logins' and accounts' readings off the UI thread.
pub fn read(m: &mut Model) {
    let a = accounts(m);
    let Some(store) = a.store.as_ref() else {
        return;
    };
    if a.reading {
        return;
    }
    a.reading = true;
    let cfg = store.config.clone();
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    m.later(
        move || Read::from_disk(&cfg, &rush_state::dir(), &claude_json(), now),
        |m, r| {
            let a = accounts(m);
            a.reading = false;
            let changed = a.logins != r.logins || a.quotas != r.quotas;
            a.logins = r.logins;
            a.quotas = r.quotas;
            if !changed {
                return Out::Idle;
            }
            // A session's usage reading arrives with the list's reading.
            super::autoswitch::auto_switch(m);
            Out::Draw
        },
    );
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
        let using = std::fs::read_to_string(dir.join(rush_state::LOGINS_KIND).join("using"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let fetched: BTreeMap<String, Reading> = std::fs::read(dir.join("usage.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let root = signed_in_as(claude_json);
        Read {
            logins: logins(cfg, &using, &root, &fetched, now),
            quotas: usage::load(&dir.join("quotas.json")).into_iter().collect(),
        }
    }
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
fn claude_json() -> PathBuf {
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
