//! The Claude logins rush keeps, which ~/.claude can be signed in as, as
//! rush_core's accounts; their plans and limits; and keeping their
//! sign-ins. Port of internal/adapters/claude/accounts.go and logins.go.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command as Cmd;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use rush_core::adapter::{
    self, Accounts, PlanReader, Profile, QuotaSource, SignInDone, SignInReader,
};
use rush_core::time::{Ms, now};
use rush_core::usage::{self as rusage, Quota, Reading};
use rush_state::vault::{FoundLogin, Restored};
use rush_state::{Config, Login};
use serde::Deserialize;
use serde_json::json;

use crate::account::Account;
use crate::adapter::{Claude, KIND, account};
use crate::files::{hex, merge_history, random_bytes};
use crate::home::{self, active, has_home, home_of, using_shown};
use crate::json::nd;
use crate::login::{self, signed, signed_in_as, the_vault};
use crate::oauth::{self, OauthError};
use crate::usage::{self, Usage, login_key};

/// Marks a Claude login's account key: the rest is where its reading is
/// kept in usage.json.
const KEY_PREFIX: &str = "claude:";

/// A saved login as rush's own account.
pub fn account_of(l: &Login) -> adapter::Account {
    adapter::Account {
        kind: KIND.into(),
        id: l.id.clone(),
        key: format!("{KEY_PREFIX}{}", l.usage_key()),
        name: l.name.clone(),
        email: l.email.clone(),
        org: l.org.clone(),
        ..adapter::Account::default()
    }
}

/// A sign-in's plan, from who Claude Code says it is.
fn plan_of(prof: Option<&rush_state::json::RawJson>) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct P {
        #[serde(rename = "organizationRateLimitTier", deserialize_with = "nd")]
        tier: String,
        #[serde(rename = "organizationType", deserialize_with = "nd")]
        typ: String,
    }
    let p: P = prof
        .and_then(|p| serde_json::from_str(p.get()).ok())
        .unwrap_or_default();
    if p.typ.is_empty() { p.tier } else { p.typ }
}

fn boxed(e: impl std::fmt::Display) -> adapter::Error {
    e.to_string().into()
}

impl Claude {
    /// The logins rush keeps, which ~/.claude can be signed in as.
    pub fn saved_accounts(&self) -> Vec<adapter::Account> {
        self.config().logins.iter().map(account_of).collect()
    }

    fn login(&self, acct: &adapter::Account) -> Option<Login> {
        self.config()
            .logins
            .into_iter()
            .find(|l| format!("{KEY_PREFIX}{}", l.usage_key()) == acct.key)
    }
}

impl Accounts for Claude {
    /// The login p's sessions run as: the one in use, in its home, or the
    /// one p is signed in as.
    fn current(&self, _p: &Profile) -> adapter::Result<adapter::Account> {
        let id = signed_in_as(&self.run_as());
        if id.is_empty() {
            return Err(Box::new(OauthError::NotSignedIn));
        }
        if let Some(l) = self.config().logins.iter().find(|l| l.id == id) {
            return Ok(account_of(l));
        }
        Ok(adapter::Account {
            kind: KIND.into(),
            key: format!("{KEY_PREFIX}{}", login_key(&id)),
            ..adapter::Account::default()
        })
    }

    /// Signs p in as acct, from the sign-in rush keeps for it.
    fn switch(&self, p: &Profile, acct: &adapter::Account) -> adapter::Result<()> {
        let Some(l) = self.login(acct) else {
            return Err(format!("rush has no sign-in for {}", acct.name).into());
        };
        home::use_login(&account(p), &l).map_err(boxed)
    }

    /// Claude Code's own sign-in, in a folder of its own: done keeps the
    /// sign-in in the vault and removes the folder, so p stays as it is
    /// until you switch.
    fn sign_in(&self, _p: &Profile) -> adapter::Result<(Cmd, SignInDone)> {
        let scratch = Account::at(
            "sign-in",
            rush_state::dir().join(format!("signin-{}", hex(&random_bytes::<4>()))),
        );
        let mut c = Cmd::new(crate::plugins::PROGRAM);
        c.args(["auth", "login"]).env_clear();
        for e in scratch.env() {
            if let Some((k, v)) = e.split_once('=') {
                c.env(k, v);
            }
        }
        let done: SignInDone = Box::new(move || {
            let l = home::adopt_login(&scratch).map_err(boxed)?;
            let mut acct = account_of(&l);
            acct.plan = plan_of(l.profile.as_ref());
            Ok(acct)
        });
        Ok((c, done))
    }

