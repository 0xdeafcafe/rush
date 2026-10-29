//! GitHub Copilot as a rush adapter: the Copilot CLI, run over ACP in rush
//! mode, and Copilot's coding agent, whose sessions on GitHub rush lists
//! and reads. Both are reached with your GitHub sign-in: gh's, when the
//! CLI has none of its own.
//!
//! Ported from Go's internal/adapters/copilot: copilot.go:1-122 (the
//! adapter), accounts.go:1-164 (gh's accounts), attribution.go:1-38,
//! quota.go:1-47 (premium requests), github.go (in `github`) and
//! remote.go (in `remote`). The CLI runs through rush-acp's [`Agent`]
//! (Go's `cli`): [`COMMAND`] with [`ARGS`], with what
//! [`Copilot::acp_options`] adds.

mod github;
mod gojson;
mod remote;

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;

use rush_acp::Agent;
use rush_core::adapter::{
    Account, Accounts, Adapter, Attributor, Conn, Discoverer, Driver, Feature, Features,
    HistoryReader, Kind, Known, Lesser, Level, LoginHinter, Made, Profile, Programmer, Published,
    Publisher, QuotaSource, Registry, Result, Session, SignInDone, Sink, StartOptions, Support,
};
use rush_core::event::Event;
use rush_core::time::Ms;
use rush_core::usage::{Quota, Source, Window};

pub use github::User;
pub use remote::{RemoteSession, call_of, read_log};

/// Copilot's kind.
pub const KIND: &str = "copilot";

/// The Copilot CLI's program.
pub const COMMAND: &str = "copilot";

/// What makes the Copilot CLI speak ACP.
pub const ARGS: [&str; 1] = ["--acp"];

/// What the Copilot CLI adds to gh alone.
pub const HINT: &str =
    "install the Copilot CLI (npm i -g @github/copilot) to run Copilot sessions here, in rush mode";

/// Reads an environment variable: the process's, or a test's own.
pub type Getenv = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// GitHub Copilot.
#[derive(Clone)]
pub struct Copilot {
    reg: Registry,
    /// The Copilot CLI over ACP (Go's cli).
    cli: Agent,
    inner: Arc<Inner>,
}

/// What the adapter keeps between calls: which account it runs on, gh's
/// tokens, its API, and the remote sessions as last listed.
pub(crate) struct Inner {
    getenv: Getenv,
    /// gh, when a test names its own.
    gh: Option<String>,
    /// GitHub's API.
    github: String,
    /// The account chosen, and when rush's config was read for it.
    chosen: Mutex<(String, Option<Instant>)>,
    /// gh's tokens by login, and when each was asked for.
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    base: Mutex<String>,
    repos: Mutex<HashMap<i64, String>>,
    listing: Mutex<remote::Listing>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn err(s: impl Into<String>) -> rush_core::adapter::Error {
    s.into().into()
}

impl Copilot {
    /// Copilot as rush runs it, knowing what's installed from reg.
    pub fn new(reg: Registry) -> Copilot {
        Copilot::with(
            reg,
            Arc::new(|k| std::env::var(k).ok().filter(|v| !v.is_empty())),
            None,
            github::GITHUB,
        )
    }

    /// With the environment read through getenv, gh at gh and GitHub's API
    /// at github: for tests.
    pub fn with(reg: Registry, getenv: Getenv, gh: Option<String>, github: &str) -> Copilot {
        Copilot {
            reg,
            cli: cli(),
            inner: Arc::new(Inner {
                getenv,
                gh,
                github: github.to_string(),
                chosen: Mutex::new((String::new(), None)),
                tokens: Mutex::new(HashMap::new()),
                base: Mutex::new(String::new()),
                repos: Mutex::new(HashMap::new()),
                listing: Mutex::new(remote::Listing::default()),
            }),
        }
    }

