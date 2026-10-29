//! The plan in the header's quiet line: each provider's accounts in the
//! order rush moves sessions through them, the rest batched, and a
//! provider's windows in a few cells. After Go's internal/ui/bars_quiet.go
//! (planRows :94, drains :127, acctMeter :153, used :163, near :168,
//! amount :172, queueLine :187, queueMin :197, queueDraw :206,
//! windowsUsed :258, backIn :282, singleLine :310, fitDrawn :358,
//! planSummary :446, quotaNote :469, firstWindows :484, compactWindow
//! :530, systemAlerts :548), view.go:468 (resetIn, roughly) and
//! agentquota.go (nextAccount :119, accountChain :140, nearlyOut :159,
//! nearSwitch :192, upcoming :198's pick). Not here yet: reading the
//! accounts (accountRows, acctQueues, the accounts group's), so these
//! take their rows ready made, a provider's mark and colour in each.

use rush_core::text::{Color, Line, Span};
use rush_core::time::Ms;
use rush_core::usage::{EVERY, LEAD, Quota, Window};

use super::meters::{FRESH, MeterRow, pct, usage_color};
use crate::style::*;

/// Used from here on, an account's number shows (nearAt).
pub const NEAR_AT: f64 = 80.0;
/// How wide the meter of the account in use is (meterW).
pub const METER_W: usize = 6;
/// How full rush lets an account get before it moves on (state.SwitchAt).
pub const SWITCH_AT: f64 = 95.0;
/// How full the account in use gets before the header says which one is
/// next (nearSwitch).
pub const NEAR_SWITCH: f64 = SWITCH_AT - 15.0;
const HOUR: Ms = 3_600_000;

/// An account in a queue (inQueue).
#[derive(Clone, Debug, Default)]
pub struct InQueue {
    pub name: String,
    pub cur: bool,
    pub q: Quota,
}

/// One provider's accounts in the order rush moves sessions through
/// them, the one in use first (acctQueue): its mark and colour lead.
#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub kind: String,
    pub glyph: String,
    pub colour: Option<Color>,
    pub accts: Vec<InQueue>,
}

fn mark(glyph: &str, colour: Option<Color>) -> Span {
    sp(glyph, colour.unwrap_or(TEXT))
}

/// A duration to one unit: 40m, 3h, 5d (roughly).
pub fn roughly(d: Ms) -> String {
    let round = |d: Ms, u: Ms| (d + u / 2) / u;
    if d < HOUR {
        format!("{}m", round(d, 60_000).max(1))
    } else if d < 48 * HOUR {
        format!("{}h", round(d, HOUR))
    } else {
        format!("{}d", round(d, 24 * HOUR))
    }
}

/// When a window resets, faint: " ↻3h", or " reset" when it has (resetIn).
pub fn reset_in(at: Ms, now: Ms) -> Option<Span> {
    match at {
        0 => None,
        at if at <= now => Some(sp(" reset", FAINT)),
        at => Some(sp(format!(" ↻{}", roughly(at - now)), FAINT)),
    }
}

/// p percent as a bar n cells wide, coloured by how full (acctMeter).
pub fn acct_meter(p: f64, n: usize) -> Line {
    let mut f = (p / 100.0 * n as f64 + 0.5) as usize;
    if p > 0.0 && f == 0 {
        f = 1; // something used shows as something
    }
    let f = f.min(n);
    line([
        sp("▰".repeat(f), usage_color(p)),
        sp("▱".repeat(n - f), FAINT),
    ])
}

/// How much of an account's tightest window is used (used).
pub fn used(q: &Quota, now: Ms) -> f64 {
    q.since(now).used("").clamp(0.0, 100.0)
}

/// An account worth a number: near its limit, or out (near).
pub fn near(q: &Quota, now: Ms) -> bool {
    used(q, now) >= NEAR_AT
}

/// When a reading can't be trusted: a window has reset since, or it's
/// old (quotaNote).
pub fn quota_note(q: &Quota, now: Ms) -> Option<Span> {
    if q.windows.iter().any(|w| w.resets_at != 0 && now >= w.resets_at) {
        return Some(sp(" refresh needed", YELLOW));
    }
    let every = EVERY.as_millis() as Ms;
    if !q.problem.is_empty() || q.fetched_at != 0 && now - q.fetched_at > 3 * every {
        return Some(sp(" stale", YELLOW));
    }
    None
}

