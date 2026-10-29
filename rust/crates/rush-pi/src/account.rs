//! Who Pi runs as, and how it signs in. Port of
//! internal/adapters/pi/account.go:1-88.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use rush_core::adapter::{self, AccountSummary, Profile};
use serde::Deserialize;

use crate::discover::home;
use crate::json::nd;

#[derive(Default, Deserialize)]
#[serde(default)]
struct Cred {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    email: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    #[serde(rename = "defaultProvider", deserialize_with = "nd")]
    provider: String,
}

/// The provider Pi runs on by default and how it's signed in to it, from
/// auth.json, settings.json and the environment. Keys and tokens are only
/// checked for, never returned.
pub(crate) fn summary(p: &Profile) -> AccountSummary {
    let dir = if p.dir.is_empty() {
        home()
    } else {
        p.dir.clone()
    };
    let creds: BTreeMap<String, Cred> = std::fs::read(Path::new(&dir).join("auth.json"))
        .ok()
        .and_then(|b| {
            serde_json::from_slice::<Option<BTreeMap<String, Cred>>>(&b)
                .ok()
                .flatten()
        })
        .unwrap_or_default();
    let settings: Settings = std::fs::read(Path::new(&dir).join("settings.json"))
        .ok()
        .and_then(|b| {
            serde_json::from_slice::<Option<Settings>>(&b)
                .ok()
                .flatten()
        })
        .unwrap_or_default();
    let stored: Vec<&str> = creds.keys().map(String::as_str).collect(); // sorted
    let prov = settings.provider;
    let mut s = AccountSummary {
        name: prov.clone(),
        ..AccountSummary::default()
    };
    let env_set =
        !prov.is_empty() && std::env::var_os(env_key(&prov)).is_some_and(|v| !v.is_empty());
    if let Some(c) = creds.get(&prov) {
        (s.signed_in, s.email) = (true, c.email.clone());
        s.method = if c.typ == "oauth" {
            "OAuth · auth.json"
        } else {
            "API key · auth.json"
        }
        .into();
    } else if env_set {
        (s.signed_in, s.method) = (true, format!("API key · {}", env_key(&prov)));
    }
    if prov.is_empty() && !stored.is_empty() {
        s.signed_in = true;
        s.note = format!(
            "No default provider set; signed in to {}",
            stored.join(", ")
        );
    } else if !s.signed_in && !prov.is_empty() {
        s.note = format!(
            "No credentials for {prov}: run /login in Pi or set {}",
            env_key(&prov)
        );
    } else if !s.signed_in {
        s.note = "No credentials found: run /login in Pi, or set a provider's API key".into();
    } else if stored.len() > 1 {
        s.note = format!("Also signed in to {}", stored.join(", "));
    }
    s
}

/// The variable Pi reads a provider's key from, for the usual ones.
pub(crate) fn env_key(provider: &str) -> String {
    if provider == "google" {
        return "GEMINI_API_KEY".into();
    }
    format!("{}_API_KEY", provider.replace('-', "_").to_uppercase())
}

/// Opens Pi itself: it signs in with /login, inside a session.
pub(crate) fn sign_in_command(bin: &str, p: &Profile) -> adapter::Result<Command> {
    if bin.is_empty() {
        return Err(crate::conn::says("Pi is not installed"));
    }
    let mut cmd = Command::new(bin);
    if !p.dir.is_empty() && p.dir != home() {
        cmd.env("PI_CODING_AGENT_DIR", &p.dir);
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_summary() {
        let d = tempfile::tempdir().unwrap();
        let p = Profile {
            kind: crate::KIND.into(),
            dir: d.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        assert!(!summary(&p).signed_in, "empty dir signed in");
        std::fs::write(
            d.path().join("settings.json"),
            r#"{"defaultProvider":"anthropic"}"#,
        )
        .unwrap();
        std::fs::write(
            d.path().join("auth.json"),
            r#"{"anthropic":{"type":"oauth","access":"x","refresh":"y","expires":1},"openai":{"type":"api_key","key":"k"}}"#,
        )
        .unwrap();
        let s = summary(&p);
        assert!(s.signed_in);
        assert_eq!(s.method, "OAuth · auth.json");
        assert_eq!(s.name, "anthropic");
        assert_eq!(s.note, "Also signed in to anthropic, openai");
        assert_eq!(env_key("google"), "GEMINI_API_KEY");
        assert_eq!(env_key("open-router"), "OPEN_ROUTER_API_KEY");
    }
}
