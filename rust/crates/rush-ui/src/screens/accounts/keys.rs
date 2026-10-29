//! The keys on an account row (Settings › Providers draws the rows): enter
//! switches to it, r renames it (or refreshes a Claude Code login's
//! expired sign-in), l signs in to another, b marks a login borrowed, d
//! forgets it. And who each agent is signed in as, read off the UI thread
//! after each reading of the list. After Go's internal/ui/accounts.go
//! (findSignIns :319, signInsMsg.applyTo :392, switchAccount :422,
//! acctSwitchedMsg.applyTo :445, addAccount :474, acctAddedMsg.applyTo
//! :517, forgetAccount :538, signInKey :559, renameSignIn :581, loginKey
//! :591) and logins.go (useLogin :565, signedInAs :554, onBorrowed :339,
//! roomyLogins :352, hasRoom :365). Switching, refreshing and adding
//! Claude Code's logins keep their sign-ins in rush's vault, which isn't
//! in this rush yet: those go through [`LoginOps`] once it is.

use rush_core::adapter::{Account, AccountSummary, Profile};
use rush_core::text::Line;
use rush_core::usage;
use rush_state::{Login, SignIn};

use super::look::{LOGINS_KIND, agent_name, harness_name};
use super::rows::{AcctRow, account_rows, expired, profile_of, start_kind};
use super::signin::{Finish, Start, sign_in};
use super::state::{LoginView, accounts, cfg, cfg_mut, ensure_store, peek, save};
use crate::frame::sheet::{Sheet, sheet_title, text_field};
use crate::frame::{Confirm, Out, Press};
use crate::model::Model;
use crate::style::*;

/// What's done with Claude Code's logins through rush's vault: set once
/// the vault is ported (switchLogin, renewLogin, addLogin, and forgetting
/// a login's keychain entry).
#[derive(Clone, Copy)]
pub struct LoginOps {
    /// Makes to the login new sessions run as; with move, idle sessions
    /// follow at once.
    pub switch: fn(&mut Model, Login, &str, bool) -> Out,
    pub renew: fn(&mut Model, &LoginView) -> Out,
    /// Signs in to a login named so.
    pub add: fn(&mut Model, &str) -> Out,
    pub forget: fn(&mut Model, &str),
}

/// The vault's operations, when there are any.
pub fn login_ops(m: &Model) -> Option<LoginOps> {
    peek(m).login_ops
}

fn no_vault(m: &mut Model) -> Out {
    m.flash_err(&format!(
        "{}'s logins can't be switched from this rush yet",
        harness_name(&m.reg, LOGINS_KIND)
    ));
    Out::Draw
}

// ---- Who each agent is signed in as ----

/// What asking an agent who it's signed in as found (signInsMsg).
#[derive(Default)]
pub struct SignIns {
    pub kind: String,
    pub summary: Option<AccountSummary>,
    pub cur: Account,
    pub err: Option<String>,
    pub known: Vec<Account>,
}

/// Asks each agent rush can switch who it's signed in as, and which
/// accounts it knows of itself; one that runs on a key its program keeps,
/// whether it has it. Off the UI thread (findSignIns).
pub fn find_sign_ins(m: &mut Model) {
    let mut asks: Vec<Box<dyn FnOnce() -> SignIns + Send>> = Vec::new();
    for ad in m.reg.installed_all() {
        let kind = ad.kind();
        if m.reg.current_kind(kind.as_str()) != kind {
            continue;
        }
        let k = kind.0.clone();
        let found = profile_of(m, &ad);
        if ad.key_checker().is_some() {
            let native = ad.authenticator().is_some();
            if found.is_none() && !native {
                continue;
            }
            let p = found.unwrap_or(Profile {
                kind: kind.clone(),
                ..Profile::default()
            });
            let ad = ad.clone();
            asks.push(Box::new(move || {
                // A native login may have made or moved its home since.
                let mut p = p;
                if ad.authenticator().is_some()
                    && let Some(f) = ad.profiles().into_iter().next()
                {
                    p = f;
                }
                let mut s = SignIns {
                    kind: k,
                    ..SignIns::default()
                };
                if let Some(kc) = ad.key_checker() {
                    s.err = kc.check_key(&p).err().map(|e| e.to_string());
                }
                s.summary = ad.account_summary_reader().map(|r| r.account_summary(&p));
                s
            }));
            continue;
        }
        if ad.accounts().is_none() {
            // Signed in by its own program, with nothing to switch: only
            // what it says of itself.
            if ad.account_summary_reader().is_some()
                && let Some(p) = found
            {
                let ad = ad.clone();
                asks.push(Box::new(move || SignIns {
                    kind: k,
                    summary: ad.account_summary_reader().map(|r| r.account_summary(&p)),
                    ..SignIns::default()
                }));
            }
            continue;
        }
        if k == LOGINS_KIND {
            continue;
        }
        let Some(p) = found else { continue };
        let ad = ad.clone();
        asks.push(Box::new(move || {
            let mut s = SignIns {
                kind: k,
                ..SignIns::default()
            };
            if let Some(acc) = ad.accounts() {
                match acc.current(&p) {
                    Ok(a) => s.cur = a,
                    Err(e) => s.err = Some(e.to_string()),
                }
            }
            s.known = ad.known().map(|k| k.known()).unwrap_or_default();
            s
        }));
    }
    for ask in asks {
        m.later(ask, took_sign_ins);
    }
}

