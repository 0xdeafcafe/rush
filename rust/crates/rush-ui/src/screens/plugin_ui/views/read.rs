//! What rush last read of each session's views, and the reading. After
//! Go's internal/ui/agentviews.go (sessionViews :26, viewsOf :35,
//! pastViewsOf :63, viewSummary :85, refreshViews :966) and the files
//! internal/host/views.go writes (rush-host's views).
//!
//! Go reads on the UI thread while drawing, at most once a second. Here
//! nothing reads on the UI thread: the open session's views are followed
//! by a thread of their own that waits on its views folder (kqueue) and
//! reads only when it changes, so an idle panel costs nothing; the list's
//! summaries are read after each reading of the fleet, only for sessions
//! whose folder changed. With no loop (tests, `--render`) both read at
//! once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use rush_core::agent::Agent;
use rush_sys::fswait::{Stop, Watcher};

use super::model::View;
use super::panel::ViewsState;
use super::{plugin_draws_views, shown_views, view_summary};
use crate::frame::Out;
use crate::model::Model;
use crate::worker::Note;

/// What rush last read of one session's views (sessionViews).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionViews {
    pub list: Vec<View>,
    /// Put away, the one gone last first.
    pub past: Vec<View>,
    /// When its folder last changed, as read.
    pub at: Option<SystemTime>,
}

/// The views rush has read, by session id, the session it follows, and
/// how you've left each session's panel, by the session's key.
#[derive(Default)]
pub struct Views {
    pub by_id: HashMap<String, SessionViews>,
    pub states: HashMap<String, ViewsState>,
    /// The session whose views folder is followed, and what stops it.
    following: String,
    stop: Option<Arc<Stop>>,
}

impl Drop for Views {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            s.close();
        }
    }
}

/// The views folder's id for agent a: a rush session's host id, "" for
/// any other (Go's a.ID with a.Rush).
pub fn id_of(a: &Agent) -> &str {
    if a.rush { &a.job.id } else { "" }
}

/// Reads session id's views and past views now: for a worker.
pub fn read(id: &str) -> SessionViews {
    let (list, at) = rush_host::views::views(id);
    SessionViews {
        list,
        past: rush_host::views::past_views(id),
        at,
    }
}

/// The session's views as they show: a plugin's only while it runs
/// allowed to draw them (viewsOf).
pub fn views_of(m: &Model, id: &str) -> Vec<View> {
    let Some(sv) = m.kept.peek::<Views>().and_then(|v| v.by_id.get(id)) else {
        return Vec::new();
    };
    if sv.list.iter().all(|v| v.by.is_empty()) {
        return sv.list.clone();
    }
    shown_views(&sv.list, |p| plugin_draws_views(m, p))
}

/// The session's views put away, as last read (pastViewsOf).
pub fn past_views_of(m: &Model, id: &str) -> Vec<View> {
    m.kept
        .peek::<Views>()
        .and_then(|v| v.by_id.get(id))
        .map(|sv| sv.past.clone())
        .unwrap_or_default()
}

/// The view whose summary agent a's row gives the list, if any
/// (viewSummary): only a rush session keeps views.
pub fn summary_of(m: &Model, a: &Agent) -> Option<View> {
    let id = id_of(a);
    if id.is_empty() {
        return None;
    }
    view_summary(&views_of(m, id)).cloned()
}

/// Keeps what was read of id: whether it differs from what was kept.
fn took(m: &mut Model, id: &str, sv: SessionViews) -> bool {
    let v = m.kept::<Views>();
    if v.by_id.get(id) == Some(&sv) {
        return false;
    }
    v.by_id.insert(id.to_string(), sv);
    true
}

/// Follows session id's views folder, the open session's, reading them
/// again whenever it changes; "" stops following. A thread waits on the
/// folder, and reads only when it changed: nothing runs while nothing
/// changes. With no loop it reads once, now.
pub fn follow(m: &mut Model, id: &str) {
    let v = m.kept::<Views>();
    if v.following == id {
        return;
    }
    v.following = id.to_string();
    if let Some(s) = v.stop.take() {
        s.close();
    }
    if id.is_empty() {
        return;
    }
    let Some(tx) = m.sender() else {
        let sv = read(id);
        took(m, id, sv);
        return;
    };
    let Ok(stop) = Stop::new() else {
        return;
    };
    let stop = Arc::new(stop);
    m.kept::<Views>().stop = Some(stop.clone());
    let tx = tx.lane(1);
    let id = id.to_string();
    let _ = std::thread::Builder::new()
        .name("rush-views".into())
        .spawn(move || {
            let w = Watcher::new();
            // Watched before each reading, so a change during it still
            // wakes; a folder not made yet waits on the one above it.
            w.watch(&[rush_host::paths::dir(&id).join("views")]);
            let mut last: Option<SessionViews> = None;
            loop {
                let _ = w.changes();
                let sv = read(&id);
                if last.as_ref() != Some(&sv) {
                    last = Some(sv.clone());
                    let id = id.clone();
                    let sent = tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                        if took(m, &id, sv) {
                            Out::Draw
                        } else {
                            Out::Idle
                        }
                    })));
                    if !sent {
                        break; // the loop has gone
                    }
                }
                if stop.is_closed() || !w.wait(Some(&stop), None) {
                    break;
                }
            }
            w.close();
        });
}

/// Reads session id's views again at once: you changed them
/// (refreshViews). Off the UI thread, as everything is.
pub fn refresh(m: &mut Model, id: &str) {
    let id = id.to_string();
    let read_id = id.clone();
    m.later(
        move || read(&read_id),
        move |m, sv| {
            if took(m, &id, sv) {
                Out::Draw
            } else {
                Out::Idle
            }
        },
    );
}

/// After a reading of the fleet: the list's summaries, read again for
/// each rush session whose views folder changed since last read, all in
/// one go off the UI thread. The open session's are its follower's.
pub fn on_fleet(m: &mut Model) {
    let following = m.kept::<Views>().following.clone();
    let known: Vec<(String, Option<SystemTime>)> = m
        .agents
        .iter()
        .map(id_of)
        .filter(|id| !id.is_empty() && *id != following)
        .map(|id| {
            let at = m
                .kept
                .peek::<Views>()
                .and_then(|v| v.by_id.get(id))
                .and_then(|sv| sv.at);
            (id.to_string(), at)
        })
        .collect();
    if known.is_empty() {
        return;
    }
    m.later(
        move || {
            known
                .into_iter()
                .filter_map(|(id, at)| {
                    let now = rush_host::views::views_changed(&id);
                    (now != at).then(|| {
                        let sv = read(&id);
                        (id, sv)
                    })
                })
                .collect::<Vec<_>>()
        },
        |m, read| {
            let mut changed = false;
            for (id, sv) in read {
                changed |= took(m, &id, sv);
            }
            if changed { Out::Draw } else { Out::Idle }
        },
    );
}

/// How you've left session key's panel.
pub fn state<'a>(m: &'a mut Model, key: &str) -> &'a mut ViewsState {
    m.kept::<Views>().states.entry(key.to_string()).or_default()
}

/// The session key's panel as last drawn, if it was.
pub fn state_of<'a>(m: &'a Model, key: &str) -> Option<&'a ViewsState> {
    m.kept.peek::<Views>()?.states.get(key)
}
