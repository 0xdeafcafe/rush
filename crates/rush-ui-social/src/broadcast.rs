//! #broadcast sends one message to several agents. Named in the command
//! (#broadcast all … or #broadcast @a @b …) it goes at once; without a
//! message, rush waits while you click agents in the list to pick them,
//! then the box's next message goes to every one picked. Esc gives up.
//! Port of Go's broadcast.go, with the tags it reads agents by from
//! mention.go:30-71 (mentionName, tags, mentionable, mentionsIn) and the
//! sending of localqueue.go:61 and outside.go:42 (reply, replyOutside) for
//! agents rush doesn't run.

use std::collections::HashSet;

use rush_core::adapter::Profile;
use rush_core::agent::Agent;

use crate::frame::{Mouse, Out, Press};
use crate::model::Model;

/// Who's being picked; None while not broadcasting.
#[derive(Default)]
pub struct Broadcast {
    pub picked: Option<HashSet<String>>,
}

/// An agent's handle, how it's tagged: its title now, kebab-cased (see
/// crate::mention::mention_name).
pub fn mention_name(a: &Agent) -> String {
    crate::mention::mention_name(a, crate::mention::host_id(a))
}

/// Whether tag names agent a: its handle, its title, its kept id or the
/// start of its handle when it's the only one (tags).
pub fn tags(a: &Agent, tag: &str) -> bool {
    crate::mention::tags(a, crate::mention::host_id(a), tag)
}

/// Whether a message can reach it from here (mentionable).
pub fn mentionable(a: &Agent) -> bool {
    !a.interactive && !a.headless
}

/// The agents in list order.
fn in_order(m: &Model) -> impl Iterator<Item = &Agent> {
    m.order.iter().map(|&i| &m.agents[i])
}

/// Model's side of this module: Model is rush-ui-base's, so its
/// methods here are a trait's.
pub trait ModelBroadcast {
    fn mentions_in(&self, text: &str) -> Vec<usize>;
    fn broadcast_all(&self) -> Vec<usize>;
    fn broadcast_command(&mut self, arg: &str) -> Out;
    fn broadcast_pick(&mut self, key: &str);
    fn send_broadcast(&mut self, text: &str) -> Out;
    fn broadcast_to(&mut self, to: &[usize], text: &str) -> Out;
    fn reply_to(&mut self, a: &Agent, text: &str);
}

impl ModelBroadcast for Model {
    /// The agents the words after the first in text tag (mentionsIn).
    fn mentions_in(&self, text: &str) -> Vec<usize> {
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
    fn broadcast_all(&self) -> Vec<usize> {
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
    fn broadcast_command(&mut self, arg: &str) -> Out {
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
    fn broadcast_pick(&mut self, key: &str) {
        let Some(a) = in_order(self).find(|a| a.key == key) else {
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
    fn send_broadcast(&mut self, text: &str) -> Out {
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
        // Each agent it names is kept by its session's id.
        let text = &crate::mention::keep(text);
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
    fn reply_to(&mut self, a: &Agent, text: &str) {
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
                    Err(e) => m.fail(&format!("couldn't send to {name}: {e}")),
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
    m.input.clear_sent();
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
