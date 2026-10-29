//! A background task can be left running long after it's any use: a
//! monitor waiting on what never comes, a loop polling a queue that moved
//! on. Rush never stops one; it asks the agent to look, since it knows
//! what each was for. Subagents and workflows aren't asked about: they're
//! working, rush shows their steps, and the agent hears when they end.
//!
//! Ported from internal/host/longtask.go (the server's share is in
//! server/watch.rs).

use std::collections::HashMap;
use std::time::Duration;

use rush_state::Time;

use crate::stuck::{agent_command, short};
use crate::wire::Task;

/// A background task running this long is asked about.
pub const LONG_TASK_AFTER: Duration = Duration::from_secs(30 * 60);
/// And a session is asked at most this often.
pub const LONG_TASK_AGAIN: Duration = Duration::from_secs(3600);
/// A task asked about once is asked about again only this long after:
/// most were fine (a server kept up, a long build), and said so.
pub const LONG_TASK_REASK: Duration = Duration::from_secs(4 * 3600);

/// The key in nudged for when the session was last asked.
pub const LONG_TASK_LAST: &str = "";

fn since(now: Time, t: Time) -> Duration {
    Duration::from_nanos(now.sub_nanos(t).max(0) as u64)
}

/// The shells and monitors running past LONG_TASK_AFTER not asked about in
/// the last LONG_TASK_REASK. Once one is due, so is every other running
/// past LONG_TASK_AFTER, so they're asked about in one check-in; and none
/// is asked about within LONG_TASK_AGAIN of the last check-in.
pub fn overdue_tasks(tasks: &[Task], nudged: &HashMap<String, Time>, now: Time) -> Vec<Task> {
    if let Some(&at) = nudged.get(LONG_TASK_LAST)
        && since(now, at) < LONG_TASK_AGAIN
    {
        return Vec::new();
    }
    let mut long = Vec::new();
    let mut fresh = false;
    for t in tasks {
        if t.started_at.is_zero()
            || since(now, t.started_at) < LONG_TASK_AFTER
            || subagent_task(&t.kind)
        {
            continue;
        }
        long.push(t.clone());
        if nudged
            .get(&t.id)
            .is_none_or(|&at| since(now, at) >= LONG_TASK_REASK)
        {
            fresh = true;
        }
    }
    if !fresh {
        return Vec::new();
    }
    long
}

/// Whether a task of this type is an agent doing work.
pub fn subagent_task(typ: &str) -> bool {
    matches!(
        typ,
        "local_agent" | "remote_agent" | "in_process_teammate" | "local_workflow"
    )
}

/// What the agent is told about them.
pub fn long_task_note(due: &[Task], now: Time) -> String {
    let mut b =
        String::from("Rush check-in: these background tasks have been running a long time:\n");
    for t in due {
        let ran = since(now, t.started_at);
        let ran = Duration::from_secs((ran.as_secs() + 30) / 60 * 60); // to the minute
        let label = if t.label.is_empty() { &t.id } else { &t.label };
        b += &format!(
            "- {} {} ({}), running {}\n",
            if t.kind.is_empty() { "task" } else { &t.kind },
            crate::reload::go_quote(&short(&agent_command(label))),
            t.id,
            rush_gate::godur::string(ran)
        );
    }
    b += "You have the context to judge them: is each still doing something useful, or waiting on something that won't happen? Have a look and decide what to do, then carry on. If they're fine, say so in a line and keep going.";
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ago(now: Time, m: u64) -> Time {
        now.add_nanos(-((m * 60) as i64) * 1_000_000_000)
    }

    fn task(id: &str, label: &str, kind: &str, at: Time) -> Task {
        Task {
            id: id.into(),
            label: label.into(),
            kind: kind.into(),
            started_at: at,
            ..Default::default()
        }
    }

    // longtask_test.go:27
    #[test]
    fn overdue() {
        let now = Time::now();
        let tasks = [
            task("a", "wait for queue 40", "monitor", ago(now, 150)),
            task("b", "fresh", "", ago(now, 5)),
            task("c", "asked lately", "", ago(now, 180)),
        ];
        // a is due, and c, long too, comes in the same check-in.
        let due = overdue_tasks(&tasks, &HashMap::from([("c".into(), ago(now, 10))]), now);
        let ids: Vec<&str> = due.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        let n = long_task_note(&due, now);
        assert!(
            n.contains("\"wait for queue 40\"") && n.contains("2h30m"),
            "{n}"
        );
        let c = [tasks[2].clone()];
        assert!(overdue_tasks(&c, &HashMap::from([("c".into(), ago(now, 120))]), now).is_empty());
        assert_eq!(
            overdue_tasks(&c, &HashMap::from([("c".into(), ago(now, 300))]), now).len(),
            1
        );
    }

    // longtask_test.go:50
    #[test]
    fn overdue_skips_subagents() {
        let now = Time::now();
        let tasks = [
            task("a", "", "local_agent", ago(now, 40)),
            task("w", "", "local_workflow", ago(now, 40)),
        ];
        assert!(overdue_tasks(&tasks, &HashMap::new(), now).is_empty());
    }

    // longtask_test.go:61
    #[test]
    fn one_check_in_an_hour() {
        let now = Time::now();
        let tasks = [
            task("a", "", "local_bash", ago(now, 31)),
            task("b", "", "local_bash", ago(now, 30)),
        ];
        // b crosses 30m a minute after a was asked about: it waits.
        let nudged = HashMap::from([
            ("a".to_string(), ago(now, 1)),
            (LONG_TASK_LAST.to_string(), ago(now, 1)),
        ]);
        assert!(overdue_tasks(&tasks, &nudged, now).is_empty());
        // An hour on, both in one.
        let later = now + Duration::from_secs(3600);
        assert_eq!(overdue_tasks(&tasks, &nudged, later).len(), 2);
        // Nothing new to ask about: none.
        let asked = HashMap::from([("a".to_string(), now), ("b".to_string(), now)]);
        assert!(overdue_tasks(&tasks, &asked, now + Duration::from_secs(60)).is_empty());
    }
}