/// Takes in who an agent is signed in as (signInsMsg.applyTo).
pub fn took_sign_ins(m: &mut Model, s: SignIns) -> Out {
    ensure_store(m);
    if let Some(v) = s.summary {
        accounts(m).summary.insert(s.kind.clone(), v);
    }
    let before = cfg(m).sign_ins.clone();
    let note = |m: &mut Model, a: &Account| {
        if !a.id.is_empty() {
            cfg_mut(m).note_sign_in(
                SignIn {
                    kind: s.kind.clone(),
                    id: a.id.clone(),
                    email: a.email.clone(),
                    plan: a.plan.clone(),
                    ..SignIn::default()
                },
                &a.name,
            );
        }
    };
    for a in &s.known {
        note(m, a);
    }
    match &s.err {
        Some(e) => {
            let a = accounts(m);
            a.why.insert(s.kind.clone(), e.clone());
            a.now.remove(&s.kind);
        }
        None => {
            note(m, &s.cur);
            let a = accounts(m);
            a.now.insert(s.kind.clone(), s.cur.id.clone());
            a.why.remove(&s.kind);
        }
    }
    // Only the rush that leads notes them in config.json: it finds them
    // too.
    if cfg(m).sign_ins != before && m.leads() {
        save(m);
    }
    Out::Draw
}

// ---- Other agents' accounts ----

/// Signs another agent's home in as a (switchAccount).
pub fn switch_account(m: &mut Model, a: Account, why: &str) -> Out {
    switch_account_and(m, a, why, None)
}

/// What runs once a switch is done.
pub type AfterSwitch = Box<dyn FnOnce(&mut Model) -> Out + Send>;

/// switch_account, then then once it's done, as Go sequences a session's
/// start after it (accountSwitch, tea.Sequence): then runs whether the
/// switch went or not.
pub fn switch_account_then(m: &mut Model, a: Account, why: &str, then: AfterSwitch) -> Out {
    switch_account_and(m, a, why, Some(then))
}

fn switch_account_and(m: &mut Model, a: Account, why: &str, then: Option<AfterSwitch>) -> Out {
    let Some(ad) = m.reg.get(a.kind.as_str()) else {
        return then.map_or(Out::Idle, |f| f(m));
    };
    let p = profile_of(m, &ad);
    let (true, Some(p)) = (ad.accounts().is_some(), p) else {
        m.flash_err(&format!("rush can't switch {}'s account", ad.name()));
        return then.map_or(Out::Draw, |f| f(m));
    };
    let why = why.to_string();
    let to = a.clone();
    m.later(
        move || {
            ad.accounts()
                .map(|acc| acc.switch(&p, &a).map_err(|e| e.to_string()))
                .unwrap_or(Ok(()))
        },
        move |m, r| {
            let out = switched(m, to, &why, r);
            match then {
                Some(f) => f(m),
                None => out,
            }
        },
    );
    Out::Draw
}

/// Another agent's home signed in as to, or why not
/// (acctSwitchedMsg.applyTo).
fn switched(m: &mut Model, to: Account, why: &str, r: Result<(), String>) -> Out {
    if let Err(e) = r {
        if why.is_empty() {
            m.fail(&format!("couldn't switch to {}: {e}", to.name));
        } else {
            // rush switched on its own
            m.news(
                &format!("rush couldn't switch to {}: {why} · {e}", to.name),
                "",
            );
        }
        return Out::Draw;
    }
    let k = to.kind.0.clone();
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    {
        let a = accounts(m);
        a.now.insert(k.clone(), to.id.clone());
        a.switched_at.insert(k.clone(), now);
    }
    if m.reg.get(&k).is_some_and(|ad| ad.known().is_some()) {
        // Which of its accounts the agent runs on is rush's to keep.
        cfg_mut(m).using.insert(k.clone(), to.id.clone());
        save(m);
    }
    let text = format!("new {} sessions run on {}", agent_name(&m.reg, &k), to.name);
    if why.is_empty() {
        m.flash(&text);
    } else {
        m.news(&format!("switched to {}: {why} · {text}", to.name), "");
    }
    super::state::read(m);
    Out::Draw
}

