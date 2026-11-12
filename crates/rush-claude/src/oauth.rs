//! Asking Anthropic about a sign-in with the sign-in itself: whose it is,
//! its plan usage, and refreshing it as Claude Code would. Port of
//! internal/adapters/claude/claude/usagefetch.go (ownerErr at :31,
//! accessToken at :118) and renew.go.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::Duration;

use rush_core::time::{Ms, now};
use rush_core::usage::EXPIRED;
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;

use crate::account::Account;
use crate::json::nd;
use crate::login::{read_creds, the_vault, write_creds};
use crate::usage::{RawWindow, Usage, login_key, window_of};

/// Where Anthropic answers a sign-in's questions.
const API: &str = "https://api.anthropic.com/api/oauth/";

/// Claude Code's own refresh, as 2.1.284 has it (strings of its binary):
/// TOKEN_URL and CLIENT_ID, a JSON POST of {grant_type: "refresh_token",
/// refresh_token, client_id, scope}, answered with access_token,
/// refresh_token (the old one when absent) and expires_in in seconds. A
/// 400 or 401 saying invalid_grant is a revoked sign-in.
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// What asking about a sign-in fails with, in Go's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OauthError {
    NotSignedIn,
    /// usage::EXPIRED: one rush may be able to refresh.
    Expired,
    /// A saved sign-in with neither token nor refresh token.
    NoToken,
    /// A saved sign-in that isn't JSON.
    Unreadable,
    Revoked,
    InUse,
    /// When the usage endpoint says to try again.
    RateLimited(Ms),
    Other(String),
}

impl std::fmt::Display for OauthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OauthError::NotSignedIn => "not signed in",
            OauthError::Expired => EXPIRED,
            OauthError::NoToken => "its saved sign-in has no token; sign in again",
            OauthError::Unreadable => "its saved sign-in can't be read; sign in again",
            OauthError::Revoked => "sign-in revoked · l signs in again",
            OauthError::InUse => "in use by a running Claude Code, which refreshes it itself",
            OauthError::RateLimited(_) => "usage refresh rate-limited",
            OauthError::Other(s) => s,
        })
    }
}

impl std::error::Error for OauthError {}

impl From<io::Error> for OauthError {
    fn from(e: io::Error) -> OauthError {
        OauthError::Other(e.to_string())
    }
}

fn other(s: impl Into<String>) -> OauthError {
    OauthError::Other(s.into())
}

/// Why owner couldn't say whose a sign-in is, as shown: what's wrong with
/// the sign-in itself as it is, Anthropic's answer behind what was asked
/// (usagefetch.go:31 ownerErr).
fn owner_err(e: OauthError) -> OauthError {
    match e {
        OauthError::Expired | OauthError::NoToken | OauthError::Unreadable => e,
        e => other(format!("couldn't tell whose sign-in it is: {e}")),
    }
}

