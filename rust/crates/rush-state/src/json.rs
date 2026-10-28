//! JSON as Go rush reads and writes it (photon/jsonx: encoding/json/v2 with
//! v1's habits), so either rush can read what the other wrote, byte for
//! byte:
//!
//! - map keys in byte order, struct fields in declaration order;
//! - `<`, `>` and `&` as they are, control characters escaped as v2 does;
//! - floats as ES6 writes numbers (`1`, `0.35`, `1e+21`, `0.000001`);
//! - indented two spaces a level, `": "` after a name, `{}` and `[]` empty;
//! - a raw value kept as written, only its whitespace made to fit.
//!
//! Reading is as forgiving as jsonx: a repeated key's last value wins, a
//! string cut mid-character still reads, and (beyond Go) one field of the
//! wrong type or null reads as its zero rather than spoiling the rest.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::ser::{CharEscape, Formatter};
use std::io::{self, Write};

/// v is JSON, compact, as `jsonx.Marshal` writes it.
pub fn to_vec<T: Serialize + ?Sized>(v: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, GoFormatter::compact());
    v.serialize(&mut ser)?;
    Ok(out)
}

/// v is JSON written to w as it's made, compact: `jsonx.MarshalWrite`.
pub fn to_writer<W: Write, T: Serialize + ?Sized>(w: W, v: &T) -> serde_json::Result<()> {
    let mut ser = serde_json::Serializer::with_formatter(w, GoFormatter::compact());
    v.serialize(&mut ser)
}

/// v is JSON indented two spaces a level, as `jsonx.MarshalIndent` writes
/// it: no newline at the end.
pub fn to_vec_indent<T: Serialize + ?Sized>(v: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(512);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, GoFormatter::indented());
    v.serialize(&mut ser)?;
    Ok(out)
}

/// Reads the JSON in b into a T, as `jsonx.Unmarshal` does: a repeated
/// key's last value wins, and invalid UTF-8 reads as U+FFFD.
pub fn from_slice<T: DeserializeOwned>(b: &[u8]) -> serde_json::Result<T> {
    match serde_json::from_slice(b) {
        Ok(v) => Ok(v),
        Err(first) => {
            // A repeated key, or a string cut mid-character: read it whole
            // as a value first, where the last of a key wins.
            let text = String::from_utf8_lossy(b);
            let v: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => return Err(first),
            };
            serde_json::from_value(v).map_err(|_| first)
        }
    }
}

/// Whether b is one JSON value: `jsonx.Valid`.
pub fn valid(b: &[u8]) -> bool {
    match std::str::from_utf8(b) {
        Ok(s) => serde_json::from_str::<serde::de::IgnoredAny>(s).is_ok(),
        Err(_) => {
            serde_json::from_str::<serde::de::IgnoredAny>(&String::from_utf8_lossy(b)).is_ok()
        }
    }
}

/// The JSON in b indented two spaces a level, its strings and numbers as
/// they were written: `jsonx.Indent`. None when b isn't JSON.
pub fn indent(b: &[u8]) -> Option<Vec<u8>> {
    if !valid(b) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() + b.len() / 4);
    reformat(b, Some(0), &mut out);
    Some(out)
}

/// Writes the JSON in src to out with its whitespace made over: indented
/// two spaces a level, starting at depth, or compact when depth is None.
/// Strings and numbers are copied as they are. src must be valid.
fn reformat(src: &[u8], depth: Option<usize>, out: &mut Vec<u8>) {
    let mut depth = depth;
    let newline = |out: &mut Vec<u8>, d: Option<usize>| {
        if let Some(d) = d {
            out.push(b'\n');
            for _ in 0..d {
                out.extend_from_slice(b"  ");
            }
        }
    };
    let next_non_ws = |i: usize| -> Option<u8> {
        src[i..]
            .iter()
            .copied()
            .find(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
    };
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'{' | b'[' => {
                let close = if c == b'{' { b'}' } else { b']' };
                out.push(c);
                i += 1;
                if next_non_ws(i) == Some(close) {
                    // Empty: {} or [], on one line.
                    while src[i] != close {
                        i += 1;
                    }
                    out.push(close);
                    i += 1;
                    continue;
                }
                depth = depth.map(|d| d + 1);
                newline(out, depth);
            }
            b'}' | b']' => {
                depth = depth.map(|d| d.saturating_sub(1));
                newline(out, depth);
                out.push(c);
                i += 1;
            }
            b',' => {
                out.push(b',');
                newline(out, depth);
                i += 1;
            }
            b':' => {
                out.push(b':');
                if depth.is_some() {
                    out.push(b' ');
                }
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < src.len() {
                    match src[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                out.extend_from_slice(&src[start..i.min(src.len())]);
            }
            _ => {
                // A number, true, false or null: as written.
                let start = i;
                while i < src.len()
                    && !matches!(
                        src[i],
                        b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' | b'}' | b']'
                    )
                {
                    i += 1;
                }
                out.extend_from_slice(&src[start..i]);
            }
        }
    }
}

