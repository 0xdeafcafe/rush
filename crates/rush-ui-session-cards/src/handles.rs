//! Which agent each @tag names, told to the conversation so an @agent
//! draws in its colour and opens it when clicked; and the tag the pointer
//! is over, underlined, with who it is said under the box. Port of
//! internal/ui/pane_pointer.go (setHandles :190, sameHandles :236,
//! linkedAgent :154, the tag in hoverPane :58, underlineLink :168).

use std::collections::HashMap;

use rush_convo::handles::{Handle, set_handles};
use rush_core::agent::Agent;
use rush_core::text::{Line, Link, Mods};

use rush_room::handle::Names;

use super::mention::{ROOM_KEY_PREFIX, handle_tint, mention_name, mentionable, tags};
use crate::model::Model;
use crate::style::one_line;
use rush_ui_base::mention::{handle_key, kebab};

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

// An agent's host id (the frame's, crate::mention) gives it its handle.
pub use rush_ui_base::mention::host_id;

/// The id a mention of a keeps, when rush runs it (not a room).
fn kept(a: &Agent) -> Option<&str> {
    host_id(a).filter(|id| !id.is_empty() && !a.key.starts_with(ROOM_KEY_PREFIX))
}

/// Tells rush_room::handle every session rush runs, by its title now, so
/// whatever draws a handle draws the same one; and tells the conversation
/// which agent each tag names: its handle and a kept @{id} first, then
/// its title and the animal handle it had before where nothing else
/// does. Only when what they're worked out from changed (setHandles,
/// sameHandles).
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
    rush_room::handle::set_known(Names::new(m.order.iter().filter_map(|&i| {
        let a = &m.agents[i];
        kept(a).map(|id| (id.to_string(), one_line(&a.display_name)))
    })));
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
                tint: handle_tint(&handle_key(a)),
                title: one_line(&a.display_name),
                name: name.clone(),
            };
            if pass == 0 {
                add(&name, &h);
                if let Some(id) = kept(a) {
                    add(&format!("{{{id}}}"), &h);
                }
                continue;
            }
            add(&kebab(&one_line(&a.display_name)), &h);
            if let Some(id) = kept(a) {
                add(&rush_room::handle::legacy_name(id), &h);
            }
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
        let mut m = crate::model::Model::bare("test", |_| {});
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
        assert_eq!(name, "docs-site", "its title, kebab-cased");
        let h3 = rush_convo::handles::handle_of("@{b1b2c3d4}").expect("its kept id tags it");
        assert_eq!(h3.name, name);
        let old = format!("@{}", rush_room::handle::legacy_name("b1b2c3d4"));
        let h4 = rush_convo::handles::handle_of(&old).expect("its old animal handle tags it");
        assert_eq!(h4.name, name);
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
