//! The runs a session's own subagent calls put in the background, each a
//! row its call's step draws in place of the boilerplate the harness
//! returned for it ("Async agent launched… output_file… Do NOT Read…"):
//! its mark, what it was asked, how it stands, and "open ›"
//! (rush_convo's Options::runs, by the call's id). Rust-only: Go drew the
//! call's result as text.

use std::collections::HashMap;

use rush_core::text::{Line, Span, Style};

use super::paint::{dim, faint};
use super::subname::{sub_mark, sub_task, sub_word};
use super::subview::kind_of;
use crate::model::Model;
use crate::style::*;

/// Sets the open session key's rows for its runs, as they stand now: one
/// for each run its call started (by the call's id). Run as the dock is
/// drawn, after the runs are read.
pub fn sync(m: &mut Model, key: &str) {
    let home = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let (now, tick) = (m.now, m.tick);
    let reg = m.reg.clone();
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_mut().filter(|v| v.key == key) else {
        return;
    };
    let rows: HashMap<String, Line> = {
        let Some(r) = kept
            .peek::<super::Core>()
            .and_then(|c| c.conns.get(key))
            .map(|c| &c.runs)
        else {
            if !v.o.runs.is_empty() {
                v.o.runs.clear();
            }
            return;
        };
        let s = &v.sess;
        let jobs = s.subagent_jobs();
        let mut rows = HashMap::new();
        for sa in &r.subs {
            // A workflow's agents are its row's, not a call's.
            if sa.tool_use_id.is_empty() || !sa.workflow.is_empty() {
                continue;
            }
            let (status, live) = r.state(s, sa, &jobs, now);
            let kind = kind_of(r, &home, &sa.id);
            let ink = if live { bold(TEXT) } else { Style::fg(TEXT) };
            let row = line([
                sub_mark(&reg, &kind, &status, live, tick),
                plain(" "),
                Span::new(sub_task(sa), ink),
                dim(" · "),
                sub_word(&status, live),
                plain("  "),
                faint("open ›"),
            ]);
            rows.entry(sa.tool_use_id.clone()).or_insert(row);
        }
        rows
    };
    v.o.runs = rows;
}
