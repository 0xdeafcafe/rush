//! What an agent runs besides talking: shells, monitors and workflows,
//! in the background or with the turn waiting on them, as the session's
//! dock shows them: a mark and kind each, what it runs, how it stands,
//! and its latest line of output. After Go's internal/ui/jobs.go
//! (dockJobs :33, jobIcon :46, jobLabel :59, jobHint :156, jobRow :168,
//! jobState :259, crashRe :307, jobCrashed :318, jobsPreview :348,
//! tailLines :1193, lastLines :1255, firstWord :1266, tailFollows :1098),
//! job_activity.go:12 (jobActivity) and rushmode.go:2175 (spread). A
//! task's output is read off the UI thread with `read_tail` and kept as
//! a `Tailed`; a frame draws what was last read. Not here yet: which
//! subagent started a task (runs aren't linked in Rust), a shell's CPU
//! and memory (the process table), plugins' task cards, and the
//! background view's opened rows (the session group's view).

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use rush_convo::text::drop_cd;
use rush_convo::{Job, Session};
use rush_core::text::{Color, Line, width};
use rush_core::time::Ms;

use crate::style::*;

/// How many running tasks the dock shows at once (dockJobsShown).
pub const DOCK_JOBS_SHOWN: usize = 4;
/// The most of a task's output shown: an opened one's (jobTailMost).
pub const JOB_TAIL_MOST: usize = 30;
/// How often a running task's output is looked at again (tailEvery).
pub const TAIL_EVERY: Ms = 500;
/// The most of a line of output kept, in bytes (tailWidest).
pub const TAIL_WIDEST: usize = 1024;
/// How long output must have stayed on a crash before it's called one:
/// a program may be restarting (crashQuiet).
pub const CRASH_QUIET: Ms = 20_000;
const MINUTE: Ms = 60_000;

/// A line a program writes as it dies, or as it gives up and waits
/// (crashRe).
static CRASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^panic: |Traceback \(most recent call last\)|\bFATAL\b|EADDRINUSE|address already in use|app crashed|exited with code [1-9]|Segmentation fault|npm ERR!|ELIFECYCLE|Cannot find module|command not found|^error(\[E\d+\])?: |^Killed$|^fatal error: ",
    )
    .expect("crash pattern")
});

/// A task's output file as last read (jobTailed): its size (-1 when
/// nothing's there), when it last changed, and its last lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tailed {
    pub size: i64,
    pub mod_at: Ms,
    pub lines: Vec<String>,
}

/// Reads the file at p's last lines when it isn't the size and time it
/// was; None when it's unchanged (readTail). Blocks on the disk: run it
/// with Model::later, never on the UI thread.
pub fn read_tail(p: &str, was: Option<&Tailed>) -> Option<Tailed> {
    let Ok(st) = std::fs::metadata(p) else {
        return Some(Tailed {
            size: -1,
            ..Tailed::default()
        });
    };
    let mod_at = st
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as Ms);
    if was.is_some_and(|w| w.size == st.len() as i64 && w.mod_at == mod_at) {
        return None;
    }
    Some(Tailed {
        size: st.len() as i64,
        mod_at,
        lines: tail_lines(Path::new(p), JOB_TAIL_MOST),
    })
}

/// s without its terminal escapes (ansi.Strip): CSI and OSC sequences.
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && it.peek() == Some(&'\\') {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// One line of output as a row has it: a progress bar's latest state,
/// no escapes or trailing blanks, tabs as two spaces.
fn clean(l: &str) -> String {
    let l = l.trim_end_matches('\r');
    let l = match l.rfind('\r') {
        Some(i) => &l[i + 1..],
        None => l,
    };
    let mut l = strip_ansi(l)
        .trim_end_matches([' ', '\t', '\r'])
        .to_string();
    if l.len() > TAIL_WIDEST {
        let mut cut = TAIL_WIDEST;
        while !l.is_char_boundary(cut) {
            cut -= 1;
        }
        l.truncate(cut);
    }
    l.replace('\t', "  ")
}

/// The last n non-blank lines of a file, from its last 16 KB (tailLines).
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    const MOST: u64 = 16 << 10;
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let size = f.metadata().map_or(0, |m| m.len());
    if size == 0 {
        return Vec::new();
    }
    let off = size.saturating_sub(MOST);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(off)).is_err() || f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut out: Vec<String> = Vec::new();
    for (k, l) in text.split('\n').enumerate() {
        if off > 0 && k == 0 {
            continue; // cut mid-line
        }
        let l = clean(l);
        if !l.is_empty() {
            out.push(l);
        }
    }
    let from = out.len().saturating_sub(n);
    out.split_off(from)
}

