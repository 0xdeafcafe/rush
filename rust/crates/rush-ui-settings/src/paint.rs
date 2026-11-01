//! Small drawing helpers the settings pages share, in Go's terms
//! (ansi.Truncate, firstNonEmpty, pbox and sideBySide from projects.go:222,
//! :285).

use rush_core::text::{Color, Line};

use crate::style::*;

/// l cut to w cells with … when it's wider (ansi.Truncate(s, w, "…")).
pub fn trunc(l: Line, w: usize) -> Line {
    if l.width() <= w {
        return l;
    }
    rush_draw::truncate_line(&l, w, "…")
}

/// The first of xs that isn't empty (firstNonEmpty).
pub fn first_non_empty<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// on or off (onOffWord).
pub fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// The edge colour of a box (cEdge).
pub const EDGE: Color = Color::Rgb(79, 73, 67);

/// A titled box w wide around body (pbox).
pub fn pbox(title: Line, meta: Line, body: &[Line], w: usize, lit: bool) -> Vec<Line> {
    let col = if lit { ORANGE } else { EDGE };
    let mut head = line([sp("╭─ ", col)]);
    join(&mut head, title);
    head.spans.push(plain(" "));
    let mut tail = Line::new();
    if meta.width() > 0 {
        tail.spans.push(plain(" "));
        join(&mut tail, meta);
        tail.spans.push(plain(" "));
    }
    let fill = (w as isize - head.width() as isize - tail.width() as isize - 2).max(1) as usize;
    let mut top = head;
    top.spans.push(sp("─".repeat(fill), col));
    join(&mut top, tail);
    top.spans.push(sp("─╮", col));
    let mut out = vec![fit(top, w)];
    for l in body {
        let mut r = line([sp("│", col), plain(" ")]);
        join(&mut r, fit(l.clone(), w.saturating_sub(4)));
        r.spans.push(plain(" "));
        r.spans.push(sp("│", col));
        out.push(r);
    }
    out.push(line([sp(
        format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
        col,
    )]));
    out
}

/// Two columns of lines, the left lw wide (sideBySide).
pub fn side_by_side(l: &[Line], r: &[Line], lw: usize) -> Vec<Line> {
    let n = l.len().max(r.len());
    (0..n)
        .map(|i| {
            let mut a = fit(l.get(i).cloned().unwrap_or_default(), lw);
            a.spans.push(plain(" "));
            join(&mut a, r.get(i).cloned().unwrap_or_default());
            a
        })
        .collect()
}

/// body filled out to n lines (pad).
pub fn pad_lines(mut body: Vec<Line>, n: usize) -> Vec<Line> {
    while body.len() < n {
        body.push(Line::new());
    }
    body
}

/// A moment in local time, by its parts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    /// 1–12.
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    /// 0 is Sunday.
    pub weekday: i64,
}

impl Civil {
    /// "Jan".
    pub fn month_name(&self) -> &'static str {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        MONTHS[(self.month.clamp(1, 12) - 1) as usize]
    }

    /// "Mon".
    pub fn weekday_name(&self) -> &'static str {
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        DAYS[self.weekday.rem_euclid(7) as usize]
    }
}

/// Unix ms at, in local time.
pub fn civil(at: rush_core::time::Ms) -> Civil {
    let local = at.div_euclid(1000) + crate::model::local_zone(at);
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Civil {
        year,
        month,
        day,
        hour: secs / 3600,
        minute: secs % 3600 / 60,
        // 1970-01-01 was a Thursday.
        weekday: (days + 4).rem_euclid(7),
    }
}

/// A line of text with SGR colour escapes as rush's own spans: the
/// status line's colours, true-colour foregrounds, bold, dim and resets.
/// Other escapes are dropped.
pub fn from_ansi(s: &str) -> Line {
    use rush_core::text::{Mods, Span, Style};
    let mut out = Line::new();
    let mut st = Style::default();
    let mut text = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            text.push(c);
            continue;
        }
        if it.peek() != Some(&'[') {
            continue;
        }
        it.next();
        let mut params = String::new();
        let mut end = ' ';
        for c in it.by_ref() {
            if c.is_ascii_digit() || c == ';' || c == ':' {
                params.push(c);
            } else {
                end = c;
                break;
            }
        }
        if end != 'm' {
            continue;
        }
        if !text.is_empty() {
            out.spans.push(Span::new(std::mem::take(&mut text), st));
        }
        let nums: Vec<u16> = params
            .split([';', ':'])
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let mut i = 0;
        while i < nums.len() {
            match nums[i] {
                0 => st = Style::default(),
                1 => st.mods = st.mods.with(Mods::BOLD),
                2 => st.mods = st.mods.with(Mods::DIM),
                22 => st.mods = Mods::NONE,
                39 => st.fg = Color::Default,
                49 => st.bg = Color::Default,
                38 | 48 if nums.get(i + 1) == Some(&2) && i + 4 < nums.len() => {
                    let c = Color::Rgb(nums[i + 2] as u8, nums[i + 3] as u8, nums[i + 4] as u8);
                    if nums[i] == 38 {
                        st.fg = c;
                    } else {
                        st.bg = c;
                    }
                    i += 4;
                }
                _ => {}
            }
            i += 1;
        }
    }
    if !text.is_empty() {
        out.spans.push(Span::new(text, st));
    }
    out
}
