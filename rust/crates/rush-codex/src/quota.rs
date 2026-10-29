//! The account's limits, as Codex's app-server reads them, and its earned
//! resets. Port of internal/adapters/codex/quota.go:1-289.

use std::collections::BTreeMap;
use std::time::Duration;

use rush_core::adapter::{self, Profile};
use rush_core::time::now;
use rush_core::usage::{Billing, Quota, ResetCredit, Resets, Scope, Source, Window};
use serde_json::Value;

use crate::json::{arr, get, int, num, s, st};
use crate::rpc::{Client, spawn};

/// How long a reading of its own may take: Go's caller bounded it.
const WITHIN: Duration = Duration::from_secs(30);

/// One of a snapshot's windows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LimitWindow {
    pub used_percent: f64,
    pub mins: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Codex's RateLimitSnapshot: one metered limit with up to two windows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Snapshot {
    pub limit_id: String,
    pub limit_name: String,
    pub normal_model_slug: String,
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
    pub plan_type: String,
    /// (has credits, unlimited)
    pub credits: Option<(bool, bool)>,
    pub reached_type: String,
}

impl Snapshot {
    /// The app-server's, in camel case.
    pub fn from_value(v: &Value) -> Snapshot {
        let w = |k: &str| {
            get(v, k).map(|w| LimitWindow {
                used_percent: num(w, "usedPercent"),
                mins: int(w, "windowDurationMins"),
                resets_at: int(w, "resetsAt"),
            })
        };
        Snapshot {
            limit_id: st(v, "limitId"),
            limit_name: st(v, "limitName"),
            normal_model_slug: st(v, "normalModelSlug"),
            primary: w("primary"),
            secondary: w("secondary"),
            plan_type: st(v, "planType"),
            credits: get(v, "credits").map(|c| {
                (
                    c.get("hasCredits")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    c.get("unlimited").and_then(Value::as_bool).unwrap_or(false),
                )
            }),
            reached_type: st(v, "rateLimitReachedType"),
        }
    }

    /// A rollout's token_count rate_limits: the same in snake case.
    pub fn from_rollout(v: &Value) -> Snapshot {
        let w = |k: &str| {
            get(v, k).map(|w| LimitWindow {
                used_percent: num(w, "used_percent"),
                mins: int(w, "window_minutes"),
                resets_at: int(w, "resets_at"),
            })
        };
        Snapshot {
            limit_id: st(v, "limit_id"),
            limit_name: st(v, "limit_name"),
            primary: w("primary"),
            secondary: w("secondary"),
            plan_type: st(v, "plan_type"),
            ..Snapshot::default()
        }
    }

    /// How a snapshot says the account pays: past a limit with credits on
    /// hand, they pay; otherwise the plan does. None when the snapshot
    /// doesn't say, as a partial update may not.
    pub fn billing(&self) -> Option<Billing> {
        if self.primary.is_none() && self.secondary.is_none() && self.credits.is_none() {
            return None;
        }
        if self.plan_type.ends_with("_usage_based") {
            return Some(Billing::Metered);
        }
        let mut spent = !self.reached_type.is_empty();
        for w in [&self.primary, &self.secondary].into_iter().flatten() {
            spent = spent || w.used_percent >= 100.0;
        }
        if spent
            && self
                .credits
                .is_some_and(|(has, unlimited)| has || unlimited)
        {
            return Some(Billing::Overage);
        }
        Some(Billing::Plan)
    }
}

/// How an account/read answer pays: an API key by the token, a ChatGPT
/// sign-in out of its plan.
pub(crate) fn account_billing(a: &Value) -> Option<Billing> {
    let acct = get(a, "account")?;
    let (typ, plan) = (s(acct, "type"), s(acct, "planType"));
    if typ == "apiKey" || plan.ends_with("_usage_based") {
        return Some(Billing::Metered);
    }
    (typ == "chatgpt").then_some(Billing::Plan)
}

/// The limit Codex's own snapshot reports on: its windows are "primary"
/// and "secondary", and other limits' are prefixed with their id.
const MAIN_LIMIT: &str = "codex";

/// A snapshot as rush's windows. A window the snapshot doesn't have is
/// left out rather than read as empty.
pub(crate) fn quota_from(s: &Snapshot) -> Quota {
    Quota {
        plan: s.plan_type.clone(),
        fetched_at: now(),
        source: Source::Live,
        windows: windows_of(s),
        ..Quota::default()
    }
}

