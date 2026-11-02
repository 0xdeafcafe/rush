//! Rush suggests which conversation view fits right now (Settings ›
//! Interface › "Suggest a conversation view": off by default, suggest, or
//! switch when I come back). Only at two moments: you come back to a
//! session (open it, or look again after 10 minutes away or with turns
//! finished you haven't seen), or a turn ends while it's open; never on a
//! timer, never while you type or scroll. The rules (rush_convo::suggest)
//! pick first; when they can't tell, the session's harness's quick model is
//! asked through the advisor's Ask (its adapter's Querier, headless), off
//! the UI thread, at most once per session every 10 minutes, with counts
//! only. Suggest marks the view's tab with ✦ and says why under the box,
//! with a key to take it; switch changes the view as you come back, says
//! so, with the same key to undo, and never while you're in the session.
//! What you take and turn down is kept per moment and view in rush's
//! folder (view-suggest.json): three turned down in a row stops that one.
//! Off, none of it runs. New in the Rust port.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rush_convo::Preset;
use rush_convo::suggest::{self, Moment};
use rush_core::adapter::{Profile, Registry};
use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_draw::MouseKind;
use serde::{Deserialize, Serialize};

pub use rush_efficiency::advisor::Ask;
pub use rush_efficiency::advisor::run::Call;

use super::{agent, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// The keymap's action that takes the suggestion, or undoes a switch.
pub const ACTION: &str = "session.take_view_suggestion";
/// Its key as rush's handling knows it.
pub const KEY: &str = "alt+t";
/// A key or the wheel this recently, and a moment waits.
const QUIET: Ms = 3_000;
/// The quick model is asked at most this often for a session.
const ASK_EVERY: Ms = 10 * 60_000;
/// What its question may take, and spend.
const ASK_TIME: Duration = Duration::from_secs(30);
const ASK_BUDGET: f64 = 0.05;
/// Turned down this many times in a row, a suggestion isn't made again.
const DECLINES: u32 = 3;
/// The views you chose last that the quick model is told.
const RECENT: usize = 5;

/// The setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Suggest,
    /// Switch when you come back.
    Switch,
}

/// The setting as rush's config has it: off unless set.
pub fn mode(m: &Model) -> Mode {
    match crate::accounts::loaded_config(m).map(|c| c.view_suggest.as_str()) {
        Some("suggest") => Mode::Suggest,
        Some("switch") => Mode::Switch,
        _ => Mode::Off,
    }
}

/// A suggestion showing: the view, why, its kind ("back:catch-up") and,
/// when rush switched to it, the view it switched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub view: Preset,
    pub reason: String,
    pub kind: String,
    pub from: Option<Preset>,
}

/// How often one kind of suggestion was taken and turned down, and how
/// many times in a row it was last turned down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    #[serde(default)]
    pub taken: u32,
    #[serde(default)]
    pub declined: u32,
    #[serde(default)]
    pub streak: u32,
}

/// What's kept in rush's folder: the counts by kind, and the views you
/// chose last.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Learned {
    #[serde(default)]
    pub kinds: BTreeMap<String, Count>,
    #[serde(default)]
    pub recent: Vec<String>,
}

/// One session's looks and suggestion.
#[derive(Default)]
struct Per {
    /// The frame it was last drawn in, and when.
    frame: u64,
    at: Ms,
    /// The last finished turn seen.
    ended: u32,
    /// A moment waiting for you to stop typing or scrolling, and when you
    /// had looked before it.
    pending: Option<Moment>,
    seen: Ms,
    /// Bumped at every moment: a model's late answer for an older one is
    /// dropped.
    moment: u64,
    shown: Option<Shown>,
    /// When the quick model was last asked.
    asked: Ms,
}

/// What the suggestions keep between frames.
#[derive(Default)]
pub struct Suggestions {
    by: HashMap<String, Per>,
    learned: Option<Learned>,
    /// Kept in memory only (tests).
    no_disk: bool,
    /// What asks the quick model; None is the harness's own (advisor Cli).
    asker: Option<Arc<dyn Ask + Send + Sync>>,
    /// When a key was last pressed in a session, and the wheel turned.
    typed: Ms,
    scrolled: Ms,
}

