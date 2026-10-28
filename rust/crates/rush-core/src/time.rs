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

#[cfg(test)]
mod tests {
    use super::*;

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
