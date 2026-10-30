//! What's typed while a run is watched: to a live Claude Code subagent
//! in a session rush runs, into its own queue and its inbox a step at a
//! time (or all of it now), or, where its host has no inbox, passed on by
//! the main session; to a finished one, the main session, with a note
//! under it saying so. Each message waiting for a run goes into its inbox
//! once it has taken a step since the last; one that finished first
//! hands what it still had to the main session. After Go's
//! internal/ui/rushmode.go (pickedSub :6229, relaySub :6252, sendPane's
//! relay :4745-4790), subqueue.go (subQueue :25, queueSub :34, tellSub
//! :55, onSubSent :101-127 (an automatic send noted, its failure news), subNoteLines :133, flushSubQueues :173,
//! sendSubQueueNow :216) and asksheets.go:64 (relayPrompt).
//!
//! Stopping one run alone (stopSub) is jobkeys.rs's. Not here: passing a
//! message on to a Codex thread (childSub).

use rush_convo::Subagent;
use rush_core::text::Line;

use super::host::Op;
use super::subqueue::{self, Due, SubNote};
use super::{conn, core};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::list_extras::spawns::SPAWN_PREFIX;

/// The run watched, and whether it still works.
fn watched(m: &mut Model, key: &str) -> Option<(Subagent, bool)> {
    if !super::subview::watching(m, key) {
        return None;
    }
    let now = m.now;
    let Model { session, kept, .. } = m;
    let s = &session.as_ref().filter(|v| v.key == key)?.sess;
    let c = kept.get::<super::Core>().conns.get(key)?;
    let r = &c.runs;
    let sa = r.subs.iter().find(|sa| sa.id == c.watch.open)?.clone();
    let live = r.state(s, &sa, &s.subagent_jobs(), now).1;
    Some((sa, live))
}

/// The run watched when what's typed goes to it: a live Claude Code
/// subagent in a session rush runs (relaySub).
pub fn relay_sub(m: &mut Model, key: &str) -> Option<Subagent> {
    let (sa, live) = watched(m, key)?;
    (live && conn(m, key).hosted() && !sa.id.starts_with(SPAWN_PREFIX)).then_some(sa)
}

/// Asks Claude to pass msg on to its running subagent sa word for word:
/// Claude Code takes no message into a subagent but its own SendMessage
/// tool (relayPrompt).
pub fn relay_prompt(sa: &Subagent, msg: &str) -> String {
    format!(
        "Pass this message on to your running subagent {} ({}) with SendMessage, word for word, then carry on with what you were doing. Don't act on it yourself.\n\nThe message: {msg}",
        sa.id, sa.typ
    )
}

/// How many steps run id has taken, as last read (subSteps).
fn steps(m: &mut Model, key: &str, id: &str) -> usize {
    conn(m, key).runs.stats.get(id).map_or(0, |t| t.steps)
}

/// enter or ctrl+enter (now) with something typed while a run is
/// watched: to the run, or noted as gone to the main session and left
/// for the box's own send (Pass).
pub fn send(m: &mut Model, key: &str, now: bool) -> Out {
    let text = conn(m, key).input.sent(true).trim().to_string();
    if text.is_empty() || text.starts_with('#') || text.starts_with('/') {
        return Out::Pass;
    }
    let images = text.contains("[Image #");
    if let Some(sa) = relay_sub(m, key).filter(|_| !images) {
        conn(m, key).input.clear();
        if conn(m, key).info.inbox {
            // Into its own queue, or with ctrl+enter straight in at its
            // next tool call.
            let steps = steps(m, key, &sa.id);
            let at = m.now;
            let q = core(m)
                .local_q
                .entry(subqueue::key(key, &sa.id))
                .or_default();
            if now {
                let all = subqueue::now_all(q, &text, steps, at);
                m.flash(&format!(
                    "sent to {} · it reads it after the step it's on",
                    sa.typ
                ));
                if let Some(all) = all {
                    tell(m, key, &sa, all, false);
                }
                return Out::Draw;
            }
            subqueue::queue(q, steps, &text, at);
            m.flash(&subqueue::queued_words(&sa.typ));
            return Out::Draw;
        }
        // Into the main session's turn at its next step, not its queue.
        let prompt = relay_prompt(&sa, &text);
        via(
            m,
            key,
            &sa,
            text,
            "the main session passes it on at its next step",
            Op::Guide {
                text: prompt,
                images: Vec::new(),
            },
            false,
        );
        return Out::Draw;
    }
    if let Some((sa, live)) = watched(m, key) {
        let mut why = "went to the main session".to_string();
        if !live {
            why.push_str(" · it has finished");
        } else if images {
            why.push_str(" · a subagent takes no images");
        }
        note(
            m,
            key,
            &sa.id,
            SubNote {
                text,
                how: why,
                told: false,
            },
        );
    }
    Out::Pass
}

