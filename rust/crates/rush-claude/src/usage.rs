//! An account's plan usage: as Claude Code caches it, as a session reports
//! it while it runs, as Anthropic gives it, and as rush shares it between
//! its processes in usage.json. Port of
//! internal/adapters/claude/claude/account.go:82-214 (Window, Usage,
//! LiveUsage, ReadUsage), quota.go, usagecache.go, and
//! adapters/claude/lastquota.go.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_core::time::{Ms, now, parse_rfc3339};
use rush_core::usage::{self, Quota, Reading, Scope, Source};
use rush_core::wire;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::account::Account;
use crate::files::{create_temp, mkdir_private};
use crate::json::nd;
use crate::login::signed_in_as;
use crate::oauth::{self, OauthError};

/// One of a plan's windows, as read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Window {
    pub present: bool,
    #[serde(with = "wire::float")]
    pub percent: f64,
    #[serde(with = "wire::time")]
    pub resets_at: Ms,
}

impl Window {
    /// A 5-hour window as read. Anthropic's top plan has no 5-hour limit:
    /// the usage endpoint then sends five_hour as null, or with no
    /// resets_at, and a session's report leaves it out or gives no
    /// resetsAt. Such a window is no 5-hour limit, not one 0% used, so it
    /// reads as absent. A 5-hour window in use always has its reset.
    pub fn five_hour(self) -> Window {
        if self.present && self.resets_at != 0 {
            self
        } else {
            Window::default()
        }
    }
}

/// A reading of an account's plan usage. Its JSON is Go's, field names
/// and all, as every rush process shares it through usage.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Usage {
    /// The signed-in account's uuid.
    #[serde(rename = "AccountID", deserialize_with = "nd")]
    pub account_id: String,
    #[serde(deserialize_with = "nd")]
    pub email: String,
    #[serde(deserialize_with = "nd")]
    pub org: String,
    #[serde(deserialize_with = "nd")]
    pub plan: String,
    #[serde(deserialize_with = "nd")]
    pub five_hour: Window,
    #[serde(deserialize_with = "nd")]
    pub seven_day: Window,
    #[serde(with = "wire::time")]
    pub fetched_at: Ms,
    /// Why no fresh reading: not signed in, expired, rate-limited.
    #[serde(deserialize_with = "nd")]
    pub problem: String,
    /// Read from Anthropic by rush, not Claude Code's cache.
    #[serde(deserialize_with = "nd")]
    pub fetched: bool,
    #[serde(deserialize_with = "nd")]
    pub role: String,
    #[serde(deserialize_with = "nd")]
    pub billing: String,
    #[serde(deserialize_with = "nd")]
    pub org_type: String,
    #[serde(deserialize_with = "nd")]
    pub extra: bool,
}

impl Usage {
    /// The fuller of the two windows, in percent: how close the account is
    /// to being stopped.
    pub fn used(&self) -> f64 {
        [self.five_hour, self.seven_day]
            .iter()
            .filter(|w| w.present)
            .fold(0.0, |p, w| p.max(w.percent))
    }

    /// Empties the windows that have reset since the reading was made:
    /// nothing has been used of them yet, as far as the reading knows.
    pub fn since(mut self, now: Ms) -> Usage {
        for w in [&mut self.five_hour, &mut self.seven_day] {
            if w.present && w.resets_at != 0 && now > w.resets_at {
                *w = Window {
                    present: true,
                    ..Window::default()
                };
            }
        }
        self
    }

    /// The reading as rush's own model of a plan's limits, kept under key.
    pub fn quota(&self, key: &str) -> Quota {
        let mut q = Quota {
            account: key.to_string(),
            plan: self.plan.clone(),
            fetched_at: self.fetched_at,
            problem: self.problem.clone(),
            source: if self.fetched {
                Source::Fetched
            } else {
                Source::Cached
            },
            ..Quota::default()
        };
        for (id, label, name, span, w, scope) in windows() {
            let w = match id {
                "five_hour" => self.five_hour,
                "seven_day" => self.seven_day,
                _ => w,
            };
            if !w.present {
                continue;
            }
            q.windows.push(usage::Window {
                id: id.into(),
                label: label.into(),
                name: name.into(),
                span,
                percent: w.percent,
                resets_at: w.resets_at,
                scope,
                ..usage::Window::default()
            });
        }
        q
    }

