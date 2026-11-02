//! A time as Go's time.Time keeps it in rush's files: an instant to the
//! nanosecond and the zone offset it was written in, so a file Go wrote
//! reads and writes back the same ("2026-10-28T14:04:56.5+01:30"). Two
//! times compare by instant alone, as Go's Equal, Before and After do.

use rush_core::time::Ms;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;

/// Seconds from 0001-01-01 to 1970-01-01: Go's zero time.
const ZERO_UNIX: i64 = -62_135_596_800;

#[derive(Clone, Copy)]
pub struct Time {
    unix: i64,
    nanos: u32,
    offset: i32, // seconds east of UTC
}

impl Default for Time {
    /// Go's zero time, 0001-01-01T00:00:00Z: what omitzero leaves out.
    fn default() -> Self {
        Time {
            unix: ZERO_UNIX,
            nanos: 0,
            offset: 0,
        }
    }
}

impl Time {
    /// Now, in the local zone, as Go's time.Now writes it.
    pub fn now() -> Time {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let unix = d.as_secs() as i64;
        Time {
            unix,
            nanos: d.subsec_nanos(),
            offset: local_offset(unix),
        }
    }

    /// The instant unix seconds and nanos past, in UTC.
    pub fn from_unix(unix: i64, nanos: u32) -> Time {
        let mut t = Time {
            unix,
            nanos: 0,
            offset: 0,
        };
        t = t.add_nanos(nanos as i64);
        t
    }

    /// The instant ms Unix milliseconds name, in UTC; 0 is the zero time.
    pub fn from_ms(ms: Ms) -> Time {
        if ms == 0 {
            return Time::default();
        }
        Time::from_unix(
            ms.div_euclid(1000),
            (ms.rem_euclid(1000) * 1_000_000) as u32,
        )
    }

    /// Unix milliseconds; 0 for the zero time, as rush_core counts unknown.
    pub fn to_ms(self) -> Ms {
        if self.is_zero() {
            return 0;
        }
        self.unix * 1000 + (self.nanos / 1_000_000) as i64
    }

    /// Unix nanoseconds.
    pub fn unix_nanos(self) -> i128 {
        self.unix as i128 * 1_000_000_000 + self.nanos as i128
    }

    pub fn is_zero(self) -> bool {
        self.unix == ZERO_UNIX && self.nanos == 0
    }

    pub fn after(self, o: Time) -> bool {
        self > o
    }

    pub fn before(self, o: Time) -> bool {
        self < o
    }

    /// t moved by n nanoseconds, in the same zone.
    pub fn add_nanos(self, n: i64) -> Time {
        // In i128, so no n can overflow it; the seconds saturate.
        let total = self.nanos as i128 + n as i128;
        let secs = self.unix as i128 + total.div_euclid(1_000_000_000);
        Time {
            unix: secs.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
            nanos: total.rem_euclid(1_000_000_000) as u32,
            offset: self.offset,
        }
    }

    /// The time from o to t, in nanoseconds.
    pub fn sub_nanos(self, o: Time) -> i128 {
        self.unix_nanos() - o.unix_nanos()
    }

    /// The same instant, written in the local zone, as Go's Local.
    pub fn local(self) -> Time {
        if self.is_zero() {
            return self;
        }
        Time {
            offset: local_offset(self.unix),
            ..self
        }
    }

    /// The same instant, written in the zone offset seconds east of UTC.
    pub fn with_offset(self, offset: i32) -> Time {
        Time { offset, ..self }
    }

    /// Parses RFC 3339 as Go's time.Time reads it from JSON:
    /// `2006-01-02T15:04:05.999999999Z07:00`.
    pub fn parse(s: &str) -> Option<Time> {
        let b = s.as_bytes();
        if b.len() < 20
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
        {
            return None;
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
        let (y, mo, d, h, mi, sec) = (
            num(0..4)?,
            num(5..7)?,
            num(8..10)?,
            num(11..13)?,
            num(14..16)?,
            num(17..19)?,
        );
        if !(1..=12).contains(&mo)
            || !(1..=days_in_month(y, mo)).contains(&d)
            || h > 23
            || mi > 59
            || sec > 59
        {
            return None;
        }
        let mut i = 19;
        let mut nanos = 0u32;
        if b[i] == b'.' {
            i += 1;
            let start = i;
            let mut scale = 100_000_000u32;
            while i < b.len() && b[i].is_ascii_digit() {
                if i - start < 9 {
                    nanos += (b[i] - b'0') as u32 * scale;
                    scale /= 10;
                }
                i += 1;
            }
            if i == start {
                return None;
            }
        }
        let offset = match b.get(i)? {
            b'Z' if i + 1 == b.len() => 0,
            c @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
                let sign = if *c == b'-' { -1 } else { 1 };
                let (oh, om) = (num(i + 1..i + 3)?, num(i + 4..i + 6)?);
                if oh > 23 || om > 59 {
                    return None;
                }
                sign * (oh * 3600 + om * 60) as i32
            }
            _ => return None,
        };
        let unix = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset as i64;
        Some(Time {
            unix,
            nanos,
            offset,
        })
    }

