//! What the open conversation draws of its background tasks that a plugin
//! knows more of: a rush watch (pr or run) the agent put in the background
//! reads in the conversation as its card, in place of "running in the
//! background": what it waits on, its checks so far, and the time left as
//! its workflows usually take. The agent hears none of it: it's told once,
//! when the watch ends. Read from the dock's card (jobs::job_card_line),
//! once a frame, while the conversation is open. New in the Rust port.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rush_core::text::Line;
use rush_ui_list_extras::jobs::job_card_line;

use crate::model::Model;

/// How often a conversation showing a card is drawn again: its clock.
const EVERY: Duration = Duration::from_secs(1);

/// Hands session key's open view the rows of its running background
/// tasks that have a card.
pub fn sync(m: &mut Model, key: &str) {
    let now = m.now;
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return;
    };
    let rows: HashMap<String, Line> = v
        .sess
        .running_jobs()
        .filter(|j| j.background)
        .filter_map(|j| Some((j.id.clone(), job_card_line(j, now)?)))
        .collect();
    let any = !rows.is_empty();
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key)
        && v.o.tasks != rows
    {
        v.o.tasks = rows;
    }
    if any {
        m.ask_wake(Instant::now() + EVERY);
    }
}
