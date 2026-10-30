//! The hosted sessions as the cards see them: what a session rush runs is
//! and says (session_cards::HostInfo) and what a card asks of it
//! (session_cards::Act), for the binary to link once
//! (session_cards::link). In Go the cards read the hostConn straight:
//! rushmode.go's hostConn (its key, kind, client, input, sel, subs,
//! sess.Info and sess.Usage), caps.go:29 sessionAgent, stopnote.go:23
//! (enter: stopPicked, then sendPane, as one tea.Sequence), and the
//! client's ops for an answer (client.go Allow :533, Deny :537,
//! SetPermissionMode :560, DismissDecisions, SetWithout, Ask);
//! ui/settings.go (the config fields by JSON name the cards read and
//! save). asksheets.go:21 askClaude (a control request, its answer
//! handed back by id), keys.go:726 replyTo (a message to another agent,
//! however it takes them), clipimage.go:114 attachImages (images into the
//! box as [Image #N]), entersheet.go:38 key (what enter does, done now),
//! slimsheet.go:126-138 (#slim's SetWithout) and the session's rewinds,
//! compactions, forks and hand-offs (carry.rs).
//!
//! Renaming from enter's first answer is the list's startRename
//! (list_extras::rename), a room set up from a chat the room screens'
//! sheet (social::roomsetup, room.go roomSetup).

use rush_core::agent::State;
use rush_fleet::Predicates;

use super::host::Op;
use super::{agent, conn, jobkeys};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::session_cards::{self as cards, Act, HostInfo, Reply, Sub};

/// The link the binary hands the cards.
pub fn host() -> cards::Host {
    cards::Host {
        info,
        act,
        set_input,
        ask,
        setting,
        save_setting,
    }
}

/// What the cards know of session key: cold when rush doesn't run it now
/// (Go's hostConn with a nil client: an idle or past session, open or
/// beside the list).
fn info(m: &Model, key: &str) -> Option<HostInfo> {
    let a = agent(m, key);
    let empty;
    let c = match m.kept.peek::<super::Core>().and_then(|k| k.conns.get(key)) {
        Some(c) => c,
        None if a.is_some() => {
            empty = super::Conn::default();
            &empty
        }
        None => return None,
    };
    let v = m.session.as_ref().filter(|v| v.key == key);
    // The harness its host says it runs in, else the list's (sessionAgent).
    let kind = v
        .map(|v| v.sess.info.kind.clone())
        .filter(|k| !k.is_empty())
        .or_else(|| a.map(|a| a.kind.as_str().to_string()))
        .unwrap_or_default();
    let state = if !c.info.state.is_empty() {
        c.info.state.clone()
    } else {
        match a.map(|a| a.job.state) {
            Some(State::Working) => "working".into(),
            Some(State::Blocked) => "blocked".into(),
            _ => "idle".into(),
        }
    };
    Some(HostInfo {
        cold: !c.hosted(),
        kind,
        name: a.map(|a| a.display_name.clone()).unwrap_or_default(),
        acct: a.map(|a| a.acct.clone()).unwrap_or_default(),
        permission_mode: c.info.permission_mode.clone(),
        asleep: c.sleeping || c.info.sleeping,
        input: c.input.string(),
        cwd: a.map(|a| a.job.cwd.clone()).unwrap_or_default(),
        id: a.map(|a| a.job.id.clone()).unwrap_or_default(),
        proto: c.info.proto as i32,
        state,
        model: c.info.model.clone(),
        session_id: a.map(|a| a.job.session_id.clone()).unwrap_or_default(),
        path: a.map(|a| a.job.transcript_path.clone()).unwrap_or_default(),
        stops: jobkeys::stops(m, key),
        branches: c.branches.clone(),
        permission_modes: c.info.now.permission_modes.clone(),
        subs: c
            .runs
            .subs
            .iter()
            .map(|sa| Sub {
                tool_use_id: sa.tool_use_id.clone(),
                r#type: sa.typ.clone(),
                description: sa.description.clone(),
            })
            .collect(),
        decisions: c.info.now.decisions.clone(),
        effort: if c.info.now.effort.is_empty() {
            c.info.began.effort.clone()
        } else {
            c.info.now.effort.clone()
        },
        cache_warm: c.info.cache_warm,
        without: c.info.now.without.clone(),
        unused: c.unused.clone(),
        limit: c.info.limit.clone(),
        updated: (c.info.now.updated_ns / 1_000_000) as rush_core::time::Ms,
        conv_tokens: c.context.as_ref().map(|u| {
            let msg = &u.messages;
            let all =
                msg.tool_calls + msg.tool_results + msg.assistant + msg.user + msg.attachments;
            (all, msg.tool_results)
        }),
    })
}