/// An agent whose requests wait at most timeout, or less when they're
/// part of a reading of usage that has less left of its deadline
/// (usage::within).
fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(rush_core::usage::within(timeout)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

/// A body read to at most 1 MiB.
fn body(res: &mut ureq::http::Response<ureq::Body>) -> Vec<u8> {
    res.body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_vec()
        .unwrap_or_default()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Creds {
    #[serde(rename = "claudeAiOauth", deserialize_with = "nd")]
    oauth: OAuth,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OAuth {
    #[serde(rename = "accessToken", deserialize_with = "nd")]
    token: String,
    #[serde(rename = "expiresAt", deserialize_with = "nd")]
    expires_at: i64,
    #[serde(rename = "refreshToken", deserialize_with = "nd")]
    refresh: String,
    #[serde(deserialize_with = "nd")]
    scopes: Vec<String>,
    #[serde(rename = "clientId", deserialize_with = "nd")]
    client_id: String,
    /// The plan the sign-in is on, as Claude Code keeps it from the
    /// profile's organization_type: "max", "pro", "team", "enterprise".
    #[serde(rename = "subscriptionType", deserialize_with = "nd")]
    subscription: String,
}

/// A sign-in's refresh token; None when it can't be read.
pub(crate) fn refresh_token(cred: &[u8]) -> Option<String> {
    serde_json::from_slice::<Creds>(cred)
        .ok()
        .map(|c| c.oauth.refresh)
}

/// A sign-in's token, while it still works. One that has lost its token
/// but kept its refresh token counts as expired, so it's refreshed like
/// one rather than left until you sign in again (usagefetch.go:118).
fn access_token(raw: &[u8]) -> Result<String, OauthError> {
    let c: Creds = serde_json::from_slice(raw).map_err(|_| OauthError::Unreadable)?;
    if c.oauth.token.is_empty() && !c.oauth.refresh.is_empty() {
        return Err(OauthError::Expired);
    }
    if c.oauth.token.is_empty() {
        return Err(OauthError::NoToken);
    }
    if c.oauth.expires_at > 0 && c.oauth.expires_at < now() {
        return Err(OauthError::Expired);
    }
    Ok(c.oauth.token)
}

fn oauth_get(base: &str, what: &str, token: &str) -> Result<ureq::http::Response<ureq::Body>, ()> {
    agent(Duration::from_secs(15))
        .get(&format!("{base}{what}"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .call()
        .map_err(|_| ())
}

/// Whose each token is, by its hash: a token only ever belongs to one
/// account, so each is asked about once.
static OWNERS: LazyLock<Mutex<HashMap<[u8; 32], String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The uuid of the account a sign-in belongs to, asked of Anthropic with
/// the sign-in itself. Which account a config folder says it's signed in
/// as can be wrong: a Claude Code started before a switch writes the old
/// account's sign-in back when it refreshes it, leaving the new one's name
/// beside it.
pub fn owner(raw: &[u8]) -> Result<String, OauthError> {
    owner_at(API, raw)
}

pub(crate) fn owner_at(base: &str, raw: &[u8]) -> Result<String, OauthError> {
    use sha2::Digest;
    let token = access_token(raw)?;
    let sum: [u8; 32] = sha2::Sha256::digest(token.as_bytes()).into();
    if let Some(id) = OWNERS.lock().unwrap_or_else(|e| e.into_inner()).get(&sum) {
        return Ok(id.clone());
    }
    let mut res =
        oauth_get(base, "profile", &token).map_err(|_| other("profile request failed"))?;
    match res.status().as_u16() {
        200 => {}
        401 => return Err(OauthError::Expired),
        s => return Err(other(format!("profile request returned {s}"))),
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct A {
        #[serde(deserialize_with = "nd")]
        uuid: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct P {
        #[serde(deserialize_with = "nd")]
        account: A,
    }
    let p: P = serde_json::from_slice(&body(&mut res)).map_err(|e| other(e.to_string()))?;
    if p.account.uuid.is_empty() {
        return Err(other("profile names no account"));
    }
    OWNERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(sum, p.account.uuid.clone());
    Ok(p.account.uuid)
}

/// Asks Anthropic for an account's plan usage with the account's own
/// sign-in. The token is read for this request only and never kept.
pub fn fetch_usage(a: &Account) -> Result<Usage, OauthError> {
    let raw = read_creds(a).map_err(|_| OauthError::NotSignedIn)?;
    // Whose reading this is: ~/.claude may be signed in as another account
    // by the time it's looked at, and the sign-in may not be the account
    // the folder names. Unless Anthropic says whose, it's no one's.
    let id = owner(&raw).map_err(owner_err)?;
    let mut u = fetch_usage_with(&raw)?;
    u.account_id = id;
    Ok(u)
}

/// fetch_usage_with for a sign-in kept as account id's: it's asked only
/// once Anthropic says the sign-in is id's, so a sign-in kept under the
/// wrong name never gives its reading to another account.
pub fn fetch_usage_as(raw: &[u8], id: &str) -> Result<Usage, OauthError> {
    match owner(raw) {
        Err(e) => return Err(owner_err(e)),
        Ok(o) if o != id => {
            return Err(other(
                "rush's sign-in for it is another account's; sign in again",
            ));
        }
        Ok(_) => {}
    }
    let mut u = fetch_usage_with(raw)?;
    u.account_id = id.to_string();
    Ok(u)
}

/// fetch_usage with a sign-in already in hand: a login's that isn't the
/// one in use.
pub fn fetch_usage_with(raw: &[u8]) -> Result<Usage, OauthError> {
    fetch_usage_with_at(API, raw)
}

pub(crate) fn fetch_usage_with_at(base: &str, raw: &[u8]) -> Result<Usage, OauthError> {
    let token = access_token(raw)?;
    let mut res = oauth_get(base, "usage", &token).map_err(|_| other("usage request failed"))?;
    match res.status().as_u16() {
        200 => {}
        429 => {
            let v = res
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            return Err(OauthError::RateLimited(retry_after(&v)));
        }
        401 => return Err(OauthError::Expired),
        s => return Err(other(format!("usage request returned {s}"))),
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct D {
        five_hour: Option<RawWindow>,
        seven_day: Option<RawWindow>,
    }
    let d: D = serde_json::from_slice(&body(&mut res)).map_err(|e| other(e.to_string()))?;
    Ok(Usage {
        plan: plan_of(raw),
        five_hour: window_of(d.five_hour).five_hour(),
        seven_day: window_of(d.seven_day),
        fetched_at: now(),
        fetched: true,
        ..Usage::default()
    })
}

/// The plan a sign-in is on, as Claude Code saved it with the sign-in;
/// "" when it didn't say. The usage endpoint doesn't name it.
fn plan_of(raw: &[u8]) -> String {
    serde_json::from_slice::<Creds>(raw)
        .map(|c| c.oauth.subscription.trim().to_lowercase())
        .unwrap_or_default()
}

/// When to ask again: what Retry-After says, but not before 15 minutes.
fn retry_after(v: &str) -> Ms {
    let least = now() + 15 * 60_000;
    match v.parse::<i64>() {
        Ok(s) if now().saturating_add(s.saturating_mul(1000)) > least => {
            now().saturating_add(s.saturating_mul(1000))
        }
        _ => least,
    }
}

// ---- Refreshing a sign-in (renew.go) ----

/// Whether a Claude Code running now has a's folder as its config.
pub fn in_use(a: &Account) -> bool {
    let snap = rush_sys::proc::snapshot();
    runs_on(
        &a.config_dir,
        snap.procs
            .values()
            .filter(|p| {
                Path::new(&*p.comm)
                    .file_name()
                    .is_some_and(|n| n == "claude")
            })
            .map(|p| rush_sys::proc::readable_env(p.pid)),
    )
}

/// Whether any of the Claude Codes with these environments runs on dir.
/// One whose environment the kernel won't give (None) doesn't: that's
/// another user's process, or one that has exited (a zombie), and neither
/// refreshes this user's sign-in in dir. Go counted it (renew.go InUse),
/// which left an idle login's expired sign-in unrefreshed, and its usage
/// unread, for as long as such a process was around.
fn runs_on(dir: &Path, envs: impl IntoIterator<Item = Option<Vec<String>>>) -> bool {
    let want = crate::files::clean(&dir.to_string_lossy());
    envs.into_iter()
        .flatten()
        .any(|e| crate::files::clean(&config_dir_in(&e)) == want)
}

/// The folder a Claude Code with env runs in.
fn config_dir_in(env: &[String]) -> String {
    env.iter()
        .find_map(|kv| {
            kv.strip_prefix("CLAUDE_CONFIG_DIR=")
                .filter(|v| !v.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| Account::home().config_dir.to_string_lossy().into_owned())
}

/// Takes Claude Code's own lock on refreshing dir's sign-in: a folder,
/// stale once a minute old. A Claude Code starting meanwhile waits for it,
/// then finds the new sign-in rather than refreshing. Dropping what it
/// gives lets go.
fn refresh_lock(dir: &Path) -> Result<impl Drop, OauthError> {
    struct Release(std::path::PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.0);
        }
    }
    let p = dir.join(".oauth_refresh.lock");
    if let Ok(m) = fs::metadata(&p)
        && m.modified()
            .is_ok_and(|t| t.elapsed().is_ok_and(|e| e > Duration::from_secs(60)))
    {
        let _ = fs::remove_dir(&p);
    }
    match fs::create_dir(&p) {
        Ok(()) => Ok(Release(p)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(OauthError::InUse),
        Err(e) => Err(e.into()),
    }
}

/// Refreshes the sign-in a's folder holds, as Claude Code would there,
/// unless a Claude Code runs there now: two refreshing one sign-in revoke
/// each other's. It holds rush's vault lock and Claude Code's own refresh
/// lock on the folder, and keeps the vault's copy in step.
pub fn renew(a: &Account) -> Result<(), OauthError> {
    renew_unless(a, &in_use)
}

pub(crate) fn renew_unless(a: &Account, busy: &dyn Fn(&Account) -> bool) -> Result<(), OauthError> {
    if busy(a) {
        return Err(OauthError::InUse);
    }
    let v = the_vault();
    let _unlock = v.lock()?;
    let _release = refresh_lock(&a.config_dir)?;
    renew_at(TOKEN_URL, &|| read_creds(a), &|b| {
        write_creds(a, b).map_err(OauthError::from)
    })?;
    let _ = v.keep(a);
    Ok(())
}

/// Refreshes the sign-in the vault keeps for id, one no folder holds and
/// so no Claude Code runs on.
pub fn renew_kept(id: &str) -> Result<(), OauthError> {
    let v = the_vault();
    let _unlock = v.lock()?;
    renew_at(
        TOKEN_URL,
        &|| v.get(id).map_err(|_| OauthError::NotSignedIn),
        &|b| v.put(id, b).map_err(OauthError::from),
    )
}

type Read<'a> = &'a dyn Fn() -> Result<Vec<u8>, OauthError>;
type Write<'a> = &'a dyn Fn(&[u8]) -> Result<(), OauthError>;

/// The refresh tokens Anthropic has called revoked, by their hash. Asking
/// again with one only gets the same answer, so a sign-in is tried again
/// once it holds another (signed in again, or refreshed by Claude Code),
/// not every ten minutes for as long as rush runs. Not in Go.
static REFUSED: LazyLock<Mutex<HashSet<[u8; 32]>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Refreshes the sign-in read gives at url and writes it back, unless it
/// changed meanwhile (another refresh's is as good). A refresh token
/// Anthropic has refused already isn't sent again.
pub(crate) fn renew_at(url: &str, read: Read, write: Write) -> Result<(), OauthError> {
    use sha2::Digest;
    let cred = read().map_err(|_| OauthError::NotSignedIn)?;
    let c: Creds = serde_json::from_slice(&cred).map_err(|_| OauthError::NotSignedIn)?;
    if c.oauth.refresh.is_empty() {
        return Err(OauthError::NotSignedIn);
    }
    let sum: [u8; 32] = sha2::Sha256::digest(c.oauth.refresh.as_bytes()).into();
    let refused = || REFUSED.lock().unwrap_or_else(|e| e.into_inner());
    if refused().contains(&sum) {
        return Err(OauthError::Revoked);
    }
    let next = refreshed(url, &cred, &c.oauth);
    if next == Err(OauthError::Revoked) {
        refused().insert(sum);
    }
    if let Ok(now) = read()
        && refresh_token(&now).is_some_and(|r| r != c.oauth.refresh)
    {
        return Ok(());
    }
    write(&next?)
}

/// cred with the tokens its refresh token is exchanged for, every other
/// field as it was. The scopes asked are the ones stored, which Claude
/// Code falls back to when its wider ask is refused.
fn refreshed(url: &str, cred: &[u8], c: &OAuth) -> Result<Vec<u8>, OauthError> {
    let mut ask = BTreeMap::from([
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", c.refresh.clone()),
        ("client_id", CLIENT_ID.to_string()),
    ]);
    if !c.client_id.is_empty() {
        ask.insert("client_id", c.client_id.clone());
    }
    if !c.scopes.is_empty() {
        ask.insert("scope", c.scopes.join(" "));
    }
    let b = serde_json::to_vec(&ask).map_err(|e| other(e.to_string()))?;
    let mut res = agent(Duration::from_secs(30))
        .post(url)
        .header("Content-Type", "application/json")
        .send(&b[..])
        .map_err(|_| other("refresh request failed"))?;
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Got {
        #[serde(deserialize_with = "nd")]
        access_token: String,
        #[serde(deserialize_with = "nd")]
        refresh_token: String,
        #[serde(deserialize_with = "nd")]
        expires_in: i64,
        error: Option<Box<RawValue>>,
    }
    let status = res.status().as_u16();
    let got: Got = serde_json::from_slice(&body(&mut res)).unwrap_or_default();
    if (status == 400 || status == 401) && oauth_error(got.error.as_deref()) == "invalid_grant" {
        return Err(OauthError::Revoked);
    }
    if status != 200 {
        return Err(other(format!("refresh returned {status}")));
    }
    if got.access_token.is_empty() || got.expires_in <= 0 {
        return Err(other("refresh answered without a token"));
    }
    let refresh = if got.refresh_token.is_empty() {
        c.refresh.clone()
    } else {
        got.refresh_token
    };
    with_tokens(
        cred,
        &got.access_token,
        &refresh,
        now().saturating_add(got.expires_in.saturating_mul(1000)),
    )
}

/// An error answer's code: a string, or an object's type.
fn oauth_error(v: Option<&RawValue>) -> String {
    let Some(v) = v else {
        return String::new();
    };
    if let Ok(s) = serde_json::from_str::<String>(v.get()) {
        return s;
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct O {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
    }
    serde_json::from_str::<O>(v.get())
        .map(|o| o.typ)
        .unwrap_or_default()
}

/// cred with its access token, refresh token and expiry replaced, and
/// nothing else changed.
fn with_tokens(
    cred: &[u8],
    access: &str,
    refresh: &str,
    expires_at: i64,
) -> Result<Vec<u8>, OauthError> {
    let mut all: BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(cred).map_err(|e| other(e.to_string()))?;
    let inner = all.get("claudeAiOauth").map_or("null", |v| v.get());
    let mut oauth: BTreeMap<String, Box<RawValue>> =
        serde_json::from_str(inner).map_err(|e| other(e.to_string()))?;
    let raw = |v: serde_json::Value| {
        RawValue::from_string(v.to_string()).map_err(|e| other(e.to_string()))
    };
    oauth.insert("accessToken".into(), raw(json!(access))?);
    oauth.insert("refreshToken".into(), raw(json!(refresh))?);
    oauth.insert("expiresAt".into(), raw(json!(expires_at))?);
    let inner = serde_json::to_string(&oauth).map_err(|e| other(e.to_string()))?;
    all.insert(
        "claudeAiOauth".into(),
        RawValue::from_string(inner).map_err(|e| other(e.to_string()))?,
    );
    serde_json::to_vec(&all).map_err(|e| other(e.to_string()))
}

/// When a sign-in was last tried, and what a reading says until the next
/// try when it couldn't be refreshed (renew_says).
type Tried = (Ms, Option<OauthError>);

/// When each sign-in was last refreshed on its own, by key.
static RENEWED_AT: LazyLock<Mutex<HashMap<String, Tried>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a reading says when its expired sign-in couldn't be refreshed:
/// revoked, or left to the Claude Code running on it, which refreshes it
/// itself when it's next used. Either is said until the next try rather
/// than "sign-in expired", which reads as a sign-in that needs you when
/// one in use is fine. Anything else (the network) leaves it expired. Go
/// said only revoked, and only on the try itself.
fn renew_says(e: &OauthError) -> Option<OauthError> {
    matches!(e, OauthError::Revoked | OauthError::InUse).then(|| e.clone())
}

fn always() -> bool {
    true
}

/// Whether this rush may refresh an expired sign-in on its own, asked as
/// each reading finds one: every process may unless told otherwise.
static LEADS: RwLock<fn() -> bool> = RwLock::new(always);

/// Makes refreshing an expired sign-in on its own wait on leads: only the
/// rush that leads refreshes, as a refresh replaces the sign-in's tokens
/// and the old ones stop working for whoever else holds them (a rush that
/// follows reads beside the one that leads, on the same sign-ins). Asking
/// for one (renew_login) isn't held back. Rust-only: Go's following view
/// is the one over SSH, on the same Mac's keychain as the one it follows.
pub fn renew_while(leads: fn() -> bool) {
    *LEADS.write().unwrap_or_else(|e| e.into_inner()) = leads;
}

/// Whether a reading may refresh an expired sign-in now (renew_while).
pub fn renews() -> bool {
    (LEADS.read().unwrap_or_else(|e| e.into_inner()))()
}

/// fetch's reading; when its sign-in has expired it's refreshed first, at
/// most once in ten minutes for key, by the rush that leads (renew_while)
/// and not in a dry run. A revoked sign-in, or one a running Claude Code
/// refreshes, shows as the refresh's own error (renew_says).
pub fn or_renew(
    key: &str,
    fetch: impl Fn() -> Result<Usage, OauthError>,
    renew: impl FnOnce() -> Result<(), OauthError>,
) -> Result<Usage, OauthError> {
    or_renew_if(renews(), key, fetch, renew)
}

/// or_renew, refreshing only when leads.
pub fn or_renew_if(
    leads: bool,
    key: &str,
    fetch: impl Fn() -> Result<Usage, OauthError>,
    renew: impl FnOnce() -> Result<(), OauthError>,
) -> Result<Usage, OauthError> {
    let got = fetch();
    if got != Err(OauthError::Expired) || !leads {
        return got;
    }
    {
        let mut at = RENEWED_AT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, said)) = at.get(key)
            && now() - t < 10 * 60_000
        {
            return said.clone().map_or(got, Err);
        }
        at.insert(key.to_string(), (now(), None));
    }
    if rush_core::dryrun::would("renew sign-in", key, "its sign-in expired") {
        return got;
    }
    let rerr = renew();
    let mut f = json!({"account": key, "why": "its sign-in expired"});
    if let Err(e) = &rerr {
        f["err"] = json!(e.to_string());
    }
    rush_sys::events::log("signin-renewed", f);
    let Err(e) = rerr else { return fetch() };
    let Some(said) = renew_says(&e) else {
        return got;
    };
    if let Some(at) = RENEWED_AT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(key)
    {
        at.1 = Some(said.clone());
    }
    Err(said)
}

/// fetch, the vault's sign-in for id refreshed first when it has expired:
/// see or_renew.
pub fn renew_kept_on_expiry(
    id: &str,
    fetch: impl Fn() -> Result<Usage, OauthError>,
) -> Result<Usage, OauthError> {
    or_renew(&login_key(id), fetch, || renew_kept(id))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A server answering every request with status and body, until the
    /// test ends; each request's body is passed to check. Its URL, and how
    /// many it was asked.
    pub(crate) fn server(
        status: u16,
        body: &'static str,
        check: impl Fn(&str, &str, &[u8]) + Send + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let asked = Arc::new(AtomicUsize::new(0));
        let n = asked.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut b = vec![0; len];
                let _ = r.read_exact(&mut b);
                n.fetch_add(1, Ordering::SeqCst);
                let first = head.lines().next().unwrap_or("").to_string();
                check(&first, &head, &b);
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, asked)
    }

    const FAKE_CRED: &str = r#"{"claudeAiOauth":{"accessToken":"fake-access-1","refreshToken":"fake-refresh-1","expiresAt":1,"scopes":["user:profile","user:inference"],"subscriptionType":"max"},"mcpOAuth":{"x":{"accessToken":"fake-mcp"}}}"#;

    fn check_refresh(first: &str, head: &str, b: &[u8]) {
        assert!(first.starts_with("POST "), "{first}");
        assert!(
            head.to_lowercase()
                .contains("content-type: application/json"),
            "{head}"
        );
        let got: BTreeMap<String, String> = serde_json::from_slice(b).unwrap();
        for (k, v) in [
            ("grant_type", "refresh_token"),
            ("refresh_token", "fake-refresh-1"),
            ("client_id", CLIENT_ID),
            ("scope", "user:profile user:inference"),
        ] {
            assert_eq!(got.get(k).map(String::as_str), Some(v), "{k}");
        }
    }

    fn mem(cred: &str) -> Mutex<Vec<u8>> {
        Mutex::new(cred.as_bytes().to_vec())
    }

    // renew_test.go:58 (TestRenewWritesRotatedTokens): the refresh token
    // rotates: the new one, the new access token and its expiry are
    // written back, every other field as it was.
    #[test]
    fn renew_writes_rotated_tokens() {
        let (url, _) = server(
            200,
            r#"{"access_token":"fake-access-2","refresh_token":"fake-refresh-2","expires_in":3600}"#,
            check_refresh,
        );
        let store = mem(FAKE_CRED);
        renew_at(&url, &|| Ok(store.lock().unwrap().clone()), &|b| {
            *store.lock().unwrap() = b.to_vec();
            Ok(())
        })
        .unwrap();
        let got: serde_json::Value = serde_json::from_slice(&store.lock().unwrap()).unwrap();
        let o = &got["claudeAiOauth"];
        assert_eq!(o["accessToken"], "fake-access-2");
        assert_eq!(o["refreshToken"], "fake-refresh-2");
        assert_eq!(o["subscriptionType"], "max");
        assert_eq!(o["scopes"].as_array().unwrap().len(), 2);
        assert_eq!(got["mcpOAuth"]["x"]["accessToken"], "fake-mcp");
        let left = o["expiresAt"].as_i64().unwrap() - now();
        assert!(
            (59 * 60_000..=3_600_000).contains(&left),
            "expires in {left}ms"
        );
    }

    // renew_test.go:87 (TestRenewRevoked); and a refresh token Anthropic
    // refused isn't sent again, every ten minutes for as long as rush runs,
    // until the sign-in holds another. (Its own token: the refusal is kept
    // for the whole process.)
    #[test]
    fn renew_revoked_asks_once_per_refresh_token() {
        let (url, asked) = server(
            400,
            r#"{"error":"invalid_grant","error_description":"Refresh token not found or invalid"}"#,
            |_, _, _| {},
        );
        let cred = FAKE_CRED.replace("fake-refresh-1", "fake-refresh-refused");
        let store = mem(&cred);
        let renew = || {
            renew_at(&url, &|| Ok(store.lock().unwrap().clone()), &|b| {
                *store.lock().unwrap() = b.to_vec();
                Ok(())
            })
        };
        assert_eq!(renew(), Err(OauthError::Revoked));
        assert_eq!(*store.lock().unwrap(), cred.as_bytes());
        assert_eq!(renew(), Err(OauthError::Revoked));
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "a refused refresh token was sent again"
        );
        // Signed in again: the new sign-in is tried.
        *store.lock().unwrap() = cred
            .replace("fake-refresh-refused", "fake-refresh-new")
            .into_bytes();
        assert_eq!(renew(), Err(OauthError::Revoked));
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    // renew_test.go:96 (TestRenewRefusesFolderInUse): a Claude Code
    // running on the folder refreshes it itself: rush doesn't.
    #[test]
    fn renew_refuses_folder_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path());
        assert_eq!(renew_unless(&a, &|_| true), Err(OauthError::InUse));
    }

    // Which Claude Codes run on a folder: one naming it, or naming none
    // when it's ~/.claude. One whose environment can't be read (another
    // user's, an exited one) runs on none, so it never keeps an idle
    // login's expired sign-in from being refreshed.
    #[test]
    fn runs_on_only_a_readable_environment_naming_the_folder() {
        let home = Account::home().config_dir;
        let other = std::path::PathBuf::from("/x/rush/homes/abc/.claude");
        let named = |d: &str| Some(vec![format!("CLAUDE_CONFIG_DIR={d}"), "A=1".into()]);
        for (what, dir, envs, want) in [
            ("unreadable", &other, vec![None], false),
            ("unreadable, home", &home, vec![None], false),
            (
                "names it",
                &other,
                vec![None, named("/x/rush/homes/abc/.claude/")],
                true,
            ),
            ("names another", &other, vec![named("/x/elsewhere")], false),
            (
                "names none, home",
                &home,
                vec![Some(vec!["A=1".into()])],
                true,
            ),
            ("empty, home", &home, vec![Some(vec![])], true),
            ("names none, not home", &other, vec![Some(vec![])], false),
            ("none running", &other, vec![], false),
        ] {
            assert_eq!(runs_on(dir, envs), want, "{what}");
        }
    }

    // An exited Claude Code its parent hasn't reaped yet still lists as
    // claude, but runs on no folder.
    #[test]
    fn an_exited_claude_is_not_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude");
        // A copy, not a link: the listed name is the file run, not the link.
        std::fs::copy("/usr/bin/true", &bin).unwrap();
        let mut child = std::process::Command::new(&bin).spawn().unwrap();
        let pid = child.id() as i32;
        while !rush_sys::proc::zombie(pid) {
            std::thread::yield_now();
        }
        let a = Account::at("", dir.path());
        let listed = rush_sys::proc::snapshot()
            .procs
            .values()
            .any(|p| p.pid == pid && &*p.comm == "claude");
        let busy = in_use(&a);
        child.wait().unwrap();
        assert!(listed, "the exited claude isn't listed");
        assert!(!busy, "an exited claude counted as running on the folder");
    }

    // renew_test.go:107 (TestOrRenewOnceInTenMinutes)
    #[test]
    fn or_renew_once_in_ten_minutes() {
        let renews = AtomicUsize::new(0);
        for _ in 0..2 {
            let _ = or_renew(
                "login:test-once",
                || Err(OauthError::Expired),
                || {
                    renews.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            );
        }
        assert_eq!(renews.load(Ordering::SeqCst), 1);
    }

    // A sign-in that can't be refreshed says why until the next try, ten
    // minutes on: revoked, or in use by a running Claude Code (which
    // refreshes it itself), not "sign-in expired" on every reading but the
    // try's. A failure that says nothing of the sign-in (the network)
    // leaves it expired.
    #[test]
    fn or_renew_says_why_until_the_next_try() {
        for (key, err, want) in [
            (
                "login:test-says-revoked",
                OauthError::Revoked,
                OauthError::Revoked,
            ),
            (
                "login:test-says-in-use",
                OauthError::InUse,
                OauthError::InUse,
            ),
            (
                "login:test-says-network",
                other("refresh request failed"),
                OauthError::Expired,
            ),
        ] {
            let renews = AtomicUsize::new(0);
            for n in 0..3 {
                let got = or_renew_if(
                    true,
                    key,
                    || Err(OauthError::Expired),
                    || {
                        renews.fetch_add(1, Ordering::SeqCst);
                        Err(err.clone())
                    },
                );
                assert_eq!(got, Err(want.clone()), "{key}: reading {n}");
            }
            assert_eq!(renews.load(Ordering::SeqCst), 1, "{key}");
        }
    }

    // A rush that doesn't lead never refreshes a sign-in on its own, and a
    // dry run only writes down that it would.
    #[test]
    fn or_renew_only_leading_and_not_dry() {
        let renews = AtomicUsize::new(0);
        let got = or_renew_if(
            false,
            "login:test-follows",
            || Err(OauthError::Expired),
            || {
                renews.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert_eq!(got, Err(OauthError::Expired));
        let dir = tempfile::tempdir().unwrap();
        rush_core::dryrun::set_thread(Some((true, Some(dir.path().join("dry.log")))));
        let got = or_renew_if(
            true,
            "login:test-dry",
            || Err(OauthError::Expired),
            || {
                renews.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        rush_core::dryrun::set_thread(None);
        assert_eq!(got, Err(OauthError::Expired));
        assert_eq!(renews.load(Ordering::SeqCst), 0);
        let log = std::fs::read_to_string(dir.path().join("dry.log")).unwrap();
        assert!(
            log.contains("renew sign-in") && log.contains("login:test-dry"),
            "{log}"
        );
    }

    #[test]
    fn usage_and_owner_from_anthropic() {
        let cred = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"fake-usage-token","expiresAt":{}}}}}"#,
            now() + 3_600_000
        );
        let (url, _) = server(
            200,
            r#"{"five_hour":{"utilization":42,"resets_at":"2026-10-01T10:00:00Z"},"seven_day":null}"#,
            |first, head, _| {
                assert!(first.starts_with("GET /usage"), "{first}");
                assert!(
                    head.contains("Bearer fake-usage-token") && head.contains("oauth-2025-04-20"),
                    "{head}"
                );
            },
        );
        let u = fetch_usage_with_at(&url, cred.as_bytes()).unwrap();
        assert!(u.fetched && u.five_hour.present && !u.seven_day.present);
        assert_eq!(u.five_hour.percent, 42.0);

        let (url, asked) = server(200, r#"{"account":{"uuid":"u-9"}}"#, |first, _, _| {
            assert!(first.starts_with("GET /profile"))
        });
        let cred = cred.replace("fake-usage-token", "fake-owner-token");
        assert_eq!(owner_at(&url, cred.as_bytes()).unwrap(), "u-9");
        assert_eq!(owner_at(&url, cred.as_bytes()).unwrap(), "u-9");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "a token is asked about once"
        );

        let (url, _) = server(429, "{}", |_, _, _| {});
        let cred = cred.replace("fake-owner-token", "fake-limited");
        assert!(
            matches!(fetch_usage_with_at(&url, cred.as_bytes()), Err(OauthError::RateLimited(t)) if t >= now() + 14 * 60_000)
        );

        let old = r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":1}}"#;
        assert_eq!(
            fetch_usage_with_at(&url, old.as_bytes()),
            Err(OauthError::Expired)
        );
        assert_eq!(fetch_usage_with_at(&url, b"{}"), Err(OauthError::NoToken));
    }

    // A plan without a 5-hour limit, as Anthropic's usage endpoint sends
    // it: five_hour left out, null, or with no reset time. Each is no
    // 5-hour limit, never one 0% used; the weekly stands as sent. The
    // plan's name comes from the sign-in itself.
    #[test]
    fn usage_without_a_five_hour_limit() {
        let cred = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"fake-no5h-token","expiresAt":{},"subscriptionType":"max"}}}}"#,
            now() + 3_600_000
        );
        for (shape, body) in [
            (
                "absent",
                r#"{"seven_day":{"utilization":66,"resets_at":"2026-11-01T15:00:00Z"}}"#,
            ),
            (
                "null",
                r#"{"five_hour":null,"seven_day":{"utilization":66,"resets_at":"2026-11-01T15:00:00Z"}}"#,
            ),
            (
                "no reset",
                r#"{"five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":66,"resets_at":"2026-11-01T15:00:00Z"}}"#,
            ),
        ] {
            let (url, _) = server(200, body, |_, _, _| {});
            let u = fetch_usage_with_at(&url, cred.as_bytes()).unwrap();
            assert!(!u.five_hour.present, "{shape}: read as a 5-hour window");
            assert_eq!(u.five_hour, crate::usage::Window::default(), "{shape}");
            assert!(
                u.seven_day.present && u.seven_day.percent == 66.0,
                "{shape}"
            );
            assert_eq!(u.plan, "max", "{shape}");
            let q = u.quota("login:x");
            assert!(q.window("five_hour").is_none(), "{shape}");
            assert_eq!(q.used(""), 66.0, "{shape}: the weekly decides");
        }
        // One with the limit keeps it.
        let (url, _) = server(
            200,
            r#"{"five_hour":{"utilization":65,"resets_at":"2026-10-30T23:20:00Z"},"seven_day":{"utilization":100,"resets_at":"2026-10-31T16:00:00Z"}}"#,
            |_, _, _| {},
        );
        let u = fetch_usage_with_at(&url, cred.as_bytes()).unwrap();
        assert!(u.five_hour.present && u.five_hour.percent == 65.0);
        // No plan saved with the sign-in: none guessed.
        let bare = cred.replace(r#","subscriptionType":"max""#, "");
        let (url, _) = server(200, r#"{"five_hour":null}"#, |_, _, _| {});
        assert_eq!(fetch_usage_with_at(&url, bare.as_bytes()).unwrap().plan, "");
    }

    // renew_test.go:110 (TestAccessTokenMissing): a sign-in that lost its
    // token but kept its refresh token is expired, so it's refreshed; one
    // with neither, or that can't be read, says so rather than "not
    // signed in".
    #[test]
    fn access_token_missing() {
        for (cred, want) in [
            (
                r#"{"claudeAiOauth":{"refreshToken":"r"}}"#,
                OauthError::Expired,
            ),
            (
                r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"r"}}"#,
                OauthError::Expired,
            ),
            (
                r#"{"mcpOAuth":{"x":{"accessToken":"m"}}}"#,
                OauthError::NoToken,
            ),
            (r#"{"claudeAiOauth":"#, OauthError::Unreadable),
        ] {
            assert_eq!(access_token(cred.as_bytes()), Err(want), "{cred}");
            let err = owner(cred.as_bytes()).unwrap_err();
            assert_eq!(owner_err(err.clone()), err, "{cred}: its own words");
        }
    }

    // renew_test.go:131 (TestRenewGivesMissingToken): refreshing a sign-in
    // with no token gives it one.
    #[test]
    fn renew_gives_missing_token() {
        let (url, _) = server(
            200,
            r#"{"access_token":"fake-access-2","refresh_token":"fake-refresh-2","expires_in":3600}"#,
            check_refresh,
        );
        let store = mem(
            r#"{"claudeAiOauth":{"refreshToken":"fake-refresh-1","scopes":["user:profile","user:inference"]}}"#,
        );
        renew_at(&url, &|| Ok(store.lock().unwrap().clone()), &|b| {
            *store.lock().unwrap() = b.to_vec();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            access_token(&store.lock().unwrap()),
            Ok("fake-access-2".to_string())
        );
    }
}
