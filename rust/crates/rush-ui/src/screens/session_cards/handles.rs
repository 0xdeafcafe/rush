//! Which agent each @tag names, told to the conversation so an @agent
//! draws in its colour and opens it when clicked; and the tag the pointer
//! is over, underlined, with who it is said under the box. Port of
//! internal/ui/pane_pointer.go (setHandles :190, sameHandles :236,
//! linkedAgent :154, the tag in hoverPane :58, underlineLink :168).

use std::collections::HashMap;

use rush_convo::handles::{Handle, set_handles};
use rush_core::agent::Agent;
use rush_core::text::{Line, Link, Mods};

use super::mention::{handle_tint, mention_name, mentionable, nickname, tags, title_tag};
use crate::model::Model;
use crate::style::one_line;

/// What an agent's tags are worked out from (handleKey).
#[derive(Clone, PartialEq, Eq)]
struct HandleKey {
    key: String,
    id: String,
    title: String,
    rush: bool,
    interactive: bool,
    headless: bool,
}

/// The list's agents as the tags were last worked out from.
#[derive(Default)]
struct HandlesFor(Option<Vec<HandleKey>>);

/// An agent's host id, when rush runs it: its handle is the animal name
/// that id gives it.
pub fn host_id(a: &Agent) -> Option<&str> {
    a.rush.then_some(a.job.id.as_str())
}

/// Tells the conversation which agent each tag names: its handle first,
/// then its nickname and title where nothing else does. Only when what
/// they're worked out from changed (setHandles, sameHandles).
pub fn sync(m: &mut Model) {
    let now: Vec<HandleKey> = m
        .order
        .iter()
        .map(|&i| {
            let a = &m.agents[i];
            HandleKey {
                key: a.key.clone(),
                id: a.job.id.clone(),
                title: a.display_name.clone(),
                rush: a.rush,
                interactive: a.interactive,
                headless: a.headless,
            }
        })
        .collect();
    if m.kept::<HandlesFor>().0.as_ref() == Some(&now) {
        return;
    }
    m.kept::<HandlesFor>().0 = Some(now);
    let mut hs: HashMap<String, Handle> = HashMap::with_capacity(4 * m.order.len());
    let mut add = |tag: &str, h: &Handle| {
        let k = tag.to_lowercase();
        if !k.is_empty() {
            hs.entry(k).or_insert_with(|| h.clone());
        }
    };
    for pass in 0..2 {
        for &i in &m.order {
            let a = &m.agents[i];
            if !mentionable(a) {
                continue;
            }
            let name = mention_name(a, host_id(a));
            let h = Handle {
                tint: handle_tint(&format!("@{name}")),
                title: one_line(&a.display_name),
                name: name.clone(),
            };
            if pass == 0 {
                add(&name, &h);
                continue;
            }
            add(&nickname(&a.key), &h);
            add(&title_tag(&a.display_name), &h);
            add(
                &one_line(&a.display_name)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("-"),
                &h,
            );
        }
    }
    set_handles(hs);
}

/// The agent an @agent link opens, by its index in m.agents (linkedAgent).
pub fn linked_agent(m: &Model, u: &str) -> Option<usize> {
    let tag = u.strip_prefix("rush:agent/")?;
    m.order.iter().copied().find(|&i| {
        let a = &m.agents[i];
        mentionable(a) && tags(a, host_id(a), tag)
    })
}

/// What's said under the box while the pointer is on an @agent: who it is
/// and that a click opens it (hoverPane).
pub fn hover_words(a: &Agent) -> String {
    format!(
        "@{} · {} · click to open",
        mention_name(a, host_id(a)),
        one_line(&a.display_name)
    )
}

/// Underlines the words of each link to u in row (underlineLink).
pub fn underline_link(row: &mut Line, u: &str) {
    let l = Link::to(u);
    for s in &mut row.spans {
        if s.style.link == l {
            s.style.mods = s.style.mods.with(Mods::UNDERLINE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::text::{Span, Style};

    // mention_test.go TestMentionLinksOpenAgents: any case of its tag or
    // its title tags it; a link opens the agent it names; hovering
    // underlines the tag alone.
    #[test]
    fn tags_link_agents() {
        let mut m = Model::new("test");
        let mut docs = Agent {
            key: "b".into(),
            display_name: "docs site".into(),
            rush: true,
            ..Agent::default()
        };
        docs.job.id = "b1b2c3d4".into();
        m.agents = vec![docs.clone()];
        m.order = vec![0];
        sync(&mut m);
        let name = mention_name(&docs, host_id(&docs));
        let h = rush_convo::handles::handle_of(&format!("@{}", name.to_uppercase()))
            .expect("any case tags it");
        assert_eq!(h.name, name);
        let h2 = rush_convo::handles::handle_of("Docs-Site").expect("its title tags it");
        assert_eq!(h2.name, name);
        let u = rush_convo::handles::agent_link(&h);
        assert_eq!(linked_agent(&m, &u), Some(0));
        assert_eq!(linked_agent(&m, "rush:agent/nobody"), None);
        assert_eq!(
            hover_words(&docs),
            format!("@{name} · docs site · click to open")
        );
        let mut row = Line::from_spans(vec![
            Span::plain("hi "),
            Span::new("@x", Style::default().bold().linked(&u)),
            Span::plain(" there"),
        ]);
        underline_link(&mut row, &u);
        assert!(row.spans[1].style.mods.contains(Mods::UNDERLINE));
        assert!(!row.spans[0].style.mods.contains(Mods::UNDERLINE));
        assert!(!row.spans[2].style.mods.contains(Mods::UNDERLINE));
        rush_convo::handles::set_handles(HashMap::new());
    }
}
