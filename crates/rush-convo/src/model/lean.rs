//! A Claude Code call's input kept once (Rust-only). Its raw JSON says
//! again what rush read from it (the command, the file's contents, the
//! prompt): the step keeps the JSON with each such string cut out, a
//! byte no JSON text holds in its place saying which of the call's own
//! fields goes there, and puts the JSON back together when it's asked
//! for. Only when that gives back the very bytes the agent sent; else the
//! raw stays as it came.

use rush_core::tool::{Call, Input};
use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;

/// The call's fields a string of its input can be: a hole's byte is its
/// place here, plus one.
const FIELDS: [fn(&Input) -> Option<&str>; 10] = [
    |i| Some(&i.command),
    |i| Some(&i.description),
    |i| Some(&i.prompt),
    |i| Some(&i.content),
    |i| Some(&i.path),
    |i| Some(&i.pattern),
    |i| Some(&i.query),
    |i| Some(&i.url),
    |i| one_edit(i).map(|e| e.old.as_str()),
    |i| one_edit(i).map(|e| e.new.as_str()),
];

/// A string shorter than this is left in the JSON: cutting it out saves
/// less than finding it costs.
const LEAST: usize = 24;

fn one_edit(i: &Input) -> Option<&rush_core::tool::Replace> {
    match &i.edits[..] {
        [e] => Some(e),
        _ => None,
    }
}

/// The most values of an object looked at: a call's input has a few.
const MOST: usize = 16;

/// An object's values, as they lie in its JSON; None for one with more
/// than MOST.
struct Values<'a>([Option<&'a RawValue>; MOST]);

