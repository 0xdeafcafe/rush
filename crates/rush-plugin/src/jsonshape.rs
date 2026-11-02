//! Reading JSON as Go rush's jsonx does (encoding/json/v2 with v1's
//! habits), for the files people and plugins write by hand: a member's
//! name matches its field in any case, null leaves a field at its zero,
//! and, where Go asks for RejectUnknown, a name no field takes is refused
//! with Go's words. Raw values (an agent's definition) keep their text
//! when nothing needed changing.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// What a JSON value is meant to hold, as far as names matter.
pub(crate) enum Shape {
    /// A Go struct: its type name, for errors, and each field's JSON name
    /// with what it holds.
    Struct(&'static str, &'static [(&'static str, Shape)]),
    /// A slice of them.
    Seq(&'static Shape),
    /// A map with string keys to them.
    Map(&'static Shape),
    /// Anything: names inside aren't looked at.
    Any,
}

/// Reads b as a T of the given shape.
pub(crate) fn decode<T: DeserializeOwned>(
    b: &[u8],
    shape: &Shape,
    strict: bool,
) -> Result<T, String> {
    let text = String::from_utf8_lossy(b);
    let mut v: Value = serde_json::from_str(&text).map_err(syntax)?;
    let changed = normalize(&mut v, shape, strict, "")?;
    if !changed && let Ok(t) = serde_json::from_slice::<T>(b) {
        return Ok(t);
    }
    serde_json::from_value(v).map_err(|e| format!("json: {e}"))
}

/// A JSON value read the same way, then as a T: for params already parsed
/// out of a message.
pub(crate) fn decode_str<T: DeserializeOwned>(
    s: &str,
    shape: &Shape,
    strict: bool,
) -> Result<T, String> {
    decode(s.as_bytes(), shape, strict)
}

fn syntax(e: serde_json::Error) -> String {
    if e.is_eof() {
        return "jsontext: unexpected EOF".into();
    }
    format!("jsontext: {e}")
}

/// Renames members to their fields' names, drops nulls on fields, and
/// refuses unknown names when strict. It says whether it changed v.
fn normalize(v: &mut Value, shape: &Shape, strict: bool, path: &str) -> Result<bool, String> {
    let mut changed = false;
    match (shape, v) {
        (Shape::Struct(ty, fields), Value::Object(obj)) => {
            let keys: Vec<String> = obj.keys().cloned().collect();
            for k in keys {
                let field = fields
                    .iter()
                    .find(|(n, _)| *n == k)
                    .or_else(|| fields.iter().find(|(n, _)| n.eq_ignore_ascii_case(&k)));
                let Some((name, sub)) = field else {
                    if strict {
                        let within = if path.is_empty() {
                            String::new()
                        } else {
                            format!(" within {}", serde_json::Value::from(path))
                        };
                        return Err(format!(
                            "json: unable to unmarshal JSON string into Go {ty}: unknown object member name {}{within}",
                            serde_json::Value::from(k.as_str())
                        ));
                    }
                    continue;
                };
                let mut val = obj.remove(&k).unwrap_or(Value::Null);
                if *name != k {
                    changed = true;
                }
                if val.is_null() {
                    changed = true;
                    continue;
                }
                let p = format!("{path}/{}", pointer(name));
                changed |= normalize(&mut val, sub, strict, &p)?;
                obj.insert((*name).to_string(), val);
            }
        }
        (Shape::Seq(sub), Value::Array(items)) => {
            for (i, it) in items.iter_mut().enumerate() {
                changed |= normalize(it, sub, strict, &format!("{path}/{i}"))?;
            }
        }
        (Shape::Map(sub), Value::Object(obj)) => {
            for (k, it) in obj.iter_mut() {
                changed |= normalize(it, sub, strict, &format!("{path}/{}", pointer(k)))?;
            }
        }
        _ => {}
    }
    Ok(changed)
}

fn pointer(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        os: Vec<String>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct M {
        name: String,
        tools: bool,
        requires: R,
    }
    const RS: Shape = Shape::Struct("main.R", &[("os", Shape::Any)]);
    const MS: Shape = Shape::Struct(
        "main.M",
        &[
            ("name", Shape::Any),
            ("tools", Shape::Any),
            ("requires", RS),
        ],
    );

    #[test]
    fn as_go_reads_it() {
        let m: M = decode(
            br#"{"NAME":"p","tools":null,"requires":{"OS":["darwin"]}}"#,
            &MS,
            true,
        )
        .unwrap();
        assert_eq!(
            (m.name.as_str(), m.tools, m.requires.os.len()),
            ("p", false, 1)
        );
        let e = decode::<M>(br#"{"name":"p","sandbox":false}"#, &MS, true)
            .err()
            .unwrap();
        assert_eq!(
            e,
            r#"json: unable to unmarshal JSON string into Go main.M: unknown object member name "sandbox""#
        );
        let e = decode::<M>(br#"{"name":"p","requires":{"oz":1}}"#, &MS, true)
            .err()
            .unwrap();
        assert_eq!(
            e,
            r#"json: unable to unmarshal JSON string into Go main.R: unknown object member name "oz" within "/requires""#
        );
        assert!(decode::<M>(br#"{"name":"p","sandbox":false}"#, &MS, false).is_ok());
        assert!(decode::<M>(br#"{"name":"p""#, &MS, false).is_err());
    }
}
