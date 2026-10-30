//! #away and #loop: an open session carries on while you're gone, told to
//! keep going at each check-in until the time's up, what it asks saved for
//! when you're back. After Go's internal/ui/away.go (parseAway :68, the
//! back sheet's body :117, heldReply :200, awayChip :220, meter :250) and
//! host/away.go:28 (Away, Held, On, DefaultAwayEvery). Not here yet: the
//! commands themselves and the back sheet's keys, which wait on rush
//! hosting sessions (wave B's host); these are the words and pieces they
//! draw with.

use rush_core::text::Line;
use rush_core::time::Ms;

use crate::frame::sheet::{sheet_title, window};
use crate::style::*;

const MINUTE: Ms = 60_000;
/// How often an away checks in unless told (host.DefaultAwayEvery).
pub const DEFAULT_EVERY: Ms = 30 * MINUTE;

/// An away or a loop on a session (host.Away): times in ms, 0 for none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Away {
    pub from: Ms,
    pub until: Ms,
    pub every: Ms,
    /// The next check-in.
    pub next: Ms,
    /// Check-ins sent so far.
    pub nudges: usize,
    /// You being there: no "I'm away", and nothing held.
    pub looping: bool,
    /// When an away's time ran out or it said it's all done.
    pub ended: Ms,
    pub held: Vec<Held>,
}

/// Something the agent asked, or asked to do, while you were away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// question, or permission
    pub kind: String,
    pub text: String,
    pub at: Ms,
}

impl Away {
    /// Whether it's still checking in (On).
    pub fn on(&self) -> bool {
        self.ended == 0
    }
}

/// A Go duration ("3h", "90m", "1h30m", "1.5h") in ms; None when it
/// isn't one (time.ParseDuration, to the millisecond).
pub fn parse_duration(s: &str) -> Option<Ms> {
    let mut rest = s.trim();
    let neg = rest.strip_prefix('-').inspect(|r| rest = r).is_some();
    if rest.is_empty() {
        return None;
    }
    if rest == "0" {
        return Some(0);
    }
    let mut total = 0f64;
    while !rest.is_empty() {
        let n = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if n == 0 {
            return None;
        }
        let v: f64 = rest[..n].parse().ok()?;
        rest = &rest[n..];
        let u = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = match &rest[..u] {
            "ns" => 1e-6,
            "us" | "µs" | "μs" => 1e-3,
            "ms" => 1.0,
            "s" => 1e3,
            "m" => 6e4,
            "h" => 3.6e6,
            _ => return None,
        };
        rest = &rest[u..];
        total += v * unit;
    }
    let ms = total as Ms;
    Some(if neg { -ms } else { ms })
}

/// Reads "3h", "3h every 15m" or "3h 15m"; a loop needs no time: "",
/// "every 15m" (parseAway).
pub fn parse_away(arg: &str, looping: bool, now: Ms) -> Result<Away, String> {
    let (long, mut every, cut) = match arg.split_once("every") {
        Some((l, e)) => (l, e.to_string(), true),
        None => (arg, String::new(), false),
    };
    let mut f: Vec<&str> = long.split_whitespace().collect();
    if !cut && f.len() == 2 {
        every = f[1].to_string();
        f.truncate(1);
    }
    let every = every.trim();
    if f.len() > 1 || f.is_empty() && !looping || cut && every.is_empty() {
        if looping {
            return Err("#loop, #loop every 15m, #loop 3h every 15m, #loop off".into());
        }
        return Err("#away how long: #away 3h, #away 3h every 15m, #away off".into());
    }
    let mut a = Away {
        from: now,
        every: DEFAULT_EVERY,
        looping,
        ..Away::default()
    };
    if let Some(t) = f.first() {
        match parse_duration(t) {
            Some(d) if d > 0 => a.until = now + d,
            _ => return Err(format!("{t}: a time like 3h or 90m")),
        }
    }
    if !every.is_empty() {
        match parse_duration(every) {
            Some(d) if d >= MINUTE => a.every = d,
            _ => return Err(format!("every {every}: a minute or more, like 15m")),
        }
    }
    Ok(a)
}

/// One message answering everything the agent saved while you were away,
/// for you to fill in (heldReply).
pub fn held_reply(held: &[Held]) -> String {
    if held.is_empty() {
        return String::new();
    }
    let mut b = String::from("On what you saved while I was away:");
    for h in held {
        let t = one_line(&h.text);
        if h.kind == "permission" {
            let (tool, what) = t.split_once(' ').unwrap_or((&t, ""));
            b += &format!("\n- you wanted {tool}: {what} → ");
            continue;
        }
        b += &format!("\n- {t} → ");
    }
    b
}

