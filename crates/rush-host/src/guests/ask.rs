//! Asking a guest: you with `@kimi …` in the box (route), an agent with
//! rush's ask_guest tool (ask). Each name is found among the guests by
//! its handle, else among the agents start_agent starts, named as
//! spawn_agent names them (agentmatch), and started the first time as a
//! session of its own beside the lead's conversation; the ask goes in the
//! chat, the guest is told what's new there since its last turn, and the
//! ask is a card in the lead's conversation. Called off the UI's thread,
//! in a host, or in `rush mcp-tools`. Rust-only.

use rush_core::adapter::{Registry, migrated};
use rush_core::event::{Exchange, Peer};
use rush_room::room::{Entry, INFO, SAY, USER};
use rush_state::Time;
use serde_json::json;

use super::prompt::{Brief, brief, turn};
use super::roster::{self, Guest, Roster};
use super::{GUEST_NAME, GUEST_OF, HOP_CAP, PHASE, chat, lead_name, mention, titled};
use crate::agentmatch::{clean_name, matches};
use crate::agents::{Pick, session_picks};
use crate::client::{Client, deliver_how, read_config, read_info, spawn};
use crate::events::{clip, log_event};
use crate::subtool::{FOREGROUND, SubAgents, await_answer, peer, report, stuck_behind};

/// Who asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asker {
    /// You, from the conversation's box.
    User,
    /// An agent of the chat, by its rush session id.
    Agent(String),
}

/// One ask.
#[derive(Clone, Debug)]
pub struct Ask {
    /// The conversation: its lead's session id.
    pub lead: String,
    /// Whom, by name or tag, as typed.
    pub to: Vec<String>,
    pub text: String,
    pub asker: Asker,
    /// The end of the conversation as plain text, for a guest new to it;
    /// "" when not known.
    pub before: String,
    /// Wait for the answer (an agent's ask only).
    pub wait: bool,
}

/// Who a name in a conversation is.
#[derive(Clone, Debug)]
enum Target {
    Lead,
    Guest(Guest),
    New {
        /// What found it, for choose.
        query: String,
        name: String,
        handle: String,
    },
}

/// What's said when agents have asked each other too often since you
/// last spoke.
pub fn hops_note() -> String {
    format!(
        "{HOP_CAP} asks between agents since the user last spoke: rush holds the rest until the user says something."
    )
}

/// Sends text typed in lead's conversation to the guests it starts by
/// tagging; None when it tags none (it goes to the lead as ever). before
/// is the conversation's end, for a guest new to it.
pub fn route(
    reg: &Registry,
    lead: &str,
    text: &str,
    before: &str,
) -> Result<Option<String>, String> {
    let Some((tags, rest)) = mention::leading(text) else {
        return Ok(None);
    };
    let r = roster_for(reg, lead);
    let ps = picks_for(reg, lead);
    let (targets, rest) = resolve_all(reg, &r, &ps, &tags, &rest);
    // The lead tagged among guests hears it with them, at its next turn.
    let targets: Vec<Target> = targets
        .into_iter()
        .filter(|t| !matches!(t, Target::Lead))
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }
    if rest.trim().is_empty() {
        return Err("type the message after the name".into());
    }
    let a = Ask {
        lead: lead.into(),
        to: Vec::new(),
        text: rest,
        asker: Asker::User,
        before: before.into(),
        wait: false,
    };
    ask_targets(reg, &a, targets).map(Some)
}

/// Asks a.to, as a.asker: the answer once given when a.wait, else what
/// became of the ask.
pub fn ask(reg: &Registry, a: Ask) -> Result<String, String> {
    if a.text.trim().is_empty() {
        return Err("ask_guest needs a prompt: what to ask".into());
    }
    let r = roster_for(reg, &a.lead);
    let ps = picks_for(reg, &a.lead);
    let mut targets = Vec::new();
    for name in &a.to {
        let (w, rest) = mention::first_word(name.trim_start_matches('@'));
        match resolve(reg, &r, &ps, w, rest) {
            Some((t, _)) => targets.push(t),
            None => {
                return Err(crate::agentmatch::no_agent(reg, name, "", &ps));
            }
        }
    }
    if targets.is_empty() {
        return Err("ask_guest needs a name: whom to ask".into());
    }
    ask_targets(reg, &a, targets)
}