/// The last n non-blank lines of s (lastLines).
pub fn last_lines(s: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = strip_ansi(s)
        .split('\n')
        .map(|l| l.trim_end_matches([' ', '\t', '\r']).replace('\t', "  "))
        .filter(|l| !l.is_empty())
        .collect();
    let from = out.len().saturating_sub(n);
    out.split_off(from)
}

/// A command's program (firstWord).
pub fn first_word(cmd: &str) -> &str {
    cmd.split_whitespace()
        .find(|f| !f.contains('='))
        .unwrap_or(cmd)
}

/// The files a tail -f or -F command follows, made absolute from dir;
/// none for any other command (tailFollows).
pub fn tail_follows(cmd: &str, dir: &str) -> Vec<String> {
    let w: Vec<&str> = cmd.split_whitespace().collect();
    if w.len() < 2 || w[0] != "tail" {
        return Vec::new();
    }
    let mut follow = false;
    let mut files = Vec::new();
    let mut i = 1;
    while i < w.len() {
        let a = w[i];
        if a == "-n" || a == "-c" {
            i += 1; // its count
        } else if let Some(flags) = a.strip_prefix('-') {
            follow = follow
                || flags.contains(['f', 'F']) && !a.starts_with("--")
                || a == "--follow"
                || a.starts_with("--follow=");
        } else if a.starts_with('/') {
            files.push(clean_path(a));
        } else if !dir.is_empty() {
            files.push(clean_path(&format!("{}/{a}", dir.trim_end_matches('/'))));
        }
        i += 1;
    }
    if follow { files } else { Vec::new() }
}

/// p with its . and .. worked out, as filepath.Clean does.
fn clean_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// left, then right at the end of w cells, two apart at least; left alone,
/// fit, when they don't both fit (spread).
pub fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut out = left;
    out.spans.push(plain(blanks(gap as usize)));
    join(&mut out, right);
    out
}

/// A row picked: the bar at its left, orange when the pane has the keys,
/// on the selection's ground (picked1).
pub fn picked(l: Line, w: usize, focused: bool) -> Line {
    let bar = sp("▍", if focused { ORANGE } else { FAINT });
    let body = rush_draw::truncate_line(&fit(l, w.saturating_sub(1)), w, "");
    let mut out = line([bar]);
    // The row's first cell is the bar's: what's under it goes.
    let rest = cut_first(body);
    join(&mut out, rest);
    highlight(out, w)
}

/// l without its first cell.
fn cut_first(mut l: Line) -> Line {
    if let Some(s) = l.spans.iter_mut().find(|s| !s.text.is_empty()) {
        let mut cs = s.text.chars();
        cs.next();
        s.text = cs.as_str().to_string();
    }
    l
}

/// The tasks the dock shows: what runs in the background, but subagents,
/// which have their own block; what the turn waits on shows in the
/// conversation (dockJobs).
pub fn dock_jobs(s: &Session) -> Vec<&Job> {
    s.running_jobs()
        .filter(|j| j.background && s.job_kind(j) != "subagent")
        .collect()
}

/// A task's mark and colour, by what it is (jobIcon).
pub fn job_icon(kind: &str) -> (&'static str, Color) {
    match kind {
        "monitor" => ("◎", BLUE),
        "workflow" => ("⧉", QUEUE),
        "subagent" => ("⇉", BLUE),
        _ => ("$", SUB),
    }
}