fn note(m: &mut Model, key: &str, sub: &str, n: SubNote) {
    let notes = core(m)
        .sub_notes
        .entry(subqueue::key(key, sub))
        .or_default();
    subqueue::note(notes, n);
}

/// Puts text in run sa's inbox, for the end of its step (tellSub).
fn tell(m: &mut Model, key: &str, sa: &Subagent, text: String, auto: bool) {
    let op = Op::Tell {
        sub: sa.id.clone(),
        text: text.clone(),
    };
    via(
        m,
        key,
        sa,
        text,
        "in its inbox · it reads it when this step ends",
        op,
        auto,
    );
}

/// Sends op, meant for run sa with text; how says where it went (viaSub,
/// tellSub), auto when the queue sent it on its own.
fn via(m: &mut Model, key: &str, sa: &Subagent, text: String, how: &str, op: Op, auto: bool) {
    via_from(m, key, sa, text, how, op, (auto, false));
}

/// via, from the box under the run in the subagents view's column when
/// side: a message that doesn't go goes back there.
fn via_from(
    m: &mut Model,
    key: &str,
    sa: &Subagent,
    text: String,
    how: &str,
    op: Op,
    (auto, side): (bool, bool),
) {
    let Some(link) = conn(m, key).link.clone() else {
        if side {
            super::subview::give_back(m, key, &text);
            m.fail("couldn't send: the session isn't running in rush · back in its box");
        }
        return;
    };
    let told = matches!(op, Op::Tell { .. });
    let (key, sub, name, how) = (
        key.to_string(),
        sa.id.clone(),
        sa.typ.clone(),
        how.to_string(),
    );
    m.later(
        move || link.op(op),
        move |m, r| {
            sent(
                m,
                Sent {
                    key: &key,
                    sub: &sub,
                    name: &name,
                    text,
                    how,
                    told,
                    auto,
                    side,
                },
                r,
            )
        },
    );
}

/// How a message meant for a run went (subSentMsg).
struct Sent<'a> {
    key: &'a str,
    sub: &'a str,
    name: &'a str,
    text: String,
    how: String,
    told: bool,
    auto: bool,
    /// From the box under the run in the subagents view's column.
    side: bool,
}

/// Notes where a message went, or keeps one that didn't: back at the head
/// of its queue, else in the box, else as a note saying why (onSubSent).
fn sent(m: &mut Model, s: Sent, r: Result<(), String>) -> Out {
    match r {
        Ok(()) => {
            let how = s.how.clone();
            note(
                m,
                s.key,
                s.sub,
                SubNote {
                    text: s.text,
                    how: s.how,
                    told: s.told,
                },
            );
            if s.auto {
                m.noted(&format!("queued message sent to {}: {how}", s.name));
            } else {
                m.flash(&format!("sent to {} · {how}", s.name));
            }
        }
        Err(e) => {
            let why = format!("couldn't send to {}: {e}", s.name);
            // A message the queue sent on its own failing is news: you
            // weren't the one who sent it (onSubSent's say).
            let say = |m: &mut Model, w: &str| {
                if s.auto {
                    m.news(w, "");
                } else {
                    m.fail(w);
                }
            };
            if s.side && !s.told {
                super::subview::give_back(m, s.key, &s.text);
                say(m, &format!("{why} · back in its box"));
            } else if s.told {
                let q = core(m)
                    .local_q
                    .entry(subqueue::key(s.key, s.sub))
                    .or_default();
                q.items.insert(0, s.text);
                say(m, &format!("{why} · back in its queue"));
            } else if conn(m, s.key).input.is_empty() {
                conn(m, s.key).input.insert_str(&s.text);
                say(m, &format!("{why} · back in the box"));
            } else {
                note(
                    m,
                    s.key,
                    s.sub,
                    SubNote {
                        text: s.text,
                        how: format!("✗ not sent: {e}"),
                        told: false,
                    },
                );
                say(m, &why);
            }
        }
    }
    Out::Draw
}

