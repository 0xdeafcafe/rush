//! Times as Go's time.Time writes them in JSON (RFC 3339, its zero time
//! "0001-01-01T00:00:00Z"), kept as Unix ms with 0 for the zero time, so
//! update.json reads the same from Go and Rust.

use rush_core::time::{Ms, parse_rfc3339};
use serde::{Deserialize, Deserializer, Serializer};

const ZERO: &str = "0001-01-01T00:00:00Z";

pub fn serialize<S: Serializer>(t: &Ms, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format(*t))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Ms, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.as_deref().map_or(0, parse))
}

/// t read from RFC 3339; 0 for Go's zero time or what isn't a time.
pub fn parse(s: &str) -> Ms {
    if s.starts_with("0001-01-01T00:00:00") {
        return 0;
    }
    parse_rfc3339(s)
}

/// t as Go's RFC3339Nano writes it in UTC: fraction without trailing zeros.
pub fn format(t: Ms) -> String {
    if t == 0 {
        return ZERO.to_string();
    }
    let secs = t.div_euclid(1000);
    let ms = t.rem_euclid(1000);
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    let mut out = format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}",
        sod / 3600,
        sod / 60 % 60,
        sod % 60
    );
    if ms != 0 {
        let f = format!("{ms:03}");
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out.push('Z');
    out
}

/// The civil date of a count of days since 1970-01-01 (Howard Hinnant's
/// algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
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

/// A pseudo-version's time, "20060102150405" in UTC; None when it isn't one
/// (Go's time.Parse fails).
pub fn parse_stamp(s: &str) -> Option<Ms> {
    if s.len() != 14 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(99);
    let (mo, d, h, mi, sec) = (n(4..6), n(6..8), n(8..10), n(10..12), n(12..14));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let iso = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[8..10],
        &s[10..12],
        &s[12..14]
    );
    Some(parse_rfc3339(&iso))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// t's day where you are, as Go's "Jan 2" writes it.
pub fn local_month_day(t: Ms) -> String {
    let secs = t.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r only writes the tm we give it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        let (_, mo, d) = civil(t.div_euclid(1000).div_euclid(86_400));
        return format!("{} {d}", MONTHS[(mo - 1) as usize]);
    }
    format!("{} {}", MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday)
}