/// A task by its description, else its command; a subagent's type first
/// (jobLabel).
pub fn job_label(s: &Session, j: &Job) -> String {
    let mut l = j.label.clone();
    if l.is_empty() {
        l = s.job_command(j);
    }
    if !j.agent.is_empty() {
        l = format!("{}  {l}", j.agent);
    }
    one_line(&tildify(drop_cd(&l)))
}

/// How a task stands, in a word or two, and how long it took (jobState).
pub fn job_state(j: &Job, now: Ms) -> Line {
    let end = if j.running() { now } else { j.end };
    let took = if j.start != 0 {
        format!(" · {}", dur(((end - j.start + 500) / 1000 * 1000).max(0)))
    } else {
        String::new()
    };
    if j.running() && j.background {
        return line([sp(format!("in background{took}"), DIM)]);
    }
    let (word, c) = match (j.running(), j.status.as_str()) {
        (true, _) => ("waiting on it".to_string(), ORANGE),
        (_, "completed") => ("✓ done".to_string(), GREEN),
        (_, "failed") => ("✗ failed".to_string(), RED),
        (_, st) => (format!("⏹ {st}"), DIM),
    };
    line([sp(word, c), sp(took, DIM)])
}

/// Whether a running task's output ends on a crash and has stayed that
/// way a while: running as far as the agent knows, but likely dead
/// (jobCrashed).
pub fn job_crashed(j: &Job, out: Option<&Tailed>, now: Ms) -> bool {
    let Some(t) = out else { return false };
    if !j.running() || t.mod_at == 0 || now - t.mod_at < CRASH_QUIET {
        return false;
    }
    let from = t.lines.len().saturating_sub(3);
    t.lines[from..].iter().any(|l| CRASH.is_match(l.trim()))
}

/// What ends a task's row: a crash called out, else how it stands
/// (jobRight).
pub fn job_right(j: &Job, out: Option<&Tailed>, now: Ms) -> Line {
    let mut l = Line::new();
    if job_crashed(j, out, now) {
        l.spans.push(sp("⚠ error in recent output", RED));
        l.spans.push(sp("  ·  ", DIM));
    }
    join(&mut l, job_state(j, now));
    l.spans.push(plain("  "));
    l
}

/// What a task's output says of it, not a guess at its process: a quiet
/// one may be healthy (jobActivity). Empty once it's done.
pub fn job_activity(j: &Job, out: Option<&Tailed>, now: Ms) -> Line {
    if !j.running() {
        return Line::new();
    }
    if let Some(t) = out.filter(|t| t.mod_at != 0 && t.size > 0) {
        let quiet = (now - t.mod_at).max(0);
        if quiet >= MINUTE {
            return line([
                sp(format!("quiet for {}", age(quiet)), YELLOW),
                sp(" · may be waiting; silence does not mean stuck", SUB),
            ]);
        }
        return line([
            sp("output active", GREEN),
            sp(format!(" · updated {} ago", age(quiet)), SUB),
        ]);
    }
    let s = if j.progress_at != 0 {
        format!(
            "progress updated {} ago · no captured output",
            age((now - j.progress_at).max(0))
        )
    } else if j.start != 0 {
        format!(
            "no captured output · running for {}",
            age((now - j.start).max(0))
        )
    } else {
        "no captured output · activity unknown".to_string()
    };
    line([sp(s, SUB)])
}

/// What the keys do on a picked task (jobHint). part is the command of
/// its chain running now, when it's a shell the turn waits on; pause is
/// what p does to it (shells.go pauseWord).
pub fn job_hint(j: &Job, part: Option<(&str, &str)>) -> Line {
    if j.background {
        return keys(&[
            ("x", "stop"),
            ("shift+x", "…and say why"),
            ("space", "output"),
            ("d", "command details"),
        ]);
    }
    if let Some((cmd, pause)) = part {
        let kill = format!("kill {}", first_word(cmd));
        return keys(&[
            ("k", &kill),
            ("p", pause),
            ("b", "background"),
            ("x", "stop"),
            ("shift+x", "…and say why"),
        ]);
    }
    keys(&[
        ("b", "background"),
        ("x", "stop"),
        ("shift+x", "…and say why"),
        ("space", "output"),
        ("d", "command details"),
    ])
}

