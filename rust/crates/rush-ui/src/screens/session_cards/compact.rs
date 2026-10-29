//! #compact: compact a session with its own model, as /compact does, or
//! have another write the summary (a cheaper Claude, a local model) and
//! carry on in a fresh conversation that starts with it. The cache goes
//! either way; the conversation left is a path for /rewind. And a typed
//! /compact, which on a cold cache has the cheap model write it. After
//! Go's internal/ui/compactsheet.go (summarizer :23, compactSheet :33,
//! openCompact :43, body :116, key :170, isDefault :226, cheapCompact
//! :327, compactTyped :350) and convo/overview.go (CacheCold :91).
//! Summarising and carrying on (compactBy, with the session's text, and
//! the host's CompactedIfUnchanged) are the session's: [`Act::Compact`].

use rush_convo::Session;
use rush_core::adapter::Feature;
use rush_core::text::Line;
use rush_core::time::Ms;

use super::{Act, act, info, save_setting, setting};
use crate::frame::sheet::{sheet_row, sheet_title, window};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// Who writes a summary: kind's model, "" for the session's own
/// /compact; prune has a decision model drop the steps it judges done
/// and keep the rest word for word, in place of a summary (summarizer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summ {
    pub kind: String,
    /// "Ollama", "Claude".
    pub label: String,
    pub model: String,
    pub prune: bool,
}

/// CompactKind for a default that prunes with PruneModel (prunerKind).
pub const PRUNER_KIND: &str = "ollama-prune";

/// The decision model's pruner (pruner).
pub fn pruner(model: &str) -> Summ {
    Summ {
        kind: "ollama".into(),
        label: "Ollama prune".into(),
        model: model.into(),
        prune: true,
    }
}

/// The #compact sheet (compactSheet).
#[derive(Default)]
pub struct CompactSheet {
    pub sess: String,
    pub name: String,
    pub opts: Vec<Summ>,
    /// Summarisers that couldn't say their models.
    pub errs: Vec<String>,
    pub loading: bool,
    pub cur: usize,
    pub details: usize,
}

/// What the model list found, when it landed with the sheet out.
#[derive(Default)]
struct Found(Option<(String, Vec<Summ>, Vec<String>)>);

