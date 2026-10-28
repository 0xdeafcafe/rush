//! Unix milliseconds written as RFC 3339, the way Go's encoding/json writes
//! a time.Time (UTC here: Go reads any offset), and serde helpers for the
//! times and nulls gh and Go write.

use rush_core::time::{Ms, parse_rfc3339};
use serde::{Deserialize, Deserializer, Serializer};

/// `2026-10-23T12:00:00Z`, or `…:00.123Z` when there are milliseconds.
pub fn format(ms: Ms) -> String {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let (h, mi, s) = (tod / 3600, tod % 3600 / 60, tod % 60);
    if frac == 0 {
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    } else {
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{frac:03}Z")
    }
}

/// A JSON null read as the type's zero, as Go reads it.
pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// An RFC 3339 time read as Unix milliseconds; null or "" is 0.
pub fn de_time<'de, D: Deserializer<'de>>(d: D) -> Result<Ms, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.map(|s| parse_rfc3339(&s)).unwrap_or(0))
}

/// An RFC 3339 time read as Unix milliseconds, refusing one that isn't, as
/// Go's time.Time does: a hold whose time can't be read isn't sent at once.
/// null is 0.
pub fn de_time_strict<'de, D: Deserializer<'de>>(d: D) -> Result<Ms, D::Error> {
    let Some(s) = Option::<String>::deserialize(d)? else {
        return Ok(0);
    };
    let ms = parse_rfc3339(&s);
    // parse_rfc3339 says 0 for what it can't read; the epoch itself reads
    // as 0 too.
    if ms == 0 && !(s.starts_with("1970-01-01") || s.starts_with("1969-12-31")) {
        return Err(serde::de::Error::custom(format!(
            "parsing time {s:?} as RFC 3339"
        )));
    }
    Ok(ms)
}

pub fn ser_time<S: Serializer>(ms: &Ms, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format(*ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for (ms, want) in [
            (0, "1970-01-01T00:00:00Z"),
            (1_793_190_896_789, "2026-10-28T12:34:56.789Z"),
            (951_782_400_000, "2000-02-29T00:00:00Z"),
        ] {
            assert_eq!(format(ms), want);
            assert_eq!(parse_rfc3339(want), ms);
        }
    }
}
