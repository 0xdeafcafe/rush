//! The write lease in Claude Code, whatever its mode: a PreToolUse hook
//! on edits and Bash (`rush guests hook <id>`). Claude Code in auto,
//! acceptEdits or bypassPermissions asks nothing before an edit, so the
//! host's approval step never sees it; the hook takes the lease, or waits
//! for it (a look a second, at most WAIT), then refuses the call with
//! why. A session in no conversation with guests starts no rush: the
//! command's shell test finds neither guests.json nor guest-of in its
//! folder first. Rust-only.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::lease::{self, Took};
use crate::paths::dir;

/// The longest the hook waits for another participant's lease: inside
/// Claude Code's timeout for it (HOOK_TIMEOUT).
pub const WAIT: Duration = Duration::from_secs(9 * 60);
/// The seconds Claude Code gives the hook.
pub const HOOK_TIMEOUT: u64 = 600;
/// The tools it runs before.
pub const MATCHER: &str = "Edit|Write|MultiEdit|NotebookEdit|Bash";

/// A guest's mark in its own folder: its lead's id, for the hook's shell
/// test.
pub fn guest_of_path(id: &str) -> std::path::PathBuf {
    dir(id).join("guest-of")
}

/// The hook's command for session id: only a session with guests, or one,
/// starts rush.
pub fn command(id: &str) -> String {
    let Some(exe) = rush_sys::selfexe::resolved() else {
        return String::new();
    };
    let q = |p: &std::path::Path| crate::shim::quote(&p.to_string_lossy());
    format!(
        "for f in {} {}; do [ -e \"$f\" ] && exec {} guests hook {}; done; exit 0",
        q(&super::roster::path(id)),
        q(&guest_of_path(id)),
        crate::shim::quote(&exe.to_string_lossy()),
        crate::shim::quote(id)
    )
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HookInput {
    tool_name: String,
    tool_input: serde_json::Value,
    cwd: String,
}

/// Whether the call a PreToolUse hook's input names changes files.
pub fn changes(input: &[u8]) -> bool {
    let Ok(h) = serde_json::from_slice::<HookInput>(input) else {
        return false;
    };
    let s = |k: &str| h.tool_input.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match h.tool_name.as_str() {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => true,
        "Bash" => lease::shell_changes(s("command"), &h.cwd),
        _ => false,
    }
}

/// What the hook writes to refuse a call.
pub fn refusal(why: &str) -> Vec<u8> {
    #[derive(Serialize)]
    struct Specific<'a> {
        #[serde(rename = "hookEventName")]
        event: &'a str,
        #[serde(rename = "permissionDecision")]
        decision: &'a str,
        #[serde(rename = "permissionDecisionReason")]
        reason: &'a str,
    }
    #[derive(Serialize)]
    struct Out<'a> {
        #[serde(rename = "hookSpecificOutput")]
        out: Specific<'a>,
    }
    crate::json::line(&Out {
        out: Specific {
            event: "PreToolUse",
            decision: "deny",
            reason: why,
        },
    })
}

/// The hook for session id: given the call on input, it returns once the
/// session may make it, or writes a refusal to out once it has waited
/// long enough. wait sleeps between looks and says whether to look again.
pub fn hook(
    id: &str,
    input: &mut dyn Read,
    out: &mut dyn Write,
    wait: &mut dyn FnMut(Duration) -> bool,
) -> Result<(), String> {
    let mut b = Vec::new();
    input.read_to_end(&mut b).map_err(|e| e.to_string())?;
    if !changes(&b) {
        return Ok(());
    }
    let Ok(cfg) = crate::client::read_config(id) else {
        return Ok(());
    };
    let Some(lead) = super::lead_of(id, &cfg.meta) else {
        return Ok(());
    };
    let name = cfg
        .meta
        .get(super::GUEST_NAME)
        .cloned()
        .unwrap_or_else(|| super::roster::load(&lead).lead);
    let turn = crate::client::read_info(id).map_or(0, |i| i.turn);
    let start = Instant::now();
    loop {
        match lease::take(&lead, id, &name, turn).map_err(|e| e.to_string())? {
            Took::Mine => return Ok(()),
            Took::Held(l) => {
                let left = WAIT.saturating_sub(start.elapsed());
                if left.is_zero() || !wait(Duration::from_secs(1).min(left)) {
                    let why = format!(
                        "{} is changing files in this checkout (one participant at a time). Wait for its turn to end, then try again; reading is fine meanwhile.",
                        l.name
                    );
                    out.write_all(&refusal(&why)).map_err(|e| e.to_string())?;
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_calls_wait() {
        for (input, want) in [
            (
                r#"{"tool_name":"Edit","tool_input":{"file_path":"/a.rs"}}"#,
                true,
            ),
            (r#"{"tool_name":"Write","tool_input":{}}"#, true),
            (r#"{"tool_name":"NotebookEdit"}"#, true),
            (
                r#"{"tool_name":"Read","tool_input":{"file_path":"/a.rs"}}"#,
                false,
            ),
            (
                r#"{"tool_name":"Bash","tool_input":{"command":"cargo test"}}"#,
                false,
            ),
            (
                r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m x"}}"#,
                true,
            ),
            (
                r#"{"tool_name":"Bash","tool_input":{"command":"echo x > f"},"cwd":"/src"}"#,
                true,
            ),
            ("not json", false),
        ] {
            assert_eq!(changes(input.as_bytes()), want, "{input}");
        }
    }

    #[test]
    fn a_refusal_says_why() {
        let v: serde_json::Value = serde_json::from_slice(&refusal("Kimi is editing")).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            "Kimi is editing"
        );
    }

    #[test]
    fn a_session_with_no_guests_goes_on() {
        let mut out = Vec::new();
        let mut never = |_: Duration| -> bool { panic!("waited") };
        hook(
            "guests-hook-none",
            &mut br#"{"tool_name":"Edit","tool_input":{}}"#.as_slice(),
            &mut out,
            &mut never,
        )
        .unwrap();
        assert!(out.is_empty());
    }
}
