//! Flags read as Go's flag package reads them, with its words when one is
//! wrong, so rush watch and rush pr follow-run answer as Go's do.
//! Ported from Go's flag/flag.go (parseOne, failf, defaultUsage,
//! PrintDefaults) for the string, duration and int flags rush uses.

use std::io::Write;

use crate::go::{parse_duration, quote};

/// A flag's kind: what its value must be, and the word its usage gives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    String,
    Duration,
    Int,
}

struct Flag {
    name: &'static str,
    kind: Kind,
    /// Its default as Go's DefValue says it ("done", "25m0s", "0").
    default: String,
    value: String,
}

/// A set of flags, named as Go's FlagSet is (its usage says "Usage of
/// <name>:").
pub struct FlagSet {
    name: String,
    flags: Vec<Flag>,
}

/// What reading the flags came to besides the rest of the args: -h asked
/// for the usage, or a flag was wrong. Either way it's been said.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    Help,
    Wrong(String),
}

impl FlagSet {
    pub fn new(name: &str) -> FlagSet {
        FlagSet {
            name: name.to_string(),
            flags: Vec::new(),
        }
    }

    /// Adds a flag; default as Go's String of the value says it.
    pub fn add(&mut self, name: &'static str, kind: Kind, default: &str) {
        self.flags.push(Flag {
            name,
            kind,
            default: default.to_string(),
            value: default.to_string(),
        });
        self.flags.sort_by_key(|f| f.name);
    }

    /// The flag's value as given, or its default.
    pub fn get(&self, name: &str) -> &str {
        self.flags
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
            .unwrap_or("")
    }

    /// A duration flag's value, in nanoseconds.
    pub fn duration(&self, name: &str) -> i64 {
        parse_duration(self.get(name)).unwrap_or(0)
    }

    /// An int flag's value.
    pub fn int(&self, name: &str) -> i64 {
        parse_int(self.get(name)).unwrap_or(0)
    }

    /// Reads args up to the first that isn't a flag, and gives the rest.
    /// What's wrong goes to out with the usage, as Go's flag says it.
    pub fn parse(&mut self, args: &[String], out: &mut dyn Write) -> Result<Vec<String>, Stop> {
        let mut i = 0;
        while i < args.len() {
            let s = &args[i];
            if s.len() < 2 || !s.starts_with('-') {
                break;
            }
            let mut name = &s[1..];
            if let Some(n) = name.strip_prefix('-') {
                if n.is_empty() {
                    // "--" ends the flags
                    i += 1;
                    break;
                }
                name = n;
            }
            if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
                return Err(self.fail(out, format!("bad flag syntax: {s}")));
            }
            i += 1;
            // The equals sign can't be first. Go reads the name a byte at
            // a time; '=' is ASCII, so where it is is a char boundary.
            let (name, value) = match name.bytes().skip(1).position(|b| b == b'=') {
                Some(eq) => (&name[..eq + 1], Some(name[eq + 2..].to_string())),
                None => (name, None),
            };
            let Some(k) = self.flags.iter().position(|f| f.name == name) else {
                if name == "help" || name == "h" {
                    self.usage(out);
                    return Err(Stop::Help);
                }
                return Err(self.fail(out, format!("flag provided but not defined: -{name}")));
            };
            let value = match value {
                Some(v) => v,
                None if i < args.len() => {
                    i += 1;
                    args[i - 1].clone()
                }
                None => return Err(self.fail(out, format!("flag needs an argument: -{name}"))),
            };
            let bad = match self.flags[k].kind {
                Kind::String => None,
                Kind::Duration => parse_duration(&value).is_none().then_some("parse error"),
                Kind::Int => parse_int(&value).err(),
            };
            if let Some(why) = bad {
                return Err(self.fail(
                    out,
                    format!("invalid value {} for flag -{name}: {why}", quote(&value)),
                ));
            }
            self.flags[k].value = value;
        }
        Ok(args[i..].to_vec())
    }

    fn fail(&self, out: &mut dyn Write, msg: String) -> Stop {
        let _ = writeln!(out, "{msg}");
        self.usage(out);
        Stop::Wrong(msg)
    }

    /// Go's defaultUsage: the flags in name order, each with its default
    /// where it isn't the zero one.
    pub fn usage(&self, out: &mut dyn Write) {
        let _ = writeln!(out, "Usage of {}:", self.name);
        for f in &self.flags {
            let word = match f.kind {
                Kind::String => "string",
                Kind::Duration => "duration",
                Kind::Int => "int",
            };
            let mut line = format!("  -{} {word}\n    \t", f.name);
            let zero = match f.kind {
                Kind::String => "",
                Kind::Duration => "0s",
                Kind::Int => "0",
            };
            if f.default != zero {
                if f.kind == Kind::String {
                    line += &format!(" (default {})", quote(&f.default));
                } else {
                    line += &format!(" (default {})", f.default);
                }
            }
            let _ = writeln!(out, "{line}");
        }
    }
}

