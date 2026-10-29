//! #broadcast sends one message to several agents. Named in the command
//! (#broadcast all … or #broadcast @a @b …) it goes at once; without a
//! message, rush waits while you click agents in the list to pick them,
//! then the box's next message goes to every one picked. Esc gives up.
//! Port of Go's broadcast.go, with the tags it reads agents by from
//! mention.go:30-71 (mentionName, titleTag, tags, nickname, mentionable,
//! mentionsIn) and the sending of localqueue.go:61 and outside.go:42
//! (reply, replyOutside) for agents rush doesn't run.

use std::collections::HashSet;

use rush_core::adapter::Profile;
use rush_core::agent::Agent;

use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::one_line;

/// Who's being picked; None while not broadcasting.
#[derive(Default)]
pub struct Broadcast {
    pub picked: Option<HashSet<String>>,
}

const NICK_WORDS: &str = "brave quick sleepy muddy lucky fuzzy jolly tiny grumpy sunny dizzy bold calm cheeky clever cosy daring eager fancy gentle happy humble keen lively loyal merry nimble plucky proud quiet rusty scruffy shy silly snappy speedy spry sturdy swift wily witty zesty";
const NICK_NAMES: &str = "biscuit pickle waffles noodle pepper bean maple ziggy rocket bonnie mochi pudding scout banjo tofu nugget juniper dottie fudge pip heron robin wren finch puffin magpie kestrel plover osprey lark sparrow starling swift tern ibis egret pelican toucan kiwi dodo condor";

/// key's nickname: the same key always gets the same one (nickname).
pub fn nickname(key: &str) -> String {
    let mut h: u32 = 0x811c9dc5;
    for b in key.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    let words: Vec<&str> = NICK_WORDS.split(' ').collect();
    let names: Vec<&str> = NICK_NAMES.split(' ').collect();
    format!(
        "{}-{}",
        words[(h % words.len() as u32) as usize],
        names[(h / words.len() as u32 % names.len() as u32) as usize]
    )
}

/// title's first three words that aren't filler, lowercase, dashed: the
/// same tag the board shows as its @username (titleTag).
pub fn title_tag(title: &str) -> String {
    rush_room::community::tag(&one_line(title))
}

/// An agent's handle, how it's tagged: its title's tag, with no title yet
/// a nickname from its key (mentionName).
pub fn mention_name(a: &Agent) -> String {
    let t = title_tag(&a.display_name);
    if t.is_empty() { nickname(&a.key) } else { t }
}

