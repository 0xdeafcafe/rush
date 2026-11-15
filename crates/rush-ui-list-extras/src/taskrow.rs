//! A background task as one row (or a few), the same in the dock and in
//! the conversation under the call that started it (Rust-only, no Go
//! counterpart: Go's jobRow, jobs.go:168, drew a kind and a label). Its
//! head: a mark (spinning while it runs, ✓ ✗ once done), what it is bold,
//! where dim, and on the right a slot that's never cut: a bar and its
//! counts, then how long it has run (and is expected to). The head's
//! words give way first, then the bar shrinks, then the counts go.
//!
//! - A rush watch (pr or run), from its plugin card's `TaskCard`: the
//!   workflows' name and branch · sha, a bar of its checks, `6m / ~9m`
//!   from how long its workflows usually take, and its checks as chips
//!   (✓ ✗ spinner, each one's time), wrapping to a second row; a failed
//!   check red with the first line of its log that says why. Once it has
//!   ended, one row: `passed in 8m · 2m ago`, `test failed: <why>`.
//! - Any other command: the command shortened (no `cd … &&`, no `2>&1`,
//!   no `| tail …`), how far it has got when its output says
//!   (rush_convo's `progress_any`: cargo's bar and tests, `n/m`, `n%`) as
//!   a bar and counts, its clock, and under it its last line that says
//!   something.

use rush_bundled::prwatch::{CardJob, JobCard, TaskCard};
use rush_core::text::{Line, Span, width};
use rush_core::time::Ms;

use crate::style::*;

/// The most cells a bar takes, and the fewest it's drawn with.
pub const BAR_MOST: usize = 12;
pub const BAR_LEAST: usize = 4;
/// The fewest cells of a head's name kept beside its right slot.
const NAME_LEAST: usize = 12;
/// The most rows a watch's chips take; past them, "+N more".
const CHIP_ROWS: usize = 2;
/// The most cells of a check's name on its chip.
const CHIP_NAME: usize = 28;
/// The most failed checks given a row of their own, with why.
const WHY_ROWS: usize = 2;

/// How long something has run, as a task row says it: 42s, 1m04s (under
/// ten minutes, where the seconds still tell), 12m, 1h05m.
pub fn clock(d: Ms) -> String {
    let s = d.max(0) / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{:02}m", s / 3600, s / 60 % 60)
    }
}

/// A whole watch's time, coarser than a check's: 42s, 6m, 1h05m.
pub fn minutes(d: Ms) -> String {
    if d < 60_000 {
        return clock(d);
    }
    let m = d / 60_000;
    if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h{:02}m", m / 60, m % 60)
    }
}

/// A bar n cells wide: what passed green, what failed red, the rest
/// faint; a part that has any gets a cell at least.
pub fn bar(done: u64, failed: u64, total: u64, n: usize) -> Vec<Span> {
    if total == 0 || n == 0 {
        return Vec::new();
    }
    let cells = |k: u64| {
        if k == 0 {
            0
        } else {
            ((k * n as u64 / total) as usize).max(1)
        }
    };
    let bad = cells(failed).min(n);
    let good = cells(done).min(n - bad);
    let rest = n - bad - good;
    [(good, GREEN, "━"), (bad, RED, "━"), (rest, FAINT, "─")]
        .into_iter()
        .filter(|(k, _, _)| *k > 0)
        .map(|(k, c, r)| sp(r.repeat(k), c))
        .collect()
}

/// A head row's right end: what comes before its bar ("212/430"), the
/// bar (done, failed, of total), and what comes after ("✓2 ◌1  7m"). The
/// bar takes what's left of BAR_MOST; before goes when even the
/// smallest bar and after don't fit.
#[derive(Clone, Debug, Default)]
pub struct Right {
    pub before: Vec<Span>,
    pub bar: Option<(u64, u64, u64)>,
    pub after: Vec<Span>,
}

fn spans_w(ss: &[Span]) -> usize {
    ss.iter().map(|s| width(&s.text)).sum()
}