fn path() -> PathBuf {
    rush_state::dir().join("view-suggest.json")
}

/// What's been learned, read from disk the first time.
fn learned(m: &mut Model) -> &mut Learned {
    let st = m.kept::<Suggestions>();
    if st.learned.is_none() {
        let l = if st.no_disk {
            Learned::default()
        } else {
            std::fs::read(path())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        };
        st.learned = Some(l);
    }
    st.learned.get_or_insert_with(Learned::default)
}

/// Writes what's been learned, off the UI thread.
fn save(m: &mut Model) {
    if m.kept::<Suggestions>().no_disk {
        return;
    }
    let Ok(b) = serde_json::to_vec_pretty(&*learned(m)) else {
        return;
    };
    m.later(
        move || {
            let p = path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = p.with_extension("json.tmp");
            if std::fs::write(&tmp, &b).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        },
        |_, _| Out::Idle,
    );
}

/// Counts a suggestion of kind taken or turned down.
fn note(m: &mut Model, kind: &str, taken: bool) {
    let c = learned(m).kinds.entry(kind.to_string()).or_default();
    if taken {
        c.taken += 1;
        c.streak = 0;
    } else {
        c.declined += 1;
        c.streak += 1;
    }
    save(m);
}

/// Keeps what's learned in memory only, starting empty (tests: they share
/// one home).
pub fn in_memory(m: &mut Model) {
    let st = m.kept::<Suggestions>();
    st.no_disk = true;
    st.learned = Some(Learned::default());
}

/// Asks the quick model through ask rather than the harness (tests).
pub fn use_asker(m: &mut Model, ask: Arc<dyn Ask + Send + Sync>) {
    m.kept::<Suggestions>().asker = Some(ask);
}

/// How one kind of suggestion has fared.
pub fn count(m: &mut Model, kind: &str) -> Count {
    learned(m).kinds.get(kind).copied().unwrap_or_default()
}

/// The sessions being followed for suggestions: none while it's off.
pub fn followed(m: &mut Model) -> usize {
    m.kept::<Suggestions>().by.len()
}

/// The suggestion showing in session key, if one is.
pub fn shown(m: &mut Model, key: &str) -> Option<Shown> {
    if mode(m) == Mode::Off {
        return None;
    }
    m.kept::<Suggestions>()
        .by
        .get(key)
        .and_then(|p| p.shown.clone())
}

/// The view session key's tabs mark with ✦: the one suggested, not one
/// rush switched to.
pub fn suggested(m: &mut Model, key: &str) -> Option<Preset> {
    shown(m, key).filter(|s| s.from.is_none()).map(|s| s.view)
}

/// Notes the conversation of session key drawn this frame: the moments,
/// and a suggestion once you're not typing or scrolling. Nothing when off.
pub fn look(m: &mut Model, key: &str) {
    let mode = mode(m);
    if mode == Mode::Off {
        return;
    }
    let (frame, now) = (m.frames, m.now);
    let Some(v) = m.session.as_ref().filter(|v| v.read) else {
        return;
    };
    let last_done = v
        .sess
        .turns
        .iter()
        .rev()
        .find(|t| !t.live)
        .map_or(0, |t| t.n);
    let (pframe, pat, pended) = {
        let p = m.kept::<Suggestions>().by.get(key);
        p.map_or((0, 0, 0), |p| (p.frame, p.at, p.ended))
    };
    let v = m.session.as_ref().expect("checked");
    let gap = pframe == 0 || pframe + 1 < frame;
    let moment = if gap {
        let unseen = pat > 0
            && v.sess
                .turns
                .iter()
                .rev()
                .take_while(|t| t.live || t.end > pat)
                .any(|t| !t.live);
        (pat == 0 || now - pat >= suggest::AWAY || unseen).then_some(Moment::Back)
    } else {
        (last_done > pended).then_some(Moment::Ended)
    };
    let st = m.kept::<Suggestions>();
    let p = st.by.entry(key.to_string()).or_default();
    (p.frame, p.at, p.ended) = (frame, now, last_done);
    let mut ignored = None;
    if let Some(mo) = moment {
        // One still showing at the next moment was let be: a switch kept,
        // a suggestion passed over.
        ignored = p.shown.take().map(|s| (s.kind, s.from.is_some()));
        p.pending = Some(mo);
        p.seen = pat;
        p.moment += 1;
    }
    let pending = p.pending;
    let quiet = (now - st.typed).min(now - st.scrolled).max(0);
    if let Some((kind, kept)) = ignored {
        note(m, &kind, kept);
    }
    let Some(mo) = pending else {
        return;
    };
    if quiet < QUIET {
        m.ask_wake(Instant::now() + Duration::from_millis((QUIET - quiet) as u64));
        return;
    }
    resolve(m, key, mo, mode);
}

