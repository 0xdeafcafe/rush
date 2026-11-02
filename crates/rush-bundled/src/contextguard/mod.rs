//! The bundled plugin that keeps what goes into an agent's context lean,
//! from its tool calls' hooks: no reading a file again that hasn't
//! changed since the agent read it, no reading a huge file whole without
//! a word first, no running a command a fourth time that failed the same
//! way three times, and a shell command's output squeezed of terminal
//! codes, progress redraws and repeats. Each is a setting of its own, on
//! by default. rush gate hook asks it, as it asks search-guard; what it
//! refuses or squeezes is logged, for rush why and rush events.
//! Rust-only: Go rush has no context guard.

pub mod hook;
pub mod rules;
pub mod squeeze;

use rush_plugin::{Bundle, Manifest, SettingSpec};

pub use hook::Answer;
pub use rules::Rules;

/// The plugin's.
pub const NAME: &str = "context-guard";

/// Its settings' keys.
pub const REREAD: &str = "reread";
pub const BIG_READ: &str = "big-read";
pub const REPEAT: &str = "repeat-fail";
pub const SQUEEZE: &str = "squeeze";
/// Its rules' numbers' keys: the smallest file a re-read is refused for,
/// in KB; how many lines make a file huge (its KB: 48 per 1,000 lines);
/// failures in a row before the next run is refused; and how many lines
/// of output are cut (a third of them kept at each end).
pub const REREAD_KB: &str = "reread-kb";
pub const BIG_LINES: &str = "big-read-lines";
pub const REPEATS: &str = "repeat-after";
pub const SQUEEZE_LINES: &str = "squeeze-lines";

/// The choices of each number, as stored, and each one's default
/// (today's rule).
pub const REREAD_KB_CHOICES: &[&str] = &["1", "2", "4", "8"];
pub const BIG_LINES_CHOICES: &[&str] = &["500", "1000", "2000"];
pub const REPEATS_CHOICES: &[&str] = &["2", "3", "5"];
pub const SQUEEZE_LINES_CHOICES: &[&str] = &["300", "600", "1200"];

/// The context guard's. Its settings are edited in Settings › Usage ›
/// Context guard; the plugin's own page points there.
pub fn manifest() -> Manifest {
    let on = |key: &str, title: &str, description: &str| SettingSpec {
        key: key.into(),
        title: title.into(),
        kind: "bool".into(),
        choices: Vec::new(),
        default: "true".into(),
        description: description.into(),
    };
    let pick =
        |key: &str, title: &str, choices: &[&str], default: &str, description: &str| SettingSpec {
            key: key.into(),
            title: title.into(),
            kind: "choice".into(),
            choices: choices.iter().map(|c| c.to_string()).collect(),
            default: default.into(),
            description: description.into(),
        };
    Manifest {
        settings: vec![
            on(
                REREAD,
                "No re-reading unchanged files",
                "Refuses an agent's read of a file it already read (the same lines or more) when the file hasn't \
                 changed since and it hasn't edited it; after its context is compacted it may read it again. Files \
                 under 2 KB (as set) are let through",
            ),
            on(
                BIG_READ,
                "No reading huge files whole",
                "Refuses, once per file, a whole read of a text file over 1,000 lines or 48 KB (as set), and points the agent \
                 to rg -n and offset and limit; asked again, it reads it. Images, PDFs and notebooks are left alone",
            ),
            on(
                REPEAT,
                "Stop repeat failures",
                "Refuses a shell command that failed the same way (same exit, same output's end) 3 times in a row (as set) \
                 with no edit in between, and tells the agent to change something first; asked again, it runs",
            ),
            on(
                SQUEEZE,
                "Squeeze command output",
                "Takes terminal colours and codes, progress bars' redraws, spaces at line ends, runs of blank lines \
                 and repeated lines out of a shell command's output before the agent sees it, and cuts one over 600 lines (as set) \
                 to its first and last third (the whole kept in a file it's pointed to). Never the output of \
                 cat, sed, git diff, rg and the like, whose exact spaces an edit needs",
            ),
            pick(
                REREAD_KB,
                "Smallest file it applies to",
                REREAD_KB_CHOICES,
                "2",
                "No re-reading unchanged files lets a file under this many KB be read again",
            ),
            pick(
                BIG_LINES,
                "Huge means over",
                BIG_LINES_CHOICES,
                "1000",
                "How many lines make a file too big to read whole without a word first; 48 KB per 1,000 lines \
                 counts too",
            ),
            pick(
                REPEATS,
                "Stop after",
                REPEATS_CHOICES,
                "3",
                "How many identical failures in a row before the next run is refused",
            ),
            pick(
                SQUEEZE_LINES,
                "Cut output longer than",
                SQUEEZE_LINES_CHOICES,
                "600",
                "Output longer than this many lines keeps a third of them at each end",
            ),
        ],
        ..crate::manifest(
            NAME,
            "",
            "Keeps agents' context lean from their tool calls: no re-reading files that haven't changed, no \
             reading huge files whole, no running a command again that failed the same way 3 times, and command \
             output without colour codes, progress bars and repeats.",
            16,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), false, crate::run_switch)
}

