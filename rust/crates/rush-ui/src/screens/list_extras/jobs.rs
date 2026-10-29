//! What an agent runs besides talking: shells, monitors and workflows,
//! in the background or with the turn waiting on them, as the session's
//! dock shows them: a mark and kind each, what it runs, how it stands,
//! and its latest line of output (a workflow's with the subagents, as of
//! bb17d8d2). After Go's internal/ui/jobs.go
//! (dockJobs :33, jobIcon :46, jobLabel :59, jobHint :156, jobRow :168,
//! jobState :259, crashRe :307, jobCrashed :318, jobsPreview :348,
//! tailLines :1193, lastLines :1255, firstWord :1266, tailFollows :1098),
//! job_activity.go:12 (jobActivity) and rushmode.go:2175 (spread). A
//! task's output is read off the UI thread with `read_tail` and kept as
//! a `Tailed`; a frame draws what was last read. Whose a task is comes
//! from the Session's runs (session_core::running's `Dock::who`). The
//! background view is `job_lines` (jobLines :416-671, tailWhen :1128):
//! every task but subagents, a workflow's phases under its row (no output
//! after them: it's its progress again, as JSON, 13d7ab1d), an opened
//! task's edge beside its rows and its workflow agents' (c3a92cd8); a
//! shell's CPU and memory and the processes under an opened task
//! (jobUsage :347, jobLines :580-597, read by session_core::jobprocs), the
//! chain's command running now (RunningPart, :543), a plugin's task card
//! (cardRow :212, segs :244, toneColour :253, jobLines :527-541), output
//! read from the files a command writes when its own has none
//! (jobTailFrom :817, liveFor :997, fileRows :1059, the other files an
//! opened task writes :625-644, jobFileLines :1165) and a finished task's
//! rows kept until what they're drawn from changes (jobRowsKey :1014,
//! jobRowsMemo :1025).

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use std::sync::Mutex;

use regex::Regex;
use rush_bundled::prwatch::{JobCard, Seg, Tone};
use rush_convo::text::drop_cd;
use rush_convo::{Job, RunningPart, Session};
use rush_core::text::{Color, Line, Span, width};
use rush_core::time::Ms;

use crate::screens::session_core::jobprocs::ProcRow;
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
/// How much of each further file an opened task writes is shown
/// (jobFileLines).
pub const JOB_FILE_LINES: usize = 10;
/// How recently a file a running task writes must have changed to stand
/// for its output (liveFor).
pub const LIVE_FOR: Ms = 30_000;
/// The most of a file before its tail that's read to number the tail's
/// lines; past it they go unnumbered (tailCounted).
const TAIL_COUNTED: u64 = 32 << 20;
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
    /// Each line's number in the file, when it was counted.
    pub nos: Vec<usize>,
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
    let (lines, nos) = tail_lines_numbered(Path::new(p), JOB_TAIL_MOST);
    Some(Tailed {
        size: st.len() as i64,
        mod_at,
        lines,
        nos,
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
    tail_lines_numbered(path, n).0
}

/// tail_lines and each line's number in the file; no numbers for a file
/// too big to count (tailLines).
pub fn tail_lines_numbered(path: &Path, n: usize) -> (Vec<String>, Vec<usize>) {
    use std::io::{Read, Seek, SeekFrom};
    const MOST: u64 = 16 << 10;
    let Ok(mut f) = std::fs::File::open(path) else {
        return (Vec::new(), Vec::new());
    };
    let size = f.metadata().map_or(0, |m| m.len());
    if size == 0 {
        return (Vec::new(), Vec::new());
    }
    let off = size.saturating_sub(MOST);
    // The number of the buffer's first line, when what's before it isn't
    // too much to count.
    let first = (off <= TAIL_COUNTED).then(|| 1 + newlines(&mut f, off));
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(off)).is_err() || f.read_to_end(&mut buf).is_err() {
        return (Vec::new(), Vec::new());
    }
    let text = String::from_utf8_lossy(&buf);
    let mut out: Vec<String> = Vec::new();
    let mut nos: Vec<usize> = Vec::new();
    for (k, l) in text.split('\n').enumerate() {
        if off > 0 && k == 0 {
            continue; // cut mid-line
        }
        let l = clean(l);
        if !l.is_empty() {
            out.push(l);
            nos.push(first.unwrap_or(0) + k);
        }
    }
    let from = out.len().saturating_sub(n);
    if first.is_none() {
        nos.clear();
    } else {
        nos = nos.split_off(from);
    }
    (out.split_off(from), nos)
}