    /// o as the Copilot CLI starts over ACP (Go's Start): without a
    /// sign-in of its own it gets gh's, which it takes as GH_TOKEN, and
    /// it keeps to the profile's folder.
    pub fn acp_options(&self, mut o: StartOptions) -> Result<StartOptions> {
        if o.binary.is_empty() && !self.reg.runs(KIND) {
            return Err(err(format!("copilot: {HINT}")));
        }
        if !signed_in(Path::new(&o.profile.dir))
            && !has_token(&o.env)
            && let Ok(tok) = self.inner.token()
        {
            o.env.push(format!("GH_TOKEN={tok}"));
        }
        if !o.profile.dir.is_empty() {
            o.env.push(format!("COPILOT_HOME={}", o.profile.dir));
        }
        Ok(o)
    }
}

/// Registers Copilot with reg (Go's init).
pub fn register(reg: &Registry) {
    reg.register(Copilot::new(reg.clone()));
}

/// Whether the CLI has a sign-in of its own in dir.
fn signed_in(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("config.json")).is_ok_and(|b| b.contains("logged_in_users"))
}

fn has_token(env: &[String]) -> bool {
    env.iter().any(|kv| {
        ["COPILOT_GITHUB_TOKEN=", "GH_TOKEN=", "GITHUB_TOKEN="]
            .iter()
            .any(|k| kv.starts_with(k))
    })
}

/// The Copilot CLI over ACP (Go's cli). Its features are what ACP gives
/// every agent, with the coding agent's on GitHub over them: its sessions,
/// accounts and limits are read here.
fn cli() -> Agent {
    use Feature::*;
    Agent {
        id: Kind::from(KIND),
        title: "Copilot".into(),
        command: COMMAND.into(),
        args: ARGS.iter().map(|a| a.to_string()).collect(),
        tried: Level::Tested,
        more: Features::from([
            (Live, Support::YES.with("the coding agent's, on GitHub")),
            (History, Support::YES),
            (Remote, Support::YES.with("view only")),
            (Switch, Support::YES.with("gh's accounts")),
            (SignIn, Support::YES),
            (Quota, Support::YES.with("premium requests")),
            (
                Background,
                Support::YES.with("async shells, ended once Copilot reads one back"),
            ),
            (
                Pricing,
                Support::PLANNED.with("its models' premium-request multipliers"),
            ),
        ]),
        ..Agent::default()
    }
}

impl Adapter for Copilot {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "Copilot".into()
    }
    /// The CLI's over ACP, and the coding agent's on GitHub.
    fn features(&self) -> Features {
        self.cli.all_features()
    }
    /// It's been tried lightly against the real CLI.
    fn level(&self) -> Level {
        self.cli.tried
    }
    /// COPILOT_HOME, or ~/.copilot, when the CLI is installed or gh is: the
    /// coding agent needs only a GitHub sign-in.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(KIND) {
            return Vec::new();
        }
        let dir = match (self.inner.getenv)("COPILOT_HOME") {
            Some(d) => d,
            None => match (self.inner.getenv)("HOME") {
                Some(h) => Path::new(&h)
                    .join(".copilot")
                    .to_string_lossy()
                    .into_owned(),
                None => return Vec::new(),
            },
        };
        vec![Profile {
            kind: Kind::from(KIND),
            name: "Copilot".into(),
            dir,
        }]
    }
    /// Any gh account's premium requests can be read.
    fn reads_any_account(&self) -> bool {
        true
    }
    rush_core::has!(
        driver: Driver,
        login_hinter: LoginHinter,
        made: Made,
        publisher: Publisher,
        programmer: Programmer,
        lesser: Lesser,
        attributor: Attributor,
        accounts: Accounts,
        known: Known,
        quota_source: QuotaSource,
        discoverer: Discoverer,
        history_reader: HistoryReader,
    );
}

impl Driver for Copilot {
    /// Runs the Copilot CLI over ACP, with gh's sign-in when it has none of
    /// its own.
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        let o = self.acp_options(o)?;
        Ok(Box::new(self.cli.run(o, sink, false)?))
    }
}

impl LoginHinter for Copilot {
    /// How to sign Copilot in again: it runs on gh's sign-in.
    fn login_hint(&self) -> String {
        "run `gh auth login` in a terminal".into()
    }
}

impl Made for Copilot {
    /// GitHub, whose service Copilot's models come through.
    fn maker(&self) -> String {
        "GitHub".into()
    }
}

impl Publisher for Copilot {
    fn published(&self) -> Published {
        Published {
            npm: "@github/copilot".into(),
            ..Published::default()
        }
    }
}

