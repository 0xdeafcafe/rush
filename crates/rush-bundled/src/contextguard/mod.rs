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

/// The context guard's.
pub fn manifest() -> Manifest {
    let on = |key: &str, title: &str, description: &str| SettingSpec {
        key: key.into(),
        title: title.into(),
        kind: "bool".into(),
        choices: Vec::new(),
        default: "true".into(),
        description: description.into(),
    };
    Manifest {
        settings: vec![
            on(
                REREAD,
                "No re-reading unchanged files",
                "Refuses an agent's read of a file it already read (the same lines or more) when the file hasn't \
                 changed since and it hasn't edited it; after its context is compacted it may read it again. Files \
                 under 2 KB are let through",
            ),
            on(
                BIG_READ,
                "No reading huge files whole",
                "Refuses, once per file, a whole read of a text file over 1,000 lines or 48 KB, and points the agent \
                 to rg -n and offset and limit; asked again, it reads it. Images, PDFs and notebooks are left alone",
            ),
            on(
                REPEAT,
                "Stop repeat failures",
                "Refuses a shell command that failed the same way (same exit, same output's end) 3 times in a row \
                 with no edit in between, and tells the agent to change something first; asked again, it runs",
            ),
            on(
                SQUEEZE,
                "Squeeze command output",
                "Takes terminal colours and codes, progress bars' redraws, spaces at line ends, runs of blank lines \
                 and repeated lines out of a shell command's output before the agent sees it, and cuts one over 600 \
                 lines to its first and last 200 (the whole kept in a file it's pointed to). Never the output of \
                 cat, sed, git diff, rg and the like, whose exact spaces an edit needs",
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
        return Rules {
            reread: false,
            big_read: false,
            repeat: false,
            squeeze: false,
        };
    }
    rules_of(&rush_plugin::setting_values_of(&manifest()))
}

/// The rules its settings' values turn on.
pub fn rules_of(values: &std::collections::BTreeMap<String, String>) -> Rules {
    let on = |k: &str| values.get(k).is_none_or(|v| v != "false");
    Rules {
        reread: on(REREAD),
        big_read: on(BIG_READ),
        repeat: on(REPEAT),
        squeeze: on(SQUEEZE),
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
        // Every setting is on by default.
        for s in manifest().settings {
            assert_eq!(s.default, "true", "{}", s.key);
        }
    }
}
