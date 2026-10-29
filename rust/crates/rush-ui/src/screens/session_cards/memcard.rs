//! The memory card in a session's dock: a step waiting on you that writes
//! one of its agent's memory notes (a Write or Edit, or a shell heredoc
//! into one) is asked about as the note, not the call. What's on disk is
//! read off the UI thread, once per approval. e allows it, then opens the
//! note in the memory view once it's written.
//!
//! Go: internal/ui/rushmode.go:3136 (cardRows' memory card), :6040-6078
//! (cardKey's memory buttons, no always for a shell write, e),
//! memcard.go (peekMem :151, memoryCard :266, memBtnsFor :425,
//! openWritten :540). The card itself is projects_machine::memory's.

use std::collections::{HashMap, HashSet};

use rush_convo::Status;
use rush_core::text::Line;
use rush_core::tool::Call;

use super::CardBtn;
use crate::model::Model;
use crate::screens::projects_machine::memory::{
    MemCard, MemPeek, NoteWrite, mem_btns_for, mem_note_of, memory_card, peek_mem,
};
use crate::screens::session_core::views;

/// What's on disk for each note a card asks about, by approval and path,
/// and the notes to open once written, by session.
#[derive(Default)]
struct MemCards {
    peeks: HashMap<String, MemPeek>,
    reading: HashSet<String>,
    /// Session key to (step id, note's path): e was pressed on it.
    edit_after: HashMap<String, (String, String)>,
}

/// The memory note call writes, when its card is the memory card.
pub fn note_of(call: &Call) -> Option<NoteWrite> {
    mem_note_of(call)
}

/// The note at path as approval id found it, reading it off the UI
/// thread the first time (peekMem): None until it's read.
fn peek(m: &mut Model, id: &str, path: &str) -> Option<MemPeek> {
    let k = format!("{id} {path}");
    let mc = m.kept::<MemCards>();
    if let Some(p) = mc.peeks.get(&k) {
        return Some(p.clone());
    }
    if mc.reading.insert(k.clone()) {
        let path = path.to_string();
        m.later(
            move || peek_mem(&path),
            move |m, p| {
                let mc = m.kept::<MemCards>();
                mc.reading.remove(&k);
                mc.peeks.insert(k, p);
                crate::frame::Out::Draw
            },
        );
    }
    let mc = m.kept::<MemCards>();
    mc.peeks.get(&format!("{id} {path}")).cloned()
}

/// The memory card for note, approval id waiting on you, w wide in at
/// most max_h rows, and its buttons.
#[allow(clippy::too_many_arguments)]
pub fn rows(
    m: &mut Model,
    id: &str,
    note: &NoteWrite,
    focus: bool,
    pick: usize,
    hover: &str,
    w: usize,
    max_h: usize,
) -> (Vec<Line>, Vec<CardBtn>) {
    let input = &note.call.input;
    let p = peek(m, id, &input.path);
    let known = p.is_some();
    let p = p.unwrap_or_default();
    let c = MemCard {
        input,
        edit: note.call.kind == rush_core::tool::Kind::Edit,
        appends: note.appends,
        shell: note.shell,
        peek: &p,
        known,
        focus,
        pick,
        hover,
    };
    let (rows, btns) = memory_card(&c, w, max_h);
    let btns = btns
        .into_iter()
        .map(|b| CardBtn {
            x: b.x,
            y: b.y,
            w: b.w,
            key: b.key,
        })
        .collect();
    (rows, btns)
}

/// The keys of the buttons the memory card for note has.
pub fn btn_keys(note: &NoteWrite) -> Vec<String> {
    mem_btns_for(note.shell)
        .iter()
        .map(|b| b[0].to_string())
        .collect()
}

/// e on the card: once step id of session key is written, the memory
/// view opens on the note.
pub fn edit_after(m: &mut Model, key: &str, step: &str, path: &str) {
    m.kept::<MemCards>()
        .edit_after
        .insert(key.to_string(), (step.to_string(), path.to_string()));
}

/// Opens the note e asked to edit, in the memory view, once its step is
/// written (openWritten); a step that didn't write it is let go.
pub fn open_written(m: &mut Model, key: &str) {
    let Some((step, path)) = m.kept::<MemCards>().edit_after.get(key).cloned() else {
        return;
    };
    let status = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .and_then(|v| v.sess.steps().iter().find(|s| *s.id == *step))
        .map(|s| s.status);
    match status {
        None | Some(Status::Running | Status::Waiting) => {}
        Some(st) => {
            m.kept::<MemCards>().edit_after.remove(key);
            if st == Status::Ok && views::show_view(m, key, "memory") {
                crate::screens::session_core::memory::forget(m, key);
                if let Some(v) = m.session.as_mut() {
                    v.sel = format!("mem:{path}");
                }
            }
        }
    }
}