/// The header's say on an away or looping session: time left, and when
/// wide a bar of it and when it's next checked on; once an away's over,
/// what it saved for you (awayChip).
pub fn away_chip(a: &Away, now: Ms, wide: bool) -> Line {
    if !a.on() {
        return line([
            sp("✈ back", BLUE),
            sp(format!(" · {} saved for you · #back", a.held.len()), DIM),
        ]);
    }
    let left = a.until - now;
    let mut chip = if a.looping { "⟳ loop" } else { "✈ away" }.to_string();
    if a.until != 0 {
        chip += &format!(" {}", dur(left));
    }
    let mut l = line([sp(chip, BLUE)]);
    if !wide {
        return l;
    }
    if a.until != 0 {
        l.spans.push(plain(" "));
        join(&mut l, meter(left, a.until - a.from, 8));
    }
    let next = if now < a.next {
        format!("next check-in {}", dur(a.next - now))
    } else {
        "checks in once idle".to_string()
    };
    let held = match a.held.len() {
        0 => String::new(),
        n => format!(" · {n} saved"),
    };
    l.spans
        .push(sp(format!(" · {next} · {} sent{held}", a.nudges), DIM));
    l
}

/// A bar w cells wide, part of whole filled (meter).
pub fn meter(part: Ms, whole: Ms, w: usize) -> Line {
    let n = if whole > 0 {
        ((w as f64 * part as f64 / whole as f64 + 0.5) as isize).clamp(0, w as isize) as usize
    } else {
        0
    };
    line([sp("━".repeat(n), BLUE), sp("─".repeat(w - n), FAINT)])
}

/// "15:04" at ms, here.
pub fn clock(ms: Ms) -> String {
    let h = crate::model::local_hour(ms);
    let m = (ms / 1000 + crate::model::local_zone(ms)).rem_euclid(3600) / 60;
    format!("{h:02}:{m:02}")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The back sheet: what happened while you were away, for how long, the
/// check-ins, its last words and what it saved for you (backSheet.body).
/// detail is the session's last words.
pub fn back_lines(a: Option<&Away>, detail: &str, now: Ms, w: usize, h: usize) -> Vec<Line> {
    let Some(a) = a else {
        return vec![
            sheet_title("Back", "not away any more", w),
            Line::new(),
            keys_fit(w, &[("esc", "close")]),
        ];
    };
    let (end, still) = if a.on() {
        (now, format!(" · still away till {}", clock(a.until)))
    } else {
        (a.ended, String::new())
    };
    let about = format!(
        "away {} · {} check-in{}{still}",
        dur(end - a.from),
        a.nudges,
        plural(a.nudges)
    );
    let mut out = vec![sheet_title("Back", &about, w), Line::new()];
    let d = unmark(&one_line(detail));
    if !d.is_empty() {
        let mut l = line([sp("its last words  ", DIM)]);
        join(&mut l, fit(line([sp(d, TEXT)]), w.saturating_sub(16)));
        out.push(l);
        out.push(Line::new());
    }
    if a.held.is_empty() {
        out.push(line([sp("nothing saved for you", DIM)]));
    } else {
        out.push(line([sp(
            format!(
                "saved for you · {}, put in the box as one reply when you end it",
                a.held.len()
            ),
            DIM,
        )]));
        let (from, to) = window(a.held.len(), 0, 1.max(h as isize - 9) as usize);
        for it in &a.held[from..to] {
            let mark = if it.kind == "permission" {
                sp("! ", ORANGE)
            } else {
                sp("? ", YELLOW)
            };
            let mut l = line([plain("  "), mark, sp(format!("{}  ", clock(it.at)), FAINT)]);
            join(
                &mut l,
                fit(line([sp(one_line(&it.text), TEXT)]), w.saturating_sub(12)),
            );
            out.push(l);
        }
        if to < a.held.len() {
            out.push(line([sp(
                format!("    and {} more", a.held.len() - to),
                FAINT,
            )]));
        }
    }
    let later = if a.on() { "keep it" } else { "later" };
    out.push(Line::new());
    out.push(keys_fit(
        w,
        &[
            ("enter", "end away"),
            ("l", "end, then loop"),
            ("esc", later),
        ],
    ));
    out
}
