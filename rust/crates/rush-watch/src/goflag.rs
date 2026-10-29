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
        self.get(name).parse().unwrap_or(0)
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
            // The equals sign can't be first.
            let (name, value) = match name[1..].find('=') {
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
                Kind::Int => int_error(&value),
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

/// Why Go's int flag refuses s (strconv.ParseInt(s, 0, 64)), if it does.
fn int_error(s: &str) -> Option<&'static str> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Some("parse error");
    }
    if s.parse::<i64>().is_err() {
        return Some("value out of range");
    }
    None
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
