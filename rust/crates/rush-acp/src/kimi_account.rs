//! Kimi's account: its config, the credential of the provider it runs on,
//! what rush can say of it, and the Kimi Code plan's limits as Kimi's own
//! /usage reads them. Port of internal/adapters/acp/kimi_account.go:1-360.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rush_core::adapter::{
    self, Account, AccountSummary, AccountSummaryReader, Profile, QuotaSource,
};
use rush_core::time::{Ms, now, parse_rfc3339};
use rush_core::usage::{Quota, Source, Window};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::kimi::{Kimi, kimi_home};
use crate::rpc::Error;
use crate::util::env;

/// What rush reads of Kimi's config.toml (or its older config.json).
#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct KimiConfig {
    pub default_model: String,
    pub models: HashMap<String, KimiModel>,
    pub providers: HashMap<String, KimiProvider>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct KimiModel {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub max_context_size: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct KimiProvider {
    #[serde(rename = "type")]
    pub kind: String,
    pub base_url: String,
    pub api_key: String,
    pub oauth: Option<KimiOAuth>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct KimiOAuth {
    pub key: String,
    pub storage: String,
}

pub(crate) fn read_kimi_config(dir: &str) -> Result<KimiConfig, String> {
    let dir = Path::new(dir);
    match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(b) => return toml::from_str(&b).map_err(|e| e.to_string()),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
        Err(_) => {}
    }
    let b = std::fs::read(dir.join("config.json")).map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}

/// The credential of the provider Kimi runs on: its default model's, else
/// the first model's that has one. token is what goes in the Authorization
/// header; it is never shown.
#[derive(Default, Debug)]
pub(crate) struct KimiCred {
    pub provider: String,
    pub method: String,
    pub base_url: String,
    pub token: String,
    pub refreshable: bool,
    /// An OAuth access token past its expiry; Kimi refreshes it on its
    /// next run.
    pub expired: bool,
}

pub(crate) fn kimi_credential(dir: &str) -> KimiCred {
    let Ok(cfg) = read_kimi_config(dir) else {
        return KimiCred::default();
    };
    let mut names: Vec<&String> = cfg.models.keys().collect();
    names.sort();
    if cfg.models.contains_key(&cfg.default_model) {
        names = vec![&cfg.default_model]; // another model's account is not the active one
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Token {
        access_token: String,
        refresh_token: String,
        expires_at: f64,
    }
    for name in names {
        let key = &cfg.models[name].provider;
        let Some(prov) = cfg.providers.get(key) else {
            continue;
        };
        let mut c = KimiCred {
            provider: key.clone(),
            base_url: prov.base_url.clone(),
            ..KimiCred::default()
        };
        if !prov.api_key.is_empty() {
            c.method = "API key · Kimi config".into();
            c.token = prov.api_key.clone();
            return c;
        }
        let Some(r) = prov.oauth.as_ref().filter(|r| !r.key.is_empty()) else {
            continue;
        };
        let file = Path::new(dir)
            .join("credentials")
            .join(format!("{}.json", crate::util::base(&r.key)));
        c.method = "OAuth · Kimi credentials file".into();
        let mut b = std::fs::read(file);
        if b.is_err() && r.storage == "keyring" {
            b = rush_state::vault::keychain::read("kimi-code", &r.key);
            c.method = "OAuth · system keychain".into();
        }
        let Ok(b) = b else {
            continue;
        };
        let Ok(tok) = serde_json::from_slice::<Token>(&b) else {
            continue;
        };
        if tok.access_token.is_empty() && tok.refresh_token.is_empty() {
            continue;
        }
        c.refreshable = !tok.refresh_token.is_empty();
        c.expired = tok.access_token.is_empty()
            || (tok.expires_at > 0.0 && (tok.expires_at as i64) * 1000 < now());
        c.token = tok.access_token;
        if c.token.is_empty() {
            c.token = "refresh-only".into(); // signed in; Kimi mints an access token when it next runs
        }
        return c;
    }
    KimiCred::default()
}

/// The endpoints Kimi's managed providers use, by name.
fn platform(provider: &str) -> &'static str {
    match provider {
        "managed:kimi-code" => "Kimi Code",
        "managed:moonshot-cn" => "Moonshot AI (moonshot.cn)",
        "managed:moonshot-ai" => "Moonshot AI (moonshot.ai)",
        _ => "",
    }
}

/// A URL's scheme and host, as Go's url.Parse gives them; None when it has
/// no host.
pub(crate) fn origin(u: &str) -> Option<(String, String)> {
    let (scheme, rest) = u.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or(host);
    if scheme.is_empty() || host.is_empty() {
        return None;
    }
    Some((scheme.to_string(), host.to_string()))
}

impl AccountSummaryReader for Kimi {
    /// The provider Kimi runs on and how it signs in. Kimi keeps no name or
    /// email on disk; the plan's limits are its Quota.
    fn account_summary(&self, p: &Profile) -> AccountSummary {
        let dir = if p.dir.is_empty() {
            kimi_home()
        } else {
            p.dir.clone()
        };
        let c = kimi_credential(&dir);
        if c.token.is_empty() {
            return AccountSummary {
                status: "unknown".into(),
                note: "Rush could not verify Kimi credentials in this config home. Kimi may use credentials that Rush cannot read; this does not establish that you are signed out.".into(),
                ..AccountSummary::default()
            };
        }
        let mut s = AccountSummary {
            signed_in: true,
            status: "credentials available".into(),
            method: c.method.clone(),
            plan: platform(&c.provider).into(),
            ..AccountSummary::default()
        };
        if s.plan.is_empty() {
            s.plan = c.provider.clone();
        }
        if let Some((scheme, host)) = origin(&c.base_url) {
            s.endpoint = format!("{scheme}://{host}");
        }
        s.note = "Kimi doesn't keep the account's name or email locally.".into();
        if c.expired {
            if c.refreshable {
                s.note =
                    "Access token expired; a refresh credential is available for Kimi to renew it."
                        .into();
            } else {
                s.signed_in = false;
                s.status = "access token expired".into();
                s.note = "Stored access token expired and no refresh credential was found. Check sign-in in Kimi.".into();
            }
        }
        s
    }
}

/// Where Kimi Code says what's left of the plan: only Kimi Code has one.
fn usage_url(c: &KimiCred) -> Option<String> {
    if c.provider != "managed:kimi-code" {
        return None;
    }
    let mut base = env("KIMI_CODE_BASE_URL");
    if base.is_empty() {
        base = c.base_url.clone();
    }
    if base.is_empty() {
        base = "https://api.kimi.com/coding/v1".into();
    }
    Some(format!("{}/usages", base.trim_end_matches('/')))
}

fn fail(text: impl Into<String>) -> adapter::Error {
    Box::new(Error::other(text))
}

impl QuotaSource for Kimi {
    /// The Kimi Code plan's limits, as Kimi's own /usage reads them. Asking
    /// spends nothing.
    fn quota(&self, p: &Profile, _: &Account) -> adapter::Result<Quota> {
        let dir = if p.dir.is_empty() {
            kimi_home()
        } else {
            p.dir.clone()
        };
        let c = kimi_credential(&dir);
        if c.token.is_empty() {
            return Err(fail("Kimi has no credentials: run kimi login"));
        }
        let Some(u) = usage_url(&c) else {
            return Err(fail("Kimi reports limits only for Kimi Code plans"));
        };
        if c.expired {
            return Err(fail(
                "Kimi's access token expired: run kimi once to refresh it",
            ));
        }
        let res = crate::http::get(&u, &c.token).map_err(fail)?;
        match res.code {
            401 => return Err(fail("Kimi refused its credentials: run kimi login")),
            200 => {}
            _ => return Err(fail(format!("Kimi usage: {}", res.status))),
        }
        kimi_quota_of(&res.body, now()).map_err(fail)
    }
}

/// A number Kimi may send as a string.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Num {
    v: f64,
    ok: bool,
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Num, D::Error> {
        let v = Value::deserialize(d)?;
        let parsed = match &v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        };
        Ok(Num {
            v: parsed.unwrap_or(0.0),
            ok: parsed.is_some(),
        })
    }
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Limit {
    name: String,
    title: String,
    scope: String,
    limit: Num,
    used: Num,
    remaining: Num,
    duration: Num,
    #[serde(rename = "timeUnit")]
    time_unit: String,
    reset_at: String,
    #[serde(rename = "resetAt")]
    reset_at2: String,
    reset_time: String,
    #[serde(rename = "resetTime")]
    reset_time2: String,
    reset_in: Num,
    #[serde(rename = "resetIn")]
    reset_in2: Num,
}

