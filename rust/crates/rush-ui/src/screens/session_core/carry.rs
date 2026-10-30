//! A hosted session carried on somewhere else: rewound to an earlier
//! point or a fresh conversation, compacted by another model, forked, or
//! handed to another harness; what the cards' sheets work out is done
//! here on the session's host. After Go's internal/ui/rewindsheet.go
//! (rewindHost :438, hangUp :463, onRewound :498), compactsheet.go
//! (compactBy :244, pruneKeep :240), forksheet.go:350 (start's
//! host.Spawn), handoff.go (conversationLater :19, handoffTo :58) and
//! rushmode.go:2156 (the message put back once the host is reached).
//!
//! Each waits off the UI thread; the session's link is let go once its
//! host has switched, and dialled again.

use rush_convo::Session;

use super::host::{Info, hosts};
use super::{agent, conn, core, later, live};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::session_cards::compact::Summ;
use crate::screens::session_cards::fork::Fork;
use crate::screens::session_cards::rewind::{Branch, Rewound, left_of, restart_note};

/// A probability at or over which a decision model's step is kept: tev1
/// scores what the work still needs around 0.4–0.55, the unrelated under
/// 0.3 (pruneKeep).
pub const PRUNE_KEEP: f64 = 0.3;

/// Session key's conversation as shown, opened or beside the list.
fn sess_mut<'a>(m: &'a mut Model, key: &str) -> Option<&'a mut Session> {
    [m.session.as_mut(), m.pane.as_mut()]
        .into_iter()
        .flatten()
        .find(|v| v.key == key)
        .map(|v| &mut v.sess)
}

fn sess<'a>(m: &'a Model, key: &str) -> Option<&'a Session> {
    [m.session.as_ref(), m.pane.as_ref()]
        .into_iter()
        .flatten()
        .find(|v| v.key == key)
        .map(|v| &v.sess)
}

/// The rush id of session key's host; "" when the list hasn't it, which
/// its host's dial says is no session.
fn host_id(m: &Model, key: &str) -> String {
    agent(m, key).map(|a| a.job.id.clone()).unwrap_or_default()
}

/// Has session key's host carry on from rw's conversation (rewindHost),
/// then says so and lets the link go (onRewound).
pub fn rewind(m: &mut Model, key: &str, rw: Rewound) -> Out {
    let id = host_id(m, key);
    let h = hosts(m);
    let key = key.to_string();
    later(
        m,
        move || {
            h.rewind(&id, &rw.session_id, rw.resume, &rw.left)
                .map(|r| (r, rw))
        },
        move |m, r| match r {
            Err(e) => {
                m.fail(&e);
                Out::Draw
            }
            Ok((restarted, rw)) => rewound(
                m,
                &key,
                &format!("{}{}", rw.text, restart_note(restarted)),
                rw.draft,
            ),
        },
    );
    Out::Draw
}

/// A rewind or compaction done: it's said, the message is put back in the
/// box once the host is reached again, and the link is let go now rather
/// than waiting to notice the host did (onRewound).
pub fn rewound(m: &mut Model, key: &str, text: &str, draft: String) -> Out {
    m.flash(text);
    if !draft.is_empty() {
        core(m).rewound.insert(key.to_string(), draft);
    }
    if live::showing(m).as_deref() == Some(key) {
        live::drop_link(m, key);
        live::sync(m);
    }
    Out::Draw
}

/// Clears the dock's "compacting" once the compaction is done or failed.
fn compacted_done(m: &mut Model, key: &str) {
    if let Some(s) = sess_mut(m, key) {
        s.mark_compacting(0);
    }
}