impl Programmer for Copilot {
    /// The Copilot CLI: what runs its sessions here. Its coding agent's
    /// sessions on GitHub need only gh, and are listed without it.
    fn program(&self) -> (String, Vec<String>) {
        (COMMAND.into(), vec![".copilot/bin".into()])
    }
}

impl Lesser for Copilot {
    /// gh: with it alone, rush lists Copilot's coding agent's sessions on
    /// GitHub, its accounts and their premium requests.
    fn lesser(&self) -> (String, Vec<String>, String) {
        ("gh".into(), Vec::new(), HINT.into())
    }
}

// Copilot CLI adds a Co-authored-by trailer to its commits unless its
// settings.json says includeCoAuthoredBy false.
impl Attributor for Copilot {
    /// Whether p's Copilot CLI credits itself.
    fn attribution(&self, p: &Profile) -> (bool, bool) {
        let Ok(s) = rush_state::settingsfile::File::load(Path::new(&p.dir).join("settings.json"))
        else {
            return (false, false);
        };
        match s.get::<bool>("includeCoAuthoredBy") {
            Some(on) => (on, true),
            None => (false, false),
        }
    }

    /// Says whether p's Copilot CLI credits itself: on leaves it to its
    /// default, which does.
    fn set_attribution(&self, p: &Profile, on: bool) -> Result<()> {
        let mut s = rush_state::settingsfile::File::load(Path::new(&p.dir).join("settings.json"))?;
        if on {
            s.delete("includeCoAuthoredBy");
        } else {
            s.set("includeCoAuthoredBy", &false)?;
        }
        s.save()?;
        Ok(())
    }
}

// Copilot's accounts are the GitHub accounts gh is signed in to. Which one
// Copilot runs on is rush's to say (its config's Using), not gh's active
// account, so switching Copilot leaves the rest of your gh alone.

fn account_key(login: &str) -> String {
    format!("copilot:gh:{login}")
}

fn account_of(login: &str) -> Account {
    Account {
        kind: Kind::from(KIND),
        id: login.into(),
        key: account_key(login),
        name: login.into(),
        email: login.into(),
        ..Account::default()
    }
}

/// How long rush's config is trusted for the account chosen.
const CHOSEN_FOR: Duration = Duration::from_secs(30);

impl Inner {
    /// The GitHub account rush runs Copilot on; empty is gh's active one.
    /// It's read from rush's config at most every half minute.
    fn chosen(&self) -> String {
        let mut c = lock(&self.chosen);
        if c.1.is_none_or(|at| at.elapsed() > CHOSEN_FOR) {
            let store = rush_state::Store::load();
            c.0 = store.config.using.get(KIND).cloned().unwrap_or_default();
            c.1 = Some(Instant::now());
        }
        c.0.clone()
    }

    /// The accounts gh is signed in to on github.com, and which is active.
    fn gh_logins(&self) -> Result<(Vec<String>, String)> {
        let out = Command::new(self.gh())
            .args(["auth", "status", "-h", "github.com", "--json", "hosts"])
            .output();
        let out = match out {
            Ok(o) if o.status.success() => o.stdout,
            _ => return Err(err("copilot: gh isn't signed in to GitHub (gh auth login)")),
        };
        logins_of(&out)
    }
}

/// gh auth status's answer: the logins on github.com, and the active one.
fn logins_of(out: &[u8]) -> Result<(Vec<String>, String)> {
    #[derive(Deserialize)]
    struct Status {
        #[serde(default)]
        hosts: HashMap<String, Vec<Host>>,
    }
    #[derive(Deserialize)]
    struct Host {
        #[serde(default)]
        login: String,
        #[serde(default)]
        active: bool,
    }
    let st: Status = gojson::from_slice(out)?;
    let mut logins = Vec::new();
    let mut active = String::new();
    for h in st.hosts.get("github.com").into_iter().flatten() {
        logins.push(h.login.clone());
        if h.active {
            active = h.login.clone();
        }
    }
    Ok((logins, active))
}

impl Copilot {
    /// The GitHub accounts gh is signed in to.
    pub fn logins(&self) -> Vec<Account> {
        let (logins, _) = self.inner.gh_logins().unwrap_or_default();
        logins.iter().map(|l| account_of(l)).collect()
    }
}

impl Known for Copilot {
    fn known(&self) -> Vec<Account> {
        self.logins()
    }
}