/// How a message from the box under run id in the subagents view's
/// column reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// An agent rush hosts: straight to its own host (its session's id,
    /// and its agent's kind).
    Spawned { host: String, kind: String },
    /// A running subagent whose session's host has an inbox: into it, read
    /// after the step it's on.
    Inbox,
    /// A Claude Code subagent with no line of its own: the main session is
    /// asked to pass it on with its SendMessage tool, which also resumes
    /// one that has finished.
    Relay { live: bool },
}

impl Route {
    /// What the box says of where its message goes.
    pub fn hint(&self, m: &mut Model) -> String {
        match self {
            Route::Spawned { kind, .. } => super::spawned::how(m, kind, false),
            Route::Inbox => "into its inbox · it reads it after the step it's on".into(),
            Route::Relay { live: true } => {
                "no direct line · the main session passes it on with SendMessage".into()
            }
            Route::Relay { live: false } => {
                "it finished · the main session resumes it with SendMessage".into()
            }
        }
    }
}

/// Where a message for run id can go from the box under it, or why none
/// can reach it.
pub fn route(m: &mut Model, key: &str, id: &str) -> Result<Route, String> {
    let now = m.now;
    let home = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let hosted = conn(m, key).hosted();
    let inbox = conn(m, key).info.inbox;
    let Model { session, kept, .. } = m;
    let s = &session
        .as_ref()
        .filter(|v| v.key == key)
        .ok_or("the session isn't open")?
        .sess;
    let r = &kept.get::<super::Core>().conns.get(key).ok_or("")?.runs;
    if id.starts_with(SPAWN_PREFIX) {
        return match r.hosted_of(id) {
            Some(h) => Ok(Route::Spawned {
                host: h.hosted.clone(),
                kind: h.kind.clone(),
            }),
            None => Err("rush doesn't host this agent · nothing can reach it".into()),
        };
    }
    let sa = r
        .subs
        .iter()
        .find(|sa| sa.id == id)
        .ok_or("rush hasn't found this agent's conversation yet")?;
    if sa.id.starts_with(super::subs::JOB_SUB_PREFIX) {
        return Err("rush hasn't found this agent's conversation yet".into());
    }
    if !hosted {
        return Err("the session isn't running in rush · resume it to send".into());
    }
    let live = r.state(s, sa, &s.subagent_jobs(), now).1;
    if live && inbox {
        return Ok(Route::Inbox);
    }
    if home != "claude" {
        return Err("its agent has no way to pass a message to a subagent".into());
    }
    if !sa.workflow.is_empty() {
        // A workflow runs its own agents: the session can't SendMessage
        // them.
        return Err(if live {
            "a workflow's agent takes messages only through an inbox, and this session's host has none".into()
        } else {
            "it finished with its workflow · nothing can reach it".into()
        });
    }
    Ok(Route::Relay { live })
}

/// Asks the main session to pass msg on to its finished subagent sa with
/// SendMessage, which resumes it.
fn resume_prompt(sa: &Subagent, msg: &str) -> String {
    format!(
        "Send this message to your subagent {} ({}) with SendMessage, word for word: it has finished, and SendMessage resumes it. Then carry on with what you were doing. Don't act on it yourself.\n\nThe message: {msg}",
        sa.id, sa.typ
    )
}