/// Works out what fits at moment mo in session key: the rules, else the
/// quick model.
fn resolve(m: &mut Model, key: &str, mo: Moment, mode: Mode) {
    let now = m.now;
    let p = m
        .kept::<Suggestions>()
        .by
        .entry(key.to_string())
        .or_default();
    p.pending = None;
    let (seen, id, asked) = (p.seen, p.moment, p.asked);
    // Switching is only for coming back.
    if mode == Mode::Switch && mo == Moment::Ended {
        return;
    }
    let Some(d) = m
        .session
        .as_ref()
        .and_then(|v| suggest::digest(&v.sess, seen, now))
    else {
        return;
    };
    if let Some((view, why)) = suggest::rules(&d, mo) {
        offer(m, key, mo, view, why);
        return;
    }
    if asked > 0 && now - asked < ASK_EVERY {
        return;
    }
    let Some(a) = agent(m, key).cloned() else {
        return;
    };
    m.kept::<Suggestions>()
        .by
        .entry(key.to_string())
        .or_default()
        .asked = now;
    let using = super::presets::current(m);
    let recent: Vec<Preset> = learned(m)
        .recent
        .iter()
        .map(|r| Preset::from_name(r))
        .collect();
    let prompt = suggest::prompt(&d, mo, using, &recent);
    let reg = m.reg.clone();
    let asker = m.kept::<Suggestions>().asker.clone();
    let k = key.to_string();
    m.later(
        move || {
            let ask: Arc<dyn Ask + Send + Sync> =
                asker.unwrap_or_else(|| Arc::new(rush_efficiency::advisor::Cli::new(reg.clone())));
            let prof = profile(&reg, &a);
            let (quick, _) = ask.models(&prof);
            if quick.is_empty() {
                return None;
            }
            let call = Call {
                model: quick,
                budget: ASK_BUDGET,
                timeout: ASK_TIME,
                dirs: Vec::new(),
                system: suggest::SYSTEM,
                prompt,
                schema: suggest::SCHEMA.into(),
            };
            let (_, got) = ask.ask(&prof, &call);
            suggest::answer(&got.ok()?)
        },
        move |m, got| {
            let Some((view, why)) = got else {
                return Out::Idle;
            };
            // Still the moment it was asked for, in the session open.
            let same = m
                .kept::<Suggestions>()
                .by
                .get(&k)
                .is_some_and(|p| p.moment == id && p.shown.is_none());
            if !same || open_key(m).as_deref() != Some(k.as_str()) || mode(m) == Mode::Off {
                return Out::Idle;
            }
            offer(m, &k, mo, view, why);
            Out::Draw
        },
    );
}

/// The profile session a runs as, for its harness's quick model.
fn profile(reg: &Registry, a: &Agent) -> Profile {
    let fallback = || Profile {
        kind: a.kind.clone(),
        ..Profile::default()
    };
    let Some(ad) = reg.get(rush_core::adapter::migrated(a.kind.as_str()).as_str()) else {
        return fallback();
    };
    let ps = reg.profiles_of(&ad);
    ps.iter()
        .find(|p| p.name == a.job.account)
        .or(ps.first())
        .cloned()
        .unwrap_or_else(fallback)
}

