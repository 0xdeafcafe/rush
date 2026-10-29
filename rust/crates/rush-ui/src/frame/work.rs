//! Work handed off the UI thread and its result handed back: nothing that
//! waits (the disk, the network, another process) runs on the UI thread.
//! After Go's sheet.go:24 (sheetMsg, sheetDo) and the `later` and
//! `applyMsg` it stands beside (model.go): a closure the UI thread runs
//! with the model, sent on a lane of its own (DESIGN.md "Threads").

use std::thread;

use rush_draw::Sender;

use super::Out;
use crate::model::Model;
use crate::worker::Note;

/// What the UI thread runs with the model when a Note::Apply lands.
pub type Apply = Box<dyn FnOnce(&mut Model) -> Out + Send>;

impl Model {
    /// Runs work on a thread of its own, then apply with its result on the
    /// UI thread (later). With no loop (tests, `rush --render`) both run
    /// at once, as Go's Frame drains what a key started.
    pub fn later<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut Model, T) -> Out + Send + 'static,
    ) {
        let Some(tx) = self.sender() else {
            let v = work();
            let _ = apply(self, v);
            return;
        };
        let tx = tx.lane(1);
        let spawned = thread::Builder::new()
            .name("rush-later".into())
            .spawn(move || {
                let v = work();
                tx.send(Note::Apply(Box::new(move |m: &mut Model| apply(m, v))));
            });
        if spawned.is_err() {
            self.fail("couldn't start a thread for that");
        }
    }

    /// A sender into the loop, for a worker of a screen's own: give it a
    /// lane of its own (`lane(cap)`) and send `Note::Apply`. None with no
    /// loop. Never send from the UI thread itself.
    pub fn sender(&self) -> Option<Sender<Note>> {
        self.tx.clone()
    }
}
