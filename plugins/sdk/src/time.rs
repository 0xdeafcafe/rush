//! RFC 3339 times, as rush writes them (`"at"` in a `ui.event`) and as a
//! plugin may keep them, with no clock crate.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// t in UTC, as `2026-09-28T10:00:00Z`, with as many digits of the second
/// as it needs (`10:00:00.25Z`).
pub fn format(t: SystemTime) -> String {
    let (secs, nanos) = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let s = -(d.as_secs() as i64);
            match d.subsec_nanos() {
                0 => (s, 0),
                n => (s - 1, 1_000_000_000 - n),
            }
        }
    };
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let mut s = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    if nanos > 0 {
        let frac = format!("{nanos:09}");
        s.push('.');
        s.push_str(frac.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

/// Reads `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`; None if it isn't one.
pub fn parse(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = b.get(from..from + len)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(part).ok()?.parse().ok())?
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut nanos: u32 = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            if i - start < 9 {
                nanos = nanos * 10 + u32::from(b[i] - b'0');
            }
            i += 1;
        }
        if i == start {
            return None;
        }
        for _ in (i - start).min(9)..9 {
            nanos *= 10;
        }
    }
    let offset = match *b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        c @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let o = num(i + 1, 2)? * 3600 + num(i + 4, 2)? * 60;
            if c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    let t = if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    };
    Some(t + Duration::from_nanos(u64::from(nanos)))
}

/// Serializes a SystemTime as an RFC 3339 string:
/// `#[serde(with = "rush_plugin_sdk::time::rfc3339")]`.
pub mod rfc3339 {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::SystemTime;

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::format(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let s = String::deserialize(d)?;
        super::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a time: {s:?}")))
    }
}

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date days after 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let t = UNIX_EPOCH + Duration::from_secs(951_868_800);
        assert_eq!(format(t), "2000-03-01T00:00:00Z");
        assert_eq!(parse("2000-03-01T00:00:00Z"), Some(t));
        assert_eq!(parse("2000-03-01T01:30:00+01:30"), Some(t));
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(UNIX_EPOCH));
        let half = t + Duration::from_millis(500);
        assert_eq!(format(half), "2000-03-01T00:00:00.5Z");
        assert_eq!(parse("2000-03-01T00:00:00.500000000Z"), Some(half));
        let t = UNIX_EPOCH + Duration::new(1_790_589_600, 123_456_789);
        assert_eq!(parse(&format(t)), Some(t));
        assert_eq!(
            format(parse("2026-09-28T10:00:00Z").unwrap()),
            "2026-09-28T10:00:00Z"
        );
        assert_eq!(
            format(parse("2024-02-29T23:59:59-00:00").unwrap()),
            "2024-02-29T23:59:59Z"
        );
        assert_eq!(
            format(UNIX_EPOCH - Duration::from_secs(1)),
            "1969-12-31T23:59:59Z"
        );
        for bad in [
            "",
            "2000-03-01",
            "2000-13-01T00:00:00Z",
            "2000-03-01T00:00:00",
            "2000-03-01T00:00:00.Z",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