    /// Drops rush's copy of a's sign-in.
    fn forget(&self, a: &adapter::Account) -> adapter::Result<()> {
        home::forget_login(&a.id).map_err(boxed)
    }
}

impl QuotaSource for Claude {
    /// Asks Anthropic for acct's limits with the sign-in rush keeps for it,
    /// whichever folder is signed in as it. It doesn't share or cache the
    /// reading: see refresh_login.
    fn quota(&self, _p: &Profile, acct: &adapter::Account) -> adapter::Result<Quota> {
        let key = acct.key.strip_prefix(KEY_PREFIX).unwrap_or(&acct.key);
        let id = key.strip_prefix("login:").unwrap_or(key);
        let cred = rush_state::vault()
            .get(id)
            .map_err(|_| Box::new(OauthError::NotSignedIn) as adapter::Error)?;
        let u = oauth::fetch_usage_as(&cred, id).map_err(|e| Box::new(e) as adapter::Error)?;
        let mut q = u.quota(&acct.key);
        q.source = rusage::Source::Fetched;
        Ok(q)
    }
}

impl SignInReader for Claude {
    /// The login p's folder is signed in as.
    fn signed_in_as(&self, p: &Profile) -> String {
        signed_in_as(&account(p))
    }
}

/// The plan readings Claude Code cached in each folder's state file, with
/// the file's time when read.
static PLANS: LazyLock<Mutex<HashMap<PathBuf, (Reading, SystemTime)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl PlanReader for Claude {
    /// The plan Claude Code last cached for p's account.
    fn plan(&self, p: &Profile) -> Reading {
        let a = account(p);
        let Some(m) = std::fs::metadata(a.state_path())
            .ok()
            .and_then(|m| m.modified().ok())
        else {
            return Reading::default();
        };
        let mut plans = PLANS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((r, at)) = plans.get(&a.config_dir)
            && *at == m
        {
            return r.clone();
        }
        let r = usage::read_usage(&a).unwrap_or_default().reading();
        plans.insert(a.config_dir.clone(), (r.clone(), m));
        r
    }

    /// The readings shared through path; one Anthropic said to wait for
    /// says until when.
    fn readings(&self, path: &str) -> BTreeMap<String, Reading> {
        usage::readings(Path::new(path))
    }

    /// p's plan, asked of Anthropic when the shared reading is old enough.
    fn refresh_plan(&self, path: &str, p: &Profile, offline: bool) -> Reading {
        usage::refresh_usage(Path::new(path), &account(p), offline).reading()
    }
}

/// The sign-in found in ~/.claude as another account's than it names, and
/// when: (was, now, at).
static MISMATCH: LazyLock<Mutex<(String, String, Ms)>> =
    LazyLock::new(|| Mutex::new((String::new(), String::new(), 0)));

/// Whether ~/.claude signed in as was while naming now is a switch put
/// back rather than a sign-in under way (claude /login writes the sign-in
/// a moment before the name): it has to be seen twice, at least half a
/// minute apart.
fn put_back(was: &str, now_id: &str) -> bool {
    let mut m = MISMATCH.lock().unwrap_or_else(|e| e.into_inner());
    let t = now();
    if m.0 != was || m.1 != now_id || t - m.2 > 10 * 60_000 {
        *m = (was.to_string(), now_id.to_string(), t);
        return false;
    }
    t - m.2 >= 30_000
}

/// What finding logins found: the logins signed in where rush looks, a
/// switch put right, whether every older folder was taken in whole, and
/// why ~/.claude's sign-in couldn't be kept.
pub type Found = rush_state::vault::FoundLogins;