/// Sends op to session key's host off the UI thread; a failure says why.
fn send_op(m: &mut Model, key: &str, op: Op) -> Out {
    let Some(link) = conn(m, key).link.clone() else {
        m.flash_err("this session can't take answers here");
        return Out::Draw;
    };
    m.later(
        move || link.op(op),
        |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                m.fail(&e);
                Out::Draw
            }
        },
    );
    Out::Draw
}

/// Does what a card asks on session key.
fn act(m: &mut Model, key: &str, a: Act) -> Out {
    match a {
        Act::Allow {
            id,
            input: None,
            always,
        } => send_op(m, key, Op::Allow { id, always }),
        Act::Allow {
            id,
            input: Some(input),
            always,
        } => send_op(m, key, Op::Answer { id, input, always }),
        Act::Deny { id, why } => send_op(m, key, Op::Deny { id, message: why }),
        Act::Permission(mode) => send_op(m, key, Op::Permission(mode)),
        Act::Send(text) => send_op(
            m,
            key,
            Op::Send {
                text,
                images: Vec::new(),
                now: false,
            },
        ),
        Act::Stop { key: k, sel, send } => {
            // What was picked when the stop was asked for is what stops
            // (stopnote.go:23-31), and what's in the box goes straight
            // after, in that order, as one (tea.Sequence).
            if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
                v.sel = sel;
            }
            let send = send && !conn(m, key).input.string().trim().is_empty();
            super::in_order(m, |m| {
                let out = jobkeys::stop_picked(m, key, &k);
                if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
                    v.sel.clear();
                }
                if send {
                    return super::keys::send(m, key, true);
                }
                if out == Out::Pass { Out::Draw } else { out }
            })
        }
        Act::Paste(text) => {
            // One paste, its chip and a space after it (quickask.go:249).
            let input = &mut conn(m, key).input;
            let chip = input.pastes.add(&text);
            input.insert_str(&format!("{chip} "));
            Out::Draw
        }
        Act::DismissDecisions(id) => send_op(m, key, Op::DismissDecisions(id)),
        Act::ContinueAtReset(yes) => send_op(m, key, Op::ContinueAtReset(yes)),
        Act::Without(w) => send_op(m, key, Op::Without(w.unwrap_or_default())),
        Act::Rewind(rw) => super::carry::rewind(m, key, *rw),
        Act::Compact { by, draft } => super::carry::compact(m, key, by, draft),
        Act::Fork(f) => super::carry::fork(m, key, *f),
        Act::Handoff { to, name } => super::carry::handoff(m, key, &to, &name),
        Act::ReplyTo { to, text, tagged } => reply_to(m, &to, text, &tagged),
        Act::AttachImages(imgs) => {
            attach_images(m, key, &imgs);
            Out::Draw
        }
        Act::EnterOn(open) => enter_on(m, key, open),
        Act::Room(ask) => crate::screens::social::roomsetup::chat_room(m, *ask),
    }
}

/// Sends text to agent to however it takes messages: its host, a resume
/// into rush mode, its queue while it's busy, or its own daemon; tagged
/// is the text with its pastes marked, for rush sessions (replyTo).
pub(crate) fn reply_to(m: &mut Model, to: &str, text: String, tagged: &str) -> Out {
    let Some(a) = agent(m, to).cloned() else {
        return Out::Idle;
    };
    seen(m, &a);
    m.flash(&format!("sending to {}…", a.display_name));
    if a.rush {
        // sendHosted: queued while it works.
        let h = super::host::hosts(m);
        let (id, name, text) = (a.job.id.clone(), a.display_name.clone(), tagged.to_string());
        super::later(
            m,
            move || h.send_to(&id, &text, false),
            move |m, r| {
                match r {
                    Ok(()) => m.flash(&format!("sent to {name}")),
                    Err(e) => m.fail(&e),
                }
                Out::Draw
            },
        );
        return Out::Draw;
    }
    // The Agents box's images as their files' paths, [image: /a.png],
    // which Claude Code opens itself (keys.go:735-736).
    let text = m.imgs.paths(&text);
    m.imgs = Default::default();
    if a.past {
        super::keys::adopt(m, &a, &text);
        return Out::Draw;
    }
    let queued = super::core(m)
        .local_q
        .get(to)
        .is_some_and(|q| !q.items.is_empty());
    if super::busy(&a) || queued {
        let now = m.now;
        super::queue::queue_local(m, to, &text, now);
        m.flash(&format!("queued for {} · goes within 15s", a.display_name));
        return Out::Draw;
    }
    super::keys::reply(m, &a, text);
    Out::Draw
}

