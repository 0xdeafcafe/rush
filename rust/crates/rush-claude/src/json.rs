//! Small helpers for reading Claude Code's JSON lines cheaply: borrowed
//! strings, raw values decoded only when needed, null as the default (as
//! Go's decoder leaves a field untouched), and a local calendar day.

use std::borrow::Cow;

use rush_core::time::{Ms, parse_rfc3339};

use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

/// A JSON string, borrowed when it has no escapes; null reads as "".
/// (serde's own Cow<str> always copies.)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Str<'a>(pub Cow<'a, str>);

impl std::ops::Deref for Str<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl<'a> Str<'a> {
    pub fn into_owned(self) -> String {
        self.0.into_owned()
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Str<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Str<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string")
            }
            fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
                Ok(Str(Cow::Borrowed(v)))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(Str(Cow::Owned(v.to_owned())))
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(Str(Cow::Owned(v)))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Str::default())
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(Str::default())
            }
        }
        d.deserialize_any(V)
    }
}

/// Null (or missing, with `default`) as the type's zero value.
pub(crate) fn nd<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A raw value as a string, when it is one.
pub(crate) fn str_of<'a>(raw: Option<&'a RawValue>) -> Option<Cow<'a, str>> {
    let r = raw?;
    if !r.get().starts_with('"') {
        return None;
    }
    serde_json::from_str::<Str<'a>>(r.get()).ok().map(|s| s.0)
}

/// A raw value as a string, or "" when it isn't one.
pub(crate) fn str_or_empty<'a>(raw: Option<&'a RawValue>) -> Cow<'a, str> {
    str_of(raw).unwrap_or(Cow::Borrowed(""))
}

/// An RFC 3339 timestamp, or 0.
pub(crate) fn ms(s: &str) -> Ms {
    if s.is_empty() { 0 } else { parse_rfc3339(s) }
}

/// b's lines without their newlines, the last one too even when empty, as
/// Go's bytes.Split would give them.
pub(crate) fn lines(b: &[u8]) -> impl Iterator<Item = &[u8]> {
    b.split(|&c| c == b'\n')
}

pub(crate) fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}

/// The local calendar day of t, as yyyymmdd: what a day's spend is kept
/// under (Go's claude.Day).
pub fn day(t: Ms) -> u32 {
    let secs = t.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r only writes the tm we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        return 0;
    }
    ((tm.tm_year + 1900) as u32) * 10_000 + ((tm.tm_mon + 1) as u32) * 100 + tm.tm_mday as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_like_go() {
        let got: Vec<&[u8]> = lines(b"a\nb\n").collect();
        assert_eq!(got, vec![&b"a"[..], b"b", b""]);
    }

    #[test]
    fn strings_only() {
        let r: &RawValue = serde_json::from_str("\"x\\ny\"").unwrap();
        assert_eq!(str_of(Some(r)).as_deref(), Some("x\ny"));
        let r: &RawValue = serde_json::from_str("3").unwrap();
        assert_eq!(str_of(Some(r)), None);
    }
}
