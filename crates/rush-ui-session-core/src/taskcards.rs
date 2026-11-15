//! What the open conversation draws of its background tasks that a plugin
//! knows more of: a rush watch (pr or run) the agent put in the background
//! reads in the conversation as its card, in place of "running in the
//! background": what it waits on, its checks so far, and the time left as
//! its workflows usually take. The agent hears none of it: it's told once,
//! when the watch ends; once it has, its row says how it went, one line,
//! from the record the watch keeps a day. Laid out as the dock lays it
//! out (jobs::watch_head, taskrow.rs), once a frame, while the
//! conversation is open. New in the Rust port.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rush_convo::model::LiveOut;
use rush_convo::render::BgOut;
use rush_core::text::Line;
use rush_ui_list_extras::jobs::watch_head;

use crate::model::Model;

/// How often a conversation showing a card is drawn again: its clock.
/// The tick its spinners turn with moves once a second too
/// (rush_ui_base's wake_draw), so nothing is drawn more often.
const EVERY: Duration = Duration::from_secs(1);

/// The cells a call's note is drawn in from the conversation's edge: its
/// gutter, its indent and the "↳ " before it.
const GUTTER: usize = 10;

/// Whether cmd runs a rush watch (pr or run).
fn is_watch(cmd: &str) -> bool {
    cmd.contains("rush watch pr") || cmd.contains("rush watch run")
}

/// Hands session key's open view what its background commands and
/// monitors have written, by the call that started each, and the rows the
/// background view drew for them (running::task_rows): drawn under the
/// call, the call opened by itself while its task runs (rush_convo's
/// bg_rows). From the tails the runs' reader keeps (Runs::from_of, once a
/// second while anything runs); after running::sync, so it's this
/// frame's. Rust-only.
pub fn sync_out(m: &mut Model, key: &str) {
    let Some(w) = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| v.o.width)
    else {
        return;
    };
    // Under a step, on its gutter: a few cells in.
    let rows = super::running::task_rows(m, key, w.saturating_sub(6).max(40));
    // Its marks spin and its lines come in while one runs.
    if fill_out(m, key, rows) {
        m.ask_wake(Instant::now() + EVERY);
    }
}

/// sync_out's work, with each task's rows by its call: whether a task it
/// drew still runs.
fn fill_out(m: &mut Model, key: &str, mut rows: HashMap<String, Vec<Line>>) -> bool {
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
        if j.tool_use_id.is_empty() || !super::running::is_bg_step(s, j) {
            continue;
        }
        let lines = rows.remove(&j.tool_use_id).unwrap_or_default();
        let (dropped, text) = match r.from_of(s, j, now).1 {
            Some(t) => (
                t.nos.first().map_or(0, |n| n.saturating_sub(1)),
                t.lines.join("\n"),
            ),
            None => (0, String::new()),
        };
        if text.is_empty() && lines.is_empty() {
            continue;
        }
        let out = LiveOut { out: text, dropped };
        let running = j.running();
        let was = v.o.bg.get(&j.tool_use_id);
        let b = BgOut {
            out: match was.filter(|b| *b.out == out) {
                Some(b) => b.out.clone(),
                None => Arc::new(out),
            },
            running,
            // The same rows keep their Arc: the turn isn't drawn again.
            rows: match was.filter(|b| *b.rows == lines) {
                Some(b) => b.rows.clone(),
                None => Arc::new(lines),
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

/// Hands session key's open view the head rows of its background tasks
/// that have a card: a rush watch running (its card's head, as the dock
/// draws it: taskrow.rs), and one that has ended (how it went, one row).
/// Laid out to the conversation's width less its step's gutter; its
/// spinner turns with the tick, which moves once a second, so a turn
/// with one running is drawn again once a second, no more.
pub fn sync(m: &mut Model, key: &str) {
    let (now, tick) = (m.now, m.tick);
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return;
    };
    let w = v.o.width.saturating_sub(GUTTER).max(40);
    let s = &v.sess;
    let mut any = false;
    let rows: HashMap<String, Line> = s
        .jobs()
        .iter()
        .filter(|j| j.background)
        .filter_map(|j| {
            // A finished task is asked of only when it was a rush watch.
            let watch = !j.running() && is_watch(&s.job_command(j));
            if !j.running() && !watch {
                return None;
            }
            let row = watch_head(j, watch, tick, now, w)?;
            any |= j.running();
            Some((j.id.clone(), row))
        })
        .collect();
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key)
        && v.o.tasks != rows
    {
        v.o.tasks = rows;
    }
    if any {
        m.ask_wake(Instant::now() + EVERY);
    }
}