    /// RFC 3339 with nanoseconds, trailing zeros trimmed, as Go's
    /// RFC3339Nano: "Z" for a zero offset.
    pub fn format(self) -> String {
        let local = self.unix + self.offset as i64;
        let days = local.div_euclid(86_400);
        let secs = local.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        let mut s = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            y,
            m,
            d,
            secs / 3600,
            secs / 60 % 60,
            secs % 60
        );
        if self.nanos != 0 {
            let frac = format!("{:09}", self.nanos);
            s.push('.');
            s.push_str(frac.trim_end_matches('0'));
        }
        if self.offset == 0 {
            s.push('Z');
        } else {
            let o = self.offset.abs();
            s.push(if self.offset < 0 { '-' } else { '+' });
            s.push_str(&format!("{:02}:{:02}", o / 3600, o / 60 % 60));
        }
        s
    }
}

/// t moved on by d.
impl std::ops::Add<std::time::Duration> for Time {
    type Output = Time;
    fn add(self, d: std::time::Duration) -> Time {
        self.add_nanos(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
    }
}

impl PartialEq for Time {
    fn eq(&self, o: &Time) -> bool {
        self.unix == o.unix && self.nanos == o.nanos
    }
}

impl Eq for Time {}

impl PartialOrd for Time {
    fn partial_cmp(&self, o: &Time) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Time {
    fn cmp(&self, o: &Time) -> Ordering {
        (self.unix, self.nanos).cmp(&(o.unix, o.nanos))
    }
}

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.format())
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        Time::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a time: {s:?}")))
    }
}

/// The local zone's offset at unix, in seconds east of UTC.
fn local_offset(unix: i64) -> i32 {
    let t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes only tm, which lives on this stack.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i32
}

/// How many days month m of year y has: Go rejects a day past it.
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date days after 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Local: the same instant, in the zone Go's time.Now writes.
    #[test]
    fn local_keeps_the_instant_in_this_zone() {
        let t = Time::from_unix(1_761_714_754, 468_226_000);
        let l = t.local();
        assert_eq!(l, t);
        assert_eq!(l.offset, local_offset(t.unix));
        assert!(Time::default().local().is_zero());
    }

    #[test]
    fn round_trips_as_go_writes() {
        for s in [
            "2026-10-28T12:34:56.123456789Z",
            "2026-10-28T14:04:56+01:30",
            "2026-10-28T05:34:56.023456789-07:00",
            "1999-01-02T03:04:05.1Z",
            "0001-01-01T00:00:00Z",
            "2026-10-28T12:00:00.0000005+01:00",
        ] {
            let t = Time::parse(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(t.format(), s);
        }
        assert!(Time::parse("0001-01-01T00:00:00Z").unwrap().is_zero());
        assert!(Time::default().is_zero());
        assert!(Time::parse("nope").is_none());
        assert!(Time::parse("2026-10-28T12:34:56").is_none());
        // A day the month hasn't, as Go's time.Parse refuses it.
        assert!(Time::parse("2026-02-30T00:00:00Z").is_none());
        assert!(Time::parse("2023-02-29T00:00:00Z").is_none());
        assert!(Time::parse("2024-02-29T00:00:00Z").is_some());
        // No addition overflows.
        let t = Time::parse("2026-10-28T12:00:00Z").unwrap();
        assert!(t.add_nanos(i64::MAX).after(t) && t.add_nanos(i64::MIN).before(t));
        assert!((t + std::time::Duration::MAX).after(t));
    }

    #[test]
    fn compares_by_instant() {
        let a = Time::parse("2026-10-28T13:00:00+01:00").unwrap();
        let b = Time::parse("2026-10-28T12:00:00Z").unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.to_ms(),
            rush_core::time::parse_rfc3339("2026-10-28T12:00:00Z")
        );
        assert!(a.add_nanos(1).after(b));
        assert_eq!(Time::from_ms(a.to_ms()), a);
        assert_eq!(Time::from_ms(0).to_ms(), 0);
    }
}