/// When an account has room again (backIn): for one out, when it's back,
/// its day and time past a day off; for one in use, its tightest
/// window's reset once that's half used; for any other, that reset.
pub fn back_in(q: &Quota, now: Ms, cur: bool) -> Option<Span> {
    let back = q
        .since(now)
        .windows
        .iter()
        .filter(|w| w.percent >= 100.0)
        .map(|w| w.resets_at)
        .max()
        .unwrap_or(0);
    if back != 0 {
        if back - now > 24 * HOUR {
            return Some(sp(
                format!(" until {}", super::meters::day_clock(back)),
                DIM,
            ));
        }
        return reset_in(back, now);
    }
    let w = q.tightest("")?;
    if cur && w.percent < 50.0 {
        return None;
    }
    reset_in(w.resets_at, now)
}

/// An account's number: how much is used, or out in red, and when it's
/// back (amount).
pub fn amount(q: &Quota, now: Ms, cur: bool) -> Line {
    let p = used(q, now);
    let s = if p >= 100.0 {
        sp("out", RED)
    } else {
        sp(pct(p), usage_color(p))
    };
    let mut l = line([s]);
    l.spans.extend(back_in(q, now, cur));
    l.spans.extend(quota_note(q, now));
    l
}

/// Each provider's account in use (drains): a mark and a number for one
/// near its limit; the rest batched, their marks and one meter for the
/// fullest.
pub fn drains(rows: &[MeterRow], now: Ms) -> Line {
    let mut out = Line::new();
    let mut quiet = Line::new();
    let mut most: f64 = 0.0;
    for r in rows {
        if near(&r.q, now) {
            out.spans.push(plain("  "));
            out.spans.push(mark(&r.glyph, r.colour));
            out.spans.push(plain(" "));
            join(&mut out, amount(&r.q, now, false));
            continue;
        }
        quiet.spans.push(mark(&r.glyph, r.colour));
        most = most.max(used(&r.q, now));
    }
    if quiet.spans.is_empty() {
        return out;
    }
    let mut l = line([plain("  ")]);
    join(&mut l, quiet);
    l.spans.push(plain(" "));
    join(&mut l, acct_meter(most, 3));
    join(&mut l, out);
    l
}

/// The most of what draw draws that fits in w (fitDrawn): every detail
/// (level 0) before any of the n things drawn goes, then each level
/// less; then the last things, one at a time, with the least detail.
pub fn fit_drawn(draw: impl Fn(usize, usize) -> Line, n: usize, w: usize) -> Line {
    for keep in (1..=n).rev() {
        for level in 0..=2 {
            if keep < n && level < 2 {
                continue; // later ones go only once the rest are short
            }
            let s = draw(level, keep);
            if s.width() <= w {
                return s;
            }
        }
    }
    rush_draw::truncate_line(&draw(2, 1), w, "…")
}

/// A queue drawn at a level of detail, 0 to 2, with its first keep
/// accounts drawn on their own: the one in use with its meter, then
/// the others near their limit or out; the rest counted (queueDraw).
pub fn queue_draw(qu: &Queue, now: Ms) -> impl Fn(usize, usize) -> Line + '_ {
    let mut accts: Vec<&InQueue> = qu.accts.iter().collect();
    let none_cur = !accts.iter().any(|a| a.cur);
    let mut shown: Vec<(&InQueue, bool)> = Vec::new();
    let mut ready = 0;
    for (i, a) in accts.drain(..).enumerate() {
        // None in use yet: the first in line is the one a session starts on.
        let cur = a.cur || none_cur && i == 0;
        if cur {
            shown.insert(0, (a, true));
        } else if near(&a.q, now) {
            shown.push((a, false));
        } else {
            ready += 1;
        }
    }
    move |level, keep| {
        let mut out = line([mark(&qu.glyph, qu.colour)]);
        let keep = keep.min(shown.len());
        for &(a, cur) in &shown[..keep] {
            let name = rush_draw::truncate(&a.name, 12, "…").into_owned();
            if cur {
                out.spans.push(plain(" "));
                out.spans.push(sp(format!("▸ {name}"), BRIGHT));
                out.spans.push(plain(" "));
                join(&mut out, acct_meter(used(&a.q, now), METER_W));
                if near(&a.q, now) {
                    out.spans.push(plain(" "));
                    join(&mut out, amount(&a.q, now, true));
                }
                continue;
            }
            out.spans.push(plain("  "));
            out.spans.push(sp("→ ", FAINT));
            out.spans.push(sp(name, DIM));
            out.spans.push(plain(" "));
            if level == 0 {
                join(&mut out, acct_meter(used(&a.q, now), 3));
                out.spans.push(plain(" "));
            }
            if level < 2 {
                join(&mut out, amount(&a.q, now, false));
            } else {
                let p = used(&a.q, now);
                out.spans.push(if p >= 100.0 {
                    sp("out", RED)
                } else {
                    sp(pct(p), usage_color(p))
                });
            }
        }
        let hidden = shown.len() - keep;
        if hidden > 0 {
            out.spans.push(sp(format!("  +{}", hidden + ready), DIM));
        } else if ready > 0 {
            out.spans.push(sp(format!("  +{ready} ready"), DIM));
        }
        out
    }
}

