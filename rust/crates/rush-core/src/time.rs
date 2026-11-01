//! Times as Unix milliseconds (0 = unknown), and the RFC 3339 timestamps
//! transcripts carry.

/// Unix time in milliseconds; 0 means unknown.
pub type Ms = i64;

/// Now, in Unix milliseconds.
pub fn now() -> Ms {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as Ms)
        .unwrap_or(0)
}

/// Parses `2026-10-28T12:34:56.789Z` (or with a `+hh:mm` offset, any number
/// of fraction digits) to Unix milliseconds; 0 when it isn't one.
pub fn parse_rfc3339(s: &str) -> Ms {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return 0;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let mut n = 0i64;
        for &c in &b[r] {
            if !c.is_ascii_digit() {
                return None;
            }
            n = n * 10 + (c - b'0') as i64;
        }
        Some(n)
    };
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (
        num(0..4),
        num(5..7),
        num(8..10),
        num(11..13),
        num(14..16),
        num(17..19),
    ) else {
        return 0;
    };
    let mut i = 19;
    let mut ms = 0i64;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut scale = 100;
        while i < b.len() && b[i].is_ascii_digit() {
            ms += (b[i] - b'0') as i64 * scale;
            scale /= 10;
            i += 1;
        }
    }
    let mut off = 0i64;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') && b.len() >= i + 6 {
        let sign = if b[i] == b'-' { -1 } else { 1 };
        if let (Some(oh), Some(om)) = (num(i + 1..i + 3), num(i + 4..i + 6)) {
            off = sign * (oh * 60 + om) * 60_000;
        }
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 86_400 + h * 3600 + mi * 60 + sec) * 1000 + ms) - off
}

/// `ms` as RFC 3339 in UTC, as Go's time.Time writes it in JSON: the
/// fraction only when there is one, its trailing zeros dropped
/// (`2026-10-28T12:34:56.789Z`, `2026-10-28T12:34:56Z`).
pub fn format_rfc3339(ms: Ms) -> String {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // The civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let mut s = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    );
    if frac != 0 {
        let f = format!("{frac:03}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

/// `ms` as RFC 3339 in this machine's zone, as Go's time.Time of a
/// time.Now() writes it in JSON (`2026-10-28T13:34:56.789+01:00`; `Z`
/// where the zone is UTC's).
pub fn format_rfc3339_local(ms: Ms) -> String {
    let off = local_offset(ms.div_euclid(1000));
    let mut s = format_rfc3339(ms + off * 1000);
    s.pop();
    if off == 0 {
        s.push('Z');
    } else {
        let a = off.abs();
        s.push_str(&format!(
            "{}{:02}:{:02}",
            if off < 0 { '-' } else { '+' },
            a / 3600,
            a / 60 % 60
        ));
    }
    s
}

/// This machine's offset from UTC at Unix time secs, in seconds.
fn local_offset(secs: i64) -> i64 {
    let t = secs as libc::time_t;
    // SAFETY: localtime_r writes only tm, which lives on this stack.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats() {
        for (ms, want) in [
            (0, "1970-01-01T00:00:00Z"),
            (1_793_190_896_789, "2026-10-28T12:34:56.789Z"),
            (1_793_190_896_700, "2026-10-28T12:34:56.7Z"),
            (1_793_190_896_000, "2026-10-28T12:34:56Z"),
            (951_782_400_000, "2000-02-29T00:00:00Z"),
        ] {
            assert_eq!(format_rfc3339(ms), want);
            assert_eq!(parse_rfc3339(want), ms);
        }
    }

    use super::*;

    /// A time in this machine's zone reads back as the same instant.
    #[test]
    fn formats_local() {
        for ms in [0, 1_793_190_896_789, 1_793_190_896_000, 951_782_400_000] {
            let s = format_rfc3339_local(ms);
            assert_eq!(parse_rfc3339(&s), ms, "{s}");
            assert!(
                s.ends_with('Z') || s[s.len() - 6..].starts_with(['+', '-']),
                "{s}"
            );
        }
    }

    #[test]
    fn parses() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), 0);
        assert_eq!(parse_rfc3339("2026-10-28T12:34:56.789Z"), 1_793_190_896_789);
        assert_eq!(
            parse_rfc3339("2026-10-28T13:34:56.789+01:00"),
            1_793_190_896_789
        );
        assert_eq!(parse_rfc3339("nope"), 0);
    }
}
