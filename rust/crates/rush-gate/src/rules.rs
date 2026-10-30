//! How each program is gated, from the gate's settings.
//! Ported from internal/gate/rules.go:1-169 (cargo 4 GiB at :30).

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::godur;

/// A Rule is how one program is gated: the memory a run of it is expected
/// to need before it may start, and how long it may run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    /// Bytes.
    pub memory: u64,
    pub limit: Duration,
}

impl Rule {
    /// The most memory a run's processes may hold together before the
    /// watchdog stops it: three times what it was expected to need.
    /// It wraps as Go's uint64 does, so a memory past a third of u64
    /// read from settings doesn't panic.
    pub fn cap(&self) -> u64 {
        self.memory.wrapping_mul(3)
    }
}

/// The rule for each program, by name.
pub type Rules = BTreeMap<String, Rule>;

pub const GIB: u64 = 1 << 30;

/// The memory a run of each program is expected to need; a program named
/// without one expects [`OTHER_ESTIMATE`].
pub const ESTIMATES: &[(&str, u64)] = &[
    ("tsc", 4 * GIB),
    ("tsgo", 4 * GIB),
    ("tsrs", 2 * GIB),
    ("vitest", 3 * GIB),
    ("jest", 3 * GIB),
    ("playwright", 2 * GIB),
    ("next", 3 * GIB),
    ("webpack", 3 * GIB),
    ("vite", 3 * GIB),
    ("go", 2 * GIB),
    ("cargo", 4 * GIB),
    ("rustc", 2 * GIB),
    ("golangci-lint", GIB),
    ("oxlint", GIB),
    ("eslint", GIB),
    ("biome", GIB),
];

pub const OTHER_ESTIMATE: u64 = 2 * GIB;

/// The settings' own, for any left unset.
pub const DEFAULTS: &[(&str, &str)] = &[
    (
        "names",
        "tsc, tsgo, tsrs, go, cargo, rustc, webpack, vite, next, vitest, jest, playwright, \
         golangci-lint, oxlint, eslint, biome",
    ),
    ("limit", "30m"),
    ("reserve", "15%"),
    ("floor", "5%"),
];

/// The default of setting k, or "".
pub fn default_of(k: &str) -> &'static str {
    DEFAULTS
        .iter()
        .find(|(n, _)| *n == k)
        .map_or("", |(_, v)| v)
}

/// The settings by key, as the plugin keeps them.
pub type Settings = HashMap<String, String>;

fn estimate(name: &str) -> u64 {
    ESTIMATES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(OTHER_ESTIMATE, |(_, m)| *m)
}

/// The rule for each program from the gate's settings: names says which,
/// limit how long any may run, and limits such as "tsc=6G/45m, vitest=4G"
/// change a program's memory or time, or add it. Parts it can't read are
/// left as they were; a setting missing takes its default. The slot
/// pool's settings (scope, parallel, stagger, busy, overrides) are read by
/// no one now.
pub fn parse_rules(v: &Settings) -> Rules {
    let get = |k: &str| -> String {
        match v.get(k) {
            Some(s) => s.trim().to_string(),
            None => default_of(k).to_string(),
        }
    };
    let limit = godur::parse_positive(&get("limit")).unwrap_or(Duration::from_secs(30 * 60));
    let rule = |name: &str| Rule {
        memory: estimate(name),
        limit,
    };
    let mut out = Rules::new();
    for n in fields(&get("names")) {
        out.insert(n.to_string(), rule(n));
    }
    let limits = v.get("limits").map(String::as_str).unwrap_or("");
    for o in fields(limits) {
        let (name, parts) = o.split_once('=').unwrap_or((o, ""));
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut r = out.get(name).copied().unwrap_or_else(|| rule(name));
        for p in parts.split('/') {
            let p = p.trim();
            if let Some(d) = godur::parse_positive(p) {
                r.limit = d;
            } else if let Some(b) = parse_size(p) {
                r.memory = b;
            }
        }
        out.insert(name.to_string(), r);
    }
    out
}

/// The share of the machine's memory the reserve setting keeps free of
/// gated runs: "15%", or 0.15.
pub fn parse_reserve(v: &Settings) -> f64 {
    let s = v
        .get("reserve")
        .map(String::as_str)
        .unwrap_or(default_of("reserve"))
        .trim();
    let parsed = go_float(s.strip_suffix('%').unwrap_or(s));
    let mut f = parsed.unwrap_or(0.0);
    if s.ends_with('%') || f > 1.0 {
        f /= 100.0;
    }
    if parsed.is_none() || f < 0.0 || f >= 1.0 {
        return 0.15;
    }
    f
}

/// The floor setting in percent: the memory left at which the guard stops
/// agents' programs; 0 when it's off.
pub fn parse_floor(v: &Settings) -> i32 {
    let s = v
        .get("floor")
        .map(String::as_str)
        .unwrap_or(default_of("floor"))
        .trim();
    if s == "off" {
        return 0;
    }
    match s.strip_suffix('%').unwrap_or(s).parse::<i64>() {
        Ok(n) if n > 0 && n < 50 => n as i32,
        _ => crate::guard::DEFAULT_FLOOR,
    }
}

