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

/// Whether a key can take a whole box at once: cmd+backspace and its
/// like, from the cursor to a line's end or start.
pub(crate) fn wipes(key: &str) -> bool {
    matches!(
        key,
        "super+backspace" | "ctrl+u" | "super+delete" | "ctrl+k"
    )
}

impl Model {
    /// What each box holds, by its session's key ("" for the Agents box).
    pub(crate) fn box_texts(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.prompt_undoes() {
            out.push((String::new(), self.input.string()));
        }
        for v in [self.session.as_ref(), self.pane.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(input) = crate::sess::conn_box(self, &v.key)
                && !out.iter().any(|(k, _)| *k == v.key)
            {
                out.push((v.key.clone(), input.string()));
            }
        }
        out
    }

    /// A box a key emptied whole is cleared as esc clears it: kept in
    /// the stash's history as Cleared, and the flash says undo brings it
    /// back (undo already does).
    pub(crate) fn note_wiped(&mut self, held: Vec<(String, String)>) {
        for (key, was) in held {
            if was.trim().is_empty() {
                continue;
            }
            let now = if key.is_empty() {
                self.prompt_undoes().then(|| self.input.string())
            } else {
                crate::sess::conn_box(self, &key).map(|e| e.string())
            };
            if now.is_some_and(|t| t.is_empty()) {
                self.ext.calls.cleared()(self, &key, &was);
            }
        }
    }

    /// The Agents box holds a message, whose changes undo takes back: not
    /// a name being renamed.
    fn prompt_undoes(&self) -> bool {
        self.ext.calls.renaming()(self).is_none()
    }

    pub(crate) fn guard_boxes(&self) -> BoxGuard {
        let prompt = self.prompt_undoes().then(|| Held::of(&self.input));
        let mut conns = Vec::new();
        for v in [self.session.as_ref(), self.pane.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(input) = crate::sess::conn_box(self, &v.key)
                && !conns.iter().any(|(k, _): &(String, Held)| *k == v.key)
            {
                conns.push((v.key.clone(), Held::of(input)));
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
        for (key, h) in g.conns {
            if let Some(input) = crate::sess::conn_box_mut(self, &key) {
                h.keep(input);
            }
        }
    }

    /// Runs f, keeping any change it made to a box for undo.
    #[doc(hidden)]
    pub fn guarded<T>(&mut self, f: impl FnOnce(&mut Model) -> T) -> T {
        let g = self.guard_boxes();
        let out = f(self);
        self.keep_undo(g);
        out
    }
}