/// A provider's accounts in the order rush moves through them, fitted to
/// w: short of room it leaves out the others' meters, then their resets,
/// then the last of them, counted; never the one in use (queueLine).
pub fn queue_line(qu: &Queue, now: Ms, w: usize) -> Line {
    let n = qu
        .accts
        .iter()
        .filter(|a| a.cur || near(&a.q, now))
        .count();
    fit_drawn(queue_draw(qu, now), n.max(1), w)
}

/// The least of a queue that's drawn: the account in use (queueMin).
pub fn queue_min(qu: &Queue, now: Ms) -> usize {
    queue_draw(qu, now)(2, 1).width()
}

/// How much of its short and long windows an account has used, "2%/4%",
/// and of its short one alone; "out", red, when one is full (windowsUsed).
pub fn windows_used(q: &Quota, now: Ms) -> (Line, Line) {
    if q.since(now).used("") >= 100.0 {
        let out = line([sp("out", RED)]);
        return (out.clone(), out);
    }
    let chosen = first_windows(q);
    let Some(first) = chosen.first() else {
        let none = line([sp("–", DIM)]);
        return (none.clone(), none);
    };
    let used = |w: &Window| {
        let p = w.percent.clamp(0.0, 100.0);
        sp(pct(p), usage_color(p))
    };
    let short = line([used(first)]);
    match chosen.get(1) {
        None => (short.clone(), short),
        Some(second) => (line([used(first), sp("/", DIM), used(second)]), short),
    }
}

/// The providers with one account (singleLine): those near their limit
/// each with its mark, name, number and reset; the rest batched as marks
/// and one meter. Short of room: resets go, then names, then the last.
pub fn single_line(rows: &[MeterRow], now: Ms, w: usize) -> Line {
    struct Part {
        glyph: Span,
        name: Span,
        used: Line,
        back: Vec<Span>,
    }
    let mut parts = Vec::new();
    let mut quiet = Line::new();
    let mut most: f64 = 0.0;
    for r in rows {
        if !near(&r.q, now) {
            quiet.spans.push(mark(&r.glyph, r.colour));
            most = most.max(used(&r.q, now));
            continue;
        }
        let (_, short) = windows_used(&r.q, now);
        let mut used = line([plain(" ")]);
        join(&mut used, short);
        parts.push(Part {
            glyph: mark(&r.glyph, r.colour),
            name: sp(format!(" {}", r.kind), DIM),
            used,
            back: back_in(&r.q, now, false)
                .into_iter()
                .chain(quota_note(&r.q, now))
                .collect(),
        });
    }
    let mut batch = Line::new();
    if !quiet.spans.is_empty() {
        batch = quiet;
        batch.spans.push(plain(" "));
        join(&mut batch, acct_meter(most, 4));
    }
    if parts.is_empty() {
        return rush_draw::truncate_line(&batch, w, "…");
    }
    let n = parts.len();
    fit_drawn(
        |level, keep| {
            let mut out = batch.clone();
            for p in &parts[..keep] {
                if !out.spans.is_empty() {
                    out.spans.push(plain("   "));
                }
                out.spans.push(p.glyph.clone());
                if level < 2 {
                    out.spans.push(p.name.clone());
                }
                join(&mut out, p.used.clone());
                if level < 1 {
                    out.spans.extend(p.back.iter().cloned());
                }
            }
            if keep < n {
                out.spans.push(sp(format!("  +{}", n - keep), DIM));
            }
            out
        },
        n,
        w,
    )
}

