//! A session's inbox holds messages for its running subagents, a file per
//! subagent by its id, taken by the agent's hook (rush inbox) at that
//! subagent's next tool call.
//!
//! Ported from internal/host/inbox.go (all but unread, which is the
//! server's: server.rs).

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::dir;

/// The inbox file for the main session itself: slipped into its turn at
/// its next tool call, without stopping it.
pub const MAIN_INBOX: &str = "main";

/// Heads what the hook hands over. The transcript keeps it, so a view can
/// show what follows it as your message.
pub const TELL_NOTE: &str = "The user sent you this message directly while you work. Take it into account, then carry on:\n\n";

/// The inbox file for what rush itself tells the main session.
pub const MAIN_NOTICE: &str = "rush-notice";
/// Heads a notice: it's rush's word, not yours.
pub const NOTICE_NOTE: &str = "Rush (not the user) noticed this while you work:\n\n";

pub fn inbox_dir(id: &str) -> PathBuf {
    dir(id).join("inbox")
}

/// The hook command for session id: only a waiting message starts rush,
/// so the shell's glob is all most tool calls cost.
pub fn inbox_hook(id: &str) -> String {
    let Ok(exe) = std::env::current_exe() else {
        return String::new();
    };
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let d = crate::shim::quote(&inbox_dir(id).to_string_lossy());
    format!(
        "set -- {d}/*; [ -e \"$1\" ] && exec {} inbox {d}; exit 0",
        crate::shim::quote(&exe.to_string_lossy())
    )
}

/// Whether kind's subagents can be sent messages straight.
pub fn takes_inbox(reg: &rush_core::adapter::Registry, kind: &str) -> bool {
    let h = reg.harness_of(kind);
    reg.get(h.as_str())
        .is_some_and(|a| a.takes_subagent_messages())
}

fn bad_name(sub: &str) -> bool {
    sub.contains(['/', '\\', '.'])
}

/// Leaves text for subagent sub, after anything already waiting.
pub fn tell(id: &str, sub: &str, text: &str) -> Result<(), String> {
    if sub.is_empty() || bad_name(sub) {
        return Err("no such subagent".into());
    }
    std::fs::create_dir_all(inbox_dir(id)).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(inbox_dir(id).join(sub))
        .map_err(|e| e.to_string())?;
    f.write_all(format!("{text}\n\n").as_bytes())
        .map_err(|e| e.to_string())
}

/// Leaves text for session id's main session as rush's word, when its
/// host runs and its agent takes the inbox.
pub fn notice(id: &str, text: &str) -> Result<(), String> {
    let info = crate::client::read_info(id).map_err(|e| e.to_string())?;
    if !info.inbox || !crate::alive(info.host_pid) {
        return Ok(());
    }
    tell(id, MAIN_NOTICE, text)
}

/// Whether session id's host runs.
pub fn session_alive(id: &str) -> bool {
    crate::client::read_info(id).is_ok_and(|i| crate::alive(i.host_pid))
}

/// What waits in dir's file name, taken by renaming, so two calls at once
/// can't both hand it over.
fn take(dir: &Path, name: &str) -> String {
    let p = dir.join(name);
    let taking = dir.join(format!(".{name}.taking"));
    if std::fs::rename(&p, &taking).is_err() {
        return String::new();
    }
    let b = std::fs::read(&taking).unwrap_or_default();
    let _ = std::fs::remove_file(&taking);
    String::from_utf8_lossy(&b).trim().to_string()
}

/// The hook: given a tool call's hook input on input, it writes to out
/// what's waiting in dir for the subagent that made it, and takes it.
pub fn inbox(dir: &Path, input: &mut dyn Read, out: &mut dyn Write) -> Result<(), String> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Hook {
        hook_event_name: String,
        agent_id: String,
    }
    let mut b = Vec::new();
    input.read_to_end(&mut b).map_err(|e| e.to_string())?;
    let mut h: Hook = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    if bad_name(&h.agent_id) {
        return Ok(());
    }
    let mut msg = String::new();
    if h.agent_id.is_empty() {
        h.agent_id = MAIN_INBOX.into(); // the main session's own call
        let n = take(dir, MAIN_NOTICE);
        if !n.is_empty() {
            msg = format!("{NOTICE_NOTE}{n}");
        }
    }
    let t = take(dir, &h.agent_id);
    if !t.is_empty() {
        msg = format!("{msg}\n\n{TELL_NOTE}{t}").trim().to_string();
    }
    if msg.is_empty() {
        return Ok(());
    }
    #[derive(Serialize)]
    struct Specific<'a> {
        #[serde(rename = "additionalContext")]
        additional_context: &'a str,
        #[serde(rename = "hookEventName")]
        hook_event_name: &'a str,
    }
    #[derive(Serialize)]
    struct Out<'a> {
        #[serde(rename = "hookSpecificOutput")]
        hook_specific_output: Specific<'a>,
    }
    let b = crate::json::line(&Out {
        hook_specific_output: Specific {
            additional_context: &msg,
            hook_event_name: &h.hook_event_name,
        },
    });
    out.write_all(&b).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // queue_test.go:41: a message for a subagent is handed over at its next
    // tool call, to it alone, and only once.
    #[test]
    fn inbox_hands_over_once() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a1"), "use the v2 API\n\n").unwrap();
        let mut out = Vec::new();
        inbox(
            d.path(),
            &mut br#"{"hook_event_name":"PostToolUse"}"#.as_slice(),
            &mut out,
        )
        .unwrap();
        assert!(out.is_empty(), "the main session's call took it: {out:?}");
        inbox(
            d.path(),
            &mut br#"{"hook_event_name":"PostToolUse","agent_id":"a1","tool_name":"Bash"}"#
                .as_slice(),
            &mut out,
        )
        .unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        assert!(
            s.contains(r#""hookEventName":"PostToolUse""#) && s.contains("use the v2 API"),
            "{s}"
        );
        assert_eq!(
            s,
            r#"{"hookSpecificOutput":{"additionalContext":"The user sent you this message directly while you work. Take it into account, then carry on:\n\nuse the v2 API","hookEventName":"PostToolUse"}}"#
        );
        out.clear();
        inbox(
            d.path(),
            &mut br#"{"hook_event_name":"PostToolUse","agent_id":"a1"}"#.as_slice(),
            &mut out,
        )
        .unwrap();
        assert!(out.is_empty(), "handed over twice");
    }

    // A notice is rush's word, before your message, to the main session.
    #[test]
    fn notice_goes_first_to_the_main_session() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(MAIN_NOTICE), "a build was stopped\n").unwrap();
        std::fs::write(d.path().join(MAIN_INBOX), "and look at this\n").unwrap();
        let mut out = Vec::new();
        inbox(
            d.path(),
            &mut br#"{"hook_event_name":"PostToolUse"}"#.as_slice(),
            &mut out,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["additionalContext"],
            format!("{NOTICE_NOTE}a build was stopped\n\n{TELL_NOTE}and look at this")
        );
        assert!(tell("x", "../up", "no").is_err());
    }
}
