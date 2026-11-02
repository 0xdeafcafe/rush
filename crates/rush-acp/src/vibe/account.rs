//! What rush can say of Vibe's account: only from Vibe's own local
//! credential and identity cache. The key picks its cache entry and is
//! never returned. Port of internal/adapters/vibe/account.go:1-101.

use std::collections::HashMap;
use std::path::Path;

use rush_core::adapter::{AccountSummary, AccountSummaryReader, Profile};
use rush_core::time::now;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{KEY_ENV, Vibe, dotenv_key};
use crate::kimi_account::origin;
use crate::util::env;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Payload {
    plan_name: String,
    plan_type: String,
    organization_kind: String,
    customer_id: String,
    api_base: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Entry {
    stored_at_timestamp: i64,
    payload: Payload,
}

impl AccountSummaryReader for Vibe {
    fn account_summary(&self, p: &Profile) -> AccountSummary {
        let dir = Vibe::dir(p);
        let mut summary = AccountSummary {
            status: "unknown".into(),
            endpoint: "https://api.mistral.ai".into(),
            note: "Rush could not read Mistral credentials from the environment, keychain, or Vibe config home. This does not establish that you are signed out.".into(),
            ..AccountSummary::default()
        };
        let mut key = env(KEY_ENV).into_bytes();
        if !key.is_empty() {
            summary.method = "API key · environment".into();
        } else {
            key = (self.stored_key)();
            if !key.is_empty() {
                summary.method = "API key · system keychain".into();
            } else {
                key = dotenv_key(&Path::new(&dir).join(".env"))
                    .unwrap_or_default()
                    .into_bytes();
                if !key.is_empty() {
                    summary.method = "API key · Vibe .env".into();
                }
            }
        }
        if key.is_empty() {
            return summary;
        }
        summary.signed_in = true;
        summary.status = "credentials available".into();
        summary.note =
            "Credentials found; account details appear after Vibe caches its account lookup."
                .into();
        let hash: String = Sha256::digest(&key)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..32]
            .to_string();
        key.fill(0);
        let Ok(b) = std::fs::read(Path::new(&dir).join("whoami_cache.json")) else {
            return summary;
        };
        let Ok(mut cache) = serde_json::from_slice::<HashMap<String, Entry>>(&b) else {
            return summary;
        };
        let Some(entry) = cache.remove(&hash) else {
            return summary;
        };
        if entry.stored_at_timestamp <= now() / 1000 - 6 * 3600 {
            summary.note = "Credentials found; cached account details expired. Open Vibe once to refresh them.".into();
            return summary;
        }
        let pl = entry.payload;
        summary.plan = if pl.plan_name.is_empty() {
            pl.plan_type
        } else {
            pl.plan_name
        };
        summary.org = pl.organization_kind;
        if !pl.customer_id.is_empty() {
            summary.name = format!("Customer {}", pl.customer_id);
        }
        if let Some((scheme, host)) = origin(&pl.api_base)
            && (scheme == "https" || scheme == "http")
        {
            summary.endpoint = format!("{scheme}://{host}");
        }
        summary.note =
            "From Vibe's local account cache; Vibe does not expose email in this record.".into();
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::path_str;

    fn profile(dir: &Path) -> Profile {
        Profile {
            dir: path_str(dir),
            ..Profile::default()
        }
    }

    // account_test.go: TestAccountSummaryMatchesCurrentCredential, its key
    // from the keychain rather than the process's environment.
    #[test]
    fn summary_matches_the_current_credential() {
        if !env(KEY_ENV).is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let hash: String = Sha256::digest(b"test-key-do-not-display")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..32]
            .to_string();
        let t = now() / 1000;
        std::fs::write(
            d.path().join("whoami_cache.json"),
            format!(
                r#"{{"{hash}":{{"stored_at_timestamp":{t},"payload":{{"plan_type":"pro","plan_name":"Pro","organization_kind":"personal","customer_id":"customer-1","api_base":"https://api.mistral.ai/path?secret=hidden"}}}},"another-key":{{"stored_at_timestamp":{t},"payload":{{"plan_name":"Wrong account"}}}}}}"#
            ),
        )
        .unwrap();
        let v = Vibe {
            stored_key: || b"test-key-do-not-display".to_vec(),
            ..Vibe::without_keychain()
        };
        let s = v.account_summary(&profile(d.path()));
        assert!(s.signed_in);
        assert_eq!(
            (s.plan.as_str(), s.name.as_str(), s.endpoint.as_str()),
            ("Pro", "Customer customer-1", "https://api.mistral.ai")
        );
        assert_eq!(s.method, "API key · system keychain");
        let text = format!("{s:?}");
        assert!(
            !text.contains("test-key") && !text.contains("secret="),
            "{text}"
        );
    }

    // account_test.go: TestAccountSummaryUnreadableCredentialsAreUnknown
    #[test]
    fn unreadable_credentials_are_unknown() {
        if !env(KEY_ENV).is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let s = Vibe::without_keychain().account_summary(&profile(d.path()));
        assert!(!s.signed_in && s.status == "unknown", "{s:?}");
        assert!(!s.note.contains("No Mistral credentials found"));
        std::fs::write(d.path().join(".env"), "MISTRAL_API_KEY=\"abc\"\n").unwrap();
        let s = Vibe::without_keychain().account_summary(&profile(d.path()));
        assert_eq!(s.method, "API key · Vibe .env");
    }
}
