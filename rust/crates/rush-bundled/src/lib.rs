//! The plugins that ship inside rush, rush's own MCP server inside a
//! session, and rush's status line for Claude Code.
//!
//! Ported from Go `internal/bundled/*` (clean, cleanup, drafts, drive,
//! gate, haven, hexview, prwatch, queue, searchguard, ts, yap),
//! `internal/agtools` and `internal/statusline`. Each bundled plugin has
//! the name, manifest and behaviour its Go one has, so Go and Rust rush
//! share their switches, settings and data folders.
//!
//! A bundled plugin's code is rush's own: the broker starts it as `rush
//! plugin run <name>`, speaking rush's protocol on fd 3 (see
//! rush_plugin::bundled). Go registers each from its package's init; here
//! [`register`] does it, once, as rush starts.

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Once};

use rush_plugin::rpc::{Handler, Raw, no_method, to_raw};
use rush_plugin::{Bundle, Conn, Error};
use serde::Serialize;

// pub mod agtools;
// pub mod clean;
pub mod cleanup;
pub mod drafts;
pub mod drive;
pub mod gate;
pub mod gatecore;
pub mod haven;
pub mod hexview;
pub mod prwatch;
pub mod queue;
pub mod searchguard;
// pub mod statusline;
// pub mod ts;
pub mod yap;

mod exec;

/// Every plugin bundled with rush, by name.
pub fn bundles() -> Vec<Bundle> {
    let mut out = vec![
        cleanup::bundle(),
        drafts::bundle(),
        drive::bundle(),
        gate::bundle(),
        haven::bundle(),
        hexview::bundle(),
        prwatch::bundle(),
        queue::bundle(),
        searchguard::bundle(),
        yap::bundle(),
    ];
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    out
}

/// Makes every bundled plugin known to rush_plugin, once: what Go's
/// packages do in their init.
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for b in bundles() {
            // Registered for the life of the process.
            let _ = rush_plugin::register_bundle(b);
        }
    });
}

/// A handler's answer: v as JSON.
pub(crate) fn answer<T: Serialize + ?Sized>(v: &T) -> Result<Option<Raw>, Error> {
    to_raw(v).map(Some)
}

/// `{}`, what most calls are answered with.
pub(crate) fn empty() -> Result<Option<Raw>, Error> {
    answer(&serde_json::Map::new())
}

/// `{"tools": []}`.
pub(crate) fn no_tools() -> Result<Option<Raw>, Error> {
    answer(&serde_json::json!({ "tools": [] }))
}

/// Runs a plugin's handler on its connection to the broker until the
/// broker hangs up.
pub(crate) fn serve(s: UnixStream, h: Handler) -> Result<(), String> {
    let conn = Conn::new(s, Some(h)).map_err(|e| e.to_string())?;
    conn.wait();
    Ok(())
}

/// The Run of a plugin that has nothing to do but be on: rush does its
/// work, and asks whether it's on. After the Run of Go's hexview, drive,
/// prwatch, cleanup, searchguard and yap.
pub(crate) fn run_switch(s: UnixStream) -> Result<(), String> {
    serve(
        s,
        Arc::new(|_, _, method, _| match method {
            "initialize" => empty(),
            "tools.list" => no_tools(),
            _ => Err(no_method(method)),
        }),
    )
}

/// A plugin's manifest with what every bundled one has: rush as its
/// command, and its memory limit.
pub(crate) fn manifest(
    name: &str,
    category: &str,
    description: &str,
    memory_mb: i64,
) -> rush_plugin::Manifest {
    rush_plugin::Manifest {
        name: name.into(),
        category: category.into(),
        description: description.into(),
        command: vec!["rush".into()],
        memory_mb,
        ..Default::default()
    }
}

/// A bundle of m run by run.
pub(crate) fn bundle_of(
    m: rush_plugin::Manifest,
    optional: bool,
    run: fn(UnixStream) -> Result<(), String>,
) -> Bundle {
    Bundle {
        manifest: m,
        optional,
        run: Some(Arc::new(run)),
        switched: None,
    }
}

/// An MCP tool's result: `{"content":[{"type":"text","text":…}],"isError":…}`.
pub(crate) fn tool_result(text: &str, is_err: bool) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_err,
    })
}

/// A Go time.Duration as its String method writes it: "10m0s", "1h0m0s",
/// "1.5s", "300ms".
pub(crate) fn go_duration(nanos: i64) -> String {
    if nanos == 0 {
        return "0s".into();
    }
    let neg = nanos < 0;
    let u = nanos.unsigned_abs();
    let mut out = String::new();
    if u < 1_000_000_000 {
        // Under a second: ns, µs or ms, with a fraction.
        let (unit, div) = if u < 1_000 {
            ("ns", 1)
        } else if u < 1_000_000 {
            ("µs", 1_000)
        } else {
            ("ms", 1_000_000)
        };
        out.push_str(&frac(u, div));
        out.push_str(unit);
    } else {
        let h = u / 3_600_000_000_000;
        let m = u / 60_000_000_000 % 60;
        let s = u % 60_000_000_000;
        if h > 0 {
            out.push_str(&format!("{h}h{m}m"));
        } else if m > 0 {
            out.push_str(&format!("{m}m"));
        }
        out.push_str(&frac(s, 1_000_000_000));
        out.push('s');
    }
    if neg {
        out.insert(0, '-');
    }
    out
}

/// n/div with its fraction, trailing zeros dropped.
fn frac(n: u64, div: u64) -> String {
    let whole = n / div;
    let rest = n % div;
    if rest == 0 {
        return whole.to_string();
    }
    let digits = div.to_string().len() - 1;
    let f = format!("{rest:0digits$}");
    format!("{whole}.{}", f.trim_end_matches('0'))
}

#[cfg(test)]
pub(crate) mod testutil;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_durations() {
        for (n, want) in [
            (0, "0s"),
            (600_000_000_000, "10m0s"),
            (3_600_000_000_000, "1h0m0s"),
            (1_500_000_000, "1.5s"),
            (300_000_000, "300ms"),
            (5_400_000_000_000, "1h30m0s"),
            (1_500, "1.5µs"),
        ] {
            assert_eq!(go_duration(n), want, "{n}");
        }
    }

    #[test]
    fn every_bundle_registers() {
        let _b = testutil::bundles_lock();
        let names: Vec<String> = bundles().into_iter().map(|b| b.manifest.name).collect();
        assert_eq!(
            names,
            [
                "cleanup",
                "drafts",
                "drive",
                "gate",
                "git-clean",
                "go-clean",
                "haven",
                "hex",
                "js-clean",
                "queue",
                "search-guard",
                "ts",
                "watch",
                "yap"
            ]
        );
        // Each manifest is one a bundled plugin may have: register panics
        // on one that isn't.
        for b in bundles() {
            rush_plugin::register_bundle(b).forget();
        }
    }
}