impl<'de> Deserialize<'de> for Values<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Values<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<Values<'de>, M::Error> {
                let mut out = [None; MOST];
                let mut n = 0;
                while let Some((_, v)) = m.next_entry::<IgnoredAny, &RawValue>()? {
                    if n == MOST {
                        return Err(serde::de::Error::custom("too many"));
                    }
                    out[n] = Some(v);
                    n += 1;
                }
                Ok(Values(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// How long s is as JSON, as serde_json writes it: quoted, escaped.
fn json_len(s: &str) -> usize {
    2 + s
        .bytes()
        .map(|c| match c {
            b'"' | b'\\' | 0x08 | 0x0c | b'\n' | b'\r' | b'\t' => 2,
            0..0x20 => 6,
            _ => 1,
        })
        .sum::<usize>()
}

/// The call's raw input with what its fields hold cut out, taking the
/// raw from it; None, the call as it was, when nothing's worth cutting.
/// A string is cut out only where the field, written as serde_json
/// writes it, is the very bytes it was: put back, the JSON is as sent.
pub(crate) fn lean(c: &mut Call) -> Option<Box<str>> {
    let raw = c.raw.as_ref()?.get();
    let Values(vals) = serde_json::from_str(raw).ok()?;
    let mut tpl = String::new();
    let mut from = 0;
    let mut buf = Vec::new();
    for v in vals.into_iter().flatten() {
        let text = v.get();
        if text.len() < LEAST + 2 || !text.starts_with('"') {
            continue;
        }
        let Some(f) = FIELDS.iter().position(|f| {
            f(&c.input).is_some_and(|s| {
                json_len(s) == text.len() && {
                    buf.clear();
                    serde_json::to_writer(&mut buf, s).is_ok() && buf == text.as_bytes()
                }
            })
        }) else {
            continue;
        };
        if tpl.capacity() == 0 {
            tpl.reserve_exact(raw.len());
        }
        let at = text.as_ptr() as usize - raw.as_ptr() as usize;
        tpl.push_str(&raw[from..at]);
        tpl.push(char::from(f as u8 + 1));
        from = at + text.len();
    }
    if from == 0 {
        return None;
    }
    tpl.push_str(&raw[from..]);
    c.raw = None;
    Some(tpl.into_boxed_str())
}

/// The raw input again, from what lean left and the call's fields.
pub(crate) fn rebuild(tpl: &str, i: &Input) -> String {
    let mut out = Vec::with_capacity(len(tpl, i));
    let mut rest = tpl;
    while let Some(at) = rest.bytes().position(|b| b < 0x10) {
        out.extend_from_slice(&rest.as_bytes()[..at]);
        let f = (rest.as_bytes()[at] - 1) as usize;
        let s = FIELDS.get(f).and_then(|f| f(i)).unwrap_or("");
        // Writing a str into a Vec doesn't fail.
        let _ = serde_json::to_writer(&mut out, s);
        rest = &rest[at + 1..];
    }
    out.extend_from_slice(rest.as_bytes());
    // Whole strs and serde_json's escapes of them: UTF-8 throughout.
    String::from_utf8(out).unwrap_or_default()
}

/// How long rebuild's JSON is, without making it.
pub(crate) fn len(tpl: &str, i: &Input) -> usize {
    tpl.bytes()
        .map(|b| match b {
            0..0x10 => json_len(
                FIELDS
                    .get((b - 1) as usize)
                    .and_then(|f| f(i))
                    .unwrap_or(""),
            ),
            _ => 1,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::tool::{Kind, Replace};
    use rush_core::wire::RawJson;

    fn call(raw: &str, input: Input) -> Call {
        Call {
            kind: Kind::Shell,
            input,
            raw: Some(RawJson(RawValue::from_string(raw.into()).unwrap())),
            ..Call::default()
        }
    }

    /// What's cut out comes back byte for byte; what can't stays raw.
    #[test]
    fn comes_back_the_same() {
        let long = "cd /work/rush && cargo test -p rush-convo -- --nocapture";
        let quoted = "echo \"a\tb\" \\\n && printf '%s' \u{2028}é";
        let cases: Vec<(String, Input, bool)> = vec![
            (
                format!(r#"{{"command":{},"description":"Run it"}}"#, serde_json::to_string(long).unwrap()),
                Input { command: long.into(), description: "Run it".into(), ..Input::default() },
                true,
            ),
            (
                format!(r#"{{"command":{},"timeout":120000,"run_in_background":true}}"#, serde_json::to_string(quoted).unwrap()),
                Input { command: quoted.into(), ..Input::default() },
                true,
            ),
            // Escaped otherwise than serde_json would: kept raw.
            (
                r#"{"command":"cd \/work\/rush && cargo test --workspace"}"#.to_string(),
                Input { command: "cd /work/rush && cargo test --workspace".into(), ..Input::default() },
                false,
            ),
            // Short: kept raw.
            (
                r#"{"command":"ls"}"#.to_string(),
                Input { command: "ls".into(), ..Input::default() },
                false,
            ),
            (
                r#"{"file_path":"/work/rush/crates/rush-convo/src/model.rs","old_string":"fn a() {}\n","new_string":"fn b() {}\n// the same, but longer\n","replace_all":false}"#.to_string(),
                Input {
                    path: "/work/rush/crates/rush-convo/src/model.rs".into(),
                    edits: vec![Replace {
                        old: "fn a() {}\n".into(),
                        new: "fn b() {}\n// the same, but longer\n".into(),
                        ..Replace::default()
                    }],
                    ..Input::default()
                },
                true,
            ),
        ];
        for (raw, input, cut) in cases {
            let mut c = call(&raw, input);
            let tpl = lean(&mut c);
            assert_eq!(tpl.is_some(), cut, "{raw}");
            match tpl {
                Some(t) => {
                    assert!(c.raw.is_none());
                    assert!(t.len() < raw.len());
                    assert_eq!(rebuild(&t, &c.input), raw);
                    assert_eq!(len(&t, &c.input), raw.len());
                }
                None => assert_eq!(c.raw.as_ref().map(|r| r.get()), Some(raw.as_str())),
            }
        }
    }
}