/// Signs in to another account of agent k (addAccount).
pub fn add_account(m: &mut Model, k: &str) -> Out {
    if k == LOGINS_KIND {
        ask_text(m, "login name", "", |m, name| add_login(m, &name));
        return Out::Draw;
    }
    let Some(ad) = m.reg.get(k) else {
        return Out::Idle;
    };
    let p = profile_of(m, &ad);
    let (true, Some(p)) = (ad.accounts().is_some(), p) else {
        if ad.authenticator().is_some() {
            return native_sign_in(m, k);
        }
        m.flash_err(&format!(
            "{} does not expose a sign-in flow to rush",
            harness_name(&m.reg, k)
        ));
        return Out::Draw;
    };
    let kind = k.to_string();
    // Making the sign-in (its home, its command) touches the disk: done
    // off the UI thread.
    let start: Start = std::sync::Arc::new(move || {
        let acc = ad.accounts().ok_or("no accounts")?;
        let (cmd, done) = acc.sign_in(&p).map_err(|e| e.to_string())?;
        let kind = kind.clone();
        let finish: Finish = Box::new(move || {
            let r = done().map_err(|e| e.to_string());
            Box::new(move |m: &mut Model| added(m, &kind, r))
        });
        Ok((cmd, finish))
    });
    sign_in(m, &agent_name(&m.reg, k), start)
}

/// Signs agent k in through its own program's sign-in (nativeSignIn).
pub fn native_sign_in(m: &mut Model, k: &str) -> Out {
    let Some(ad) = m.reg.get(k) else {
        return Out::Idle;
    };
    let p = ad.profiles().into_iter().next().unwrap_or(Profile {
        kind: ad.kind(),
        ..Profile::default()
    });
    let name = harness_name(&m.reg, k);
    let start: Start = std::sync::Arc::new(move || {
        let au = ad.authenticator().ok_or("no sign-in")?;
        let cmd = au.sign_in_command(&p).map_err(|e| e.to_string())?;
        let name = name.clone();
        let finish: Finish = Box::new(move || {
            Box::new(move |m: &mut Model| {
                m.flash(&format!("signed in to {name}"));
                find_sign_ins(m);
                Out::Draw
            })
        });
        Ok((cmd, finish))
    });
    let title = agent_name(&m.reg, k);
    sign_in(m, &title, start)
}

/// An account just signed in to (acctAddedMsg.applyTo).
fn added(m: &mut Model, kind: &str, r: Result<Account, String>) -> Out {
    let a = match r {
        Ok(a) => a,
        Err(e) => {
            m.fail(&format!(
                "didn't add a {} account: {e}",
                agent_name(&m.reg, kind)
            ));
            return Out::Draw;
        }
    };
    let known = cfg(m).sign_ins_of(kind).iter().any(|s| s.id == a.id);
    let s = cfg_mut(m).note_sign_in(
        SignIn {
            kind: kind.into(),
            id: a.id.clone(),
            email: a.email.clone(),
            plan: a.plan.clone(),
            ..SignIn::default()
        },
        &a.name,
    );
    save(m);
    if known {
        m.flash(&format!("signed in to {} again", s.name));
    } else {
        m.flash(&format!("added {} · enter switches to it", s.name));
    }
    super::state::read(m);
    Out::Draw
}

/// Drops another agent's account, after asking (forgetAccount).
pub fn forget_account(m: &mut Model, r: &AcctRow) -> Out {
    let name = r.name(m);
    if r.current {
        m.flash_err(&format!(
            "switch to another account before forgetting {name}"
        ));
        return Out::Draw;
    }
    let email = r.email();
    let who = if email.is_empty() {
        agent_name(&m.reg, &r.kind)
    } else {
        email
    };
    let (kind, acct) = (r.kind.clone(), r.acct.clone());
    m.ask(Confirm::new(
        format!(
            "Forget {name} ({who})? rush drops its saved sign-in; sessions already on it keep going."
        ),
        move |m| {
            cfg_mut(m).forget_sign_in(&kind, &acct.id);
            save(m);
            if let Some(ad) = m.reg.get(&kind)
                && ad.accounts().is_some()
            {
                m.later(
                    move || {
                        if let Some(acc) = ad.accounts() {
                            let _ = acc.forget(&acct);
                        }
                    },
                    |_, _| Out::Idle,
                );
            }
            Out::Draw
        },
    ));
    Out::Draw
}

