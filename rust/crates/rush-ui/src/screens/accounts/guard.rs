//! The runaway guard's part of the accounts (Rust-only): every few
//! seconds the leading rush reads each account's week (its longest
//! window, whichever provider), keeps the readings in weekuse.json, works
//! out each account's pace, and does what rush_core::guard::plan says
//! with its sessions: holds a session's next turn (its host's queue held,
//! as alt+h holds it), lets it go, or moves it to the account new
//! sessions start on, after switching that to the one with the most room.
//! A provider that reports no limits has no pace: only the cap and away
//! apply to it. Every hold, move and release is in the session's events
//! (guard-held, guard-moved, guard-released), so `rush why` tells it.
//!
//! Hosted sessions are reached through [`HostOps`], rush's hosts unless a
//! test hands in its own.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use rush_core::guard::{self, Acct, Do, History, Hold, Level, Pace, Plan, Sample, Seat};
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_host::wire::Info;
use serde_json::{Value, json};

use super::look::LOGINS_KIND;
use super::rows::{AcctRow, account_rows};
use super::state::{cfg, peek};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

const SEC: Ms = 1000;
const MIN: Ms = 60 * SEC;

/// How often the guard looks.
const EVERY: Ms = 5 * SEC;

/// Where the guard says new subagents and workflows are refused, in rush's
/// folder: `rush gate hook` reads it. It lapses on its own if no rush has
/// looked for this long.
pub const SPAWNS_FILE: &str = "guard.json";

/// What the guard holds and has moved, in rush's folder, so the next rush
/// to lead knows which held queues are its own to let go.
pub const HELD_FILE: &str = "guard-held.json";

#[derive(Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Kept {
    held: HashMap<String, Held>,
    moved: HashMap<String, Ms>,
}
const SPAWNS_FOR: Ms = 2 * MIN;

/// What the guard does to sessions and asks of the machine.
pub trait HostOps: Send + Sync {
    /// Every rush session's host info.
    fn list(&self) -> Vec<Info>;
    /// Holds session id's next turn, or lets it go.
    fn hold(&self, id: &str, on: bool) -> Result<(), String>;
    /// Has session id follow the account its agent runs on now.
    fn relogin(&self, id: &str) -> Result<(), String>;
    /// Notes an event: session id's, or rush's own when id is "".
    fn log(&self, id: &str, kind: &str, f: Value);
    /// A desktop notification.
    fn notify(&self, title: &str, body: &str);
    /// How long since any key on the machine, and whether its screen is
    /// locked; None where it can't tell.
    fn idle(&self) -> (Option<Ms>, Option<bool>);
}

/// rush's own hosts and the machine.
struct Real;

impl HostOps for Real {
    fn list(&self) -> Vec<Info> {
        rush_host::client::list()
    }
    fn hold(&self, id: &str, on: bool) -> Result<(), String> {
        rush_host::Client::dial(id)
            .and_then(|c| c.hold_queue(on))
            .map_err(|e| e.to_string())
    }
    fn relogin(&self, id: &str) -> Result<(), String> {
        rush_host::Client::dial(id)
            .and_then(|c| c.relogin())
            .map_err(|e| e.to_string())
    }
    fn log(&self, id: &str, kind: &str, f: Value) {
        if id.is_empty() {
            rush_sys::events::log(kind, f);
        } else {
            rush_host::events::log_event(id, kind, f);
        }
    }
    fn notify(&self, title: &str, body: &str) {
        let (t, b) = (title.to_string(), body.to_string());
        std::thread::spawn(move || rush_sys::actions::notify(&t, &b));
    }
    fn idle(&self) -> (Option<Ms>, Option<bool>) {
        (
            rush_sys::idle::since_input().map(|d| d.as_millis() as Ms),
            rush_sys::idle::screen_locked(),
        )
    }
}

/// A session the guard holds, why, and since when.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Held {
    pub hold: Hold,
    pub since: Ms,
}

