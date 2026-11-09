//! The host's side of Emeritus (docs/emeritus.md): the nudge past the
//! threshold, the retire op, and the handover as a transaction. The host
//! owns the trigger: it measures the context its agent's requests send,
//! and nudges once, at a turn's end, while the prompt cache is warm. A
//! session that asks to retire does so at its turn's end, never mid-turn:
//! it's frozen (what you type is refused, other agents' messages wait),
//! its baton gets rush's facts, a successor starts, and only once the
//! successor's first request has come back is it retired for good, what
//! was its moved to the successor. A successor that can't start is
//! retired instead and the session carries on, told why. Each step is
//! journalled, so a host cut short settles it at its next start
//! (crate::emeritus::recover). Rust-only.

use rush_core::event;
use rush_state::Time;
use serde_json::json;

use super::agent::answers;
use super::{Host, State};
use crate::emeritus::{
    self as em, Forwarded, Journal, Phase, START_WAIT, Start, baton_file, baton_path, facts,
    from_agents, nudge, readdressed, refusal, rolled_back_note, successor_config, successor_prompt,
    user_queued, waits_for_tasks, write_journal,
};
use crate::op::Op;
use crate::wire::{Emeritus, queue_exchanges};

/// What a host keeps of its session's handover while it runs.
#[derive(Default)]
pub(crate) struct Em {
    /// The baton the agent asked to retire with: handed over at its
    /// turn's end.
    pub(super) due: Option<String>,
    /// A handover under way: what you type is refused, the queue held.
    pub(super) handing: bool,
    /// The turn under way is a nudge's: it answers no one.
    pub(super) nudged: bool,
    /// Its agent started while Emeritus was off, so the tools it was
    /// given lack `retire`: it isn't nudged until it starts again (a
    /// wake, a resume, a restart), when it's listed. Its tools aren't
    /// changed while it runs: that would write its whole prompt cache
    /// again, what Emeritus is there to save.
    pub(super) unlisted: bool,
    /// How many handovers this host has settled, either way: what a test
    /// waits on.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) settled: u64,
}

/// What a session's info says of its Emeritus once nothing is under way.
fn at_rest(cfg: &crate::wire::Config) -> String {
    cfg.emeritus.as_ref().map_or("", Emeritus::word).into()
}

/// What a session handing over says to what you send it.
const HANDING: &str = "this session is handing over to a successor: nothing sent now is passed on. Write to the successor once it has started, or here again if the handover is rolled back.";

impl Host {
    /// Emeritus's ops (retire, emeritus_adopt, emeritus_void), and what a
    /// retired session or one handing over refuses; None for an op that
    /// goes on as usual.
    pub(super) fn emeritus_op(&self, o: &Op) -> Option<Result<(), String>> {
        let mut st = self.lock();
        match o.op.as_str() {
            "retire" => return Some(self.ask_retire(&mut st, &o.text)),
            "emeritus_adopt" => {
                // A session it started now answers to the one that took
                // over from its parent.
                if !o.id.is_empty()
                    && matches!(o.text.as_str(), "spawnedBy" | "startedFrom")
                    && st.cfg.meta.get(&o.text).is_some_and(|p| *p == o.was)
                {
                    st.cfg.meta.insert(o.text.clone(), o.id.clone());
                    st.info.meta.insert(o.text.clone(), o.id.clone());
                    self.save_config(&st);
                    let f = json!({"key": o.text, "from": o.was, "to": o.id});
                    self.event(&st, "emeritus-adopted", f);
                    self.publish(&mut st);
                }
                return Some(Ok(()));
            }
            "emeritus_void" => {
                // A successor whose start was rolled back: retired, for good.
                let e = st.cfg.emeritus.get_or_insert_with(Emeritus::default);
                e.retired_at = Time::now();
                e.why = o.text.clone();
                st.info.emeritus = "retired".into();
                self.save_config(&st);
                drop(st);
                return Some(self.do_op_inner(&Op::named("stop")));
            }
            _ => {}
        }
        if let Some(why) = refusal(&st.cfg) {
            // Retired: only stopping, and what changes nothing it does.
            if !matches!(o.op.as_str(), "stop" | "context" | "decisions_dismiss") {
                return Some(Err(why));
            }
            return None;
        }
        if st.em.handing {
            let refused = match o.op.as_str() {
                // Yours is refused; another agent's waits in the queue.
                "send" => o.exchange.is_none(),
                "queue_send" | "rewind" | "compacted" | "keep_warm" | "ask" | "relogin"
                | "model" | "mode" | "effort" | "upgrade" => true,
                _ => false,
            };
            if refused {
                return Some(Err(HANDING.into()));
            }
        }
        None
    }