/// The line ends in a file's first n bytes (newlines).
fn newlines(f: &mut std::fs::File, n: u64) -> usize {
    use std::io::{Read, Seek, SeekFrom};
    if n == 0 || f.seek(SeekFrom::Start(0)).is_err() {
        return 0;
    }
    let mut buf = vec![0u8; 64 << 10];
    let (mut left, mut count) = (n, 0);
    while left > 0 {
        let want = (left as usize).min(buf.len());
        let Ok(k) = f.read(&mut buf[..want]) else {
            break;
        };
        if k == 0 {
            break;
        }
        count += buf[..k].iter().filter(|&&b| b == b'\n').count();
        left -= k as u64;
    }
    count
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

/// The tasks the dock shows: what runs in the background, but subagents
/// and workflows, which have their own block; what the turn waits on
/// shows in the conversation (dockJobs).
pub fn dock_jobs(s: &Session) -> Vec<&Job> {
    s.running_jobs()
        .filter(|j| j.background && !matches!(s.job_kind(j), "subagent" | "workflow"))
        .collect()
}

/// A task's mark and colour, by what it is (jobIcon).
pub fn job_icon(kind: &str) -> (&'static str, Color) {
    match kind {
        "monitor" => ("◎", BLUE),
        "workflow" => ("⋔", QUEUE), // one cell wide everywhere, unlike ⧉
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
    job_right_used(j, out, None, now)
}

/// job_right with what the task's processes use, before how it stands
/// (jobRight, jobUsage).
pub fn job_right_used(j: &Job, out: Option<&Tailed>, used: Option<(u64, f64)>, now: Ms) -> Line {
    let mut l = Line::new();
    if job_crashed(j, out, now) {
        l.spans.push(sp("⚠ error in recent output", RED));
        l.spans.push(sp("  ·  ", DIM));
    } else if let Some((b, cpu)) = used {
        l.spans.push(cpu_color(cpu, format!("{cpu:.0}%")));
        l.spans.push(sp(" · ", DIM));
        l.spans.push(mem_color(b, mem(b)));
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
pub fn job_hint(j: &Job, workflow: bool, part: Option<(&str, &str)>) -> Line {
    if workflow {
        return keys(&[
            ("space", "its agents"),
            ("x", "stop"),
            ("shift+x", "…and say why"),
        ]);
    }
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

/// What the keys do on picked task j of session s, k and p offered while
/// a command of its chain runs (jobHint).
pub fn picked_hint(s: &Session, j: &Job) -> Line {
    let part = s.running_part(&j.tool_use_id);
    let pause = crate::screens::projects_machine::shells::pause_word(&j.tool_use_id);
    job_hint(
        j,
        s.job_kind(j) == "workflow",
        part.as_ref().map(|p| (p.command.as_str(), pause)),
    )
}

/// How the dock draws: the task picked (its ref, "job:<id>"), whether
/// the pane has the keys, the spinner's step, each task's output as last
/// read, whose each task is (jobWho: "→ Explore" for a subagent's), and
/// a workflow's phase now (workflowNow).
pub struct Dock<'a> {
    pub sel: &'a str,
    pub focus: bool,
    pub tick: usize,
    pub now: Ms,
    pub out: &'a dyn Fn(&Job) -> Option<&'a Tailed>,
    pub who: &'a dyn Fn(&Job) -> Line,
    pub flow: &'a dyn Fn(&Job, usize) -> Line,
    /// What a running shell's processes use, memory and CPU, when read
    /// (jobUsage).
    pub usage: &'a dyn Fn(&Job) -> Option<(u64, f64)>,
}

/// Nothing read of a task's processes.
pub fn no_usage(_: &Job) -> Option<(u64, f64)> {
    None
}

/// A plugin's card for running task j: the first that knows it
/// (convo.JobCardOf; the watch plugin's is the one there is).
pub fn job_card_of(j: &Job, now: Ms) -> Option<JobCard> {
    if !j.running() {
        return None;
    }
    rush_bundled::prwatch::card_for(j.kind(), &j.id, rush_state::Time::from_ms(now))
}

/// The colour rush draws a card's tone in (toneColour).
pub fn tone_colour(t: Tone) -> Color {
    match t {
        Tone::Text => TEXT,
        Tone::Dim => DIM,
        Tone::Faint => FAINT,
        Tone::Good => GREEN,
        Tone::Bad => RED,
        Tone::Wait => ORANGE,
        Tone::Info => BLUE,
        Tone::Sub => SUB,
    }
}

/// A card's text in rush's colours (segs).
pub fn segs(ss: &[Seg]) -> Line {
    Line::from_spans(
        ss.iter()
            .map(|s| sp(s.text.clone(), tone_colour(s.tone)))
            .collect(),
    )
}

/// A card's mark: the spinner, turning, while it waits.
fn card_mark(c: &JobCard, tick: usize) -> Span {
    if c.spin {
        sp(SPINNER[tick % SPINNER.len()], tone_colour(c.tone))
    } else {
        sp(c.mark.clone(), tone_colour(c.tone))
    }
}

/// The cells before a row's mark: its lead, and under a run its ↳.
fn left_lead(lead: &str, who: bool) -> usize {
    width(lead) + if who { 0 } else { 2 }
}

/// A task a plugin draws, as job_row lays out a shell's: its mark, its
/// kind, the plugin's words, and its own right end; picked, the plugin's
/// detail under it (cardRow).
#[allow(clippy::too_many_arguments)]
fn card_row(
    j: &Job,
    card: &JobCard,
    i: usize,
    w: usize,
    lead: &str,
    who: bool,
    d: &Dock,
) -> Vec<Line> {
    let whose = if who { (d.who)(j) } else { Line::new() };
    let mut left = line([plain(lead)]);
    let name = if who {
        spb(format!("{:<7}", card.kind), TEXT)
    } else {
        left.spans.push(sp("↳", FAINT));
        left.spans.push(plain(" "));
        sp(format!("{:<7}", card.kind), SUB)
    };
    left.spans
        .extend([card_mark(card, d.tick + i), plain(" "), name, plain(" ")]);
    join(&mut left, segs(&card.body));
    join(&mut left, whose);
    let mut right = segs(&card.right);
    right.spans.push(plain("  "));
    let keep = (w as isize - right.width() as isize - 2).max(20) as usize;
    let mut rows = vec![spread(rush_draw::truncate_line(&left, keep, "…"), right, w)];
    if d.sel == format!("job:{}", j.id) {
        let pad = blanks(left_lead(lead, who) + 2 + 8);
        for det in &card.detail {
            let mut l = line([plain(pad.clone())]);
            join(&mut l, segs(det));
            rows.push(rush_draw::truncate_line(&l, w.saturating_sub(1), "…"));
        }
        rows = rows.into_iter().map(|r| picked(r, w, d.focus)).collect();
    }
    rows
}

/// No one's: the session's own task, at home.
pub fn nobody(_: &Job) -> Line {
    Line::new()
}

/// No workflow's phase to say.
pub fn no_flow(_: &Job, _: usize) -> Line {
    Line::new()
}

/// A dock task's row (jobRow): lead, its mark and kind, what it runs and
/// whose it is when who, its latest line of output (a workflow's phase
/// now), and how it stands at the end. Under a subagent (not who) it
/// reads as its, hung under it with ↳.
pub fn job_row(s: &Session, j: &Job, i: usize, w: usize, lead: &str, who: bool, d: &Dock) -> Line {
    job_rows(s, j, i, w, lead, who, d).swap_remove(0)
}

/// job_row with a plugin card's detail under it when it's picked.
pub fn job_rows(
    s: &Session,
    j: &Job,
    i: usize,
    w: usize,
    lead: &str,
    who: bool,
    d: &Dock,
) -> Vec<Line> {
    if let Some(card) = job_card_of(j, d.now) {
        return card_row(j, &card, i, w, lead, who, d);
    }
    vec![shell_row(s, j, i, w, lead, who, d)]
}

/// A shell's (or monitor's, workflow's) row in the dock (jobRow).
fn shell_row(s: &Session, j: &Job, i: usize, w: usize, lead: &str, who: bool, d: &Dock) -> Line {
    let kind = s.job_kind(j);
    let (icon, col) = job_icon(kind);
    let mark = if j.background {
        sp(icon, col)
    } else {
        sp(SPINNER[(d.tick + i) % SPINNER.len()], ORANGE)
    };
    let out = (d.out)(j);
    let right = job_right_used(j, out, (d.usage)(j), d.now);
    let whose = if who { (d.who)(j) } else { Line::new() };
    let room =
        (w as isize - right.width() as isize - whose.width() as isize - 12 - width(lead) as isize)
            .max(12) as usize;
    let mut left = line([plain(lead)]);
    let name = if who {
        spb(format!("{kind:<7}"), TEXT)
    } else {
        left.spans.push(sp("↳", FAINT));
        left.spans.push(plain(" "));
        sp(format!("{kind:<7}"), SUB)
    };
    left.spans.extend([
        mark,
        plain(" "),
        name,
        plain(" "),
        sp(
            rush_draw::truncate(&job_label(s, j), room.min(40), "…").into_owned(),
            SUB,
        ),
    ]);
    join(&mut left, whose);
    // Its latest line of output, on the same row, as far as there's room;
    // a workflow's phase now.
    if kind == "workflow" {
        let now = (d.flow)(j, d.tick + i);
        if !now.spans.is_empty() {
            left.spans.push(plain("  "));
            left.spans.push(sp("›", ORANGE));
            left.spans.push(plain(" "));
            join(&mut left, now);
        }
    } else if let Some(last) = out.and_then(|t| t.lines.last()) {
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

/// What the dock calls its tasks: their kind when they're all one, else
/// tasks (jobsWord).
pub fn jobs_word(s: &Session, jobs: &[&Job]) -> String {
    let mut word = "";
    for j in jobs {
        let k = s.job_kind(j);
        if word.is_empty() {
            word = k;
        } else if k != word {
            return "tasks".into();
        }
    }
    if word.is_empty() {
        "shell".into()
    } else {
        word.into()
    }
}

/// The dock's block of running tasks: a heading with what the keys do,
/// then a row each, at most DOCK_JOBS_SHOWN, kept round the one picked,
/// and each row's ref (jobsPreview). hint is what the heading says when
/// none is picked (the pane's ↑ to pick).
pub fn jobs_preview(
    s: &Session,
    jobs: &[&Job],
    w: usize,
    d: &Dock,
    hint: Line,
) -> (Vec<Line>, Vec<String>) {
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
        Some(i) => picked_hint(s, jobs[i]),
        None => hint,
    };
    hint.spans.push(plain("  "));
    let word = jobs_word(s, jobs);
    let title = format!("{}{}", word[..1].to_uppercase(), &word[1..]);
    let head = line([
        plain(" "),
        spb(title, SUB),
        plain("  "),
        sp(format!("{} {head}", jobs.len()), DIM),
    ]);
    let mut out = vec![spread(head, hint, w)];
    let mut refs = vec![String::new()];
    let start = match picked_at {
        Some(p) if p >= DOCK_JOBS_SHOWN => p - DOCK_JOBS_SHOWN + 1,
        _ => 0,
    };
    if start > 0 {
        out.push(line([sp(format!("  … {start} before"), DIM)]));
        refs.push(String::new());
    }
    let end = jobs.len().min(start + DOCK_JOBS_SHOWN);
    for (i, j) in jobs.iter().enumerate().take(end).skip(start) {
        for r in job_rows(s, j, i, w, "  ", true, d) {
            out.push(r);
            refs.push(format!("job:{}", j.id));
        }
    }
    if jobs.len() > start + DOCK_JOBS_SHOWN {
        let rest = jobs.len() - start - DOCK_JOBS_SHOWN;
        out.push(line([sp(
            format!("  + {rest} more in the background view"),
            DIM,
        )]));
        refs.push(String::new());
    }
    (out, refs)
}

/// The edge down the left of an opened task (cEdge).
pub const EDGE: Color = Color::Rgb(79, 73, 67);

/// Rows and each one's ref.
pub type Rows = (Vec<Line>, Vec<String>);

/// How the background view draws: the row picked and whether the pane
/// has the keys, the row under the pointer, what's open, and each task's
/// output, owner, phases, processes and launch script as the Session last
/// read them.
pub struct Bg<'a> {
    pub w: usize,
    pub now: Ms,
    /// The spinner's step.
    pub tick: usize,
    pub sel: &'a str,
    pub focused: bool,
    pub hover: &'a str,
    /// Whether ref is opened (c.open).
    pub open: &'a dyn Fn(&str) -> bool,
    /// Whether a task is a spawned agent's shell: the subagents view has
    /// those (spawnJob).
    pub spawned: &'a dyn Fn(&Job) -> bool,
    /// The file its output is read from and that file as last read: its
    /// own, else one its command writes (jobTailFrom).
    pub out: &'a dyn Fn(&Job) -> (String, Option<&'a Tailed>),
    /// Its own output file (jobOutput).
    pub own: &'a dyn Fn(&Job) -> String,
    /// Any file as last read (c.tails).
    pub tailed: &'a dyn Fn(&str) -> Option<&'a Tailed>,
    /// The files its command writes (jobWrites).
    pub writes: &'a dyn Fn(&Job) -> Vec<String>,
    /// What its processes use, and those under it once opened (jobUsage,
    /// Table.Tree), as last read.
    pub usage: &'a dyn Fn(&Job) -> Option<(u64, f64)>,
    pub procs: &'a dyn Fn(&Job) -> Option<&'a [ProcRow]>,
    /// The command of its chain running now (RunningPart).
    pub part: &'a dyn Fn(&Job) -> Option<RunningPart>,
    /// A path as a task's rows name it (shownPath), and the folder a
    /// relative path a tail -f follows is from.
    pub shown: &'a dyn Fn(&str) -> String,
    pub cwd: &'a str,
    /// Whose it is (jobWho).
    pub who: &'a dyn Fn(&Job) -> Line,
    /// A workflow's rows under its own, opened or not, with each row's ref
    /// (workflowRows).
    pub flow: &'a dyn Fn(&Job, bool) -> Rows,
    /// Its launch script laid out, indented 6 (Session.JobLines).
    pub details: &'a dyn Fn(&Job) -> Vec<Line>,
    /// Finished tasks' rows as last drawn (c.jobRows).
    pub memo: Option<&'a Mutex<HashMap<String, RowsMemo>>>,
}

/// What a finished task's rows are drawn from (jobRowsKey).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowsKey {
    w: usize,
    status: String,
    from: String,
    last: String,
    error: String,
    summary: String,
    label: String,
    agent: String,
    steps: i64,
    tokens: i64,
    end: Ms,
    tail: usize,
    /// When its file last changed: the "updated … ago" it says.
    age: String,
}

/// A finished task's rows as last drawn, unpicked (jobRowsMemo).
#[derive(Clone, Debug, Default)]
pub struct RowsMemo {
    key: RowsKey,
    rows: Vec<Line>,
}

#[cfg(test)]
impl RowsMemo {
    /// Marks the kept rows, to see them used.
    pub fn mark_for_test(&mut self) {
        if let Some(r) = self.rows.first_mut() {
            *r = line([plain("(kept)")]);
        }
    }
}

/// When a task's output last changed, as last read: " · updated 12s
/// ago", so output that's stopped coming reads as such (tailWhen).
pub fn tail_when(t: Option<&Tailed>, now: Ms) -> Option<Span> {
    let t = t.filter(|t| t.mod_at != 0)?;
    Some(sp(
        format!(" · updated {} ago", age((now - t.mod_at).max(0))),
        FAINT,
    ))
}

/// Task j's output and the file it's from: its own, or, when that has
/// nothing and it's a shell, the file it writes that changed last; while
/// it runs, only one still changing: a file written at its start and
/// left isn't its output, and would read as done (jobTailFrom). writes
/// are the files its command writes, tailed each file as last read.
pub fn tail_from<'a>(
    s: &Session,
    j: &Job,
    own: &str,
    writes: &[String],
    tailed: &dyn Fn(&str) -> Option<&'a Tailed>,
    now: Ms,
) -> (String, Option<&'a Tailed>) {
    let t = tailed(own);
    if t.is_some_and(|t| !t.lines.is_empty()) || s.job_kind(j) != "shell" {
        return (own.to_string(), t);
    }
    let (mut from, mut best, mut at) = (String::new(), None, 0);
    for w in writes {
        let Some(e) = tailed(w) else { continue };
        if e.lines.is_empty()
            || j.running() && now - e.mod_at > LIVE_FOR
            || e.mod_at <= at && !from.is_empty()
        {
            continue;
        }
        from.clone_from(w);
        best = Some(e);
        at = e.mod_at;
    }
    (from, best)
}

/// What a file a command writes last held: a log as it is, anything
/// else as a document, its lines numbered as they are in it, coloured as
/// its kind of file has them, indent cells in and w wide (fileRows).
pub fn file_rows(
    p: &str,
    lines: &[String],
    t: Option<&Tailed>,
    indent: usize,
    w: usize,
) -> Vec<Line> {
    if !rush_convo::render::is_code(p) {
        return lines
            .iter()
            .map(|l| {
                rush_draw::truncate_line(
                    &line([plain(blanks(indent)), sp("│ ", FAINT), sp(l.clone(), TEXT)]),
                    w,
                    "…",
                )
            })
            .collect(); // a log, not a document
    }
    let nos: &[usize] = match t {
        Some(t) if t.nos.len() == t.lines.len() && t.nos.len() >= lines.len() => {
            &t.nos[t.nos.len() - lines.len()..]
        }
        _ => &[],
    };
    rush_convo::render::code_rows(p, lines, nos, (w.saturating_sub(indent)).max(10))
        .into_iter()
        .map(|r| {
            let mut l = line([plain(blanks(indent))]);
            join(&mut l, r);
            l
        })
        .collect()
}

/// The last n lines of t.
fn last_of(t: Option<&Tailed>, n: usize) -> Vec<String> {
    t.map(|t| t.lines[t.lines.len().saturating_sub(n)..].to_vec())
        .unwrap_or_default()
}

/// The tasks the background view lists, what's running first, the latest
/// finished first after: every task but subagents' runs, a spawned
/// agent's shell (the subagents view has those) and what the turn waits
/// on (the conversation has those).
pub fn bg_jobs<'a>(s: &'a Session, spawned: &dyn Fn(&Job) -> bool) -> (Vec<&'a Job>, Vec<&'a Job>) {
    let (mut run, mut done) = (Vec::new(), Vec::new());
    for j in s.work_jobs() {
        if spawned(j) || !j.background {
            continue;
        }
        if j.running() {
            run.push(j);
        } else {
            done.push(j);
        }
    }
    done.reverse();
    (run, done)
}

