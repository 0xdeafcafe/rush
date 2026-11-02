//! Go's flag package as rush serve, rush remote and rush session read
//! their flags: -name or --name, a value after = or as the next argument,
//! a bool alone, a multi (flag.Var) any number of times; flags end at the
//! first argument that isn't one, or at --. After Go's cmd/rush/session.go
//! (multi :108, idAndFlags :115, newFlags :132).

/// One command's flags.
pub(crate) struct Flags {
    pub name: String,
    strings: Vec<(&'static str, String)>,
    bools: Vec<(&'static str, bool)>,
    multis: Vec<(&'static str, Vec<String>)>,
    /// -help says nothing (newFlags: its output io.Discard).
    quiet: bool,
    pub rest: Vec<String>,
}

impl Flags {
    pub fn new(name: &str, strings: &[&'static str], bools: &[&'static str]) -> Flags {
        Flags {
            name: name.into(),
            strings: strings.iter().map(|s| (*s, String::new())).collect(),
            bools: bools.iter().map(|b| (*b, false)).collect(),
            multis: Vec::new(),
            quiet: false,
            rest: Vec::new(),
        }
    }

    /// A session command's flags (newFlags): -help prints nothing.
    pub fn session(
        name: &str,
        strings: &[&'static str],
        bools: &[&'static str],
        multis: &[&'static str],
    ) -> Flags {
        let mut f = Flags::new(name, strings, bools);
        f.multis = multis.iter().map(|m| (*m, Vec::new())).collect();
        f.quiet = true;
        f
    }

    pub fn parse(&mut self, args: &[String]) -> Result<(), String> {
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--" {
                self.rest.extend(args[i + 1..].iter().cloned());
                return Ok(());
            }
            let Some(f) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
                self.rest.extend(args[i..].iter().cloned());
                return Ok(());
            };
            if f.is_empty() || f.starts_with('-') || f.starts_with('=') {
                return Err(format!("bad flag syntax: {a}"));
            }
            let (k, v) = match f.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (f, None),
            };
            let value = |v: Option<String>, i: &mut usize| match v {
                Some(v) => Ok(v),
                None => {
                    *i += 1;
                    args.get(*i)
                        .cloned()
                        .ok_or_else(|| format!("flag needs an argument: -{k}"))
                }
            };
            if let Some(b) = self.bools.iter_mut().find(|(n, _)| *n == k) {
                b.1 = match v.as_deref() {
                    None | Some("true") | Some("1") | Some("t") | Some("T") | Some("TRUE")
                    | Some("True") => true,
                    Some("false") | Some("0") | Some("f") | Some("F") | Some("FALSE")
                    | Some("False") => false,
                    Some(x) => {
                        return Err(format!("invalid boolean value {x:?} for -{k}: parse error"));
                    }
                };
            } else if let Some(s) = self.strings.iter_mut().find(|(n, _)| *n == k) {
                s.1 = value(v, &mut i)?;
            } else if let Some(m) = self.multis.iter_mut().find(|(n, _)| *n == k) {
                m.1.push(value(v, &mut i)?);
            } else if k == "help" || k == "h" {
                if !self.quiet {
                    eprint!("{}", self.usage());
                }
                return Err("flag: help requested".into());
            } else {
                return Err(format!("flag provided but not defined: -{k}"));
            }
            i += 1;
        }
        Ok(())
    }

    /// What -help prints, as Go's flag.PrintDefaults words it: each flag
    /// in name order, with no usage text of its own.
    fn usage(&self) -> String {
        let mut names: Vec<(&str, bool)> = self
            .strings
            .iter()
            .map(|(n, _)| (*n, true))
            .chain(self.multis.iter().map(|(n, _)| (*n, true)))
            .chain(self.bools.iter().map(|(n, _)| (*n, false)))
            .collect();
        names.sort();
        let mut out = format!("Usage of {}:\n", self.name);
        for (n, string) in names {
            let kind = if string { " value" } else { "" };
            let kind = if self.strings.iter().any(|(s, _)| *s == n) {
                " string"
            } else {
                kind
            };
            out.push_str(&format!("  -{n}{kind}\n    \t\n"));
        }
        out
    }

    pub fn s(&self, k: &str) -> String {
        self.strings
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    pub fn b(&self, k: &str) -> bool {
        self.bools.iter().any(|(n, v)| *n == k && *v)
    }

    /// Every value a multi was given, in order.
    pub fn m(&self, k: &str) -> Vec<String> {
        self.multis
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// The session id, before or after the flags (idAndFlags).
    pub fn id_and_flags(&mut self, args: &[String]) -> Result<String, String> {
        let (mut id, rest) = match args.first() {
            Some(a) if !a.starts_with('-') => (a.clone(), &args[1..]),
            _ => (String::new(), args),
        };
        self.parse(rest)?;
        if id.is_empty()
            && let Some(a) = self.rest.first()
        {
            id = a.clone();
        }
        if id.is_empty() {
            return Err(format!("usage: rush session {} <id>", self.name));
        }
        Ok(id)
    }
}

/// Whether args hold flag f set on, as Go's hasFlag reads them: the
/// command's JSON-ness for an error its flags made.
pub(crate) fn has_flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f || *a == format!("{f}=true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn flags_as_go_reads_them() {
        let mut f = Flags::new("start", &["cwd", "name"], &["json"]);
        f.parse(&s(&[
            "--cwd", "/x", "-name=n", "--json", "rest", "--cwd", "y",
        ]))
        .unwrap();
        assert_eq!(f.s("cwd"), "/x");
        assert_eq!(f.s("name"), "n");
        assert!(f.b("json"));
        assert_eq!(f.rest, s(&["rest", "--cwd", "y"]));
        let mut f = Flags::new("send", &[], &["now"]);
        assert_eq!(f.id_and_flags(&s(&["abc", "--now"])).unwrap(), "abc");
        assert!(f.b("now"));
        let mut f = Flags::new("send", &[], &["now"]);
        assert_eq!(f.id_and_flags(&s(&["--now", "abc"])).unwrap(), "abc");
        let mut f = Flags::new("send", &[], &[]);
        assert_eq!(
            f.id_and_flags(&[]).unwrap_err(),
            "usage: rush session send <id>"
        );
        let mut f = Flags::new("x", &[], &[]);
        assert_eq!(
            f.parse(&s(&["--nope"])).unwrap_err(),
            "flag provided but not defined: -nope"
        );
        let mut f = Flags::session("start", &[], &[], &["meta"]);
        f.parse(&s(&["--meta", "a=1", "-meta=b=2"])).unwrap();
        assert_eq!(f.m("meta"), s(&["a=1", "b=2"]));
        let mut f = Flags::session("start", &[], &[], &["image"]);
        assert_eq!(
            f.parse(&s(&["--image"])).unwrap_err(),
            "flag needs an argument: -image"
        );
        assert_eq!(f.parse(&s(&["-h"])).unwrap_err(), "flag: help requested");
        assert!(has_flag(&s(&["x", "--json=true"]), "--json"));
        assert!(!has_flag(&s(&["x", "--json=false"]), "--json"));
    }
}
