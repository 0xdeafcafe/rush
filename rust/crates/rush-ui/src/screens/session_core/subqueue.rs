//! A subagent you're watching has a queue of its own: what you send it
//! waits there, and one message goes into its inbox each time it finishes
//! a step; a run that finished hands what it still had to the main
//! session. After Go's internal/ui/subqueue.go (subQKey :21, queueSub :34,
//! subSentMsg/onSubSent :75-115, noteSub :94, subNoteLines :119, said
//! :144, flushSubQueues :163, sendSubQueueNow :193).
//!
//! The queue, its notes and when each message is due are here; relay.rs
//! routes the box to a watched subagent through them.

use rush_convo::Session;
use rush_convo::model::Item;
use rush_core::text::Line;
use rush_core::time::Ms;

use super::host::join_queue;
use super::queue::LocalQueue;
use crate::style::*;

/// The key of session key's subagent id's queue and notes.
pub fn key(session: &str, id: &str) -> String {
    format!("{session}\x00{id}")
}

/// A message meant for a subagent that its conversation doesn't show:
/// sent another way, or in its inbox and not read yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubNote {
    pub text: String,
    pub how: String,
    /// Into its inbox.
    pub told: bool,
}

/// How many notes each subagent keeps.
pub const NOTES_KEPT: usize = 10;

/// Adds n to notes, keeping the last NOTES_KEPT.
pub fn note(notes: &mut Vec<SubNote>, n: SubNote) {
    notes.push(n);
    let extra = notes.len().saturating_sub(NOTES_KEPT);
    notes.drain(..extra);
}

/// Adds text to a watched subagent's queue; steps is how many it has
/// taken. The first waits for the end of the step it's on, unless one
/// went in on this step already (queueSub).
pub fn queue(q: &mut LocalQueue, steps: usize, text: &str, now: Ms) {
    if q.items.is_empty() {
        q.since = now;
        if q.sent_at == 0 || steps > q.at {
            q.at = steps.saturating_sub(1);
        }
    }
    q.items.push(text.to_string());
}

/// The flash queue gives, for a subagent of kind.
pub fn queued_words(kind: &str) -> String {
    format!("queued for {kind} · it reads it when the step it's on ends")
}

/// What flushing a watched subagent's queue does now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    Nothing,
    /// Into its inbox: the next message.
    Tell(String),
    /// It ended first: all of it goes to the main session, as this.
    Main {
        mine: String,
        text: String,
    },
}

/// What q sends now for the subagent of kind, steps taken, live or not;
/// q is changed as though sent (flushSubQueues).
pub fn due(q: &mut LocalQueue, kind: &str, live: bool, steps: usize, now: Ms) -> Due {
    if q.items.is_empty() {
        return Due::Nothing;
    }
    if !live {
        let mine = join_queue(&q.items).trim().to_string();
        let text =
            format!("For your subagent {kind}, which finished before it got this:\n\n{mine}");
        q.items.clear();
        return Due::Main { mine, text };
    }
    if !q.held && steps > q.at {
        let text = q.items.remove(0);
        q.at = steps;
        q.sent_at = now;
        return Due::Tell(text);
    }
    Due::Nothing
}

/// All q has, and extra after it, as one message now; None when there's
/// nothing (sendSubQueueNow).
pub fn now_all(q: &mut LocalQueue, extra: &str, steps: usize, now: Ms) -> Option<String> {
    let mut items = std::mem::take(&mut q.items);
    if !extra.is_empty() {
        items.push(extra.to_string());
    }
    if items.is_empty() {
        return None;
    }
    q.sent_at = now;
    q.at = steps;
    Some(join_queue(&items).trim().to_string())
}

/// Whether s has text among your messages (said).
pub fn said(s: &Session, text: &str) -> bool {
    s.turns.iter().any(|t| {
        t.prompt.contains(text)
            || t.items
                .iter()
                .any(|it| matches!(it, Item::Interject(x) if x.text.contains(text)))
    })
}

/// The watched subagent's notes, the latest three, w wide; one in its
/// inbox goes once its conversation (sub) shows it (subNoteLines).
pub fn note_lines(notes: &[SubNote], sub: &Session, live: bool, w: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for n in notes {
        if n.told && said(sub, &n.text) {
            continue;
        }
        let how = if n.told && !live {
            "it ended before reading it · the main session gets it"
        } else {
            n.how.as_str()
        };
        let how = format!(" · {how}");
        let room = 10.max(w.saturating_sub(6 + rush_core::text::width(&how)));
        let text = rush_draw::truncate(&one_line(&n.text), room, "…").into_owned();
        out.push(line([
            plain("  "),
            sp("↪ ", SUB),
            sp(text, TEXT),
            sp(how, DIM),
        ]));
    }
    let skip = out.len().saturating_sub(3);
    out.split_off(skip)
}