    /// The reading as rush's own reading of an account.
    pub fn reading(&self) -> Reading {
        Reading {
            account_id: self.account_id.clone(),
            email: self.email.clone(),
            org: self.org.clone(),
            plan: self.plan.clone(),
            role: self.role.clone(),
            billing: self.billing.clone(),
            org_type: self.org_type.clone(),
            extra: self.extra,
            windows: self.quota("").windows,
            fetched_at: self.fetched_at,
            problem: self.problem.clone(),
            fetched: self.fetched,
        }
    }
}

/// Claude's plan windows, by the names Anthropic gives them.
fn windows() -> [(
    &'static str,
    &'static str,
    &'static str,
    Duration,
    Window,
    Scope,
); 3] {
    let week = Duration::from_secs(7 * 24 * 3600);
    [
        (
            "five_hour",
            "5h",
            "5-hour",
            Duration::from_secs(5 * 3600),
            Window::default(),
            Scope::default(),
        ),
        (
            "seven_day",
            "7d",
            "weekly",
            week,
            Window::default(),
            Scope::default(),
        ),
        (
            "seven_day_opus",
            "Opus 7d",
            "Opus weekly",
            week,
            Window::default(),
            Scope {
                models: vec!["opus".into()],
            },
        ),
    ]
}

/// The plan usage a Claude Code session reports as it runs (a
/// rate_limit_event's info): the same windows the usage endpoint gives,
/// fresh with every request.
pub fn live_usage(info: &[u8], at: Ms) -> Option<Usage> {
    #[derive(Deserialize)]
    struct Raw {
        utilization: Option<f64>,
        #[serde(rename = "resetsAt", default)]
        resets_at: Option<i64>,
    }
    #[derive(Deserialize)]
    struct R {
        #[serde(rename = "unifiedWindows", default)]
        windows: Option<BTreeMap<String, Option<Raw>>>,
    }
    let r: R = serde_json::from_slice(info).ok()?;
    let mut u = Usage {
        fetched_at: at,
        fetched: true,
        ..Usage::default()
    };
    let ws = r.windows.unwrap_or_default();
    for (name, w) in [
        ("five_hour", &mut u.five_hour),
        ("seven_day", &mut u.seven_day),
    ] {
        if let Some(Some(raw)) = ws.get(name)
            && let Some(p) = raw.utilization
        {
            *w = Window {
                present: true,
                percent: p * 100.0,
                resets_at: raw
                    .resets_at
                    .filter(|&t| t > 0)
                    .map_or(0, |t| t.saturating_mul(1000)),
            };
        }
    }
    u.five_hour = u.five_hour.five_hour();
    (u.five_hour.present || u.seven_day.present).then_some(u)
}

/// A window as Anthropic writes it: a percentage and when it resets.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct RawWindow {
    #[serde(deserialize_with = "nd")]
    utilization: f64,
    #[serde(deserialize_with = "nd")]
    resets_at: String,
}

pub(crate) fn window_of(w: Option<RawWindow>) -> Window {
    match w {
        None => Window::default(),
        Some(w) => Window {
            present: true,
            percent: w.utilization,
            resets_at: parse_rfc3339(&w.resets_at),
        },
    }
}

/// The plan usage Claude Code already cached for this account. It never
/// touches credentials or the network.
pub fn read_usage(a: &Account) -> io::Result<Usage> {
    parse_usage_file(&fs::read(a.state_path())?)
}