/// A size such as 6G, 512M or 1.5GiB in bytes, counted in 1024s. A bare
/// number isn't a size.
pub fn parse_size(s: &str) -> Option<u64> {
    let up = s.trim().to_uppercase();
    let s = up.strip_suffix('B').unwrap_or(&up);
    let s = s.strip_suffix('I').unwrap_or(s);
    let last = *s.as_bytes().last()?;
    let mult: f64 = match last {
        b'K' => (1u64 << 10) as f64,
        b'M' => (1u64 << 20) as f64,
        b'G' => (1u64 << 30) as f64,
        b'T' => (1u64 << 40) as f64,
        _ => return None,
    };
    let f = go_float(&s[..s.len() - 1])?;
    if f <= 0.0 || f.is_nan() {
        return None;
    }
    Some((f * mult) as u64)
}

/// s as Go's strconv.ParseFloat reads the plain forms: digits, a point,
/// an exponent, a sign, Inf and NaN.
fn go_float(s: &str) -> Option<f64> {
    let t = s.trim_start_matches(['+', '-']);
    let lower = t.to_ascii_lowercase();
    if !(lower == "inf" || lower == "infinity" || lower == "nan")
        && !t
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    s.parse::<f64>().ok()
}

/// n bytes for people: 3.1 GiB, 640 MiB.
pub fn size(n: u64) -> String {
    if n >= GIB {
        return format!("{:.1} GiB", n as f64 / GIB as f64);
    }
    format!("{} MiB", n >> 20)
}

/// s's words, split at commas and blanks.
pub(crate) fn fields(s: &str) -> impl Iterator<Item = &str> {
    s.split([',', ' ', '\t', '\n']).filter(|w| !w.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(kv: &[(&str, &str)]) -> Settings {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // gate_test.go TestParseRules
    #[test]
    fn parse_rules_reads_the_settings() {
        let got = parse_rules(&settings(&[
            ("names", "tsc, go"),
            ("limit", "20m"),
            ("scope", "worktree"),
            ("parallel", "3"),
            ("overrides", "tsc=system/1/10s"),
            ("limits", "tsc=6G/45m, cargo=512M, go=1h, bad"),
        ]));
        let min = |m: u64| Duration::from_secs(m * 60);
        let want: Rules = [
            (
                "tsc",
                Rule {
                    memory: 6 * GIB,
                    limit: min(45),
                },
            ),
            (
                "go",
                Rule {
                    memory: 2 * GIB,
                    limit: min(60),
                },
            ),
            (
                "cargo",
                Rule {
                    memory: 512 << 20,
                    limit: min(20),
                },
            ),
            (
                "bad",
                Rule {
                    memory: OTHER_ESTIMATE,
                    limit: min(20),
                },
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert_eq!(got, want);
        let d = parse_rules(&Settings::new());
        assert_eq!(
            d["vitest"],
            Rule {
                memory: 3 * GIB,
                limit: min(30)
            }
        );
        assert_eq!(d["oxlint"].memory, GIB);
        assert_eq!(d["tsgo"].memory, 4 * GIB);
        for n in ["golangci-lint", "oxlint", "playwright"] {
            assert!(d.contains_key(n), "{n} isn't gated by default");
        }
        for (input, want) in [
            ("15%", 0.15),
            ("0.2", 0.2),
            ("25", 0.25),
            ("nonsense", 0.15),
        ] {
            assert_eq!(
                parse_reserve(&settings(&[("reserve", input)])),
                want,
                "{input}"
            );
        }
        for (input, want) in [
            ("6G", 6 * GIB),
            ("1.5GiB", 3 * GIB / 2),
            ("512mb", 512 << 20),
            ("4", 0),
            ("10s", 0),
        ] {
            assert_eq!(parse_size(input).unwrap_or(0), want, "{input}");
        }
    }

    // guard_test.go TestParseFloor
    #[test]
    fn parse_floor_reads_the_setting() {
        for (input, want) in [
            ("5%", 5),
            ("10%", 10),
            ("off", 0),
            ("nonsense", 5),
            ("80%", 5),
        ] {
            assert_eq!(parse_floor(&settings(&[("floor", input)])), want, "{input}");
        }
        assert_eq!(parse_floor(&Settings::new()), 5);
    }

    #[test]
    fn sizes_for_people() {
        assert_eq!(size(3 * GIB + GIB / 10), "3.1 GiB");
        assert_eq!(size(640 << 20), "640 MiB");
        assert_eq!(size(6 * GIB), "6.0 GiB");
    }

    // A memory too large to triple, from settings, wraps as Go's does
    // rather than panicking; so does what the room counts of it.
    #[test]
    fn huge_memory_doesnt_panic() {
        let rules = parse_rules(&Settings::from([(
            "limits".to_string(),
            "tsc=99999999999T".to_string(),
        )]));
        let r = rules["tsc"];
        assert_eq!(r.memory, u64::MAX);
        assert_eq!(r.cap(), u64::MAX.wrapping_mul(3));
        let t = crate::share::Ticket {
            running: true,
            memory: r.memory,
            ..Default::default()
        };
        assert_eq!(crate::share::room(&[t.clone(), t], u64::MAX, 1), 0);
    }
}
