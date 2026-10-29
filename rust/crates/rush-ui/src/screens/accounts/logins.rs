//! Claude Code's logins through rush's vault: keeping the sign-in
//! ~/.claude holds and saving the logins found (and one a Claude Code put
//! back, switched again), switching the login new sessions run as and
//! telling the rush sessions, refreshing an expired sign-in, adding a
//! login and forgetting one. After Go's internal/ui/logins.go (loginsMsg
//! :26, findLogins :63, onLogins :102, loginName :219, autoSwitch's
//! stopped and carry-on :240-277, switchLogin :307, reloginHosts :385,
//! renewLogin :474, renewedMsg.applyTo :499, addLogin :516, onAddedLogin
//! :537, hostBusy :600, stillLimited :659, restartClaude :675, homeName
//! :744) and accounts.go:650 (forgetting a login's keychain entry).
//!
//! The work is the logins agent's [`LoginKeeper`] (state.Logins): the
//! binary registers rush-claude's, a test hands in its own with
//! [`use_keeper`]. Nothing here runs without a loop (a test's own calls
//! aside): `--render`'s keys never switch a real login. Only the rush
//! that leads looks for logins, which can switch ~/.claude back.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use rush_core::adapter::Profile;
use rush_core::time::Ms;
use rush_state::vault::{FoundLogin, FoundLogins, LoginKeeper};
use rush_state::{Config, Login};
use serde_json::json;

use super::keys::{LoginOps, LoginSwitched, Restored, login_switched, logins_found, roomy_logins};
use super::look::{LOGINS_KIND, harness_name};
use super::signin::{Finish, Start, sign_in};
use super::state::{LoginView, RenewNote, accounts, cfg, cfg_mut, ensure_store, peek, read, save};
use crate::frame::Out;
use crate::model::Model;

/// How often rush looks for logins to keep (model.go:1125, every 60 ticks).
pub const FIND_EVERY: Ms = 60_000;

/// What's done with Claude Code's logins through the vault.
pub const OPS: LoginOps = LoginOps {
    switch: switch_login,
    renew: renew_login,
    add: add_login,
    forget: forget_login,
};

static KEEPER: OnceLock<Arc<dyn LoginKeeper>> = OnceLock::new();

/// The logins agent's keeper, for every model: the binary's, once.
pub fn register_keeper(k: Arc<dyn LoginKeeper>) {
    let _ = KEEPER.set(k);
}

/// The keeper this model uses, and the vault's operations with it: tests,
/// with a fake one.
pub fn use_keeper(m: &mut Model, k: Arc<dyn LoginKeeper>) {
    m.kept::<Logins>().keeper = Some(k);
    accounts(m).login_ops = Some(OPS);
}

fn keeper(m: &mut Model) -> Option<Arc<dyn LoginKeeper>> {
    m.kept::<Logins>()
        .keeper
        .clone()
        .or_else(|| KEEPER.get().cloned())
}

/// What this module keeps between calls.
#[derive(Default)]
pub struct Logins {
    keeper: Option<Arc<dyn LoginKeeper>>,
    /// When rush last looked for logins, and whether a look is out.
    found_at: Ms,
    finding: bool,
}

