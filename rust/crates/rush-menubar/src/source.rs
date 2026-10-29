//! What the feed reads the menu from: a reading of every agent, the logins
//! and accounts with their limits, and other agents' limits. Port of the
//! parts of internal/fleet (Snapshot, Agent, LoginView, AccountView,
//! logins.go:25 logins) and internal/menubar/feed.go:80-112
//! (otherAccounts) the feed reads.
//!
//! The fleet's rows of sessions rush hosts, its accounts and its logins
//! aren't all ported yet, so the feed reads through [`Source`], and
//! [`FleetSource`] is what there is today: every adapter's rows through
//! rush-fleet's Loader, the logins agent's plan, the saved logins'
//! readings, and other agents' limits from quotas.json.

use std::path::PathBuf;

use rush_core::adapter::{Profile, Registry};
use rush_core::agent::State;
use rush_core::time::Ms;
use rush_core::usage::{Quota, Reading};

/// One row of the list, as much as the menu needs of it (Go's
/// fleet.Agent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// The host's session id, for a session rush hosts.
    pub id: String,
    pub key: String,
    /// Which agent it is.
    pub kind: String,
    pub display_name: String,
    pub state: State,
    pub needs: String,
    pub detail: String,
    pub account: String,
    pub repo: String,
    pub transcript_path: String,
    pub pid: i32,
    /// A session rush hosts.
    pub rush: bool,
    /// Asleep, a hosted session waiting on a decision.
    pub decides: bool,
    pub seen: bool,
    /// Its turn just ended and the agent hasn't said how yet.
    pub checking: bool,
    pub interactive: bool,
    pub created_at: Ms,
    pub updated_at: Ms,
}

impl Row {
    /// Working, or waiting on you.
    pub fn live(&self) -> bool {
        matches!(self.state, State::Working | State::Blocked)
    }
}

impl From<&rush_core::agent::Agent> for Row {
    fn from(a: &rush_core::agent::Agent) -> Row {
        Row {
            id: a.job.id.clone(),
            key: a.key.clone(),
            kind: a.kind.as_str().to_string(),
            display_name: a.display_name.clone(),
            state: a.job.state,
            needs: a.job.needs.clone(),
            detail: a.job.detail.clone(),
            account: a.job.account.clone(),
            repo: a.repo.clone(),
            transcript_path: a.job.transcript_path.clone(),
            pid: a.pid,
            rush: false,
            decides: false,
            seen: false,
            checking: a.checking,
            interactive: a.interactive,
            created_at: a.job.created_at,
            updated_at: a.job.updated_at,
        }
    }
}

/// A login ~/.claude can be signed in as (Go's fleet.LoginView).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Login {
    pub name: String,
    pub email: String,
    pub usage: Reading,
    pub quota: Quota,
    /// The one new sessions run as.
    pub current: bool,
}

/// An account folder (Go's fleet.AccountView).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub name: String,
    pub usage: Reading,
    pub quota: Quota,
    pub live: i64,
    pub current: bool,
}

/// Another agent's account that reports its limits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Other {
    pub name: String,
    pub kind: String,
    pub quota: Quota,
}

/// One reading of everything the menu shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub agents: Vec<Row>,
    pub logins: Vec<Login>,
    pub accounts: Vec<Account>,
    /// The accounts of every other agent that reports its limits, as every
    /// rush last read them.
    pub others: Vec<Other>,
}

/// A fresh reading of an account's plan, asked of its provider.
pub type Fetch = Box<dyn FnOnce() -> (String, Reading) + Send>;

/// Where the feed reads from.
pub trait Source: Send {
    /// A reading of it all now.
    fn snapshot(&mut self, now: Ms) -> Snapshot;
    /// What to watch for the next reading: a change to any of them is a
    /// reason to read again.
    fn watched(&self) -> Vec<PathBuf>;
    /// What a watch saw change; None when it can't tell what.
    fn changed(&mut self, paths: Option<&[PathBuf]>);
    /// Asks of each account's provider how much of its limits it has
    /// used: each Fetch runs on a thread of its own, and its reading
    /// comes back through `fetched`.
    fn refresh(&mut self) -> Vec<Fetch>;
    /// Takes a reading a Fetch made, by key.
    fn fetched(&mut self, key: String, r: Reading);
}

/// Every adapter's rows, the logins agent's plan and logins, and other
/// agents' limits: what the Rust fleet reads today.
pub struct FleetSource {
    reg: Registry,
    loader: rush_fleet::Loader,
    fetched: std::collections::HashMap<String, Reading>,
}

/// The kind of the agent whose accounts are the logins.
fn logins_kind() -> &'static str {
    rush_state::LOGINS_KIND
}

impl FleetSource {
    pub fn new(reg: Registry) -> FleetSource {
        FleetSource {
            loader: rush_fleet::Loader::new(reg.clone()),
            reg,
            fetched: Default::default(),
        }
    }

    fn usage_path() -> PathBuf {
        rush_state::dir().join("usage.json")
    }