fn first<'a>(ss: &[&'a str]) -> &'a str {
    ss.iter()
        .copied()
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

pub(crate) fn kimi_quota_of(body: &[u8], now: Ms) -> Result<Quota, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Membership {
        level: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct User {
        membership: Membership,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        user: User,
        usage: Option<Limit>,
        limits: Vec<Value>,
    }
    let r: R = serde_json::from_slice(body).map_err(|e| format!("Kimi usage: {e}"))?;
    let mut q = Quota {
        plan: "Kimi Code".into(),
        fetched_at: now,
        source: Source::Fetched,
        ..Quota::default()
    };
    let lvl = r.user.membership.level;
    let lvl = lvl.strip_prefix("LEVEL_").unwrap_or(&lvl);
    if !lvl.is_empty() {
        // The first letter up, the rest down; a level that starts with a
        // letter of more than one byte is cut at its end, not inside it.
        let first = lvl.chars().next().map_or(0, char::len_utf8);
        q.plan += &format!(
            " · {}{}",
            lvl[..first].to_uppercase(),
            lvl[first..].to_lowercase()
        );
    }
    if let Some(u) = &r.usage
        && let Some(mut w) = window(u, &Limit::default(), now)
    {
        if w.name.is_empty() {
            w.id = "weekly".into();
            w.label = "7d".into();
            w.name = "weekly".into();
            w.span = Duration::from_secs(7 * 24 * 3600);
        }
        q.windows.push(w);
    }
    for (i, item) in r.limits.iter().enumerate() {
        let l: Limit = serde_json::from_value(item.clone()).unwrap_or_default();
        let sub = |k: &str| -> Option<Limit> {
            item.get(k)
                .filter(|v| !v.is_null())
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        };
        let mut d = sub("detail").unwrap_or_else(|| l.clone());
        let mut win = sub("window").unwrap_or_default();
        if win.duration == Num::default() {
            win.duration = l.duration;
            win.time_unit = l.time_unit.clone();
        }
        if d.name.is_empty() && d.title.is_empty() && d.scope.is_empty() {
            d.name = first(&[&l.name, &l.title, &l.scope]).to_string();
        }
        if let Some(mut w) = window(&d, &win, now) {
            if w.name.is_empty() {
                w.id = format!("limit_{}", i + 1);
                w.label = format!("#{}", i + 1);
                w.name = format!("limit {}", i + 1);
            }
            q.windows.push(w);
        }
    }
    Ok(q)
}