/// Which of its rules are on: none while it's off. It reads the disk.
pub fn rules() -> Rules {
    if !rush_plugin::bundled_on(NAME) {
        return Rules::NONE;
    }
    rules_of(&rush_plugin::setting_values_of(&manifest()))
}

/// The rules its settings' values turn on, at the numbers they say.
pub fn rules_of(values: &std::collections::BTreeMap<String, String>) -> Rules {
    let on = |k: &str| values.get(k).is_none_or(|v| v != "false");
    let n = |k: &str| {
        values
            .get(k)
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
    };
    let all = Rules::ALL;
    let big_lines = n(BIG_LINES).unwrap_or(all.big_lines);
    let cap_lines = n(SQUEEZE_LINES).map_or(all.cap_lines, |n| n as usize);
    Rules {
        reread: on(REREAD),
        big_read: on(BIG_READ),
        repeat: on(REPEAT),
        squeeze: on(SQUEEZE),
        small_file: n(REREAD_KB).map_or(all.small_file, |kb| kb * 1024),
        big_lines,
        big_bytes: big_lines * all.big_bytes / all.big_lines,
        repeats: n(REPEATS).map_or(all.repeats, |r| r as u32),
        cap_lines,
        keep_lines: cap_lines / 3,
    }
}

/// What it makes of an agent's hook input, while it's on: what the hook
/// answers and what rush logs; None to say nothing.
pub fn step(input: &[u8]) -> Option<Answer> {
    let rules = rules();
    if !rules.any() {
        return None;
    }
    let local = rush_core::time::format_rfc3339_local(rush_core::time::now());
    let env = hook::Env {
        dir: rush_state::dir(),
        // The agent's own scratch folder, where rush starts it with one.
        tmp: std::env::temp_dir(),
        rules,
        clock: local.get(11..19).unwrap_or_default().to_string(),
    };
    hook::step(input, &env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn settings_turn_rules_off() {
        let v = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let cases: Vec<(BTreeMap<String, String>, Rules)> = vec![
            (v(&[]), Rules::ALL),
            (
                v(&[(REREAD, "false"), (SQUEEZE, "true")]),
                Rules {
                    reread: false,
                    ..Rules::ALL
                },
            ),
            (
                v(&[(BIG_READ, "false"), (REPEAT, "false"), (SQUEEZE, "false")]),
                Rules {
                    big_read: false,
                    repeat: false,
                    squeeze: false,
                    ..Rules::ALL
                },
            ),
        ];
        for (values, want) in cases {
            assert_eq!(rules_of(&values), want, "{values:?}");
        }
        // Every rule is on by default, at today's numbers.
        let defaults: BTreeMap<String, String> = manifest()
            .settings
            .into_iter()
            .map(|s| (s.key, s.default))
            .collect();
        assert_eq!(rules_of(&defaults), Rules::ALL, "{defaults:?}");
        for k in [REREAD, BIG_READ, REPEAT, SQUEEZE] {
            assert_eq!(defaults[k], "true", "{k}");
        }
    }

    #[test]
    fn settings_set_the_numbers() {
        let v = |k: &str, x: &str| -> BTreeMap<String, String> {
            BTreeMap::from([(k.to_string(), x.to_string())])
        };
        let cases: Vec<(BTreeMap<String, String>, Rules)> = vec![
            (
                v(REREAD_KB, "8"),
                Rules {
                    small_file: 8 * 1024,
                    ..Rules::ALL
                },
            ),
            (
                v(BIG_LINES, "500"),
                Rules {
                    big_lines: 500,
                    big_bytes: 24 * 1024,
                    ..Rules::ALL
                },
            ),
            (
                v(BIG_LINES, "2000"),
                Rules {
                    big_lines: 2000,
                    big_bytes: 96 * 1024,
                    ..Rules::ALL
                },
            ),
            (
                v(REPEATS, "5"),
                Rules {
                    repeats: 5,
                    ..Rules::ALL
                },
            ),
            (
                v(SQUEEZE_LINES, "1200"),
                Rules {
                    cap_lines: 1200,
                    keep_lines: 400,
                    ..Rules::ALL
                },
            ),
            // Nonsense reads as the default.
            (v(REPEATS, "0"), Rules::ALL),
            (v(SQUEEZE_LINES, "lots"), Rules::ALL),
        ];
        for (values, want) in cases {
            assert_eq!(rules_of(&values), want, "{values:?}");
        }
    }
}
