//! How full each account's limits are, in the header and a session's
//! card: the mini meters, the low note, a limit's countdown. After Go's
//! internal/ui/bars.go:339 (usageColor), agentquota.go:86 (fresh) and
//! :188 (pct), usagebar.go (lowAt :21, miniBar :56, miniMeter :63,
//! meterRow :75, switchNote :187) and usage_limit.go (quotaWindowName
//! :129, limitCountdown :132). Not here yet: reading the accounts
//! themselves (accountRows, the accounts group's), so a meter takes its
//! row ready made: a provider's mark and colour, a label and its quota.

use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_core::usage::{Quota, Window};

use crate::style::*;

/// How full, in percent, an account's tightest window gets before a
/// session on it says where else there's room (lowAt).
pub const LOW_AT: f64 = 80.0;
/// How old a quota reading can be and still count (fresh).
pub const FRESH: Ms = 15 * 60_000;
const HOUR: Ms = 3_600_000;

/// An account as a meter shows it (acctRow, the parts read here).
#[derive(Clone, Debug, Default)]
pub struct MeterRow {
    /// The provider's mark and its colour (lookOf).
    pub glyph: String,
    pub colour: Option<Color>,
    /// harness:account, as rush names it (acctRow.label).
    pub label: String,
    pub q: Quota,
}

/// Green, yellow from 60%, red from 85% (usageColor).
pub fn usage_color(p: f64) -> Color {
    if p >= 85.0 {
        RED
    } else if p >= 60.0 {
        YELLOW
    } else {
        GREEN
    }
}

/// "42%" (pct).
pub fn pct(p: f64) -> String {
    format!("{p:.0}%")
}

/// A five-cell bar of p percent used (miniBar).
pub fn mini_bar(p: f64) -> Line {
    let fill = ((p / 20.0 + 0.5) as isize).clamp(0, 5) as usize;
    line([
        sp("━".repeat(fill), usage_color(p)),
        sp("─".repeat(5 - fill), FAINT),
    ])
}

/// An account's tightest window at a glance: its provider's glyph, the
/// free tier said as such, a short bar and the percentage (miniMeter).
pub fn mini_meter(r: &MeterRow) -> Line {
    let mut l = line([sp(r.glyph.clone(), r.colour.unwrap_or(TEXT)), plain(" ")]);
    if r.q.plan == "free" {
        l.spans.push(sp("free ", DIM));
    }
    let p = r.q.used("");
    join(&mut l, mini_bar(p));
    l.spans.push(plain(" "));
    l.spans.push(sp(pct(p), usage_color(p)));
    l
}

/// A mini meter for each row with limits, the least room first, as many
/// as fit in w (meterRow).
pub fn meter_row(rows: &[MeterRow], w: usize) -> Line {
    let mut have: Vec<&MeterRow> = rows.iter().filter(|r| !r.q.windows.is_empty()).collect();
    have.sort_by(|a, b| b.q.used("").total_cmp(&a.q.used("")));
    let mut out = Line::new();
    for r in have {
        let mut next = out.clone();
        next.spans.push(plain("  "));
        join(&mut next, mini_meter(r));
        if next.width() > w {
            break;
        }
        out = next;
    }
    out
}

/// The low note's words: nothing while q has room or its reading is old;
/// else how full it is, and where there's most room, when anywhere has
/// room (switchNote).
pub fn switch_note(q: &Quota, others: &[MeterRow], now: Ms) -> String {
    let Some(w) = q.tightest("") else {
        return String::new();
    };
    if w.percent < LOW_AT || now - q.fetched_at > FRESH {
        return String::new();
    }
    let mut s = format!("low: {} of {} used", pct(w.percent), w.label);
    let best = others
        .iter()
        .filter(|r| {
            !r.q.windows.is_empty() && now - r.q.fetched_at <= HOUR && r.q.used("") < LOW_AT
        })
        .fold(None::<&MeterRow>, |best, r| match best {
            Some(b) if b.q.used("") <= r.q.used("") => Some(b),
            _ => Some(r),
        });
    if let Some(b) = best {
        s += &format!(" · {} has {} left", b.label, pct(100.0 - b.q.used("")));
    }
    s
}

/// A window's name in words (quotaWindowName).
pub fn window_name(w: &Window) -> &str {
    [&w.name, &w.label, &w.id]
        .into_iter()
        .find(|s| !s.is_empty())
        .map_or("Provider quota", String::as_str)
}

/// "Mon 15:04" at ms, here.
pub fn day_clock(ms: Ms) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let local = ms / 1000 + crate::model::local_zone(ms);
    let day = DAYS[local.div_euclid(86_400).rem_euclid(7) as usize];
    format!("{day} {}", super::away::clock(ms))
}

/// How long until a limit resets, and when (limitCountdown).
pub fn limit_countdown(reset: Ms, now: Ms) -> String {
    if reset == 0 {
        return "Reset time unknown".into();
    }
    if reset <= now {
        return "Reset time passed · refresh to check usage".into();
    }
    let s = ((reset - now + 500) / 1000).max(1);
    format!(
        "Reset in {:02}:{:02}:{:02} · {}",
        s / 3600,
        s / 60 % 60,
        s % 60,
        day_clock(reset)
    )
}
