//! What's typed in the box under the run picked in the subagents view: to
//! a live Claude Code subagent in a session rush runs, into its own queue
//! and its inbox a step at a time (or all of it now), or, where its host
//! has no inbox, passed on by the main session; to a finished one, the
//! main session resumes it. Each message waiting for a run goes into its inbox
//! once it has taken a step since the last; one that finished first has
//! what it still had put back in its box, said under it, for you to send
//! on (Rust-only: Go handed it to the main session as a turn of its own,
//! and put one that didn't go in the session's box). After Go's
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
use crate::spawns::SPAWN_PREFIX;

/// What's said of messages queued for a run that ended before it took
/// them.
const ENDED_FIRST: &str =
    "it ended first · not sent: back in its box · enter sends it on, its main session resuming it";

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
            super::subbox::give_back(m, key, &sa.id, &text);
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
}

/// Notes where a message went, or keeps one that didn't: back at the head
/// of its queue, else in the run's own box (onSubSent; Go's put one back
/// in the session's box).
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
            if s.told {
                let q = core(m)
                    .local_q
                    .entry(subqueue::key(s.key, s.sub))
                    .or_default();
                q.items.insert(0, s.text);
                say(m, &format!("{why} · back in its queue"));
            } else {
                // It was the run's: back in the run's own box, never the
                // session's, where enter would make it a turn of the main
                // session's (Rust-only).
                super::subbox::give_back(m, s.key, s.sub, &s.text);
                say(m, &format!("{why} · back in its box"));
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
            super::subbox::give_back(m, key, id, &text);
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

/// Puts one message from run id's queue in its inbox now, ahead of the
/// rest (the queue's s).
pub fn tell_now(m: &mut Model, key: &str, id: &str, text: String) {
    let Some(sa) = conn(m, key)
        .runs
        .subs
        .iter()
        .find(|sa| sa.id == id)
        .cloned()
    else {
        return;
    };
    let (steps, at) = (steps(m, key, id), m.now);
    let q = core(m).local_q.entry(subqueue::key(key, id)).or_default();
    q.sent_at = at;
    q.at = steps;
    tell(m, key, &sa, text, false);
}

/// Sends each of session key's runs the next message it has waiting, once
/// it has taken a step since the last; a run that has finished has what
/// it still had put back in its box, said (flushSubQueues).
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
            // It ended first: what it still had goes back in its box, not
            // to the main session on its own; the note under it says so,
            // and enter there sends it on the way its box says (the main
            // session resuming it, where it can). Rust-only: Go sent it to
            // the main session as a turn of its own.
            Due::Main { mine, .. } => {
                core(m).local_q.remove(&subqueue::key(key, &sa.id));
                super::subbox::give_back(m, key, &sa.id, &mine);
                note(
                    m,
                    key,
                    &sa.id,
                    SubNote {
                        text: mine,
                        how: ENDED_FIRST.into(),
                        told: false,
                    },
                );
                m.news(
                    &format!(
                        "{} ended before it got your messages · back in its box",
                        sa.typ
                    ),
                    "",
                );
            }
            Due::Nothing => {}
        }
    }
}

/// What became of the messages sent run id from its box, the latest
/// three, w wide, under its conversation (subNoteLines).
pub fn note_lines(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let notes = core(m)
        .sub_notes
        .get(&subqueue::key(key, id))
        .cloned()
        .unwrap_or_default();
    if notes.is_empty() {
        return Vec::new();
    }
    let now = m.now;
    let Model { session, kept, .. } = m;
    let Some(s) = session.as_ref().filter(|v| v.key == key).map(|v| &v.sess) else {
        return Vec::new();
    };
    let Some(c) = kept.get::<super::Core>().conns.get(key) else {
        return Vec::new();
    };
    let r = &c.runs;
    let jobs = s.subagent_jobs();
    let live = r
        .subs
        .iter()
        .find(|sa| sa.id == id)
        .is_some_and(|sa| r.state(s, sa, &jobs, now).1);
    let Some(d) = r.detail.as_ref().filter(|d| d.id == id) else {
        return Vec::new();
    };
    subqueue::note_lines(&notes, &d.view.sess, live, w)
}
