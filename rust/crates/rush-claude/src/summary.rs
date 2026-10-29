//! Summaries written by claude -p with no tools, as the login in use: how
//! a session is compacted by a cheaper Claude model. Port of
//! internal/adapters/claude/summary.go.

use std::io::Write;
use std::process::{Command, Stdio};

use rush_core::adapter::fit_tokens;
use serde::Deserialize;

use crate::json::nd;

/// Claude's models to summarise with, the cheapest first.
pub fn summary_models() -> Vec<String> {
    vec!["haiku".into(), "sonnet".into(), "opus".into()]
}

/// claude -p at prog, with env, summarising text with model under system.
pub fn summarize(
    prog: &str,
    env: &[String],
    model: &str,
    system: &str,
    text: &str,
) -> Result<String, String> {
    if prog.is_empty() {
        return Err("claude isn't installed".into());
    }
    let mut c = Command::new(prog);
    c.args([
        "-p",
        "--model",
        model,
        "--output-format",
        "json",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--tools",
        "",
        "--system-prompt",
        system,
    ])
    .env_clear()
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for e in env {
        if let Some((k, v)) = e.split_once('=') {
            c.env(k, v);
        }
    }
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let input = fit_tokens(text, 150_000);
    if let Some(mut stdin) = child.stdin.take() {
        // Written from its own thread: a long text fills the pipe before
        // claude has read it, while claude waits to be read itself.
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    read_answer(
        &out.stdout,
        out.status.success(),
        &out.status.to_string(),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// What claude -p --output-format json printed, as the summary or why
/// there isn't one.
fn read_answer(stdout: &[u8], ran: bool, status: &str, stderr: &str) -> Result<String, String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(deserialize_with = "nd")]
        is_error: bool,
        #[serde(deserialize_with = "nd")]
        result: String,
    }
    match serde_json::from_slice::<R>(stdout) {
        Err(_) if !ran => Err(format!("{status}: {stderr}").trim().to_string()),
        Err(_) => Err("its answer couldn't be read".into()),
        Ok(r) if r.is_error => Err(r.result),
        Ok(r) => Ok(r.result.trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn answers_as_claude_gives_them() {
        assert_eq!(
            read_answer(br#"{"result":" short \n"}"#, true, "", ""),
            Ok("short".into())
        );
        assert_eq!(
            read_answer(
                br#"{"is_error":true,"result":"Credit balance is too low"}"#,
                true,
                "",
                ""
            ),
            Err("Credit balance is too low".into())
        );
        assert_eq!(
            read_answer(b"", false, "exit status: 1", "bad flag"),
            Err("exit status: 1: bad flag".into())
        );
        assert_eq!(
            read_answer(b"nope", true, "", ""),
            Err("its answer couldn't be read".into())
        );
        assert_eq!(
            summarize("", &[], "haiku", "", ""),
            Err("claude isn't installed".into())
        );
    }

    // A fake claude, never the real one: it reads its stdin and answers
    // with what it was given.
    #[test]
    fn runs_a_fake_claude() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude");
        std::fs::write(
            &bin,
            "#!/bin/sh\nin=$(cat)\nprintf '{\"result\":\"%s via %s\"}' \"$in\" \"$3\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = vec![format!(
            "PATH={}",
            std::env::var("PATH").unwrap_or_default()
        )];
        let got = summarize(
            &bin.to_string_lossy(),
            &env,
            "haiku",
            "be brief",
            "the text",
        );
        assert_eq!(got, Ok("the text via haiku".into()));
    }
}