impl Right {
    /// Laid out for room cells at most: as much as fits, the time last
    /// to go.
    fn lay(&self, room: usize) -> Line {
        let (bw, aw) = (spans_w(&self.before), spans_w(&self.after));
        let gap = |w: usize| usize::from(w > 0);
        let with_bar = |before: usize| -> Option<usize> {
            let fixed = before + gap(before) + gap(aw) + aw;
            let n = room.checked_sub(fixed)?.min(BAR_MOST);
            (n >= BAR_LEAST).then_some(n)
        };
        let mut l = Line::new();
        let push = |l: &mut Line, ss: &[Span]| {
            if ss.is_empty() {
                return;
            }
            if !l.spans.is_empty() {
                l.spans.push(plain(" "));
            }
            l.spans.extend(ss.iter().cloned());
        };
        if let Some((done, failed, total)) = self.bar.filter(|b| b.2 > 0) {
            if let Some(n) = with_bar(bw) {
                push(&mut l, &self.before);
                push(&mut l, &bar(done, failed, total, n));
                push(&mut l, &self.after);
                return l;
            }
            if let Some(n) = with_bar(0) {
                push(&mut l, &bar(done, failed, total, n));
                push(&mut l, &self.after);
                return l;
            }
        }
        if bw + gap(bw) + aw <= room {
            push(&mut l, &self.before);
            push(&mut l, &self.after);
        } else if aw <= room {
            push(&mut l, &self.after);
        }
        l
    }
}

/// A task's head row w wide: lead, its mark, its name (bold when strong),
/// where it is dim, and right at the end in a slot the words give way to:
/// the place is cut first, then the name down to NAME_LEAST, then right
/// is laid out smaller.
pub fn head_row(lead: &str, mark: Span, name: Span, place: &str, right: &Right, w: usize) -> Line {
    let least = width(lead) + 2 + width(&name.text).min(NAME_LEAST);
    let mut left = line([plain(lead), mark, plain(" "), name]);
    if !place.is_empty() {
        left.spans.push(plain("  "));
        left.spans.push(sp(place.to_string(), DIM));
    }
    let r = right.lay(w.saturating_sub(least + 2));
    let rw = r.width();
    if rw == 0 {
        return fit(left, w);
    }
    let keep = w.saturating_sub(rw + 2);
    let left = rush_draw::truncate_line(&left, keep, "…");
    let gap = w.saturating_sub(left.width() + rw);
    let mut out = left;
    out.spans.push(plain(blanks(gap)));
    join(&mut out, r);
    out
}

/// One chip: its mark, its name, how long it took or has run.
#[derive(Clone, Debug)]
pub struct Chip {
    pub mark: Span,
    pub name: Span,
    pub time: String,
}

impl Chip {
    fn spans(&self) -> Vec<Span> {
        let mut out = vec![self.mark.clone(), plain(" "), self.name.clone()];
        if !self.time.is_empty() {
            out.push(plain(" "));
            out.push(sp(self.time.clone(), DIM));
        }
        out
    }
    fn w(&self) -> usize {
        spans_w(&self.spans())
    }
}

/// chips in rows indent cells in and w wide, three cells apart, rows at
/// most; what doesn't fit is "+N more" at the end of the last.
pub fn chip_rows(chips: &[Chip], indent: usize, w: usize, rows: usize) -> Vec<Line> {
    chips_laid(chips, indent, w, rows).0
}

/// chip_rows, and how many of chips it shows.
fn chips_laid(chips: &[Chip], indent: usize, w: usize, rows: usize) -> (Vec<Line>, usize) {
    const GAP: usize = 3;
    let room = w.saturating_sub(indent).max(1);
    let rows = rows.max(1);
    let mut laid: Vec<Vec<Span>> = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut used = 0usize;
    let mut i = 0;
    while i < chips.len() {
        let last_row = laid.len() + 1 >= rows;
        // The last row keeps room for what says how many more.
        let reserve = if last_row && i + 1 < chips.len() {
            GAP + 8
        } else {
            0
        };
        let gap = if used == 0 { 0 } else { GAP };
        let cw = chips[i].w();
        if used + gap + cw + reserve <= room {
            if gap > 0 {
                cur.push(plain(blanks(gap)));
            }
            cur.extend(chips[i].spans());
            used += gap + cw;
            i += 1;
            continue;
        }
        if used == 0 {
            // Alone and still too wide: cut to the room it has.
            let avail = room.saturating_sub(reserve).max(1);
            let cut = rush_draw::truncate_line(&Line::from_spans(chips[i].spans()), avail, "…");
            used = cut.width();
            cur.extend(cut.spans);
            i += 1;
            continue;
        }
        if last_row {
            break;
        }
        laid.push(std::mem::take(&mut cur));
        used = 0;
    }
    if i < chips.len() {
        if used > 0 {
            cur.push(plain(blanks(GAP)));
        }
        cur.push(sp(format!("+{} more", chips.len() - i), FAINT));
        used += 1;
    }
    if used > 0 {
        laid.push(cur);
    }
    let out = laid
        .into_iter()
        .map(|ss| {
            let mut l = line([plain(blanks(indent))]);
            l.spans.extend(ss);
            l
        })
        .collect();
    (out, i)
}

