//! JSON as Go rush writes it (photon `jsonx.Marshal`, encoding/json/v2 with
//! Deterministic): an object's keys in order, at every depth; <, > and &
//! as they are; control characters as \n, \t, … or \u00xx. A number read
//! back into a map (`map[string]any`) is a float64 in Go, written in ES6
//! form; `as_float` says to write numbers that way.

use serde_json::Value;

/// v as Go's jsonx writes it.
pub(crate) fn marshal(v: &Value, as_float: bool) -> String {
    let mut out = String::new();
    write(&mut out, v, as_float);
    out
}

fn write(out: &mut String, v: &Value, as_float: bool) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if !as_float && (n.is_i64() || n.is_u64()) {
                out.push_str(&n.to_string());
            } else {
                out.push_str(&float(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => string(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(out, v, as_float);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(out, k);
                out.push(':');
                write(out, &m[k], as_float);
            }
            out.push('}');
        }
    }
}

/// f as Go writes a float64: decimal between 1e-6 and 1e21, else with an
/// exponent (1e+21, 1.5e-7); shortest digits that read back the same.
pub(crate) fn float(f: f64) -> String {
    let abs = f.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let s = format!("{f:e}");
        match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        }
    } else {
        format!("{f}")
    }
}

/// s as a JSON string, escaped as Go's jsontext escapes it.
fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_as_go_does() {
        for (v, as_float, want) in [
            (
                json!({"b": 1, "a": {"z": true, "y": null}}),
                false,
                r#"{"a":{"y":null,"z":true},"b":1}"#,
            ),
            (json!("a<b>&\"c\"\n\u{1}"), false, r#""a<b>&\"c\"\n\u0001""#),
            (json!(1.5), false, "1.5"),
            (json!(7), true, "7"),
            (json!(1e21), true, "1e+21"),
            (json!(1.5e-7), true, "1.5e-7"),
            (json!(12345678901234567890u64), true, "12345678901234567000"),
            (json!([1, "x"]), false, r#"[1,"x"]"#),
        ] {
            assert_eq!(marshal(&v, as_float), want, "{v}");
        }
    }
}