/// Whether tag names agent a: its tag, its nickname or its title (tags).
pub fn tags(a: &Agent, tag: &str) -> bool {
    let eq = |s: &str| s.eq_ignore_ascii_case(tag) || s.to_lowercase() == tag.to_lowercase();
    eq(&mention_name(a))
        || eq(&nickname(&a.key))
        || eq(&title_tag(&a.display_name))
        || eq(&one_line(&a.display_name)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-"))
}

/// Whether a message can reach it from here (mentionable).
pub fn mentionable(a: &Agent) -> bool {
    !a.interactive && !a.headless
}

impl Model {
    /// The agents in list order.
    fn in_order(&self) -> impl Iterator<Item = &Agent> {
        self.order.iter().map(|&i| &self.agents[i])
    }

    /// The agents the words after the first in text tag (mentionsIn).
    pub(crate) fn mentions_in(&self, text: &str) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, f) in text.split_whitespace().enumerate() {
            let Some(tag) = f.strip_prefix('@') else {
                continue;
            };
            if i == 0 {
                continue;
            }
            let tag = tag.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'', '"']);
            for &j in &self.order {
                let a = &self.agents[j];
                if mentionable(a) && tags(a, tag) && !out.contains(&j) {
                    out.push(j);
                }
            }
        }
        out
    }

    /// The agents "all" means: the ones a message can reach that are still
    /// at work, not finished or past (broadcastAll).
    pub(crate) fn broadcast_all(&self) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|&i| {
                let a = &self.agents[i];
                mentionable(a) && !a.past && !super::rooms::is_room_key(&a.key)
            })
            .collect()
    }

    /// #broadcast arg (broadcastCommand).
    pub(crate) fn broadcast_command(&mut self, arg: &str) -> Out {
        let words: Vec<&str> = arg.split_whitespace().collect();
        let mut to = Vec::new();
        let mut n = 0;
        while n < words.len() {
            let w = words[n];
            if w.eq_ignore_ascii_case("all") {
                to.extend(self.broadcast_all());
                n += 1;
                continue;
            }
            let Some(tag) = w.strip_prefix('@') else {
                break;
            };
            let found = self.mentions_in(&format!("# @{tag}"));
            if found.is_empty() {
                self.flash_err(&format!("no agent called @{tag}"));
                return Out::Draw;
            }
            to.extend(found);
            n += 1;
        }
        let msg = words[n..].join(" ");
        if !msg.is_empty() && !to.is_empty() {
            return self.broadcast_to(&to, &msg);
        }
        self.kept::<Broadcast>().picked =
            Some(to.iter().map(|&i| self.agents[i].key.clone()).collect());
        if !msg.is_empty() {
            self.input.set(&msg);
        }
        self.flash(
            "broadcast: click agents to pick them, type the message, enter sends · esc stops",
        );
        Out::Draw
    }

    /// Picks or unpicks the agent at key, when it can be sent to
    /// (broadcastPick).
    pub(crate) fn broadcast_pick(&mut self, key: &str) {
        let Some(a) = self.in_order().find(|a| a.key == key) else {
            return;
        };
        if !mentionable(a) || super::rooms::is_room_key(key) {
            return;
        }
        let picked = self.kept::<Broadcast>().picked.get_or_insert_default();
        if !picked.remove(key) {
            picked.insert(key.to_string());
        }
        let n = picked.len();
        self.flash(&format!("broadcast to {n} · enter sends · esc stops"));
    }

    /// Sends text to the picked agents and leaves picking (sendBroadcast).
    pub(crate) fn send_broadcast(&mut self, text: &str) -> Out {
        let picked = self.kept::<Broadcast>().picked.clone().unwrap_or_default();
        let to: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&i| picked.contains(&self.agents[i].key))
            .collect();
        if to.is_empty() {
            self.flash_err("pick an agent first: click its name in the list");
            return Out::Draw;
        }
        self.broadcast_to(&to, text)
    }

    /// Sends text to each of to, once each (broadcastTo).
    fn broadcast_to(&mut self, to: &[usize], text: &str) -> Out {
        self.kept::<Broadcast>().picked = None;
        let mut seen = HashSet::new();
        for &i in to {
            let a = self.agents[i].clone();
            if !seen.insert(a.key.clone()) {
                continue;
            }
            self.reply_to(&a, text);
        }
        if seen.is_empty() {
            self.flash_err("no agents at work to send to");
            return Out::Draw;
        }
        self.flash(&format!("sent to {} agents", seen.len()));
        Out::Draw
    }

    /// Sends text to an agent rush doesn't run, through its adapter, off
    /// the UI thread (reply, replyOutside).
    pub(crate) fn reply_to(&mut self, a: &Agent, text: &str) {
        let reg = self.reg.clone();
        let (kind, account, id, name) = (
            a.kind.to_string(),
            a.job.account.clone(),
            a.job.id.clone(),
            a.display_name.clone(),
        );
        let text = text.to_string();
        self.later(
            move || {
                let Some(ad) = reg.get(&kind) else {
                    return Err(format!("{kind} isn't known to this rush"));
                };
                let Some(r) = ad.replier() else {
                    return Err(format!(
                        "{} takes no messages from outside it",
                        reg.harness_label(&kind)
                    ));
                };
                let profiles = ad.profiles();
                let p = profiles
                    .iter()
                    .find(|p| p.name == account || p.dir == account)
                    .or(profiles.first())
                    .cloned()
                    .unwrap_or_else(|| Profile {
                        kind: kind.as_str().into(),
                        ..Profile::default()
                    });
                r.reply(&p, &id, &text).map_err(|e| e.to_string())
            },
            move |m, res| {
                match res {
                    Ok(()) => m.flash(&format!("sent to {name}")),
                    Err(e) => m.flash_err(&format!("couldn't send to {name}: {e}")),
                }
                Out::Draw
            },
        );
    }
}