/// A check's mark: ✓ passed, ✗ failed, ⊘ cancelled, – skipped, the
/// spinner while it runs, ◌ before it has started.
fn job_mark(j: &CardJob, tick: usize) -> Span {
    match j.state.as_str() {
        "passed" => sp("✓", GREEN),
        "failed" => sp("✗", RED),
        "cancelled" => sp("⊘", DIM),
        "skipped" => sp("–", FAINT),
        _ if j.started != 0 => sp(SPINNER[tick % SPINNER.len()], ORANGE),
        _ => sp("◌", FAINT),
    }
}

/// How long a check took, or has run; "" before it has started.
fn job_time(j: &CardJob, now: Ms) -> String {
    match (j.started, j.ended) {
        (0, _) => String::new(),
        (s, 0) if j.state == "pending" => clock(now - s),
        (_, 0) => String::new(),
        (s, e) => clock(e - s),
    }
}

fn chip_of(j: &CardJob, tick: usize, now: Ms) -> Chip {
    let colour = match j.state.as_str() {
        "failed" => RED,
        "pending" if j.started != 0 => TEXT,
        "pending" => DIM,
        _ => SUB,
    };
    Chip {
        mark: job_mark(j, tick),
        name: sp(
            rush_draw::truncate(&j.name, CHIP_NAME, "…").into_owned(),
            colour,
        ),
        time: job_time(j, now),
    }
}

/// A watch's checks as chips, for w cells from indent: each one when they
/// fit in CHIP_ROWS rows; else those that passed or were skipped as one
/// "✓ N passed" chip first, then the rest.
fn watch_chips(t: &TaskCard, indent: usize, w: usize, tick: usize, now: Ms) -> Vec<Line> {
    let all: Vec<Chip> = t.jobs.iter().map(|j| chip_of(j, tick, now)).collect();
    let (rows, shown) = chips_laid(&all, indent, w, CHIP_ROWS);
    if shown == all.len() {
        return rows;
    }
    let done = t
        .jobs
        .iter()
        .filter(|j| matches!(j.state.as_str(), "passed" | "skipped"))
        .count();
    if done == 0 {
        return rows;
    }
    let mut chips = vec![Chip {
        mark: sp("✓", GREEN),
        name: sp(format!("{done} passed"), SUB),
        time: String::new(),
    }];
    chips.extend(
        t.jobs
            .iter()
            .filter(|j| !matches!(j.state.as_str(), "passed" | "skipped"))
            .map(|j| chip_of(j, tick, now)),
    );
    chip_rows(&chips, indent, w, CHIP_ROWS)
}

/// The elapsed and expected time of a running watch: "6m / ~9m", past
/// the usual in yellow; "6m" when nothing is expected.
fn watch_clock(t: &TaskCard, now: Ms) -> Vec<Span> {
    let ran = now - t.began;
    if t.expect > t.began {
        let usual = t.expect - t.began;
        let c = if now > t.expect { YELLOW } else { SUB };
        return vec![
            sp(minutes(ran), c),
            sp(" / ", FAINT),
            sp(format!("~{}", minutes(usual)), DIM),
        ];
    }
    vec![sp(minutes(ran), SUB)]
}

/// A running watch's counts: ✓ passed, ✗ failed, ◌ still going.
fn watch_counts(t: &TaskCard, tick: usize) -> Vec<Span> {
    let mut out = vec![sp(format!("✓{}", t.passed + t.skipped), GREEN)];
    if t.failed > 0 {
        out.push(sp(format!(" ✗{}", t.failed), RED));
    }
    if t.pending > 0 {
        out.push(sp(
            format!(" {}{}", SPINNER[tick % SPINNER.len()], t.pending),
            ORANGE,
        ));
    }
    out
}

/// What a watch's name is drawn as: bold.
fn watch_name(t: &TaskCard) -> Span {
    spb(t.name.clone(), TEXT)
}

