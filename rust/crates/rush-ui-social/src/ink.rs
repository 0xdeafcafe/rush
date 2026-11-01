//! The few drawing words the social screens share, as Go writes them:
//! spread and onBg (rushmode.go:2170-2181), keysControls
//! (settings_keys.go:118), wrap with @mentions kept whole (style.go:360),
//! tagged (stream.go:310), the handles' colours (stream.go:233-262) and
//! local days (community.go:257-268).

use rush_core::text::{Color, Line, Span, Style, width};
use rush_core::time::Ms;

use crate::style::*;

/// A reply's ground inside a card (style.go:81 bgBtw).
pub const BTW_BG: Color = Color::Rgb(36, 33, 30);

/// left, then right flush with the edge w cells on; left alone, cut, when
/// there's no room for two blanks between (Go's spread, rushmode.go:2175:
/// the header's own spread keeps a margin, this one doesn't).
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

/// l fit to w on ground bg, kept on it under every span (onBg).
pub fn on_bg(bg: Color, l: Line, w: usize) -> Line {
    on_ground(l, w, bg)
}

/// "key label" pairs, wrapped whole onto as many rows as they need: none
/// disappear when the box is narrow (keysControls).
pub fn keys_controls(w: usize, pairs: &[(&str, &str)]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut row = Line::new();
    for (k, what) in pairs {
        let item = line([spb(*k, TEXT), plain(" "), sp(*what, TEXT)]);
        if row.width() > 0 && row.width() + 3 + item.width() > w {
            out.push(std::mem::take(&mut row));
        }
        if row.width() > 0 {
            row.spans.push(plain(" · "));
        }
        join(&mut row, item);
    }
    if row.width() > 0 {
        out.push(row);
    }
    out
}

/// s wrapped to w cells, as style's wrap does, but an @mention never
/// breaks at its hyphens: they wrap as non-breaking ones (style.go:371).
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    if !s.contains('@') {
        return crate::style::wrap(s, w);
    }
    let held = rush_room::community::mention_re()
        .replace_all(s, |c: &regex::Captures| c[0].replace('-', "\u{2011}"));
    crate::style::wrap(&held, w)
        .into_iter()
        .map(|l| l.replace('\u{2011}', "-"))
        .collect()
}

/// A line with its @mentions and #hashtags painted; the rest is in ink
/// (tagged).
pub fn tagged(text: &str, ink: Color) -> Line {
    let mut l = Line::new();
    for (i, w) in text.split(' ').enumerate() {
        if i > 0 {
            l.spans.push(Span::new(" ", Style::fg(ink)));
        }
        if w.is_empty() {
            continue;
        }
        let c = if w.len() > 1 && w.starts_with('@') {
            BLUE
        } else if w.len() > 1 && w.starts_with('#') {
            QUEUE
        } else {
            ink
        };
        l.spans.push(sp(w, c));
    }
    l
}

/// FNV-1a, 32 bits, as Go's hash/fnv New32a.
fn fnv32a(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

const PALETTE: [Color; 6] = [BLUE, GREEN, YELLOW, QUEUE, ORANGE, RED];

fn alike(a: Color, b: Color) -> bool {
    a == b || (a == ORANGE || a == RED) && (b == ORANGE || b == RED)
}

/// username's colour, unless the run before already wears it or one alike
/// (orange and red are); then the next one along alike to neither that
/// nor the run after's own, so the change stops with this run
/// (tintBeside).
pub fn tint_beside(username: &str, before: Option<Color>, after: Option<Color>) -> Color {
    let mut i = (fnv32a(username) % PALETTE.len() as u32) as usize;
    let Some(before) = before else {
        return PALETTE[i];
    };
    if !alike(PALETTE[i], before) {
        return PALETTE[i];
    }
    for _ in 0..PALETTE.len() {
        i = (i + 1) % PALETTE.len();
        if !alike(PALETTE[i], before) && !after.is_some_and(|a| alike(PALETTE[i], a)) {
            break;
        }
    }
    PALETTE[i]
}

/// The colour of an @handle, the same wherever it shows (handleTint).
pub fn handle_tint(username: &str) -> Color {
    tint_beside(username, None, None)
}

/// The local day ms falls in, as a number of days: two times on one day
/// have the same (localDay).
pub fn local_day(ms: Ms) -> i64 {
    (ms.div_euclid(1000) + crate::model::local_zone(ms)).div_euclid(86_400)
}

/// The day a local day number names: year, month and day.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ms's local day for a separator: Today, Yesterday, Mon 29 Sep
/// (dayLabel).
pub fn day_label(ms: Ms, now: Ms) -> String {
    let d = local_day(ms);
    let today = local_day(now);
    if d == today {
        return "Today".into();
    }
    if d == local_day(now - 86_400_000) {
        return "Yesterday".into();
    }
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, day) = civil(d);
    format!(
        "{} {day} {}",
        DAYS[d.rem_euclid(7) as usize],
        MONTHS[(m - 1) as usize]
    )
}

