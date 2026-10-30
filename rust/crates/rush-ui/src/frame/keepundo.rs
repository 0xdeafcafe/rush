//! Undo for every way text leaves a message box. Typing, a paste, the
//! box cleared with esc or ctrl+c already keep what was there (Go's
//! internal/ui/editor.go:267 editInput, stash.go:173 wipeBox); this keeps
//! it for everything else that changes a box without saying so: a #
//! picker or a / command completing over what's typed, a plugin or the
//! history setting it, the box emptied going to another place, an
//! editor's text coming back. What was sent lets its undo go
//! (Editor::clear_sent), as Go's submit does; text moved to another box
//! says so (Undo::moved).
//!
//! Each event handled is compared with what the Agents box and the
//! opened Sessions' boxes held before it: a box that changed while its
//! undo didn't gets the text it had as one step to undo.

use crate::model::Model;
use crate::screens::session_core::Core;

/// What one box held before an event.
struct Held {
    text: Vec<char>,
    pos: usize,
    ver: u64,
}

impl Held {
    fn of(e: &super::Editor) -> Held {
        Held {
            text: e.text.clone(),
            pos: e.pos,
            ver: e.undo.ver(),
        }
    }

    /// Keeps the text as a step to undo when the box changed and nothing
    /// said so.
    fn keep(self, e: &mut super::Editor) {
        if e.undo.ver() == self.ver && e.text != self.text {
            e.undo.save(&self.text, self.pos, false);
        }
    }
}

/// What the boxes held before an event.
pub(crate) struct BoxGuard {
    prompt: Option<Held>,
    conns: Vec<(String, Held)>,
}

impl Model {
    /// The Agents box holds a message, whose changes undo takes back: not
    /// a name being renamed.
    fn prompt_undoes(&self) -> bool {
        crate::screens::list_extras::rename::renaming(self).is_none()
    }

    pub(crate) fn guard_boxes(&self) -> BoxGuard {
        let prompt = self.prompt_undoes().then(|| Held::of(&self.input));
        let mut conns = Vec::new();
        if let Some(core) = self.kept.peek::<Core>() {
            for v in [self.session.as_ref(), self.pane.as_ref()]
                .into_iter()
                .flatten()
            {
                if let Some(c) = core.conns.get(&v.key)
                    && !conns.iter().any(|(k, _): &(String, Held)| *k == v.key)
                {
                    conns.push((v.key.clone(), Held::of(&c.input)));
                }
            }
        }
        BoxGuard { prompt, conns }
    }

    pub(crate) fn keep_undo(&mut self, g: BoxGuard) {
        if let Some(h) = g.prompt
            && self.prompt_undoes()
        {
            h.keep(&mut self.input);
        }
        if g.conns.is_empty() {
            return;
        }
        let core = self.kept::<Core>();
        for (key, h) in g.conns {
            if let Some(c) = core.conns.get_mut(&key) {
                h.keep(&mut c.input);
            }
        }
    }

    /// Runs f, keeping any change it made to a box for undo.
    pub(crate) fn guarded<T>(&mut self, f: impl FnOnce(&mut Model) -> T) -> T {
        let g = self.guard_boxes();
        let out = f(self);
        self.keep_undo(g);
        out
    }
}
