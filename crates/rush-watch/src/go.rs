//! Go's words for things, where rush's output says them as Go does: a
//! time.Duration's String and ParseDuration, strconv.Quote (%q), and the
//! small helpers watch and prdrive share.
//! Ported from Go's time/time.go (Duration.String, fmtFrac, fmtInt),
//! time/format.go (ParseDuration), strconv/quote.go, and
//! internal/watch/pr.go:641-664 (short, clip, round).

use std::time::Duration;

/// A Duration as Go's time.Duration.String writes it: "1h5m0s", "45m0s",
/// "1.5s", "300ms", "0s".
pub fn duration(d: Duration) -> String {
    duration_ns(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// duration for a signed count of nanoseconds, as Go keeps one.
pub fn duration_ns(d: i64) -> String {
    let neg = d < 0;
    let mut u = d.unsigned_abs();
    let mut out: Vec<u8> = Vec::with_capacity(32);
    // Built back to front, as Go does, then turned round.
    if u < 1_000_000_000 {
        if u == 0 {
            return "0s".into();
        }
        out.push(b's');
        let prec;
        if u < 1_000 {
            prec = 0;
            out.push(b'n');
        } else if u < 1_000_000 {
            prec = 3;
            // µ, back to front
            out.push(0xB5);
            out.push(0xC2);
        } else {
            prec = 6;
            out.push(b'm');
        }
        u = frac(&mut out, u, prec);
        int(&mut out, u);
    } else {
        out.push(b's');
        u = frac(&mut out, u, 9);
        int(&mut out, u % 60);
        u /= 60;
        if u > 0 {
            out.push(b'm');
            int(&mut out, u % 60);
            u /= 60;
            if u > 0 {
                out.push(b'h');
                int(&mut out, u);
            }
        }
    }
    if neg {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Go's fmtFrac, back to front: v's fraction of prec digits, trailing
/// zeros dropped, and no point when it's all zeros; v without it.
fn frac(out: &mut Vec<u8>, mut v: u64, prec: u32) -> u64 {
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            out.push(digit as u8 + b'0');
        }
        v /= 10;
    }
    if print {
        out.push(b'.');
    }
    v
}

/// Go's fmtInt, back to front.
fn int(out: &mut Vec<u8>, mut v: u64) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    while v > 0 {
        out.push((v % 10) as u8 + b'0');
        v /= 10;
    }
}

/// d rounded to a multiple of m, halfway away from zero: Go's
/// Duration.Round.
pub fn round_to(d: Duration, m: Duration) -> Duration {
    let (d, m) = (d.as_nanos(), m.as_nanos());
    if m == 0 {
        return Duration::from_nanos(d as u64);
    }
    let r = d % m;
    let down = d - r;
    let out = if r + r < m { down } else { down + m };
    Duration::from_nanos(u64::try_from(out).unwrap_or(u64::MAX))
}

/// d cut down to a multiple of m: Go's Duration.Truncate.
pub fn truncate(d: Duration, m: Duration) -> Duration {
    let (dn, mn) = (d.as_nanos(), m.as_nanos());
    if mn == 0 {
        return d;
    }
    Duration::from_nanos(u64::try_from(dn - dn % mn).unwrap_or(u64::MAX))
}

/// Parses a duration as Go's time.ParseDuration does ("25m", "1h30m",
/// "1.5s", "0"), as a signed count of nanoseconds. None when Go refuses it.
pub fn parse_duration(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if let Some(&c) = b.first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        i = 1;
    }
    if &b[i..] == b"0" {
        return Some(0);
    }
    if i == b.len() {
        return None;
    }
    let mut total: u64 = 0;
    while i < b.len() {
        // The number: digits, perhaps a point and more digits.
        if !(b[i] == b'.' || b[i].is_ascii_digit()) {
            return None;
        }
        let start = i;
        let mut v: u64 = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            v = v.checked_mul(10)?.checked_add((b[i] - b'0') as u64)?;
            if v > 1 << 63 {
                return None;
            }
            i += 1;
        }
        let pre = i != start;
        let mut f: u64 = 0;
        let mut scale: f64 = 1.0;
        let mut post = false;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let fs = i;
            let mut overflow = false;
            while i < b.len() && b[i].is_ascii_digit() {
                if !overflow {
                    if f > (1 << 63) / 10 {
                        overflow = true;
                    } else {
                        f = f * 10 + (b[i] - b'0') as u64;
                        scale *= 10.0;
                    }
                }
                i += 1;
            }
            post = i != fs;
        }
        if !pre && !post {
            return None;
        }
        // The unit.
        let us = i;
        while i < b.len() && b[i] != b'.' && !b[i].is_ascii_digit() {
            i += 1;
        }
        let unit: u64 = match &s[us..i] {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60 * 1_000_000_000,
            "h" => 3600 * 1_000_000_000,
            _ => return None,
        };
        if v > (1 << 63) / unit {
            return None;
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1 << 63 {
                return None;
            }
        }
        total = total.checked_add(v)?;
        if total > 1 << 63 {
            return None;
        }
    }
    if neg {
        return Some((total as i128).wrapping_neg() as i64);
    }
    if total > i64::MAX as u64 {
        return None;
    }
    Some(total as i64)
}

