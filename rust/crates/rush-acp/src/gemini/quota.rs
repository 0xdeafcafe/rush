//! Gemini's allowance on Code Assist, read as Gemini itself reads it:
//! loadCodeAssist for the tier and project, retrieveUserQuota for what's
//! left of each model's allowance. Asking spends nothing. Port of
//! internal/adapters/gemini/quota.go:1-172.

use std::path::Path;
use std::time::Duration;

use rush_core::adapter::{self, Account, Profile, QuotaSource};
use rush_core::time::{Ms, now, parse_rfc3339};
use rush_core::usage::{Quota, Scope, Source, Window};
use serde::Deserialize;
use serde_json::json;

use super::{CREDS_FILE, Gemini};
use crate::rpc::Error;
use crate::util::env;

/// Code Assist's tier for a Google account that pays nothing.
const FREE_TIER: &str = "free-tier";

fn no_google() -> Error {
    Error::other("not signed in with Google: an API key has no allowance to read")
}

/// Gemini renews its token only as it runs; rush doesn't sign in for it.
fn lapsed() -> Error {
    Error::other("Gemini's Google sign-in has lapsed: it renews when gemini next runs")
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(super) struct Tier {
    id: String,
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct LoadResponse {
    #[serde(rename = "currentTier")]
    current_tier: Option<Tier>,
    #[serde(rename = "paidTier")]
    paid_tier: Option<Tier>,
    #[serde(rename = "cloudaicompanionProject")]
    project: String,
}

/// One model's allowance: what fraction of it is left, and when it fills
/// again.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Bucket {
    #[serde(rename = "remainingAmount")]
    remaining_amount: String,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: String,
    #[serde(rename = "modelId")]
    model_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct QuotaResponse {
    buckets: Vec<Bucket>,
}

impl QuotaSource for Gemini {
    /// The Google account's allowance, read with the token Gemini keeps; an
    /// API key has none.
    fn quota(&self, p: &Profile, _: &Account) -> adapter::Result<Quota> {
        let dir = Gemini::dir(p);
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Creds {
            access_token: String,
            /// ms
            expiry_date: i64,
        }
        let c = std::fs::read(Path::new(&dir).join(CREDS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Creds>(&b).ok())
            .filter(|c| !c.access_token.is_empty())
            .ok_or_else(no_google)?;
        if c.expiry_date > 0 && now() > c.expiry_date {
            return Err(Box::new(lapsed()));
        }
        let mut proj = env("GOOGLE_CLOUD_PROJECT");
        let mut meta = json!({"metadata": {"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}});
        if !proj.is_empty() {
            meta["cloudaicompanionProject"] = json!(proj);
        }
        let load: LoadResponse = self.post(&c.access_token, "loadCodeAssist", &meta)?;
        if !load.project.is_empty() {
            proj = load.project.clone();
        }
        if proj.is_empty() {
            return Err(Box::new(Error::other(
                "Gemini hasn't set Code Assist up for this account: run gemini once",
            )));
        }
        let r: QuotaResponse = self.post(
            &c.access_token,
            "retrieveUserQuota",
            &json!({"project": proj}),
        )?;
        let mut q = quota_of(&load, &r, now());
        q.email = active_email(&dir);
        Ok(q)
    }
}

impl Gemini {
    /// Calls a Code Assist method.
    fn post<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, Error> {
        let url = format!("{}:{method}", self.endpoint);
        let res =
            crate::http::post(&url, token, body.to_string().as_bytes()).map_err(Error::other)?;
        match res.code {
            401 => return Err(lapsed()),
            429 => return Err(Error::other("Gemini's limits: rate-limited")),
            c if c >= 300 => return Err(Error::other(format!("Gemini's limits: {}", res.status))),
            _ => {}
        }
        Ok(serde_json::from_slice(&res.body)?)
    }
}

/// The allowance as rush's windows, one a model: a day's requests, reset
/// each day. The free tier's plan says so.
pub(super) fn quota_of(load: &LoadResponse, r: &QuotaResponse, now: Ms) -> Quota {
    let mut q = Quota {
        fetched_at: now,
        source: Source::Fetched,
        ..Quota::default()
    };
    match load.paid_tier.as_ref().or(load.current_tier.as_ref()) {
        None => {}
        Some(t) if t.id == FREE_TIER => q.plan = "free".into(),
        Some(t) => {
            q.plan = if t.name.is_empty() {
                t.id.clone()
            } else {
                t.name.clone()
            }
        }
    }
    for b in &r.buckets {
        let Some(frac) = b.remaining_fraction else {
            continue;
        };
        if b.model_id.is_empty() {
            continue;
        }
        let left = frac.clamp(0.0, 1.0);
        let mut w = Window {
            id: b.model_id.clone(),
            label: b
                .model_id
                .strip_prefix("gemini-")
                .unwrap_or(&b.model_id)
                .to_string(),
            name: format!("{} today", b.model_id),
            span: Duration::from_secs(24 * 3600),
            percent: 100.0 * (1.0 - left),
            scope: Scope {
                models: vec![b.model_id.clone()],
            },
            ..Window::default()
        };
        if let Ok(n) = b.remaining_amount.parse::<f64>()
            && left > 0.0
        {
            w.limit = (n / left + 0.5).trunc();
            w.used = w.limit - n;
        }
        w.resets_at = parse_rfc3339(&b.reset_time);
        q.windows.push(w);
    }
    q
}

/// The Google account Gemini is signed in as.
fn active_email(dir: &str) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct A {
        active: String,
    }
    std::fs::read(Path::new(dir).join("google_accounts.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<A>(&b).ok())
        .map(|a| a.active)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::path_str;

    // quota_test.go: TestQuota
    #[test]
    fn quota() {
        let srv = crate::http::fake::serve(|r| {
            assert_eq!(r.method, "POST");
            if r.auth != "Bearer tok" {
                return (401, String::new());
            }
            if r.path.ends_with(":loadCodeAssist") {
                return (
                    200,
                    r#"{"currentTier":{"id":"free-tier","name":"Gemini Code Assist for individuals"},"cloudaicompanionProject":"proj-1"}"#.into(),
                );
            }
            (
                200,
                r#"{"buckets":[
                {"modelId":"gemini-2.5-pro","remainingFraction":0.25,"remainingAmount":"25","resetTime":"2026-10-01T00:00:00Z","tokenType":"REQUESTS"},
                {"modelId":"gemini-2.5-flash","remainingFraction":1},
                {"tokenType":"REQUESTS"}]}"#
                    .into(),
            )
        });
        let a = Gemini::at(&format!("{}/v1internal", srv.url));
        let d = tempfile::tempdir().unwrap();
        let p = Profile {
            dir: path_str(d.path()),
            ..Profile::default()
        };
        let err = a.quota(&p, &Account::default()).unwrap_err();
        assert_eq!(err.to_string(), no_google().to_string());
        let creds = |expiry: Ms| {
            std::fs::write(
                d.path().join(CREDS_FILE),
                format!(r#"{{"access_token":"tok","expiry_date":{expiry}}}"#),
            )
            .unwrap();
        };
        creds(now() - 60_000);
        let err = a.quota(&p, &Account::default()).unwrap_err();
        assert_eq!(err.to_string(), lapsed().to_string());
        creds(now() + 3_600_000);
        std::fs::write(
            d.path().join("google_accounts.json"),
            r#"{"active":"a@example.com","old":[]}"#,
        )
        .unwrap();
        let q = a.quota(&p, &Account::default()).unwrap();
        assert_eq!(
            (q.plan.as_str(), q.email.as_str(), q.windows.len()),
            ("free", "a@example.com", 2)
        );
        let pro = &q.windows[0];
        assert_eq!(pro.label, "2.5-pro");
        assert_eq!((pro.percent, pro.used, pro.limit), (75.0, 75.0, 100.0));
        assert_ne!(pro.resets_at, 0);
        assert_eq!(q.used("gemini-2.5-flash"), 0.0);
        assert_eq!(q.used("gemini-2.5-pro"), 75.0);
    }
}
