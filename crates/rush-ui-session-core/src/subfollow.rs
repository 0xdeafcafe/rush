//! The running subagent picked in the Subagents tab, followed as its
//! transcript grows, the way the open conversation follows its own: a
//! thread waits on the file (rush_sys::fswait, kqueue on macOS) and wakes
//! the UI the moment something is written, and the runs are read again
//! then rather than on the next second's refresh. Only while the tab
//! shows and the subagent runs: one hidden or finished is let go, so a
//! quiet one costs nothing. New in the Rust port.

use std::sync::Arc;

use rush_sys::fswait::{Stop, Watcher};

use super::subs::DetailSrc;
use super::{conn, open_key};
use crate::frame::Out;
use crate::model::Model;
use crate::worker::Note;

/// The run followed: its id, the file waited on, and what ends the wait
/// (closed when this goes). No wait with no loop (tests): woken by hand.
pub struct Follow {
    pub id: String,
    pub path: String,
    stop: Option<Arc<Stop>>,
}

impl Drop for Follow {
    fn drop(&mut self) {
        if let Some(s) = &self.stop {
            s.close();
        }
    }
}

/// The run to follow in session key now, and its transcript: the one
/// picked in the Subagents tab, showing, and running.
fn want(m: &Model, key: &str) -> Option<(String, String)> {
    if open_key(m).as_deref() != Some(key) || super::views::view_name(m, key) != "subagents" {
        return None;
    }
    let v = m.session.as_ref().filter(|v| v.key == key)?;
    let id = v.sel.strip_prefix("sub:")?;
    if !super::subview::run_live(m, key, id) {
        return None;
    }
    let path = match super::subview::src_of(m, key, id)? {
        DetailSrc::Transcript { path, .. } | DetailSrc::History { path, .. } => path,
    };
    (!path.is_empty()).then(|| (id.to_string(), path))
}

/// Follows what session key wants followed, and lets go of what it no
/// longer does. Run with each reading of its runs.
pub fn sync(m: &mut Model, key: &str) {
    let want = want(m, key);
    let has = conn(m, key)
        .runs
        .live
        .as_ref()
        .map(|f| (f.id.clone(), f.path.clone()));
    if want == has {
        return;
    }
    let tx = m.sender();
    conn(m, key).runs.live = want.map(|(id, path)| start(tx, key, id, path));
}

/// Starts waiting on path for run id of session key: each write wakes the
/// UI (woke) once it has taken the last.
fn start(tx: Option<rush_draw::Sender<Note>>, key: &str, id: String, path: String) -> Follow {
    let Some(tx) = tx else {
        return Follow {
            id,
            path,
            stop: None,
        };
    };
    let Ok(stop) = Stop::new().map(Arc::new) else {
        return Follow {
            id,
            path,
            stop: None,
        };
    };
    let tx = tx.lane(1);
    let (st, p, k, i) = (stop.clone(), path.clone(), key.to_string(), id.clone());
    let _ = std::thread::Builder::new()
        .name("rush-subfollow".into())
        .spawn(move || {
            let w = Watcher::new();
            loop {
                // Watched again each time: a file replaced is followed.
                w.watch(std::slice::from_ref(&p));
                if !w.wait(Some(&st), None) {
                    return;
                }
                let _ = w.changes();
                let (k, i) = (k.clone(), i.clone());
                if !tx.send(Note::Apply(Box::new(move |m: &mut Model| woke(m, &k, &i)))) {
                    return;
                }
            }
        });
    Follow {
        id,
        path,
        stop: Some(stop),
    }
}

/// Run id's transcript grew: its runs are read now, or, no longer wanted
/// (the tab hidden, the run finished), it's let go.
pub fn woke(m: &mut Model, key: &str, id: &str) -> Out {
    sync(m, key);
    let followed = conn(m, key).runs.live.as_ref().is_some_and(|f| f.id == id);
    if !followed {
        return Out::Idle;
    }
    super::subs::kick(m, key);
    super::subs::refresh(m, key);
    Out::Draw
}