fn windows_of(s: &Snapshot) -> Vec<Window> {
    let (mut prefix, mut scope) = (String::new(), Scope::default());
    if !s.limit_id.is_empty() && s.limit_id != MAIN_LIMIT {
        prefix = format!("{}:", s.limit_id);
        if !s.normal_model_slug.is_empty() {
            scope.models = vec![s.normal_model_slug.clone()];
        }
    }
    let mut out = Vec::new();
    for (id, w) in [("primary", &s.primary), ("secondary", &s.secondary)] {
        let Some(w) = w else { continue };
        let span = w.mins.map_or(Duration::ZERO, |m| {
            Duration::from_secs(m.max(0) as u64 * 60)
        });
        let (mut label, mut name) = span_words(span);
        if !prefix.is_empty() && !s.limit_name.is_empty() {
            (label, name) = (
                format!("{} {label}", s.limit_name),
                format!("{} {name}", s.limit_name),
            );
        }
        out.push(Window {
            id: format!("{prefix}{id}"),
            label,
            name,
            span,
            percent: w.used_percent,
            resets_at: w.resets_at.map_or(0, |t| t * 1000),
            scope: scope.clone(),
            ..Window::default()
        });
    }
    out
}

/// Names a window by how long it runs: "5h" and "5-hour", "7d" and
/// "weekly".
pub(crate) fn span_words(d: Duration) -> (String, String) {
    const HOUR: u64 = 3600;
    const DAY: u64 = 24 * HOUR;
    let secs = d.as_secs();
    match secs {
        0 => ("limit".into(), "usage limit".into()),
        s if s == 7 * DAY => ("7d".into(), "weekly".into()),
        s if s == DAY => ("1d".into(), "daily".into()),
        s if s % DAY == 0 => (format!("{}d", s / DAY), format!("{}-day", s / DAY)),
        s if s % HOUR == 0 => (format!("{}h", s / HOUR), format!("{}-hour", s / HOUR)),
        s => (format!("{}m", s / 60), format!("{}-minute", s / 60)),
    }
}

/// A whole rate-limit reading, account/rateLimits/read's answer: the main
/// limit, then every other one by id.
pub(crate) fn quota_from_response(r: &Value) -> Quota {
    let by: BTreeMap<String, Snapshot> = get(r, "rateLimitsByLimitId")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), Snapshot::from_value(v)))
                .collect()
        })
        .unwrap_or_default();
    let mut main = match by.get(MAIN_LIMIT) {
        Some(m) => m.clone(),
        None => Snapshot::from_value(r.get("rateLimits").unwrap_or(&Value::Null)),
    };
    if main.limit_id.is_empty() {
        main.limit_id = MAIN_LIMIT.into();
    }
    let mut q = quota_from(&main);
    for (id, s) in by.iter().filter(|(id, _)| id.as_str() != MAIN_LIMIT) {
        let mut s = s.clone();
        if s.limit_id.is_empty() {
            s.limit_id = id.clone();
        }
        q.windows.extend(windows_of(&s));
        if q.plan.is_empty() {
            q.plan = s.plan_type.clone();
        }
    }
    let account = s(r, "accountId");
    if !account.is_empty() {
        q.account = format!("codex:{account}");
    }
    if let Some(rc) = get(r, "rateLimitResetCredits") {
        let mut resets = Resets {
            available: int(rc, "availableCount").unwrap_or(0),
            ..Resets::default()
        };
        for c in arr(rc, "credits") {
            let status = s(c, "status");
            if !status.is_empty() && status != "available" {
                continue;
            }
            resets.credits.push(ResetCredit {
                id: st(c, "id"),
                title: st(c, "title"),
                about: st(c, "description"),
                granted: int(c, "grantedAt").unwrap_or(0) * 1000,
                expires: int(c, "expiresAt").map_or(0, |t| t * 1000),
            });
        }
        q.resets = Some(resets);
    }
    q.source = Source::Fetched;
    q
}

/// Asks a connected app-server for the account's limits. Neither call
/// spends model quota.
pub(crate) fn read_quota(c: &Client, within: Option<Duration>) -> adapter::Result<Quota> {
    let rl = c.call("account/rateLimits/read", Value::Null, within)?;
    let mut q = quota_from_response(&crate::json::parse(rl.get()));
    if let Ok(a) = c.call("account/read", serde_json::json!({}), within) {
        let a = crate::json::parse(a.get());
        if let Some(acct) = get(&a, "account") {
            let plan = s(acct, "planType");
            if !plan.is_empty() {
                q.plan = plan.to_string();
            }
            q.email = st(acct, "email");
            if q.account.is_empty() && !q.email.is_empty() {
                q.account = format!("codex:{}", q.email);
            }
        }
    }
    Ok(q)
}