/// Sends text from the box under run id in the subagents view's column,
/// the way route says; now sends a spawned agent's or an inbox's at once.
pub fn send_to(m: &mut Model, key: &str, id: &str, text: String, now: bool) -> Out {
    let rt = match route(m, key, id) {
        Ok(rt) => rt,
        Err(e) => {
            super::subview::give_back(m, key, &text);
            m.fail(&e);
            return Out::Draw;
        }
    };
    if let Route::Spawned { host, kind } = &rt {
        return super::spawned::send_text(m, key, (id, host, kind), text, now, true);
    }
    let Some(sa) = conn(m, key)
        .runs
        .subs
        .iter()
        .find(|sa| sa.id == id)
        .cloned()
    else {
        return Out::Idle;
    };
    match rt {
        Route::Inbox if now => {
            let (steps, at) = (steps(m, key, id), m.now);
            let q = core(m).local_q.entry(subqueue::key(key, id)).or_default();
            if let Some(all) = subqueue::now_all(q, &text, steps, at) {
                tell(m, key, &sa, all, false);
            }
            m.flash(&format!(
                "sent to {} · it reads it after the step it's on",
                sa.typ
            ));
        }
        Route::Inbox => {
            let (steps, at) = (steps(m, key, id), m.now);
            let q = core(m).local_q.entry(subqueue::key(key, id)).or_default();
            subqueue::queue(q, steps, &text, at);
            m.flash(&subqueue::queued_words(&sa.typ));
        }
        Route::Relay { live } => {
            let busy = super::agent(m, key).is_some_and(super::busy);
            let (prompt, how) = if live {
                (
                    relay_prompt(&sa, &text),
                    "the main session passes it on at its next step",
                )
            } else {
                (
                    resume_prompt(&sa, &text),
                    "the main session resumes it with SendMessage",
                )
            };
            // Into the turn under way; else a turn of its own.
            let op = if busy {
                Op::Guide {
                    text: prompt,
                    images: Vec::new(),
                }
            } else {
                Op::Send {
                    text: prompt,
                    images: Vec::new(),
                    now: false,
                }
            };
            via_from(m, key, &sa, text, how, op, (false, true));
        }
        Route::Spawned { .. } => {}
    }
    Out::Draw
}

/// Sends each of session key's runs the next message it has waiting, once
/// it has taken a step since the last; a run that has finished hands what
/// it still had to the main session (flushSubQueues).
pub fn flush(m: &mut Model, key: &str) {
    if !conn(m, key).hosted() {
        return;
    }
    let now = m.now;
    let mut due: Vec<(Subagent, Due)> = Vec::new();
    {
        let Model { session, kept, .. } = &mut *m;
        let Some(s) = session.as_ref().filter(|v| v.key == key).map(|v| &v.sess) else {
            return;
        };
        let core = kept.get::<super::Core>();
        let Some(c) = core.conns.get(key) else {
            return;
        };
        let r = &c.runs;
        let jobs = s.subagent_jobs();
        for sa in &r.subs {
            let k = subqueue::key(key, &sa.id);
            let Some(q) = core.local_q.get_mut(&k).filter(|q| !q.items.is_empty()) else {
                continue;
            };
            let live = r.state(s, sa, &jobs, now).1;
            let steps = r.stats.get(&sa.id).map_or(0, |t| t.steps);
            let d = subqueue::due(q, &sa.typ, live, steps, now);
            if d != Due::Nothing {
                due.push((sa.clone(), d));
            }
        }
    }
    for (sa, d) in due {
        match d {
            Due::Tell(text) => tell(m, key, &sa, text, true),
            Due::Main { mine, text } => {
                core(m).local_q.remove(&subqueue::key(key, &sa.id));
                via(
                    m,
                    key,
                    &sa,
                    mine,
                    "it ended first · went to the main session",
                    Op::Send {
                        text,
                        images: Vec::new(),
                        now: false,
                    },
                    true,
                );
            }
            Due::Nothing => {}
        }
    }
}

/// The watched run's notes, the latest three, w wide (subNoteLines).
pub fn note_lines(m: &mut Model, key: &str, w: usize) -> Vec<Line> {
    let Some((sa, live)) = watched(m, key) else {
        return Vec::new();
    };
    let notes = core(m)
        .sub_notes
        .get(&subqueue::key(key, &sa.id))
        .cloned()
        .unwrap_or_default();
    if notes.is_empty() {
        return Vec::new();
    }
    let c = conn(m, key);
    let Some(d) = c.runs.detail.as_ref() else {
        return Vec::new();
    };
    subqueue::note_lines(&notes, &d.view.sess, live, w)
}