    /// The retire op: the agent asks to retire after its turn, handing
    /// over on baton.
    fn ask_retire(&self, st: &mut State, baton: &str) -> Result<(), String> {
        if !rush_state::Store::load().config.emeritus.on {
            return Err("Emeritus is off: it's turned on in rush's settings (emeritus.on).".into());
        }
        if st.em.handing {
            return Err("a handover is already under way".into());
        }
        let lacks = em::baton_lacks(baton);
        if !lacks.is_empty() {
            return Err(format!("the baton lacks {}", lacks.join(", ")));
        }
        st.em.due = Some(baton.trim().to_string());
        st.info.emeritus = "retiring".into();
        let f = json!({"context": st.info.context_tokens, "bytes": baton.len(), "state": st.info.state});
        self.event(st, "emeritus-retire-asked", f);
        self.publish(st);
        if st.info.state == "idle" && st.pending.is_empty() {
            // Asked between turns: there's no turn to wait out.
            let end = event::TurnEnd {
                reason: "done".into(),
                ..Default::default()
            };
            if self.emeritus_turn_end(st, &end) {
                self.publish(st);
            }
        }
        Ok(())
    }

    /// At a turn's end with nothing asked of the user: hands over if the
    /// agent asked to retire, else nudges it to if it's due. Whether it
    /// did either, settling the turn's end.
    pub(super) fn emeritus_turn_end(&self, st: &mut State, e: &event::TurnEnd) -> bool {
        if let Some(baton) = st.em.due.take() {
            st.info.emeritus = at_rest(&st.cfg);
            if e.reason == "interrupted" {
                // Stopped by you: you have something to say to it.
                self.event(st, "emeritus-called-off", json!({"why": "interrupted"}));
                return false;
            }
            if user_queued(&st.info) {
                // What you wrote is for it: it never goes to another.
                self.event(st, "emeritus-called-off", json!({"why": "user wrote"}));
                self.cut_in(st, em::CALLED_OFF, &[], None, 0, None, false);
                return false;
            }
            let tasks = st.info.background.len().max(st.info.tasks.max(0) as usize);
            if tasks > 0 {
                self.event(
                    st,
                    "emeritus-called-off",
                    json!({"why": "background tasks", "tasks": tasks}),
                );
                if self.send_locked(st, &waits_for_tasks(tasks)).is_ok() {
                    st.em.nudged = true; // the turn that asked to retire answers no one
                }
                return true;
            }
            self.begin_handover(st, baton);
            return true;
        }
        if answers(st)
            || e.reason != "done"
            || !st.info.queue.is_empty()
            || st.info.decisions.is_some()
            || !st.info.needs.is_empty()
        {
            // An agent another started is nudged when new work comes to it
            // (nudge_first), not once its task is done; one that waits on
            // you isn't nudged.
            return false;
        }
        let Some(past) = self.nudge_due(st) else {
            self.note_capped(st);
            return false;
        };
        self.nudge(st, past, false);
        true
    }

    /// Past its threshold with the cap on handovers in a row reached: it
    /// isn't nudged, and that's said once, in its info (the session's
    /// header shows it) and its events.
    pub(super) fn note_capped(&self, st: &mut State) {
        let set = rush_state::Store::load().config.emeritus;
        let lineage = em::lineage(&st.cfg);
        if !set.on
            || !set.capped(lineage)
            || st.cfg.retired()
            || st
                .cfg
                .emeritus
                .as_ref()
                .is_some_and(|e| !e.capped_at.is_zero())
        {
            return;
        }
        let model = if st.info.model.is_empty() {
            &st.cfg.model
        } else {
            &st.info.model
        };
        let window = self.reg.context_window(&st.cfg.kind, model);
        if !set.due(st.info.context_tokens, window) {
            return;
        }
        let e = st.cfg.emeritus.get_or_insert_with(Emeritus::default);
        e.capped_at = Time::now();
        self.save_config(st);
        st.info.emeritus = "capped".into();
        let f =
            json!({"context": st.info.context_tokens, "lineage": lineage, "depth": set.depth()});
        self.event(st, "emeritus-capped", f);
        self.publish(st);
    }