/// s cut to w cells with tail when it's longer (cellw.Truncate).
pub fn truncate(s: &str, w: usize, tail: &str) -> String {
    rush_draw::truncate(s, w, tail).into_owned()
}

/// s's width in cells.
pub fn cells(s: &str) -> usize {
    width(s)
}

/// What a post or a name says once stripped of what a terminal would act
/// on: escapes and controls gone, tabs a space (communityText).
pub fn clean_text(s: &str) -> String {
    crate::frame::editor::clean_paste(&strip_ansi(s))
}

/// s without its escape sequences (ansi.Strip).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('[') => {
                it.next();
                // CSI: parameters, then one final byte in @..~
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                it.next();
                // OSC: to BEL or ESC \
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            Some(_) => {
                it.next();
            }
            None => {}
        }
    }
    out
}

/// The single line s makes: whitespace runs one space (strings.Fields
/// joined).
pub fn fields(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_go_s() {
        assert_eq!(fnv32a(""), 0x811c9dc5);
        assert_eq!(fnv32a("a"), 0xe40c292c);
        assert_eq!(fnv32a("foobar"), 0xbf9cf968);
    }

    // Neighbouring runs never share a colour, nor orange beside red; an
    // author keeps their own colour unless the run before already wears
    // it, and one that can't keeps clear of the next run's.
    #[test]
    fn neighbours_never_share_a_tint() {
        let names = [
            "ann", "bo", "cy", "di", "ed", "flo", "gus", "hal", "ivy", "jo",
        ];
        for a in names {
            for b in names {
                for c in names {
                    let (before, own, after) = (handle_tint(a), handle_tint(b), handle_tint(c));
                    let got = tint_beside(b, Some(before), Some(after));
                    assert!(!alike(got, before), "{b} beside {a}: alike");
                    assert!(
                        alike(own, before) || got == own,
                        "{b} keeps its colour beside {a}"
                    );
                    assert!(got == own || !alike(got, after), "{b}, changed, clears {c}");
                }
            }
        }
    }

    #[test]
    fn wrap_keeps_mentions_whole() {
        for w in 15..30 {
            for l in wrap("ping @scout-dodo-two about it", w) {
                assert!(
                    !l.contains('@') || l.contains("@scout-dodo-two"),
                    "at {w} the mention broke: {l:?}"
                );
            }
        }
    }

    #[test]
    fn tags_mentions_and_hashtags() {
        let l = tagged("ping @worker about #flaky tests", SUB);
        assert_eq!(l.plain(), "ping @worker about #flaky tests");
        assert!(
            l.spans
                .iter()
                .any(|s| s.text == "@worker" && s.style.fg == BLUE)
        );
        assert!(
            l.spans
                .iter()
                .any(|s| s.text == "#flaky" && s.style.fg == QUEUE)
        );
    }

    #[test]
    fn days() {
        let now = 1_790_000_000_000; // a Wednesday in September 2026, give or take the zone
        assert_eq!(day_label(now, now), "Today");
        assert_eq!(day_label(now - 86_400_000, now), "Yesterday");
        let l = day_label(now - 3 * 86_400_000, now);
        assert!(l.ends_with(" Sep") && l.split(' ').count() == 3, "{l}");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_000), (2024, 10, 4));
    }

    #[test]
    fn strips() {
        assert_eq!(
            strip_ansi("a\x1b[2Jb\x1b[38;2;1;2;3mc\x1b]8;;x\x07d"),
            "abcd"
        );
    }
}
