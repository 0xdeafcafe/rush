//! The log of what changed how tokens are spent: the markers on the
//! Timeline, in events.jsonl. Port of internal/efficiency/events.go.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::sync::Mutex;

use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::catalog::{CATALOG, find};
use crate::detect::{Env, Found, Status};

/// Something that changed how tokens are spent: a saver installed or
/// removed, a setting changed, or a note you wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    pub at: Time,
    /// install, remove, setting, note; "used" is worked out, never kept.
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub saver: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub account: String,
    /// How it's known: rush did it, rush noticed it, it was read from an
    /// install date, or you wrote it.
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// A handover's numbers, on a "handover" event (Rust-only): what
    /// Emeritus later says it saved is worked out from them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handover: Option<Handover>,
}

/// The kind of the event a handover logs.
pub const HANDOVER: &str = "handover";

/// One Emeritus handover (docs/emeritus.md): the session that retired, at
/// what context and cost, and the one that took over, at what context it
/// started. Rust-only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Handover {
    /// The rush session that retired, and its agent's conversation.
    pub from: String,
    #[serde(rename = "fromSession", skip_serializing_if = "String::is_empty")]
    pub from_session: String,
    /// The rush session that took over, and its agent's conversation:
    /// with the folder and the account, its transcript exactly.
    pub to: String,
    #[serde(rename = "toSession", skip_serializing_if = "String::is_empty")]
    pub to_session: String,
    /// The folder both worked in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The retiring session's context, in tokens, as its last request
    /// sent it, and what it had cost.
    #[serde(rename = "fromContext")]
    pub from_context: i64,
    #[serde(rename = "fromCostUsd")]
    pub from_cost_usd: f64,
    /// The successor's context at its first request: the baton and its
    /// system prompt.
    #[serde(rename = "toContext")]
    pub to_context: i64,
    /// The threshold that nudged it, in tokens; 0 when it retired unasked.
    #[serde(skip_serializing_if = "is_zero")]
    pub threshold: i64,
    /// How many handovers led to the successor: 1 for the first.
    #[serde(skip_serializing_if = "is_zero")]
    pub lineage: i64,
    /// Whether the session was another's subagent.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub subagent: bool,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Logs a handover, rush's own doing, for account (the profile's
/// folder).
pub fn add_handover(account: &str, h: Handover) -> std::io::Result<()> {
    add_event(Event {
        kind: HANDOVER.into(),
        account: account.into(),
        source: "rush".into(),
        detail: format!(
            "{} handed over to {} at {} tokens",
            h.from, h.to, h.from_context
        ),
        handover: Some(h),
        ..Event::default()
    })
}

static EVENTS: Mutex<()> = Mutex::new(());

fn events_path() -> PathBuf {
    crate::dir().join("events.jsonl")
}

fn detected_path() -> PathBuf {
    crate::dir().join("detected.json")
}

/// The log, oldest first.
pub fn load_events() -> Vec<Event> {
    let _l = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(f) = std::fs::File::open(events_path()) else {
        return Vec::new();
    };
    let mut out: Vec<Event> = BufReader::new(f)
        .split(b'\n')
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_slice::<Event>(&l).ok())
        .filter(|e| !e.at.is_zero())
        .collect();
    out.sort_by_key(|e| e.at);
    out
}