/// s quoted as Go's %q (strconv.Quote) writes it.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if c == '\u{FFFD}' || !c.is_control() && !is_unprintable(c) => out.push(c),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// The few printable-looking runes Go's IsPrint refuses: the spaces other
/// than U+0020, and the format characters.
fn is_unprintable(c: char) -> bool {
    matches!(c as u32,
        0x80..=0xA0 | 0xAD | 0x1680 | 0x2000..=0x200F | 0x2028..=0x202F | 0x205F..=0x206F
        | 0x3000 | 0xFEFF | 0xFFF9..=0xFFFB)
}

/// A commit's first seven characters.
pub fn short(sha: &str) -> &str {
    sha.get(..7).filter(|_| sha.len() > 7).unwrap_or(sha)
}

/// s cut to n runes, its last an ellipsis.
pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        return out;
    }
    s.to_string()
}

/// d for people: 40s, 14m, 1h5m.
pub fn round(d: Duration) -> String {
    if d < Duration::from_secs(60) {
        return format!("{}s", d.as_secs());
    }
    if d < Duration::from_secs(3600) {
        return format!("{}m", d.as_secs() / 60);
    }
    let s = duration(truncate(d, Duration::from_secs(60)));
    s.strip_suffix("0s").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_as_go_writes_them() {
        for (ns, want) in [
            (0i64, "0s"),
            (1, "1ns"),
            (1_100, "1.1µs"),
            (2_200_000, "2.2ms"),
            (1_500_000_000, "1.5s"),
            (105_000_000_000, "1m45s"),
            (2_700_000_000_000, "45m0s"),
            (3_900_000_000_000, "1h5m0s"),
            (-60_000_000_000, "-1m0s"),
        ] {
            assert_eq!(duration_ns(ns), want, "{ns}");
        }
    }

    #[test]
    fn parses_as_go() {
        for (s, want) in [
            ("25m", Some(1_500_000_000_000i64)),
            ("0", Some(0)),
            ("1h30m", Some(5_400_000_000_000)),
            ("1.5s", Some(1_500_000_000)),
            ("-5m", Some(-300_000_000_000)),
            ("300ms", Some(300_000_000)),
            ("5", None),
            ("", None),
            ("m", None),
            ("1x", None),
        ] {
            assert_eq!(parse_duration(s), want, "{s:?}");
        }
    }

    #[test]
    fn rounds_for_people() {
        for (secs, want) in [
            (40, "40s"),
            (14 * 60 + 5, "14m"),
            (3900, "1h5m"),
            (7200, "2h0m"),
        ] {
            assert_eq!(round(Duration::from_secs(secs)), want);
        }
    }

    #[test]
    fn quotes_as_go() {
        assert_eq!(quote("a\"b\\c\n\u{1}é"), "\"a\\\"b\\\\c\\n\\x01é\"");
    }
}
