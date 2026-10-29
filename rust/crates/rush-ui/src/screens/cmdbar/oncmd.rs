//! #on starts an agent on another machine's rush (a peer of rush remote
//! add): `#on MACHINE [folder] [agent=KIND] task…`. Port of
//! internal/ui/oncmd.go:30-58 (onStart: reading the argument against the
//! machines kept) and :115 (onBack). Sending it (remote.StartOn) waits on
//! the remote module's port; until then #on says so and its command goes
//! back in the box.

use crate::frame::Out;
use crate::model::Model;

/// What #on sends: the folder (the machine's own, ~ by default), the
/// agent, and the task.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartRequest {
    pub cwd: String,
    pub agent: String,
    pub prompt: String,
}

/// Reads #on's argument against the machines kept (onStart): the machine
/// named and what to start there, or what's wrong with it.
pub fn on_start(peers: &[&str], arg: &str) -> Result<(String, StartRequest), String> {
    let arg = arg.trim();
    let (name, rest) = arg.split_once(' ').unwrap_or((arg, ""));
    if name.is_empty() {
        return Err("#on MACHINE [folder] [agent=KIND] task".into());
    }
    let Some(p) = peers.iter().find(|p| p.eq_ignore_ascii_case(name)) else {
        return Err(format!("no machine {name} · rush remote add it first"));
    };
    let mut req = StartRequest {
        cwd: "~".into(),
        ..Default::default()
    };
    let mut rest = rest.trim().to_string();
    let first = |r: &str| {
        let (w, after) = r.split_once(' ').unwrap_or((r, ""));
        (w.to_string(), after.trim().to_string())
    };
    let (w, after) = first(&rest);
    if w.starts_with('/') || w == "~" || w.starts_with("~/") {
        req.cwd = w;
        rest = after;
    }
    let (w, after) = first(&rest);
    if let Some(k) = w.strip_prefix("agent=") {
        req.agent = k.to_string();
        rest = after;
    }
    if rest.is_empty() {
        return Err(format!("#on {name}: say what the agent should do"));
    }
    req.prompt = rest;
    Ok((p.to_string(), req))
}

/// Puts draft back in the Agents box if nothing has been typed there
/// since; whether it did (onBack).
pub fn on_back(m: &mut Model, draft: &str) -> bool {
    if !m.input.is_empty() {
        return false;
    }
    m.input.insert_str(draft);
    true
}

/// #on typed: read, and since sending waits on the remote module, said so
/// with the command back in the box.
pub fn on_command(m: &mut Model, arg: &str) -> Out {
    let draft = format!("#on {arg}").trim().to_string();
    on_back(m, &draft);
    if arg.trim().is_empty() {
        m.flash_err("#on MACHINE [folder] [agent=KIND] task");
    } else {
        m.flash_err("starting on another machine isn't in this rush yet · your command is back in the box");
    }
    Out::Draw
}