/// Offers view at moment mo, unless it's the one showing or turned down
/// too often: marked as suggested, or switched to as you come back.
fn offer(m: &mut Model, key: &str, mo: Moment, view: Preset, reason: String) {
    let cur = super::presets::current(m);
    if view == cur {
        return;
    }
    let kind = format!("{}:{}", mo.name(), view.name());
    if count(m, &kind).streak >= DECLINES {
        return;
    }
    let from = match (mode(m), mo) {
        (Mode::Suggest, _) => None,
        (Mode::Switch, Moment::Back) => {
            super::presets::show(m, view);
            Some(cur)
        }
        _ => return,
    };
    m.kept::<Suggestions>()
        .by
        .entry(key.to_string())
        .or_default()
        .shown = Some(Shown {
        view,
        reason,
        kind,
        from,
    });
}

/// You chose view p (the key through them, the picker, a tab): it's
/// remembered among the last chosen, and a suggestion showing is taken
/// when it's the one, turned down when it isn't.
pub fn chose(m: &mut Model, p: Preset) {
    if mode(m) == Mode::Off {
        return;
    }
    let l = learned(m);
    l.recent.retain(|r| r != p.name());
    l.recent.push(p.name().to_string());
    let over = l.recent.len().saturating_sub(RECENT);
    l.recent.drain(..over);
    let shown = open_key(m).and_then(|k| {
        m.kept::<Suggestions>()
            .by
            .get_mut(&k)
            .and_then(|s| s.shown.take())
    });
    match shown {
        Some(s) => note(m, &s.kind, s.from.is_none() && s.view == p),
        None => save(m),
    }
}

/// Every key in a session: when you last pressed one, so a moment waits
/// while you type or move about.
pub fn watch_key(m: &mut Model, _k: &Press) -> Out {
    if mode(m) != Mode::Off {
        let now = m.now;
        m.kept::<Suggestions>().typed = now;
    }
    Out::Pass
}

/// The wheel: when it last turned.
pub fn watch_mouse(m: &mut Model, e: &Mouse) -> Out {
    if matches!(e.kind, MouseKind::ScrollUp | MouseKind::ScrollDown) && mode(m) != Mode::Off {
        let now = m.now;
        m.kept::<Suggestions>().scrolled = now;
    }
    Out::Pass
}

/// The key that takes the suggestion showing, or undoes the switch rush
/// made.
pub fn key(m: &mut Model, k: &Press) -> Out {
    if k.s != KEY || mode(m) == Mode::Off {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(s) = m
        .kept::<Suggestions>()
        .by
        .get_mut(&key)
        .and_then(|p| p.shown.take())
    else {
        return Out::Pass;
    };
    match s.from {
        None => {
            super::presets::show(m, s.view);
            note(m, &s.kind, true);
        }
        Some(prev) => {
            super::presets::show(m, prev);
            note(m, &s.kind, false);
        }
    }
    Out::Draw
}

/// The key that takes it, as the hint says it: yours if you moved it.
fn key_hint(m: &mut Model) -> String {
    let km = crate::keybind::key_map(m);
    let keys: Vec<String> = km.keys(ACTION).iter().map(|k| k.to_string()).collect();
    let k = keys
        .iter()
        .find(|k| !k.contains(' '))
        .or(keys.first())
        .cloned()
        .unwrap_or_else(|| KEY.to_string());
    rush_convo::render::key_word(&k)
}

/// The line under the box while a suggestion shows, w wide: "tab: Code
/// fits · changed 4 files, ran tests  ⌥t take it", or what rush switched to
/// and the key back.
pub fn hint(m: &mut Model, key: &str, w: usize) -> Option<Line> {
    let s = shown(m, key)?;
    let k = key_hint(m);
    let mut l = match s.from {
        None => line([
            sp("tab: ", DIM),
            sp(s.view.title(), ORANGE),
            sp(" fits", TEXT),
        ]),
        Some(_) => line([sp("switched to ", DIM), sp(s.view.title(), ORANGE)]),
    };
    if !s.reason.is_empty() {
        l.spans.push(sp(format!(" · {}", s.reason), DIM));
    }
    l.spans.push(sp("   ", DIM));
    l.spans.push(sp(k, TEXT));
    let what = match s.from {
        None => "take it".to_string(),
        Some(prev) => format!("back to {}", prev.title()),
    };
    l.spans.push(sp(format!(" {what}"), DIM));
    Some(fit(l, w))
}