/// One of Kimi's limits as a rush window; win carries its duration when
/// Kimi sends one.
fn window(d: &Limit, win: &Limit, now: Ms) -> Option<Window> {
    if !d.limit.ok || d.limit.v <= 0.0 {
        return None;
    }
    let used = if d.used.ok {
        d.used.v
    } else if d.remaining.ok {
        d.limit.v - d.remaining.v
    } else {
        return None;
    };
    let mut w = Window {
        used,
        limit: d.limit.v,
        percent: (100.0 * used / d.limit.v).clamp(0.0, 100.0),
        ..Window::default()
    };
    if win.duration.ok && win.duration.v > 0.0 {
        let n = win.duration.v as u64;
        let u = win.time_unit.to_uppercase();
        // A duration past reason saturates rather than overflowing.
        w.span = Duration::from_secs(n.saturating_mul(if u.contains("MINUTE") {
            60
        } else if u.contains("HOUR") {
            3600
        } else if u.contains("DAY") {
            24 * 3600
        } else {
            1
        }));
        w.label = span(w.span);
        w.id = format!("window_{}", w.label);
        w.name = w.label.clone();
    }
    let name = first(&[&d.name, &d.title, &d.scope]);
    if !name.is_empty() {
        w.name = name.to_string();
        if w.label.is_empty() {
            w.label = name.to_string();
            w.id = name.replace(' ', "_").to_lowercase();
        }
    }
    for s in [&d.reset_at, &d.reset_at2, &d.reset_time, &d.reset_time2] {
        let t = parse_rfc3339(s);
        if t != 0 {
            w.resets_at = t;
            break;
        }
    }
    if w.resets_at == 0 {
        for n in [d.reset_in, d.reset_in2] {
            if n.ok && n.v > 0.0 {
                w.resets_at = now.saturating_add((n.v * 1000.0) as Ms);
                break;
            }
        }
    }
    Some(w)
}

