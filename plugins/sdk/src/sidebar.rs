//! Arranging rush's agent list, with the `sidebar` capability: the
//! plugin's own sections, in its order, and a name and place for each agent
//! it knows by Claude Code session id. It only labels agents rush shows
//! anyway, so it grants the plugin nothing.
//!
//! [`Sidebar`] is what `sidebar.set` sends. rush's broker checks it and
//! refuses it whole if anything is out of bounds; [`Sidebar::check`] makes
//! the same check, so a plugin's tests can tell what rush will take.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// At most this many sections.
pub const MAX_SECTIONS: usize = 32;
/// At most this many agents.
pub const MAX_AGENTS: usize = 2000;
/// The longest title, of the list or a section, in characters.
pub const MAX_TITLE: usize = 64;
/// The longest agent name, in characters.
pub const MAX_NAME: usize = 200;

/// How a plugin arranges the agent list: `sidebar.set`'s params.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidebar {
    /// What the list's group-by mode is called; the plugin's name if empty.
    #[serde(default)]
    pub title: String,
    /// In the order the list shows them. None clears the arrangement.
    #[serde(default, deserialize_with = "crate::proto::nullable")]
    pub sections: Vec<Section>,
    /// By Claude Code session id (`sessionId` in `sessions.list`).
    #[serde(default, deserialize_with = "crate::proto::nullable")]
    pub agents: BTreeMap<String, Agent>,
}

/// One section of the list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
}

/// Where one agent goes, and what it's called there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Shown unless the user renamed the agent in rush; empty keeps its own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// One of the sections' titles.
    #[serde(default)]
    pub section: String,
    /// Its place in the section, from 0.
    #[serde(default)]
    pub order: i64,
}

impl Sidebar {
    /// Whether it arranges nothing: sent, it clears the arrangement.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Reads params as rush's broker does, and checks them: unknown fields
    /// are refused, and null or nothing clears it.
    pub fn parse(params: &Value) -> Result<Sidebar, String> {
        let mut s = if params.is_null() {
            Sidebar::default()
        } else {
            serde_json::from_value(params.clone()).map_err(|e| e.to_string())?
        };
        s.check()?;
        Ok(s)
    }

    /// Checks it as rush's broker does, cleaning every string of what a
    /// terminal would act on, as rush does before it keeps it. An error
    /// says what rush would refuse it for.
    pub fn check(&mut self) -> Result<(), String> {
        if self.sections.is_empty() {
            self.title.clear();
            self.agents.clear();
            return Ok(());
        }
        self.title = clean_label("title", &self.title, MAX_TITLE)?;
        if self.sections.len() > MAX_SECTIONS {
            return Err(format!(
                "{} sections: at most {MAX_SECTIONS}",
                self.sections.len()
            ));
        }
        let mut seen = HashSet::new();
        for sec in &mut self.sections {
            let t = clean_label("section title", &sec.title, MAX_TITLE)?;
            if t.is_empty() {
                return Err("a section has no title".into());
            }
            if !seen.insert(t.clone()) {
                return Err(format!("section {t:?} appears twice"));
            }
            sec.title = t;
        }
        if self.agents.len() > MAX_AGENTS {
            return Err(format!(
                "{} agents: at most {MAX_AGENTS}",
                self.agents.len()
            ));
        }
        for (id, a) in &mut self.agents {
            if !is_session_id(id) {
                return Err(format!("agent id {id:?} is not a session id"));
            }
            a.name = clean_label("agent name", &a.name, MAX_NAME)?;
            a.section = clean_label("section", &a.section, MAX_TITLE)?;
            if !seen.contains(&a.section) {
                return Err(format!(
                    "agent {id} is in section {:?}, which isn't in sections",
                    a.section
                ));
            }
        }
        Ok(())
    }
}

