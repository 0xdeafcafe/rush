//! What /model and /effort with nothing named list for the open
//! conversation: the agent's own default first, then its list, the one
//! running marked (the frame's ChoiceSheet draws it). After Go's
//! internal/ui/choicesheet.go (argOptions :17, argRunning :33, argNow
//! :44, openChoices :136, body's about :173, key :214).
//!
//! The open conversation here is read from its transcript (a native
//! session, Go's hostConn with no client): enter says to bring it into
//! rush first, as Go's does, and r would.

use rush_core::adapter::{Choice, Registry};

use super::setup::models;
use crate::frame::Out;
use crate::frame::choice::{Choice as Row, ChoiceSheet};
use crate::model::Model;

/// The values /name takes for agent kind, as its adapter lists them;
/// None when it has no list for name (argOptions).
pub fn arg_options(reg: &Registry, kind: &str, name: &str) -> Option<Vec<Choice>> {
    let ch = reg.choices_of(kind)?;
    match name {
        "model" => Some(ch.models),
        "effort" => Some(ch.efforts),
        _ => None,
    }
}

/// Which of opts the session runs with, "" for the agent's own default:
/// the one last picked, else the one running is, else the one whose
/// words it has the most of ("opus[1m]" in a full model id with opus and
/// 1m in it). The bool is false when it's none of them (argNow).
pub fn arg_now(picked: Option<&str>, running: &str, opts: &[Choice]) -> (String, bool) {
    if let Some(p) = picked {
        return (p.to_string(), true);
    }
    if running.is_empty() {
        return (String::new(), false);
    }
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    let has = words(running);
    let (mut now, mut best) = (String::new(), 0);
    for o in opts {
        if o.id == running {
            return (o.id.clone(), true);
        }
        let ws = words(&o.id);
        if !ws.is_empty() && ws.iter().all(|w| has.contains(w)) && ws.len() > best {
            now = o.id.clone();
            best = ws.len();
        }
    }
    (now, best > 0)
}

/// The rows of /name's list and the one running: the agent's own default
/// first, then opts, then the running one when the list hasn't it
/// (openChoices).
pub fn choice_rows(
    opts: &[Choice],
    running: &str,
    picked: Option<&str>,
) -> (Vec<Row>, Option<usize>) {
    let mut rows = vec![Row {
        id: String::new(),
        note: "the agent's own".into(),
    }];
    rows.extend(opts.iter().map(|o| Row {
        id: o.id.clone(),
        note: o.note.clone(),
    }));
    let (mut now, mut known) = arg_now(picked, running, opts);
    if !known && !running.is_empty() {
        // One the list doesn't have still shows, to stay on.
        rows.push(Row {
            id: running.to_string(),
            note: "running now".into(),
        });
        (now, known) = (running.to_string(), true);
    }
    if known && !now.is_empty() && !rows.iter().any(|r| r.id == now) {
        rows.push(Row {
            id: now.clone(),
            note: "selected for this session".into(),
        });
    }
    let at = rows.iter().position(|r| known && r.id == now);
    (rows, at)
}

/// Opens /name's list for the open conversation; false when its agent
/// has none (openChoices).
pub fn open_choices(m: &mut Model, name: &str) -> bool {
    if name != "model" && name != "effort" {
        return false;
    }
    let Some(v) = m.session.as_ref() else {
        return false;
    };
    let Some(a) = m.agents.iter().find(|a| a.key == v.key) else {
        return false;
    };
    let kind = a.kind.as_str().to_string();
    let s = &v.sess;
    let running = if name == "effort" {
        s.effort().to_string()
    } else if s.info.model.is_empty() {
        s.model.clone()
    } else {
        s.info.model.clone()
    };
    let opts = if name == "model" {
        models(m, &kind)
    } else {
        arg_options(&m.reg, &kind, name).unwrap_or_default()
    };
    let (rows, now) = choice_rows(&opts, &running, None);
    let mut sheet = ChoiceSheet::new(
        name,
        "native session · r brings its conversation into rush to change settings",
        rows,
        Box::new(|_, _| Out::Close),
    );
    sheet.now = now;
    sheet.cur = now.unwrap_or(0);
    sheet.on_key = Some(Box::new(|m, k| match k {
        "enter" => {
            m.flash_err("Native session · press r to bring its conversation into rush first");
            Out::Draw
        }
        "r" => {
            m.flash_err("bringing a conversation into rush isn't in this rush yet");
            Out::Close
        }
        _ => Out::Pass,
    }));
    m.open_sheet(Box::new(sheet));
    true
}
