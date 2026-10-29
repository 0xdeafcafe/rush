//! Starting sessions rush hosts: a task typed in the Agents box, #new,
//! #use, the sheets and #ask all come here, and each starts through the
//! hosts the binary set ([`super::host::Hosts::start`]). Once the list
//! has the new session it's picked and opened, its box with the keys.
//! After Go's internal/ui/rushmode.go (startHosted :5341, configAs
//! :5392, sendHostedID :5419), keys.go (submit's start :850),
//! newcmd.go:51 (startAs: the account switched to first), setup.go:282
//! (accountSwitch), askrush.go:16 (askRush's config) and model.go:1056
//! (hostStartedMsg: select on load, the pane focused).

use rush_state::LOGINS_KIND;

use super::core;
use super::host::{Start, Started, hosts};
use crate::frame::Out;
use crate::frame::editor::{extract_images, look_disk};
use crate::model::Model;
use crate::screens::accounts::askrush::HostedStart;
use crate::screens::accounts::rows::{account_rows, accounts_of, start_dir, start_profile};
use crate::screens::accounts::setup::{StartOver, next_start, start_id};
use crate::screens::accounts::state::{accounts, cfg};

/// Lets the screens that start sessions start them here (set_hosts with
/// hosts that can).
pub fn install(m: &mut Model) {
    let a = accounts(m);
    a.starter = Some(starter);
    a.hosted_starter = Some(hosted_starter);
    // The cleanup agent starts here too (machinecare.go:313).
    m.kept::<crate::screens::projects_machine::CareStarter>().0 =
        Some(crate::screens::projects_machine::care_start_with_hosts);
}

/// Accounts' starter: a session on o with msg in dir, once o's account is
/// switched to (startAs).
fn starter(m: &mut Model, o: &StartOver, msg: &str, dir: &str) -> Out {
    accounts(m).start_over = Some(o.clone());
    let (msg, dir) = (msg.to_string(), dir.to_string());
    switch_then(m, o, Box::new(move |m| start_hosted(m, &msg, &dir, None)))
}

/// #ask's start (askRush): in rush's folder, never in a worktree of its
/// own, with the guide in its system prompt.
fn hosted_starter(m: &mut Model, s: HostedStart) -> Out {
    let dir = s.dir.clone();
    let task = s.task.clone();
    start_hosted(m, &task, &dir, Some(s))
}

/// Enter in the Agents box with a task (submit): a new session, on what
/// the next one starts as, in the folder new ones start in. A / command
/// isn't a task.
pub fn submit(m: &mut Model, text: &str) -> Out {
    if !hosts(m).can_start() {
        return Out::Pass;
    }
    let dir = start_dir(m);
    start_hosted(m, text, &dir, None)
}

/// o's account switched to first, when it names one other than the one
/// in use, then then (accountSwitch, then the start in sequence).
fn switch_then(m: &mut Model, o: &StartOver, then: Then) -> Out {
    if o.account.is_empty() {
        return then(m);
    }
    let want = crate::screens::accounts::setup::name_word(&o.account);
    let row = accounts_of(&account_rows(m), &o.kind)
        .into_iter()
        .find(|r| !r.current && crate::screens::accounts::setup::name_word(&r.name(m)) == want);
    let Some(r) = row else {
        return then(m);
    };
    match r.login {
        None => crate::screens::accounts::keys::switch_account_then(m, r.acct, "", then),
        Some(l) => match crate::screens::accounts::keys::login_ops(m) {
            Some(ops) => {
                // The vault switches it in place before anything else
                // reads it.
                let out = (ops.switch)(m, l.login.clone(), "", true);
                let _ = then(m);
                out
            }
            None => {
                m.flash_err(&format!(
                    "{}'s logins can't be switched from this rush yet · start it on the one in use",
                    crate::screens::accounts::look::harness_name(&m.reg, LOGINS_KIND)
                ));
                Out::Draw
            }
        },
    }
}

/// What runs once an account switch is done.
pub type Then = crate::screens::accounts::keys::AfterSwitch;

/// A new rush-mode session in dir with text its first message: the model,
/// effort and mode Settings gives its agent, or the start picked for this
/// one session; ask is #ask's (startHosted).
pub fn start_hosted(m: &mut Model, text: &str, dir: &str, ask: Option<HostedStart>) -> Out {
    let text = crate::screens::session_cards::mention::with_mentions(m, text.trim(), "");
    let (text, images) = extract_images(&text, &look_disk);
    // Each agent starts with what its own Settings page says, or what the
    // start sheet picked for this one session.
    let next = next_start(m, dir);
    let why = m.reg.unreadable(&next.kind, &next.model, &images);
    if !why.is_empty() {
        m.flash_err(&why);
        return Out::Draw;
    }
    let mut s = config_as(m, &next, dir);
    s.prompt = text;
    s.images = images;
    s.profile = if next.profile.is_empty() {
        start_profile(m, dir).name
    } else {
        next.profile.clone()
    };
    let project = rush_fleet::main_checkout(dir);
    s.own_worktree = cfg(m).own_worktree(&project);
    if let Some(a) = ask {
        if !a.name.is_empty() {
            s.name = a.name;
        }
        s.own_worktree = a.own_worktree;
        s.system_prompt = a.system_prompt;
    }
    let a = accounts(m);
    a.profile.clear(); // a profile picked with #preset is for one session
    a.start_over = None; // and a start picked on the sheet
    m.flash("starting a new session…");
    let h = hosts(m);
    m.later(move || h.start(&s), started);
    Out::Draw
}

/// A new session in dir as o starts it, with what Settings gives its
/// agent for the rest (configAs).
pub fn config_as(m: &Model, o: &StartOver, dir: &str) -> Start {
    let c = cfg(m);
    let d = &c.dispatch;
    let mut s = Start {
        kind: o.kind.clone(),
        cwd: dir.to_string(),
        model: o.model.clone(),
        effort: o.effort.clone(),
        permission_mode: d.resolved(&start_id(&m.reg, o), &o.kind).mode,
        billing: o.billing.clone(),
        ..Start::default()
    };
    if o.mode_set {
        s.permission_mode = o.mode.clone();
    }
    if o.kind == LOGINS_KIND {
        // Dispatch's own settings are this agent's, and its account the
        // one switched in.
        let f = c.active_account();
        s.account = Some((f.name, f.config_dir));
        s.limit_mode = d.on_limit.clone();
        s.lean = d.quick_start();
    }
    s
}

/// A session started, or why not (hostStartedMsg): picked once a reading
/// of the list has it, its box with the keys.
fn started(m: &mut Model, r: Result<Started, String>) -> Out {
    let st = match r {
        Ok(st) => st,
        Err(e) => {
            m.fail(&e);
            return Out::Draw;
        }
    };
    core(m).select_on_load = rush_state::dir::key(&st.acct, &format!("a:{}", st.id));
    m.flash(&format!("started {}", st.name));
    pick_started(m);
    Out::Draw
}

/// After each reading of the list: the session just started, once it's
/// in the list, picked and opened (selectOnLoad).
pub fn pick_started(m: &mut Model) {
    let key = core(m).select_on_load.clone();
    if key.is_empty() {
        return;
    }
    let Some(i) = m.agents.iter().position(|a| a.key == key) else {
        return;
    };
    core(m).select_on_load.clear();
    if m.session.is_some() {
        m.session = None;
    }
    m.select(key);
    m.open(i);
    super::live::sync(m); // its host's link, now it's showing
}

#[cfg(test)]
mod tests;
