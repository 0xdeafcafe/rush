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