impl Accounts for Copilot {
    /// The GitHub account Copilot runs on.
    fn current(&self, _p: &Profile) -> Result<Account> {
        let c = self.inner.chosen();
        if !c.is_empty() {
            return Ok(account_of(&c));
        }
        let (_, active) = self.inner.gh_logins()?;
        if active.is_empty() {
            return Err(err("copilot: gh has no active GitHub account"));
        }
        Ok(account_of(&active))
    }

    /// Checks gh has a sign-in for a; rush then records a as the account
    /// Copilot runs on.
    fn switch(&self, _p: &Profile, a: &Account) -> Result<()> {
        let r = (|| {
            let (logins, _) = self.inner.gh_logins()?;
            if logins.contains(&a.id) {
                *lock(&self.inner.chosen) = (a.id.clone(), Some(Instant::now()));
                self.inner.forget_tokens();
                return Ok(());
            }
            Err(err(format!(
                "gh isn't signed in as {}; sign in to it again",
                a.id
            )))
        })();
        let mut f = rush_events::Fields::new();
        f.insert("agent".into(), "copilot".into());
        f.insert("to".into(), a.id.clone().into());
        if let Err(e) = &r {
            f.insert("err".into(), e.to_string().into());
        }
        rush_events::log("account-switched", f);
        r
    }

    /// Adds a GitHub account to gh; done puts back the account gh had
    /// active before, so only Copilot moves when you switch to the new
    /// one.
    fn sign_in(&self, _p: &Profile) -> Result<(Command, SignInDone)> {
        let (before, was) = self.inner.gh_logins().unwrap_or_default();
        let mut cmd = Command::new(self.inner.gh());
        cmd.args(["auth", "login", "-h", "github.com", "--web"]);
        let inner = self.inner.clone();
        let done: SignInDone = Box::new(move || {
            let (after, active) = inner.gh_logins()?;
            let mut added = active.clone();
            for l in &after {
                if !before.contains(l) {
                    added = l.clone();
                }
            }
            if added.is_empty() {
                return Err(err("not signed in"));
            }
            if !was.is_empty() && was != active {
                let _ = Command::new(inner.gh())
                    .args(["auth", "switch", "-h", "github.com", "-u", &was])
                    .output();
            }
            inner.forget_tokens();
            Ok(account_of(&added))
        });
        Ok((cmd, done))
    }

    /// Leaves gh's sign-in be: it's gh's, and other things use it.
    fn forget(&self, _a: &Account) -> Result<()> {
        Ok(())
    }
}

impl QuotaSource for Copilot {
    /// Your premium requests this month: what Copilot's agents, and its
    /// better models, spend. a is the GitHub account to read; with none,
    /// the one rush uses.
    fn quota(&self, _p: &Profile, a: &Account) -> Result<Quota> {
        let u = if a.id.is_empty() {
            self.inner.read_user()?
        } else {
            self.inner.read_user_as(&a.id)?
        };
        let mut q = quota_of(&u, rush_core::time::now());
        if !a.id.is_empty() {
            q.account = account_key(&a.id);
            q.email = a.id.clone();
        }
        Ok(q)
    }
}

/// Your Copilot's limits as rush's. Chat and completions are unlimited on
/// paid plans; premium requests are what run out.
pub fn quota_of(u: &User, now: Ms) -> Quota {
    let mut q = Quota {
        plan: u.plan.clone(),
        fetched_at: now,
        source: Source::Fetched,
        ..Quota::default()
    };
    let Some(p) = u.quotas.get("premium_interactions") else {
        return q;
    };
    if p.unlimited {
        return q;
    }
    q.windows.push(Window {
        id: "premium_interactions".into(),
        label: "month".into(),
        name: "premium requests".into(),
        span: Duration::from_secs(30 * 24 * 3600),
        percent: 100.0 - p.percent,
        used: p.entitlement - p.remaining,
        limit: p.entitlement,
        resets_at: rush_core::time::parse_rfc3339(&u.resets_at),
        ..Window::default()
    });
    q
}

impl Discoverer for Copilot {
    /// Your remote sessions still working.
    fn live(&self, p: &Profile) -> Vec<Session> {
        self.inner.remote(p, false)
    }
    /// Your remote sessions that have ended.
    fn past(&self, p: &Profile) -> Vec<Session> {
        self.inner.remote(p, true)
    }
}