    fn quotas_path() -> PathBuf {
        rush_state::dir().join("quotas.json")
    }

    /// The logins agent's profile the active account is.
    fn active_profile() -> Profile {
        let st = rush_state::Store::load();
        let f = st.config.active_account();
        Profile {
            kind: logins_kind().into(),
            name: f.name,
            dir: f.config_dir,
        }
    }
}

/// Where a reading is kept: its account's login, else its folder
/// (fleet.go:438 ReadingKey).
pub fn reading_key(p: &Profile, r: &Reading) -> String {
    if !r.account_id.is_empty() {
        return format!("login:{}", r.account_id);
    }
    p.dir.clone()
}

impl Source for FleetSource {
    fn snapshot(&mut self, now: Ms) -> Snapshot {
        let agents = self.loader.load(now);
        self.loader.scan(&agents);
        let rows: Vec<Row> = agents.iter().map(Row::from).collect();
        let st = rush_state::Store::load();
        let mut snap = Snapshot {
            agents: rows,
            ..Snapshot::default()
        };
        // The logins agent's own home: who it's signed in as, and the
        // sessions it runs.
        let plans = self.reg.get(logins_kind());
        let mut root = Reading::default();
        if let Some(a) = &plans
            && let Some(pr) = a.plan_reader()
        {
            let p = Self::active_profile();
            root = pr.plan(&p);
            let key = reading_key(&p, &root);
            if let Some(f) = self.fetched.get(&key)
                && f.fetched_at > root.fetched_at
            {
                root = f.clone();
            }
            let live = snap
                .agents
                .iter()
                .filter(|r| r.kind == logins_kind() && r.live())
                .count() as i64;
            snap.accounts.push(Account {
                name: if p.name.is_empty() {
                    a.name()
                } else {
                    p.name.clone()
                },
                quota: root.since(now).quota(&key),
                usage: root.clone(),
                live,
                current: true,
            });
            // Readings other rushes shared.
            for (k, r) in pr.readings(&Self::usage_path().to_string_lossy()) {
                let newer = self
                    .fetched
                    .get(&k)
                    .is_none_or(|f| r.fetched_at > f.fetched_at);
                if newer {
                    self.fetched.insert(k, r);
                }
            }
        }
        // The saved logins (logins.go:25).
        for lg in &st.config.logins {
            let key = lg.usage_key();
            let mut u = self.fetched.get(&key).cloned().unwrap_or_default();
            if !lg.id.is_empty() && root.account_id == lg.id && root.fetched_at >= u.fetched_at {
                let problem = std::mem::take(&mut u.problem);
                u = root.clone();
                u.problem = problem;
            }
            u.email = lg.email.clone();
            u.org = lg.org.clone();
            let u = u.since(now);
            snap.logins.push(Login {
                name: lg.name.clone(),
                email: lg.email.clone(),
                quota: u.quota(&key),
                usage: u,
                current: !lg.id.is_empty() && lg.id == root.account_id,
            });
        }
        // Other agents' limits, as every rush last read them
        // (feed.go:80 otherAccounts).
        let readings = rush_core::usage::load(&Self::quotas_path());
        let shown = logins_kind();
        for a in self.reg.installed_all() {
            let k = a.kind();
            if a.quota_source().is_none()
                || k.as_str() == shown
                || self.reg.current_kind(k.as_str()) != k
                || !self.reg.runs(k.as_str())
            {
                continue;
            }
            let Some(p) = a.profiles().into_iter().next() else {
                continue;
            };
            let Some(q) = readings.get(&format!("{}:{}", p.kind.as_str(), p.dir)) else {
                continue;
            };
            snap.others.push(Other {
                name: a.name(),
                kind: k.as_str().to_string(),
                quota: q.clone(),
            });
        }
        snap
    }

    fn watched(&self) -> Vec<PathBuf> {
        let mut w = self.loader.watched();
        w.push(rush_state::dir().join("config.json"));
        w.push(Self::usage_path());
        w.push(Self::quotas_path());
        w
    }

    fn changed(&mut self, paths: Option<&[PathBuf]>) {
        self.loader.changed(paths);
    }

    fn refresh(&mut self) -> Vec<Fetch> {
        let Some(a) = self.reg.get(logins_kind()) else {
            return vec![];
        };
        if a.plan_reader().is_none() {
            return vec![];
        }
        let p = Self::active_profile();
        let path = Self::usage_path().to_string_lossy().into_owned();
        vec![Box::new(move || {
            let Some(pr) = a.plan_reader() else {
                return (String::new(), Reading::default());
            };
            let u = pr.refresh_plan(&path, &p, false);
            (reading_key(&p, &u), u)
        })]
    }

    fn fetched(&mut self, key: String, r: Reading) {
        if key.is_empty() {
            return;
        }
        // An empty reading keeps the last good numbers, noting why they
        // aren't refreshing (fleet.go:427 setFetched).
        if r.fetched_at == 0
            && let Some(old) = self.fetched.get_mut(&key)
        {
            old.problem = r.problem;
            return;
        }
        self.fetched.insert(key, r);
    }
}
