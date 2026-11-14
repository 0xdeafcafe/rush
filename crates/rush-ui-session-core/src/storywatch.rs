//! The Live view's watcher (DESIGN.md "Live"): while the Live view is on
//! screen, a model is asked what the session is doing and its answer
//! patches the story the view draws (rush_convo::live). It's asked once as
//! the view opens (what's new since it last looked: the catch-up), when a
//! turn ends, and while one runs every live::EVERY at most; never for a
//! session not on screen. Each ask is one query through the harness's
//! Querier (the advisor's Ask, as viewsuggest.rs asks): the dashboard as
//! it stands and what's new, so no process stays up between asks. Claude
//! Code's sessions ask Sonnet; others their quick model. The Now line is
//! the small-job model's (rush_core::adapter::cheap: Apple's on-device
//! model first), asked as the step running changes. New in the Rust port.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rush_convo::Preset;
use rush_convo::live::{self, Cursor, Story};
use rush_core::adapter::LEGACY_KIND;
use rush_core::time::Ms;
use rush_efficiency::advisor::Ask;
use rush_efficiency::advisor::run::Call;

use super::agent;
use crate::frame::Out;
use crate::model::Model;

/// What an ask may take, and spend.
const ASK_TIME: Duration = Duration::from_secs(90);
const ASK_BUDGET: f64 = 0.05;
/// The most sessions whose story is kept, the last looked at.
const KEPT: usize = 16;

/// One session's story and how far its watcher has been told.
#[derive(Default)]
struct Watch {
    story: Arc<Story>,
    cursor: Cursor,
    /// When it was last asked (0: never), and whether that ask is still
    /// out.
    asked: Ms,
    asking: bool,
    /// The last ask failed: the next waits live::EVERY.
    failed: bool,
    /// The step the Now line was asked for, and whether that's still out.
    now_of: String,
    now_asking: bool,
    /// When the view last drew it.
    seen: Ms,
}

#[derive(Default)]
struct Watches {
    by: HashMap<String, Watch>,
    /// What asks the watcher; None: the harness's Querier.
    asker: Option<Arc<dyn Ask + Send + Sync>>,
    /// What says the Now line; None: the small-job model.
    now: Option<Arc<NowFn>>,
}

/// Says the Now line for a step: its words in, a few words out.
pub type NowFn = dyn Fn(&str) -> Option<String> + Send + Sync;

/// Has the watcher ask through ask instead (tests).
pub fn use_asker(m: &mut Model, ask: Arc<dyn Ask + Send + Sync>) {
    m.kept::<Watches>().asker = Some(ask);
}

/// Has the Now line said by f instead (tests).
pub fn use_now(m: &mut Model, f: Arc<NowFn>) {
    m.kept::<Watches>().now = Some(f);
}

/// The story of session key, for the Live view to draw.
pub fn story(m: &mut Model, key: &str) -> Option<Arc<Story>> {
    m.kept::<Watches>().by.get(key).map(|w| w.story.clone())
}

/// Notes the conversation of session key drawn this frame in the Live
/// view: asks the watcher when it's due, and the Now line when the step
/// running has changed. Nothing in any other view.
pub fn look(m: &mut Model, key: &str) {
    if super::presets::current(m) != Preset::Live {
        return;
    }
    let now = m.now;
    if !m.session.as_ref().is_some_and(|v| v.read) {
        return;
    }
    // Taken out while the session is read, and put back.
    let st = m.kept::<Watches>();
    if !st.by.contains_key(key) && st.by.len() >= KEPT {
        let oldest = st
            .by
            .iter()
            .filter(|(_, w)| !w.asking && !w.now_asking)
            .min_by_key(|(_, w)| w.seen)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            st.by.remove(&k);
        }
    }
    let mut w = st.by.remove(key).unwrap_or_default();
    w.seen = now;
    let s = &m.session.as_ref().expect("checked").sess;
    // The Now line, for the step running: cleared as it changes, said
    // once the small-job model has.
    let running = live::running(s);
    let of = running.map(|id| id.to_string()).unwrap_or_default();
    let mut now_ask = None;
    if of != w.now_of && !w.now_asking {
        w.now_of = of.clone();
        Arc::make_mut(&mut w.story).set_now(&of, "");
        if let Some(id) = running {
            w.now_asking = true;
            now_ask = Some((of, live::step_line(s, id, now)));
        }
    }
    // The watcher, when due; after a failure, not for EVERY.
    let mut watch_ask = None;
    let wait = w.failed && now - w.asked < live::EVERY;
    if !w.asking && !wait && live::due(s, &w.cursor, w.asked, now) {
        let mut cursor = w.cursor.clone();
        w.asked = now;
        match live::delta(s, &mut cursor, now) {
            Some(d) => {
                w.asking = true;
                watch_ask = Some((cursor, live::prompt(&w.story, &d)));
            }
            None => w.cursor = cursor,
        }
    }
    if w.story.serial == 0 && w.story.note.is_empty() {
        Arc::make_mut(&mut w.story).set_note("watching…");
    }
    m.kept::<Watches>().by.insert(key.to_string(), w);
    if let Some((of, words)) = now_ask {
        ask_now(m, key, of, words);
    }
    if let Some((cursor, prompt)) = watch_ask {
        ask(m, key, cursor, prompt);
    }
}