fn parse_usage_file(b: &[u8]) -> io::Result<Usage> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct OAuth {
        #[serde(rename = "accountUuid", deserialize_with = "nd")]
        uuid: String,
        #[serde(rename = "emailAddress", deserialize_with = "nd")]
        email: String,
        #[serde(rename = "organizationName", deserialize_with = "nd")]
        org: String,
        #[serde(rename = "organizationRole", deserialize_with = "nd")]
        role: String,
        #[serde(rename = "organizationType", deserialize_with = "nd")]
        org_type: String,
        #[serde(rename = "hasExtraUsageEnabled", deserialize_with = "nd")]
        extra: bool,
        #[serde(rename = "billingType", deserialize_with = "nd")]
        billing: String,
        #[serde(rename = "seatTier", deserialize_with = "nd")]
        seat_tier: String,
        #[serde(rename = "userRateLimitTier", deserialize_with = "nd")]
        user_rate_limit: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Util {
        five_hour: Option<RawWindow>,
        seven_day: Option<RawWindow>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Cached {
        #[serde(rename = "fetchedAtMs", deserialize_with = "nd")]
        fetched_at_ms: i64,
        #[serde(deserialize_with = "nd")]
        utilization: Util,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct F {
        #[serde(rename = "oauthAccount")]
        oauth: Option<OAuth>,
        #[serde(rename = "cachedUsageUtilization")]
        cached: Option<Cached>,
    }
    let f: F = serde_json::from_slice(b).map_err(io::Error::other)?;
    let mut u = Usage::default();
    if let Some(o) = f.oauth {
        u.plan = [&o.user_rate_limit, &o.seat_tier, &o.org, &o.billing]
            .into_iter()
            .find(|v| !v.is_empty())
            .cloned()
            .unwrap_or_default();
        (u.account_id, u.email, u.org) = (o.uuid, o.email, o.org);
        (u.role, u.billing, u.org_type, u.extra) = (o.role, o.billing, o.org_type, o.extra);
    }
    if let Some(c) = f.cached {
        u.five_hour = window_of(c.utilization.five_hour).five_hour();
        u.seven_day = window_of(c.utilization.seven_day);
        u.fetched_at = c.fetched_at_ms;
    }
    Ok(u)
}

// ---- Readings every rush process shares (usagecache.go) ----

/// How old rush's last reading of an account may get before it asks
/// Anthropic again. Every rush process shares the readings, so restarts,
/// several views and --soak runs don't each ask.
pub const USAGE_EVERY: Ms = 5 * 60_000;

/// What asking Anthropic for plan usage is called where rush shows what
/// waits on the network.
pub const USAGE_JOB: &str = "Claude plan usage";

/// rush's last good reading of an account, and until when Anthropic asked
/// it not to ask again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FetchedUsage {
    #[serde(deserialize_with = "nd")]
    pub usage: Usage,
    #[serde(with = "wire::time", skip_serializing_if = "is_zero")]
    pub wait: Ms,
}

fn is_zero(t: &Ms) -> bool {
    *t == 0
}

/// The readings rush keeps at path, by config folder or login. A 5-hour
/// window kept with no reset time, as rush wrote one before it knew of
/// plans without that limit, reads as no 5-hour limit.
pub fn load_fetched_usage(path: &Path) -> BTreeMap<String, FetchedUsage> {
    let mut all: BTreeMap<String, FetchedUsage> = fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for f in all.values_mut() {
        f.usage.five_hour = f.usage.five_hour.five_hour();
    }
    all
}

/// Records one account's entry, keeping the others.
pub fn save_fetched_usage(path: &Path, key: &str, f: FetchedUsage) -> io::Result<()> {
    update_fetched_usage(path, |all| {
        all.insert(key.to_string(), f);
        true
    })
}

/// Keeps a reading made elsewhere (a session's, as it runs) under key,
/// unless the one there is newer; when Anthropic had said to wait, the
/// wait stands.
pub fn record_usage(path: &Path, key: &str, u: Usage) -> io::Result<()> {
    update_fetched_usage(path, |all| {
        let f = all.entry(key.to_string()).or_default();
        if u.fetched_at <= f.usage.fetched_at {
            return false;
        }
        f.usage = u;
        true
    })
}

/// Whose the sign-in a folder holds is, asked of Anthropic once per
/// token: tests give their own.
pub type OwnerOf = dyn Fn(&Account) -> Result<String, OauthError>;

/// Keeps a running Claude Code's own reading of its plan usage as
/// started_as's, the login a's folder was signed in as when it started,
/// but only while the folder still is: once it's switched to another,
/// Claude Code picks up the new sign-in as it goes, and whose reading it
/// sent can't be told, so it's left out rather than written onto the
/// wrong login. The folder's name alone won't do: a Claude Code started as
/// started_as writes its name back over a switch, beside the other login's
/// sign-in it has since picked up, so the sign-in has to be started_as's
/// too.
pub fn record_live_usage(path: &Path, a: &Account, started_as: &str, u: Usage) -> io::Result<()> {
    record_live_usage_with(path, a, started_as, u, &|a| {
        oauth::owner(&crate::login::read_creds(a)?)
    })
}

pub fn record_live_usage_with(
    path: &Path,
    a: &Account,
    started_as: &str,
    mut u: Usage,
    owner: &OwnerOf,
) -> io::Result<()> {
    if started_as.is_empty() || signed_in_as(a) != started_as {
        return Ok(());
    }
    match owner(a) {
        Ok(id) if id == started_as => {}
        _ => return Ok(()),
    }
    u.account_id = started_as.to_string();
    record_usage(path, &login_key(started_as), u)
}

/// Where a login's readings are kept.
pub fn login_key(id: &str) -> String {
    format!("login:{id}")
}

/// Changes the readings at path under a lock: every rush process and
/// every session's host writes them.
fn update_fetched_usage(
    path: &Path,
    change: impl FnOnce(&mut BTreeMap<String, FetchedUsage>) -> bool,
) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    mkdir_private(dir)?;
    let _lock = lock_file(&with_suffix(path, ".lock"));
    let mut all = load_fetched_usage(path);
    if !change(&mut all) {
        return Ok(());
    }
    let b = serde_json::to_vec(&all).map_err(io::Error::other)?;
    let (tmp, mut f) = create_temp(dir, ".usage-")?;
    if let Err(e) = f.write_all(&b) {
        drop(f);
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    drop(f);
    fs::rename(&tmp, path)
}

/// An exclusive lock (flock, as Go's) on the file at path, held until
/// dropped; without one it holds nothing.
fn lock_file(path: &Path) -> Option<fs::File> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .ok()?;
    let _ = f.lock();
    Some(f)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// An account's plan usage, asked of Anthropic only when the reading
/// shared through path is older than USAGE_EVERY and Anthropic hasn't said
/// to wait; offline never asks. Every rush process goes through here, so
/// however many are open an account is asked once per USAGE_EVERY.
/// Readings are kept by the login the folder is signed in as, so a
/// session's own readings and the login's fetch are the same one.
pub fn refresh_usage(path: &Path, a: &Account, offline: bool) -> Usage {
    let who = signed_in_as(a);
    let key = if who.is_empty() {
        a.config_dir.to_string_lossy().into_owned()
    } else {
        login_key(&who)
    };
    let k = key.clone();
    refresh_usage_for(path, &key, offline, move || {
        let u = oauth::or_renew(&k, || oauth::fetch_usage(a), || oauth::renew(a))?;
        if !who.is_empty() && u.account_id != who {
            // Its sign-in is another account's than it says: the reading
            // isn't who's. Finding logins puts it right.
            return Err(OauthError::Other(
                "signed in as another account than it says".into(),
            ));
        }
        Ok(u)
    })
}

/// Where a reading of the account a is signed in as is kept.
pub fn usage_key(a: &Account, u: &Usage) -> String {
    if !u.account_id.is_empty() {
        return login_key(&u.account_id);
    }
    a.config_dir.to_string_lossy().into_owned()
}

/// refresh_usage for readings kept under key, fetched by fetch.
pub fn refresh_usage_for(
    path: &Path,
    key: &str,
    offline: bool,
    fetch: impl FnOnce() -> Result<Usage, OauthError>,
) -> Usage {
    // One asks at a time, so the others find its reading.
    let _lock = (!offline).then(|| lock_key(path, key));
    let f = load_fetched_usage(path).remove(key).unwrap_or_default();
    let mut u = f.usage.clone();
    let now = now();
    if now < f.wait {
        u.problem = format!("rate-limited to {}", clock(f.wait));
    }
    if offline || now < f.wait || now - u.fetched_at < USAGE_EVERY {
        return u;
    }
    if !rush_sys::events::net_run(USAGE_JOB) {
        return u; // the network's down: the last reading, until it's back
    }
    let got = fetch();
    rush_sys::events::net_done(USAGE_JOB);
    match got {
        Ok(got) => {
            let _ = record_usage(path, key, got.clone());
            got
        }
        Err(OauthError::RateLimited(until)) => {
            let _ = save_fetched_usage(
                path,
                key,
                FetchedUsage {
                    usage: f.usage,
                    wait: until,
                },
            );
            rush_sys::events::log(
                "usage-backoff",
                json!({"account": key, "until": rush_core::time::format_rfc3339(until), "why": "Anthropic rate-limited the usage ask"}),
            );
            u.problem = format!("rate-limited to {}", clock(until));
            u
        }
        Err(e) => {
            u.problem = e.to_string();
            u
        }
    }
}

/// Holds the lock on asking Anthropic about key, across every rush
/// process, until dropped.
fn lock_key(path: &Path, key: &str) -> Option<fs::File> {
    use sha2::Digest;
    let sum = sha2::Sha256::digest(key.as_bytes());
    let dir = with_suffix(path, ".locks");
    mkdir_private(&dir).ok()?;
    lock_file(&dir.join(crate::files::hex(&sum[..8])))
}

/// t as the local time of day, 15:04.
pub(crate) fn clock(t: Ms) -> String {
    let secs = t.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r only writes the tm we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// The readings shared through path, by key; one Anthropic said to wait
/// for says until when.
pub fn readings(path: &Path) -> BTreeMap<String, Reading> {
    let now = now();
    load_fetched_usage(path)
        .into_iter()
        .map(|(k, mut f)| {
            if now < f.wait {
                f.usage.problem = format!("rate-limited to {}", clock(f.wait));
            }
            (k, f.usage.reading())
        })
        .collect()
}

/// The newest reading rush keeps in usage.json for the folder dir, or for
/// the login it's signed in as, since rush reads them per login.
pub fn last_quota(path: &Path, dir: &str) -> Option<Quota> {
    let all = load_fetched_usage(path);
    let mut f = all.get(dir).cloned();
    let id = signed_in_as(&Account::at("", dir));
    if !id.is_empty()
        && let Some(g) = all.get(&login_key(&id))
        && g.usage.fetched_at > f.as_ref().map_or(0, |f| f.usage.fetched_at)
    {
        f = Some(g.clone());
    }
    let f = f?;
    Some(f.usage.since(now()).quota(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    // quota_test.go (TestUsageAsQuota)
    #[test]
    fn usage_as_quota() {
        let reset = parse_rfc3339("2026-09-26T17:00:00Z");
        let u = Usage {
            plan: "max".into(),
            five_hour: Window {
                present: true,
                percent: 62.0,
                resets_at: reset,
            },
            seven_day: Window {
                present: true,
                percent: 20.0,
                resets_at: 0,
            },
            fetched: true,
            ..Usage::default()
        };
        let q = u.quota("login:abc");
        assert_eq!(
            (q.account.as_str(), q.plan.as_str(), q.windows.len()),
            ("login:abc", "max", 2)
        );
        let w = q.window("five_hour").unwrap();
        assert_eq!(
            (w.percent, w.resets_at, w.span),
            (62.0, reset, Duration::from_secs(5 * 3600))
        );
        assert_eq!(q.used("claude-sonnet-5"), u.used());
        let one = Usage {
            seven_day: Window {
                present: true,
                ..Window::default()
            },
            ..Usage::default()
        };
        assert_eq!(
            one.quota("").windows.len(),
            1,
            "a missing window became a window"
        );
    }

    // usagecache_test.go:11 (TestFetchedUsageKeepsOtherAccounts)
    #[test]
    fn fetched_usage_keeps_other_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let at = now() / 1000 * 1000;
        let u = Usage {
            fetched_at: at,
            seven_day: Window {
                present: true,
                percent: 50.0,
                resets_at: 0,
            },
            ..Usage::default()
        };
        save_fetched_usage(&path, "/a", FetchedUsage { usage: u, wait: 0 }).unwrap();
        let wait = at + 3_600_000;
        save_fetched_usage(
            &path,
            "/b",
            FetchedUsage {
                wait,
                ..FetchedUsage::default()
            },
        )
        .unwrap();
        let all = load_fetched_usage(&path);
        assert_eq!(all["/a"].usage.seven_day.percent, 50.0);
        assert_eq!(all["/a"].usage.fetched_at, at);
        assert_eq!(all["/b"].wait, wait);
        // Go's names, as Go reads them.
        let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["/a"]["usage"]["SevenDay"]["Percent"], 50);
        assert_eq!(raw["/a"]["usage"]["FiveHour"]["ResetsAt"], wire::ZERO_TIME);
        assert!(raw["/a"].get("wait").is_none(), "a zero wait is left out");
    }

    // A file as Go rush writes it reads back the same.
    #[test]
    fn reads_go_usage_json() {
        let b = br#"{"login:u":{"usage":{"AccountID":"u","Email":"","Org":"","Plan":"max","FiveHour":{"Present":true,"Percent":12.5,"ResetsAt":"2026-10-29T15:00:00.123456789+01:00"},"SevenDay":{"Present":false,"Percent":0,"ResetsAt":"0001-01-01T00:00:00Z"},"FetchedAt":"2026-10-29T10:00:00Z","Problem":"","Fetched":true,"Role":"","Billing":"","OrgType":"","Extra":false},"wait":"2026-10-29T11:00:00Z"}}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        fs::write(&path, b).unwrap();
        let f = &load_fetched_usage(&path)["login:u"];
        assert_eq!(f.usage.five_hour.percent, 12.5);
        assert_eq!(
            f.usage.five_hour.resets_at,
            parse_rfc3339("2026-10-29T14:00:00.123Z")
        );
        assert_eq!(f.usage.seven_day.resets_at, 0);
        assert_eq!(f.wait, parse_rfc3339("2026-10-29T11:00:00Z"));
    }

    // usagecache_test.go:31 (TestUsageSinceEmptiesResetWindows)
    #[test]
    fn usage_since_empties_reset_windows() {
        let n = now();
        let u = Usage {
            five_hour: Window {
                present: true,
                percent: 3.0,
                resets_at: n - 3_600_000,
            },
            seven_day: Window {
                present: true,
                percent: 1.0,
                resets_at: n + 48 * 3_600_000,
            },
            ..Usage::default()
        }
        .since(n);
        assert_eq!(
            u.five_hour,
            Window {
                present: true,
                ..Window::default()
            }
        );
        assert!(u.seven_day.present && u.seven_day.percent == 1.0);
    }

    // usagecache_test.go:46 (TestLiveUsage)
    #[test]
    fn live() {
        let info = br#"{"status":"allowed","resetsAt":1790368200,"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.96,"resetsAt":1790368200},"seven_day":{"utilization":0.26,"resetsAt":1790521200}}}"#;
        let at = now();
        let u = live_usage(info, at).unwrap();
        assert!(u.five_hour.present);
        assert_eq!((u.five_hour.percent + 0.5) as i64, 96);
        assert_eq!((u.seven_day.percent + 0.5) as i64, 26);
        assert_eq!(
            (u.five_hour.resets_at, u.fetched_at),
            (1_790_368_200_000, at)
        );
        assert!(live_usage(br#"{"status":"rejected","resetsAt":1790368200}"#, at).is_none());
    }

    // usagecache_test.go:62 (TestRecordUsageKeepsNewer)
    #[test]
    fn record_usage_keeps_newer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let n = now() / 1000 * 1000;
        let wait = n + 3_600_000;
        let at = |t: Ms, p| Usage {
            fetched_at: t,
            five_hour: Window {
                present: true,
                percent: p,
                resets_at: t + 3_600_000,
            },
            ..Usage::default()
        };
        save_fetched_usage(
            &path,
            "k",
            FetchedUsage {
                usage: at(n, 50.0),
                wait,
            },
        )
        .unwrap();
        record_usage(&path, "k", at(n - 60_000, 10.0)).unwrap();
        assert_eq!(
            load_fetched_usage(&path)["k"].usage.five_hour.percent,
            50.0,
            "an older reading replaced a newer"
        );
        record_usage(&path, "k", at(n + 60_000, 60.0)).unwrap();
        let f = &load_fetched_usage(&path)["k"];
        assert_eq!(
            (f.usage.five_hour.percent, f.wait),
            (60.0, wait),
            "the wait is kept"
        );
    }

    fn sign_in(a: &Account, id: &str) {
        fs::create_dir_all(&a.config_dir).unwrap();
        fs::write(
            a.state_path(),
            format!(r#"{{"oauthAccount":{{"accountUuid":"{id}"}}}}"#),
        )
        .unwrap();
    }

    // usagecache_test.go:83 (TestRecordLiveUsageOnlyWhileStillSignedIn)
    #[test]
    fn record_live_usage_only_while_still_signed_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let a = Account::at("", dir.path().join("cfg"));
        let owner: &OwnerOf = &|_| Ok("alex".into());
        let n = now();
        let at = |t: Ms, p| Usage {
            fetched_at: t,
            five_hour: Window {
                present: true,
                percent: p,
                resets_at: t + 3_600_000,
            },
            ..Usage::default()
        };
        sign_in(&a, "alex");
        record_live_usage_with(&path, &a, "alex", at(n, 10.0), owner).unwrap();
        sign_in(&a, "borrowed");
        record_live_usage_with(&path, &a, "alex", at(n + 60_000, 84.0), owner).unwrap();
        let all = load_fetched_usage(&path);
        let got = &all[&login_key("alex")].usage;
        assert_eq!(
            (got.five_hour.percent, got.account_id.as_str()),
            (10.0, "alex")
        );
        assert!(!all.contains_key(&login_key("borrowed")));
    }

    // usagecache_test.go:118 (TestRecordLiveUsageNotOnAnotherLoginsSignIn)
    #[test]
    fn record_live_usage_not_on_another_logins_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let a = Account::at("", dir.path().join("cfg"));
        sign_in(&a, "personal");
        let u = Usage {
            fetched_at: now(),
            five_hour: Window {
                present: true,
                percent: 53.0,
                resets_at: 0,
            },
            ..Usage::default()
        };
        record_live_usage_with(&path, &a, "personal", u, &|_| Ok("alex".into())).unwrap();
        assert!(!load_fetched_usage(&path).contains_key(&login_key("personal")));
    }

    #[test]
    fn refresh_asks_only_when_old() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let fresh = Usage {
            fetched_at: now(),
            plan: "max".into(),
            ..Usage::default()
        };
        record_usage(&path, "k", fresh).unwrap();
        let got = refresh_usage_for(&path, "k", false, || panic!("asked while fresh"));
        assert_eq!(got.plan, "max");
        let got = refresh_usage_for(&path, "old", false, || {
            Err(OauthError::RateLimited(now() + 3_600_000))
        });
        assert!(
            got.problem.starts_with("rate-limited to "),
            "{}",
            got.problem
        );
        let again = refresh_usage_for(&path, "old", false, || panic!("asked while told to wait"));
        assert!(again.problem.starts_with("rate-limited to "));
        let off = refresh_usage_for(&path, "other", true, || panic!("asked offline"));
        assert_eq!(off, Usage::default());
        let got = refresh_usage_for(&path, "new", false, || {
            Ok(Usage {
                fetched_at: now(),
                plan: "pro".into(),
                ..Usage::default()
            })
        });
        assert_eq!(got.plan, "pro");
        assert_eq!(load_fetched_usage(&path)["new"].usage.plan, "pro");
    }

    // The three shapes a reading of an account takes now Anthropic's top
    // plan has no 5-hour limit, as rush kept them in usage.json: the
    // window not there; there but with no reset time (the API sent it with
    // a null resets_at); and a real one. The first two are no 5-hour
    // limit, the weekly alone deciding; the third keeps its window.
    #[test]
    fn readings_without_a_five_hour_limit() {
        let b = br#"{
"login:a4e79fcb":{"usage":{"AccountID":"a4e79fcb","Plan":"","FiveHour":{"Present":false,"Percent":0,"ResetsAt":"0001-01-01T00:00:00Z"},"SevenDay":{"Present":true,"Percent":66,"ResetsAt":"2099-11-01T16:00:00+01:00"},"FetchedAt":"2026-10-31T14:15:11+01:00","Fetched":true}},
"login:e8b97b0d":{"usage":{"AccountID":"e8b97b0d","Plan":"","FiveHour":{"Present":true,"Percent":0,"ResetsAt":"0001-01-01T00:00:00Z"},"SevenDay":{"Present":true,"Percent":100,"ResetsAt":"2099-10-31T07:00:00Z"},"FetchedAt":"2026-10-30T23:28:45+01:00","Fetched":true}},
"login:a9ac2531":{"usage":{"AccountID":"a9ac2531","Plan":"","FiveHour":{"Present":true,"Percent":65,"ResetsAt":"2099-10-30T23:20:00Z"},"SevenDay":{"Present":true,"Percent":100,"ResetsAt":"2099-10-31T16:00:00Z"},"FetchedAt":"2026-10-30T23:26:41+01:00","Fetched":true}}}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        fs::write(&path, b).unwrap();
        let all = load_fetched_usage(&path);
        for (id, week) in [("a4e79fcb", 66.0), ("e8b97b0d", 100.0)] {
            let u = &all[&login_key(id)].usage;
            assert!(!u.five_hour.present, "{id}: a 5-hour window");
            assert_eq!(u.used(), week, "{id}");
            let q = u.quota("");
            assert!(q.window("five_hour").is_none(), "{id}");
            assert_eq!(q.windows.len(), 1, "{id}");
            assert_eq!(q.used(""), week, "{id}");
        }
        let u = &all[&login_key("a9ac2531")].usage;
        assert!(u.five_hour.present && u.five_hour.percent == 65.0);
        assert_eq!(u.quota("").windows.len(), 2);
        // Shared readings, as every screen gets them.
        let r = readings(&path);
        assert!(
            r[&login_key("e8b97b0d")]
                .windows
                .iter()
                .all(|w| w.id != "five_hour")
        );
        assert_eq!(r[&login_key("a9ac2531")].windows[0].id, "five_hour");
        // A 5-hour window that has reset since its reading is still one:
        // used 0%, not gone.
        let n = now();
        let reset = Usage {
            five_hour: Window {
                present: true,
                percent: 65.0,
                resets_at: n - 60_000,
            },
            ..Usage::default()
        }
        .since(n);
        assert!(reset.quota("").window("five_hour").is_some());
    }

    // Claude Code's own cache, and a session's report, with no 5-hour
    // limit: none read.
    #[test]
    fn cache_and_live_without_a_five_hour_limit() {
        for b in [
            &br#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"seven_day":{"utilization":66,"resets_at":"2026-11-01T15:00:00Z"}}}}"#[..],
            br#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"five_hour":null,"seven_day":{"utilization":66,"resets_at":"2026-11-01T15:00:00Z"}}}}"#,
            br#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":66,"resets_at":"2026-11-01T15:00:00Z"}}}}"#,
        ] {
            let u = parse_usage_file(b).unwrap();
            assert!(!u.five_hour.present && u.seven_day.percent == 66.0);
        }
        let at = now();
        for info in [
            &br#"{"status":"allowed","unifiedWindows":{"seven_day":{"utilization":0.66,"resetsAt":1790521200}}}"#[..],
            br#"{"status":"allowed","unifiedWindows":{"five_hour":null,"seven_day":{"utilization":0.66,"resetsAt":1790521200}}}"#,
            br#"{"status":"allowed","unifiedWindows":{"five_hour":{"utilization":0},"seven_day":{"utilization":0.66,"resetsAt":1790521200}}}"#,
        ] {
            let u = live_usage(info, at).unwrap();
            assert!(!u.five_hour.present && u.seven_day.present);
        }
    }

    #[test]
    fn reads_claude_codes_cache() {
        let b = br#"{"oauthAccount":{"accountUuid":"u","emailAddress":"me@x","organizationName":"Org","billingType":"stripe","seatTier":"","userRateLimitTier":"default_max"},"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"five_hour":{"utilization":40,"resets_at":"2026-10-01T10:00:00Z"}}}}"#;
        let u = parse_usage_file(b).unwrap();
        assert_eq!(
            (u.account_id.as_str(), u.plan.as_str(), u.billing.as_str()),
            ("u", "default_max", "stripe")
        );
        assert_eq!((u.five_hour.percent, u.fetched_at), (40.0, 1000));
        assert!(!u.seven_day.present);
    }
}