/// Lead's guests, the lead named even before it has any: its agent's
/// name is its own, never a new guest's.
fn roster_for(reg: &Registry, lead: &str) -> Roster {
    let mut r = roster::load(lead);
    if r.lead.is_empty() {
        let kind = read_config(lead).map(|c| c.kind).unwrap_or_default();
        r.lead = lead_name(reg, &kind);
    }
    r
}

/// The agents a conversation can have as guests: start_agent's.
fn picks_for(reg: &Registry, lead: &str) -> Vec<Pick> {
    let kind = read_config(lead)
        .map(|c| migrated(&c.kind).0)
        .unwrap_or_default();
    session_picks(reg, &reg.harness_of(&kind).0)
}

/// The targets tags name, from the first, as many as name someone; the
/// rest of the text after them (tags that name no one stay in it).
fn resolve_all(
    reg: &Registry,
    r: &Roster,
    ps: &[Pick],
    tags: &[String],
    rest: &str,
) -> (Vec<Target>, String) {
    let mut out = Vec::new();
    for (i, tag) in tags.iter().enumerate() {
        // Only the last tag's name may run on into the words after it.
        let next = if i + 1 == tags.len() { rest } else { "" };
        match resolve(reg, r, ps, tag, next) {
            Some((t, took)) => {
                out.push(t);
                if took {
                    return (out, mention::first_word(rest).1.to_string());
                }
            }
            None => {
                let left: Vec<String> = tags[i..].iter().map(|t| format!("@{t}")).collect();
                return (out, format!("{} {rest}", left.join(" ")).trim().to_string());
            }
        }
    }
    (out, rest.to_string())
}

/// Who tag names in r's conversation, and whether it took the first word
/// of next with it ("@claude haiku": Haiku, even where Claude leads).
fn resolve(
    reg: &Registry,
    r: &Roster,
    ps: &[Pick],
    tag: &str,
    next: &str,
) -> Option<(Target, bool)> {
    let h = clean_name(tag);
    if h.is_empty() {
        return None;
    }
    if let Some(g) = r.find(&h) {
        return Some((Target::Guest(g.clone()), false));
    }
    let (w, _) = mention::first_word(next);
    let two = (!w.is_empty()).then(|| format!("{tag} {w}")).and_then(|q| {
        let p = matches(reg, ps, &q)?;
        // Taken only when the word narrows it: it's in the name or model.
        let cw = clean_name(w);
        let named = !cw.is_empty()
            && (clean_name(&p.name).contains(&cw) || clean_name(&p.model).contains(&cw));
        named.then_some((p, q))
    });
    let (pick, query, took) = match two {
        Some((p, q)) => (p, q, true),
        None if h == "lead" || h == clean_name(&r.lead) => return Some((Target::Lead, false)),
        None => (matches(reg, ps, tag)?, tag.to_string(), false),
    };
    let handle = clean_name(&pick.name);
    if let Some(g) = r.find(&handle) {
        return Some((Target::Guest(g.clone()), took));
    }
    if handle == clean_name(&r.lead) {
        return Some((Target::Lead, took));
    }
    let name = titled(&handle);
    Some((
        Target::New {
            query,
            name,
            handle,
        },
        took,
    ))
}

/// The asker as the chat names it, and as one end of an exchange.
fn asker_of(r: &Roster, lead: &str, a: &Asker) -> (String, Peer) {
    match a {
        Asker::User => (
            USER.to_string(),
            Peer {
                name: "you".into(),
                ..Peer::default()
            },
        ),
        Asker::Agent(id) => {
            let name = if id == lead {
                r.lead.clone()
            } else {
                r.by_id(id).map(|g| g.name.clone()).unwrap_or_else(|| {
                    read_config(id)
                        .ok()
                        .and_then(|c| c.meta.get(GUEST_NAME).cloned())
                        .unwrap_or_else(|| "an agent".into())
                })
            };
            let mut p = peer(id);
            p.name = name.clone();
            (name, p)
        }
    }
}

