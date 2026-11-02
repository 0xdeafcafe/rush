//! Finding the thread a spawn_agent call started, among the rollouts.
//! Port of internal/adapters/codex/children.go:1-83.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use rush_core::adapter::{Kind, Profile, Session};
use rush_core::time::{Ms, now};

use crate::KIND;
use crate::discover::{day_dir, is_rollout, local_day};
use crate::items::path_base;
use crate::json::{Envelope, get, ms, parse, s};
use crate::rollout::read_lines;

const DAY: Ms = 24 * 3600 * 1000;

/// The thread parent spawned as child: its thread id, or the task it was
/// given (spawn_agent's task_name). Rollouts are filed by the day they
/// began, so only those days' folders are looked in.
pub(crate) fn find_child(p: &Profile, parent: &str, child: &str, start: Ms) -> Option<Session> {
    if parent.is_empty() || child.is_empty() {
        return None;
    }
    let from = start - 3000;
    // Filed by the local day, begun a day early for a clock read elsewhere.
    let last = local_day(now() + DAY, 0);
    // A year at most, for a start time far in the past.
    for n in 0..=366 {
        let day = local_day(from, 1 - n);
        if day > last {
            break;
        }
        let dir = day_dir(&p.dir, day);
        let mut ents: Vec<_> = std::fs::read_dir(&dir)
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        ents.sort_by_key(|e: &std::fs::DirEntry| e.file_name());
        for e in ents {
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_rollout(&name) {
                continue;
            }
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as Ms);
            if modified < from {
                continue;
            }
            if let Some(mut s) = child_of(&dir.join(&name), parent, child) {
                (s.kind, s.profile) = (Kind::from(KIND), p.clone());
                return Some(s);
            }
        }
    }
    None
}

/// The rollout at file's meta: the session, when it's the thread parent
/// spawned as child.
fn child_of(file: &Path, parent: &str, child: &str) -> Option<Session> {
    let f = File::open(file).ok()?;
    let mut first = Vec::new();
    let _ = read_lines(&mut BufReader::with_capacity(64 << 10, f), |b, _| {
        first = b.to_vec();
        false
    });
    let l: Envelope = serde_json::from_slice(&first).ok()?;
    if &*l.typ != "session_meta" {
        return None;
    }
    let m = parse(l.payload?.get());
    if !m.is_object() {
        return None;
    }
    let spawn = get(&m, "source")
        .and_then(|x| get(x, "subagent"))
        .and_then(|x| get(x, "thread_spawn"))
        .cloned()
        .unwrap_or_default();
    let id = s(&m, "id");
    if s(&m, "parent_thread_id") != parent && s(&spawn, "parent_thread_id") != parent {
        return None;
    }
    if id != child && path_base(s(&spawn, "agent_path")) != path_base(child) {
        return None;
    }
    let at = ms(&l.timestamp);
    Some(Session {
        id: id.to_string(),
        name: s(&spawn, "agent_nickname").to_string(),
        transcript: file.to_string_lossy().into_owned(),
        created_at: at,
        updated_at: at,
        ..Session::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rollout::tests::{Rollout, meta, t0};
    use serde_json::json;

    // children_test.go:TestSpawnAgentChild: that thread is found by its
    // task among the rollouts, its own, apart from its parent's.
    #[test]
    fn spawn_agent_child() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        let day = day_dir(&home, local_day(t0(), 0));
        let kid = |id: &str, parent: &str, path: &str| {
            let mut m = meta(
                id,
                json!({"subagent": {"thread_spawn": {
                    "parent_thread_id": parent, "agent_path": path, "agent_nickname": "Arendt"}}}),
            );
            m["parent_thread_id"] = parent.into();
            m
        };
        Rollout::new(t0())
            .add("session_meta", kid("other", "someone", "/root/trace_bug"))
            .write(&day.join("rollout-a-other.jsonl"));
        Rollout::new(t0())
            .add("session_meta", kid("mine", "parent", "/root/trace_bug"))
            .write(&day.join("rollout-b-mine.jsonl"));
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis((t0() + 5000) as u64);
        for f in ["rollout-a-other.jsonl", "rollout-b-mine.jsonl"] {
            File::options()
                .write(true)
                .open(day.join(f))
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        let p = Profile {
            dir: home,
            ..Profile::default()
        };
        let s = find_child(&p, "parent", "trace_bug", t0()).expect("the child");
        assert_eq!(
            (s.id.as_str(), s.name.as_str(), s.kind.as_str()),
            ("mine", "Arendt", KIND)
        );
        assert!(find_child(&p, "nobody", "trace_bug", t0()).is_none());
    }
}
