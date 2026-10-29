//! A view the agent has stopped keeping goes grey, so the eye goes to the
//! conversation instead: one the agent left alone for a whole turn, and
//! one that only says what the reply beside it says. rush's own rows at
//! its top (the subagents) stay bright: they're live whatever the agent
//! does. After Go's internal/ui/viewquiet.go (viewQuietKey :19, quietWhy
//! :29, viewQuiet :45, repeats :74, viewWords :89, wordSet :119).

use std::collections::HashSet;

use rush_convo::model::{Item as Said, Session, Turn};
use rush_core::time::Ms;

use super::model::{Item, View};
use super::panel::ViewsState;

/// What a view's greying is worked out from (viewQuietKey).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuietKey {
    name: String,
    updated: Ms,
    turns: usize,
    end: Ms,
}

impl ViewsState {
    /// Why the session's view v shows grey, in a few words for the
    /// panel's foot; "" when it doesn't. Kept until the view or the
    /// session's turns change (quietWhy).
    pub fn quiet_why(&mut self, s: Option<&Session>, v: &View) -> String {
        let Some(s) = s else { return String::new() };
        if !v.by.is_empty() || v.pinned {
            return String::new(); // a plugin's or one you brought back: theirs and yours
        }
        let k = QuietKey {
            name: v.name.clone(),
            updated: v.updated.to_ms(),
            turns: s.turns.len(),
            end: s.turns.last().map_or(0, |t| t.end),
        };
        if self.quiet_key != k {
            self.quiet_memo = view_quiet(&s.turns, v).to_string();
            self.quiet_key = k;
        }
        self.quiet_memo.clone()
    }
}

/// quiet_why worked out from the session's turns (viewQuiet).
pub fn view_quiet(turns: &[Turn], v: &View) -> &'static str {
    let updated = v.updated.to_ms();
    let mut made = None; // the turn the view was last set in
    for t in turns {
        if t.start > updated {
            if !t.live && t.end != 0 {
                return "not updated for a turn";
            }
            break;
        }
        made = Some(t);
    }
    let Some(made) = made.filter(|t| !t.live) else {
        return ""; // its reply is still being written
    };
    let mut reply = String::new();
    for it in &made.items {
        if let Said::Text { text, .. } = it {
            reply.push_str(text);
            reply.push(' ');
        }
    }
    if repeats(&view_words(v), &reply) {
        return "same as the reply";
    }
    ""
}

/// Whether most of a view's words, of eight or more, are the reply's too
/// (repeats).
fn repeats(words: &HashSet<String>, reply: &str) -> bool {
    if words.len() < 8 {
        return false;
    }
    let reply = word_set(reply);
    let n = words.iter().filter(|w| reply.contains(*w)).count();
    n * 10 >= words.len() * 7
}

/// The words the agent wrote in v (viewWords).
fn view_words(v: &View) -> HashSet<String> {
    let mut b = String::new();
    let mut add = |ss: &[&str]| {
        for s in ss {
            b.push_str(s);
            b.push(' ');
        }
    };
    add(&[&v.title, &v.summary, &v.note]);
    for g in &v.groups {
        add(&[&g.title, &g.text]);
        for r in &g.rows {
            add(&[&r.text, &r.meta]);
        }
    }
    fn items(its: &[Item], add: &mut dyn FnMut(&[&str])) {
        for it in its {
            add(&[&it.text, &it.meta, &it.from, &it.to]);
            let cells: Vec<&str> = it.cells.iter().map(String::as_str).collect();
            add(&cells);
            items(&it.items, add);
        }
    }
    for bl in &v.blocks {
        add(&[&bl.title, &bl.text]);
        items(&bl.items, &mut add);
    }
    word_set(&b)
}

/// s's words of four letters or more, lower case (wordSet).
fn word_set(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphabetic() && !c.is_numeric())
        .filter(|w| w.chars().count() >= 4)
        .map(String::from)
        .collect()
}