/// The background view: every task this session has run but its
/// subagents, what's running first, each with its output under it when
/// opened; a workflow with its phases under it. With each row's ref
/// (jobLines, jobs.go:416-671).
pub fn job_lines(s: &Session, b: &Bg) -> Vec<(Line, String)> {
    let w = b.w;
    let (run, done) = bg_jobs(s, b.spawned);
    let total = run.len() + done.len();
    let mut first = line([plain("  ")]);
    if !run.is_empty() {
        first
            .spans
            .push(sp(format!("{} running", run.len()), ORANGE));
        first.spans.push(sp(" · ", DIM));
    }
    first.spans.push(sp(format!("{total} tasks"), DIM));
    first.spans.push(sp(
        " · space or click: output · d: command details · x: stop",
        DIM,
    ));
    let mut lines: Vec<(Line, String)> =
        vec![(fit(first, w), String::new()), (Line::new(), String::new())];
    if total == 0 {
        lines.push((
            line([sp(
                "  nothing running · shells, monitors and workflows your agent starts show here",
                DIM,
            )]),
            String::new(),
        ));
        return lines;
    }
    // A workflow's agent's row: its workflow's ref, so an opened
    // workflow's edge runs beside its agents' rows too.
    let mut inside: HashMap<String, String> = HashMap::new();
    for (title, jobs) in [("Running", &run), ("Finished", &done)] {
        if jobs.is_empty() {
            continue;
        }
        lines.push((fit(line([plain("  "), spb(title, SUB)]), w), String::new()));
        for (i, j) in jobs.iter().enumerate() {
            job_block(s, b, j, i, &mut lines, &mut inside);
        }
        lines.push((Line::new(), String::new()));
    }
    lines
}

