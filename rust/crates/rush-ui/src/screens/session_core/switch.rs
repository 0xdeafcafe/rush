//! Switching a session rush hosts to another setup: its model, effort,
//! permission mode and account in place when it stays on the same agent
//! paid the same way; else its conversation handed over to a new session
//! that starts as the setup, the one it came from stopped and put away
//! in Earlier once the new one runs (a switch, not a copy). The start
//! sheet's switch and a usage limit's hand-on (accounts/handoff.rs) come
//! here through Accounts' switcher and hand_over.
//!
//! After Go's internal/ui/setup.go (inPlace :325, switchSessionMessage
//! :337-406, handOverMessage :420-503), choicesheet.go:78-105 (setArg's
//! words), handoff.go:19-42 (conversationLater), convo/handoff.go:15-151
//! (Session.Conversation, history, turnHistory, handoffClip),
//! handoff_ready.go:12 (handoffQuiet) and model.go:1056-1069
//! (hostStartedMsg's retire). The conversation is read, and the new
//! session started, off the UI thread: through the Hosts' hand_over,
//! which the binary answers with rush's host module.

use rush_convo::{Item, Session, Status};
use rush_core::adapter::{Conversation, Feature, Kind, Step};
use rush_core::agent::{Agent, PreviewLine, Todo};

use super::host::{HandOver, HandedOver, Op, hosts};
use super::{agent, conn, core};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::accounts::setup::{StartOver, start_name};
use crate::screens::accounts::state::accounts;
use crate::session::Source;

/// Lets the start sheet's switch and a usage limit's hand-on move hosted
/// sessions (set with hosts that can start them).
pub fn install(m: &mut Model) {
    let a = accounts(m);
    a.switcher = Some(switch_session_message);
    a.hand_over = Some(hand_over_key);
}

/// Whether a session running as from can switch to to without a new
/// session: the same agent, paid the same way, and for a local model the
/// same one (inPlace).
pub fn in_place(from: &StartOver, to: &StartOver, provider: &str) -> bool {
    from.kind == to.kind
        && from.billing == to.billing
        && (provider != "ollama" || from.model == to.model)
}

/// Switches session key to o, then sends message when there is one: its
/// model, effort, mode and account in place when it stays on the same
/// agent paid the same way; else its conversation is handed on to a new
/// session that starts as o (switchSessionMessage).
pub fn switch_session_message(m: &mut Model, key: &str, o: StartOver, message: &str) -> Out {
    let Some(a) = agent(m, key).cloned() else {
        return Out::Idle;
    };
    let from = crate::screens::bars::agent::session_start(m, key).unwrap_or_default();
    if !in_place(&from, &o, &m.reg.provider_of(&o.kind)) {
        if !m.reg.supports(&o.kind, Feature::HandoffIn) {
            let n = start_name(m, &o);
            m.flash_err(&format!("{n} can't take a conversation on from another"));
            return Out::Draw;
        }
        if a.rush && !quiet(m, key) {
            m.flash_err(
                "switch harness between turns · finish or stop this turn and send or clear queued messages first",
            );
            // What you meant to say stays in the box for after.
            let c = conn(m, key);
            if c.input.is_empty() && !message.is_empty() {
                c.input.set(message);
            }
            return Out::Draw;
        }
        return hand_over_message(m, &a, o, message);
    }
    if !conn(m, key).hosted() && conn(m, key).sleeping {
        let (key, message) = (key.to_string(), message.to_string());
        return wake_then(m, &a, move |m| switch_session_message(m, &key, o, &message));
    }
    if !conn(m, key).hosted() {
        m.flash_err("switching works in rush-mode sessions · /rush moves this one over");
        return Out::Draw;
    }
    let kind = a.kind.as_str().to_string();
    let mut ops = Vec::new();
    let mut said = Vec::new();
    if o.mode_set && o.mode != from.mode {
        let c = crate::screens::session_cards::info(m, key);
        if !crate::screens::session_cards::permissions::permission_known(
            m,
            &kind,
            c.as_ref(),
            &o.mode,
        ) {
            m.flash_err(&format!(
                "This harness does not offer permission mode {}",
                o.mode
            ));
            return Out::Draw;
        }
        ops.push(Op::Permission(o.mode.clone()));
        said.push("permission change requested".to_string());
    }
    if o.model != from.model {
        ops.push(Op::Model(o.model.clone()));
        let w = crate::screens::accounts::look::model_word(&m.reg, &o.kind, &o.model);
        said.push(if w.is_empty() {
            "its default model".into()
        } else {
            w
        });
    }
    if o.effort != from.effort {
        ops.push(Op::Effort(o.effort.clone()));
        let e = if o.effort.is_empty() {
            "its default"
        } else {
            &o.effort
        };
        said.push(format!("{e} effort from the next turn"));
    }
    let switches = !o.account.is_empty()
        && crate::screens::accounts::rows::account_of(m, &o.kind) != o.account;
    if switches {
        let h = m.reg.harness_label(&o.kind);
        said.push(format!(
            "{} for every {h} session, this one's too",
            o.account
        ));
    }
    if !message.is_empty() {
        // After the changes, so the instruction runs with them.
        ops.push(Op::Send {
            text: message.to_string(),
            images: Vec::new(),
            now: false,
        });
    }
    let n = start_name(m, &o);
    if said.is_empty() {
        m.flash(&format!("already {n}"));
    } else {
        m.flash(&format!("{n} · {}", said.join(" · ")));
    }
    let key = key.to_string();
    let out = super::start::switch_then(
        m,
        &o,
        Box::new(move |m| {
            if !ops.is_empty() {
                super::queue::run_all(m, &key, ops);
            }
            Out::Draw
        }),
    );
    drawn(out)
}

