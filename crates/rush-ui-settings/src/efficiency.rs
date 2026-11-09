//! Settings › Usage's figures (Rust-only; Go had no Efficiency screen in
//! the end): under the rules, what the last 7 days of your agents'
//! transcripts say, as `rush efficiency` prints it. Each agent's totals,
//! the sessions that processed most tokens, the findings biggest cost
//! first, and what Emeritus's handovers saved, by estimate. Each row's
//! About says the rest. Read as the page opens, off the UI; the sections
//! show once there's something in them.

use std::rc::Rc;
use std::sync::OnceLock;

use rush_core::text::Line;
use rush_efficiency::report::{
    self, Report, SAVING_METHOD, cost_of, count, found_in, handover_line, processed, query,
    session_line, span, span_words,
};
use rush_efficiency::{Store, load_events, money, tokens};
use rush_state::Time;

use super::form::{Section, Setting};
use super::paint::trunc;
use super::{common, dialog};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// How far back the page looks.
pub const SPAN: i64 = 7 * 24 * 3600;

/// The transcripts' figures, kept while rush runs: a refresh reads only
/// what's new.
static STORE: OnceLock<Store> = OnceLock::new();

/// Reads what's new in every agent's transcripts and works out the
/// report, off the UI. A read that finds nothing never hides one shown.
pub fn read(m: &mut Model) {
    m.later(
        || {
            let reg = common::reg();
            let profiles: Vec<_> = reg.all().iter().flat_map(|a| reg.profiles_of(a)).collect();
            let store = STORE.get_or_init(Store::open);
            store.refresh(&profiles);
            store.save();
            let q = query(Time::now(), SPAN, "");
            Report::build(store, &q, &found_in(&profiles), &load_events())
        },
        |m, r| {
            show(m, r);
            Out::Draw
        },
    );
}

/// Shows r on the page, unless it's empty and the page has one already.
pub fn show(m: &mut Model, r: Report) {
    if let Some(d) = m.kept::<super::State>().dialog.as_mut()
        && (!r.is_empty() || d.efficiency.is_none())
    {
        d.efficiency = Some(Rc::new(r));
    }
}

/// A row that's a fact, drawn as l; label and what are its About.
fn fact(label: String, l: Line, what: String) -> Setting {
    Setting {
        label,
        what,
        line: Some(Rc::new(move |w: usize| trunc(l.clone(), w))),
        ..Setting::default()
    }
}

/// The figures' sections, none until the report has something.
pub fn sections(m: &Model) -> Vec<Section> {
    let Some(r) = dialog(m).efficiency.clone().filter(|r| !r.is_empty()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let reg = common::reg();
    let rows = r
        .agents
        .iter()
        .map(|(kind, a)| {
            let t = &a.b;
            let name = reg.harness_label(kind);
            let l = line([
                sp(format!("{name:<12}"), TEXT),
                sp(format!("{:>7} req", count(t.req)), TEXT),
                sp(
                    format!(
                        " · in {} / {} / {} · out {} · ",
                        tokens(t.input),
                        tokens(t.cw),
                        tokens(t.cr),
                        tokens(t.out)
                    ),
                    SUB,
                ),
                sp(cost_of(t.cost()), TEXT),
            ]);
            let what = format!(
                "{name}: {} sessions and {} subagent runs started in these {}; {} requests sent {} uncached, wrote {} to the cache and read {} from it (a tenth of the price), and {} came out. That cost {}: list prices, where rush has them; it has none for Codex. rush efficiency prints the same.",
                a.sessions,
                a.subagents,
                span_words(&r),
                count(t.req),
                tokens(t.input),
                tokens(t.cw),
                tokens(t.cr),
                tokens(t.out),
                cost_of(t.cost())
            );
            fact(name, l, what)
        })
        .collect();
    out.push(Section {
        title: "Where tokens went".into(),
        note: format!("the last {}, from your agents' transcripts", span_words(&r)),
        rows,
        ..Section::default()
    });

    if !r.top.is_empty() {
        let rows = r
            .top
            .iter()
            .map(|s| {
                let l = line([
                    sp(format!("{:>6}  ", tokens(processed(s))), TEXT),
                    sp(session_line(s), SUB),
                ]);
                let what = format!(
                    "{} tokens sent and written over {} requests, {}, on {}{}; its context peaked at {} and cost {}. {}",
                    tokens(processed(s)),
                    count(s.b.req),
                    span(s.first, s.last),
                    s.model,
                    if s.sub { " as a subagent" } else { "" },
                    tokens(s.peak_ctx),
                    cost_of(s.b.cost()),
                    s.path
                );
                fact(report::project_name(&s.project), l, what)
            })
            .collect();
        out.push(Section {
            title: "Most tokens processed".into(),
            note: "each session's input and output".into(),
            rows,
            ..Section::default()
        });
    }

    if !r.findings.is_empty() {
        let rows = r
            .findings
            .iter()
            .map(|f| {
                let cost = if f.cost > 0.0 {
                    money(f.cost)
                } else {
                    String::new()
                };
                let l = line([
                    sp(format!("{cost:>7}  "), ORANGE),
                    sp(f.title.clone(), TEXT),
                ]);
                fact(f.title.clone(), l, f.detail.clone())
            })
            .collect();
        out.push(Section {
            title: "Findings".into(),
            note: "biggest cost first".into(),
            rows,
            ..Section::default()
        });
    }

    if !r.handovers.is_empty() {
        let (t, u) = r.saved();
        let mut note = format!("saving ≈{} tokens by estimate", tokens(t));
        if u > 0.0 {
            note += &format!(", ≈{}", money(u));
        }
        let rows = r
            .handovers
            .iter()
            .map(|s| {
                let h = &s.h;
                let what = format!(
                    "{}; threshold {}, lineage {}. {SAVING_METHOD}",
                    handover_line(s),
                    tokens(h.threshold),
                    h.lineage
                );
                // The row says what it saved; About says the rest.
                let mut l = format!(
                    "{} → {}: {} → {}",
                    h.from,
                    h.to,
                    tokens(h.from_context),
                    tokens(h.to_context)
                );
                if s.transcript.is_empty() {
                    l += &format!(", ≈{} a request less", tokens(s.per_request));
                } else {
                    l += &format!(", saved ≈{} tokens", tokens(s.tokens));
                    if s.usd > 0.0 {
                        l += &format!(", ≈{}", money(s.usd));
                    }
                }
                fact(format!("{} → {}", h.from, h.to), line([sp(l, TEXT)]), what)
            })
            .collect();
        out.push(Section {
            title: "Emeritus handovers".into(),
            note,
            rows,
            ..Section::default()
        });
    }
    out
}