/// serde_json's formatter, writing as Go's encoding/json/v2 does with
/// jsonx's options.
pub struct GoFormatter {
    indent: bool,
    depth: usize,
    has_value: bool,
}

impl GoFormatter {
    pub fn compact() -> Self {
        GoFormatter {
            indent: false,
            depth: 0,
            has_value: false,
        }
    }

    pub fn indented() -> Self {
        GoFormatter {
            indent: true,
            depth: 0,
            has_value: false,
        }
    }

    fn line<W: ?Sized + Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(b"\n")?;
        for _ in 0..self.depth {
            w.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl Formatter for GoFormatter {
    fn write_f64<W: ?Sized + Write>(&mut self, w: &mut W, v: f64) -> io::Result<()> {
        w.write_all(format_float(v).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, w: &mut W, v: f32) -> io::Result<()> {
        // Go formats a float32 by its own shortest digits.
        w.write_all(es6(&format!("{v:e}")).as_bytes())
    }

    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        w: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        // As v2 escapes: the short forms, else \u00xx in lower case.
        let s: &[u8] = match escape {
            CharEscape::Quote => b"\\\"",
            CharEscape::ReverseSolidus => b"\\\\",
            CharEscape::Solidus => b"\\/",
            CharEscape::Backspace => b"\\b",
            CharEscape::FormFeed => b"\\f",
            CharEscape::LineFeed => b"\\n",
            CharEscape::CarriageReturn => b"\\r",
            CharEscape::Tab => b"\\t",
            CharEscape::AsciiControl(c) => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let b = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[(c >> 4) as usize],
                    HEX[(c & 0xf) as usize],
                ];
                return w.write_all(&b);
            }
        };
        w.write_all(s)
    }

    fn begin_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.depth += 1;
        self.has_value = false;
        w.write_all(b"[")
    }

    fn end_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.depth -= 1;
        if self.indent && self.has_value {
            self.line(w)?;
        }
        self.has_value = true;
        w.write_all(b"]")
    }

    fn begin_array_value<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        if self.indent {
            self.line(w)?;
        }
        Ok(())
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }

    fn begin_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.depth += 1;
        self.has_value = false;
        w.write_all(b"{")
    }

    fn end_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.depth -= 1;
        if self.indent && self.has_value {
            self.line(w)?;
        }
        self.has_value = true;
        w.write_all(b"}")
    }

    fn begin_object_key<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        if self.indent {
            self.line(w)?;
        }
        Ok(())
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(if self.indent { b": " } else { b":" })
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }

    fn write_raw_fragment<W: ?Sized + Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        // A raw value's whitespace is made over to fit, as v2 does; its
        // strings and numbers stay as written.
        let mut out = Vec::with_capacity(fragment.len() + 16);
        reformat(
            fragment.as_bytes(),
            self.indent.then_some(self.depth),
            &mut out,
        );
        w.write_all(&out)
    }
}