    /// The threshold the agent's context is past, when it's due a nudge:
    /// Emeritus on, past its threshold (and, nudged before, grown a
    /// quarter of it since), its prompt cache still warm, and nothing of
    /// its own running or waiting.
    pub(super) fn nudge_due(&self, st: &State) -> Option<i64> {
        let set = rush_state::Store::load().config.emeritus;
        if !set.on
            || st.em.unlisted
            || st.cfg.retired()
            || st.em.handing
            || st.em.due.is_some()
            || st.em.nudged
            || set.capped(em::lineage(&st.cfg))
        {
            return None;
        }
        let model = if st.info.model.is_empty() {
            &st.cfg.model
        } else {
            &st.info.model
        };
        let window = self.reg.context_window(&st.cfg.kind, model);
        let ctx = st.info.context_tokens;
        if !set.due(ctx, window) {
            return None;
        }
        let past = set.threshold(window);
        if let Some(e) = &st.cfg.emeritus
            && !e.nudged_at.is_zero()
            && ctx < e.nudged_context + past / em::RENUDGE_SHARE
        {
            return None;
        }
        // A cold cache would be written whole for the baton's turn: dearer
        // than compacting (docs/emeritus.md, "Lab tests").
        if !self.warm_at(st, Time::now()) {
            return None;
        }
        let busy = !st.pending.is_empty()
            || !st.info.background.is_empty()
            || st.info.tasks > 0
            || st.info.limit.is_some()
            || st.info.retry.is_some();
        (!busy).then_some(past)
    }

    /// Whether a message from another agent arriving while it's idle
    /// should wait behind a nudge: the threshold it's past, when so.
    pub(super) fn nudge_first(&self, st: &State) -> Option<i64> {
        if st.info.state != "idle" || !st.info.queue.is_empty() {
            return None;
        }
        self.nudge_due(st)
    }

    /// Sends the agent the nudge to hand over, past the threshold; first
    /// says new work waits behind it, so the nudge's turn answers no one.
    pub(super) fn nudge(&self, st: &mut State, past: i64, first: bool) {
        let ctx = st.info.context_tokens;
        let e = st.cfg.emeritus.get_or_insert_with(Emeritus::default);
        e.nudged_at = Time::now();
        e.nudged_context = ctx;
        e.nudged_past = past;
        self.save_config(st);
        let f = json!({"context": ctx, "threshold": past, "first": first, "cacheWarm": st.info.cache_warm});
        self.event(st, "emeritus-nudged", f);
        match self.send_locked(st, &nudge(ctx, past)) {
            Ok(()) => st.em.nudged = first,
            Err(e) => self.event(st, "emeritus-nudge-failed", json!({"err": e})),
        }
        self.publish(st);
    }

    /// Freezes the session and starts its handover on a thread of its
    /// own: the first step of the transaction.
    fn begin_handover(&self, st: &mut State, baton: String) {
        let threshold = st
            .cfg
            .emeritus
            .as_ref()
            .filter(|e| !e.nudged_at.is_zero())
            .map(|e| e.nudged_past)
            .unwrap_or(0);
        let j = Journal {
            phase: Phase::Frozen,
            from: st.cfg.id.clone(),
            at: Time::now(),
            from_context: st.info.context_tokens,
            from_cost_usd: st.info.cost_usd,
            threshold,
            ..Journal::default()
        };
        if let Err(e) = write_journal(&st.cfg.id, &j) {
            self.event(st, "emeritus-unjournalled", json!({"err": e.to_string()}));
            let _ = self.send_locked(
                st,
                &rolled_back_note(&format!("couldn't be journalled: {e}")),
            );
            return;
        }
        st.em.handing = true;
        st.info.emeritus = "handing".into();
        st.info.detail = "handing over to a successor".into();
        let f = json!({"context": j.from_context, "cost": j.from_cost_usd, "threshold": threshold});
        self.event(st, "emeritus-handover", f);
        let (cfg, info) = (st.cfg.clone(), st.info.clone());
        let h = self.clone();
        self.go("handover", move |_| h.hand_over(cfg, info, baton, j));
    }

