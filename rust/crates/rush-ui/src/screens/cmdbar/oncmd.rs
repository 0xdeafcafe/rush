//! #on starts an agent on another machine's rush (a peer of rush remote
//! add): `#on MACHINE [folder] [agent=KIND] task…`. One POST of what rush
//! session --on MACHINE start sends, never retried; every way it fails
//! puts the command back in the box. Port of internal/ui/oncmd.go (onStart
//! :30, onCommand :60, onBack :115).

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

/// A start on another machine waiting for its answer (onBusy).
#[derive(Default)]
pub struct OnBusy(pub bool);

/// #on typed: read against the machines kept, then sent off the UI
/// thread (onCommand).
pub fn on_command(m: &mut Model, arg: &str) -> Out {
    let draft = format!("#on {arg}").trim().to_string();
    if m.kept::<OnBusy>().0 {
        on_back(m, &draft);
        m.flash_err(
            "a start on another machine is still waiting: wait for it before sending another",
        );
        return Out::Draw;
    }
    let cfg = match rush_remote::load_config() {
        Ok(c) => c,
        Err(e) => {
            on_back(m, &draft);
            m.fail(&e);
            return Out::Draw;
        }
    };
    let names: Vec<&str> = cfg.peers.iter().map(|p| p.name.as_str()).collect();
    let (name, req) = match on_start(&names, arg) {
        Ok(v) => v,
        Err(e) => {
            on_back(m, &draft);
            m.fail(&e);
            return Out::Draw;
        }
    };
    let Some(p) = cfg.peer(&name) else {
        on_back(m, &draft);
        m.fail(&format!("no machine {name} · rush remote add it first"));
        return Out::Draw;
    };
    if !m.imgs.path.is_empty() {
        on_back(m, &draft);
        m.flash_err("images aren't sent to another machine yet: remove them or start here");
        return Out::Draw;
    }
    let note = if crate::screens::accounts::state::peek(m)
        .start_over
        .is_some()
    {
        " · your start options aren't used there"
    } else {
        ""
    };
    m.kept::<OnBusy>().0 = true;
    m.flash(&format!("starting on {}…", p.name));
    let req = rush_remote::StartRequest {
        cwd: req.cwd,
        agent: req.agent,
        prompt: req.prompt,
        ..Default::default()
    };
    m.later(
        move || {
            let r = rush_remote::client::start_on(&p, &req);
            let attached = r.is_ok() && rush_remote::client::attached(&p.name);
            (p.name, r, attached)
        },
        move |m, (machine, r, attached)| {
            m.kept::<OnBusy>().0 = false;
            match r {
                Err(e) => {
                    let back = on_back(m, &draft);
                    if e.contains(rush_remote::client::MAYBE_STARTED) {
                        m.fail(&format!("{e} · check the list before sending it again"));
                    } else if back {
                        m.fail(&format!("{e} · your command is back in the box"));
                    } else {
                        m.fail(&format!("{e} · your command: {draft}"));
                    }
                }
                Ok(s) => {
                    let mut text = format!("started {} on {machine}{note}", s.info.name);
                    if !attached {
                        text.push_str(&format!(" · rush remote attach {machine} shows it here"));
                    }
                    m.flash(&text);
                }
            }
            Out::Draw
        },
    );
    Out::Draw
}