/// A watch's rows w wide, lead before its mark: its head, then (while it
/// runs) its checks as chips and why each failed one did. Once it has
/// ended, its head alone, saying how it went. tick turns its spinners.
pub fn watch_rows(c: &JobCard, lead: &str, tick: usize, now: Ms, w: usize) -> Vec<Line> {
    let t = &c.task;
    if t.ended != 0 {
        return vec![ended_row(t, lead, now, w)];
    }
    let held = c.mark == "✗";
    let mark = if held {
        sp("✗", RED)
    } else {
        sp(SPINNER[tick % SPINNER.len()], ORANGE)
    };
    let total = t.passed + t.failed + t.pending + t.skipped;
    let mut after = Vec::new();
    if t.asking {
        after.push(sp("asking GitHub…", DIM));
    } else if total > 0 {
        after = watch_counts(t, tick);
    }
    if !after.is_empty() {
        after.push(plain(" "));
    }
    after.extend(watch_clock(t, now));
    let right = Right {
        before: Vec::new(),
        bar: (total > 0).then_some(((t.passed + t.skipped) as u64, t.failed as u64, total as u64)),
        after,
    };
    let mut out = vec![head_row(lead, mark, watch_name(t), &t.place, &right, w)];
    let indent = width(lead) + 4;
    out.extend(watch_chips(t, indent, w, tick, now));
    for j in t
        .jobs
        .iter()
        .filter(|j| j.state == "failed" && !j.note.is_empty())
        .take(WHY_ROWS)
    {
        let l = line([
            plain(blanks(indent)),
            sp("✗ ", RED),
            sp(j.name.clone(), RED),
            plain("  "),
            sp(j.note.clone(), DIM),
        ]);
        out.push(rush_draw::truncate_line(&l, w, "…"));
    }
    if t.limited {
        let l = line([
            plain(blanks(indent)),
            sp("GitHub's rate limit is low: asking less often", YELLOW),
        ]);
        out.push(rush_draw::truncate_line(&l, w, "…"));
    }
    out
}

/// A watch that has ended, as one row: ✓ and "passed in 8m · 2m ago",
/// or ✗ and what failed and why.
fn ended_row(t: &TaskCard, lead: &str, now: Ms, w: usize) -> Line {
    let took = minutes(t.ended - t.began);
    let ago = sp(
        format!(" · {} ago", crate::style::age((now - t.ended).max(0))),
        FAINT,
    );
    let failed: Vec<&CardJob> = t.jobs.iter().filter(|j| j.state == "failed").collect();
    let (mark, after) = match t.outcome.as_str() {
        "passed" => (
            sp("✓", GREEN),
            vec![sp(format!("passed in {took}"), GREEN), ago],
        ),
        "failed" => {
            let mut a = match failed.first() {
                Some(j) => {
                    let more = if failed.len() > 1 {
                        format!(" +{}", failed.len() - 1)
                    } else {
                        String::new()
                    };
                    vec![sp(
                        format!(
                            "✗ {}{more} failed",
                            rush_draw::truncate(&j.name, CHIP_NAME, "…")
                        ),
                        RED,
                    )]
                }
                None => vec![sp(format!("failed in {took}"), RED)],
            };
            // Why, as far as there's room beside the name.
            if let Some(j) = failed.first().filter(|j| !j.note.is_empty()) {
                let used = spans_w(&a) + width(&ago.text) + width(lead) + 2 + NAME_LEAST + 4;
                let room = w.saturating_sub(used).min(48);
                if room >= 12 {
                    a.push(sp(
                        format!(": {}", rush_draw::truncate(&j.note, room - 2, "…")),
                        DIM,
                    ));
                }
            }
            a.push(ago);
            (sp("✗", RED), a)
        }
        "timed out" => (
            sp("◷", YELLOW),
            vec![sp(format!("timed out after {took}"), YELLOW), ago],
        ),
        o => (
            sp("⏹", DIM),
            vec![
                sp(
                    format!("{} after {took}", if o.is_empty() { "ended" } else { o }),
                    DIM,
                ),
                ago,
            ],
        ),
    };
    let right = Right {
        before: Vec::new(),
        bar: None,
        after,
    };
    head_row(lead, mark, watch_name(t), &t.place, &right, w)
}