/// After each reading of the list, with a loop: the vault's operations
/// once there's a keeper, the hosts' carry-on, and a look for logins every
/// minute.
pub fn on_fleet(m: &mut Model) {
    if m.sender().is_none() || keeper(m).is_none() {
        return;
    }
    let a = accounts(m);
    if a.login_ops.is_none() {
        a.login_ops = Some(OPS);
    }
    if a.limit_stopped.is_none() {
        a.limit_stopped = Some(limit_stopped);
    }
    if a.relogin.is_none() {
        a.relogin = Some(carry_on);
    }
    if peek(m).store.is_none() {
        return; // looked for once the config is loaded
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let st = m.kept::<Logins>();
    if !st.finding && now - st.found_at >= FIND_EVERY {
        find_logins(m);
    }
}

/// Keeps ~/.claude's sign-in in the vault and takes in the logins found,
/// off the UI thread; only the rush that leads looks, as a look can switch
/// ~/.claude back (findLogins).
pub fn find_logins(m: &mut Model) {
    if !m.leads() {
        return;
    }
    let Some(k) = keeper(m) else { return };
    let Some(store) = peek(m).store.as_ref() else {
        return;
    };
    let cfg = store.config.clone();
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let st = m.kept::<Logins>();
    st.finding = true;
    st.found_at = now;
    m.later(move || k.find_logins(&cfg), on_logins);
}

/// Says why rush can't switch account and a login put back, then saves
/// the logins not saved yet, and who each saved one is now. A new one's
/// usage is read at once, so a switch it makes possible needn't wait
/// (onLogins).
pub fn on_logins(m: &mut Model, f: FoundLogins) -> Out {
    m.kept::<Logins>().finding = false;
    let restored = f.restored.map(|r| Restored {
        was: r.was,
        now: r.now,
        err: r.err,
    });
    logins_found(m, f.failed, restored);
    ensure_store(m);
    let (mut changed, mut added) = (false, false);
    for fl in f.found {
        let i = cfg(m).logins.iter().position(|l| l.id == fl.login.id);
        let Some(i) = i else {
            let mut l = fl.login.clone();
            l.name = login_name(m, &fl);
            rush_events::log(
                "login-added",
                obj(json!({"name": l.name, "email": l.email, "org": l.org})),
            );
            cfg_mut(m).logins.push(l);
            (changed, added) = (true, true);
            continue;
        };
        let l = &mut cfg_mut(m).logins[i];
        if l.email != fl.login.email || l.org != fl.login.org || l.profile != fl.login.profile {
            l.email = fl.login.email;
            l.org = fl.login.org;
            l.profile = fl.login.profile;
            changed = true;
        }
    }
    let c = cfg(m);
    if f.imported && (!c.folders_imported || !c.old_folders().is_empty()) {
        // Older folders' sign-ins are accounts now and their past
        // sessions are in ~/.claude: taken in once, so one you forget
        // stays forgotten. The config as it was is kept aside.
        rush_state::store::keep_before(if c.folders_imported {
            "before-folders"
        } else {
            "before-accounts"
        });
        let c = cfg_mut(m);
        c.folders_imported = true;
        c.folders = c.root_folder();
        changed = true;
    }
    if !cfg(m).active.is_empty() {
        // Which folder new sessions started in is rush's choice now.
        cfg_mut(m).active.clear();
        changed = true;
    }
    if changed {
        save(m);
    }
    if added {
        refresh_logins(m);
    }
    if changed { Out::Draw } else { Out::Idle }
}

/// What a newly found login is called: its folder's name, or its email's
/// name for ~/.claude's, unless another login has it already (loginName).
pub fn login_name(m: &Model, f: &FoundLogin) -> String {
    let mut name = f.name.clone();
    if name.is_empty() || name == home_name(m) {
        name = f.login.email.split('@').next().unwrap_or("").to_string();
    }
    if cfg(m)
        .logins
        .iter()
        .any(|l| l.name.eq_ignore_ascii_case(&name))
    {
        return f.login.email.clone();
    }
    if name.is_empty() {
        return f.login.id.chars().take(8).collect();
    }
    name
}

/// What the logins agent's own home is called (homeName).
fn home_name(m: &Model) -> String {
    m.reg
        .get(LOGINS_KIND)
        .and_then(|a| a.homer().map(|h| h.home().name))
        .unwrap_or_default()
}

/// Every saved login's plan read again but the one in use's, then the
/// readings taken in (fetchLoginUsage).
fn refresh_logins(m: &mut Model) {
    let Some(k) = keeper(m) else { return };
    let c = cfg(m).clone();
    let logins: Vec<Login> = c
        .logins
        .iter()
        .filter(|lg| !peek(m).logins.iter().any(|l| l.id == lg.id && l.current))
        .cloned()
        .collect();
    if logins.is_empty() {
        return;
    }
    m.later(
        move || {
            let path = rush_state::dir().join("usage.json");
            for lg in &logins {
                k.refresh_login(&path.to_string_lossy(), &c, lg, false);
            }
        },
        |m, ()| {
            read(m);
            Out::Idle
        },
    );
}

/// Makes to the login new sessions run as, in its home; ~/.claude stays
/// signed in as it is. With mv, idle rush sessions rest so their next
/// message starts on it, and those a limit stopped carry on now; without,
/// each follows once its cache has cooled. why is "" when you picked it
/// (switchLogin).
pub fn switch_login(m: &mut Model, to: Login, why: &str, mv: bool) -> Out {
    let Some(k) = keeper(m) else {
        return Out::Idle;
    };
    if accounts(m).switching {
        return Out::Idle;
    }
    accounts(m).switching = true;
    let c = cfg(m).clone();
    let root = c.active_account();
    let roomy = roomy_logins(m);
    let why = why.to_string();
    m.later(
        move || {
            let mut f = json!({"from": root.name, "to": to.name, "why": why, "move": mv});
            if why.is_empty() {
                f["why"] = "picked by you".into();
            }
            if let Err(e) = k.use_login(&c, &to) {
                f["err"] = e.to_string().into();
                rush_events::log("login-switch", obj(f));
                return LoginSwitched {
                    to: to.name,
                    err: Some(e.to_string()),
                    ..LoginSwitched::default()
                };
            }
            if !mv {
                rush_events::log("login-switch", obj(f));
                return LoginSwitched {
                    to: to.name,
                    why,
                    stay: true,
                    ..LoginSwitched::default()
                };
            }
            mark_moved(); // asleep ones too, which can't be told
            let (resumed, waiting) = relogin_hosts(&root.name, &c, &roomy);
            f["resumed"] = resumed.into();
            f["waiting"] = waiting.into();
            rush_events::log("login-switch", obj(f));
            LoginSwitched {
                to: to.name,
                why,
                resumed,
                waiting,
                ..LoginSwitched::default()
            }
        },
        login_switched,
    );
    Out::Draw
}

/// Refreshes a login's expired sign-in in the background, then reads its
/// usage again; its row says how it went (renewLogin, renewedMsg).
pub fn renew_login(m: &mut Model, lv: &LoginView) -> Out {
    let Some(k) = keeper(m) else {
        return Out::Idle;
    };
    if accounts(m)
        .renew
        .get(&lv.id)
        .is_some_and(|n| n.why.is_empty())
    {
        return Out::Idle; // under way
    }
    accounts(m)
        .renew
        .insert(lv.id.clone(), RenewNote::default());
    let (c, lg) = (cfg(m).clone(), lv.login.clone());
    m.later(
        move || {
            let err = k.renew_login(&c, &lg).err();
            if err.is_none() {
                let path = rush_state::dir().join("usage.json");
                k.refresh_login(&path.to_string_lossy(), &c, &lg, false);
                // A sign-in that works again: what the old one did no
                // longer counts.
                rush_state::health::reset(LOGINS_KIND, &lg.id);
            }
            (lg, err)
        },
        |m, (lg, err)| {
            if let Some(why) = err {
                let at = m.clock.unwrap_or_else(rush_core::time::now);
                accounts(m).renew.insert(lg.id, RenewNote { why, at });
                return Out::Draw;
            }
            accounts(m).renew.remove(&lg.id);
            m.flash(&format!("refreshed {}'s sign-in", lg.name));
            read(m);
            Out::Draw
        },
    );
    Out::Draw
}

/// Signs in to an account in a folder of its own, then keeps the sign-in
/// in the vault and removes the folder: ~/.claude stays as it is until
/// you switch to it (addLogin).
pub fn add_login(m: &mut Model, name: &str) -> Out {
    let ad = m.reg.get(LOGINS_KIND).filter(|a| a.loginer().is_some());
    let (Some(ad), Some(k)) = (ad, keeper(m)) else {
        m.flash_err(&format!(
            "{} can't sign in from rush",
            harness_name(&m.reg, LOGINS_KIND)
        ));
        return Out::Draw;
    };
    let title = format!("{} · {name}", harness_name(&m.reg, LOGINS_KIND));
    let name = name.to_string();
    let start: Start = Arc::new(move || {
        let scratch = Profile {
            kind: LOGINS_KIND.into(),
            name: name.clone(),
            dir: rush_state::dir()
                .join(format!("signin-{}", scratch_hex()))
                .to_string_lossy()
                .into_owned(),
        };
        let cmd = ad.loginer().ok_or("no sign-in")?.login(&scratch);
        let (k, name) = (k.clone(), name.clone());
        let finish: Finish = Box::new(move || {
            let r = k.adopt_login(&scratch).map_err(|e| e.to_string());
            if let Ok(l) = &r {
                rush_state::health::reset(LOGINS_KIND, &l.id);
            }
            Box::new(move |m: &mut Model| on_added_login(m, &name, r))
        });
        Ok((cmd, finish))
    });
    sign_in(m, &title, start)
}

/// Four random bytes as hex, for a sign-in's folder of its own.
fn scratch_hex() -> String {
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let x = (t.as_nanos() as u32) ^ std::process::id().rotate_left(16);
    format!("{x:08x}")
}

/// A login just signed in to from Accounts, kept (onAddedLogin).
pub fn on_added_login(m: &mut Model, name: &str, r: Result<Login, String>) -> Out {
    let mut l = match r {
        Ok(l) => l,
        Err(e) => {
            m.fail(&format!("didn't sign in to {name}: {e}"));
            return Out::Draw;
        }
    };
    l.name = name.to_string();
    let c = cfg_mut(m);
    match c.logins.iter().position(|x| x.id == l.id) {
        Some(i) => {
            l.name = c.logins[i].name.clone();
            l.borrowed = c.logins[i].borrowed;
            let said = format!("signed in to {} again ({})", l.name, l.email);
            c.logins[i] = l;
            m.flash(&said);
        }
        None => {
            let said = format!("added {} ({}) · enter switches to it", l.name, l.email);
            c.logins.push(l);
            m.flash(&said);
        }
    }
    save(m);
    refresh_logins(m);
    read(m);
    Out::Draw
}

/// Drops a forgotten login's keychain entry and home, off the UI thread
/// (accounts.go:650).
pub fn forget_login(m: &mut Model, id: &str) {
    let Some(k) = keeper(m) else { return };
    let id = id.to_string();
    m.later(
        move || {
            let _ = k.forget_login(&id);
        },
        |_, ()| Out::Idle,
    );
}

// ---- The rush sessions on the folder whose login changed ----

/// Whether a rush session on the folder named root was stopped by a usage
/// limit under a profile that moves on from it. One stopped since keeps
/// its detail but waits for nothing: counted, every session would be told
/// to move each minute (autoSwitch's stopped, logins.go:246-253).
pub fn limit_stopped(m: &Model, root: &str) -> bool {
    let c = cfg(m);
    m.agents.iter().any(|a| {
        a.rush
            && a.job.account == root
            && a.job.state != rush_core::agent::State::Stopped
            && a.job.detail.starts_with("usage limit")
            && c.profile_for(&a.job.cwd, &a.profile).limit() != rush_state::profiles::LIMIT_WAIT
    })
}

/// Sessions stopped under a login rush has switched away from since carry
/// on in the one in use, which has room (autoSwitch, logins.go:258-276).
pub fn carry_on(m: &mut Model, root: &str) -> Out {
    let c = cfg(m).clone();
    let root = root.to_string();
    m.later(
        move || {
            let (n, _) = relogin_hosts(&root, &c, &BTreeMap::new());
            rush_events::log(
                "limit-carry-on",
                obj(json!({"account": root, "resumed": n, "why": "stopped under an account rush switched away from; the one in use has room"})),
            );
            n
        },
        |m, n| {
            if n == 0 {
                return Out::Idle;
            }
            m.flash(&format!(
                "{n} stopped by a usage limit carry on, on the account now in use"
            ));
            Out::Draw
        },
    );
    Out::Draw
}

/// Leaves the mark sessions started on their login before it read: they
/// move to the login in use when they next start (host.MarkMoved).
fn mark_moved() {
    let p = rush_host::paths::moved_path();
    if let Ok(f) = std::fs::File::create(&p) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Whether a host's Claude Code is in a turn (hostBusy).
fn host_busy(i: &rush_host::wire::Info) -> bool {
    i.claude_pid != 0 && matches!(i.state.as_str(), "working" | "blocked" | "starting")
}

/// Tells every rush session in the folder named root that it's signed in
/// as another account now; how many a usage limit had stopped, and how
/// many move over once their turn ends (an idle one does at once). One
/// whose profile waits at a limit is left to wait; one running on a login
/// in roomy (by id) that no limit stopped stays on it (reloginHosts).
pub fn relogin_hosts(root: &str, c: &Config, roomy: &BTreeMap<String, String>) -> (usize, usize) {
    use rush_host::events::log_event;
    let (mut resumed, mut waiting) = (0, 0);
    for info in rush_host::client::list() {
        if (info.account != root && !info.account.is_empty()) || info.state == "stopped" {
            continue;
        }
        if let Some(name) = roomy.get(&info.login)
            && info.limit.is_none()
        {
            log_event(
                &info.id,
                "relogin",
                json!({"account": root, "how": "stays", "login": name, "why": "its own login still has room"}),
            );
            continue;
        }
        let wait = info.limit.is_some()
            && c.profile_for(&info.cwd, &info.profile).limit() == rush_state::profiles::LIMIT_WAIT;
        if !info.homes && (info.kind.is_empty() || info.kind == LOGINS_KIND) {
            // A host from before homes is stopped and started again by Go's
            // rush (replaceHost); this one leaves it as it is.
            log_event(
                &info.id,
                "relogin",
                json!({"account": root, "how": "none", "why": "a host from before homes"}),
            );
            continue;
        }
        if wait {
            log_event(
                &info.id,
                "relogin",
                json!({"account": root, "how": "none", "why": "its profile waits at a limit"}),
            );
            continue;
        }
        if info.sleeping {
            continue; // no host to tell: it starts in the new home when woken
        }
        let cl = match rush_host::client::Client::dial(&info.id) {
            Ok(cl) => cl,
            Err(e) => {
                log_event(
                    &info.id,
                    "relogin",
                    json!({"account": root, "how": "relogin", "err": e.to_string()}),
                );
                continue;
            }
        };
        let mut err = cl.relogin().err().map(|e| e.to_string());
        let mut how = "relogin";
        if err.is_none() && info.limit.is_some() && still_limited(&info.id) {
            how = "relogin, then restart its Claude Code";
            err = restart_claude(&cl, &info, rush_host::server::LIMIT_CONTINUE).err();
        }
        log_event(
            &info.id,
            "relogin",
            json!({"account": root, "how": how, "limited": info.limit.is_some(), "err": err.clone().unwrap_or_default()}),
        );
        if err.is_some() {
        } else if info.limit.is_some() {
            resumed += 1;
        } else if host_busy(&info) {
            waiting += 1;
        }
    }
    (resumed, waiting)
}

/// Waits up to 3 s for a session a limit stopped to carry on
/// (stillLimited).
fn still_limited(id: &str) -> bool {
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if rush_host::client::list()
            .iter()
            .any(|i| i.id == id && i.limit.is_none())
        {
            return false;
        }
    }
    true
}

/// Stops a session's Claude Code and sends text, which starts a fresh one
/// resuming the conversation, signed in as whoever its home is now
/// (restartClaude).
fn restart_claude(
    cl: &rush_host::client::Client,
    info: &rush_host::wire::Info,
    text: &str,
) -> Result<(), String> {
    let pid = info.claude_pid as i32;
    let r = (|| {
        if pid != 0 {
            let _ = rush_sys::proc::kill(pid, libc::SIGTERM);
            let mut gone = false;
            for _ in 0..50 {
                if rush_sys::proc::kill(pid, 0).is_err() {
                    gone = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if !gone {
                let _ = rush_sys::proc::kill(pid, libc::SIGKILL);
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        if text.is_empty() {
            return Ok(());
        }
        cl.send(text).map_err(|e| e.to_string())
    })();
    rush_host::events::log_event(
        &info.id,
        "claude-restart",
        json!({"claudePid": info.claude_pid, "text": clip_log(text), "err": r.clone().err().unwrap_or_default()}),
    );
    r
}

/// Text as the events log keeps it: 300 characters at most (clipLog).
fn clip_log(text: &str) -> String {
    if text.chars().count() <= 300 {
        return text.to_string();
    }
    text.chars().take(300).collect::<String>() + "…"
}

/// A JSON object's fields, for the events log.
fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}
