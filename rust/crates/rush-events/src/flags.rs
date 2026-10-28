//! Command-line flags as Go's flag package reads them (`flag.FlagSet`,
//! ContinueOnError, output discarded), for rush events and rush why: -x or
//! --x, a value after it or after =, a bool on its own, and the flags end
//! at the first argument that isn't one, or at --.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Str,
    Int,
    Bool,
}

/// What a FlagSet read: each flag set, and the arguments after the flags.
pub(crate) struct Parsed {
    pub(crate) set: HashMap<&'static str, String>,
    pub(crate) args: Vec<String>,
}

impl Parsed {
    pub(crate) fn str(&self, name: &str, def: &str) -> String {
        self.set
            .get(name)
            .cloned()
            .unwrap_or_else(|| def.to_string())
    }

    pub(crate) fn int(&self, name: &str, def: i64) -> i64 {
        self.set.get(name).and_then(|v| parse_int(v)).unwrap_or(def)
    }

    pub(crate) fn bool(&self, name: &str) -> bool {
        self.set
            .get(name)
            .is_some_and(|v| parse_bool(v) == Some(true))
    }
}

/// Reads args against flags. Err carries what had been read before the
/// error (Go's flags keep their values), with an error saying what.
pub(crate) fn parse(
    flags: &[(&'static str, Kind)],
    args: &[String],
) -> Result<Parsed, (Parsed, String)> {
    let mut p = Parsed {
        set: HashMap::new(),
        args: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        if s.len() < 2 || !s.starts_with('-') {
            break;
        }
        let mut name = &s[1..];
        if let Some(n) = name.strip_prefix('-') {
            if n.is_empty() {
                i += 1; // "--" ends the flags
                break;
            }
            name = n;
        }
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err((p, format!("bad flag syntax: {s}")));
        }
        i += 1;
        let (name, value) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (name, None),
        };
        let Some(&(key, kind)) = flags.iter().find(|(n, _)| *n == name) else {
            if name == "help" || name == "h" {
                return Err((p, "flag: help requested".into()));
            }
            return Err((p, format!("flag provided but not defined: -{name}")));
        };
        let value = match (kind, value) {
            (Kind::Bool, None) => "true".to_string(),
            (_, Some(v)) => v,
            (_, None) if i < args.len() => {
                i += 1;
                args[i - 1].clone()
            }
            (_, None) => return Err((p, format!("flag needs an argument: -{name}"))),
        };
        let ok = match kind {
            Kind::Str => true,
            Kind::Int => parse_int(&value).is_some(),
            Kind::Bool => parse_bool(&value).is_some(),
        };
        if !ok {
            return Err((p, format!("invalid value {value:?} for flag -{name}")));
        }
        p.set.insert(key, value);
    }
    p.args = args[i..].to_vec();
    Ok(p)
}

/// A bool as Go's strconv.ParseBool reads it.
fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// An int as Go's strconv.ParseInt(s, 0, 64) reads it: 0x, 0o, 0b or a
/// leading 0 for its base, _ between digits where there's a base prefix.
fn parse_int(s: &str) -> Option<i64> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let lower = body.to_ascii_lowercase();
    let (radix, digits, prefixed) = if let Some(d) = lower.strip_prefix("0x") {
        (16, d.to_string(), true)
    } else if let Some(d) = lower.strip_prefix("0o") {
        (8, d.to_string(), true)
    } else if let Some(d) = lower.strip_prefix("0b") {
        (2, d.to_string(), true)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, lower[1..].to_string(), true)
    } else {
        (10, lower.clone(), false)
    };
    if digits.is_empty() || (!prefixed && digits.contains('_')) {
        return None;
    }
    let digits = digits.replace('_', "");
    if digits.is_empty() || digits.starts_with('+') || digits.starts_with('-') {
        return None;
    }
    let v = i128::from_str_radix(&digits, radix).ok()?;
    let v = if neg { -v } else { v };
    i64::try_from(v).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    const FLAGS: [(&str, Kind); 3] = [("s", Kind::Str), ("n", Kind::Int), ("json", Kind::Bool)];

    #[test]
    fn reads_as_go_does() {
        let p = parse(
            &FLAGS,
            &args(&["-s", "a", "--n=3", "-json", "rest", "-n", "4"]),
        )
        .ok()
        .unwrap();
        assert_eq!(p.str("s", ""), "a");
        assert_eq!(p.int("n", 20), 3);
        assert!(p.bool("json"));
        assert_eq!(p.args, args(&["rest", "-n", "4"]));

        let p = parse(&FLAGS, &args(&["--", "-s"])).ok().unwrap();
        assert_eq!(p.args, args(&["-s"]));
        assert_eq!(p.int("n", 20), 20);

        for (bad, want) in [
            (&["-x"][..], "flag provided but not defined: -x"),
            (&["-h"], "flag: help requested"),
            (&["-s"], "flag needs an argument: -s"),
            (&["-n", "lots"], "invalid value \"lots\" for flag -n"),
            (&["---s"], "bad flag syntax: ---s"),
        ] {
            assert_eq!(parse(&FLAGS, &args(bad)).err().unwrap().1, want);
        }
    }

    #[test]
    fn ints_as_go_reads_them() {
        for (s, want) in [
            ("20", Some(20)),
            ("-3", Some(-3)),
            ("0x10", Some(16)),
            ("010", Some(8)),
            ("1_0", None),
            ("0x1_0", Some(16)),
            ("", None),
        ] {
            assert_eq!(parse_int(s), want, "{s}");
        }
    }
}
