//! A background task can be left running long after it's any use: a
//! monitor waiting on what never comes, a loop polling a queue that moved
//! on. Rush never stops one; it asks the agent to look, since it knows
//! what each was for. Subagents and workflows aren't asked about: they're
//! working, rush shows their steps, and the agent hears when they end. A
//! task is judged by its output, not its age: one still writing is getting
//! on with it, and only one quiet a long time is asked about.
//!
//! Ported from internal/host/longtask.go:15-260 (the server's share is in
//! server/agent.rs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_state::Time;

use crate::stuck::{agent_command, short};
use crate::wire::Task;

/// A background task quiet this long is asked about.
pub const LONG_TASK_AFTER: Duration = Duration::from_secs(30 * 60);
/// And a session is asked at most this often.
pub const LONG_TASK_AGAIN: Duration = Duration::from_secs(3600);
/// One running this long is asked about however much it writes: a loop
/// polling for what never comes can print all the while.
pub const LONG_TASK_ANYWAY: Duration = Duration::from_secs(4 * 3600);
/// A task asked about once is asked about again only this long after:
/// most were fine (a server kept up, a long build), and said so.
pub const LONG_TASK_REASK: Duration = Duration::from_secs(4 * 3600);

/// The key in nudged for when the session was last asked.
pub const LONG_TASK_LAST: &str = "";

fn since(now: Time, t: Time) -> Duration {
    Duration::from_nanos(now.sub_nanos(t).max(0) as u64)
}

/// The shells and monitors quiet past LONG_TASK_AFTER (or running past
/// LONG_TASK_ANYWAY) not asked about in the last LONG_TASK_REASK. wrote is
/// when each last wrote output, where that's known. Once one is due, so is
/// every other quiet that long, so they're asked about in one check-in,
/// not one a minute apart; and none is asked about within LONG_TASK_AGAIN
/// of the last check-in.
pub fn overdue_tasks(
    tasks: &[Task],
    wrote: &HashMap<String, Time>,
    nudged: &HashMap<String, Time>,
    now: Time,
) -> Vec<Task> {
    if let Some(&at) = nudged.get(LONG_TASK_LAST)
        && since(now, at) < LONG_TASK_AGAIN
    {
        return Vec::new();
    }
    let mut long = Vec::new();
    let mut fresh = false;
    for t in tasks {
        if t.started_at.is_zero() || subagent_task(&t.kind) {
            continue;
        }
        if since(now, quiet_since(t, wrote)) < LONG_TASK_AFTER
            && since(now, t.started_at) < LONG_TASK_ANYWAY
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

/// When t last showed it was doing something: its last output, or its
/// start if it has written none since.
pub fn quiet_since(t: &Task, wrote: &HashMap<String, Time>) -> Time {
    match wrote.get(&t.id) {
        Some(&w) if w.after(t.started_at) => w,
        _ => t.started_at,
    }
}

/// The folders Claude Code keeps session sid's background task output in,
/// under the session's own temp folder tmp: one for each project folder
/// it has worked in (tmp/claude-*/*/sid/tasks).
pub fn task_dirs(tmp: &Path, sid: &str) -> Vec<PathBuf> {
    if sid.is_empty() {
        return Vec::new();
    }
    let subdirs = |d: &Path| -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(d) else {
            return Vec::new();
        };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        v
    };
    let mut found = Vec::new();
    for c in subdirs(tmp) {
        if !c
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("claude-"))
        {
            continue;
        }
        for p in subdirs(&c) {
            let d = p.join(sid).join("tasks");
            if d.exists() {
                found.push(d);
            }
        }
    }
    found
}

/// When each task last wrote to its output file in dirs.
pub fn last_output(dirs: &[PathBuf], ids: &[String]) -> HashMap<String, Time> {
    let mut wrote: HashMap<String, Time> = HashMap::new();
    for dir in dirs {
        for id in ids {
            let Ok(m) = std::fs::metadata(dir.join(format!("{id}.output"))) else {
                continue;
            };
            let Some(at) = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| Time::from_unix(d.as_secs() as i64, d.subsec_nanos()))
            else {
                continue;
            };
            if wrote.get(id).is_none_or(|&w| at.after(w)) {
                wrote.insert(id.clone(), at);
            }
        }
    }
    wrote
}

/// Whether a task of this type is an agent doing work.
pub fn subagent_task(typ: &str) -> bool {
    matches!(
        typ,
        "local_agent" | "remote_agent" | "in_process_teammate" | "local_workflow"
    )
}

/// d rounded to the minute, as Go's Round(time.Minute).
fn to_minute(d: Duration) -> Duration {
    Duration::from_secs((d.as_secs() + 30) / 60 * 60)
}

