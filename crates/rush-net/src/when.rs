//! Times in files Go also reads and writes: RFC 3339 with up to nine
//! fraction digits, as Go's time.Time is in JSON. Go writes its local
//! offset; this writes UTC ("Z"), which Go reads the same. Nothing (or Go's
//! zero time) is None.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serializer};

/// The time as Go's RFC3339Nano writes it, in UTC: trailing fraction zeros
/// dropped, no fraction at all on a whole second.
pub(crate) fn format(t: SystemTime) -> String {
    let ns: i128 = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    };
    let secs = ns.div_euclid(1_000_000_000) as i64;
    let frac = ns.rem_euclid(1_000_000_000) as u32;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    let mut s = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        tod / 3600,
        tod / 60 % 60,
        tod % 60
    );
    if frac > 0 {
        let f = format!("{frac:09}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

/// Parses an RFC 3339 time with any offset and up to nine fraction digits;
/// None when it isn't one, or is Go's zero time.
pub(crate) fn parse(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        b[r].iter().try_fold(0i64, |n, &c| {
            c.is_ascii_digit().then(|| n * 10 + (c - b'0') as i64)
        })
    };
    let (y, mo, d, h, mi, sec) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0i64;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        let mut scale = 100_000_000;
        while i < b.len() && b[i].is_ascii_digit() {
            nanos += (b[i] - b'0') as i64 * scale;
            scale /= 10;
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let off = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        c @ (b'+' | b'-') if b.len() == i + 6 && b[i + 3] == b':' => {
            let sign = if *c == b'-' { -1 } else { 1 };
            sign * (num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60)
        }
        _ => return None,
    };
    let secs = days(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - off;
    if secs == days(1, 1, 1) * 86_400 && nanos == 0 {
        return None; // Go's zero time
    }
    if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos as u32))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(secs.unsigned_abs()))?
            .checked_add(Duration::from_nanos(nanos as u64))
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of days since 1970-01-01.
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// serde for an `Option<SystemTime>` field Go writes with omitzero.
pub(crate) mod opt {
    use super::*;

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&format(*t)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        Ok(s.as_deref().and_then(parse))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ns: i128) -> SystemTime {
        UNIX_EPOCH + Duration::from_nanos(ns as u64)
    }

    #[test]
    fn formats_as_go() {
        for (ns, want) in [
            (0, "1970-01-01T00:00:00Z"),
            (1_793_190_896_789_000_000, "2026-10-28T12:34:56.789Z"),
            (1_793_190_896_000_000_001, "2026-10-28T12:34:56.000000001Z"),
            (951_782_400_500_000_000, "2000-02-29T00:00:00.5Z"),
        ] {
            assert_eq!(format(at(ns)), want);
            assert_eq!(parse(want), Some(at(ns)), "{want}");
        }
    }

    #[test]
    fn reads_go() {
        for (s, want) in [
            (
                "2026-10-09T03:49:27.510218+02:00",
                Some(at(1_791_510_567_510_218_000)),
            ),
            (
                "2026-10-28T22:10:08.767571+01:00",
                Some(at(1_793_221_808_767_571_000)),
            ),
            (
                "2026-10-28T12:34:56.123456789Z",
                Some(at(1_793_190_896_123_456_789)),
            ),
            ("0001-01-01T00:00:00Z", None),
            ("", None),
            ("2026-10-28", None),
            ("2026-10-28T12:34:56", None),
            ("2026-13-28T12:34:56Z", None),
            ("2026-10-28T12:34:56.Z", None),
            ("2026-10-28T12:34:56+01", None),
            ("2026-10-28T12:34:56é", None),
        ] {
            assert_eq!(parse(s), want, "{s}");
        }
        // Far past 2554, where nanoseconds since 1970 no longer fit 64 bits,
        // and before 1970.
        let far = parse("3000-01-01T00:00:00.25Z").unwrap();
        assert_eq!(
            far.duration_since(UNIX_EPOCH).unwrap(),
            Duration::new(32_503_680_000, 250_000_000)
        );
        let before = parse("1969-12-31T23:59:59.25Z").unwrap();
        assert_eq!(
            UNIX_EPOCH.duration_since(before).unwrap(),
            Duration::from_millis(750)
        );
    }
}