/// Whether s is a Claude Code session id, or anything as safe:
/// `^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`.
pub fn is_session_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// s cut to at most n characters, the last of them an ellipsis when it
/// was cut.
pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Checks a string's length and makes it one plain line.
fn clean_label(what: &str, s: &str, max: usize) -> Result<String, String> {
    let s = clean(s);
    if s.chars().count() > max {
        return Err(format!("{what} {s:?} is over {max} characters"));
    }
    Ok(s)
}

/// s as one line of plain text: escape sequences go, and so do control
/// and direction-changing characters, with line breaks and tabs turned
/// into spaces, and runs of spaces made one.
pub fn clean(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            i += esc_len(&b[i..]);
            // An escape that took one byte of a character takes it all.
            while i < b.len() && !s.is_char_boundary(i) {
                i += 1;
            }
            continue;
        }
        let Some(c) = s[i..].chars().next() else {
            break;
        };
        i += c.len_utf8();
        match c {
            '\n' | '\t' | '\r' => out.push(' '),
            c if c.is_control() || c == '\u{FFFD}' || bidi_control(c) => {}
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Unicode's Bidi_Control characters.
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The length of the escape sequence b starts with: CSI up to its final
/// byte, OSC, DCS and the like up to BEL or ST, else ESC and one more.
fn esc_len(b: &[u8]) -> usize {
    if b.len() < 2 {
        return b.len();
    }
    match b[1] {
        b'[' => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map_or(b.len(), |i| i + 3),
        b']' | b'P' | b'_' | b'^' | b'X' => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return i + 1;
                }
                if b[i] == 0x1b && i + 1 < b.len() && b[i + 1] == b'\\' {
                    return i + 2;
                }
                i += 1;
            }
            b.len()
        }
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn takes_what_rush_takes() {
        let s = Sidebar::parse(&json!({
            "title": "Kanban\u{1b}[31m",
            "sections": [{"title": "In\nProgress"}, {"title": "Done"}],
            "agents": {"8c76706f-1c00": {"name": "Fix\u{202E} login", "section": "In Progress", "order": 0}}
        }))
        .unwrap();
        assert_eq!(s.title, "Kanban");
        assert_eq!(s.sections[0].title, "In Progress");
        assert_eq!(s.agents["8c76706f-1c00"].name, "Fix login");
    }

    #[test]
    fn refuses_what_rush_refuses() {
        for (p, want) in [
            (
                json!({"sections": [{"title": "A"}], "agents": {"../x": {"section": "A"}}}),
                "not a session id",
            ),
            (
                json!({"sections": [{"title": "A"}], "agents": {"s1": {"section": "B"}}}),
                "isn't in sections",
            ),
            (
                json!({"sections": [{"title": "A"}, {"title": "A"}]}),
                "appears twice",
            ),
            (json!({"sections": [{"title": " "}]}), "no title"),
            (
                json!({"sections": [{"title": "A"}], "colour": "red"}),
                "unknown field",
            ),
            (
                json!({"sections": [{"title": "x".repeat(65)}]}),
                "over 64 characters",
            ),
        ] {
            let e = Sidebar::parse(&p).unwrap_err();
            assert!(e.contains(want), "{p}: {e}");
        }
    }

    #[test]
    fn nothing_clears_it() {
        assert!(Sidebar::parse(&Value::Null).unwrap().is_empty());
        let s = Sidebar::parse(&json!({"title": "T", "sections": [], "agents": {}})).unwrap();
        assert!(s.is_empty() && s.title.is_empty());
    }

    #[test]
    fn cleans_and_clips() {
        assert_eq!(clean("a\u{1b}]8;;http://x\u{7}b\tc\r\nd"), "ab c d");
        assert_eq!(clean("\u{1b}é ok"), "ok");
        assert_eq!(clip("hello", 5), "hello");
        assert_eq!(clip("hello!", 5), "hell…");
        assert!(is_session_id("8c76706f-1c00-4aed-9c6d-7509f3033943"));
        assert!(!is_session_id("-x") && !is_session_id("") && !is_session_id(&"a".repeat(65)));
    }
}