/// out, drawn unless it ends rush.
fn drawn(out: Out) -> Out {
    if out == Out::Quit { out } else { Out::Draw }
}

/// Whether session key is quiet enough to carry: idle or stopped, nothing
/// queued, running beside it or retrying, as its host last said
/// (handoffQuiet). One not heard from yet is.
fn quiet(m: &mut Model, key: &str) -> bool {
    let background = usize::from(super::running::has_background(m, key));
    let i = &conn(m, key).info;
    if i.state.is_empty() {
        return true;
    }
    crate::screens::session_cards::handoff::handoff_quiet(
        &i.state,
        i.queue.len(),
        "",
        background,
        i.retry.is_some(),
    )
}

/// Wakes a's sleeping host, then does then once it's connected
/// (wakeHostThen).
fn wake_then(
    m: &mut Model,
    a: &Agent,
    then: impl FnOnce(&mut Model) -> Out + Send + 'static,
) -> Out {
    let h = hosts(m);
    let (a, key) = (a.clone(), a.key.clone());
    conn(m, &key).waking = true;
    m.flash(&format!("waking {}…", a.display_name));
    m.later(
        move || h.wake(&a, "", &[], false),
        move |m, r| {
            let ok = r.is_ok();
            let out = super::live::woke(m, &key, r, String::new());
            if ok && conn(m, &key).hosted() {
                return then(m);
            }
            out
        },
    );
    Out::Draw
}

/// Accounts' hand_over: session key's conversation carried on as o
/// (handOverMessage for the agent of key).
fn hand_over_key(m: &mut Model, key: &str, o: StartOver, message: &str) -> Out {
    match agent(m, key).cloned() {
        Some(a) => hand_over_message(m, &a, o, message),
        None => Out::Idle,
    }
}

/// Carries a's conversation on in a new session that starts as o, in the
/// same folder, with message as your next instruction when there is one.
/// An agent that takes it whole (Port) has it as its own history; another
/// starts on a summary of it. A rush session a stops and is put away in
/// Earlier once the new one runs; one that isn't rush's own is left as
/// it is (handOverMessage).
pub fn hand_over_message(m: &mut Model, a: &Agent, o: StartOver, message: &str) -> Out {
    let whole = m.reg.supports(&o.kind, Feature::Port);
    let to = start_name(m, &o);
    let mut start = super::start::config_as(m, &o, &a.job.cwd);
    start.name = if whole {
        a.display_name.clone()
    } else {
        format!("{} · on {to}", a.display_name)
    };
    start.profile = o.profile.clone();
    let conv = conversation_later(m, a);
    m.flash(&format!("handing {} to {to}…", a.display_name));
    let h = HandOver {
        start,
        from: a.job.id.clone(),
        retire: a.rush,
        conv: Conversation::default(),
        message: message.to_string(),
    };
    let from_key = a.key.clone();
    // The account it starts on is switched to first (accountSwitch, then
    // the start in sequence).
    let out = super::start::switch_then(
        m,
        &o,
        Box::new(move |m| {
            let hs = hosts(m);
            m.later(
                move || {
                    let mut h = h;
                    h.conv = conv();
                    hs.hand_over(&h)
                },
                move |m, r| handed_over(m, &from_key, r),
            );
            Out::Draw
        }),
    );
    drawn(out)
}

