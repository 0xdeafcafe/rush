//! claude -p answering one question alone, read-only, in a JSON shape.
//! Port of internal/adapters/claude/query.go.

use rush_core::adapter::{Answer, Profile, Querier, Query};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::adapter::{Claude, account};
use crate::json::nd;

impl Querier for Claude {
    /// claude -p answering q alone: no shell, no web, no settings or hooks
    /// of yours, no MCP servers, file tools kept to q.dirs, and no
    /// transcript kept, so its own runs don't show in what it reads.
    fn query_command(&self, p: &Profile, q: &Query) -> (Vec<String>, Vec<String>) {
        let mut args: Vec<String> = Vec::with_capacity(24 + 2 * q.dirs.len());
        args.extend(
            [
                "-p",
                "--model",
                &q.model,
                "--output-format",
                "json",
                "--no-session-persistence",
                "--restricted",
                "--strict-mcp-config",
                "--tools",
                "Read,Grep,Glob",
                "--permission-mode",
                "dontAsk",
                "--permission-prompts",
                "none",
                "--system-prompt",
                &q.system,
                "--max-budget-usd",
                &format!("{:.2}", q.budget),
                "--json-schema",
                &q.schema,
            ]
            .map(String::from),
        );
        let mut allow = Vec::with_capacity(3 * q.dirs.len());
        for d in &q.dirs {
            args.extend(["--add-dir".into(), d.clone()]);
            for t in ["Read", "Grep", "Glob"] {
                allow.push(format!("{t}({d}/**)"));
            }
        }
        args.extend(["--allowedTools".into(), allow.join(",")]);
        let mut a = account(p);
        if a.is_default() {
            a = self.run_as(); // the login in use, in its home
        }
        (args, a.env())
    }

    /// Reads claude -p's JSON result.
    fn query_answer(&self, stdout: &[u8]) -> Option<Answer> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R<'a> {
            #[serde(deserialize_with = "nd")]
            is_error: bool,
            #[serde(deserialize_with = "nd")]
            subtype: String,
            #[serde(deserialize_with = "nd")]
            result: String,
            #[serde(rename = "total_cost_usd", deserialize_with = "nd")]
            cost: f64,
            #[serde(rename = "structured_output", borrow)]
            structured: Option<&'a RawValue>,
        }
        let r: R = serde_json::from_slice(stdout).ok()?;
        let mut a = Answer {
            out: r
                .structured
                .map(|s| s.get().as_bytes().to_vec())
                .unwrap_or_default(),
            cost: r.cost,
            err: None,
        };
        if r.is_error {
            let first = r.result.trim().split('\n').next().unwrap_or("");
            a.err = Some(format!("{}: {first}", r.subtype).into());
        }
        Some(a)
    }

    /// Haiku and Opus.
    fn query_models(&self) -> (String, String) {
        ("haiku".into(), "opus".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Account;
    use crate::adapter::profile;

    #[test]
    fn asks_alone_and_reads_the_answer() {
        let c = Claude::new();
        let p = profile(&Account::at("x", "/cfg"));
        let q = Query {
            model: "haiku".into(),
            system: "sys".into(),
            schema: "{}".into(),
            dirs: vec!["/a".into()],
            budget: 0.5,
            ..Query::default()
        };
        let (args, env) = c.query_command(&p, &q);
        let s = args.join(" ");
        assert!(s.starts_with("-p --model haiku --output-format json --no-session-persistence"));
        assert!(s.contains("--max-budget-usd 0.50 --json-schema {} --add-dir /a"));
        assert!(s.ends_with("--allowedTools Read(/a/**),Grep(/a/**),Glob(/a/**)"));
        assert!(env.contains(&"CLAUDE_CONFIG_DIR=/cfg".to_string()));

        let a = c
            .query_answer(
                br#"{"is_error":false,"total_cost_usd":0.01,"structured_output":{"k":1}}"#,
            )
            .unwrap();
        assert_eq!((&a.out[..], a.cost), (&br#"{"k":1}"#[..], 0.01));
        assert!(a.err.is_none());
        let a = c
            .query_answer(
                br#"{"is_error":true,"subtype":"error_max_budget_usd","result":"over\nmore"}"#,
            )
            .unwrap();
        assert_eq!(a.err.unwrap().to_string(), "error_max_budget_usd: over");
        assert!(c.query_answer(b"nope").is_none());
    }
}