impl HistoryReader for Copilot {
    /// A remote session's log as the events a session sends.
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> Result<()> {
        self.inner.history(s, before, most, each)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env() -> Getenv {
        Arc::new(|_| None)
    }

    #[test]
    fn quota_of_premium_requests() {
        let mut u = User {
            plan: "individual".into(),
            resets_at: "2026-10-01T00:00:00.000Z".into(),
            ..User::default()
        };
        u.quotas.insert(
            "chat".into(),
            github::Snapshot {
                unlimited: true,
                percent: 100.0,
                ..Default::default()
            },
        );
        u.quotas.insert(
            "premium_interactions".into(),
            github::Snapshot {
                entitlement: 1500.0,
                remaining: 70.0,
                percent: 4.6,
                ..Default::default()
            },
        );
        let q = quota_of(&u, 1);
        assert_eq!(q.windows.len(), 1, "{:?}", q.windows);
        let w = &q.windows[0];
        assert_eq!((w.used, w.limit), (1430.0, 1500.0));
        assert!(w.percent >= 95.0);
        assert_eq!(
            rush_core::time::format_rfc3339(w.resets_at),
            "2026-10-01T00:00:00Z"
        );
        assert_eq!(q.plan, "individual");
    }

    #[test]
    fn attribution() {
        let dir = tempfile::tempdir().unwrap();
        let p = Profile {
            dir: dir.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let c = Copilot::with(Registry::new(), no_env(), None, "");
        assert!(!c.attribution(&p).1, "no settings.json: its default");
        c.set_attribution(&p, false).unwrap();
        assert_eq!(c.attribution(&p), (false, true));
        c.set_attribution(&p, true).unwrap();
        assert!(!c.attribution(&p).1, "on is its default");
    }

    #[test]
    fn gh_logins() {
        let (logins, active) = logins_of(
            br#"{"hosts":{"github.com":[{"login":"a","active":false,"state":"success"},{"login":"b","active":true}],"ghe.io":[{"login":"c","active":true}]}}"#,
        )
        .unwrap();
        assert_eq!(logins, ["a", "b"]);
        assert_eq!(active, "b");
    }

    /// A gh that answers auth status and auth token from a script.
    #[cfg(unix)]
    fn fake_gh(dir: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("gh");
        std::fs::write(
            &p,
            "#!/bin/sh\ncase \"$1 $2\" in\n\
             'auth status') echo '{\"hosts\":{\"github.com\":[{\"login\":\"octo\",\"active\":true}]}}' ;;\n\
             'auth token') if [ \"$5\" = -u ]; then echo \"tok-$6\"; else echo tok-active; fi ;;\n\
             *) exit 1 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    // gh's sign-in is what Copilot runs on: its token goes to the CLI
    // unless the CLI or the caller has one, and accounts are gh's.
    #[cfg(unix)]
    #[test]
    fn runs_on_ghs_sign_in() {
        let bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let gh = fake_gh(bin.path());
        let c = Copilot::with(Registry::new(), no_env(), Some(gh), "");
        // A chosen account read already, so rush's own config isn't.
        *lock(&c.inner.chosen) = (String::new(), Some(Instant::now()));
        assert_eq!(c.logins(), [account_of("octo")]);
        assert_eq!(
            c.current(&Profile::default()).unwrap().key,
            "copilot:gh:octo"
        );
        assert_eq!(c.inner.token_for("octo").unwrap(), "tok-octo");

        let p = Profile {
            dir: home.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let o = c
            .acp_options(StartOptions {
                binary: "/x/copilot".into(),
                profile: p.clone(),
                ..StartOptions::default()
            })
            .unwrap();
        assert_eq!(
            o.env,
            [
                "GH_TOKEN=tok-active".to_string(),
                format!("COPILOT_HOME={}", p.dir)
            ]
        );
        std::fs::write(home.path().join("config.json"), r#"{"logged_in_users":[]}"#).unwrap();
        let o = c
            .acp_options(StartOptions {
                binary: "/x/copilot".into(),
                profile: p.clone(),
                ..StartOptions::default()
            })
            .unwrap();
        assert_eq!(o.env, [format!("COPILOT_HOME={}", p.dir)]);
        // Not installed, and no program named: the hint.
        let e = c.acp_options(StartOptions::default()).err().unwrap();
        assert_eq!(e.to_string(), format!("copilot: {HINT}"));
    }

    /// A sink that hands each Init on.
    struct Inits(Mutex<std::sync::mpsc::Sender<rush_core::event::Init>>);

    impl Sink for Inits {
        fn event(&self, ev: Event) {
            if let Event::Init(i) = ev {
                let _ = lock(&self.0).send(*i);
            }
        }
        fn ended(&self, _: Option<rush_core::adapter::Error>) {}
    }

    // A session starts over ACP: the CLI run with --acp, in the profile's
    // folder, on gh's sign-in.
    #[cfg(unix)]
    #[test]
    fn starts_over_acp() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let gh = fake_gh(bin.path());
        let c = Copilot::with(Registry::new(), no_env(), Some(gh), "");
        // A chosen account read already, so rush's own config isn't.
        *lock(&c.inner.chosen) = (String::new(), Some(Instant::now()));
        let script = format!(
            r#"#!/bin/sh
[ "$*" = "--acp" ] || {{ echo "run with: $*" >&2; exit 2; }}
[ "$COPILOT_HOME" = '{}' ] && [ "$GH_TOKEN" = tok-active ] || {{ echo "env: $COPILOT_HOME $GH_TOKEN" >&2; exit 2; }}
reply() {{ id=$(printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'); printf '{{"jsonrpc":"2.0","id":%s,%s}}\n' "$id" "$2"; }}
read -r l; reply "$l" '"result":{{"protocolVersion":1,"agentInfo":{{"name":"copilot","version":"1.0.0"}}}}'
read -r l; reply "$l" '"result":{{"sessionId":"cop-1"}}'
cat > /dev/null
"#,
            home.path().display()
        );
        let p = bin.path().join(COMMAND);
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let conn = c
            .start(
                StartOptions {
                    binary: p.to_string_lossy().into_owned(),
                    dir: home.path().to_string_lossy().into_owned(),
                    profile: Profile {
                        dir: home.path().to_string_lossy().into_owned(),
                        ..Profile::default()
                    },
                    ..StartOptions::default()
                },
                Arc::new(Inits(Mutex::new(tx))),
            )
            .unwrap();
        let init = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(init.session_id, "cop-1");
        conn.close().unwrap();
    }

    // What Go's Copilot says: ACP's features, with the coding agent's on
    // GitHub and Copilot's async shells over them.
    #[test]
    fn features_over_acp() {
        use rush_core::adapter::Has;
        let f = Copilot::with(Registry::new(), no_env(), None, "").features();
        for k in [
            Feature::Run,
            Feature::Resume,
            Feature::Interrupt,
            Feature::Modes,
            Feature::Questions,
            Feature::Background,
            Feature::History,
            Feature::Remote,
        ] {
            assert_eq!(f[&k].is, Has::Yes, "{k:?}");
        }
        assert_eq!(
            f[&Feature::Background].note,
            "async shells, ended once Copilot reads one back"
        );
        assert_eq!(f[&Feature::Pricing].is, Has::Planned);
    }

    #[test]
    fn dormant_until_installed() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let reg = Registry::with_env(bin.path().to_str().unwrap(), home.path().to_str().unwrap());
        let h = home.path().to_string_lossy().into_owned();
        let getenv: Getenv = Arc::new(move |k| (k == "HOME").then(|| h.clone()));
        reg.register(Copilot::with(reg.clone(), getenv, None, ""));
        assert_eq!(reg.label(KIND), "GitHub (Copilot)");
        if !reg.path(KIND).is_empty() || reg.installed(KIND) {
            return; // copilot or gh is installed where every machine looks
        }
        assert!(reg.get(KIND).unwrap().profiles().is_empty());
        #[cfg(unix)]
        {
            fake_gh(bin.path());
            reg.recheck();
            assert!(
                reg.installed(KIND) && !reg.runs(KIND),
                "gh alone is the lesser program"
            );
            assert_eq!(reg.hint(KIND), HINT);
            let ps = reg.get(KIND).unwrap().profiles();
            assert_eq!(Path::new(&ps[0].dir), home.path().join(".copilot"));
        }
    }
}
