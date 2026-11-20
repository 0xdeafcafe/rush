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
//!
//! Rust-only: an ask may name one plugin and carry params, which reach
//! it as service.call's params (ask_service_of): how Projects has a kit
//! clean one item of its cleanup.report through cleanup.clean.

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
    /// What it found, row by row, for Projects to show and clean one by
    /// one through cleanup.clean; left out by a plugin that only has text.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<CleanupItem>,
}

/// One thing a cleanup.report found: a row Projects shows, cleaned by its
/// id through cleanup.clean.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CleanupItem {
    /// What cleanup.clean is asked with: the plugin's own, unique in its
    /// report.
    pub id: String,
    pub title: String,
    /// Its blocks on disk; 0 when it's no size (meta says what it is).
    #[serde(skip_serializing_if = "is_zero")]
    pub bytes: u64,
    /// Shown instead of a size: "3 branches", "kept".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub meta: String,
    /// Where it is, when it's a folder or a file.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// The worktree it's in, when it's in one (target/, node_modules).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree: String,
    /// Cleaning it loses nothing.
    pub can_go: bool,
    /// A cache that fills again by itself, as it's used.
    pub comes_back: bool,
    /// What cleaning it does, in words: "deletes it: a build puts it back".
    pub does: String,
}

/// Cleans one thing a plugin's cleanup.report listed, by its id:
/// CleanupClean in, CleanupDone out, or an error saying why not.
pub const SERVICE_CLEANUP_CLEAN: &str = "cleanup.clean";

/// What cleanup.clean is asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupClean {
    pub id: String,
}

/// A cleanup.clean answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupDone {
    /// What it did, in words: "target in wt: freed 1.2 GB."
    pub said: String,
    /// What it freed, as measured after; 0 when not known.
    #[serde(skip_serializing_if = "is_zero")]
    pub freed: u64,
}

/// What a plugin finds on disk that rush's disk engine may clean
/// (rush-fleet's disk module): DiskCandidates. The plugin removes nothing
/// itself; the engine's rules decide, and its removals ask whether
/// anything uses the path right before.
pub const SERVICE_DISK_CANDIDATES: &str = "disk.candidates";

/// What a disk.candidates ask may say: the level rush cleans at ("normal"
/// or "low") and the free bytes. rush's disk engine asks with no
/// parameters yet, so a plugin gets this empty and reports everything it
/// found.
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
    /// What it is, which plugin-candidate rules name: its ecosystem, a
    /// dot, then lowercase and dashes ("js.node-modules", "rust.sccache";
    /// the bundled clean-ups' are in rush-bundled's clean/kits.rs).
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
/// How long a cleanup.clean may take: a kit gives a clean ten minutes (git
/// gc on a big repository, a cache of many files).
pub const SERVICE_CLEAN_TIMEOUT: Duration = Duration::from_secs(11 * 60);

/// How long one plugin has to answer service.
pub fn service_timeout(service: &str) -> Duration {
    if service == SERVICE_CLEANUP_CLEAN {
        SERVICE_CLEAN_TIMEOUT
    } else {
        SERVICE_TIMEOUT
    }
}

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

/// Asks one running plugin for service with params, through the broker,
/// and returns its answer's result. That it isn't running or doesn't
/// provide it is an error, as is the broker not running.
pub fn ask_service_of<P: Serialize>(
    ctx: &Ctx,
    plugin: &str,
    service: &str,
    params: &P,
) -> Result<Raw, String> {
    let c = client::dial_broker().map_err(|_| "the plugin broker isn't running".to_string())?;
    #[derive(Serialize)]
    struct Params<'a, P> {
        service: &'a str,
        plugin: &'a str,
        params: &'a P,
    }
    let out: Result<Option<Vec<Answer>>, _> = c.call(
        ctx,
        "services.ask",
        &Params {
            service,
            plugin,
            params,
        },
    );
    c.close();
    one_answer(
        out.map_err(|e| e.to_string())?.unwrap_or_default(),
        plugin,
        service,
    )
}

/// plugin's answer among answers: its result, or its error.
fn one_answer(answers: Vec<Answer>, plugin: &str, service: &str) -> Result<Raw, String> {
    let a = answers
        .into_iter()
        .find(|a| a.plugin == plugin)
        .ok_or_else(|| format!("{plugin} isn't running, or doesn't answer {service}"))?;
    if !a.error.is_empty() {
        return Err(a.error);
    }
    a.result.ok_or_else(|| format!("{plugin} said nothing"))
}