/// The guard's own state, kept between looks.
#[derive(Default)]
pub struct State {
    /// Sessions it holds, by rush id.
    pub held: HashMap<String, Held>,
    /// When it last moved each session, by rush id.
    pub moved_at: HashMap<String, Ms>,
    /// Accounts past their pace, by key (kind:id).
    pub over: HashSet<String>,
    /// Accounts past the warning, told once each time.
    warned: HashSet<String>,
    /// Each account's pace, by key.
    pub paces: HashMap<String, Pace>,
    /// What it last decided.
    pub plan: Plan,
    /// Sessions you let through: true once the turn let through is
    /// running.
    pub let_through: HashMap<String, bool>,
    /// Whether you're away, as it last saw.
    pub away: bool,
    /// When this rush began looking: with no key since, you've been away
    /// since then.
    started: Ms,
    busy: bool,
    at: Ms,
    ops: Option<Arc<dyn HostOps>>,
}

fn state(m: &mut Model) -> &mut State {
    m.kept::<State>()
}

/// The guard's state, to read.
pub fn peek_state(m: &Model) -> Option<&State> {
    m.kept.peek::<State>()
}

/// Has the guard reach hosts through ops (tests).
pub fn use_ops(m: &mut Model, ops: Arc<dyn HostOps>) {
    state(m).ops = Some(ops);
}

/// An account's key: its agent's kind and its own id.
pub fn key_of(r: &AcctRow) -> String {
    format!("{}:{}", r.kind, r.id())
}

/// What a look starts from: the accounts, the guard as set, and what it
/// last did.
pub struct Input {
    pub settings: guard::Settings,
    /// Each account, its latest reading of its week when it has one.
    pub accts: Vec<(Acct, Option<Sample>)>,
    /// The account each agent's sessions run on when its host doesn't
    /// say, by kind.
    pub in_use: HashMap<String, String>,
    /// Agents working outside rush's sessions.
    pub others: usize,
    /// How long since a key in rush.
    pub key_idle: Ms,
    pub dir: PathBuf,
    pub now: Ms,
    pub held: HashMap<String, Held>,
    pub moved_at: HashMap<String, Ms>,
    pub over: HashSet<String>,
    pub let_through: HashMap<String, bool>,
}

/// What a look found and did; the moves are left to do once new sessions
/// start on their account.
#[derive(Default)]
pub struct Output {
    pub paces: HashMap<String, Pace>,
    pub plan: Plan,
    pub held: HashMap<String, Held>,
    pub over: HashSet<String>,
    pub let_through: HashMap<String, bool>,
    /// Sessions to move: id, the account, why.
    pub moves: Vec<Move>,
    pub away: bool,
    /// Accounts by key, as weighed.
    pub accts: Vec<Acct>,
}

/// A session to move.
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub id: String,
    pub from: String,
    pub to: String,
    pub cold: bool,
    pub context: i64,
    pub why: String,
}

/// How many agents a session has working: its turn, and each subagent and
/// workflow run (background shells don't count).
pub fn agents_of(i: &Info) -> usize {
    let bg = i
        .background
        .iter()
        .filter(|t| {
            matches!(
                t.kind.as_str(),
                "local_agent" | "remote_agent" | "in_process_teammate" | "local_workflow"
            )
        })
        .count();
    // Tasks it started that aren't in the background run in its turn:
    // subagents.
    let fg = (i.tasks.max(0) as usize).saturating_sub(i.background.len());
    usize::from(i.state == "working") + bg + fg
}

fn ms(t: rush_state::Time) -> Ms {
    if t.is_zero() { 0 } else { t.to_ms() }
}