impl Claude {
    /// Keeps the sign-in ~/.claude holds now in the vault (Claude Code
    /// replaces it as it refreshes) and reports it. It also takes in the
    /// folders an older rush was given: each one's login is reported, its
    /// sign-in put in the vault unless cfg says that was done already, and
    /// its past sessions copied into ~/.claude, where they're found and
    /// resumed like any other. imported is whether every older folder was
    /// taken in whole, and cfg can forget them. It fails when ~/.claude's
    /// sign-in can't be kept: without a copy rush never switches away from
    /// it.
    ///
    /// A sign-in in ~/.claude that isn't the account it names was put back
    /// by a Claude Code started before a switch, as it refreshed its own:
    /// the switch is made again, and restored says so.
    pub fn find_logins(&self, cfg: &Config) -> Found {
        let v = the_vault();
        let root = active(cfg);
        let mut out = Found::default();
        let kept = v.keep(&root);
        let mut owner = String::new();
        let mut lg = Login::default();
        match kept {
            Ok(Some((l, o))) => {
                out.found.push(FoundLogin {
                    login: l.clone(),
                    name: String::new(),
                });
                (lg, owner) = (l, o);
            }
            Ok(None) => {}
            Err(e) => out.failed = Some(e.to_string()),
        }
        // A home's sign-in is the newest of its login's: the vault keeps a
        // copy, in case the home goes.
        for l in &cfg.logins {
            let h = home_of(&l.id);
            if has_home(&h) {
                let _ = v.keep(&h);
            }
        }
        if out.failed.is_none()
            && !owner.is_empty()
            && put_back(&owner, &lg.id)
            && !rush_core::dryrun::would(
                "restore-login",
                &root.config_dir.to_string_lossy(),
                &format!(
                    "signed in as {owner}, but it names {}: a Claude Code started before a switch put its own back",
                    lg.id
                ),
            )
        {
            let err = v.use_login(&root, &lg).err().map(|e| e.to_string());
            if err.is_some() {
                // It can't be switched back: it's signed in as owner, so it
                // says so.
                for l in &cfg.logins {
                    if l.id == owner && l.profile.is_some() {
                        let _ = login::name(&root, l);
                    }
                }
            }
            out.restored = Some(Restored {
                was: owner,
                now: lg.id.clone(),
                err,
            });
        }
        out.imported = true;
        for f in cfg.old_folders() {
            let a = Account::at(&f.name, &f.config_dir);
            if rush_core::dryrun::would(
                "import-folder",
                &f.config_dir,
                &format!(
                    "an older rush's folder: its past sessions copied into {}",
                    root.config_dir.display()
                ),
            ) {
                out.imported = false;
                continue;
            }
            if merge_history(&a, &root).is_err() {
                out.imported = false;
            }
            if cfg.folders_imported {
                continue;
            }
            let Some((lg, cred)) = signed(&a) else {
                continue;
            };
            if v.get(&lg.id).is_err() && v.put(&lg.id, &cred).is_err() {
                out.imported = false;
                continue;
            }
            out.found.push(FoundLogin {
                login: lg,
                name: a.name,
            });
        }
        out
    }

    /// A login's plan usage, shared through path like every account's.
    /// It's asked with the login's freshest sign-in: ~/.claude's when it's
    /// signed in as it, its home's when it has one, else the one the vault
    /// kept.
    pub fn refresh_login(&self, path: &str, cfg: &Config, lg: &Login, offline: bool) -> Reading {
        let path = Path::new(path);
        let root = active(cfg);
        if signed_in_as(&root) == lg.id {
            return usage::refresh_usage(path, &root, offline).reading();
        }
        let h = home_of(&lg.id);
        if has_home(&h) {
            return usage::refresh_usage(path, &h, offline).reading();
        }
        let id = lg.id.clone();
        usage::refresh_usage_for(path, &lg.usage_key(), offline, move || {
            oauth::renew_kept_on_expiry(&id, || {
                let cred = rush_state::vault()
                    .get(&id)
                    .map_err(|_| OauthError::NotSignedIn)?;
                oauth::fetch_usage_as(&cred, &id)
            })
        })
        .reading()
    }

    /// Refreshes a login's freshest sign-in, where refresh_login reads it:
    /// ~/.claude's, its home's, or the vault's.
    pub fn renew_login(&self, cfg: &Config, lg: &Login) -> Result<(), OauthError> {
        let root = active(cfg);
        let res = if signed_in_as(&root) == lg.id {
            oauth::renew(&root)
        } else if has_home(&home_of(&lg.id)) {
            oauth::renew(&home_of(&lg.id))
        } else {
            oauth::renew_kept(&lg.id)
        };
        let mut f = json!({"account": lg.usage_key(), "why": "asked"});
        if let Err(e) = &res {
            f["err"] = json!(e.to_string());
        }
        rush_sys::events::log("signin-renewed", f);
        res
    }

    /// The login new sessions run as in its home, when it's one cfg keeps.
    pub fn using_login(&self, cfg: &Config) -> String {
        let id = using_shown(); // the fleet's, every reading
        if id.is_empty() || cfg.login(&id).is_none() {
            return String::new();
        }
        id
    }

