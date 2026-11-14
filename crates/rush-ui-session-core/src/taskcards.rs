//! What the open conversation draws of its background tasks that a plugin
//! knows more of: a rush watch (pr or run) the agent put in the background
//! reads in the conversation as its card, in place of "running in the
//! background": what it waits on, its checks so far, and the time left as
//! its workflows usually take. The agent hears none of it: it's told once,
//! when the watch ends. Read from the dock's card (jobs::job_card_line),
//! once a frame, while the conversation is open. New in the Rust port.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rush_convo::model::LiveOut;
use rush_convo::render::BgOut;
use rush_core::text::Line;
use rush_ui_list_extras::jobs::job_card_line;

use crate::model::Model;

/// How often a conversation showing a card is drawn again: its clock.
const EVERY: Duration = Duration::from_secs(1);

/// Hands session key's open view what its background commands and
/// monitors have written, by the call that started each: the end of it
/// is drawn under the call, the call opened by itself while its task runs
/// (rush_convo's bg_rows). From the tails the runs' reader keeps
/// (Runs::from_of, once a second while anything runs); after
/// running::sync, so it's this frame's. Rust-only.
pub fn sync_out(m: &mut Model, key: &str) {
    // Its marks spin and its lines come in while one runs.
    if fill_out(m, key) {
        m.ask_wake(Instant::now() + EVERY);
    }
}

/// sync_out's work: whether a task it drew still runs.
fn fill_out(m: &mut Model, key: &str) -> bool {
    let now = m.now;
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_mut().filter(|v| v.key == key) else {
        return false;
    };
    let Some(r) = kept
        .peek::<super::Core>()
        .and_then(|c| c.conns.get(key))
        .map(|c| &c.runs)
    else {
        return false;
    };
    let s = &v.sess;
    let mut bg: HashMap<String, BgOut> = HashMap::new();
    for j in s.jobs() {
        if j.tool_use_id.is_empty() {
            continue;
        }
        // A monitor always runs beside the turn; a command once it's sent
        // to the background (a foreground one's output is its step's own).
        match s.job_kind(j) {
            "monitor" => {}
            "shell" if j.background => {}
            _ => continue,
        }
        let Some(t) = r.from_of(s, j, now).1 else {
            continue;
        };
        if t.lines.is_empty() {
            continue;
        }
        let dropped = t.nos.first().map_or(0, |n| n.saturating_sub(1));
        let out = LiveOut {
            out: t.lines.join("\n"),
            dropped,
        };
        let running = j.running();
        let same =
            v.o.bg
                .get(&j.tool_use_id)
                .filter(|b| *b.out == out && b.running == running);
        let b = match same {
            Some(b) => b.clone(),
            None => BgOut {
                out: Arc::new(out),
                running,
            },
        };
        bg.insert(j.tool_use_id.clone(), b);
    }
    let running = bg.values().any(|b| b.running);
    if v.o.bg != bg {
        v.o.bg = bg;
    }
    running
}

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
