//! The commands a plugin adds to rush's own CLI: `rush <plugin> <name>
//! [args]`. rush hands the plugin the arguments and prints what it
//! answers; it runs in the plugin, with what the plugin may do, not as you.
//! Ported from internal/plugin/cli.go.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::ctx::Ctx;
use crate::manifest::{Manifest, PROTO_RUSH, name_ok};
use crate::text::go_q;

/// A command a plugin adds to rush's own (Go's CLISpec).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CliSpec {
    pub name: String,
    /// its arguments, as help shows them
    #[serde(skip_serializing_if = "String::is_empty")]
    pub usage: String,
    pub description: String,
}

/// A CLI command for a plugin to run: `cli.run`'s params.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CliRun {
    pub command: String,
    #[serde(serialize_with = "nil_if_empty")]
    pub args: Vec<String>,
    /// The folder it was run in: to say what it's about, not to read,
    /// which the sandbox doesn't allow.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
}

fn nil_if_empty<S: serde::Serializer>(v: &[String], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        return s.serialize_none();
    }
    v.serialize(s)
}

/// What a plugin answers `cli.run` with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CliResult {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub exit: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

// Limits on a plugin's CLI.
pub(crate) const MAX_CLI: usize = 16;
pub(crate) const MAX_CLI_ARGS: usize = 64;
pub(crate) const MAX_CLI_ARG: usize = 4 << 10;
/// Of each of stdout and stderr.
pub const MAX_CLI_OUTPUT: usize = 1 << 20;
/// How long a CLI command may take.
pub const CLI_TIMEOUT: Duration = Duration::from_secs(120);

/// ^[^\x00-\x1f\x7f\x{80}-\x{9f}]{0,120}$: one short line.
fn cli_line_ok(s: &str) -> bool {
    s.chars().count() <= 120
        && !s
            .chars()
            .any(|c| (c as u32) < 0x20 || c as u32 == 0x7f || (0x80..=0x9f).contains(&(c as u32)))
}

impl Manifest {
    pub(crate) fn validate_cli(&self) -> Result<(), String> {
        if self.cli.is_empty() {
            return Ok(());
        }
        if self.proto() != PROTO_RUSH {
            return Err("an MCP plugin cannot add CLI commands: it has no way to run them".into());
        }
        if self.cli.len() > MAX_CLI {
            return Err(format!("cli: at most {MAX_CLI} commands"));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.cli {
            if !name_ok(&c.name) {
                return Err(format!(
                    "cli command {}: use lowercase letters, digits and dashes, starting with a letter",
                    go_q(&c.name)
                ));
            }
            if !seen.insert(c.name.as_str()) {
                return Err(format!("cli command {} is there twice", go_q(&c.name)));
            }
            if c.description.is_empty() || !cli_line_ok(&c.description) || !cli_line_ok(&c.usage) {
                return Err(format!(
                    "cli command {} needs a description of one short line, and a usage of one",
                    go_q(&c.name)
                ));
            }
        }
        Ok(())
    }

    /// The manifest's CLI command name.
    pub fn cli_command(&self, name: &str) -> Option<&CliSpec> {
        self.cli.iter().find(|c| c.name == name)
    }

    /// Checks what the CLI hands a plugin.
    pub fn check_cli_run(&self, r: &CliRun) -> Result<(), String> {
        if self.cli_command(&r.command).is_none() {
            return Err(format!("{} has no command {}", self.name, go_q(&r.command)));
        }
        if r.args.len() > MAX_CLI_ARGS {
            return Err(format!("at most {MAX_CLI_ARGS} arguments"));
        }
        if r.args.iter().any(|a| a.len() > MAX_CLI_ARG) {
            return Err(format!("an argument is over {MAX_CLI_ARG} bytes"));
        }
        Ok(())
    }
}

impl CliResult {
    /// Keeps a result within MAX_CLI_OUTPUT.
    pub fn clip(mut self) -> CliResult {
        let clip = |s: &mut String| {
            if s.len() > MAX_CLI_OUTPUT {
                let mut end = MAX_CLI_OUTPUT;
                // Go cuts at the byte; a String is cut at the character
                // that byte is in.
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
                s.push_str("\n… (cut)\n");
            }
        };
        clip(&mut self.stdout);
        clip(&mut self.stderr);
        self
    }
}

/// The running plugins' manifests that add CLI commands, by name.
pub fn cli_plugins() -> BTreeMap<String, Manifest> {
    crate::bundled::enabled()
        .into_iter()
        .filter(|(_, a)| !a.manifest.cli.is_empty())
        .map(|(n, a)| (n, a.manifest))
        .collect()
}

/// Has the broker run a plugin's CLI command, starting the broker if it
/// isn't running.
pub fn run_cli(ctx: &Ctx, name: &str, r: &CliRun) -> Result<CliResult, String> {
    let c = match client::dial_broker() {
        Ok(c) => c,
        Err(_) => {
            client::ensure_broker()?;
            loop {
                if !ctx.sleep(Duration::from_millis(50)) {
                    return Err("rush's plugin broker didn't start".into());
                }
                if let Ok(c) = client::dial_broker() {
                    break c;
                }
            }
        }
    };
    #[derive(Serialize)]
    struct Params<'a> {
        plugin: &'a str,
        run: &'a CliRun,
    }
    let out = c.call(
        ctx,
        "cli",
        &Params {
            plugin: name,
            run: r,
        },
    );
    c.close();
    out.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_cli() {
        let ok = Manifest {
            name: "q".into(),
            command: vec!["q".into()],
            cli: vec![CliSpec {
                name: "send".into(),
                usage: "<id> <n>".into(),
                description: "send it".into(),
            }],
            ..Default::default()
        };
        ok.validate_cli().unwrap();
        let spec = |n: &str, d: &str| CliSpec {
            name: n.into(),
            description: d.into(),
            ..Default::default()
        };
        let bad: Vec<(&str, Manifest)> = vec![
            (
                "bad name",
                Manifest {
                    cli: vec![spec("Send", "x")],
                    ..Default::default()
                },
            ),
            (
                "no description",
                Manifest {
                    cli: vec![spec("send", "")],
                    ..Default::default()
                },
            ),
            (
                "twice",
                Manifest {
                    cli: vec![spec("a", "x"), spec("a", "y")],
                    ..Default::default()
                },
            ),
            (
                "two lines",
                Manifest {
                    cli: vec![spec("a", "x\ny")],
                    ..Default::default()
                },
            ),
            (
                "mcp",
                Manifest {
                    protocol: "mcp".into(),
                    cli: vec![spec("a", "x")],
                    ..Default::default()
                },
            ),
        ];
        for (why, m) in bad {
            assert!(m.validate_cli().is_err(), "{why}: should be refused");
        }
        assert!(
            ok.check_cli_run(&CliRun {
                command: "nope".into(),
                ..Default::default()
            })
            .is_err()
        );
        let huge = CliRun {
            command: "send".into(),
            args: vec!["x".repeat(MAX_CLI_ARG + 1)],
            ..Default::default()
        };
        assert!(ok.check_cli_run(&huge).is_err());
        let got = CliResult {
            stdout: "x".repeat(MAX_CLI_OUTPUT + 10),
            ..Default::default()
        }
        .clip();
        assert!(
            got.stdout.len() <= MAX_CLI_OUTPUT + 20,
            "clipped to {}",
            got.stdout.len()
        );
    }
}
