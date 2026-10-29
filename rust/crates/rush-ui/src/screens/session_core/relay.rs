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
//! Not here: stopping one run alone (stopSub, ctrl+x: the host's
//! stop_task isn't among the Session's ops yet) and passing a message on
//! to a Codex thread (childSub).

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
    let text = conn(m, key).input.string().trim().to_string();
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
    let Some(link) = conn(m, key).link.clone() else {
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
            if s.told {
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
