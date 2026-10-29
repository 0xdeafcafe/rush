//! The models agy last listed, kept so a session can start without
//! running agy to ask. Port of internal/adapters/antigravity/catalog.go:
//! 1-40, and antigravity.go:68-121 (modelsOutput, readModels,
//! parseModels).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rush_core::adapter::Choice;
use rush_core::time::{Ms, now};
use serde::{Deserialize, Serialize};

/// A model as Go's agent.Choice is written.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct SavedChoice {
    #[serde(rename = "ID")]
    id: String,
    note: String,
    context: i64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Snapshot {
    #[serde(with = "rush_core::wire::time")]
    at: Ms,
    ready: bool,
    #[serde(with = "rush_core::wire::nullvec")]
    models: Vec<SavedChoice>,
}

fn snapshot_path() -> PathBuf {
    rush_state::dir().join("antigravity-catalog.json")
}

/// Keeps what agy listed, and whether it could. Only model IDs,
/// descriptions and availability are kept, never what the CLI printed or
/// a credential; the file is replaced at once.
pub(crate) fn write_snapshot(data: &[u8], ready: bool) {
    let mut s = Snapshot {
        at: now(),
        ready,
        ..Snapshot::default()
    };
    if ready {
        s.models = parse_models(&String::from_utf8_lossy(data))
            .into_iter()
            .map(|c| SavedChoice {
                id: c.id,
                note: c.note,
                context: c.context,
            })
            .collect();
    }
    let _ = rush_state::write_json(&snapshot_path(), &s);
}

/// The models found before, and whether they're ready: only a listing
/// that worked, in the last day.
pub(crate) fn runner_snapshot() -> (Vec<Choice>, bool) {
    let Ok(b) = std::fs::read(snapshot_path()) else {
        return (Vec::new(), false);
    };
    let Ok(s) = serde_json::from_slice::<Snapshot>(&b) else {
        return (Vec::new(), false);
    };
    let now = now();
    if !s.ready || now - s.at > 24 * 60 * 60 * 1000 || s.at > now {
        return (Vec::new(), false);
    }
    let models = s
        .models
        .into_iter()
        .map(|c| Choice {
            id: c.id,
            note: c.note,
            context: c.context,
        })
        .collect();
    (models, true)
}

/// agy's model list: each line "<id> <description>", an id having a dash
/// and no colon or box line.
pub(crate) fn parse_models(text: &str) -> Vec<Choice> {
    text.split('\n')
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 2 || !f[0].contains('-') || f[0].contains([':', '│']) {
                return None;
            }
            Some(Choice {
                id: f[0].to_string(),
                note: f[1..].join(" "),
                ..Choice::default()
            })
        })
        .collect()
}

/// What `agy models` printed, asked at most every two seconds.
static MODELS: Mutex<Option<Listed>> = Mutex::new(None);

/// A listing, and when it was asked for.
type Listed = (Instant, Result<Vec<u8>, String>);

pub(crate) fn models_output(bin: &str) -> Result<Vec<u8>, String> {
    let mut m = MODELS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, r)) = m.as_ref()
        && at.elapsed() < Duration::from_secs(2)
    {
        return r.clone();
    }
    let r = read_models(bin);
    *m = Some((Instant::now(), r.clone()));
    match &r {
        Ok(b) => write_snapshot(b, true),
        Err(_) => write_snapshot(&[], false),
    }
    r
}

fn read_models(bin: &str) -> Result<Vec<u8>, String> {
    if bin.is_empty() {
        return Err("Antigravity CLI is not installed".into());
    }
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("models");
    match rush_sys::cmd::output_within(&mut cmd, Duration::from_secs(10)) {
        Ok(o) if o.ok() => Ok(o.out),
        _ => Err(
            "Antigravity models unavailable; sign in through Settings → Harnesses → Antigravity CLI"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_list() {
        let got = parse_models(
            "Fetching available models...\ngemini-example  Gemini Example\nclaude-example Claude Example\n",
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "gemini-example");
        assert_eq!(got[0].note, "Gemini Example");
    }

    #[test]
    fn runner_snapshot_needs_fresh_successful_discovery() {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        assert!(!runner_snapshot().1, "unverified catalog marked ready");
        write_snapshot(b"gemini-test Available model", true);
        let (models, ready) = runner_snapshot();
        assert!(ready);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-test");
        write_snapshot(b"private provider error", false);
        let (models, ready) = runner_snapshot();
        assert!(
            !ready && models.is_empty(),
            "failed refresh kept stale readiness"
        );
        let stale = Snapshot {
            ready: true,
            at: now() - 25 * 60 * 60 * 1000,
            ..Snapshot::default()
        };
        rush_state::write_json(&snapshot_path(), &stale).unwrap();
        assert!(!runner_snapshot().1, "expired snapshot accepted");
        // As Go writes it.
        let b = std::fs::read_to_string(snapshot_path()).unwrap();
        assert!(
            b.contains("\"At\"") && b.contains("\"Ready\": true") && b.contains("\"Models\": null"),
            "{b}"
        );
        rush_state::set_thread_dir(None);
    }
}
