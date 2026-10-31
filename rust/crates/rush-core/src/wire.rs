//! JSON as Go rush writes it (photon's jsonx: encoding/json/v2 with v1's
//! habits), so Go and Rust read and write the same files and lines:
//!
//! - fields are written whether set or not, under their Go names;
//! - a nil slice or map is `null`, and `null` reads back empty;
//! - a time is RFC 3339, the zero time `0001-01-01T00:00:00Z` (Ms 0 here);
//! - a duration is a number of nanoseconds;
//! - a float with nothing after the point is written as a whole number;
//! - bytes are base64.
//!
//! Use these with `#[serde(with = "...")]`.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::time::{Ms, format_rfc3339, parse_rfc3339};

/// Go's zero time, as it writes it.
pub const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// A time.Time as Unix milliseconds; 0 is Go's zero time.
pub mod time {
    use super::*;

    pub fn serialize<S: Serializer>(ms: &Ms, s: S) -> Result<S::Ok, S::Error> {
        if *ms == 0 {
            return s.serialize_str(ZERO_TIME);
        }
        s.serialize_str(&format_rfc3339(*ms))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Ms, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        Ok(s.map_or(0, |s| from_str(&s)))
    }

    /// A time Go wrote, as Ms: its zero time (any year before 1970's
    /// first) is 0.
    pub fn from_str(s: &str) -> Ms {
        if s.starts_with("0001-01-01T00:00:00") {
            return 0;
        }
        parse_rfc3339(s)
    }
}

/// A time.Time as it was written: its words kept, so one Go wrote (in its
/// zone, to the microsecond) is written back the same. Empty is Go's zero
/// time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp(String);

impl Stamp {
    /// The time text names, as written.
    pub fn new(text: impl Into<String>) -> Stamp {
        Stamp(text.into())
    }

    /// Its words; Go's zero time when it has none.
    pub fn text(&self) -> &str {
        if self.0.is_empty() {
            ZERO_TIME
        } else {
            &self.0
        }
    }

    /// As Unix milliseconds; 0 for the zero time.
    pub fn ms(&self) -> Ms {
        time::from_str(&self.0)
    }
}

/// ms in UTC, as a Stamp (0 is the zero time).
impl From<Ms> for Stamp {
    fn from(ms: Ms) -> Stamp {
        if ms == 0 {
            return Stamp::default();
        }
        Stamp(format_rfc3339(ms))
    }
}

impl Serialize for Stamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.text())
    }
}

impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Stamp, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        Ok(Stamp(
            s.filter(|s| !s.starts_with("0001-01-01T00:00:00"))
                .unwrap_or_default(),
        ))
    }
}

/// A time.Duration: nanoseconds as a number.
pub mod duration {
    use super::*;
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(d.as_nanos().min(i64::MAX as u128) as i64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let n: Option<i64> = Option::deserialize(d)?;
        Ok(Duration::from_nanos(n.unwrap_or(0).max(0) as u64))
    }
}

/// A slice: empty is written `null`, as Go writes a nil one, and `null`
/// reads back empty.
pub mod nullvec {
    use super::*;

    pub fn serialize<S: Serializer, T: Serialize>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
        if v.is_empty() {
            return s.serialize_none();
        }
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Vec<T>, D::Error> {
        Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
    }
}

/// A float64 as Go writes one: `12`, not `12.0`.
pub mod float {
    use super::*;

    pub fn serialize<S: Serializer>(f: &f64, s: S) -> Result<S::Ok, S::Error> {
        if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
            return s.serialize_i64(*f as i64);
        }
        s.serialize_f64(*f)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(0.0))
    }
}

/// A []byte: base64, `null` when empty.
pub mod bytes {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        if b.is_empty() {
            return s.serialize_none();
        }
        s.serialize_str(&STANDARD.encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        match Option::<String>::deserialize(d)? {
            None => Ok(Vec::new()),
            Some(s) => STANDARD.decode(s).map_err(D::Error::custom),
        }
    }
}

/// A string, read as empty when `null`.
pub mod string {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
    }
}

/// JSON kept as it was written (Go's jsontext.Value). Two are equal when
/// their text is.
#[derive(Clone, Debug)]
pub struct RawJson(pub Box<RawValue>);

impl RawJson {
    /// `s`, when it is one JSON value.
    pub fn new(s: &str) -> Result<RawJson, serde_json::Error> {
        RawValue::from_string(s.to_string()).map(RawJson)
    }

    pub fn get(&self) -> &str {
        self.0.get()
    }
}

impl PartialEq for RawJson {
    fn eq(&self, o: &RawJson) -> bool {
        self.get() == o.get()
    }
}

impl Serialize for RawJson {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for RawJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<RawJson, D::Error> {
        Box::<RawValue>::deserialize(d).map(RawJson)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A time is written back as it was read: Go's zone and digits kept;
    // the zero time reads as none and writes as Go's.
    #[test]
    fn a_stamp_keeps_its_words() {
        for (read, ms, back) in [
            (
                r#""2026-10-29T06:12:34.468226+01:00""#,
                1_793_250_754_468,
                r#""2026-10-29T06:12:34.468226+01:00""#,
            ),
            (r#""0001-01-01T00:00:00Z""#, 0, r#""0001-01-01T00:00:00Z""#),
            ("null", 0, r#""0001-01-01T00:00:00Z""#),
        ] {
            let s: Stamp = serde_json::from_str(read).unwrap();
            assert_eq!(s.ms(), ms, "{read}");
            assert_eq!(serde_json::to_string(&s).unwrap(), back, "{read}");
        }
        assert_eq!(Stamp::from(1_000).text(), "1970-01-01T00:00:01Z");
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
    #[serde(default)]
    struct T {
        #[serde(with = "time")]
        at: Ms,
        #[serde(with = "duration")]
        d: std::time::Duration,
        #[serde(with = "nullvec")]
        v: Vec<String>,
        #[serde(with = "float")]
        f: f64,
        #[serde(with = "bytes")]
        b: Vec<u8>,
    }

    #[test]
    fn writes_as_go_does() {
        for (t, want) in [
            (
                T::default(),
                r#"{"at":"0001-01-01T00:00:00Z","d":0,"v":null,"f":0,"b":null}"#,
            ),
            (
                T {
                    at: 1_790_510_400_000,
                    d: std::time::Duration::from_millis(1500),
                    v: vec!["a".into()],
                    f: 1.5,
                    b: b"png!".to_vec(),
                },
                r#"{"at":"2026-09-27T12:00:00Z","d":1500000000,"v":["a"],"f":1.5,"b":"cG5nIQ=="}"#,
            ),
        ] {
            let got = serde_json::to_string(&t).unwrap();
            assert_eq!(got, want);
            assert_eq!(serde_json::from_str::<T>(&got).unwrap(), t);
        }
        // Go's own nanoseconds and offsets read to the millisecond.
        let t: T = serde_json::from_str(r#"{"at":"2026-09-27T12:00:00.123456789+01:00"}"#).unwrap();
        assert_eq!(t.at, 1_790_506_800_123);
    }
}