/// Renames another agent's account (renameSignIn).
pub fn rename_sign_in(m: &mut Model, kind: &str, id: &str, v: &str) {
    for s in cfg_mut(m).sign_ins.iter_mut() {
        if s.kind == kind && s.id == id {
            s.name = v.to_string();
        }
    }
    save(m);
}

/// A key on an account row: Pass when it isn't one of these.
pub fn account_key(m: &mut Model, r: &AcctRow, k: &str) -> Out {
    if r.head {
        return match k {
            "l" => add_account(m, &r.kind),
            _ => Out::Pass,
        };
    }
    match &r.login {
        Some(lv) => login_key(m, lv, k),
        None => sign_in_key(m, r, k),
    }
}

/// A key on another agent's account (signInKey).
pub fn sign_in_key(m: &mut Model, r: &AcctRow, k: &str) -> Out {
    match k {
        "enter" => {
            if r.current {
                let n = r.name(m);
                m.flash(&format!("already on {n}"));
                return Out::Draw;
            }
            switch_account(m, r.acct.clone(), "")
        }
        "r" => {
            let n = r.name(m);
            let (kind, id) = (r.kind.clone(), r.acct.id.clone());
            ask_text(m, &format!("rename {n}"), &n, move |m, v| {
                rename_sign_in(m, &kind, &id, &v);
                Out::Draw
            });
            Out::Draw
        }
        "l" => add_account(m, &r.kind),
        "d" | "x" => forget_account(m, r),
        _ => Out::Pass,
    }
}

