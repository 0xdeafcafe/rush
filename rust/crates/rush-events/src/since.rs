//! When a --since means, and the times a log's lines carry. Ported from Go
//! `internal/events/query.go:383` (ParseSince), with Go's
//! `time.ParseDuration` and `time.Parse(time.RFC3339Nano, …)` as rush uses
//! them.

use rush_core::time::Ms;

/// When a --since means: a duration back from now (90s, 10m, 2h, 3d) or a
/// time (2026-10-23T09:00:00Z, 2026-10-23). "" is no time (0).
pub fn parse_since(s: &str, now: Ms) -> Result<Ms, String> {
    if s.is_empty() {
        return Ok(0);
    }
    if let Some(n) = s.strip_suffix('d')
        && let Ok(d) = n.parse::<f64>()
    {
        let ns = (d * 86_400e9) as i64;
        return Ok(now - ns / 1_000_000);
    }
    if let Ok(ns) = parse_duration(s) {
        return Ok(now - ns / 1_000_000);
    }
    // Go's layouts take an hour of one digit, as 2026-10-23T9:00.
    let s2 = pad_hour(s);
    if let Some(t) = parse_rfc3339(&s2) {
        return Ok(t);
    }
    if let Some(t) = parse_local(&s2) {
        return Ok(t);
    }
    Err(format!(
        "{} is neither a while back (10m, 2h, 3d) nor a time (2026-10-23T09:00)",
        go_quote(s)
    ))
}

/// A duration as Go's time.ParseDuration reads it ("300ms", "-1.5h",
/// "2h45m"), in nanoseconds.
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let bad = || format!("time: invalid duration {}", go_quote(s));
    let mut rest = s;
    let mut neg = false;
    if let Some(r) = rest.strip_prefix('-') {
        neg = true;
        rest = r;
    } else if let Some(r) = rest.strip_prefix('+') {
        rest = r;
    }
    if rest == "0" {
        return Ok(0);
    }
    if rest.is_empty() {
        return Err(bad());
    }
    let mut total: u128 = 0;
    while !rest.is_empty() {
        let b = rest.as_bytes();
        if !(b[0] == b'.' || b[0].is_ascii_digit()) {
            return Err(bad());
        }
        // The whole part.
        let mut i = 0;
        let mut v: u128 = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            v = v * 10 + u128::from(b[i] - b'0');
            if v > 1u128 << 63 {
                return Err(bad());
            }
            i += 1;
        }
        let pre = i > 0;
        // The fraction.
        let (mut f, mut scale): (u128, u128) = (0, 1);
        let mut post = false;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let start = i;
            let mut overflow = false;
            while i < b.len() && b[i].is_ascii_digit() {
                if !overflow {
                    if f > (i64::MAX as u128) / 10 {
                        overflow = true;
                    } else {
                        f = f * 10 + u128::from(b[i] - b'0');
                        scale *= 10;
                    }
                }
                i += 1;
            }
            post = i > start;
        }
        if !pre && !post {
            return Err(bad()); // no digits, e.g. ".s"
        }
        // The unit.
        let u_start = i;
        while i < b.len() && b[i] != b'.' && !b[i].is_ascii_digit() {
            i += 1;
        }
        let unit: u128 = match &rest[u_start..i] {
            "ns" => 1,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            "" => return Err(format!("time: missing unit in duration {}", go_quote(s))),
            u => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    go_quote(u),
                    go_quote(s)
                ));
            }
        };
        let mut part = v * unit;
        if f > 0 {
            part += ((f as f64) * (unit as f64 / scale as f64)) as u128;
        }
        total += part;
        if total > 1u128 << 63 {
            return Err(bad());
        }
        rest = &rest[i..];
    }
    if neg {
        return Ok((total as i128).wrapping_neg() as i64);
    }
    if total > i64::MAX as u128 {
        return Err(bad());
    }
    Ok(total as i64)
}

/// s with a one-digit hour after its date made two, as Go's "15" reads
/// either.
fn pad_hour(s: &str) -> std::borrow::Cow<'_, str> {
    let b = s.as_bytes();
    if b.len() > 12 && b[10] == b'T' && b[11].is_ascii_digit() && b[12] == b':' {
        return format!("{}0{}", &s[..11], &s[11..]).into();
    }
    s.into()
}

