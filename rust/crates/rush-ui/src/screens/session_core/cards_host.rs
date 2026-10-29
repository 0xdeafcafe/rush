//! The hosted sessions as the cards see them: what a session rush runs is
//! and says (session_cards::HostInfo) and what a card asks of it
//! (session_cards::Act), for the binary to link once
//! (session_cards::link). In Go the cards read the hostConn straight:
//! rushmode.go's hostConn (its key, kind, client, input, sel), caps.go:29
//! sessionAgent, stopnote.go:23 (enter: stopPicked, then sendPane), and
//! the client's ops for an answer (client.go Allow :533, Deny :537,
//! SetPermissionMode :560); ui/settings.go (the config fields by JSON
//! name the cards read and save).
//!
//! Not here yet (each says so when asked): rewinding, forking, compacting,
//! handing off and #slim's next start on the host; a room set up from a
//! chat; a reply to another agent; images attached from a card; setting
//! a decisions card aside; and asking the harness a control request
//! (askClaude). These are the hosted view's.

use rush_core::agent::State;

use super::host::Op;
use super::{agent, conn, jobkeys};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::session_cards::{self as cards, Act, HostInfo, Reply};

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

/// What the cards know of session key, when rush runs it.
fn info(m: &Model, key: &str) -> Option<HostInfo> {
    let c = m.kept.peek::<super::Core>()?.conns.get(key)?;
    if !c.hosted() {
        return None;
    }
    let a = agent(m, key);
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
        ..HostInfo::default()
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
            // (stopnote.go:23-31).
            if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
                v.sel = sel;
            }
            let out = jobkeys::stop_picked(m, key, &k);
            if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
                v.sel.clear();
            }
            if send {
                return super::keys::send(m, key, true);
            }
            if out == Out::Pass { Out::Draw } else { out }
        }
        Act::Paste(text) => {
            conn(m, key).input.insert_str(&text);
            Out::Draw
        }
        _ => {
            m.flash_err("rush can't do that on this session here yet");
            Out::Draw
        }
    }
}

/// Puts text in session key's box.
fn set_input(m: &mut Model, key: &str, text: &str) {
    conn(m, key).input.set(text);
}

/// Asking the harness a control request isn't here yet.
fn ask(m: &mut Model, _key: &str, _req: serde_json::Value, _done: Reply) -> bool {
    m.flash_err("rush can't ask this session's harness here yet");
    false
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