/// The queues a line each, then the providers with one account on a line
/// together, each fitted to its room (planRows). With fewer lines than
/// that, the last holds its queue and only the mark and tightest window
/// of each provider after it.
pub fn plan_rows(queues: &[Queue], single: &[MeterRow], now: Ms, rooms: &[usize]) -> Vec<Line> {
    let mut out = Vec::with_capacity(rooms.len());
    for (i, qu) in queues.iter().enumerate() {
        let at = out.len();
        if at + 1 == rooms.len() && (i + 1 < queues.len() || !single.is_empty()) {
            // The last line, and more to say than fits on it.
            let mut rest: Vec<MeterRow> = queues[i + 1..]
                .iter()
                .flat_map(|q| {
                    q.accts.iter().filter(|a| a.cur).map(|a| MeterRow {
                        kind: q.kind.clone(),
                        glyph: q.glyph.clone(),
                        colour: q.colour,
                        q: a.q.clone(),
                        ..MeterRow::default()
                    })
                })
                .collect();
            rest.extend(single.iter().cloned());
            let mut tail = drains(&rest, now);
            let mut w = rooms[at].saturating_sub(tail.width());
            if w < queue_min(qu, now) {
                tail = Line::new();
                w = rooms[at];
            }
            let mut l = queue_line(qu, now, w);
            join(&mut l, tail);
            out.push(l);
            return out;
        }
        out.push(queue_line(qu, now, rooms[at]));
    }
    if !single.is_empty() && out.len() < rooms.len() {
        let room = rooms[out.len()];
        out.push(single_line(single, now, room));
    }
    out
}

/// q's two shortest windows, the shortest first; one that doesn't say
/// how long it runs after those that do, two as long by their labels
/// (firstWindows).
pub fn first_windows(q: &Quota) -> Vec<Window> {
    let less = |a: &Window, b: &Window| {
        if a.span != b.span {
            if a.span.is_zero() {
                return false;
            }
            if b.span.is_zero() {
                return true;
            }
            return a.span < b.span;
        }
        a.label < b.label
    };
    let mut chosen: Vec<Window> = Vec::with_capacity(2);
    for w in &q.windows {
        if chosen.is_empty() {
            chosen.push(w.clone());
        } else if less(w, &chosen[0]) {
            let first = std::mem::replace(&mut chosen[0], w.clone());
            chosen.truncate(1);
            chosen.push(first);
        } else if chosen.len() == 1 {
            chosen.push(w.clone());
        } else if less(w, &chosen[1]) {
            chosen[1] = w.clone();
        }
    }
    chosen
}

/// A plan window in a few cells: its label, how much is used, and when
/// it resets; one that runs out within the hour, before it resets, says
/// when, in red (compactWindow).
pub fn compact_window(w: &Window, now: Ms) -> Line {
    let label = if w.label.is_empty() { "limit" } else { &w.label };
    let p = w.percent.clamp(0.0, 100.0);
    let mut l = line([sp(format!("{label} "), DIM), sp(pct(p), usage_color(p))]);
    let rate = w.rate(now);
    if rate > 0.0 && p < 100.0 {
        let left = ((100.0 - p) / rate * HOUR as f64) as Ms;
        if left < HOUR && (w.resets_at == 0 || left < w.resets_at - now) {
            l.spans.push(sp(format!(" out in {}", roughly(left)), RED));
        }
    }
    l.spans.extend(reset_in(w.resets_at, now));
    l
}