/// a's waiting on you is seen, and it's out of Earlier, now you've
/// written to it (markSeen, undone): kept in rush's state off the UI
/// thread.
fn seen(m: &mut Model, a: &rush_core::agent::Agent) {
    let waiting = a.job.state == State::Blocked || a.halted() || a.your_turn(m.now);
    let key = a.key.clone();
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            let mut changed = s.overlay.done.remove(&key).is_some();
            if waiting {
                s.overlay.seen.insert(key, rush_state::Time::now());
                changed = true;
            }
            if changed {
                s.save_overlay().map_err(|e| e.to_string())
            } else {
                Ok(())
            }
        },
        |m, r: Result<(), String>| {
            if let Err(e) = r {
                m.fail(&e);
            }
            Out::Idle
        },
    );
}

/// Image files into session key's box, each as [Image #N] at the cursor,
/// spaced off the words either side (attachImages, withMarks).
fn attach_images(m: &mut Model, key: &str, imgs: &[String]) {
    let c = conn(m, key);
    let (text, pos) = (c.input.text.clone(), c.input.pos.min(c.input.text.len()));
    c.input.undo.save(&text, pos, false);
    let marked =
        crate::screens::session_cards::imagerefs::with_marks(&text, pos, &mut c.imgs, imgs);
    c.input.pos = pos + (marked.len() - text.len());
    c.input.text = marked;
    c.input.anchor = None;
}

/// What enter on an agent does from now on, done now on agent key: open
/// it, or rename it (enterSheet's enter).
fn enter_on(m: &mut Model, key: &str, open: bool) -> Out {
    if agent(m, key).is_none() {
        return Out::Idle;
    }
    if !open {
        crate::screens::list_extras::rename::start_rename(m, key);
        return Out::Draw;
    }
    // focusPane: its conversation, opened from its row.
    m.sel = key.to_string();
    if let Some(i) = m.agents.iter().position(|a| a.key == key) {
        m.open(i);
    }
    Out::Draw
}

/// Puts text in session key's box.
fn set_input(m: &mut Model, key: &str, text: &str) {
    conn(m, key).input.set(text);
}

/// Passes a control request to session key's agent through its host;
/// done has the answer when it comes. False, having said why, when the
/// session can't be asked (askClaude).
fn ask(m: &mut Model, key: &str, req: serde_json::Value, done: Reply) -> bool {
    let c = conn(m, key);
    if c.link.is_none() {
        m.flash_err("that works in rush-mode sessions · /rush moves this one over");
        return false;
    }
    if c.info.proto < 2 {
        m.flash_err(
            "this agent's host is older than that: it can once the host restarts (it does after resting idle)",
        );
        return false;
    }
    c.ask_n += 1;
    let id = format!("ui-{}", c.ask_n);
    c.asks.insert(id.clone(), done);
    let req = req.to_string();
    send_op(m, key, Op::Ask { id, req });
    true
}

/// rush's config by its JSON name, as a word: a switch is "true" or
/// "false". The config is read from disk while Settings hasn't yet.
fn setting(m: &Model, name: &str) -> String {
    let cfg = crate::screens::settings::loaded_config(m);
    let v = match cfg {
        Some(c) => serde_json::to_value(c),
        None => serde_json::to_value(rush_state::Store::load().config),
    };
    match v.ok().as_ref().and_then(|v| v.get(name)) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    }
}

/// Saves value as rush's config field name: a switch from "true" or
/// "false", a number from its digits, anything else as words.
fn save_setting(m: &mut Model, name: &str, value: &str) -> Result<(), String> {
    use crate::screens::settings::{changed, config_mut};
    let mut v = serde_json::to_value(&*config_mut(m)).map_err(|e| e.to_string())?;
    let Some(obj) = v.as_object_mut() else {
        return Err("rush's config can't be saved here".into());
    };
    let new = match obj.get(name) {
        Some(serde_json::Value::Bool(_)) => serde_json::Value::Bool(value == "true"),
        Some(serde_json::Value::Number(_)) => value
            .parse::<i64>()
            .map(Into::into)
            .map_err(|_| format!("{name} takes a number"))?,
        _ => serde_json::Value::String(value.into()),
    };
    obj.insert(name.into(), new);
    *config_mut(m) = serde_json::from_value(v).map_err(|e| e.to_string())?;
    changed(m);
    Ok(())
}