/// A command as a task row names it: no cd before it, no 2>&1 or
/// redirect to /dev/null, no | tail, | head or | tee after it, on one
/// line, home as ~.
pub fn short_command(cmd: &str) -> String {
    let mut c = one_line(&tildify(rush_convo::text::drop_cd(cmd)));
    for tail in [" | tail", " | head", " | tee", " |& tee"] {
        if let Some(i) = c.find(tail) {
            c.truncate(i);
        }
    }
    for noise in [
        " 2>&1",
        " 2> /dev/null",
        " 2>/dev/null",
        " >/dev/null",
        " > /dev/null",
    ] {
        c = c.replace(noise, "");
    }
    c.trim().to_string()
}

/// Whether a line of output says something: not blank, and not only a
/// spinner's or a bar's leftovers.
fn says_something(l: &str) -> bool {
    let t = l.trim();
    !t.is_empty()
        && t.chars().any(|c| c.is_alphanumeric())
        && !t
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_whitespace() || "%.:/[]=>-#|".contains(c))
}

/// The last line of lines that says something.
pub fn last_said(lines: &[String]) -> Option<&str> {
    lines
        .iter()
        .rev()
        .map(String::as_str)
        .find(|l| says_something(l))
}

/// A running command as the dock and the conversation draw it.
pub struct ShellTask<'a> {
    /// What it runs (its command, else what it was called).
    pub cmd: &'a str,
    /// Its output's last lines, as last read.
    pub lines: &'a [String],
    pub start: Ms,
    pub now: Ms,
    /// Its mark: the spinner, a monitor's ◎.
    pub mark: Span,
    /// What comes first on the right: its CPU and memory, an error seen.
    pub used: Vec<Span>,
}

/// A running command's rows w wide: its head (mark, command, a bar and
/// counts when its output says how far it has got, its clock), then its
/// last line that says something.
pub fn shell_rows(t: &ShellTask, lead: &str, w: usize) -> Vec<Line> {
    let ls: Vec<&str> = t.lines.iter().map(String::as_str).collect();
    let prog = rush_convo::render::progress_any(t.cmd, &ls);
    let mut after = t.used.clone();
    after.push(sp(clock(t.now - t.start), SUB));
    let right = match prog.as_ref().and_then(|p| p.done) {
        Some((n, m)) => Right {
            before: vec![sp(
                match prog.as_ref().filter(|p| p.text.ends_with('%')) {
                    Some(p) => p.text.clone(),
                    None => format!("{n}/{m}"),
                },
                SUB,
            )],
            bar: Some((n as u64, 0, m as u64)),
            after,
        },
        None => Right {
            before: Vec::new(),
            bar: None,
            after,
        },
    };
    let name = sp(short_command(t.cmd), TEXT);
    let mut out = vec![head_row(lead, t.mark.clone(), name, "", &right, w)];
    let indent = width(lead) + 2;
    let said = last_said(t.lines).map(str::to_string).or_else(|| {
        prog.as_ref()
            .filter(|p| p.done.is_none())
            .map(|p| p.text.clone())
    });
    if let Some(l) = said {
        let row = line([
            plain(blanks(indent)),
            sp("› ", ORANGE),
            sp(l.trim().to_string(), DIM),
        ]);
        out.push(rush_draw::truncate_line(&row, w, "…"));
    }
    out
}

/// How far a running command's output says it has got, as one row for
/// under its call in the conversation: the bar, its counts and what it
/// says ("212/430 · tests 212 passed"); None when it doesn't say.
pub fn progress_row(cmd: &str, lines: &[String], indent: usize, w: usize) -> Option<Line> {
    let ls: Vec<&str> = lines.iter().map(String::as_str).collect();
    let p = rush_convo::render::progress_any(cmd, &ls)?;
    let mut l = line([plain(blanks(indent))]);
    if let Some((n, m)) = p.done.filter(|d| d.1 > 0) {
        l.spans.extend(bar(n as u64, 0, m as u64, BAR_MOST));
        l.spans.push(plain(" "));
        if !p.text.ends_with('%') {
            l.spans.push(sp(format!("{n}/{m}"), SUB));
            if p.text != format!("{n}/{m}") {
                l.spans.push(sp(format!(" · {}", p.text), DIM));
            }
        } else {
            l.spans.push(sp(p.text.clone(), SUB));
        }
    } else {
        l.spans.push(sp(p.text.clone(), SUB));
    }
    Some(rush_draw::truncate_line(&l, w, "…"))
}