/// A key on one of Claude Code's logins (loginKey).
pub fn login_key(m: &mut Model, lv: &LoginView, k: &str) -> Out {
    match k {
        // n moves only new sessions, keeping warm caches.
        "enter" | "n" => {
            if lv.current {
                m.flash(&format!("already on {}", lv.name));
                return Out::Draw;
            }
            match login_ops(m) {
                Some(ops) => (ops.switch)(m, lv.login.clone(), "", k == "enter"),
                None => no_vault(m),
            }
        }
        "r" => {
            if expired(&lv.quota) {
                return match login_ops(m) {
                    Some(ops) => (ops.renew)(m, lv),
                    None => no_vault(m),
                };
            }
            let id = lv.id.clone();
            ask_text(m, &format!("rename {}", lv.name), &lv.name, move |m, v| {
                for l in cfg_mut(m).logins.iter_mut().filter(|l| l.id == id) {
                    l.name = v.clone();
                }
                save(m);
                super::state::read(m);
                Out::Draw
            });
            Out::Draw
        }
        "l" => add_login(m, &lv.name),
        "b" => {
            for l in cfg_mut(m).logins.iter_mut().filter(|l| l.id == lv.id) {
                l.borrowed = !l.borrowed;
            }
            save(m);
            super::state::read(m);
            if lv.borrowed {
                m.flash(&format!(
                    "{} is yours again · rush may switch to it",
                    lv.name
                ));
            } else {
                m.flash(&format!(
                    "{} is borrowed · rush only uses it when you pick it",
                    lv.name
                ));
            }
            Out::Draw
        }
        "d" | "x" => {
            if lv.current {
                m.flash_err(&format!(
                    "switch to another account before forgetting {}",
                    lv.name
                ));
                return Out::Draw;
            }
            let id = lv.id.clone();
            m.ask(Confirm::new(
                format!(
                    "Forget {} ({})? rush drops its saved sign-in; sessions already on it keep going.",
                    lv.name, lv.email
                ),
                move |m| {
                    cfg_mut(m).logins.retain(|l| l.id != id);
                    save(m);
                    super::state::read(m);
                    if let Some(ops) = login_ops(m) {
                        (ops.forget)(m, &id); // the keychain and its home
                    }
                    Out::Draw
                },
            ));
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// Signs in to a Claude Code login named name (addLogin).
pub fn add_login(m: &mut Model, name: &str) -> Out {
    match login_ops(m) {
        Some(ops) => (ops.add)(m, name),
        None => {
            m.flash_err(&format!(
                "{} can't sign in from rush",
                harness_name(&m.reg, LOGINS_KIND)
            ));
            Out::Draw
        }
    }
}

/// Whether the login with this id is the one signed in (signedInAs).
pub fn signed_in_as(m: &Model, id: &str) -> bool {
    peek(m).logins.iter().any(|l| l.current && l.id == id)
}

/// Switches to the account named, or with that email: one of the agent
/// new sessions run first, then any other installed agent's (useLogin).
pub fn use_login(m: &mut Model, name: &str) -> Out {
    let k = start_kind(m);
    let mut rows = account_rows(m);
    rows.sort_by_key(|r| r.kind != k);
    for r in &rows {
        if r.head
            || r.login.is_some()
            || !(r.name(m).eq_ignore_ascii_case(name) || r.email().eq_ignore_ascii_case(name))
        {
            continue;
        }
        if r.current {
            let n = r.name(m);
            m.flash(&format!("already on {n}"));
            return Out::Draw;
        }
        return switch_account(m, r.acct.clone(), "");
    }
    let found = cfg(m)
        .logins
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(name) || l.email.eq_ignore_ascii_case(name))
        .cloned();
    if let Some(l) = found {
        if signed_in_as(m, &l.id) {
            m.flash(&format!("already on {}", l.name));
            return Out::Draw;
        }
        return match login_ops(m) {
            Some(ops) => (ops.switch)(m, l, "", true),
            None => no_vault(m),
        };
    }
    m.flash_err(&format!("no account named {name}"));
    Out::Draw
}

/// Whether the login in use is borrowed (onBorrowed).
pub fn on_borrowed(m: &Model) -> bool {
    peek(m)
        .logins
        .iter()
        .find(|l| l.current)
        .is_some_and(|l| l.borrowed)
}

fn fresh(q: &usage::Quota, now: rush_core::time::Ms) -> bool {
    now - q.fetched_at < 3 * usage::EVERY.as_millis() as rush_core::time::Ms
}

/// The logins not in use with a recent reading below where rush switches
/// away from one, by id: a session on one stays there when the one in
/// use runs out (roomyLogins).
pub fn roomy_logins(m: &Model) -> std::collections::BTreeMap<String, String> {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    peek(m)
        .logins
        .iter()
        .filter(|l| {
            !l.current
                && !l.id.is_empty()
                && fresh(&l.quota, now)
                && !l.quota.windows.is_empty()
                && l.quota.used("") < rush_state::config::SWITCH_AT
                && !l.quota.nearly_out("", usage::LEAD, now)
        })
        .map(|l| (l.id.clone(), l.name.clone()))
        .collect()
}

/// Whether the login in use has a recent reading below where rush
/// switches away from it (hasRoom).
pub fn has_room(m: &Model) -> bool {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    peek(m)
        .logins
        .iter()
        .find(|l| l.current)
        .is_some_and(|l| fresh(&l.quota, now) && !l.quota.nearly_out("", usage::LEAD, now))
}

// ---- A line of text, asked for ----

/// What's done with the text asked for.
pub type Then = Box<dyn FnOnce(&mut Model, String) -> Out>;

/// A question answered with a line of text: a name, a login's name
/// (Go's m.ask).
pub struct AskText {
    pub what: String,
    pub text: Vec<char>,
    then: Option<Then>,
}

/// Asks for a line of text, starting from value; then has it on enter.
pub fn ask_text(
    m: &mut Model,
    what: &str,
    value: &str,
    then: impl FnOnce(&mut Model, String) -> Out + 'static,
) {
    m.open_sheet(Box::new(AskText {
        what: what.into(),
        text: value.chars().collect(),
        then: Some(Box::new(then)),
    }));
}

impl Sheet for AskText {
    fn width(&self, _m: &Model) -> usize {
        72
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut l = line([plain("  ")]);
        join(
            &mut l,
            text_field(&self.text, self.text.len(), true, "", w.saturating_sub(4)),
        );
        vec![
            sheet_title(&self.what, "", w),
            Line::new(),
            l,
            Line::new(),
            keys_fit(w, &[("enter", "done"), ("esc", "cancel")]),
        ]
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "enter" => {
                let v: String = self.text.iter().collect::<String>().trim().to_string();
                if v.is_empty() {
                    return Out::Close;
                }
                if let Some(then) = self.then.take() {
                    let out = then(m, v);
                    return if out == Out::Quit { out } else { Out::Close };
                }
                return Out::Close;
            }
            "backspace" => {
                self.text.pop();
            }
            _ => match k.text() {
                Some(c) => self.text.push(c),
                None => return Out::Idle,
            },
        }
        Out::Draw
    }

    fn paste(&mut self, _m: &mut Model, text: &str) -> Out {
        self.text.extend(text.replace(['\r', '\n'], " ").chars());
        Out::Draw
    }
}