/// Asks plugin to clean the item of its cleanup.report with id, and waits
/// for it (up to SERVICE_CLEAN_TIMEOUT). It's not for the UI thread.
pub fn clean_item(plugin: &str, id: &str) -> Result<CleanupDone, String> {
    let ctx = Ctx::background().with_timeout(SERVICE_CLEAN_TIMEOUT + Duration::from_secs(5));
    let raw = ask_service_of(
        &ctx,
        plugin,
        SERVICE_CLEANUP_CLEAN,
        &CleanupClean { id: id.into() },
    )?;
    serde_json::from_str(raw.get()).map_err(|e| format!("{plugin}: {e}"))
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

    // cleanup.report on the wire: an old report, with no items, reads as
    // it did and writes no items; one with items keeps them, camelCase,
    // what's empty left out.
    #[test]
    fn cleanup_report_on_the_wire() {
        let old: CleanupReport =
            serde_json::from_str(r#"{"title":"Haven","report":"2 GB of caches"}"#).unwrap();
        assert_eq!(
            old,
            CleanupReport {
                title: "Haven".into(),
                report: "2 GB of caches".into(),
                ..Default::default()
            }
        );
        assert_eq!(
            serde_json::to_value(&old).unwrap(),
            serde_json::json!({"title": "Haven", "report": "2 GB of caches"})
        );
        let rep = CleanupReport {
            title: "rust-clean".into(),
            report: "- target in wt · 1.0G".into(),
            rules: "safe".into(),
            items: vec![
                CleanupItem {
                    id: "target:/w/wt".into(),
                    title: "target in wt".into(),
                    bytes: 1 << 30,
                    path: "/w/wt/target".into(),
                    worktree: "/w/wt".into(),
                    can_go: true,
                    comes_back: true,
                    does: "deletes it: a build puts it back".into(),
                    ..Default::default()
                },
                CleanupItem {
                    id: "merged:/w".into(),
                    title: "w: branches merged into origin/main".into(),
                    meta: "3 branches".into(),
                    can_go: true,
                    does: "deletes them locally".into(),
                    ..Default::default()
                },
            ],
        };
        let v = serde_json::to_value(&rep).unwrap();
        assert_eq!(
            v["items"],
            serde_json::json!([
                {"id": "target:/w/wt", "title": "target in wt", "bytes": 1u64 << 30,
                 "path": "/w/wt/target", "worktree": "/w/wt", "canGo": true,
                 "comesBack": true, "does": "deletes it: a build puts it back"},
                {"id": "merged:/w", "title": "w: branches merged into origin/main",
                 "meta": "3 branches", "canGo": true, "comesBack": false,
                 "does": "deletes them locally"}
            ])
        );
        let back: CleanupReport = serde_json::from_value(v).unwrap();
        assert_eq!(back, rep);
        let done = CleanupDone {
            said: "done.".into(),
            freed: 0,
        };
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            serde_json::json!({"said": "done."})
        );
        assert_eq!(
            serde_json::to_value(CleanupClean { id: "gc:/w".into() }).unwrap(),
            serde_json::json!({"id": "gc:/w"})
        );
    }

    // ask_service_of: the plugin named's result, its error, or that it
    // didn't answer.
    #[test]
    fn one_plugins_answer() {
        let raw = |s: &str| serde_json::value::RawValue::from_string(s.into()).unwrap();
        let answers = || {
            vec![
                Answer {
                    plugin: "a".into(),
                    result: Some(raw("1")),
                    ..Default::default()
                },
                Answer {
                    plugin: "b".into(),
                    error: "no such item: x".into(),
                    ..Default::default()
                },
            ]
        };
        assert_eq!(one_answer(answers(), "a", "s.t").unwrap().get(), "1");
        assert_eq!(
            one_answer(answers(), "b", "s.t").unwrap_err(),
            "no such item: x"
        );
        assert_eq!(
            one_answer(answers(), "c", "s.t").unwrap_err(),
            "c isn't running, or doesn't answer s.t"
        );
        assert_eq!(
            service_timeout(SERVICE_CLEANUP_CLEAN),
            SERVICE_CLEAN_TIMEOUT
        );
        assert_eq!(service_timeout(SERVICE_CLEANUP_REPORT), SERVICE_TIMEOUT);
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