/// Asks the watcher with prompt; its answer patches the story, and moves
/// it on to cursor. A failure is said in the story's note and tells it
/// again next time.
fn ask(m: &mut Model, key: &str, cursor: Cursor, prompt: String) {
    let Some(a) = agent(m, key).cloned() else {
        if let Some(w) = m.kept::<Watches>().by.get_mut(key) {
            w.asking = false;
        }
        return;
    };
    let reg = m.reg.clone();
    let asker = m.kept::<Watches>().asker.clone();
    let k = key.to_string();
    m.later(
        move || {
            let ask: Arc<dyn Ask + Send + Sync> =
                asker.unwrap_or_else(|| Arc::new(rush_efficiency::advisor::Cli::new(reg.clone())));
            let prof = super::viewsuggest::profile(&reg, &a);
            let model = watch_model(prof.kind.as_str(), ask.models(&prof).0);
            if model.is_empty() {
                return Err(format!("{} has no model to watch with", prof.kind.as_str()));
            }
            let call = Call {
                model,
                budget: ASK_BUDGET,
                timeout: ASK_TIME,
                dirs: Vec::new(),
                system: live::SYSTEM,
                prompt,
                schema: live::SCHEMA.into(),
            };
            let (_, got) = ask.ask(&prof, &call);
            got.map(|b| String::from_utf8_lossy(&b).into_owned())
        },
        move |m, got| {
            let Some(w) = m.kept::<Watches>().by.get_mut(&k) else {
                return Out::Idle;
            };
            w.asking = false;
            let story = Arc::make_mut(&mut w.story);
            match got.and_then(|a| story.apply(&a)) {
                Ok(()) => (w.cursor, w.failed) = (cursor, false),
                Err(e) => {
                    w.failed = true;
                    story.set_note(&format!("the watcher couldn't say: {}", one_line(&e)));
                }
            }
            Out::Draw
        },
    );
}

/// Asks the small-job model what the step `of` is doing, from its words.
fn ask_now(m: &mut Model, key: &str, of: String, words: String) {
    let reg = m.reg.clone();
    let say = m.kept::<Watches>().now.clone();
    let w = m.kept::<Watches>().by.get_mut(key).expect("added");
    w.now_asking = true;
    let k = key.to_string();
    m.later(
        move || match say {
            Some(f) => f(&words),
            None => rush_core::adapter::cheap::ask_cheap(&reg, live::NOW_SYSTEM, &words, "")
                .ok()
                .map(|(_, a)| a),
        },
        move |m, got| {
            let Some(w) = m.kept::<Watches>().by.get_mut(&k) else {
                return Out::Idle;
            };
            w.now_asking = false;
            // Still the step running: say it. Else the next look asks
            // for the one that is.
            let Some(line) = got.filter(|_| w.now_of == of) else {
                return Out::Idle;
            };
            if Arc::make_mut(&mut w.story).set_now(&of, &line) {
                Out::Draw
            } else {
                Out::Idle
            }
        },
    );
}

/// The model a session of harness kind is watched with: Sonnet for Claude
/// Code's (laying out a story is more than its quick model, Haiku, is
/// asked elsewhere), else the harness's quick one.
pub fn watch_model(kind: &str, quick: String) -> String {
    if kind == LEGACY_KIND {
        "sonnet".into()
    } else {
        quick
    }
}

fn one_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    l.chars().take(80).collect()
}