/// One look: the readings kept, each account's pace, each session weighed
/// and held or let go as plan says.
pub fn tick(mut i: Input, ops: &dyn HostOps) -> Output {
    let now = i.now;
    // What a rush before this one held and moved is this one's too.
    let kept_path = i.dir.join(HELD_FILE);
    let kept: Kept = std::fs::read(&kept_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for (id, h) in &kept.held {
        i.held.entry(id.clone()).or_insert_with(|| h.clone());
    }
    for (id, t) in &kept.moved {
        i.moved_at.entry(id.clone()).or_insert(*t);
    }
    let path = i.dir.join(guard::FILE);
    let mut h = History::load(&path);
    let mut changed = false;
    for (a, s) in &i.accts {
        if let Some(s) = s {
            changed |= h.note(&a.id, *s, now);
        }
    }
    changed |= h.forget_stale(now);
    if changed {
        let _ = h.save(&path); // one that can't be written is read again next time
    }
    let cfg = i.settings.pace.unwrap_or_default();
    let mut out = Output::default();
    for (a, _) in &mut i.accts {
        a.pace = guard::pace(h.of(&a.id), &cfg, i.over.contains(&a.id), now);
        out.paces.insert(a.id.clone(), a.pace);
        if i.settings.pace.is_some() && a.pace.level == Level::Over {
            out.over.insert(a.id.clone());
        }
    }
    let accts: Vec<Acct> = i.accts.into_iter().map(|(a, _)| a).collect();
    // Away: no key in rush, nor anywhere on the machine, for long enough,
    // or the screen locked.
    if let Some(a) = i.settings.away {
        let (sys, locked) = ops.idle();
        out.away =
            locked == Some(true) || i.key_idle >= a.after && sys.is_none_or(|s| s >= a.after);
    }
    // The sessions: those running or ready to.
    let infos: Vec<Info> = ops
        .list()
        .into_iter()
        .filter(|x| x.state != "stopped" && !x.sleeping && !x.lost)
        .collect();
    let mut seats = Vec::new();
    for x in &infos {
        if i.held.contains_key(&x.id) && !x.queue_held {
            // You let it go yourself (alt+h): its next turn goes through.
            i.held.remove(&x.id);
            i.let_through.insert(x.id.clone(), false);
            ops.log(
                &x.id,
                "guard-released",
                json!({"why": "you let it through"}),
            );
        }
        if let Some(seen) = i.let_through.get_mut(&x.id) {
            if x.state == "working" {
                *seen = true;
            } else if *seen {
                i.let_through.remove(&x.id); // its turn has run
            }
        }
        let login = if x.login.is_empty() {
            i.in_use.get(&x.kind).cloned().unwrap_or_default()
        } else {
            format!("{}:{}", x.kind, x.login)
        };
        seats.push(Seat {
            id: x.id.clone(),
            login,
            working: x.state == "working",
            agents: agents_of(x),
            turns: x.turn,
            warm_until: ms(x.cache_warm),
            context: x.context_tokens,
            moved_at: i.moved_at.get(&x.id).copied().unwrap_or(0),
            held_since: i.held.get(&x.id).map_or(0, |h| h.since),
            queued: !x.queue.is_empty(),
            keeps_on: x.away.as_ref().is_some_and(|a| a.on()),
            let_through: i.let_through.contains_key(&x.id),
        });
    }
    let plan = guard::plan(&i.settings, &accts, &seats, i.others, out.away, now);
    let clock = |t: Ms| super::rows::clock(t, false);
    let name = |key: &str| {
        accts
            .iter()
            .find(|a| a.id == key)
            .map_or(key.to_string(), |a| a.name.clone())
    };
    for (id, d) in &plan.seats {
        let x = infos
            .iter()
            .find(|x| x.id == *id)
            .expect("a seat is a session");
        match d {
            Do::Go => {}
            Do::Hold(hold) => {
                if let Some(w) = i.held.get_mut(id) {
                    if w.hold.kind() != hold.kind() {
                        ops.log(
                            id,
                            "guard-held",
                            json!({"why": hold.say(&clock), "guard": hold.kind(), "was": w.hold.kind()}),
                        );
                    }
                    w.hold = hold.clone();
                    continue;
                }
                if x.queue_held {
                    continue; // you hold it yourself: yours to let go
                }
                if rush_core::dryrun::would("guard-hold", id, &hold.say(&clock)) {
                    continue;
                }
                if let Err(e) = ops.hold(id, true) {
                    ops.log(id, "guard-hold-failed", json!({"err": e}));
                    continue;
                }
                let acct = seats
                    .iter()
                    .find(|s| s.id == *id)
                    .map(|s| name(&s.login))
                    .unwrap_or_default();
                let mut f = json!({
                    "why": hold.say(&clock),
                    "guard": hold.kind(),
                    "account": acct,
                    "context": x.context_tokens,
                    "cacheWarm": ms(x.cache_warm) > now,
                    "working": plan.working,
                    "cap": plan.cap,
                });
                if let Hold::Pace { why, .. } = hold {
                    f["weighed"] = why.clone().into();
                }
                ops.log(id, "guard-held", f);
                i.held.insert(
                    id.clone(),
                    Held {
                        hold: hold.clone(),
                        since: now,
                    },
                );
            }
            Do::Release => {
                if let Some(was) = i.held.remove(id) {
                    if x.queue_held
                        && let Err(e) = ops.hold(id, false)
                    {
                        ops.log(id, "guard-release-failed", json!({"err": e}));
                    }
                    let why = if i.let_through.contains_key(id) {
                        "you let it through".to_string()
                    } else {
                        format!("no longer {}", was.hold.kind())
                    };
                    ops.log(
                        id,
                        "guard-released",
                        json!({"why": why, "heldFor": (now - was.since) / SEC}),
                    );
                }
            }
            Do::Move {
                to,
                from,
                cold,
                context,
                why,
            } => out.moves.push(Move {
                id: id.clone(),
                from: from.clone(),
                to: to.clone(),
                cold: *cold,
                context: *context,
                why: why.clone(),
            }),
        }
    }
    // Sessions gone: nothing to hold.
    i.held.retain(|id, _| infos.iter().any(|x| x.id == *id));
    i.let_through
        .retain(|id, _| infos.iter().any(|x| x.id == *id));
    spawns(&i.dir, plan.spawns.as_deref(), now);
    out.plan = plan;
    i.moved_at.retain(|_, t| now - *t < 24 * 60 * MIN);
    let now_kept = Kept {
        held: i.held.clone(),
        moved: i.moved_at.clone(),
    };
    if now_kept != kept {
        let _ = std::fs::write(
            &kept_path,
            serde_json::to_vec(&now_kept).unwrap_or_default(),
        );
    }
    out.held = i.held;
    out.let_through = i.let_through;
    out.accts = accts;
    out
}

/// Says where `rush gate hook` reads it whether new subagents and
/// workflows are refused, and why; none, when they aren't.
fn spawns(dir: &std::path::Path, why: Option<&str>, now: Ms) {
    let p = dir.join(SPAWNS_FILE);
    match why {
        Some(w) if rush_core::dryrun::would("guard-refuse-spawns", "", w) => {}
        Some(w) => {
            let b = json!({"refuse": w, "until": now + SPAWNS_FOR}).to_string();
            let tmp = p.with_extension(format!("json.{}", std::process::id()));
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
        None => {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// With each reading of the list: the leading rush looks again every few
/// seconds, off the UI thread, while any part of the guard is on (the
/// readings of each account's week are kept all the same, so a pace set
/// later has the hours behind it).
pub fn on_fleet(m: &mut Model) {
    // Not before the accounts have been read once: with none read, none
    // would be past its pace, and what was held would be let go.
    if m.sender().is_none()
        || !m.leads()
        || peek(m).hosted
        || peek(m).store.is_none()
        || !peek(m).read_once
    {
        return;
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let st = state(m);
    if st.busy || now - st.at < EVERY {
        return;
    }
    look(m, now);
}

/// Looks now (on_fleet's, and a test's): with a loop off the UI thread,
/// else at once.
pub fn look(m: &mut Model, now: Ms) {
    let i = input(m, now);
    let ops = state(m).ops.clone().unwrap_or_else(|| Arc::new(Real));
    let st = state(m);
    st.busy = true;
    st.at = now;
    let o2 = ops.clone();
    m.later(
        move || tick(i, o2.as_ref()),
        move |m, o| applied(m, o, ops, now),
    );
}

fn input(m: &mut Model, now: Ms) -> Input {
    let c = cfg(m).guard.clone();
    let mut accts = Vec::new();
    let mut in_use = HashMap::new();
    for r in account_rows(m).into_iter().filter(|r| !r.head) {
        let key = key_of(&r);
        let s =
            r.q.weekly()
                .filter(|w| w.resets_at != 0)
                .map(|w| Sample(r.q.fetched_at, w.percent, w.resets_at));
        if r.current {
            in_use.insert(r.kind.clone(), key.clone());
        }
        accts.push((
            Acct {
                id: key,
                name: r.name(m),
                out: r.q.used("") >= rush_state::config::SWITCH_AT,
                borrowed: r.login.as_ref().is_some_and(|l| l.borrowed),
                current: r.current,
                ..Acct::default()
            },
            s,
        ));
    }
    let others = m
        .agents
        .iter()
        .filter(|a| !a.rush && a.job.live())
        .map(|a| 1 + (a.subs.direct + a.subs.nested) as usize)
        .sum();
    let st = state(m);
    if st.started == 0 {
        st.started = now;
    }
    let last = if m.last_input != 0 {
        m.last_input
    } else {
        state(m).started
    };
    let st = state(m);
    Input {
        settings: c.settings(),
        accts,
        in_use,
        others,
        key_idle: now - last,
        dir: rush_state::dir(),
        now,
        held: st.held.clone(),
        moved_at: st.moved_at.clone(),
        over: st.over.clone(),
        let_through: st.let_through.clone(),
    }
}

/// What a look found, kept; what's newly past its pace said; new sessions
/// switched to the account the plan picked, then the sessions moved.
pub(crate) fn applied(m: &mut Model, o: Output, ops: Arc<dyn HostOps>, now: Ms) -> Out {
    let quiet = cfg(m).quiet;
    let was_over = state(m).over.clone();
    let was_all = state(m).plan.all_paced.is_some();
    let was_away = state(m).away;
    let warn = cfg(m).guard.warn();
    let name = |key: &str| {
        o.accts
            .iter()
            .find(|a| a.id == key)
            .map_or(key.to_string(), |a| a.name.clone())
    };
    let mut said: Vec<(String, bool)> = Vec::new();
    for key in o.over.difference(&was_over) {
        let p = o.paces.get(key).copied().unwrap_or_default();
        let mut s = format!(
            "{} is past its pace: {}%/h of a fair {}%/h, {}",
            name(key),
            one(p.burn),
            one(p.fair),
            guard::times(p.ratio)
        );
        match (&o.plan.new_on, o.plan.all_paced) {
            (Some(to), _) => s += &format!(" · new turns go to {} · {}", name(to), o.plan.to_why),
            (_, Some(t)) if t != 0 => {
                s += &format!(" · new turns wait till {}", super::rows::clock(t, false))
            }
            _ => {}
        }
        ops.log(
            "",
            "guard-tripped",
            json!({"account": name(key), "ratio": p.ratio, "burn": p.burn, "fair": p.fair, "frees": p.frees_at}),
        );
        said.push((s, true));
    }
    if o.plan.all_paced.is_some() && !was_all {
        said.push((
            "every account is past its pace: new turns wait".into(),
            true,
        ));
    }
    if o.away != was_away && cfg(m).guard.away {
        said.push((
            if o.away {
                "you're away: each session holds after its turn, and no new subagents start".into()
            } else {
                "welcome back: what the guard held for you being away carries on".into()
            },
            false,
        ));
    }
    let st = state(m);
    for (key, p) in &o.paces {
        if p.level == Level::Warn && p.ratio >= warn && st.warned.insert(key.clone()) {
            said.push((
                format!(
                    "{} is using its week {} as fast as it may: {}%/h of a fair {}%/h",
                    name(key),
                    guard::times(p.ratio),
                    one(p.burn),
                    one(p.fair)
                ),
                false,
            ));
        }
        if p.level == Level::Under {
            st.warned.remove(key);
        }
    }
    let st = state(m);
    st.busy = false;
    st.paces = o.paces;
    st.over = o.over;
    st.held = o.held;
    st.let_through = o.let_through;
    st.away = o.away;
    let plan = o.plan.clone();
    st.plan = o.plan;
    if cfg(m).guard.settings().on() {
        for (s, loud) in said {
            if loud && !quiet {
                ops.notify("rush's guard", &s);
            }
            m.news(&s, "");
        }
    }
    let moves = o.moves;
    if let Some(to) = &plan.new_on {
        let why = format!("the guard: {}", plan.to_why);
        // Sessions move only once new ones start there: a switch that
        // failed leaves them where they are.
        let kind = to.split_once(':').map_or("", |k| k.0).to_string();
        let was = move |m: &Model| {
            if kind == LOGINS_KIND {
                Some(peek(m).login_switched_at)
            } else {
                peek(m).switched_at.get(&kind).copied()
            }
        };
        let before = was(m);
        return switch_to(
            m,
            to,
            &why,
            Box::new(move |m| {
                if was(m) == before {
                    ops.log(
                        "",
                        "guard-move-skipped",
                        json!({"why": "new sessions couldn't be switched to its account"}),
                    );
                    return Out::Draw;
                }
                move_all(m, moves, ops, now)
            }),
        );
    }
    if !moves.is_empty() {
        return move_all(m, moves, ops, now);
    }
    Out::Draw
}

/// x to one decimal place, as a rate is said.
fn one(x: f64) -> String {
    format!("{x:.1}")
}

/// New sessions of the account's agent start on the account key (kind:id)
/// from now, then.
fn switch_to(m: &mut Model, key: &str, why: &str, then: super::keys::AfterSwitch) -> Out {
    let Some((kind, id)) = key.split_once(':') else {
        return then(m);
    };
    let action = if kind == LOGINS_KIND {
        "switch-login"
    } else {
        "switch-account"
    };
    if rush_core::dryrun::would(action, key, why) {
        return Out::Draw; // nor are sessions moved to it
    }
    if kind == LOGINS_KIND {
        let lv = peek(m).logins.iter().find(|l| l.id == id).cloned();
        match (lv, super::keys::login_ops(m)) {
            (Some(lv), Some(ops)) => (ops.switch_then)(m, lv.login, why, false, then),
            _ => then(m),
        }
    } else {
        let r = account_rows(m)
            .into_iter()
            .find(|r| !r.head && r.kind == kind && r.id() == id);
        match r {
            Some(r) => super::keys::switch_account_then(m, r.acct, why, then),
            None => then(m),
        }
    }
}

/// Moves each session to the account its agent runs on now, its move
/// noted with what it cost.
fn move_all(m: &mut Model, moves: Vec<Move>, ops: Arc<dyn HostOps>, now: Ms) -> Out {
    if moves.is_empty() {
        return Out::Draw;
    }
    for mv in &moves {
        state(m).moved_at.insert(mv.id.clone(), now);
        state(m).held.remove(&mv.id);
    }
    m.later(
        move || {
            for mv in &moves {
                if rush_core::dryrun::would("guard-move", &mv.id, &mv.why) {
                    continue;
                }
                let err = ops.relogin(&mv.id).err();
                let _ = ops.hold(&mv.id, false);
                ops.log(
                    &mv.id,
                    "guard-moved",
                    json!({"why": mv.why, "from": mv.from, "to": mv.to, "context": mv.context, "cacheCold": mv.cold, "err": err.unwrap_or_default()}),
                );
            }
        },
        |_, _| Out::Draw,
    );
    Out::Draw
}

/// Whether the guard has account key (kind:id) past its pace: rush's own
/// switching doesn't move to it.
pub fn over(m: &Model, kind: &str, id: &str) -> bool {
    peek_state(m).is_some_and(|s| s.over.contains(&format!("{kind}:{id}")))
}

/// An account's pace as the guard last worked it out.
pub fn pace_of(m: &Model, kind: &str, id: &str) -> Option<Pace> {
    peek_state(m).and_then(|s| s.paces.get(&format!("{kind}:{id}")).copied())
}

/// Why the guard holds a rush session's next turn, by its rush id.
pub fn held_say(m: &Model, id: &str) -> Option<String> {
    let h = peek_state(m)?.held.get(id)?;
    Some(h.hold.say(&|t| super::rows::clock(t, false)))
}

/// The list's note for a row the guard holds: once its turn is over.
pub fn row_note(m: &Model, a: &rush_core::agent::Agent) -> Option<String> {
    if !a.rush || a.job.live() {
        return None;
    }
    held_say(m, &a.job.id)
}

/// The header's line while the guard holds anything, or you're away:
/// how many it holds and why, and how to let one through.
pub fn header_line(m: &Model) -> Option<Line> {
    let st = peek_state(m)?;
    if !cfg(m).guard.settings().on() {
        return None;
    }
    let mut l = Line::new();
    if !st.held.is_empty() {
        let first = st.held.values().min_by_key(|h| h.since).expect("not empty");
        let say = first.hold.say(&|t| super::rows::clock(t, false));
        l.spans.push(sp(
            format!(
                "‖ {} held · {}",
                st.held.len(),
                say.trim_start_matches("held: ")
            ),
            YELLOW,
        ));
        l.spans
            .push(sp(" · alt+h in a session lets it through", DIM));
    } else if st.away {
        l.spans
            .push(sp("away · sessions hold after their turn", YELLOW));
    } else {
        let p = st
            .paces
            .values()
            .filter(|p| p.level == Level::Warn)
            .map(|p| p.ratio)
            .reduce(f64::max)?;
        l.spans
            .push(sp(format!("pace {}", guard::times(p)), YELLOW));
        l.spans.push(sp(" of an account's fair share", DIM));
    }
    Some(l)
}

/// How the guard sees an account's pace, in words, for its row: None
/// while the pace guard is off; "doesn't report its limits" for a
/// provider without readings, which only the cap and away cover.
pub fn pace_words(m: &Model, kind: &str, id: &str) -> Option<Line> {
    if !cfg(m).guard.pace {
        return None;
    }
    let Some(p) = pace_of(m, kind, id).filter(|p| p.resets_at != 0) else {
        return Some(line([sp(
            "no limits read: only the agent cap and away cover it",
            FAINT,
        )]));
    };
    if p.level == Level::Unknown {
        return Some(line([sp("pace: reading the last half hour…", FAINT)]));
    }
    let col = match p.level {
        Level::Over => RED,
        Level::Warn => YELLOW,
        _ => GREEN,
    };
    let mut l = line([
        sp(guard::times(p.ratio), col),
        sp(
            format!(
                " · {}%/h of a fair {}%/h, its week resets in {}",
                one(p.burn),
                one(p.fair),
                guard::roughly(p.resets_at - m.now)
            ),
            DIM,
        ),
    ]);
    if p.frees_at != 0 {
        l.spans.push(sp(
            format!(" · frees at {}", super::rows::clock(p.frees_at, false)),
            DIM,
        ));
    }
    Some(l)
}

/// An account row's pace, when the guard has it past the warning: "  2.1×".
pub fn pace_chip(m: &Model, kind: &str, id: &str) -> Line {
    if !cfg(m).guard.pace {
        return Line::new();
    }
    match pace_of(m, kind, id) {
        Some(p) if p.level == Level::Over => {
            line([plain("  "), sp(format!("‖ {}", guard::times(p.ratio)), RED)])
        }
        Some(p) if p.level == Level::Warn => line([plain("  "), sp(guard::times(p.ratio), YELLOW)]),
        _ => Line::new(),
    }
}