/// A span as Kimi's meter says it: "7d", "5h", "30m".
fn span(d: Duration) -> String {
    let s = d.as_secs();
    if s.is_multiple_of(24 * 3600) {
        format!("{}d", s / (24 * 3600))
    } else if s.is_multiple_of(3600) {
        format!("{}h", s / 3600)
    } else {
        format!("{}m", s / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::{KeyChecker, Kind};

    fn profile(dir: &Path) -> Profile {
        Profile {
            kind: Kind::from("kimi"),
            dir: crate::util::path_str(dir),
            ..Profile::default()
        }
    }

    // kimi_account_test.go: TestKimiQuotaOf
    #[test]
    fn quota_of() {
        let now = parse_rfc3339("2026-10-01T00:00:00Z");
        let body = br#"{"user":{"membership":{"level":"LEVEL_INTERMEDIATE"}},
            "usage":{"limit":"100","remaining":"74","resetTime":"2026-10-05T00:00:00.123456789Z"},
            "limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},"detail":{"limit":"100","used":"40","reset_in":3600}}]}"#;
        let q = kimi_quota_of(body, now).unwrap();
        assert_eq!(q.plan, "Kimi Code · Intermediate");
        assert_eq!(q.windows.len(), 2);
        let (w, f) = (&q.windows[0], &q.windows[1]);
        assert_eq!((w.label.as_str(), w.used, w.percent), ("7d", 26.0, 26.0));
        assert_eq!(w.resets_at, parse_rfc3339("2026-10-05T00:00:00.123Z"));
        assert_eq!(
            (f.label.as_str(), f.span, f.percent),
            ("5h", Duration::from_secs(5 * 3600), 40.0)
        );
        assert_eq!(f.resets_at, now + 3_600_000);
    }

    // kimi_account_test.go: TestKimiCredential
    #[test]
    fn credential() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(
            dir.join("config.toml"),
            r#"default_model = "kimi-code/kimi-for-coding"
[models."kimi-code/kimi-for-coding"]
provider = "managed:kimi-code"
model = "kimi-for-coding"
max_context_size = 262144
[providers."managed:kimi-code"]
type = "kimi"
base_url = "https://api.kimi.com/coding/v1"
api_key = ""
oauth = { storage = "file", key = "oauth/kimi-code" }
"#,
        )
        .unwrap();
        let k = Kimi::new();
        assert!(
            !k.account_summary(&profile(dir)).signed_in,
            "signed in with no token"
        );
        std::fs::create_dir_all(dir.join("credentials")).unwrap();
        std::fs::write(
            dir.join("credentials/kimi-code.json"),
            r#"{"access_token":"a","refresh_token":"r","expires_at":1}"#,
        )
        .unwrap();
        let s = k.account_summary(&profile(dir));
        assert!(s.signed_in);
        assert_eq!(
            (s.plan.as_str(), s.endpoint.as_str(), s.method.as_str()),
            (
                "Kimi Code",
                "https://api.kimi.com",
                "OAuth · Kimi credentials file"
            )
        );
        assert!(
            kimi_credential(&crate::util::path_str(dir)).expired,
            "a token from 1970 has expired"
        );
    }

    // kimi_account_test.go: TestKimiUnknownAndExpiredCredentialEvidence
    #[test]
    fn unknown_and_expired_credentials() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let k = Kimi::new();
        let s = k.account_summary(&profile(dir));
        assert!(s.status == "unknown" && !s.signed_in, "{s:?}");
        std::fs::write(
            dir.join("config.toml"),
            "default_model=\"active\"\n[models.active]\nprovider=\"managed:kimi-code\"\n[providers.\"managed:kimi-code\"]\noauth={storage=\"file\",key=\"oauth/kimi-code\"}\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("credentials")).unwrap();
        let file = dir.join("credentials/kimi-code.json");
        for (body, status, signed_in) in [
            (
                r#"{"access_token":"expired","expires_at":1}"#,
                "access token expired",
                false,
            ),
            (
                r#"{"access_token":"expired","refresh_token":"refresh","expires_at":1}"#,
                "credentials available",
                true,
            ),
            (r#"{"access_token":"#, "unknown", false),
        ] {
            std::fs::write(&file, body).unwrap();
            let s = k.account_summary(&profile(dir));
            assert_eq!(
                (s.status.as_str(), s.signed_in),
                (status, signed_in),
                "{body}"
            );
        }
    }

    // kimi_account_test.go: TestKimiAccountDoesNotBorrowDifferentModelsCredential
    #[test]
    fn never_borrows_another_models_credential() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("config.toml"),
            "default_model=\"active\"\n[models.active]\nprovider=\"active-provider\"\n[models.other]\nprovider=\"other-provider\"\n[providers.active-provider]\ntype=\"kimi\"\n[providers.other-provider]\napi_key=\"other-account-key\"\n",
        )
        .unwrap();
        let s = Kimi::new().account_summary(&profile(d.path()));
        assert!(!s.signed_in && s.status == "unknown", "{s:?}");
    }

    // history_test.go: TestKimiCheckKeyRequiresCredentials
    #[test]
    fn check_key_needs_credentials() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(
            dir.join("config.toml"),
            "[models.test]\nprovider = \"kimi\"\nmodel = \"kimi\"\n[providers.kimi]\napi_key = \"\"\n[providers.kimi.oauth]\nkey = \"oauth/kimi-code\"\nstorage = \"file\"\n",
        )
        .unwrap();
        let k = Kimi::new();
        assert!(
            k.check_key(&profile(dir)).is_err(),
            "a model's config isn't a sign-in"
        );
        std::fs::create_dir_all(dir.join("credentials")).unwrap();
        let path = dir.join("credentials/kimi-code.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(
            k.check_key(&profile(dir)).is_err(),
            "an empty token file isn't a sign-in"
        );
        std::fs::write(&path, r#"{"refresh_token":"test-refresh"}"#).unwrap();
        k.check_key(&profile(dir)).unwrap();
    }

    #[test]
    fn quota_asks_kimi_code_with_its_token() {
        let srv = crate::http::fake::serve(|r| {
            if r.auth != "Bearer tok" {
                return (401, "{}".into());
            }
            assert_eq!(r.path, "/coding/v1/usages");
            (200, r#"{"usage":{"limit":10,"used":5}}"#.into())
        });
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let cfg = |key: &str| {
            format!(
                "default_model=\"m\"\n[models.m]\nprovider=\"managed:kimi-code\"\n[providers.\"managed:kimi-code\"]\nbase_url=\"{}/coding/v1/\"\napi_key=\"{key}\"\n",
                srv.url
            )
        };
        std::fs::write(dir.join("config.toml"), cfg("tok")).unwrap();
        let q = Kimi::new()
            .quota(&profile(dir), &Account::default())
            .unwrap();
        assert_eq!(q.windows[0].percent, 50.0);
        std::fs::write(dir.join("config.toml"), cfg("bad")).unwrap();
        let err = Kimi::new()
            .quota(&profile(dir), &Account::default())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Kimi refused its credentials: run kimi login"
        );
    }
}
