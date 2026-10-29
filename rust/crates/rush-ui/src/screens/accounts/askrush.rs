//! #ask: an agent that answers questions about rush and changes its
//! settings, working in rush's own folder with the guide in its system
//! prompt; and taking in config.json when something other than this rush
//! changed it. After Go's internal/ui/askrush.go (askRush :15,
//! reloadConfig :38, onConfig :45) and model.go:1115 (looked at every
//! third tick: here config.json is watched, so an idle rush notices at
//! once and wakes for nothing else).

use super::state::{accounts, peek};
use crate::frame::Out;
use crate::model::Model;

/// rush's full guide (docs/guide.md), which the #ask agent answers from.
pub const GUIDE: &str = include_str!("../../../../../../docs/guide.md");

/// A session rush hosts, started for rush's own ends (#ask): what the
/// screens that host sessions are handed (startHosted with its config).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostedStart {
    /// The first message; "" starts it waiting.
    pub task: String,
    pub dir: String,
    /// Its name; "" lets the task name it.
    pub name: String,
    /// In a worktree of its own (false in rush's folder: no project's
    /// work goes on there).
    pub own_worktree: bool,
    pub system_prompt: String,
}

/// Starts a HostedStart: set by the screens that host sessions.
pub type HostedStarter = fn(&mut Model, HostedStart) -> Out;

/// What #ask starts for question, rush's folder being dir (askRush).
pub fn ask_start(question: &str, dir: &str) -> HostedStart {
    HostedStart {
        task: question.to_string(),
        dir: dir.to_string(),
        name: if question.is_empty() {
            "asking rush".into()
        } else {
            String::new()
        },
        own_worktree: false,
        system_prompt: format!(
            "The user is asking about rush itself: the terminal app you run inside, which runs and watches coding agents. \
             Answer from rush's guide below, and say so when it doesn't cover something rather than guessing.\n\n\
             You work in rush's own folder, {dir}. config.json there holds rush's settings; the other files are its records, so leave them alone. \
             When the user wants a setting changed, first say whether one of rush's # commands does it (they type it in rush's box, e.g. #mackeys on), or a page of Settings does. \
             Otherwise edit config.json: change only what was asked, keep it valid JSON, and tell them what you changed. \
             rush reads it again within a few seconds of the change.\n\n\
             <rush-guide>\n{GUIDE}\n</rush-guide>"
        ),
    }
}

/// #ask [question].
pub fn ask_rush(m: &mut Model, question: &str) -> Out {
    let dir = rush_state::dir().to_string_lossy().into_owned();
    let s = ask_start(question.trim(), &dir);
    match peek(m).hosted_starter {
        Some(f) => f(m, s),
        None => {
            m.flash_err("rush can't start sessions here yet");
            Out::Draw
        }
    }
}

/// After each reading of the list, once rush's config is loaded: a
/// worker watching config.json, which reads it when it changes and hands
/// it over when something other than this rush (an #ask agent, another
/// rush, an editor) changed it (reloadConfig). It waits on the file, with
/// no timer.
pub fn on_fleet(m: &mut Model) {
    if peek(m).store.is_none() || peek(m).reloading {
        return;
    }
    let Some(tx) = m.sender() else {
        return;
    };
    accounts(m).reloading = true;
    let tx = tx.lane(1);
    let path = rush_state::dir().join("config.json");
    let _ = std::thread::Builder::new()
        .name("rush-config".into())
        .spawn(move || {
            let w = rush_sys::fswait::Watcher::new();
            loop {
                // Watched again each time: a save renames a new file
                // over it.
                w.watch(&[&path]);
                if let Some(b) = rush_state::config_on_disk() {
                    let apply: crate::frame::work::Apply = Box::new(move |m| on_config(m, &b));
                    if !tx.send(crate::worker::Note::Apply(apply)) {
                        return;
                    }
                }
                w.wait(None, None);
                let _ = w.changes();
            }
        });
}

/// Takes config.json b in place of the one loaded (onConfig).
pub fn on_config(m: &mut Model, b: &[u8]) -> Out {
    let Some(store) = accounts(m).store.as_mut() else {
        return Out::Idle;
    };
    let err = store.reload(b).err().map(|e| e.to_string());
    let mut f = serde_json::Map::new();
    f.insert("why".into(), "config.json changed outside this rush".into());
    f.insert("err".into(), err.clone().unwrap_or_default().into());
    rush_events::log("config-reload", f);
    match err {
        Some(e) => m.flash_err(&format!("config.json changed, but rush can't read it: {e}")),
        None => m.flash("settings changed in config.json; rush is using them"),
    }
    Out::Draw
}