/// The box's message goes to the picked agents while broadcasting.
pub(super) fn submit(m: &mut Model, text: &str) -> Out {
    if m.kept::<Broadcast>().picked.is_none() {
        return Out::Pass;
    }
    let out = m.send_broadcast(text);
    m.input.clear();
    out
}

/// Esc stops picking.
pub(super) fn list_key(m: &mut Model, k: &Press) -> Out {
    if k.s != "esc" || m.kept::<Broadcast>().picked.is_none() {
        return Out::Pass;
    }
    m.kept::<Broadcast>().picked = None;
    m.flash("broadcast stopped");
    Out::Draw
}

/// A click on an agent's row picks it while broadcasting.
pub(super) fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.kept::<Broadcast>().picked.is_none()
        || e.kind != rush_draw::MouseKind::Down(rush_draw::Button::Left)
        || m.session.is_some()
        || m.on_place()
    {
        return Out::Pass;
    }
    let y = e.y as usize;
    let Some(key) = y
        .checked_sub(m.list_top)
        .and_then(|r| m.row_keys.get(r))
        .filter(|k| !k.is_empty())
        .cloned()
    else {
        return Out::Pass;
    };
    m.broadcast_pick(&key);
    Out::Draw
}

/// #broadcast.
pub(super) fn command(m: &mut Model, arg: &str) -> Out {
    m.broadcast_command(arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(key: &str, name: &str) -> Agent {
        Agent {
            key: key.into(),
            display_name: name.into(),
            ..Agent::default()
        }
    }

    #[test]
    fn picks() {
        let mut m = super::super::feed::tests::model(100, 30);
        let mut term = agent("d", "terminal one");
        term.interactive = true;
        let mut old = agent("c", "old work");
        old.past = true;
        let agents = vec![agent("a", "fix login bug"), agent("b", "docs"), old, term];
        m.set_fleet(crate::worker::Fleet {
            roots: vec![String::new(); agents.len()],
            agents,
            ..Default::default()
        });
        let picked = |m: &mut Model| {
            let mut v: Vec<String> = m
                .kept::<Broadcast>()
                .picked
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();
            v.sort();
            v
        };
        m.broadcast_command("all");
        assert_eq!(
            picked(&mut m),
            ["a", "b"],
            "all picks the agents a message can reach"
        );
        m.broadcast_command("@docs");
        assert_eq!(picked(&mut m), ["b"], "a tag picks that agent");
        m.broadcast_pick("a");
        m.broadcast_pick("d"); // can't be messaged from here
        m.broadcast_pick("b");
        assert_eq!(picked(&mut m), ["a"], "a click picks, a second unpicks");
        m.broadcast_command("@nobody hi");
        assert_eq!(picked(&mut m), ["a"], "an unknown tag changes nothing");
        m.broadcast_command("hello there");
        assert!(picked(&mut m).is_empty() && m.input.string() == "hello there");
        m.press("esc");
        assert!(m.kept::<Broadcast>().picked.is_none(), "esc stops");
    }

    #[test]
    fn nicknames_are_go_s() {
        // The same key always gets the same nickname, from Go's lists.
        let n = nickname("claude:default:abc");
        let (w, name) = n.split_once('-').unwrap();
        assert!(NICK_WORDS.split(' ').any(|x| x == w) && NICK_NAMES.split(' ').any(|x| x == name));
        assert_eq!(n, nickname("claude:default:abc"));
        let a = agent("k", "Fix the login bug");
        assert!(tags(&a, &mention_name(&a)) && tags(&a, "Fix-the-login-bug"));
    }
}
