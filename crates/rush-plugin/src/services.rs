//! Services: what one plugin answers for others.
//! Ported from internal/plugin/services.go.
//!
//! A plugin lists in provides the services it answers, and in uses the
//! ones it may ask for. Asking for a service (services.ask, to the broker)
//! calls service.call on every running plugin that provides it, at once,
//! and hands back each answer by plugin. So a plugin can feed another
//! without either knowing the other's name: Haven provides cleanup.report,
//! and the cleanup plugin (with rush's own cleanup agent) uses whatever
//! answers it. The broker answers rush itself for any service; a plugin
//! only for the ones in its uses.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::ctx::Ctx;
use crate::manifest::{Manifest, PROTO_RUSH};
use crate::rpc::Raw;
use crate::text::go_q;

/// What a plugin knows about freeing disk and memory: a CleanupReport.
pub const SERVICE_CLEANUP_REPORT: &str = "cleanup.report";

/// A cleanup.report answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupReport {
    /// Names its source in the cleanup agent's task: "Haven".
    pub title: String,
    /// What it found, as text for the agent.
    pub report: String,
    /// What the agent may do with it, and what it must never do.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rules: String,
}

/// What a plugin finds on disk that rush's disk engine may clean
/// (rush-fleet's disk module): DiskCandidates. The plugin removes nothing
/// itself; the engine's rules decide, and its removals ask whether
/// anything uses the path right before.
pub const SERVICE_DISK_CANDIDATES: &str = "disk.candidates";

/// What a disk.candidates ask may say: the level rush cleans at ("normal"
/// or "low") and the free bytes. services.ask carries no parameters yet,
/// so a plugin gets this empty and reports everything it found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiskCandidatesAsk {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub level: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub free: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A disk.candidates answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskCandidates {
    pub candidates: Vec<DiskCandidate>,
}

/// One thing on disk a plugin reports: rush-fleet's disk::Candidate on the
/// wire, the fields a plugin can know.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiskCandidate {
    /// Absolute.
    pub path: String,
    /// What it is, which rules name ("go-build-cache", "node-modules"):
    /// lowercase, dashes.
    pub kind: String,
    /// What it is, in a few words, for you.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// Its blocks on disk, every file counted once; 0 when not measured.
    pub bytes_on_disk: u64,
    /// What removing it would free: files with no other hard link.
    pub bytes_freeable: u64,
    pub facts: DiskFacts,
}

/// What decides whether a reported candidate goes; what the plugin didn't
/// look at is left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiskFacts {
    /// When a build last wrote it, in Unix milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_build: Option<i64>,
    /// When anything in it last changed, in Unix milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<i64>,
    /// Whether anything uses it: "unknown" (or left out), "yes" or "no".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub in_use: String,
    /// The session it belongs to, and the worktree.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree: String,
}

/// How long one plugin has to answer a service.call.
pub const SERVICE_TIMEOUT: Duration = Duration::from_secs(45);

/// One plugin's answer to a services.ask.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Answer {
    pub plugin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Raw>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// ^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)+$: dotted lowercase names.
pub(crate) fn service_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            let b = p.as_bytes();
            !b.is_empty()
                && b[0].is_ascii_lowercase()
                && b[1..]
                    .iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        })
}

impl Manifest {
    pub(crate) fn validate_services(&self) -> Result<(), String> {
        for s in self.provides.iter().chain(&self.uses) {
            if !service_ok(s) {
                return Err(format!(
                    "service {}: use dotted lowercase names, like cleanup.report",
                    go_q(s)
                ));
            }
        }
        if !self.provides.is_empty() && self.proto() != PROTO_RUSH {
            return Err(
                "an MCP plugin cannot provide services: it has no way to answer them".into(),
            );
        }
        Ok(())
    }
}

/// Asks every running plugin that provides service, through the broker,
/// and returns their answers. None is not an error: nobody provides it, or
/// the broker isn't running.
pub fn ask_service(ctx: &Ctx, service: &str) -> Result<Vec<Answer>, String> {
    let Ok(c) = client::dial_broker() else {
        return Ok(Vec::new());
    };
    #[derive(Serialize)]
    struct Params<'a> {
        service: &'a str,
    }
    let out: Result<Option<Vec<Answer>>, _> = c.call(ctx, "services.ask", &Params { service });
    c.close();
    out.map(Option::unwrap_or_default)
        .map_err(|e| e.to_string())
}

/// Names the plugins that run and provide service. It reads the disk, so
/// it's not for the UI thread.
pub fn providers(service: &str) -> Vec<String> {
    // enabled() is by name, so in order already.
    crate::bundled::enabled()
        .into_iter()
        .filter(|(_, a)| {
            a.manifest.provides.iter().any(|p| p == service) && a.manifest.unmet().is_empty()
        })
        .map(|(n, _)| n)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // disk.candidates' answer, as rush-fleet's disk::from_answer reads it:
    // camelCase, what wasn't looked at left out.
    #[test]
    fn disk_candidates_on_the_wire() {
        let a = DiskCandidates {
            candidates: vec![DiskCandidate {
                path: "/Users/me/src/app/node_modules".into(),
                kind: "node-modules".into(),
                why: "app's packages".into(),
                bytes_on_disk: 900,
                bytes_freeable: 800,
                facts: DiskFacts {
                    mtime: Some(1_700_000_000_000),
                    in_use: "no".into(),
                    session: "s1".into(),
                    ..Default::default()
                },
            }],
        };
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({"candidates": [{
                "path": "/Users/me/src/app/node_modules",
                "kind": "node-modules",
                "why": "app's packages",
                "bytesOnDisk": 900,
                "bytesFreeable": 800,
                "facts": {"mtime": 1_700_000_000_000i64, "inUse": "no", "session": "s1"}
            }]})
        );
        assert_eq!(
            serde_json::to_value(DiskCandidatesAsk::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn service_names() {
        let ok = Manifest {
            name: "a".into(),
            command: vec!["rush".into()],
            provides: vec!["cleanup.report".into()],
            uses: vec!["x.y-z".into()],
            ..Default::default()
        };
        ok.validate_services().unwrap();
        for bad in ["cleanup", "Cleanup.Report", "a..b", ".a"] {
            let m = Manifest {
                name: "a".into(),
                provides: vec![bad.into()],
                ..Default::default()
            };
            assert!(m.validate_services().is_err(), "{bad} should be refused");
        }
        let mcp = Manifest {
            name: "a".into(),
            protocol: "mcp".into(),
            provides: vec!["a.b".into()],
            ..Default::default()
        };
        assert!(
            mcp.validate_services().is_err(),
            "an MCP plugin can't provide"
        );
    }
}