/// v as Go writes a float64: ES6's way, but -0 stays "-0".
pub fn format_float(v: f64) -> String {
    if !v.is_finite() {
        return "null".into(); // Go refuses these; never written by rush
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    es6(&format!("{v:e}"))
}

/// ES6's Number toString from Rust's shortest `{:e}` form ("3.5e-1").
fn es6(sci: &str) -> String {
    let (neg, sci) = match sci.strip_prefix('-') {
        Some(s) => (true, s),
        None => (false, sci),
    };
    let (mant, exp) = sci.split_once('e').unwrap_or((sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1; // where the point goes
    let mut s = String::with_capacity(24);
    if neg {
        s.push('-');
    }
    if k <= n && n <= 21 {
        s.push_str(&digits);
        for _ in 0..(n - k) {
            s.push('0');
        }
    } else if 0 < n && n <= 21 {
        s.push_str(&digits[..n as usize]);
        s.push('.');
        s.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        s.push_str("0.");
        for _ in 0..(-n) {
            s.push('0');
        }
        s.push_str(&digits);
    } else {
        s.push_str(&digits[..1]);
        if k > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if n - 1 < 0 { '-' } else { '+' });
        s.push_str(&(n - 1).abs().to_string());
    }
    s
}

// What Go's omitempty and omitzero leave out, for skip_serializing_if.

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

pub(crate) fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

/// omitzero on a float: only +0, as Go's IsZero has it.
pub(crate) fn is_zero_f64(f: &f64) -> bool {
    f.to_bits() == 0
}

pub(crate) fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Reads a field as T, or T's zero when it's null or of another type,
/// rather than failing the whole file: a hand edit gone wrong costs that
/// setting, not every other one when it's saved over.
pub(crate) fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    // Through a raw value rather than a Value, so raw values inside keep
    // their numbers as written.
    let raw = Box::<serde_json::value::RawValue>::deserialize(d)?;
    Ok(from_slice(raw.get().as_bytes()).unwrap_or_default())
}

/// A JSON value kept as it was written: `jsontext.Value`. Two are equal
/// when they were written the same.
#[derive(Clone, Debug)]
pub struct RawJson(pub Box<serde_json::value::RawValue>);

impl RawJson {
    /// v as a raw value, written compact.
    pub fn from_value<T: Serialize + ?Sized>(v: &T) -> serde_json::Result<RawJson> {
        let b = to_vec(v)?;
        let s = String::from_utf8(b).map_err(serde::ser::Error::custom)?;
        serde_json::value::RawValue::from_string(s).map(RawJson)
    }

    /// The value as it was written.
    pub fn get(&self) -> &str {
        self.0.get()
    }
}

impl PartialEq for RawJson {
    fn eq(&self, o: &RawJson) -> bool {
        self.0.get() == o.0.get()
    }
}

impl Serialize for RawJson {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for RawJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<RawJson, D::Error> {
        Box::<serde_json::value::RawValue>::deserialize(d).map(RawJson)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_as_go_writes_them() {
        for (f, want) in [
            (0.35, "0.35"),
            (1.0, "1"),
            (0.3, "0.3"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (-2.5, "-2.5"),
            (123456789.125, "123456789.125"),
            (0.0, "0"),
            (-0.0, "-0"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (100.0, "100"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.5e-7, "1.5e-7"),
        ] {
            assert_eq!(format_float(f), want, "{f}");
        }
    }

    #[test]
    fn indent_keeps_tokens() {
        let b = br#"{"a" : 1.50e+2, "b":[ ], "c":{"d":"\/&"}, "e":[1,{}]}"#;
        let got = String::from_utf8(indent(b).unwrap()).unwrap();
        assert_eq!(
            got,
            "{\n  \"a\": 1.50e+2,\n  \"b\": [],\n  \"c\": {\n    \"d\": \"\\/&\"\n  },\n  \"e\": [\n    1,\n    {}\n  ]\n}"
        );
        assert!(indent(b"{").is_none());
    }

    #[test]
    fn raw_values_read_through_a_value() {
        #[derive(Deserialize, Serialize)]
        struct S {
            #[serde(default, deserialize_with = "lenient")]
            v: Vec<L>,
        }
        #[derive(Deserialize, Serialize, Default)]
        struct L {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            p: Option<RawJson>,
        }
        let s: S = from_slice(br#"{"v":[{"p":{"n":1.50}},{"p":null},{}]}"#).unwrap();
        assert_eq!(s.v[0].p.as_ref().unwrap().get(), r#"{"n":1.50}"#);
        assert!(s.v[1].p.is_none() && s.v[2].p.is_none());
        let out = String::from_utf8(to_vec_indent(&s).unwrap()).unwrap();
        assert_eq!(
            out,
            "{\n  \"v\": [\n    {\n      \"p\": {\n        \"n\": 1.50\n      }\n    },\n    {},\n    {}\n  ]\n}"
        );
    }

    #[test]
    fn repeated_keys_last_wins() {
        #[derive(Deserialize)]
        struct S {
            a: String,
        }
        let s: S = from_slice(br#"{"a":"one","a":"two"}"#).unwrap();
        assert_eq!(s.a, "two");
        let s: S = from_slice(b"{\"a\":\"x\xffy\"}").unwrap();
        assert_eq!(s.a, "x\u{fffd}y");
        assert!(valid(b"{\"a\":\"x\xffy\"}"));
        assert!(!valid(b"{} x"));
    }
}
