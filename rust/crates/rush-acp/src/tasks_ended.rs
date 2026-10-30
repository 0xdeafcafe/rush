//! Background tasks Kimi's own records say have ended. Kimi tells of a task
//! that ends between turns only in the next turn's context, never over
//! ACP, so without this one finished in a minute reads as running for
//! good. Port of internal/adapters/acp/tasks_ended.go:1-59.

use std::path::Path;

use rush_core::event::{self, Event};
use serde::Deserialize;

use crate::kimi::kimi_home;
use crate::session::{Inner, lock};
use crate::util::glob;

/// Ends those of the session's background tasks its agent's own records say
/// have ended, and says which.
pub(crate) fn end_tasks(s: &Inner) -> Vec<String> {
    let (open, dir, home) = {
        let st = lock(&s.mu);
        if st.o.adapter != "kimi" {
            return Vec::new();
        }
        let mut home = kimi_home();
        for kv in &st.o.env {
            if let Some(v) = kv.strip_prefix("KIMI_CODE_HOME=") {
                home = v.to_string();
            }
        }
        let open: Vec<(String, String)> = st
            .calls
            .values()
            .filter(|c| !c.task.is_empty() && c.c.input.background && !c.ended)
            .map(|c| (c.c.id.clone(), c.task.clone()))
            .collect();
        (open, st.id.clone(), home)
    };
    let dir = if dir.starts_with("session_") {
        dir
    } else {
        format!("session_{dir}")
    };
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Task {
        status: String,
    }
    let mut ended = Vec::new();
    for (call, task) in open {
        let file = format!("{task}.json");
        let paths = glob(
            Path::new(&home),
            &["sessions", "wd_*", &dir, "agents", "*", "tasks", &file],
        );
        for p in paths {
            let Ok(raw) = std::fs::read(&p) else {
                continue;
            };
            let Ok(t) = serde_json::from_slice::<Task>(&raw) else {
                continue;
            };
            if t.status.is_empty() || t.status == "running" {
                continue;
            }
            let mut st = lock(&s.mu);
            if let Some(c) = st.calls.get_mut(&call)
                && !c.ended
            {
                c.ended = true;
                s.emit(Event::TaskDone(event::TaskDone {
                    id: task.clone(),
                    call_id: call.clone(),
                    status: t.status,
                    ..event::TaskDone::default()
                }));
                s.emit_background(&st);
                ended.push(task.clone());
            }
            break;
        }
    }
    ended
}