/// s as Go's int flag reads it, strconv.ParseInt(s, 0, 64): a sign, then
/// decimal, or 0x / 0o / 0b / 0 for hex, octal and binary, underscores
/// allowed between digits after a prefix. Why not, as Go's flag says it.
fn parse_int(s: &str) -> Result<i64, &'static str> {
    const SYNTAX: &str = "parse error";
    const RANGE: &str = "value out of range";
    let (neg, u) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if u.is_empty() {
        return Err(SYNTAX);
    }
    let b = u.as_bytes();
    let (base, digits): (u64, &[u8]) = if b[0] == b'0' && b.len() >= 3 {
        match b[1].to_ascii_lowercase() {
            b'b' => (2, &b[2..]),
            b'o' => (8, &b[2..]),
            b'x' => (16, &b[2..]),
            _ => (8, &b[1..]),
        }
    } else if b[0] == b'0' {
        (8, &b[1..])
    } else {
        (10, b)
    };
    let mut n: u64 = 0;
    let mut underscores = false;
    for &c in digits {
        let d = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => c - b'0',
            _ if c.is_ascii_alphabetic() => c.to_ascii_lowercase() - b'a' + 10,
            _ => return Err(SYNTAX),
        } as u64;
        if d >= base {
            return Err(SYNTAX);
        }
        n = n
            .checked_mul(base)
            .and_then(|n| n.checked_add(d))
            .ok_or(RANGE)?;
    }
    if underscores && !underscore_ok(u) {
        return Err(SYNTAX);
    }
    if neg {
        if n > 1 << 63 {
            return Err(RANGE);
        }
        Ok((n as i64).wrapping_neg())
    } else {
        i64::try_from(n).map_err(|_| RANGE)
    }
}

/// Go's underscoreOK: underscores only between digits, or between a base
/// prefix and a digit.
fn underscore_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if b.len() >= 2 && b[0] == b'0' && matches!(b[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = b[1].eq_ignore_ascii_case(&b'x');
    }
    while i < b.len() {
        let c = b[i];
        i += 1;
        if c.is_ascii_digit() || hex && matches!(c.to_ascii_lowercase(), b'a'..=b'f') {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> FlagSet {
        let mut fs = FlagSet::new("rush watch pr");
        fs.add("until", Kind::String, "done");
        fs.add("timeout", Kind::Duration, "25m0s");
        fs
    }

    #[test]
    fn reads_ints_as_go_s_parse_int_base_0() {
        for (s, want) in [
            ("42", Ok(42)),
            ("-7", Ok(-7)),
            ("+7", Ok(7)),
            ("0", Ok(0)),
            ("0x1f", Ok(31)),
            ("0X1F", Ok(31)),
            ("0o17", Ok(15)),
            ("017", Ok(15)),
            ("0b101", Ok(5)),
            ("1_000", Ok(1000)),
            ("_1", Err("parse error")),
            ("1_", Err("parse error")),
            ("0x_1f", Ok(31)),
            ("0x1__f", Err("parse error")),
            ("08", Err("parse error")),
            ("0x", Err("parse error")),
            ("", Err("parse error")),
            ("-", Err("parse error")),
            ("1e3", Err("parse error")),
            ("9223372036854775807", Ok(i64::MAX)),
            ("-9223372036854775808", Ok(i64::MIN)),
            ("9223372036854775808", Err("value out of range")),
            ("99999999999999999999x", Err("value out of range")),
        ] {
            assert_eq!(parse_int(s), want, "{s:?}");
        }
    }

    #[test]
    fn a_name_that_starts_with_a_wide_char_is_no_panic() {
        let mut out = Vec::new();
        let mut fs = set();
        let args = vec!["-é=x".to_string()];
        assert!(fs.parse(&args, &mut out).is_err());
        assert!(
            String::from_utf8(out)
                .unwrap()
                .starts_with("flag provided but not defined: -é\n")
        );
    }

    #[test]
    fn reads_flags_as_go_does() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut fs = set();
        let rest = fs
            .parse(
                &args(&["--until=fail", "-timeout", "5m", "42", "x"]),
                &mut out,
            )
            .unwrap();
        assert_eq!(rest, args(&["42", "x"]));
        assert_eq!(
            (fs.get("until"), fs.duration("timeout")),
            ("fail", 300_000_000_000)
        );
        let mut fs = set();
        assert!(fs.parse(&args(&["--nope"]), &mut out).is_err());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "flag provided but not defined: -nope\nUsage of rush watch pr:\n  -timeout duration\n    \t (default 25m0s)\n  -until string\n    \t (default \"done\")\n"
        );
    }
}