/// A hand-over made, or why not (hostStartedMsg with retire): the one it
/// came from is put away in Earlier when it was stopped, and the new one
/// picked and given the keys once a reading has it.
fn handed_over(m: &mut Model, from: &str, r: Result<HandedOver, String>) -> Out {
    let h = match r {
        Ok(h) => h,
        Err(e) => {
            m.fail(&e);
            return Out::Draw;
        }
    };
    if crate::hosted::id(m).is_some() {
        // It's in Agents; the hosted view stays on its own session.
        m.flash(&format!(
            "started {} · it's in rush's Agents",
            h.started.name
        ));
        return Out::Draw;
    }
    if h.retired && agent(m, from).is_some_and(|a| !a.done) {
        crate::screens::cmdbar::stops::toggle_done(m, from);
    }
    let st = h.started;
    core(m).select_on_load = rush_state::dir::key(&st.acct, &format!("a:{}", st.id));
    m.flash(&format!("started {}", st.name));
    super::start::pick_started(m);
    Out::Draw
}

/// What reads a's conversation, told for a hand-off: the open one's as
/// it stands now, else read from its transcript or its agent's history
/// when it's run (conversationLater). Run it off the UI thread.
pub fn conversation_later(m: &Model, a: &Agent) -> Box<dyn FnOnce() -> Conversation + Send> {
    let (kind, name, cwd) = (a.kind.clone(), a.display_name.clone(), a.job.cwd.clone());
    let tell = move |s: &Session| {
        let mut c = conversation(s, &kind);
        if c.name.is_empty() {
            c.name = name;
        }
        if c.cwd.is_empty() {
            c.cwd = cwd;
        }
        c
    };
    if let Some(v) = m.session.as_ref().filter(|v| v.key == a.key) {
        let c = tell(&v.sess);
        return Box::new(move || c);
    }
    match Source::of(&m.reg, a) {
        Some(Source::Transcript { path, decode }) => Box::new(move || {
            let mut s = Session::new();
            let _ = rush_convo::Tail::new(&path, decode).read(&mut s);
            tell(&s)
        }),
        Some(Source::History { adapter, session }) => Box::new(move || {
            let mut s = Session::new();
            if let Some(r) = adapter.history_reader()
                && let Ok(evs) = rush_core::adapter::read_history(r, &session, 0)
            {
                for ev in &evs {
                    s.apply(ev, 0);
                }
            }
            tell(&s)
        }),
        _ => Box::new(move || tell(&Session::new())),
    }
}

/// How many of the latest calls a hand-off's summary names (recentSteps).
const RECENT_STEPS: usize = 64;

/// How much of a conversation a whole hand-off carries, the latest kept:
/// about 50k tokens (historyMax).
const HISTORY_MAX: usize = 200_000;