/// How the dock draws: the task picked (its ref, "job:<id>"), whether
/// the pane has the keys, the spinner's step, and each task's output as
/// last read, by task id.
pub struct Dock<'a> {
    pub sel: &'a str,
    pub focus: bool,
    pub tick: usize,
    pub now: Ms,
    pub out: &'a dyn Fn(&Job) -> Option<&'a Tailed>,
}

/// A dock task's row (jobRow): lead, its mark and kind, what it runs, its
/// latest line of output, and how it stands at the end.
pub fn job_row(s: &Session, j: &Job, i: usize, w: usize, lead: &str, d: &Dock) -> Line {
    let kind = s.job_kind(j);
    let (icon, col) = job_icon(kind);
    let mark = if j.background {
        sp(icon, col)
    } else {
        sp(SPINNER[(d.tick + i) % SPINNER.len()], ORANGE)
    };
    let out = (d.out)(j);
    let right = job_right(j, out, d.now);
    let room = (w as isize - right.width() as isize - 12 - width(lead) as isize).max(12) as usize;
    let mut left = line([
        plain(lead),
        mark,
        plain(" "),
        spb(format!("{kind:<7}"), TEXT),
        plain(" "),
        sp(
            rush_draw::truncate(&job_label(s, j), room.min(40), "…").into_owned(),
            SUB,
        ),
    ]);
    // Its latest line of output, on the same row, as far as there's room.
    if let Some(last) = out.and_then(|t| t.lines.last()) {
        left.spans.push(plain("  "));
        left.spans.push(sp("›", ORANGE));
        left.spans.push(plain(" "));
        left.spans.push(sp(last.clone(), DIM));
    }
    let keep = (w as isize - right.width() as isize - 2).max(20) as usize;
    let row = spread(rush_draw::truncate_line(&left, keep, "…"), right, w);
    if d.sel == format!("job:{}", j.id) {
        return picked(row, w, d.focus);
    }
    row
}

/// The dock's block of running tasks: a heading with what the keys do,
/// then a row each, at most DOCK_JOBS_SHOWN, kept round the one picked
/// (jobsPreview). hint is what the heading says when none is picked
/// (the pane's ↑ to pick).
pub fn jobs_preview(s: &Session, jobs: &[&Job], w: usize, d: &Dock, hint: Line) -> Vec<Line> {
    let picked_at = jobs.iter().position(|j| d.sel == format!("job:{}", j.id));
    let fg = jobs.iter().filter(|j| !j.background).count();
    let bg = jobs.len() - fg;
    let head = if fg == 0 {
        "in the background".to_string()
    } else if bg > 0 {
        format!("running · {bg} in the background")
    } else {
        "running".to_string()
    };
    let mut hint = match picked_at {
        Some(i) => job_hint(jobs[i], None),
        None => hint,
    };
    hint.spans.push(plain("  "));
    let title = line([
        plain(" "),
        spb("shell", ORANGE),
        sp(format!(" ·{}", jobs.len()), FAINT),
        sp(format!("  {head}"), DIM),
    ]);
    let mut out = vec![spread(title, hint, w)];
    let start = match picked_at {
        Some(p) if p >= DOCK_JOBS_SHOWN => p - DOCK_JOBS_SHOWN + 1,
        _ => 0,
    };
    if start > 0 {
        out.push(line([sp(format!("  … {start} before"), DIM)]));
    }
    let end = jobs.len().min(start + DOCK_JOBS_SHOWN);
    for (i, j) in jobs.iter().enumerate().take(end).skip(start) {
        out.push(job_row(s, j, i, w, "  ", d));
    }
    if jobs.len() > start + DOCK_JOBS_SHOWN {
        let rest = jobs.len() - start - DOCK_JOBS_SHOWN;
        out.push(line([sp(
            format!("  + {rest} more in the background view"),
            DIM,
        )]));
    }
    out
}
