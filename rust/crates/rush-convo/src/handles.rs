//! The agents an @tag names, so an @agent in the conversation draws in its
//! colour and opens it when clicked (a link to rush:agent/<name>). The UI
//! says who each tag names (set_handles); the renderer only reads them.
//! Port of internal/convo/handles.go (Handle :13, SetHandles :24,
//! HandleOf :34, AgentLink :44, agentTag :47, agentLinks :55) and the tag
//! in styledAsk (render.go:1832).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use regex::Regex;
use rush_core::text::{Color, Link, Span, Style};

use crate::render::txt::Txt;

/// An agent you can tag: its @name, the colour it goes by in the list, and
/// its title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handle {
    pub name: String,
    pub tint: Color,
    pub title: String,
}

static HANDLES: RwLock<Option<Arc<HashMap<String, Handle>>>> = RwLock::new(None);

/// Who each tag names now, by tag in lower case. What was drawn with the
/// ones before is drawn again.
pub fn set_handles(hs: HashMap<String, Handle>) {
    let mut cur = HANDLES.write().unwrap_or_else(PoisonError::into_inner);
    if cur.as_deref() == Some(&hs) {
        return;
    }
    *cur = Some(Arc::new(hs));
    drop(cur);
    crate::linkinfo::redraw();
}

fn handles() -> Option<Arc<HashMap<String, Handle>>> {
    HANDLES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The agent tag names, any case, with or without its @.
pub fn handle_of(tag: &str) -> Option<Handle> {
    let hs = handles()?;
    hs.get(&tag.trim_start_matches('@').to_lowercase()).cloned()
}

/// Where a tag of h links: a click on it opens that agent.
pub fn agent_link(h: &Handle) -> String {
    format!("rush:agent/{}", h.name)
}

/// tag, as written, in its agent's colour and linked to it.
pub(crate) fn agent_tag(h: &Handle, tag: &str) -> Txt {
    Txt::styled(tag, Style::fg(h.tint).bold().linked(&agent_link(h)))
}

/// @usernames as community.MentionRE writes them.
static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@[\p{L}\p{N}]+(?:-[\p{L}\p{N}]+)*").expect("mention"));

/// Each @tag in t that names an agent drawn as agent_tag does. A tag
/// starts a word (or a run styled apart from the one before): the @ in an
/// email or a path is no tag, nor one already a link's words.
pub(crate) fn agent_links(t: Txt) -> Txt {
    if handles().is_none() || !t.0.iter().any(|s| s.text.contains('@')) {
        return t;
    }
    let plain = t.plain();
    // Where each run starts in plain, and its style.
    let mut starts = Vec::with_capacity(t.0.len());
    let mut at = 0;
    for s in &t.0 {
        starts.push(at);
        at += s.text.len();
    }
    let run_of = |i: usize| starts.partition_point(|&s| s <= i) - 1;
    let mut cuts: Vec<(usize, usize, Handle)> = Vec::new();
    let b = plain.as_bytes();
    for m in MENTION.find_iter(&plain) {
        let (i, j) = (m.start(), m.end());
        let r = run_of(i);
        let boundary = starts[r] == i && r > 0 && t.0[r - 1].style != t.0[r].style;
        if i > 0 && !b" \t([{\"'`*_".contains(&b[i - 1]) && !boundary {
            continue;
        }
        if j < b.len() && (b[j] == b'/' || b[j] == b'.' && j + 1 < b.len() && b[j + 1] != b' ') {
            continue; // a path or a file: @src/x.go
        }
        if !t.0[r].style.link.is_none() {
            continue; // already a link's words
        }
        let Some(h) = handle_of(&plain[i..j]) else {
            continue;
        };
        cuts.push((i, j, h));
    }
    if cuts.is_empty() {
        return t;
    }
    let mut out = Txt::new();
    let mut ci = 0;
    for (r, s) in t.0.iter().enumerate() {
        let (lo, hi) = (starts[r], starts[r] + s.text.len());
        let mut pos = lo;
        while pos < hi {
            // The next tag that starts in this run.
            match cuts.get(ci) {
                Some(&(i, j, ref h)) if i < hi => {
                    if i > pos {
                        out.0.push(Span::new(&plain[pos..i], s.style));
                    }
                    if i >= pos {
                        out.push(agent_tag(h, &plain[i..j]));
                    }
                    pos = j.min(hi).max(pos);
                    if j <= hi {
                        ci += 1;
                    }
                }
                _ => {
                    out.0.push(Span::new(&plain[pos..hi], s.style));
                    pos = hi;
                }
            }
        }
    }
    out
}

/// Whether s's style links to an agent.
pub fn is_agent_link(l: Link) -> bool {
    l.url().is_some_and(|u| u.starts_with("rush:agent/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::style::{C_TEXT, paint};

    fn with_ann() {
        let mut hs = HashMap::new();
        hs.insert(
            "ann".to_string(),
            Handle {
                name: "ann".into(),
                tint: Color::Indexed(2),
                title: "Ann".into(),
            },
        );
        set_handles(hs);
    }

    // handles.go agentLinks: a tag starts a word; an email, a path and a
    // tag already linked are left alone.
    #[test]
    fn links_tags_that_name_agents() {
        with_ann();
        let cases: &[(&str, &[(&str, bool)])] = &[
            (
                "ask @ann now",
                &[("ask ", false), ("@ann", true), (" now", false)],
            ),
            ("@Ann", &[("@Ann", true)]),
            ("mail ann@ann.dev", &[("mail ann@ann.dev", false)]),
            ("see @ann/x.go", &[("see @ann/x.go", false)]),
            ("see @ann.go", &[("see @ann.go", false)]),
            (
                "end @ann.",
                &[("end ", false), ("@ann", true), (".", false)],
            ),
            ("(@ann)", &[("(", false), ("@ann", true), (")", false)]),
            ("@bob", &[("@bob", false)]),
        ];
        for (s, want) in cases {
            let got = agent_links(paint(C_TEXT, s));
            let runs: Vec<(&str, bool)> = got
                .0
                .iter()
                .map(|r| (r.text.as_str(), is_agent_link(r.style.link)))
                .collect();
            assert_eq!(&runs, want, "{s:?}");
        }
        let got = agent_links(paint(C_TEXT, "@ann"));
        assert_eq!(got.0[0].style.fg, Color::Indexed(2));
        assert_eq!(got.0[0].style.link.url().as_deref(), Some("rush:agent/ann"));
        // A run styled apart starts a word too; a linked one is left be.
        let t = Txt::raw("x").and(paint(C_TEXT, "@ann"));
        assert!(is_agent_link(agent_links(t).0[1].style.link));
        let t = Txt::styled("@ann", Style::default().linked("https://x.dev"));
        assert!(!is_agent_link(agent_links(t).0[0].style.link));
    }
}