/// What the agent is told about them.
pub fn long_task_note(due: &[Task], wrote: &HashMap<String, Time>, now: Time) -> String {
    let mut b = String::from(
        "Rush check-in: these background tasks have gone quiet, or been running a long time:\n",
    );
    for t in due {
        let ran = since(now, t.started_at);
        let ran = to_minute(ran);
        let label = if t.label.is_empty() { &t.id } else { &t.label };
        b += &format!(
            "- {} {} ({}), running {}",
            if t.kind.is_empty() { "task" } else { &t.kind },
            crate::reload::go_quote(&short(&agent_command(label))),
            t.id,
            rush_gate::godur::string(ran)
        );
        let q = quiet_since(t, wrote);
        if q.after(t.started_at) {
            b += &format!(
                ", last wrote {} ago",
                rush_gate::godur::string(to_minute(since(now, q)))
            );
        } else if wrote.contains_key(&t.id) {
            b += ", nothing written yet";
        }
        b += "\n";
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
        let (now, none) = (Time::now(), HashMap::new());
        let tasks = [
            task("a", "wait for queue 40", "monitor", ago(now, 150)),
            task("b", "fresh", "", ago(now, 5)),
            task("c", "asked lately", "", ago(now, 180)),
        ];
        // a is due, and c, long too, comes in the same check-in.
        let due = overdue_tasks(
            &tasks,
            &none,
            &HashMap::from([("c".into(), ago(now, 10))]),
            now,
        );
        let ids: Vec<&str> = due.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        let n = long_task_note(&due, &none, now);
        assert!(
            n.contains("\"wait for queue 40\"") && n.contains("2h30m"),
            "{n}"
        );
        let c = [tasks[2].clone()];
        assert!(
            overdue_tasks(
                &c,
                &none,
                &HashMap::from([("c".into(), ago(now, 120))]),
                now
            )
            .is_empty()
        );
        assert_eq!(
            overdue_tasks(
                &c,
                &none,
                &HashMap::from([("c".into(), ago(now, 300))]),
                now
            )
            .len(),
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
        assert!(overdue_tasks(&tasks, &HashMap::new(), &HashMap::new(), now).is_empty());
    }

    // longtask_test.go:61
    #[test]
    fn one_check_in_an_hour() {
        let (now, none) = (Time::now(), HashMap::new());
        let tasks = [
            task("a", "", "local_bash", ago(now, 31)),
            task("b", "", "local_bash", ago(now, 30)),
        ];
        // b crosses 30m a minute after a was asked about: it waits.
        let nudged = HashMap::from([
            ("a".to_string(), ago(now, 1)),
            (LONG_TASK_LAST.to_string(), ago(now, 1)),
        ]);
        assert!(overdue_tasks(&tasks, &none, &nudged, now).is_empty());
        // An hour on, both in one.
        let later = now + Duration::from_secs(3600);
        assert_eq!(overdue_tasks(&tasks, &none, &nudged, later).len(), 2);
        // Nothing new to ask about: none.
        let asked = HashMap::from([("a".to_string(), now), ("b".to_string(), now)]);
        assert!(overdue_tasks(&tasks, &none, &asked, now + Duration::from_secs(60)).is_empty());
    }

    // longtask_test.go:85 (TestOverdueTasksJudgedByOutput)
    #[test]
    fn judged_by_output() {
        let now = Time::now();
        let mut tasks = [
            task("busy", "", "local_bash", ago(now, 50)),
            task("quiet", "", "local_bash", ago(now, 50)),
        ];
        let wrote = HashMap::from([
            ("busy".to_string(), ago(now, 2)),
            ("quiet".to_string(), ago(now, 40)),
        ]);
        let due = overdue_tasks(&tasks, &wrote, &HashMap::new(), now);
        assert!(
            due.len() == 1 && due[0].id == "quiet",
            "only the quiet one: {due:?}"
        );
        let n = long_task_note(&due, &wrote, now);
        assert!(n.contains("last wrote 40m0s ago"), "note: {n}");
        // Still writing past LONG_TASK_ANYWAY, it's asked about anyway.
        tasks[0].started_at = ago(now, 300);
        assert_eq!(
            overdue_tasks(&tasks[..1], &wrote, &HashMap::new(), now).len(),
            1,
            "a chatty task running 5h"
        );
    }

    // longtask_test.go:105 (TestLastOutput)
    #[test]
    fn last_output_reads_the_output_files() {
        let (dir, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let f = dir.path().join("b1.output");
        std::fs::write(&f, "x").unwrap();
        let at = std::time::SystemTime::now() - Duration::from_secs(3600);
        let at = std::time::UNIX_EPOCH
            + Duration::from_secs(at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(at)
            .unwrap();
        let got = last_output(
            &[dir.path().to_path_buf(), other.path().to_path_buf()],
            &["b1".to_string(), "gone".to_string()],
        );
        let want = Time::from_unix(
            at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
            0,
        );
        assert!(got.len() == 1 && got["b1"] == want, "last_output: {got:?}");
    }

    #[test]
    fn task_dirs_under_the_sessions_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp
            .path()
            .join("claude-501")
            .join("-proj")
            .join("sid1")
            .join("tasks");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(
            tmp.path()
                .join("other")
                .join("x")
                .join("sid1")
                .join("tasks"),
        )
        .unwrap();
        assert_eq!(task_dirs(tmp.path(), "sid1"), vec![d]);
        assert!(task_dirs(tmp.path(), "").is_empty());
    }
}
