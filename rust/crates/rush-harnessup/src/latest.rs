//! The newest version of a harness: from Homebrew when it installed it,
//! else from its package's registry.
//!
//! Ported from internal/harnessup/latest.go:1-111.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use super::plan::{Published, Source};
use super::version::parse;

/// The newest version of a harness that can be known cheaply, or empty:
/// Homebrew's when it installed it, else what the registry its package is
/// in says.
pub fn latest_of(publ: &Published, s: &Source, find: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    let v = brew_latest(s, find);
    if !v.is_empty() {
        return v;
    }
    let npm = if s.npm.is_empty() { &publ.npm } else { &s.npm };
    if !npm.is_empty() {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            version: String,
        }
        return get_json::<R>(&format!("https://registry.npmjs.org/{npm}/latest"))
            .map(|r| parse(&r.version))
            .unwrap_or_default();
    }
    if !publ.pypi.is_empty() {
        #[derive(Deserialize)]
        struct Info {
            #[serde(default)]
            version: String,
        }
        #[derive(Deserialize)]
        struct R {
            info: Info,
        }
        return get_json::<R>(&format!(
            "https://pypi.org/pypi/{}/json",
            path_escape(&publ.pypi)
        ))
        .map(|r| parse(&r.info.version))
        .unwrap_or_default();
    }
    String::new()
}

/// The version Homebrew would install, from what it already knows (it
/// doesn't update itself to say).
fn brew_latest(s: &Source, find: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    let (name, flag) = if !s.cask.is_empty() {
        (&s.cask, "--cask")
    } else {
        (&s.brew, "--formula")
    };
    if name.is_empty() {
        return String::new();
    }
    let Some(brew) = find("brew") else {
        return String::new();
    };
    let out = match rush_sys::cmd::output_within(
        Command::new(brew)
            .args(["info", "--json=v2", flag, name])
            .env("HOMEBREW_NO_AUTO_UPDATE", "1"),
        Duration::from_secs(20),
    ) {
        Ok(o) if o.ok() => o.out,
        _ => return String::new(),
    };
    brew_version(&out)
}

/// The version in `brew info --json=v2`'s answer.
pub(crate) fn brew_version(out: &[u8]) -> String {
    #[derive(Deserialize, Default)]
    struct Versions {
        #[serde(default)]
        stable: String,
    }
    #[derive(Deserialize)]
    struct Formula {
        #[serde(default)]
        versions: Versions,
    }
    #[derive(Deserialize)]
    struct Cask {
        #[serde(default)]
        version: String,
    }
    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        formulae: Vec<Formula>,
        #[serde(default)]
        casks: Vec<Cask>,
    }
    let Ok(r) = serde_json::from_slice::<R>(out) else {
        return String::new();
    };
    if let Some(f) = r.formulae.first() {
        return parse(&f.versions.stable);
    }
    if let Some(c) = r.casks.first() {
        return parse(c.version.split(',').next().unwrap_or(""));
    }
    String::new()
}

fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Option<T> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let mut res = agent.get(url).call().ok()?;
    let body = res.body_mut().read_to_vec().ok()?;
    serde_json::from_slice(&body).ok()
}

/// s escaped as one path segment, as Go's url.PathEscape.
fn path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~$&+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brew_answers() {
        for (out, want) in [
            (
                &br#"{"formulae":[{"versions":{"stable":"2.25.0"}}],"casks":[]}"#[..],
                "2.25.0",
            ),
            (
                br#"{"formulae":[],"casks":[{"version":"2.1.284,abc"}]}"#,
                "2.1.284",
            ),
            (br#"{"formulae":[],"casks":[]}"#, ""),
            (b"not json", ""),
        ] {
            assert_eq!(brew_version(out), want);
        }
    }

    #[test]
    fn escapes_a_path_segment() {
        assert_eq!(path_escape("mistral-vibe"), "mistral-vibe");
        assert_eq!(path_escape("a b/c?"), "a%20b%2Fc%3F");
    }

    #[test]
    fn nothing_to_ask() {
        assert_eq!(
            latest_of(&Published::default(), &Source::default(), &|_| None),
            ""
        );
        // Brew installed it but there's no brew to ask, and no package.
        let s = Source {
            brew: "x".into(),
            ..Source::default()
        };
        assert_eq!(latest_of(&Published::default(), &s, &|_| None), "");
    }
}