/// Appends to the log.
pub fn add_event(mut e: Event) -> std::io::Result<()> {
    let _l = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    if e.at.is_zero() {
        e.at = Time::now();
    }
    let mut b = serde_json::to_vec(&e).map_err(std::io::Error::other)?;
    b.push(b'\n');
    std::fs::create_dir_all(crate::dir())?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(events_path())?;
    f.write_all(&b)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Seen {
    #[serde(rename = "s")]
    status: Status,
    #[serde(rename = "v", skip_serializing_if = "String::is_empty")]
    value: String,
}

type Detected = HashMap<String, HashMap<String, Seen>>;

fn read_detected() -> Detected {
    std::fs::read(detected_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_detected(all: &Detected) {
    if let Ok(b) = serde_json::to_vec(all) {
        let _ = crate::write_file(&detected_path(), &b);
    }
}

/// Detects every saver in an account and logs what changed since it last
/// looked. The first look at an account logs only what has a known
/// install date. It returns what was found, by saver ID.
pub fn observe(env: &Env) -> HashMap<String, Found> {
    let found: HashMap<String, Found> = CATALOG
        .iter()
        .map(|s| (s.id.to_string(), env.detect(s)))
        .collect();
    let mut all = {
        let _l = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
        read_detected()
    };
    let acct = env.profile.dir.clone();
    let before = all.get(&acct).cloned();
    let mut now = HashMap::new();
    let mut ids: Vec<&String> = found.keys().collect();
    ids.sort();
    for id in ids {
        let f = &found[id];
        now.insert(
            id.clone(),
            Seen {
                status: f.status,
                value: f.value.clone(),
            },
        );
        let Some(sv) = find(id) else { continue };
        let ev = |kind: &str, source: &str, detail: String, at: Time| Event {
            at,
            kind: kind.into(),
            saver: id.clone(),
            account: acct.clone(),
            source: source.into(),
            detail,
            handover: None,
        };
        let Some(before) = &before else {
            if f.status != Status::Off && !f.since.is_zero() {
                let _ = add_event(ev("install", "install date", String::new(), f.since));
            }
            continue;
        };
        let Some(was) = before.get(id) else { continue };
        if was.status == f.status && was.value == f.value {
            continue;
        }
        if let Some(st) = &sv.setting {
            let detail = if f.status == Status::Off {
                format!("{} unset", st.key)
            } else {
                format!("{} = {}", st.key, f.value)
            };
            let _ = add_event(ev("setting", "noticed", detail, Time::default()));
        } else if f.status == Status::Off {
            let _ = add_event(ev("remove", "noticed", String::new(), Time::default()));
        } else if was.status == Status::Off || f.status == Status::On {
            let _ = add_event(ev(
                "install",
                "noticed",
                f.parts.join(", "),
                Time::default(),
            ));
        }
    }
    let _l = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    all.insert(acct, now);
    write_detected(&all);
    found
}

/// Records what rush itself just changed, so observe doesn't log it a
/// second time as noticed.
pub fn remember(env: &Env, id: &str, f: &Found) {
    let _l = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_detected();
    all.entry(env.profile.dir.clone()).or_default().insert(
        id.to_string(),
        Seen {
            status: f.status,
            value: f.value.clone(),
        },
    );
    write_detected(&all);
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    // A handover is logged with its numbers and read back whole; an event
    // an older rush wrote has none.
    #[test]
    fn handovers_are_logged_with_their_numbers() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let h = Handover {
            from: "a1".into(),
            from_session: "s-a".into(),
            to: "b1".into(),
            to_session: "s-b".into(),
            cwd: "/work/app".into(),
            agent: "claude".into(),
            model: "opus".into(),
            from_context: 307_000,
            from_cost_usd: 15.55,
            to_context: 22_000,
            threshold: 300_000,
            lineage: 1,
            subagent: true,
        };
        add_handover("/acct", h.clone()).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(events_path())
            .unwrap()
            .write_all(b"{\"at\":\"2026-01-02T03:04:05Z\",\"kind\":\"note\",\"source\":\"you\"}\n")
            .unwrap();
        let evs = load_events();
        let got: Vec<(&str, Option<&Handover>)> = evs
            .iter()
            .map(|e| (e.kind.as_str(), e.handover.as_ref()))
            .collect();
        assert_eq!(got, vec![("note", None), (HANDOVER, Some(&h))]);
        let ev = &evs[1];
        assert!(
            ev.account == "/acct" && ev.source == "rush" && ev.detail.contains("a1"),
            "{ev:?}"
        );
        let line = std::fs::read_to_string(events_path()).unwrap();
        for key in [
            "\"fromContext\":307000",
            "\"fromCostUsd\":15.55",
            "\"toContext\":22000",
            "\"subagent\":true",
            "\"fromSession\":\"s-a\"",
            "\"toSession\":\"s-b\"",
            "\"cwd\":\"/work/app\"",
        ] {
            assert!(line.contains(key), "{key} in {line}");
        }
        rush_state::set_thread_dir(None);
    }
}