/// The limits of the account profile p is signed in to, read by starting
/// an app-server of its own. binary is the codex program; empty is codex
/// on PATH. The Quota says the problem when it fails.
pub fn quota(p: &Profile, binary: &str) -> (Quota, Option<adapter::Error>) {
    let fail = |e: adapter::Error| {
        (
            Quota {
                problem: e.to_string(),
                ..Quota::default()
            },
            Some(e),
        )
    };
    let c = match spawn(binary, &p.dir, &[], &[], None) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let out = c
        .initialize(Some(WITHIN))
        .and_then(|_| read_quota(&c, Some(WITHIN)));
    c.close();
    match out {
        Ok(q) => (q, None),
        Err(e) => fail(e),
    }
}

/// What each answer of account/rateLimitResetCredit/consume says.
fn reset_outcome(o: &str) -> Option<&'static str> {
    Some(match o {
        "reset" => "its limits are reset",
        "nothingToReset" => "nothing to reset: none of its limits can be reset now",
        "noCredit" => "it has no resets left",
        "alreadyRedeemed" => "that reset was already used",
        _ => return None,
    })
}

/// Spends one of the account's earned resets (id, or the next when ""),
/// in its own app-server, and says what came of it. Asked once: the key
/// makes a retry of the same attempt safe, and there's none.
pub fn use_reset(p: &Profile, binary: &str, id: &str) -> adapter::Result<String> {
    let c = spawn(binary, &p.dir, &[], &[], None)?;
    let out = (|| {
        c.initialize(Some(WITHIN))?;
        let mut params = serde_json::json!({ "idempotencyKey": hex(&random::<16>()) });
        if !id.is_empty() {
            params["creditId"] = id.into();
        }
        let r = c.call("account/rateLimitResetCredit/consume", params, Some(WITHIN))?;
        let outcome = st(&crate::json::parse(r.get()), "outcome");
        Ok(reset_outcome(&outcome).map_or(outcome, str::to_string))
    })();
    c.close();
    out
}