/// One task's rows in the background view (jobLines' section body).
fn job_block(
    s: &Session,
    b: &Bg,
    j: &Job,
    i: usize,
    lines: &mut Vec<(Line, String)>,
    inside: &mut HashMap<String, String>,
) {
    let w = b.w;
    let cut = |l: Line| rush_draw::truncate_line(&l, w.saturating_sub(2), "…");
    let r = format!("job:{}", j.id);
    let opened = (b.open)(&r);
    let details_open = (b.open)(&format!("job-details:{}", j.id));
    let kind = s.job_kind(j);
    let (icon, col) = job_icon(kind);
    let fold = if opened {
        sp("▾", TEXT)
    } else {
        sp("▸", FAINT) // every task opens, whatever it is
    };
    let mut mark = sp(icon, col);
    if j.running() && !j.background {
        mark = sp(SPINNER[(b.tick + i) % SPINNER.len()], ORANGE);
    }
    let n = if opened {
        JOB_TAIL_MOST
    } else if j.running() {
        3
    } else {
        1
    };
    let (from, out) = (b.out)(j);
    let own = (b.own)(j);
    let mut tail = last_of(out, n);
    // A finished task's rows stay as they were drawn until what they're
    // drawn from changes: a long session has hundreds.
    let mut key = None;
    if !j.running() && !opened && kind != "workflow" {
        let k = RowsKey {
            w,
            status: j.status.clone(),
            from: from.clone(),
            last: tail.last().cloned().unwrap_or_default(),
            error: j.error.clone(),
            summary: j.summary.clone(),
            label: j.label.clone(),
            agent: j.agent.clone(),
            steps: j.tool_uses,
            tokens: j.tokens,
            end: j.end,
            tail: tail.len(),
            age: tail_when(out, b.now).map(|s| s.text).unwrap_or_default(),
        };
        if let Some(memo) = b.memo
            && let Some(e) = memo.lock().unwrap_or_else(|e| e.into_inner()).get(&j.id)
            && e.key == k
        {
            bg_emit(b, lines, inside, &r, e.rows.clone(), true);
            return;
        }
        key = Some(k);
    }
    let right = job_right_used(j, out, (b.usage)(j), b.now);
    let who = (b.who)(j);
    let room = (w as isize - right.width() as isize - who.width() as isize - 18).max(12);
    let mut right = right;
    let mut left;
    let card = job_card_of(j, b.now);
    if let Some(c) = &card {
        // The plugin that knows the task says what it is and how it stands.
        right = segs(&c.right);
        right.spans.push(plain("  "));
        left = line([
            plain("  "),
            fold,
            plain(" "),
            card_mark(c, b.tick + i),
            plain(" "),
            spb(format!("{:<8}", c.kind), TEXT),
            plain(" "),
        ]);
        join(&mut left, who);
        join(&mut left, segs(&c.body));
        let keep = (w as isize - right.width() as isize - 2).max(20) as usize;
        left = rush_draw::truncate_line(&left, keep, "…");
    } else {
        left = line([
            plain("  "),
            fold,
            plain(" "),
            mark,
            plain(" "),
            spb(format!("{kind:<8}"), TEXT),
            plain(" "),
        ]);
        join(&mut left, who);
        left.spans.push(sp(
            rush_draw::truncate(&job_label(s, j), room as usize, "…").into_owned(),
            SUB,
        ));
    }
    let mut rows = vec![spread(left, right, w)];
    for d in card.iter().flat_map(|c| &c.detail) {
        let mut l = line([plain("      ")]);
        join(&mut l, segs(d));
        rows.push(cut(l));
    }
    let mut facts = line([plain("      ")]);
    let fact = |l: &mut Line, f: Vec<Span>| {
        if l.spans.len() > 1 {
            l.spans.push(sp(" · ", DIM));
        }
        l.spans.extend(f);
    };
    if let Some(rp) = (b.part)(j) {
        fact(
            &mut facts,
            vec![
                sp(format!("▸ {}/{}", rp.at, rp.of), ORANGE),
                sp(" ", SUB),
                sp(one_line(&rp.command), TEXT),
            ],
        );
    }
    if j.tool_uses > 0 {
        fact(&mut facts, vec![sp(format!("{} steps", j.tool_uses), SUB)]);
    }
    if j.tokens > 0 {
        let t = crate::screens::session_core::paint::tokens(j.tokens);
        fact(&mut facts, vec![sp(format!("{t} tokens"), SUB)]);
    }
    let said = if j.error.is_empty() {
        &j.summary
    } else {
        &j.error
    };
    if !said.is_empty() && *said != j.label {
        fact(&mut facts, vec![sp(one_line(said), SUB)]);
    }
    if facts.spans.len() > 1 {
        rows.push(cut(facts));
    }
    // A workflow's phases, and opened, its agents: each its own row.
    let mut wf = false;
    if kind == "workflow" {
        let (wf_rows, wf_refs) = (b.flow)(j, false);
        if !wf_rows.is_empty() {
            wf = true;
            bg_emit(b, lines, inside, &r, std::mem::take(&mut rows), true);
            for (l, fr) in wf_rows.into_iter().zip(wf_refs) {
                inside.insert(fr.clone(), r.clone());
                bg_emit(b, lines, inside, &fr, vec![l], true);
            }
            tail.clear(); // its output is its progress again, as JSON
        }
    }
    let activity = job_activity(j, out, b.now);
    if !activity.spans.is_empty() && !wf {
        let mut l = line([plain("      ")]);
        join(&mut l, activity);
        rows.push(l);
    }
    // Opened, what it runs: the processes under it, busiest first, and
    // what a tail -f under it follows.
    let mut processes = Vec::new();
    let mut followed: Vec<String> = Vec::new();
    if opened && let Some(nodes) = (b.procs)(j) {
        for n in nodes {
            followed.extend(tail_follows(&n.cmd, b.cwd));
        }
        let mut busy: Vec<&ProcRow> = nodes.iter().collect();
        busy.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for n in busy.into_iter().take(5) {
            let name = if details_open { &n.cmd } else { &n.comm };
            let name = fit(line([sp(name.clone(), SUB)]), w.saturating_sub(24).max(10));
            let mut l = line([plain("      ")]);
            join(&mut l, name);
            l.spans
                .push(cpu_color(n.cpu, format!("{:>6}", format!("{:.0}%", n.cpu))));
            l.spans
                .push(mem_color(n.footprint, format!("{:>8}", mem(n.footprint))));
            processes.push(l);
        }
    }
    let other = from != own;
    if !tail.is_empty() && other {
        let mut l = line([
            plain("      "),
            sp(format!("from {}", (b.shown)(&from)), FAINT),
        ]);
        l.spans.extend(tail_when(out, b.now));
        rows.push(l);
    } else if !tail.is_empty() && opened {
        let mut l = line([plain("      "), spb("Recent output", SUB)]);
        l.spans.extend(tail_when(out, b.now));
        rows.push(l);
    }
    if other && !from.is_empty() {
        // A file it writes, as the document it is.
        rows.extend(file_rows(&from, &tail, out, 6, w.saturating_sub(2)));
    } else {
        for l in &tail {
            rows.push(cut(line([
                plain("      "),
                sp("│ ", FAINT),
                sp(l.clone(), TEXT),
            ])));
        }
    }
    if tail.is_empty() && opened && !wf {
        let none = if j.running() {
            "no output yet"
        } else {
            "no output"
        };
        rows.push(line([plain("      "), sp(none, SUB)]));
    }
    if opened {
        // Opened: every other file its command writes, and those what it
        // runs follows, too; one that's there but empty says so.
        let mut seen: Vec<String> = Vec::new();
        for f in (b.writes)(j).into_iter().chain(followed) {
            if f == from || seen.contains(&f) {
                continue;
            }
            seen.push(f.clone());
            let t = (b.tailed)(&f);
            let more = last_of(t, JOB_FILE_LINES);
            if more.is_empty() {
                if t.is_some_and(|t| t.size == 0) {
                    let mut l = line([
                        plain("      "),
                        sp(format!("from {} · empty so far", (b.shown)(&f)), FAINT),
                    ]);
                    l.spans.extend(tail_when(t, b.now));
                    rows.push(l);
                }
                continue;
            }
            let mut l = line([
                plain("      "),
                sp(format!("from {}", (b.shown)(&f)), FAINT),
            ]);
            l.spans.extend(tail_when(t, b.now));
            rows.push(l);
            rows.extend(file_rows(&f, &more, t, 6, w.saturating_sub(2)));
        }
    }
    if let (Some(k), Some(memo)) = (key, b.memo) {
        memo.lock().unwrap_or_else(|e| e.into_inner()).insert(
            j.id.clone(),
            RowsMemo {
                key: k,
                rows: rows.clone(),
            },
        );
    }
    bg_emit(b, lines, inside, &r, rows, true);
    if opened {
        if !processes.is_empty() {
            let head = line([plain("      "), sp("Processes · busiest first", DIM)]);
            bg_emit(b, lines, inside, &r, vec![head], false);
            bg_emit(b, lines, inside, &r, processes.clone(), false);
        }
        let details = if details_open {
            "▾ command details · d to hide"
        } else {
            "▸ command details · d to show"
        };
        if !s.job_command(j).is_empty() || !processes.is_empty() {
            let l = line([plain("      "), sp(details, SUB)]);
            bg_emit(b, lines, inside, &r, vec![l], false);
        }
        if details_open {
            for l in (b.details)(j) {
                lines.push((bg_edge(b, inside, &r, l), r.clone()));
            }
        }
    }
}

