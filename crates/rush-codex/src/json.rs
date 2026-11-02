//! Reading Codex's JSON as Go's decoder (photon's jsonx) does: a field
//! that's missing, null or of another type reads as its zero value, and a
//! value kept whole keeps the bytes it was written in.

use std::borrow::Cow;
use std::collections::BTreeMap;

use rush_core::time::{Ms, parse_rfc3339};
use rush_core::wire::RawJson;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

/// A JSON string, borrowed when it has no escapes; null (or anything not a
/// string) reads as "".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Str<'a>(pub Cow<'a, str>);

impl std::ops::Deref for Str<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
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
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Str::default())
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Str::default())
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Str::default())
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Str::default())
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                while a.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(Str::default())
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                while a
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(Str::default())
            }
        }
        d.deserialize_any(V)
    }
}

/// One line of JSON with its type and time read, the rest left as it was
/// written: what every rollout line is.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct Envelope<'a> {
    #[serde(borrow)]
    pub timestamp: Str<'a>,
    #[serde(rename = "type", borrow)]
    pub typ: Str<'a>,
    #[serde(borrow)]
    pub payload: Option<&'a RawValue>,
}

/// The value at key, when v is an object that has it (and it isn't null).
pub(crate) fn get<'v>(v: &'v Value, key: &str) -> Option<&'v Value> {
    v.get(key).filter(|x| !x.is_null())
}

/// The string at key; "" for anything else.
pub(crate) fn s<'v>(v: &'v Value, key: &str) -> &'v str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The string at key, owned.
pub(crate) fn st(v: &Value, key: &str) -> String {
    s(v, key).to_string()
}

/// The whole number at key, when there is one.
pub(crate) fn int(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
    })
}

/// The number at key; 0 for anything else.
pub(crate) fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// The bool at key, when there is one.
pub(crate) fn boolean(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// The array at key; empty for anything else.
pub(crate) fn arr<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The strings of the array at key; anything else in it is "".
pub(crate) fn strs(v: &Value, key: &str) -> Vec<String> {
    arr(v, key)
        .iter()
        .map(|x| x.as_str().unwrap_or("").to_string())
        .collect()
}

/// v parsed, or Null when it isn't JSON.
pub(crate) fn parse(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

/// v as raw JSON, written as Go writes a map: keys in order.
pub(crate) fn raw_of(v: &Value) -> Option<RawJson> {
    RawJson::new(&serde_json::to_string(v).ok()?).ok()
}

/// A map of keys and values as raw JSON, its keys in order as Go writes a
/// map[string]any.
pub(crate) fn object(kv: &[(&str, Value)]) -> RawJson {
    let m: BTreeMap<&str, &Value> = kv.iter().map(|(k, v)| (*k, v)).collect();
    RawJson::new(&serde_json::to_string(&m).unwrap_or_else(|_| "{}".into())).expect("a map is JSON")
}

/// raw as RawJson, kept as written.
pub(crate) fn keep(raw: &RawValue) -> RawJson {
    RawJson(raw.to_owned())
}

/// An RFC 3339 time, or 0.
pub(crate) fn ms(s: &str) -> Ms {
    if s.is_empty() { 0 } else { parse_rfc3339(s) }
}

/// What JSON text is, trimmed: "null" is no value.
pub(crate) fn is_null(raw: &str) -> bool {
    raw.trim() == "null"
}
