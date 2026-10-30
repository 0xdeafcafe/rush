//! A shell call rush ran as a script, as the conversation shows it: the
//! line it's on and since when, how long each line took, where it failed,
//! its breakpoints and whether one holds it. Go: internal/ui/scripts.go
//! (scriptEvery :51, scriptView :128, toggleScriptBreak :160, scriptLine
//! :194, goOnFromBreak :203's held). Reading the trace (rush_sys::script)
//! is a worker's; the scripts each session keeps, read as it runs, and p
//! letting one held at a breakpoint go on, are session_core/scripts.rs.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use rush_sys::script::Run;

/// How often a running script's trace is read again.
pub const SCRIPT_EVERY: Duration = Duration::from_millis(300);
/// How many of the latest turns' calls are looked for as scripts: further
/// back, they're folded anyway.
pub const SCRIPT_TURNS: usize = 3;

/// How the call a script runs for stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallIs {
    Running,
    Done,
    Failed,
}

/// What the conversation shows of a script's run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptView {
    /// The line it's on, and since when; 0 when it isn't running.
    pub at: i32,
    pub since: Option<SystemTime>,
    /// How long each line took, by line.
    pub took: HashMap<i32, Duration>,
    /// The line it failed on, 0 for none.
    pub failed_at: i32,
    pub breaks: HashSet<i32>,
    /// A breakpoint holds it at `at`.
    pub held: bool,
}

/// What the conversation shows of run r of a call that is as status,
/// ended at end (None while it runs or when unknown).
pub fn script_view(r: &Run, status: CallIs, end: Option<SystemTime>) -> ScriptView {
    let mut v = ScriptView::default();
    let running = status == CallIs::Running;
    if running {
        // A finished script's breakpoints stop nothing.
        v.breaks.extend(r.breaks.iter().copied());
    }
    for (i, h) in r.hits.iter().enumerate() {
        let end = match r.hits.get(i + 1) {
            Some(next) => Some(next.at),
            None if running || end.is_none() => {
                (v.at, v.since) = (h.line, Some(h.at));
                continue;
            }
            None => end,
        };
        if let Some(d) = end.and_then(|e| e.duration_since(h.at).ok())
            && !d.is_zero()
        {
            *v.took.entry(h.line).or_default() += d;
        }
    }
    if !running
        && status == CallIs::Failed
        && let Some(last) = r.hits.last()
    {
        v.failed_at = last.line;
    }
    v.held = running && r.held_at != 0;
    if v.held {
        v.at = r.held_at;
    }
    v
}

/// The line run r is on, 0 before it begins.
pub fn script_line(r: &Run) -> i32 {
    r.hits.last().map_or(0, |h| h.line)
}

/// Why a breakpoint can't be set at line n of run r, as the flash says it
/// (live: the call still runs), or None when it can be toggled.
pub fn break_refused(r: &Run, live: bool, n: i32) -> Option<String> {
    if !live {
        return Some("the script has finished · a breakpoint only stops one still running".into());
    }
    let at = script_line(r);
    if at >= n && !r.breaks.contains(&n) {
        return Some(format!("line {n} has already run · pick a line below {at}"));
    }
    None
}

/// What the flash says once a breakpoint is toggled at line n.
pub fn break_flash(n: i32, on: bool) -> String {
    if on {
        format!("breakpoint at line {n} · the script stops when it gets there")
    } else {
        format!("breakpoint at line {n} taken away")
    }
}

/// The script held at a breakpoint, to let go on (its pid and line), or
/// None when nothing holds it.
pub fn held(r: &Run) -> Option<(i32, i32)> {
    (r.held_at != 0 && r.pid != 0).then_some((r.pid, r.held_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_sys::script::Hit;
    use std::time::UNIX_EPOCH;

    fn at(s: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(s)
    }

    fn run(hits: &[(i32, u64)], breaks: &[i32], held_at: i32) -> Run {
        Run {
            pid: 42,
            start: Some(at(0)),
            hits: hits
                .iter()
                .map(|&(line, s)| Hit { line, at: at(s) })
                .collect(),
            breaks: breaks.to_vec(),
            held_at,
            source: String::new(),
            changed: None,
        }
    }

    #[test]
    fn a_running_script() {
        let r = run(&[(1, 10), (2, 13), (1, 15), (3, 20)], &[5], 0);
        let v = script_view(&r, CallIs::Running, None);
        assert_eq!((v.at, v.since), (3, Some(at(20))));
        assert_eq!(v.took[&1], Duration::from_secs(8));
        assert_eq!(v.took[&2], Duration::from_secs(2));
        assert!(v.breaks.contains(&5));
        assert!(!v.held);
        let v = script_view(&run(&[(1, 10)], &[2], 2), CallIs::Running, None);
        assert!(v.held);
        assert_eq!(v.at, 2);
    }

    #[test]
    fn a_finished_script() {
        let r = run(&[(1, 10), (2, 13)], &[5], 2);
        let v = script_view(&r, CallIs::Failed, Some(at(19)));
        assert_eq!(v.at, 0);
        assert_eq!(v.took[&2], Duration::from_secs(6));
        assert_eq!(v.failed_at, 2);
        assert!(v.breaks.is_empty() && !v.held);
        let v = script_view(&r, CallIs::Done, None);
        assert_eq!(v.at, 2, "no end known: where it was last");
        assert_eq!(v.failed_at, 0);
    }

    #[test]
    fn breakpoints() {
        let r = run(&[(1, 10), (4, 12)], &[3], 0);
        assert_eq!(script_line(&r), 4);
        assert_eq!(script_line(&run(&[], &[], 0)), 0);
        assert_eq!(
            break_refused(&r, false, 9).unwrap(),
            "the script has finished · a breakpoint only stops one still running"
        );
        assert_eq!(
            break_refused(&r, true, 2).unwrap(),
            "line 2 has already run · pick a line below 4"
        );
        assert_eq!(
            break_refused(&r, true, 3),
            None,
            "one set can be taken away"
        );
        assert_eq!(break_refused(&r, true, 7), None);
        assert_eq!(
            break_flash(7, true),
            "breakpoint at line 7 · the script stops when it gets there"
        );
        assert_eq!(break_flash(7, false), "breakpoint at line 7 taken away");
        assert_eq!(held(&run(&[], &[], 3)), Some((42, 3)));
        assert_eq!(held(&r), None);
    }
}