/// Has by summarise session key, or prune it, and carries it on from
/// that in a fresh conversation, with draft in its box (compactBy past
/// its guard, which the cards' compact_by is). A turn since leaves it as
/// it was.
pub fn compact(m: &mut Model, key: &str, by: Summ, draft: String) -> Out {
    let id = host_id(m, key);
    let expected = conn(m, key).info.clone();
    let left = left_of(m, key);
    let label = format!("{} {}", by.label, by.model);
    let Some(s) = sess(m, key) else {
        compacted_done(m, key);
        return Out::Idle;
    };
    let key = key.to_string();
    if by.prune {
        let (goal, steps) = super::told::prune_steps(s);
        if steps.is_empty() {
            compacted_done(m, &key);
            m.flash_err("nothing to prune yet: only the latest turns have tool steps");
            return Out::Draw;
        }
        let model = by.model.clone();
        let h = hosts(m);
        later(
            m,
            move || h.keep(&model, &goal, &steps),
            move |m, r| {
                let scores = match r {
                    Ok(s) => s,
                    Err(e) => {
                        compacted_done(m, &key);
                        m.fail(&format!("{label} couldn't judge it: {e}"));
                        return Out::Draw;
                    }
                };
                let keep: Vec<bool> = scores.iter().map(|p| *p >= PRUNE_KEEP).collect();
                let kept = keep.iter().filter(|k| **k).count();
                if kept == keep.len() {
                    compacted_done(m, &key);
                    m.fail(&format!(
                        "{label} judged all {kept} steps still needed; conversation left unchanged"
                    ));
                    return Out::Draw;
                }
                let by = format!("{label} (kept {kept} of {} steps)", keep.len());
                // Made here, on the UI thread, where the session is; a
                // turn since fails the host's check and leaves it be.
                let Some(s) = sess(m, &key) else {
                    compacted_done(m, &key);
                    return Out::Idle;
                };
                let prompt = super::told::pruned_prompt(&by, &super::told::pruned(s, &keep));
                carry(m, &key, id, by, prompt, left, expected, draft);
                Out::Draw
            },
        );
        return Out::Draw;
    }
    let text = super::told::plain_text(s);
    let reg = m.reg.clone();
    let k = key.clone();
    later(
        m,
        move || -> Result<(String, String), String> {
            let sz = reg.get(&by.kind);
            let Some(sz) = sz.as_ref().and_then(|a| a.summarizer()) else {
                return Err(format!("{} can't summarise", by.label));
            };
            let summary = sz
                .summarize(&by.model, super::told::COMPACT_SYSTEM, &text)
                .map_err(|e| format!("{label} couldn't summarise it: {e}"))?;
            if text.split_whitespace().count() > 100 && summary.split_whitespace().count() < 12 {
                return Err(format!(
                    "{label} returned too little detail to safely compact; conversation left unchanged"
                ));
            }
            let prompt = super::told::compacted_prompt(&label, &summary);
            Ok((label, prompt))
        },
        move |m, r| match r {
            Err(e) => {
                compacted_done(m, &k);
                m.fail(&e);
                Out::Draw
            }
            Ok((by, prompt)) => {
                carry(m, &k, id, by, prompt, left, expected, draft);
                Out::Draw
            }
        },
    );
    Out::Draw
}

/// Starts session id's fresh conversation on prompt, the one it had left
/// as a path for /rewind, only while it's the conversation expected
/// (compactBy's carry).
#[allow(clippy::too_many_arguments)]
fn carry(
    m: &mut Model,
    key: &str,
    id: String,
    by: String,
    prompt: String,
    left: Branch,
    expected: Info,
    draft: String,
) {
    let h = hosts(m);
    let key = key.to_string();
    later(
        m,
        move || {
            let sid = crate::screens::session_cards::rewind::new_session_id();
            h.compacted(&id, &sid, &prompt, &left, &expected)
        },
        move |m, r| {
            compacted_done(m, &key);
            match r {
                Err(e) => {
                    m.fail(&e);
                    Out::Draw
                }
                Ok(()) => rewound(
                    m,
                    &key,
                    &format!("compacted by {by} · the conversation it had is kept (/rewind)"),
                    draft,
                ),
            }
        },
    );
}

/// Starts the fork f of session key on its host (forksheet.go start's
/// host.Spawn), picked once the list has it.
pub fn fork(m: &mut Model, key: &str, f: Fork) -> Out {
    let Some(a) = agent(m, key).cloned() else {
        return Out::Idle;
    };
    let h = hosts(m);
    later(m, move || h.fork(&a, &f), super::start::started);
    Out::Draw
}

/// Starts a new session on harness to, named name, opened with session
/// key's conversation so far; this one is left as it is (handoffTo past
/// its checks, which the cards' handoff_to makes).
pub fn handoff(m: &mut Model, key: &str, to: &str, name: &str) -> Out {
    let Some(a) = agent(m, key).cloned() else {
        return Out::Idle;
    };
    // Whether to can run here is the start's to say, off the UI thread:
    // it may look for the agent's program again.
    let tell = super::switch::conversation_later(m, &a);
    let (reg, h) = (m.reg.clone(), hosts(m));
    let (to, name, cwd) = (to.to_string(), name.to_string(), a.job.cwd.clone());
    later(
        m,
        move || {
            let prompt = reg.handoff(&tell()).text;
            h.handoff(&to, &cwd, &name, &prompt)
        },
        super::start::started,
    );
    Out::Draw
}