/// name, drawn as the caller likes, and its windows, the shortest first
/// whatever's used, so the header doesn't reorder as it fills
/// (planSummary).
pub fn plan_summary(name: Line, q: &Quota, now: Ms) -> Line {
    if q.windows.is_empty() {
        if q.problem.is_empty() {
            return Line::new();
        }
        let mut l = name;
        l.spans.push(sp(" limits ", DIM));
        l.spans.push(sp("unavailable", YELLOW));
        return l;
    }
    let chosen = first_windows(q);
    let mut l = name;
    for (i, w) in chosen.iter().enumerate() {
        l.spans.push(plain(if i == 0 { " " } else { "  " }));
        join(&mut l, compact_window(w, now));
    }
    if q.windows.len() > chosen.len() {
        l.spans.push(sp(
            format!(" +{} limits", q.windows.len() - chosen.len()),
            DIM,
        ));
    }
    l.spans.extend(quota_note(q, now));
    l
}

/// Only what needs attention on the machine: the network down or shaky,
/// the disk or battery low (systemAlerts). Disk and battery as bytes free
/// of total and percent charged, None when unknown.
pub fn system_alerts(
    battery: Option<(f64, bool)>,
    disk: Option<(u64, u64)>,
    net: Option<(bool, bool)>,
) -> Line {
    let mut out: Vec<Span> = Vec::new();
    if let Some((up, steady)) = net {
        if !up {
            out.push(sp("network offline", RED));
        } else if !steady {
            out.push(sp("network unstable", YELLOW));
        }
    }
    if let Some((free, total)) = disk
        && total > 0
        && free < 20 << 30
    {
        out.push(sp("disk low", if free < 5 << 30 { RED } else { YELLOW }));
    }
    if let Some((p, charging)) = battery
        && !charging
        && p < 30.0
    {
        out.push(sp("battery low", if p < 15.0 { RED } else { YELLOW }));
    }
    let mut l = Line::new();
    for (i, s) in out.into_iter().enumerate() {
        if i > 0 {
            l.spans.push(sp(" · ", DIM));
        }
        l.spans.push(s);
    }
    l
}

/// Whether a fresh reading says the account is nearly out (nearlyOut).
pub fn nearly_out(q: &Quota, now: Ms) -> bool {
    !q.windows.is_empty() && now - q.fetched_at < FRESH && q.nearly_out("", LEAD, now)
}

/// The order rush moves an agent's sessions through its accounts: the
/// one in use, then the rest with a reading, the most room first
/// (accountChain).
pub fn account_chain(accts: &[MeterRow]) -> Vec<MeterRow> {
    let mut out: Vec<MeterRow> = accts.iter().filter(|r| r.current).cloned().collect();
    let mut rest: Vec<MeterRow> = accts
        .iter()
        .filter(|r| !r.current && !r.q.windows.is_empty())
        .cloned()
        .collect();
    rest.sort_by(|a, b| a.q.used("").total_cmp(&b.q.used("")));
    out.extend(rest);
    out
}

/// The account to switch to when the one in use is nearly out, and why:
/// the one with the most room, unless that's nearly out too
/// (nextAccount).
pub fn next_account(accts: &[MeterRow], now: Ms) -> Option<(MeterRow, String)> {
    let chain = account_chain(accts);
    let cur = chain.first()?;
    if chain.len() < 2 || !cur.current || !nearly_out(&cur.q, now) {
        return None;
    }
    for o in &chain[1..] {
        if now - o.q.fetched_at >= HOUR {
            continue; // too old a reading to move to
        }
        if o.q.used("") >= SWITCH_AT {
            break;
        }
        let why = format!("{} was at {}", cur.name, pct(cur.q.used("")));
        return Some((o.clone(), why));
    }
    None
}

/// The account new sessions move on to once the one in use, used percent
/// full, is nearly out: the other with the most room, unless that's nearly
/// out itself; None while there's room still, or when the default profile
/// waits at a limit (upcoming).
pub fn upcoming(accts: &[MeterRow], used: f64, waits: bool, now: Ms) -> Option<MeterRow> {
    if !(NEAR_SWITCH..100.0).contains(&used) || waits {
        return None;
    }
    accts
        .iter()
        .filter(|r| {
            !r.current
                && !r.q.windows.is_empty()
                && now - r.q.fetched_at < HOUR
                && r.q.used("") < SWITCH_AT
        })
        .fold(None::<&MeterRow>, |best, r| match best {
            Some(b) if b.q.used("") <= r.q.used("") => Some(b),
            _ => Some(r),
        })
        .cloned()
}