/// n random bytes, from the system.
pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    for c in b.chunks_mut(256) {
        // SAFETY: getentropy writes at most c.len() (≤ 256) bytes into c.
        if unsafe { libc::getentropy(c.as_mut_ptr().cast(), c.len()) } != 0 {
            // Never a key of zeros, which a second attempt would repeat.
            let t = rush_core::time::now() as u64 ^ (std::process::id() as u64) << 32;
            for (i, x) in c.iter_mut().enumerate() {
                *x = (t.rotate_left(i as u32 * 8) as u8) ^ (i as u8);
            }
        }
    }
    b
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(pct: f64, mins: Option<i64>, resets: Option<i64>) -> Option<LimitWindow> {
        Some(LimitWindow {
            used_percent: pct,
            mins,
            resets_at: resets,
        })
    }

    fn w(id: &str, label: &str, name: &str, span_h: u64, pct: f64) -> Window {
        Window {
            id: id.into(),
            label: label.into(),
            name: name.into(),
            span: Duration::from_secs(span_h * 3600),
            percent: pct,
            ..Window::default()
        }
    }

    // codex_test.go:TestQuotaFrom
    #[test]
    fn windows_from_snapshots() {
        let reset = 1_790_000_000i64;
        let cases: Vec<(&str, Snapshot, Vec<Window>)> = vec![
            (
                "weekly primary, no secondary",
                Snapshot {
                    limit_id: "codex".into(),
                    primary: win(12.0, Some(10080), Some(reset)),
                    ..Snapshot::default()
                },
                vec![Window {
                    resets_at: reset * 1000,
                    ..w("primary", "7d", "weekly", 168, 12.0)
                }],
            ),
            (
                "missing primary",
                Snapshot {
                    secondary: win(40.0, Some(300), None),
                    ..Snapshot::default()
                },
                vec![w("secondary", "5h", "5-hour", 5, 40.0)],
            ),
            (
                "both",
                Snapshot {
                    primary: win(1.0, Some(300), None),
                    secondary: win(2.0, Some(43200), None),
                    ..Snapshot::default()
                },
                vec![
                    w("primary", "5h", "5-hour", 5, 1.0),
                    w("secondary", "30d", "30-day", 720, 2.0),
                ],
            ),
            (
                "no duration",
                Snapshot {
                    primary: win(5.0, None, None),
                    ..Snapshot::default()
                },
                vec![w("primary", "limit", "usage limit", 0, 5.0)],
            ),
            (
                "per model",
                Snapshot {
                    limit_id: "base_model_inference".into(),
                    limit_name: "gpt-reserve".into(),
                    normal_model_slug: "gpt-5.6-luna".into(),
                    primary: win(3.0, Some(10080), None),
                    ..Snapshot::default()
                },
                vec![Window {
                    scope: Scope {
                        models: vec!["gpt-5.6-luna".into()],
                    },
                    ..w(
                        "base_model_inference:primary",
                        "gpt-reserve 7d",
                        "gpt-reserve weekly",
                        168,
                        3.0,
                    )
                }],
            ),
            ("nothing", Snapshot::default(), vec![]),
        ];
        for (name, snap, want) in cases {
            assert_eq!(quota_from(&snap).windows, want, "{name}");
        }
    }

    // codex_test.go:TestBilling
    #[test]
    fn billing() {
        let full = win(100.0, None, None);
        let part = win(40.0, None, None);
        let credits = Some((true, false));
        let cases: Vec<(&str, Snapshot, Option<Billing>)> = vec![
            (
                "within the plan",
                Snapshot {
                    primary: part.clone(),
                    credits,
                    ..Snapshot::default()
                },
                Some(Billing::Plan),
            ),
            (
                "spent, credits pay",
                Snapshot {
                    primary: part.clone(),
                    secondary: full.clone(),
                    credits,
                    ..Snapshot::default()
                },
                Some(Billing::Overage),
            ),
            (
                "spent, no credits",
                Snapshot {
                    primary: full.clone(),
                    ..Snapshot::default()
                },
                Some(Billing::Plan),
            ),
            (
                "reached, credits pay",
                Snapshot {
                    reached_type: "rate_limit_reached".into(),
                    credits,
                    ..Snapshot::default()
                },
                Some(Billing::Overage),
            ),
            (
                "usage-based plan",
                Snapshot {
                    primary: part.clone(),
                    plan_type: "enterprise_cbp_usage_based".into(),
                    ..Snapshot::default()
                },
                Some(Billing::Metered),
            ),
            (
                "says nothing",
                Snapshot {
                    limit_id: "codex".into(),
                    ..Snapshot::default()
                },
                None,
            ),
        ];
        for (name, snap, want) in cases {
            assert_eq!(snap.billing(), want, "{name}");
        }
        let a = crate::json::parse(r#"{"account":{"type":"apiKey"}}"#);
        assert_eq!(account_billing(&a), Some(Billing::Metered));
        let a = crate::json::parse(r#"{"account":{"type":"chatgpt","planType":"pro"}}"#);
        assert_eq!(account_billing(&a), Some(Billing::Plan));
    }

    // codex_test.go:TestQuotaFromResponse
    #[test]
    fn whole_readings() {
        let r = crate::json::parse(
            r#"{"rateLimits":{"limitId":"codex","primary":{"usedPercent":10,"windowDurationMins":10080,"resetsAt":1},"secondary":null,"planType":"pro"},
		"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":10,"windowDurationMins":10080,"resetsAt":1},"secondary":null,"planType":"pro"},
		"other":{"limitId":"other","normalModelSlug":"gpt-x","primary":{"usedPercent":50,"windowDurationMins":300,"resetsAt":null},"secondary":null}},
		"accountId":"acct-1"}"#,
        );
        let q = quota_from_response(&r);
        assert_eq!(
            (
                q.account.as_str(),
                q.plan.as_str(),
                q.source,
                q.windows.len()
            ),
            ("codex:acct-1", "pro", Source::Fetched, 2)
        );
        assert_eq!(q.tightest("gpt-5.5").unwrap().id, "primary");
        assert_eq!(q.tightest("gpt-x").unwrap().id, "other:primary");
    }

    // resets_test.go:TestReadsResets
    #[test]
    fn reads_resets() {
        let r = crate::json::parse(
            r#"{"rateLimits":{"limitId":"codex","primary":{"usedPercent":100}},
		"rateLimitResetCredits":{"availableCount":2,"credits":[
			{"id":"c1","title":"Limit reset","description":"Resets your 5h and weekly limits","status":"available","grantedAt":1790000000,"expiresAt":1800000000,"resetType":"codexRateLimits"},
			{"id":"c0","status":"redeemed","grantedAt":1780000000,"resetType":"codexRateLimits"}]}}"#,
        );
        let q = quota_from_response(&r);
        let rs = q.resets.expect("resets");
        assert_eq!((rs.available, rs.credits.len()), (2, 1));
        let c = &rs.credits[0];
        assert!(
            c.id == "c1" && !c.about.is_empty() && c.expires != 0,
            "{c:?}"
        );
        assert_eq!(reset_outcome("noCredit"), Some("it has no resets left"));
    }
}