    /// Readies l's home and makes it the one new sessions run in.
    pub fn use_login(&self, cfg: &Config, l: &Login) -> std::io::Result<()> {
        home::use_login(&active(cfg), l)
    }

    /// Keeps the sign-in just made in p as its login's.
    pub fn adopt_login(&self, p: &Profile) -> std::io::Result<Login> {
        home::adopt_login(&account(p))
    }

    /// Drops a login's vault sign-in and its home.
    pub fn forget_login(&self, id: &str) -> std::io::Result<()> {
        home::forget_login(id)
    }

    /// The newest reading rush keeps in usage.json for p's folder, or for
    /// the login it's signed in as.
    pub(crate) fn last_quota_of(&self, p: &Profile) -> Option<Quota> {
        usage::last_quota(&rush_state::dir().join("usage.json"), &p.dir)
    }
}

/// Claude Code's logins as rush-state's LoginKeeper (state.Logins: the
/// logins agent's), each method the inherent one of its name.
impl rush_state::vault::LoginKeeper for Claude {
    fn find_logins(&self, cfg: &Config) -> Found {
        Claude::find_logins(self, cfg)
    }
    fn refresh_login(&self, path: &str, cfg: &Config, l: &Login, offline: bool) -> Reading {
        Claude::refresh_login(self, path, cfg, l, offline)
    }
    fn renew_login(&self, cfg: &Config, l: &Login) -> Result<(), String> {
        Claude::renew_login(self, cfg, l).map_err(|e| e.to_string())
    }
    fn using_login(&self, cfg: &Config) -> String {
        Claude::using_login(self, cfg)
    }
    fn use_login(&self, cfg: &Config, l: &Login) -> std::io::Result<()> {
        Claude::use_login(self, cfg, l)
    }
    fn adopt_login(&self, p: &Profile) -> std::io::Result<Login> {
        Claude::adopt_login(self, p)
    }
    fn forget_login(&self, id: &str) -> std::io::Result<()> {
        Claude::forget_login(self, id)
    }
}

/// A reading made by a session as it runs, kept as the login it started
/// as: see usage::record_live_usage.
pub fn record_live_usage(a: &Account, started_as: &str, u: Usage) -> std::io::Result<()> {
    usage::record_live_usage(&rush_state::dir().join("usage.json"), a, started_as, u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // logins_test.go (TestPutBackNeedsTwoLooks)
    #[test]
    fn put_back_needs_two_looks() {
        assert!(
            !put_back("x", "y"),
            "put right on the first look: it may be a sign-in under way"
        );
        MISMATCH.lock().unwrap().2 = now() - 60_000;
        assert!(put_back("x", "y"), "not put right on the second look");
        assert!(!put_back("x", "z"), "another mismatch starts over");
    }

    // claude_test.go:130 (TestAccounts)
    #[test]
    fn accounts_from_the_logins_rush_keeps() {
        let cfg = Config {
            logins: vec![Login {
                name: "work".into(),
                id: "u-1".into(),
                email: "me@x".into(),
                ..Login::default()
            }],
            ..Config::default()
        };
        let a = Claude::with_config(Account::home(), Arc::new(move || cfg.clone()));
        let accts = a.saved_accounts();
        assert_eq!(accts.len(), 1);
        assert_eq!(
            (accts[0].key.as_str(), accts[0].email.as_str()),
            ("claude:login:u-1", "me@x")
        );
        assert_eq!(a.login(&accts[0]).map(|l| l.id), Some("u-1".into()));
        let dir = tempfile::tempdir().unwrap();
        let p = Profile {
            dir: dir.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let nobody = adapter::Account {
            key: "claude:login:nobody".into(),
            ..adapter::Account::default()
        };
        assert!(
            a.switch(&p, &nobody).is_err(),
            "switched to a login rush doesn't have"
        );
    }

    #[test]
    fn plan_of_a_profile() {
        let raw = |s: &str| {
            rush_state::json::RawJson(serde_json::value::RawValue::from_string(s.into()).unwrap())
        };
        assert_eq!(
            plan_of(Some(&raw(
                r#"{"organizationType":"claude_max","organizationRateLimitTier":"t"}"#
            ))),
            "claude_max"
        );
        assert_eq!(
            plan_of(Some(&raw(r#"{"organizationRateLimitTier":"t"}"#))),
            "t"
        );
        assert_eq!(plan_of(None), "");
    }
}