/// Opens #compact on agent key's session (openCompact).
pub fn open_compact(m: &mut Model, key: &str) -> Out {
    let Some(inf) = info(m, key) else {
        m.flash_err("#compact works on a session rush runs: open one first");
        return Out::Draw;
    };
    let kind = inf.kind.clone();
    let mut s = CompactSheet {
        sess: key.into(),
        name: inf.name.clone(),
        loading: true,
        ..CompactSheet::default()
    };
    if m.reg.supports(&kind, Feature::Compact) {
        s.opts.push(Summ {
            label: "native harness compaction".into(),
            ..Summ::default()
        });
    }
    if !m.reg.supports(&kind, Feature::Rewind) {
        s.loading = false;
        s.errs.push(
            "External summaries are unavailable: this harness cannot restore the original conversation."
                .into(),
        );
        m.open_sheet(Box::new(s));
        return Out::Draw;
    }
    if inf.proto < 9 {
        s.loading = false;
        s.errs.push(
            "Restart this session’s host to enable guarded local compaction; native compaction remains available."
                .into(),
        );
        m.open_sheet(Box::new(s));
        return Out::Draw;
    }
    m.open_sheet(Box::new(s));
    let all = m.reg.installed_all();
    let k = key.to_string();
    m.later(
        move || {
            let mut opts = Vec::new();
            let mut errs = Vec::new();
            for ad in all {
                let Some(sz) = ad.summarizer() else { continue };
                match sz.summary_models() {
                    Ok(models) => {
                        for md in models {
                            opts.push(Summ {
                                kind: ad.kind().as_str().to_string(),
                                label: ad.name(),
                                model: md,
                                prune: false,
                            });
                        }
                    }
                    Err(e) => errs.push(format!("{}: {e}", ad.name())),
                }
            }
            (opts, errs)
        },
        move |m, (opts, errs)| {
            let defaults = Defaults::of(m);
            match m.sheet_as::<CompactSheet>().filter(|s| s.sess == k) {
                Some(s) => s.found(opts, errs, &defaults),
                None => m.kept::<Found>().0 = Some((k, opts, errs)),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// The saved #compact default (compactKind, compactModel, pruneModel).
#[derive(Clone, Debug, Default)]
pub struct Defaults {
    pub kind: String,
    pub model: String,
    pub prune_model: String,
}

impl Defaults {
    pub fn of(m: &Model) -> Defaults {
        Defaults {
            kind: setting(m, "compactKind"),
            model: setting(m, "compactModel"),
            prune_model: setting(m, "pruneModel"),
        }
    }

    /// Whether o is the saved default (isDefault).
    pub fn is(&self, o: &Summ) -> bool {
        if o.prune {
            return self.kind == PRUNER_KIND && o.model == self.prune_model;
        }
        o.kind == self.kind && o.model == self.model
    }
}

impl CompactSheet {
    fn found(&mut self, opts: Vec<Summ>, errs: Vec<String>, d: &Defaults) {
        self.opts.extend(opts);
        self.errs = errs;
        self.loading = false;
        let mut found = false;
        for (i, o) in self.opts.iter().enumerate() {
            if d.is(o) {
                self.cur = i;
                found = true;
            }
        }
        if !found && !d.model.is_empty() {
            self.errs
                .push("Saved compaction model is unavailable; choose explicitly.".into());
        }
    }

    fn take_found(&mut self, m: &mut Model) {
        if let Some((k, opts, errs)) = m.kept::<Found>().0.take() {
            if k == self.sess {
                let d = Defaults::of(m);
                self.found(opts, errs, &d);
            } else {
                m.kept::<Found>().0 = Some((k, opts, errs));
            }
        }
    }
}

impl Sheet for CompactSheet {
    fn width(&self, _m: &Model) -> usize {
        76
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        if let Some(Some((k, opts, errs))) = m.kept.peek::<Found>().map(|f| f.0.clone())
            && k == self.sess
            && self.loading
        {
            self.found(opts, errs, &Defaults::of(m));
        }
        let d = Defaults::of(m);
        let mut out = vec![
            sheet_title("Compact", "choose who writes the summary", w),
            Line::new(),
        ];
        let (from, to) = window(
            self.opts.len(),
            self.cur,
            1.max((h as isize - 9) / 2) as usize,
        );
        for i in from..to {
            let o = &self.opts[i];
            let mut l = if o.model.is_empty() {
                line([sp(&o.label, TEXT)])
            } else {
                line([sp(fit_str(&o.label, 18), TEXT), sp(&o.model, GREEN)])
            };
            if o.prune {
                l.spans
                    .push(sp(" · experimental: keeps steps word for word", DIM));
            }
            if d.is(o) {
                l.spans.push(sp(" · default", DIM));
            }
            out.push(sheet_row(l, i == self.cur, w));
        }
        if self.loading {
            out.push(line([sp("  finding the models installed…", DIM)]));
        }
        let mut details = Vec::new();
        for e in &self.errs {
            for (j, l) in wrap(e, 8.max(w.saturating_sub(4))).into_iter().enumerate() {
                if j == 0 {
                    details.push(line([plain("  "), sp("! ", YELLOW), sp(l, DIM)]));
                } else {
                    details.push(line([plain("    "), sp(l, DIM)]));
                }
            }
        }
        let mut footer = vec![Line::new()];
        for note in [
            "External summaries start fresh; /rewind keeps the original.",
            "Native automatic compaction is unchanged.",
        ] {
            for l in wrap(note, 8.max(w.saturating_sub(2))) {
                footer.push(line([sp(format!("  {l}"), DIM)]));
            }
        }
        footer.push(Line::new());
        footer.push(keys_fit(
            w,
            &[
                ("↑↓", "choose"),
                ("enter", "compact"),
                ("d", "make default"),
                ("esc", "cancel"),
            ],
        ));
        let room = h.saturating_sub(out.len() + footer.len());
        if details.len() > room {
            let room = room.saturating_sub(1);
            self.details = self.details.min(details.len().saturating_sub(room));
            let end = details.len().min(self.details + room);
            out.extend(details[self.details..end].iter().cloned());
            out.push(line([sp(
                format!(
                    "  details {}–{}/{} · pgup/pgdn",
                    details.len().min(self.details + 1),
                    end,
                    details.len()
                ),
                DIM,
            )]));
        } else {
            self.details = 0;
            out.extend(details);
        }
        out.extend(footer);
        out.truncate(h);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        self.take_found(m);
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "pgup" => self.details = self.details.saturating_sub(6),
            "pgdown" => self.details += 6,
            "up" | "k" | "shift+tab" => self.cur = self.cur.saturating_sub(1),
            "down" | "j" | "tab" => {
                self.cur = (self.cur + 1).min(self.opts.len().saturating_sub(1))
            }
            "d" => {
                let Some(o) = self.opts.get(self.cur).cloned() else {
                    return Out::Idle;
                };
                let ok = if o.prune {
                    save_setting(m, "compactKind", PRUNER_KIND)
                        && save_setting(m, "pruneModel", &o.model)
                } else {
                    save_setting(m, "compactKind", &o.kind)
                        && save_setting(m, "compactModel", &o.model)
                };
                if ok {
                    m.flash("Compaction default saved; native automatic compaction is unchanged");
                }
            }
            "enter" => {
                let Some(o) = self.opts.get(self.cur).cloned() else {
                    return Out::Idle;
                };
                let Some(inf) = info(m, &self.sess) else {
                    return Out::Close;
                };
                if o.model.is_empty() {
                    m.flash(&format!("compacting {}…", self.name));
                    act(m, &self.sess, Act::Send("/compact".into()));
                    return Out::Close;
                }
                if busy(&inf.state) {
                    m.flash_err("it's working: let the turn end, then #compact");
                    return Out::Close;
                }
                compact_by(m, &self.sess, o, "");
                return Out::Close;
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

fn busy(state: &str) -> bool {
    matches!(state, "working" | "blocked" | "starting")
}

/// Has o summarise agent key's session and carry it on from the summary,
/// with draft in its box (compactBy's guard; the rest is the session's).
pub fn compact_by(m: &mut Model, key: &str, o: Summ, draft: &str) -> Out {
    if info(m, key).is_some_and(|i| i.proto < 9) {
        m.flash_err("Restart this session’s host before local compaction");
        return Out::Draw;
    }
    let now = super::now(m);
    // The dock's working line shows it, as it does Claude Code's own.
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
        v.sess.mark_compacting(now);
    }
    act(
        m,
        key,
        Act::Compact {
            by: o,
            draft: draft.into(),
        },
    )
}

/// When the main agent's prompt cache expired, and whether it has: the
/// next message then re-reads the whole context uncached. It isn't cold
/// before the session's first request (CacheCold).
pub fn cache_cold(s: &Session, warm: Ms, now: Ms) -> (Ms, bool) {
    let mut at = warm;
    if let Some(r) = s.requests.iter().rev().find(|r| r.run.is_empty()) {
        at = at.max(r.at + 3_600_000);
    }
    (at, at != 0 && now > at)
}

/// Who summarises agent key's session when rush compacts it rather than
/// its harness: the #compact default, else the agent's quick model; and
/// whether it can (cheapCompact).
pub fn cheap_compact(m: &Model, key: &str) -> (Summ, bool) {
    let Some(inf) = info(m, key) else {
        return (Summ::default(), false);
    };
    let d = Defaults::of(m);
    let mut o = Summ {
        kind: d.kind.clone(),
        label: m.reg.label(&d.kind),
        model: d.model.clone(),
        prune: false,
    };
    if d.kind == PRUNER_KIND {
        o = pruner(&d.prune_model);
    }
    if o.model.is_empty()
        && let Some(q) = m
            .reg
            .get(&inf.kind)
            .as_ref()
            .and_then(|a| a.querier().map(|q| q.query_models()))
    {
        o = Summ {
            kind: inf.kind.clone(),
            label: m.reg.label(&inf.kind),
            model: q.0,
            prune: false,
        };
    }
    let ok = !o.model.is_empty()
        && inf.proto >= 9
        && m.reg.supports(&inf.kind, Feature::Rewind)
        && !busy(&inf.state);
    (o, ok)
}

/// A typed /compact: on a cold cache the cheaper model of cheap_compact
/// writes the summary; warm, the harness compacts it itself from its
/// cache. /compact native, or with instructions, is always the harness's
/// own. Pass when it isn't this one's to do (compactTyped).
pub fn compact_typed(m: &mut Model, key: &str, arg: &str) -> Out {
    let Some(inf) = info(m, key) else {
        return Out::Pass;
    };
    if arg == "native" {
        return act(m, key, Act::Send("/compact".into()));
    }
    let (o, ok) = cheap_compact(m, key);
    let cold =
        super::session(m, key).is_some_and(|s| cache_cold(s, inf.cache_warm, super::now(m)).1);
    if !arg.is_empty() || !ok || !cold {
        return Out::Pass;
    }
    compact_by(m, key, o, "")
}