/// s told for a hand-off from agent from: how it started, its latest
/// calls, every turn as history, the files it changed, its last turns and
/// its todo list (Session.Conversation).
pub fn conversation(s: &Session, from: &Kind) -> Conversation {
    let mut c = Conversation {
        from: from.clone(),
        cwd: if s.cwd.is_empty() {
            s.info.cwd.clone()
        } else {
            s.cwd.clone()
        },
        ..Conversation::default()
    };
    let steps_of = |t: &rush_convo::Turn| {
        t.items
            .iter()
            .filter_map(|it| match it {
                Item::Step(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut count = 0;
    for t in &s.turns {
        if c.first.is_empty() && t.from.is_empty() {
            c.first = t.prompt.clone();
        }
        count += steps_of(t).len();
    }
    // A summary only names the latest calls: the older are counted.
    let keep = count.min(RECENT_STEPS);
    c.earlier_steps = count - keep;
    let mut steps = Vec::with_capacity(keep);
    'turns: for t in s.turns.iter().rev() {
        for id in steps_of(t).into_iter().rev() {
            if steps.len() == keep {
                break 'turns;
            }
            let st = s.step(id);
            steps.push(Step {
                call: st.call().clone(),
                failed: st.status == Status::Failed,
            });
        }
    }
    steps.reverse();
    c.steps = steps;
    c.history = history(s);
    c.changed = super::changes::changes(s)
        .into_iter()
        .map(|f| f.path)
        .collect();
    let from_turn = s.turns.len().saturating_sub(3);
    for t in &s.turns[from_turn..] {
        c.recent.extend(turn_history(s, t));
    }
    c.todos = s
        .tasks
        .iter()
        .map(|t| Todo {
            label: t.subject.clone(),
            done: t.status == "completed",
            started: t.status == "in_progress",
        })
        .collect();
    c
}

/// Every turn: its prompt, and its words and steps in order, each step a
/// → line. The oldest go first past HISTORY_MAX; it starts with one of
/// yours (history).
fn history(s: &Session) -> Vec<PreviewLine> {
    // Newest first, so a long session's old calls the cap would drop are
    // never put in words.
    let mut out = Vec::new();
    let mut n = 0;
    'turns: for t in s.turns.iter().rev() {
        for mut l in turn_history(s, t).into_iter().rev() {
            // Room for both a message and its answer, even when one is
            // larger than the whole hand-off.
            l.text = handoff_clip(&l.text, HISTORY_MAX / 2);
            if n + l.text.len() > HISTORY_MAX {
                break 'turns;
            }
            n += l.text.len();
            out.push(l);
        }
    }
    out.reverse();
    let first = out
        .iter()
        .position(|l| l.role == "user")
        .unwrap_or(out.len());
    out.drain(..first);
    out
}

/// One turn as history: your message, then what it said and did, each
/// interjection of yours between (turnHistory).
fn turn_history(s: &Session, t: &rush_convo::Turn) -> Vec<PreviewLine> {
    let mut out = Vec::new();
    let line = |role: &str, text: String, at| PreviewLine {
        role: role.into(),
        text,
        at,
        doing: String::new(),
    };
    if !t.prompt.is_empty() {
        out.push(line("user", t.prompt.clone(), t.start));
    }
    let mut said: Vec<String> = Vec::new();
    let flush = |out: &mut Vec<PreviewLine>, said: &mut Vec<String>| {
        if !said.is_empty() {
            out.push(line("assistant", said.join("\n\n"), t.end));
            said.clear();
        }
    };
    for it in &t.items {
        match it {
            Item::Exchange(x) => said.push(format!(
                "[Agent exchange: {} → {}]\n{}",
                rush_convo::peer_label(&x.exchange.sender),
                rush_convo::peer_label(&x.exchange.receiver),
                x.exchange.text
            )),
            Item::Interject(i) if !i.text.is_empty() => {
                flush(&mut out, &mut said);
                out.push(line("user", i.text.clone(), t.start));
            }
            Item::Text { text, .. } if !text.trim().is_empty() => {
                said.push(text.trim().to_string())
            }
            Item::Step(id) => {
                let st = s.step(*id);
                let mut d = rush_core::tool::doing(st.call());
                if !d.is_empty() {
                    if st.status == Status::Failed {
                        d += " (failed)";
                    }
                    said.push(format!("→ {d}"));
                }
            }
            _ => {}
        }
    }
    flush(&mut out, &mut said);
    out
}

/// s kept to max bytes by its start and end, the middle said to be left
/// out; never split within a character (handoffClip).
pub fn handoff_clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    const GAP: &str = "\n[… middle omitted for handoff …]\n";
    let mut head = (max - GAP.len()) / 2;
    let mut tail = s.len() - (max - GAP.len() - head);
    while head > 0 && !s.is_char_boundary(head) {
        head -= 1;
    }
    while tail < s.len() && !s.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{GAP}{}", &s[..head], &s[tail..])
}
