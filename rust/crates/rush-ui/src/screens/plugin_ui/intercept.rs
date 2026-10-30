//! Plugins with "intercept" asked about a message before it goes from a
//! Session's box: they let it go, change it or hold it back, within the
//! broker's budget, off the UI thread, the box left as it is meanwhile.
//! After Go's internal/ui/pluginui.go (interceptedMsg :493,
//! interceptSend :501, onIntercepted :510, wantsIntercept :548), its
//! place in sendPane (rushmode.go:4627, :4637) and events.go:4
//! (clipLog).

use rush_plugin::{Intercept, InterceptResult, clean_notice};

use super::hooks::{agent_by_key, ui_session};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::list_extras::prompt::is_hash_cmd;
use crate::screens::session_core::{conn, keys};

/// Whether text in a box goes past plugins before it's sent: a message,
/// not a # or / command, nor a line to be carried on with a trailing
/// backslash (wantsIntercept's text half).
pub fn message(raw: &str, now: bool) -> bool {
    let t = raw.trim();
    !(t.is_empty() || is_hash_cmd(t) || t.starts_with('/') || raw.ends_with('\\') && !now)
}

/// Whether session key's box goes past plugins before it's sent: one is
/// asked, it wasn't just asked, and it's a message, not an edit of one
/// queued (wantsIntercept).
pub fn wants(m: &mut Model, key: &str, now: bool) -> bool {
    let Some(h) = m.hooks() else { return false };
    let c = conn(m, key);
    if c.intercepted || c.edit_q > 0 || !h.intercepts() {
        return false;
    }
    message(&c.input.string(), now)
}

/// Asks plugins about what's in session key's box, in the background;
/// the box stays as it is until they answer (interceptSend).
pub fn ask(m: &mut Model, key: &str, now: bool) -> Out {
    let Some(h) = m.hooks() else {
        return Out::Idle;
    };
    let session = agent_by_key(m, key).map(ui_session);
    let c = conn(m, key);
    let was = c.input.string();
    c.intercepting = true;
    let req = Intercept {
        hook: "before-send".into(),
        session,
        text: c.input.sent(false).trim().to_string(),
        ..Intercept::default()
    };
    let key = key.to_string();
    m.later(
        move || h.intercept(req),
        move |m, r| answered(m, &key, &was, now, r),
    );
    Out::Idle
}

/// Sends the message, changed or not, or says why it wasn't
/// (onIntercepted). was is the box as it was asked about.
pub fn answered(m: &mut Model, key: &str, was: &str, now: bool, r: InterceptResult) -> Out {
    if m.session.as_ref().is_none_or(|v| v.key != key) {
        return Out::Idle; // the Session was left meanwhile: the box went with it
    }
    let c = conn(m, key);
    c.intercepting = false;
    if r.action == "block" || r.action == "rewrite" {
        let mut f = serde_json::Map::new();
        f.insert("key".into(), key.into());
        f.insert("plugin".into(), r.plugin.clone().into());
        f.insert("action".into(), r.action.clone().into());
        f.insert("why".into(), clip_log(&r.reason).into());
        f.insert("was".into(), clip_log(was).into());
        if r.action == "rewrite" {
            f.insert("text".into(), clip_log(&r.text).into());
        }
        rush_events::log("message-intercepted", f);
    }
    if conn(m, key).input.string() != was {
        m.flash_err("the message changed while plugins looked at it: send it again");
        return Out::Draw;
    }
    match r.action.as_str() {
        "block" => {
            let why = if r.reason.is_empty() {
                "held back"
            } else {
                &r.reason
            };
            m.flash_err(&format!("{}: {}", r.plugin, clean_notice(why)));
            return Out::Draw;
        }
        "rewrite" => {
            let c = conn(m, key);
            c.input.set(&r.text);
            c.input.pastes = Default::default();
            if !r.plugin.is_empty() {
                m.flash(&format!("{} changed the message", r.plugin));
            }
        }
        _ => {}
    }
    conn(m, key).intercepted = true;
    let o = keys::send(m, key, now);
    conn(m, key).intercepted = false;
    match o {
        Out::Pass | Out::Idle => Out::Draw,
        o => o,
    }
}

/// Text for the log, cut at 300 characters (clipLog).
pub fn clip_log(text: &str) -> String {
    match text.char_indices().nth(300) {
        None => text.to_string(),
        Some((i, _)) => format!("{}…", &text[..i]),
    }
}
