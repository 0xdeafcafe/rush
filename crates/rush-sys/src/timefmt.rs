//! Times as Go's `time.Time` writes them in JSON (RFC 3339 with
//! nanoseconds, trailing zeros dropped), in UTC, so files Rust writes read
//! the same in Go.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `t` as Go's `time.RFC3339Nano`, in UTC: `2026-10-28T12:34:56.789Z`.
pub fn rfc3339_nano(t: SystemTime) -> String {
    let (secs, nanos) = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let mut s = -(d.as_secs() as i64);
            let mut n = d.subsec_nanos();
            if n > 0 {
                s -= 1;
                n = 1_000_000_000 - n;
            }
            (s, n)
        }
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    let mut out = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    );
    if nanos > 0 {
        let f = format!("{nanos:09}");
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out.push('Z');
    out
}

/// Parses an RFC 3339 time (any offset, any fraction) to a SystemTime;
/// None when it isn't one. Millisecond precision.
pub fn parse(s: &str) -> Option<SystemTime> {
    let ms = rush_core::time::parse_rfc3339(s);
    if ms == 0 && !s.starts_with("1970-01-01T00:00:00") {
        return None;
    }
    if ms >= 0 {
        Some(UNIX_EPOCH + Duration::from_millis(ms as u64))
    } else {
        Some(UNIX_EPOCH - Duration::from_millis(ms.unsigned_abs()))
    }
}

/// Year, month and day of days since 1970-01-01 (Howard Hinnant's
/// algorithm).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_go() {
        for (ms, want) in [
            (0i64, "1970-01-01T00:00:00Z"),
            (1_793_190_896_789, "2026-10-28T12:34:56.789Z"),
            (1_793_190_896_000, "2026-10-28T12:34:56Z"),
            (951_782_400_500, "2000-02-29T00:00:00.5Z"),
        ] {
            let t = UNIX_EPOCH + Duration::from_millis(ms as u64);
            assert_eq!(rfc3339_nano(t), want);
            assert_eq!(parse(want), Some(t));
        }
        assert_eq!(parse("nope"), None);
    }
}