/// A guest as one end of an exchange.
fn guest_peer(g: &Guest) -> Peer {
    Peer {
        session_id: g.id.clone(),
        kind: g.kind.clone(),
        name: g.name.clone(),
    }
}

fn hex_id() -> String {
    crate::paths::rand_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The exchange from one end to the other, in phase.
fn exchange(from: &Peer, to: &Peer, phase: &str, text: &str) -> Exchange {
    Exchange {
        id: hex_id(),
        at: rush_core::time::now(),
        direction: "received".into(),
        phase: phase.into(),
        text: text.into(),
        sender: from.clone(),
        receiver: to.clone(),
        ..Exchange::default()
    }
}

/// Draws e in lead's conversation: through its host when it runs, else
/// into its journal, which the conversation is read back with.
pub fn show_in_lead(lead: &str, mut e: Exchange) {
    e.direction = "sent".into(); // a row of the turn it lands in, never a turn of its own
    if e.at == 0 {
        e.at = rush_core::time::now();
    }
    let live = Client::dial(lead).and_then(|c| {
        crate::client::await_line(&c, std::time::Duration::from_secs(5), |l| {
            matches!(l, crate::lines::Line::Info(_))
        })?;
        c.record_exchange(e.clone())
    });
    if live.is_err() {
        let _ = crate::exchange_journal::append_exchange(lead, &e);
    }
}

fn ask_targets(reg: &Registry, a: &Ask, targets: Vec<Target>) -> Result<String, String> {
    let lead = a.lead.as_str();
    let lead_cfg = read_config(lead).map_err(|e| format!("no conversation {lead}: {e}"))?;
    // The chat's names, and the asks between agents since you spoke.
    let user = a.asker == Asker::User;
    let lname = lead_name(reg, &lead_cfg.kind);
    let held = roster::update(lead, |r| {
        if r.lead.is_empty() {
            r.lead = lname.clone();
        }
        if user {
            r.hops = 0;
            return false;
        }
        if r.hops >= HOP_CAP {
            return true;
        }
        r.hops += 1;
        false
    })
    .map_err(|e| e.to_string())?;
    let r = roster::load(lead);
    let (from, from_peer) = asker_of(&r, lead, &a.asker);
    if held {
        let note = hops_note();
        let _ = chat::append(
            lead,
            Entry {
                from: "rush".into(),
                kind: INFO.into(),
                text: note.clone(),
                ..Entry::default()
            },
        );
        show_in_lead(
            lead,
            exchange(
                &Peer {
                    kind: "rush".into(),
                    name: "rush".into(),
                    ..Peer::default()
                },
                &from_peer,
                PHASE,
                &note,
            ),
        );
        return Err(note);
    }
    let names: Vec<String> = targets
        .iter()
        .map(|t| match t {
            Target::Lead => r.lead.clone(),
            Target::Guest(g) => g.name.clone(),
            Target::New { name, .. } => name.clone(),
        })
        .collect();
    chat::append(
        lead,
        Entry {
            from: from.clone(),
            kind: SAY.into(),
            text: a.text.clone(),
            to: names.join(", "),
            ..Entry::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let mut said = Vec::new();
    for t in targets {
        said.push(ask_one(reg, a, &lead_cfg, &from, &from_peer, t)?);
    }
    Ok(said.join("\n\n"))
}

/// Asks one target, the ask already in the chat.
fn ask_one(
    reg: &Registry,
    a: &Ask,
    lead_cfg: &crate::wire::Config,
    from: &str,
    from_peer: &Peer,
    t: Target,
) -> Result<String, String> {
    let lead = a.lead.as_str();
    let r = roster::load(lead);
    let (g, new) = match t {
        Target::Lead => {
            // An agent asking the lead: a message in its queue, which it
            // answers in the conversation.
            let text = format!("{from} asks you (ask_guest): {}", a.text);
            let e = exchange(from_peer, &peer(lead), "message", &text);
            deliver_how(lead, &e, false).map_err(|e| e.to_string())?;
            return Ok(format!(
                "Sent to {}; it answers in the conversation.",
                r.lead
            ));
        }
        Target::Guest(g) => (g, None),
        Target::New {
            query,
            name,
            handle,
        } => {
            let (g, cfg) = prepare(reg, a, lead_cfg, &r, &query, name, handle)?;
            (g, Some(cfg))
        }
    };
    if matches!(&a.asker, Asker::Agent(id) if *id == g.id) {
        return Err(format!("{} is you: ask someone else", g.name));
    }
    let first = new.is_some();
    let r = roster::load(lead);
    let es = chat::read(lead);
    let budget = rush_room::seen::budget(r.guests.iter().map(|g| g.context));
    let since = rush_room::seen::since(chat::after(&es, g.seen), &g.name, budget);
    let others: Vec<String> = r.guests.iter().map(|x| x.name.clone()).collect();
    let cwd = read_info(lead)
        .ok()
        .map(|i| i.cwd)
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| lead_cfg.cwd.clone());
    let b = first.then(|| {
        brief(&Brief {
            me: &g.name,
            lead: &r.lead,
            others: &others,
            cwd: &cwd,
            before: &a.before,
        })
    });
    let prompt = turn(b.as_deref(), &since, first, from);
    let seen = es.last().map_or(Time::now(), |e| e.at);
    roster::update(lead, |r| {
        if let Some(x) = r.guests.iter_mut().find(|x| x.id == g.id) {
            x.seen = seen;
        }
    })
    .map_err(|e| e.to_string())?;
    // The ask in the conversation, unless the lead's own tool call already
    // shows it there.
    let by_lead = matches!(&a.asker, Asker::Agent(id) if id == lead);
    if !by_lead {
        show_in_lead(lead, exchange(from_peer, &guest_peer(&g), PHASE, &a.text));
    }
    let since_t = Time::now();
    let e = exchange(from_peer, &guest_peer(&g), "message", &prompt);
    let queued = match new {
        Some(mut cfg) => {
            // Started on its first ask, as spawn_agent starts a child.
            cfg.prompt = prompt;
            cfg.prompt_exchange = Some(Box::new(e));
            if let Err(err) = spawn(cfg) {
                let _ = roster::update(lead, |x| x.guests.retain(|o| o.id != g.id));
                return Err(format!("{} couldn't start: {err}", g.name));
            }
            false
        }
        None => deliver_how(&g.id, &e, false)
            .map_err(|e| format!("{} couldn't be asked: {e}", g.name))?,
    };
    log_event(
        &g.id,
        "guest-asked",
        json!({"lead": lead, "by": from, "first": first, "queued": queued, "text": clip(&a.text)}),
    );
    match &a.asker {
        Asker::User => Ok(format!("asked {}", g.name)),
        Asker::Agent(id) if a.wait => {
            let _ = report(&g.id, "");
            await_answer(reg, &g.id, id, since_t, FOREGROUND)
        }
        Asker::Agent(id) => {
            report(&g.id, id).map_err(|e| e.to_string())?;
            let note = if queued {
                stuck_behind(reg, &g.id)
            } else {
                String::new()
            };
            Ok(format!(
                "Asked {}. Its answer is sent to you as a message, and shows in the conversation.{}",
                g.name,
                if note.is_empty() {
                    String::new()
                } else {
                    format!(" {note}")
                }
            ))
        }
    }
}

/// A new guest of lead's, the one query names: its config (a session of
/// its own in the lead's folder, with its trust), ready to start, and its
/// place in the roster.
fn prepare(
    reg: &Registry,
    a: &Ask,
    lead_cfg: &crate::wire::Config,
    r: &Roster,
    query: &str,
    name: String,
    handle: String,
) -> Result<(Guest, crate::wire::Config), String> {
    let lead = a.lead.as_str();
    let sa = SubAgents::of(reg.clone(), lead).ok_or_else(|| format!("no conversation {lead}"))?;
    let ps = picks_for(reg, lead);
    let (p, model) = sa.choose(&ps, query, "")?;
    let mut cfg = sa.config(&p, &model, &a.text, "", false);
    cfg.system_prompt.clear(); // a participant, not a worker
    cfg.name = format!("{name} · guest");
    cfg.name_first = false;
    cfg.meta.insert(GUEST_OF.into(), lead.into());
    cfg.meta.insert(GUEST_NAME.into(), name.clone());
    sa.use_agent(&mut cfg, &p.r.k)?;
    cfg.fill_ids();
    cfg.cause = rush_events::id();
    let context =
        p.r.models
            .iter()
            .find(|c| !cfg.model.is_empty() && c.id == cfg.model)
            .map_or(0, |c| c.context);
    let g = Guest {
        name: name.clone(),
        handle,
        id: cfg.id.clone(),
        kind: migrated(&cfg.kind).0,
        model: cfg.model.clone(),
        context,
        seen: Time::default(),
    };
    roster::update(lead, |x| {
        if x.lead.is_empty() {
            x.lead = r.lead.clone();
        }
        x.guests.push(g.clone());
    })
    .map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(crate::paths::dir(&cfg.id));
    // Its mark for the lease's hook (hook.rs).
    let _ = crate::json::write_private(&super::hook::guest_of_path(&cfg.id), lead.as_bytes());
    log_event(
        &cfg.id,
        "spawned",
        json!({"ev": cfg.cause, "by": "guest", "spawnedBy": lead, "guestOf": lead, "agent": cfg.kind, "model": cfg.model, "name": name, "lead": lead_cfg.kind}),
    );
    Ok((g, cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lead_and_guests_by_name() {
        let reg = rush_core::adapter::Registry::default();
        let r = Roster {
            lead: "Claude".into(),
            guests: vec![Guest {
                name: "Kimi".into(),
                handle: "kimi".into(),
                id: "g1".into(),
                ..Guest::default()
            }],
            ..Roster::default()
        };
        let who = |tag: &str, next: &str| match resolve(&reg, &r, &[], tag, next) {
            Some((Target::Lead, took)) => format!("lead {took}"),
            Some((Target::Guest(g), took)) => format!("{} {took}", g.id),
            Some((Target::New { name, .. }, took)) => format!("new {name} {took}"),
            None => "none".into(),
        };
        for (tag, next, want) in [
            ("claude", "fix it", "lead false"),
            ("lead", "", "lead false"),
            ("KIMI", "look", "g1 false"),
            ("sol", "look", "none"),
            ("", "", "none"),
        ] {
            assert_eq!(who(tag, next), want, "{tag}");
        }
    }

    #[test]
    fn tags_that_name_no_one_stay_in_the_text() {
        let reg = rush_core::adapter::Registry::default();
        let r = Roster {
            lead: "Claude".into(),
            guests: vec![Guest {
                name: "Kimi".into(),
                handle: "kimi".into(),
                id: "g1".into(),
                ..Guest::default()
            }],
            ..Roster::default()
        };
        let tags = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (ts, rest) = resolve_all(&reg, &r, &[], &tags(&["kimi", "nobody"]), "look at this");
        assert_eq!(ts.len(), 1);
        assert_eq!(rest, "@nobody look at this");
        let (ts, rest) = resolve_all(&reg, &r, &[], &tags(&["kimi"]), "look");
        assert_eq!((ts.len(), rest.as_str()), (1, "look"));
    }

    #[test]
    fn a_plain_message_isnt_routed() {
        let reg = rush_core::adapter::Registry::default();
        assert_eq!(route(&reg, "guests-ask-none", "just do it", ""), Ok(None));
        assert_eq!(
            route(&reg, "guests-ask-none", "@src/main.rs is broken", ""),
            Ok(None)
        );
        // A tag that names no agent goes to the lead as typed.
        assert_eq!(route(&reg, "guests-ask-none", "@nobody hi", ""), Ok(None));
    }
}
