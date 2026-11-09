//! A command the session's own rules allow, asked only for a well-known
//! variable or an escaped space in it, is answered here: Claude Code asks
//! about `echo $TMPDIR` or `du ~/Library/Xcode/iOS\ DeviceSupport`
//! whatever its allow rules say, even in bypass mode, and the cleanup
//! agent's survey asked for each. Only Bash calls, only with the rules
//! rush gave the session (Config.allow: the cleanup agent's CARE_READS),
//! only when rush_core's readrules says every command in it is one of
//! them, its variables TMPDIR, HOME, RUSH_SESSION, PWD or USER. Logged as
//! permission-auto, reason rules, so `rush why` says why nobody was
//! asked. Rust-only.

use rush_core::event;
use rush_core::tool::Kind;
use serde_json::json;

use super::{Host, State};
use crate::op::Op;

impl Host {
    /// Allows approval a when it's a command the session's own rules
    /// allow (readrules::reads_only), and says whether it did. The answer
    /// goes off the lock, as the user's would.
    pub(super) fn answer_as_rules(&self, st: &mut State, a: &event::Approval) -> bool {
        let cmd = a.call.input.command.as_str();
        if a.call.kind != Kind::Shell
            || cmd.trim().is_empty()
            || !st.cfg.allow.iter().any(|r| r.starts_with("Bash("))
            || !rush_core::readrules::reads_only(cmd, &st.cfg.allow)
        {
            return false;
        }
        self.event(
            st,
            "permission-auto",
            json!({
                "reason": "rules",
                "answer": "allow",
                "tool": a.call.name,
                "command": cmd,
                "asked": a.reason,
                "id": a.id,
            }),
        );
        let (h, id) = (self.clone(), a.id.clone());
        self.go("rules-answer", move |_| {
            let mut o = Op::named("allow");
            o.id = id;
            let _ = h.do_op(o);
        });
        true
    }
}
