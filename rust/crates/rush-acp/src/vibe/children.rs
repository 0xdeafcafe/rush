//! A Vibe task's child session: saved under its parent's folder
//! (agents/<agent>_…) and listed in the parent's meta.json by the call
//! that started it. Port of internal/adapters/vibe/children.go:1-61.

use std::path::Path;

use rush_core::adapter::{ChildFinder, Discoverer, Kind, Profile, Session};
use rush_core::time::{Ms, parse_rfc3339};
use serde::Deserialize;

use super::{KIND, Vibe};
use crate::util::{is_local, mod_ms, path_str};

impl ChildFinder for Vibe {
    fn find_child(&self, p: &Profile, parent: &str, child: &str, _: Ms) -> Option<Session> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Child {
            session_id: String,
            tool_call_id: String,
            agent: String,
            relative_path: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Meta {
            child_sessions: Vec<Child>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct ChildMeta {
            start_time: String,
        }
        if parent.is_empty() || child.is_empty() {
            return None;
        }
        let s = self.past(p).into_iter().find(|s| s.id == parent)?;
        let dir = Path::new(&s.transcript).parent()?.to_path_buf();
        let b = std::fs::read(dir.join("meta.json")).ok()?;
        let meta: Meta = serde_json::from_slice(&b).ok()?;
        let c = meta
            .child_sessions
            .into_iter()
            .find(|c| c.tool_call_id == child && is_local(&c.relative_path))?;
        let file = dir.join(&c.relative_path).join("messages.jsonl");
        let st = std::fs::metadata(&file).ok()?;
        let mut at = mod_ms(&st);
        if let Ok(b) = std::fs::read(dir.join(&c.relative_path).join("meta.json"))
            && let Ok(cm) = serde_json::from_slice::<ChildMeta>(&b)
        {
            let t = parse_rfc3339(&cm.start_time);
            if t != 0 {
                at = t;
            }
        }
        Some(Session {
            kind: Kind::from(KIND),
            profile: p.clone(),
            id: c.session_id,
            name: c.agent,
            cwd: s.cwd,
            transcript: path_str(&file),
            state: "done".into(),
            created_at: at,
            updated_at: mod_ms(&st),
            ..Session::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::read_history;
    use rush_core::event::Event;

    // children_test.go: TestFindChild
    #[test]
    fn find_child() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let write = |rel: &str, body: &str| {
            let path = dir.join("logs/session").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        let parent = "session_1_aaaa/";
        write(
            &format!("{parent}meta.json"),
            r#"{"session_id":"P","start_time":"2026-01-01T00:00:00Z","child_sessions":[
{"session_id":"C0","tool_call_id":"t0","agent":"explore","relative_path":"agents/explore_0_c0"},
{"session_id":"C1","tool_call_id":"t1","agent":"explore","relative_path":"agents/explore_1_c1"}]}"#,
        );
        write(
            &format!("{parent}messages.jsonl"),
            "{\"role\":\"user\",\"content\":\"look\"}\n{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"id\":\"t1\",\"function\":{\"name\":\"task\",\"arguments\":\"{\\\"task\\\":\\\"find it\\\",\\\"agent\\\":\\\"explore\\\"}\"}}]}\n",
        );
        write(
            &format!("{parent}agents/explore_0_c0/messages.jsonl"),
            "{\"role\":\"user\",\"content\":\"decoy\"}\n",
        );
        write(
            &format!("{parent}agents/explore_1_c1/meta.json"),
            r#"{"session_id":"C1","parent_session_id":"P","start_time":"2026-01-01T00:00:05Z"}"#,
        );
        write(
            &format!("{parent}agents/explore_1_c1/messages.jsonl"),
            "{\"role\":\"user\",\"content\":\"find it\"}\n{\"role\":\"assistant\",\"content\":\"found\"}\n",
        );
        write(
            "session_2_bbbb/meta.json",
            r#"{"session_id":"Q","start_time":"2026-01-01T00:00:00Z","child_sessions":[{"session_id":"X","tool_call_id":"t1","agent":"explore","relative_path":"agents/x"}]}"#,
        );
        write(
            "session_2_bbbb/messages.jsonl",
            "{\"role\":\"user\",\"content\":\"hi\"}\n",
        );

        let a = Vibe::without_keychain();
        let p = Profile {
            dir: path_str(dir),
            ..Profile::default()
        };
        let got = a.find_child(&p, "P", "t1", 0).unwrap();
        assert_eq!(got.id, "C1");
        assert_eq!(
            got.transcript,
            path_str(
                &dir.join("logs/session")
                    .join(parent)
                    .join("agents/explore_1_c1/messages.jsonl")
            )
        );
        assert_eq!(got.created_at, parse_rfc3339("2026-01-01T00:00:05Z"));
        assert!(a.find_child(&p, "P", "t9", 0).is_none());

        let evs = read_history(&a, &got, 0).unwrap();
        let Event::Message(m) = &evs[1] else {
            panic!("{evs:?}")
        };
        assert_eq!(
            (m.role.as_str(), m.parts[0].text.as_str()),
            ("assistant", "found")
        );

        let parent_log = Session {
            transcript: path_str(&dir.join("logs/session").join(parent).join("messages.jsonl")),
            ..Session::default()
        };
        let evs = read_history(&a, &parent_log, 0).unwrap();
        let Event::Message(m) = &evs[1] else {
            panic!("{evs:?}")
        };
        assert_eq!(m.parts[0].call.as_ref().unwrap().input.child, "t1");
    }
}