    /// The handover's steps after the freeze, off the lock: the baton
    /// with its facts, the successor started, then commit or roll back.
    fn hand_over(
        &self,
        a: crate::wire::Config,
        info: crate::wire::Info,
        baton: String,
        mut j: Journal,
    ) {
        let cwd = if info.cwd.is_empty() {
            a.cwd.clone()
        } else {
            info.cwd.clone()
        };
        let sid = if info.session_id.is_empty() {
            &a.session_id
        } else {
            &info.session_id
        };
        let transcript = self.reg.find_transcript(&a.kind, &a.account, &cwd, sid);
        let full = baton_file(
            &baton,
            &facts(&cwd, &transcript, info.cost_usd, info.context_tokens),
        );
        if let Err(e) = crate::json::write_private(&baton_path(&a.id), full.as_bytes()) {
            return self.roll_back(j, format!("couldn't be given its baton: {e}"));
        }
        let fwd = from_agents(&self.lock().info);
        let mut b = successor_config(&a, &info, successor_prompt(&a, &full, &transcript, &fwd));
        b.fill_ids();
        j.to = b.id.clone();
        j.phase = Phase::Starting;
        if let Err(e) = write_journal(&a.id, &j) {
            j.to.clear(); // never started: nothing to undo
            return self.roll_back(j, format!("couldn't be journalled: {e}"));
        }
        b.cause = rush_events::id();
        let _ = std::fs::create_dir_all(crate::paths::dir(&b.id));
        crate::events::log_event(
            &b.id,
            "spawned",
            json!({"ev": b.cause, "by": "emeritus", "predecessor": a.id, "agent": b.kind, "model": b.model, "lineage": em::lineage(&b)}),
        );
        let started = match &self.hooks.successor {
            Some(f) => f(b),
            None => match crate::client::spawn(b) {
                Ok(c) => em::wait_started(&c.id, START_WAIT),
                Err(e) => Start::Failed(format!("couldn't start: {e}")),
            },
        };
        match started {
            Start::Started(ctx) => self.commit(j, ctx, &fwd),
            Start::Failed(why) => self.roll_back(j, why),
            Start::Waiting => self.roll_back(j, "didn't answer".into()),
        }
    }

    /// The commit: the session's config says it's retired, handed over
    /// to its successor (the point past which it's done), then what was
    /// its moves on, what waited for it goes to the successor, the
    /// handover is logged for the Efficiency place, and it stops.
    fn commit(&self, mut j: Journal, to_context: i64, fwd: &[Forwarded]) {
        let (cfg, later) = {
            let mut st = self.lock();
            let mut cfg = st.cfg.clone();
            let e = cfg.emeritus.get_or_insert_with(Emeritus::default);
            e.retired_at = Time::now();
            e.successor = j.to.clone();
            if let Err(err) = em::write_config(&cfg) {
                drop(st);
                return self.roll_back(
                    j,
                    format!(
                        "couldn't take over: its predecessor's config couldn't be written: {err}"
                    ),
                );
            }
            st.cfg = cfg.clone();
            st.em.handing = false;
            st.info.emeritus = "retired".into();
            st.info.successor = j.to.clone();
            st.info.detail = format!("handed over to {}", j.to);
            // Other agents' messages that came after the successor's prompt
            // was written go on to it; those in its prompt are its already.
            let qe = queue_exchanges(&st.info);
            let later: Vec<_> = qe
                .into_iter()
                .flatten()
                .filter(|x| !fwd.iter().any(|f| f.id == x.id))
                .map(|x| readdressed(*x, &j.to))
                .collect();
            st.info.queue.clear();
            st.info.queue_images.clear();
            st.info.queue_exchanges.clear();
            st.go_now = 0;
            self.publish(&mut st);
            (cfg, later)
        };
        j.phase = Phase::Committed;
        let _ = write_journal(&j.from, &j);
        em::finish(&cfg, &j, to_context);
        for x in later {
            if let Err(e) = crate::client::deliver(&j.to, &x, false) {
                crate::events::log_event(
                    &j.from,
                    "emeritus-unforwarded",
                    json!({"to": j.to, "err": e.to_string()}),
                );
            }
        }
        let _ = self.do_op_inner(&Op::named("stop"));
        let mut st = self.lock();
        st.em.settled += 1;
        drop(st);
        self.cv.notify_all();
    }

    /// The roll back: the successor, if one was started, is retired for
    /// good, and the session carries on, told why.
    fn roll_back(&self, mut j: Journal, why: String) {
        if !j.to.is_empty() && crate::paths::dir(&j.to).join("config.json").exists() {
            em::void(
                &j.to,
                &format!(
                    "its start as {}'s successor was rolled back: it {why}",
                    j.from
                ),
            );
        }
        j.phase = Phase::RolledBack;
        j.why = why.clone();
        let _ = write_journal(&j.from, &j);
        let mut st = self.lock();
        st.em.handing = false;
        st.info.emeritus = at_rest(&st.cfg);
        st.info.detail.clear();
        self.event(&st, "emeritus-rolled-back", json!({"to": j.to, "why": why}));
        if !st.quit {
            match self.send_locked(&mut st, &rolled_back_note(&why)) {
                // The note's turn answers no one: what it was asked is
                // answered by the turn after.
                Ok(()) => st.em.nudged = true,
                Err(e) => self.event(&st, "emeritus-untold", json!({"err": e})),
            }
        }
        st.em.settled += 1;
        self.publish(&mut st);
        drop(st);
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests;