/// A time as Go's time.Parse(time.RFC3339Nano, s) reads it, in Unix ms:
/// 2026-10-23T09:00:00Z, with any fraction, Z or ±hh:mm.
pub(crate) fn parse_rfc3339(s: &str) -> Option<Ms> {
    let b = s.as_bytes();
    if b.len() < 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = date(&b[..10])?;
    let h = num(&b[11..13])?;
    let mi = num(&b[14..16])?;
    let sec = num(&b[17..19])?;
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0;
    if b[i] == b'.' || b[i] == b',' {
        i += 1;
        let start = i;
        let mut scale = 100;
        while i < b.len() && b[i].is_ascii_digit() {
            ms += i64::from(b[i] - b'0') * scale;
            scale /= 10;
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let off = match &b[i..] {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let oh = num(&[*h1, *h2])?;
            let om = num(&[*m1, *m2])?;
            if oh > 23 || om > 59 {
                return None;
            }
            let o = (oh * 60 + om) * 60_000;
            if *sign == b'-' { -o } else { o }
        }
        _ => return None,
    };
    Some(((days(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec) * 1000 + ms) - off)
}

/// 2026-10-23T09:00 or 2026-10-23 in the machine's own zone, in Unix ms.
fn parse_local(s: &str) -> Option<Ms> {
    let b = s.as_bytes();
    let (y, mo, d) = date(b.get(..10)?)?;
    let (h, mi) = match b.len() {
        10 => (0, 0),
        16 if b[10] == b'T' && b[13] == b':' => {
            let (h, mi) = (num(&b[11..13])?, num(&b[14..16])?);
            if h > 23 || mi > 59 {
                return None;
            }
            (h, mi)
        }
        _ => return None,
    };
    local(y, mo, d, h, mi)
}

fn local(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> Option<Ms> {
    // SAFETY: tm is plain data, zeroed then filled; mktime only reads and
    // normalises it.
    let t = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = (y - 1900) as libc::c_int;
        tm.tm_mon = (mo - 1) as libc::c_int;
        tm.tm_mday = d as libc::c_int;
        tm.tm_hour = h as libc::c_int;
        tm.tm_min = mi as libc::c_int;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm)
    };
    if t == -1 {
        return None;
    }
    Some(t as i64 * 1000)
}

/// YYYY-MM-DD, its day one the month has.
fn date(b: &[u8]) -> Option<(i64, i64, i64)> {
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (num(&b[..4])?, num(&b[5..7])?, num(&b[8..10])?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let in_month = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if d < 1 || d > in_month {
        return None;
    }
    Some((y, mo, d))
}

fn num(b: &[u8]) -> Option<i64> {
    b.iter().try_fold(0i64, |n, &c| {
        c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0'))
    })
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days(y: i64, mo: i64, d: i64) -> i64 {
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// s quoted as Go's %q (strconv.Quote) does.
pub(crate) fn go_quote(s: &str) -> String {
    let mut out = String::from("\"");
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
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if !go_printable(c) => {
                if (c as u32) <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", c as u32))
                } else {
                    out.push_str(&format!("\\U{:08x}", c as u32))
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether Go's strconv.IsPrint says c is printable: not a control,
/// format, private-use or separator character but the space. (Unassigned
/// characters Go also escapes are shown as they are.)
fn go_printable(c: char) -> bool {
    let u = c as u32;
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(u,
        0xa0 | 0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x890..=0x891 | 0x8e2
        | 0x180e | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x2064
        | 0x2066..=0x206f | 0x3000 | 0xd800..=0xf8ff | 0xfeff | 0xfff9..=0xfffb
        | 0x110bd | 0x110cd | 0x13430..=0x1343f | 0x1bca0..=0x1bca3
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f | 0xf0000..)
}

#[cfg(test)]
mod tests {
    use super::*;

    // query_test.go:204
    #[test]
    fn parses_since() {
        let now = parse_rfc3339("2026-10-23T12:00:00Z").unwrap();
        for (input, want) in [
            ("10m", now - 10 * 60_000),
            ("3d", now - 72 * 3_600_000),
            ("1.5d", now - 36 * 3_600_000),
            ("90s", now - 90_000),
            ("2h45m", now - (2 * 60 + 45) * 60_000),
            ("2026-10-23T09:00:00Z", now - 3 * 3_600_000),
            ("2026-10-23T10:00:00.5+01:00", now - 3 * 3_600_000 + 500),
            ("", 0),
        ] {
            assert_eq!(parse_since(input, now), Ok(want), "{input}");
        }
        assert_eq!(
            parse_since("2026-10-23T9:00:00Z", now),
            Ok(now - 3 * 3_600_000)
        );
        assert_eq!(
            parse_since("2026-10-23T9:00", now),
            parse_since("2026-10-23T09:00", now)
        );
        for local in ["2026-10-23T09:00", "2026-10-23"] {
            assert!(parse_since(local, now).is_ok(), "{local}");
        }
        assert_eq!(
            parse_since("soon", now),
            Err(
                r#""soon" is neither a while back (10m, 2h, 3d) nor a time (2026-10-23T09:00)"#
                    .to_string()
            )
        );
        for bad in [
            "2026-02-30",
            "2026-10-23T25:00",
            "2026-10-23T09:00:00",
            "d",
            "1x",
        ] {
            assert!(parse_since(bad, now).is_err(), "{bad}");
        }
    }

    #[test]
    fn quotes_as_go_does() {
        assert_eq!(go_quote("a\"b\n\u{7f}"), r#""a\"b\n\x7f""#);
        assert_eq!(go_quote("x\u{a0}y\u{200b}é"), r#""x\u00a0y\u200bé""#);
    }

    #[test]
    fn durations_as_go_reads_them() {
        for (s, want) in [
            ("0", Some(0)),
            ("300ms", Some(300_000_000)),
            ("-1.5h", Some(-5_400_000_000_000)),
            ("1h30m", Some(5_400_000_000_000)),
            ("1µs", Some(1000)),
            (".5s", Some(500_000_000)),
            ("1", None),
            ("", None),
            ("1d", None),
            ("s", None),
            ("-9223372036854775808ns", Some(i64::MIN)),
            ("9223372036854775808ns", None),
        ] {
            assert_eq!(parse_duration(s).ok(), want, "{s}");
        }
    }
}