/// The edge down the left of an opened task, beside every part of it, so
/// its output and command read as inside it; the pick's bar when it's
/// picked (jobLines' edge).
fn bg_edge(b: &Bg, inside: &HashMap<String, String>, r: &str, l: Line) -> Line {
    let owner = inside.get(r).map_or("", String::as_str);
    if !(b.open)(r) && (owner.is_empty() || !(b.open)(owner)) {
        return l;
    }
    let bar = if r == b.sel && b.focused {
        sp("▍", ORANGE)
    } else if r == b.sel {
        sp("▍", FAINT)
    } else {
        sp("│", EDGE)
    };
    let mut out = line([bar]);
    join(&mut out, cut_first(fit(l, b.w)));
    out
}

/// A task's rows into lines: picked, under the pointer, or edged when
/// it's open (jobLines' emit). pick: whether the rows light as its.
fn bg_emit(
    b: &Bg,
    lines: &mut Vec<(Line, String)>,
    inside: &HashMap<String, String>,
    r: &str,
    rows: Vec<Line>,
    pick: bool,
) {
    for l in rows {
        let l = if pick && r == b.sel {
            picked(l, b.w, b.focused)
        } else if pick && r == b.hover {
            bg_edge(b, inside, r, hover_line(l, b.w))
        } else {
            bg_edge(b, inside, r, fit(l, b.w))
        };
        lines.push((l, r.to_string()));
    }
}
