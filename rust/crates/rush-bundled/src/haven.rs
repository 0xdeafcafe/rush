//! The bundled plugin for Haven, the tool that runs your projects' stacks
//! and cleans up after them. It's off until you turn it on, runs only
//! where haven is installed, and answers the cleanup.report service: what
//! Haven would clean (haven machine clean, which only reports without
//! --yes) and what its stacks hold in memory, with what a cleanup agent may
//! do about it.
//! Ported from internal/bundled/haven/plugin.go:1-91.

use std::os::unix::net::UnixStream;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use rush_plugin::rpc::{from_raw, no_method};
use rush_plugin::{Bundle, CleanupReport, Ctx, Manifest, Requires, SERVICE_CLEANUP_REPORT};
use serde::Deserialize;

/// The plugin's.
pub const NAME: &str = "haven";

/// Haven's.
pub fn manifest() -> Manifest {
    Manifest {
        requires: Requires {
            bin: vec!["haven".into()],
            ..Default::default()
        },
        provides: vec![SERVICE_CLEANUP_REPORT.into()],
        ..crate::manifest(
            NAME,
            "machine",
            "Tells rush's cleanup what Haven knows: what haven machine clean would remove and what its stacks \
             hold in memory, and how a cleanup agent may free it without losing data. Off until you turn it on; runs where haven is installed.",
            32,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, run)
}

/// What one haven command may take.
const WAIT: Duration = Duration::from_secs(30);

/// What a cleanup agent may do with Haven's report.
const RULES: &str = "- If Haven's report ticks things, haven machine clean --yes removes temporary and merged worktrees, cold job scratch and build caches, and never drops databases.
- For memory, a stack whose worktree nobody works in, that isn't the focus stack, may be stopped with haven down --stack <slug>, which keeps its data.
- Never haven down --destroy, db reset or db prune.";

#[derive(Deserialize, Default)]
#[serde(default)]
struct ServiceCall {
    service: String,
}

/// The plugin's main.
fn run(s: UnixStream) -> Result<(), String> {
    crate::serve(
        s,
        Arc::new(|_, ctx, method, params| match method {
            "initialize" => crate::empty(),
            "tools.list" => crate::no_tools(),
            "service.call" => {
                let input: ServiceCall = from_raw(params).unwrap_or_default();
                if input.service == SERVICE_CLEANUP_REPORT {
                    return crate::answer(&report(ctx));
                }
                Err(no_method(method))
            }
            _ => Err(no_method(method)),
        }),
    )
}

/// Haven's cleanup report: nothing is removed to make it.
pub fn report(ctx: &Ctx) -> CleanupReport {
    CleanupReport {
        title: "Haven".into(),
        report: format!(
            "haven machine clean (its report; nothing was removed):\n{}\n\nhaven status --json stacks,footprint,focus:\n{}",
            haven(ctx, &["machine", "clean"]),
            haven(ctx, &["status", "--json", "stacks,footprint,focus"])
        ),
        rules: RULES.into(),
    }
}

fn haven(ctx: &Ctx, args: &[&str]) -> String {
    let mut cmd = Command::new("haven");
    cmd.args(args).env("HAVEN_AGENT", "1").env("NO_COLOR", "1");
    let out = crate::exec::run(cmd, ctx, Some(WAIT), true)
        .map(|o| o.stdout)
        .unwrap_or_default();
    let s = String::from_utf8_lossy(&out).trim().to_string();
    if s.chars().count() > 6000 {
        return s.chars().take(6000).collect::<String>() + "\n…";
    }
    s
}
