//! The Overview view: the session's numbers at a glance, then by section:
//! what's running now, what each turn cost and on which model, the tools
//! it called, its prompt cache, its subagents and a link to the Memory
//! tab. After Go's internal/convo/overview.go (ColdStarts :34, CacheCold
//! :93, Totals :119, PrettyModel :151, Overview :191, overview :221,
//! Session.cost :639, Cost :651, modelColour :694, turnCosts :770),
//! internal/ui/rushmode.go:2324 (the overview's first rows) and
//! overviewsections.go (Session files :69, the Memory link :83).
//!
//! Not here yet: the context window by category and how it grew (the
//! session doesn't carry its usage reading in Rust), the permission mode,
//! the artifacts and the plugin and scratch sections the UI adds.

use std::collections::HashMap;

use rush_convo::Session;
use rush_convo::model::agent_name;
use rush_convo::render::dur;
use rush_core::adapter::{Compaction, fill_of};
use rush_core::agent::TokenUsage;
use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_core::tool::Kind;

use super::paint::{bar, dim, faint, fit_to, plural, row, section, text, tokens};
use crate::model::Model;
use crate::style::*;

/// What the session's agent says of its models: prices and windows
/// (Go's Session.cost, overview.go:639, and agent.ContextWindow).
pub struct Agentry<'a> {
    pub reg: &'a rush_core::adapter::Registry,
    pub kind: &'a str,
}

impl Agentry<'_> {
    /// What model's tokens u cost at the agent's prices, else the native
    /// agent's; 0 when neither can say.
    pub fn cost(&self, model: &str, u: &TokenUsage) -> f64 {
        if let Some(c) = self.reg.price(self.kind, model, u) {
            return c;
        }
        self.reg
            .native_agent()
            .and_then(|n| self.reg.price(n.kind().as_str(), model, u))
            .unwrap_or(0.0)
    }

    /// model's context window.
    pub fn window(&self, model: &str) -> i64 {
        self.reg.context_window(self.kind, model)
    }
}

/// What each turn cost: its requests priced, the turn's own total where
/// it has one (turnCosts, overview.go:770).
pub fn turn_costs(s: &Session, ag: &Agentry) -> Vec<f64> {
    let mut out = vec![0.0; s.turns.len()];
    let mut ti = 0;
    for r in &s.requests {
        while ti + 1 < s.turns.len() && r.at >= s.turns[ti + 1].start {
            ti += 1;
        }
        if ti < s.turns.len() {
            out[ti] += ag.cost(&r.model, &r.usage);
        }
    }
    for (i, t) in s.turns.iter().enumerate() {
        if t.cost > 0.0 {
            out[i] = t.cost;
        }
    }
    out
}

/// How long the main agent's prompt cache lives.
const CACHE_HOUR: Ms = 3_600_000;

/// claude-opus-5-5[1m] as "opus 5.5 · 1M" (PrettyModel).
pub fn pretty_model(m: &str) -> String {
    if m.is_empty() {
        return "?".into();
    }
    let long = m.contains("[1m]");
    let m = m.replace("[1m]", "");
    let mut m = m.strip_prefix("claude-").unwrap_or(&m).to_string();
    // A date at its end: -20250929.
    if let Some(i) = m.rfind('-')
        && m.len() - i == 9
        && m[i + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        m.truncate(i);
    }
    let parts: Vec<&str> = m.split('-').collect();
    let mut out = parts[0].to_string();
    if parts.len() > 1 {
        out.push(' ');
        out.push_str(&parts[1..].join("."));
    }
    if long {
        out.push_str(" · 1M");
    }
    out
}

/// Tells models apart in the per-turn bars (modelColour).
pub fn model_colour(m: &str) -> Color {
    let m = m.to_lowercase();
    if m.contains("opus") {
        ORANGE
    } else if m.contains("sonnet") {
        BLUE
    } else if m.contains("haiku") {
        GREEN
    } else if m.contains("fable") {
        YELLOW
    } else {
        SUB
    }
}

/// Totals across the whole session.
#[derive(Default, Debug, PartialEq)]
pub struct Totals {
    pub cost: f64,
    pub turns: usize,
    pub working: Ms,
    pub tool_calls: u64,
    pub failed: u64,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_out: i64,
    pub requests: usize,
}

pub fn totals(s: &Session, now: Ms) -> Totals {
    let mut t = Totals::default();
    for tn in &s.turns {
        t.turns += 1;
        t.cost += tn.cost;
        if tn.start > 0 {
            let end = if tn.live { now } else { tn.end };
            t.working += (end - tn.start).max(0);
        }
    }
    if s.info.cost > t.cost {
        t.cost = s.info.cost;
    }
    for ts in s.tools.values() {
        t.tool_calls += u64::from(ts.calls);
        t.failed += u64::from(ts.failed);
    }
    for r in &s.requests {
        t.requests += 1;
        let u = &r.usage;
        t.input += u.input;
        t.output += u.output;
        t.cache_read += u.cache_read;
        t.cache_out += u.cache_write_5m + u.cache_write_1h;
    }
    t
}

/// A request that wrote the prompt cache rather than reading it, and why.
#[derive(Debug, PartialEq)]
pub struct ColdStart {
    pub at: Ms,
    pub agent: String,
    pub gap: Ms,
    pub written: i64,
    pub reason: String,
    pub expected: bool,
}

/// The requests that started the cache afresh (ColdStarts).
pub fn cold_starts(s: &Session) -> Vec<ColdStart> {
    struct Last {
        at: Ms,
        model: String,
        ttl: Ms,
        size: i64,
    }
    let mut prev: HashMap<&str, Last> = HashMap::new();
    let mut out = Vec::new();
    for r in &s.requests {
        let u = &r.usage;
        let written = u.cache_write_5m + u.cache_write_1h;
        let total = u.cache_read + written;
        let size = total + u.input;
        let cold = written > 4096 && u.cache_read * 5 < total;
        let p = prev.get(&*r.run);
        let seen = p.is_some();
        let (p_at, p_model, p_ttl, p_size) = p.map_or((0, String::new(), 0, 0), |p| {
            (p.at, p.model.clone(), p.ttl, p.size)
        });
        let ttl = if u.cache_write_1h > 0 {
            CACHE_HOUR
        } else if u.cache_write_5m > 0 {
            300_000
        } else {
            p_ttl
        };
        prev.insert(
            &r.run,
            Last {
                at: r.at,
                model: r.model.to_string(),
                ttl,
                size,
            },
        );
        if !cold {
            continue;
        }
        let gap = if seen { r.at - p_at } else { 0 };
        let (reason, expected) = if !seen && !r.run.is_empty() {
            (
                "new subagent: every subagent starts its own cache".into(),
                true,
            )
        } else if !seen {
            ("session start".into(), true)
        } else if !p_model.is_empty() && !r.model.is_empty() && p_model != *r.model {
            (
                "model changed, and each model has its own cache".into(),
                true,
            )
        } else if size * 10 < p_size * 7 {
            ("compacted: the summary starts a new cache".into(), true)
        } else if p_ttl > 0 && gap >= p_ttl - p_ttl / 12 {
            let span = if p_ttl == CACHE_HOUR {
                "hour"
            } else {
                "5 minutes"
            };
            (format!("idle past the cache's {span}"), true)
        } else if u.cache_read > 0 {
            (
                format!(
                    "the prompt changed after its first {}, so the rest was written again",
                    tokens(u.cache_read)
                ),
                false,
            )
        } else {
            ("the cache was dropped early".into(), false)
        };
        out.push(ColdStart {
            at: r.at,
            agent: r.agent.to_string(),
            gap,
            written,
            reason,
            expected,
        });
    }
    out
}

/// When the main agent's cache goes cold, and whether it has (CacheCold).
pub fn cache_cold(s: &Session, now: Ms) -> Option<(Ms, bool)> {
    let r = s.requests.iter().rev().find(|r| r.run.is_empty())?;
    let at = r.at + CACHE_HOUR;
    Some((at, now > at))
}

/// Clock time hh:mm, local.
fn clock(ms: Ms) -> String {
    let secs = (ms / 1000 + crate::model::local_zone(ms)).rem_euclid(86_400);
    format!("{:02}:{:02}", secs / 3600, secs / 60 % 60)
}

/// The Overview's rows, w wide; refs where a row does something.
pub fn lines(m: &Model, key: &str, w: usize) -> Vec<(Line, String)> {
    let Some(v) = m.session.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<(Line, String)> = vec![(Line::new(), String::new())];
    if let Some(a) = m.agents.iter().find(|a| a.key == key) {
        out.push((
            line([plain("  "), spb(one_line(&a.display_name), TEXT)]),
            String::new(),
        ));
    }
    if v.sess.partial {
        out.push((Line::new(), String::new()));
        out.push((line([plain("  "), dim("counting…")]), String::new()));
        return out;
    }
    out.extend(
        overview(&v.sess, w, v.o.now, v.verbose, &agentry(m, key))
            .into_iter()
            .map(|l| (l, String::new())),
    );
    out
}

/// The Overview's Session files: what the session edited outside its
/// project, a row a file (overviewSections' Session files,
/// overviewsections.go:69, ownfiles.go ownFileRows).
pub fn session_files(own: &[super::changes::FileChange], w: usize) -> Vec<(Line, String)> {
    if own.is_empty() {
        return Vec::new();
    }
    let (a, d) = super::changes::change_stat(own);
    let mut out = vec![
        (Line::new(), String::new()),
        (
            section(
                "Session files",
                &format!("{} edited outside the project · +{a} −{d}", own.len()),
                w,
                3,
            ),
            String::new(),
        ),
    ];
    for fc in own {
        let stat = if fc.new {
            line([sp(format!("new · {} lines", fc.add), GREEN)])
        } else {
            super::changes::add_del(fc.add, fc.del)
        };
        let p = std::path::Path::new(&fc.path);
        let base = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = p
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = line([plain("    "), text(base), plain("  "), faint(tildify(&dir))]);
        let mut right = stat;
        right.spans.push(plain("  "));
        out.push((fit(super::paint::spread(name, right, w), w), String::new()));
    }
    out
}

/// The Overview's link to the Memory tab, with what the files cost a
/// session (overviewSections' Memory, overviewsections.go:83).
pub fn memory_section(m: &mut Model, key: &str, w: usize) -> Vec<(Line, String)> {
    let files = super::memory::memory_of(m, key)
        .map(|x| x.files)
        .unwrap_or_default();
    let up: i64 = files.iter().map(|f| f.up).sum();
    let left = line([
        plain("    "),
        sp("→ ", BLUE),
        text(format!("{} files", files.len())),
        dim(format!(
            " · ≈{} tokens every session",
            super::memory::tokens(up)
        )),
    ]);
    let right = line([dim("enter opens the Memory tab"), plain("  ")]);
    let mut row = rush_draw::truncate_line(&super::paint::spread(left, right, w), w, "…");
    let picked = m.session.as_ref().is_some_and(|v| v.sel == "go:memory");
    if picked {
        row = super::dock::picked1(row, w, true);
    }
    vec![
        (Line::new(), String::new()),
        (section("Memory", "", w, 3), String::new()),
        (row, "go:memory".to_string()),
    ]
}

/// Session key's agent, as the registry knows it.
pub fn agentry<'a>(m: &'a Model, key: &str) -> Agentry<'a> {
    let kind = m
        .agents
        .iter()
        .find(|a| a.key == key)
        .map_or("", |a| a.kind.as_str());
    Agentry { reg: &m.reg, kind }
}

/// The session's numbers as sections (overview).
pub fn overview(s: &Session, width_: usize, now: Ms, verbose: bool, ag: &Agentry) -> Vec<Line> {
    let w = width_;
    let mut out: Vec<Line> = Vec::new();
    let add = |out: &mut Vec<Line>, left: Line, right: Line| {
        out.push(row(None, left, right, width_, w));
    };
    let sect = |out: &mut Vec<Line>, title: &str, meta: &str| {
        if !out.is_empty() {
            out.push(row(None, Line::new(), Line::new(), width_, w));
        }
        let h = section(title, meta, w, 2);
        out.push(row(None, h, Line::new(), width_, w));
    };
    let label = |k: &str| line([plain("    "), dim(format!("{k:<13}"))]);
    let t = totals(s, now);
    let model = s
        .requests
        .iter()
        .rev()
        .find(|r| r.agent.is_empty() && !r.model.is_empty())
        .map(|r| r.model.to_string())
        .unwrap_or_else(|| s.info.model.clone());
    let model = if model.is_empty() {
        s.model.clone()
    } else {
        model
    };
    let effort = if s.effort().is_empty() {
        "default".to_string()
    } else {
        s.effort().to_string()
    };

    // --- tiles: the numbers that matter, at a glance ---
    let all = t.cache_read + t.cache_out + t.input;
    let hit = if all > 0 {
        t.cache_read as f64 / all as f64
    } else {
        0.0
    };
    let (mut ctx, mut ctx_label, mut ctx_col) = ("—".to_string(), "context".to_string(), TEXT);
    if s.context > 0 && !model.is_empty() {
        let f = fill_of(s.context, ag.window(&model), Compaction::default());
        let pct = f.pct() / 100.0;
        ctx = format!("{:.0}%", pct * 100.0);
        ctx_label = if f.compacts() {
            "auto-compact".to_string() // a tile has no room for both windows
        } else {
            format!("context of {}", tokens(f.window))
        };
        if pct >= 0.8 {
            ctx_col = RED;
        } else if pct >= 0.5 {
            ctx_col = YELLOW;
        }
    }
    let mut calls = line([spb(t.tool_calls.to_string(), TEXT)]);
    if t.failed > 0 {
        calls.spans.push(plain("  "));
        calls.spans.push(sp(format!("✗{}", t.failed), RED));
    }
    let mut spent = t.cost;
    if spent == 0.0 {
        // A transcript carries no totals: price each call.
        spent = s.requests.iter().map(|r| ag.cost(&r.model, &r.usage)).sum();
    }
    let money = rush_convo::render::money(spent);
    let tiles = [
        (
            line([spb(
                if money.is_empty() { "$0".into() } else { money },
                TEXT,
            )]),
            "spent".to_string(),
        ),
        (line([spb(dur(t.working), TEXT)]), "working".into()),
        (line([spb(t.turns.to_string(), TEXT)]), "turns".into()),
        (calls, "tool calls".into()),
        (line([spb(ctx, ctx_col)]), ctx_label),
        (
            line([spb(format!("{:.0}%", hit * 100.0), TEXT)]),
            "from cache".into(),
        ),
    ];
    let per = 3.max(tiles.len().min(w.saturating_sub(4) / 16));
    let tw = w.saturating_sub(4) / per;
    for chunk in tiles.chunks(per) {
        let (mut top, mut bottom) = (line([plain("  ")]), line([plain("  ")]));
        for (value, l) in chunk {
            top.spans.push(plain(" "));
            join(&mut top, fit_to(value.clone(), tw.saturating_sub(2)));
            top.spans.push(plain(" "));
            bottom.spans.push(plain(" "));
            join(
                &mut bottom,
                fit_to(line([dim(l.clone())]), tw.saturating_sub(2)),
            );
            bottom.spans.push(plain(" "));
        }
        out.push(row(None, top, Line::new(), width_, w));
        out.push(row(None, bottom, Line::new(), width_, w));
        out.push(Line::new());
    }
    add(
        &mut out,
        line([
            plain("  "),
            dim("now  "),
            text(pretty_model(&model)),
            dim(" · effort "),
            text(effort),
            dim("   in "),
            text(tokens(t.input + t.cache_read + t.cache_out)),
            dim(" · out "),
            text(tokens(t.output)),
            dim(format!(" · {} requests", t.requests)),
        ]),
        Line::new(),
    );
    for st in s.steps() {
        if st.kind == Kind::Subagent && st.status == rush_convo::Status::Running {
            add(
                &mut out,
                line([
                    plain("  "),
                    sp("⇉ ", ORANGE),
                    text(agent_name(st)),
                    dim(format!("   {}", one_line(&st.call().input.description))),
                ]),
                line([sp(dur(now - st.start), ORANGE)]),
            );
        }
    }

    // --- per turn: cost as bars in the model's colour, then where the
    // model and effort changed, in words ---
    if s.turns.len() > 1 {
        let n = s.turns.len();
        let mut models = vec![String::new(); n];
        let mut efforts = vec![String::new(); n];
        for (i, tn) in s.turns.iter().enumerate() {
            if !tn.model.is_empty() && !tn.model.starts_with('<') {
                models[i] = pretty_model(&tn.model);
                efforts[i] = if tn.effort.is_empty() {
                    "default".into()
                } else {
                    tn.effort.to_string()
                };
            }
            if models[i].is_empty() && i > 0 {
                models[i] = models[i - 1].clone();
                efforts[i] = efforts[i - 1].clone();
            }
        }
        for i in (0..n - 1).rev() {
            if models[i].is_empty() {
                models[i] = models[i + 1].clone();
                efforts[i] = efforts[i + 1].clone();
            }
        }
        let avail = w.saturating_sub(12);
        let col_w = 1.max(4.min(avail / n));
        let cols = n.min(avail / col_w);
        let from = n - cols;
        let costs = turn_costs(s, ag);
        let (mut most, mut top) = (0.0f64, from);
        for (i, &c) in costs.iter().enumerate().skip(from) {
            if c > most {
                most = c;
                top = i;
            }
        }
        let mut meta = plural(n, "turn");
        if cols < n {
            meta = format!("last {cols} of {n} turns");
        }
        if most > 0.0 {
            meta.push_str(&format!(
                " · most {}, on #{}",
                rush_convo::render::money(most),
                s.turns[top].n
            ));
        }
        sect(&mut out, "Per turn", &meta);
        if most > 0.0 {
            const HEIGHT: usize = 3;
            let levels: Vec<char> = " ▁▂▃▄▅▆▇█".chars().collect();
            let bw = if col_w > 1 { col_w - 1 } else { col_w };
            for r in (0..HEIGHT).rev() {
                let mut l = line([plain("    ")]);
                for (&c, model) in costs.iter().zip(&models).skip(from) {
                    let mut v = (c / most * (HEIGHT * 8) as f64 + 0.5) as isize;
                    if c > 0.0 {
                        v = v.max(1);
                    }
                    let g = levels[(v - r as isize * 8).clamp(0, 8) as usize];
                    l.spans
                        .push(sp(g.to_string().repeat(bw), model_colour(model)));
                    if col_w > 1 {
                        l.spans.push(plain(" "));
                    }
                }
                add(&mut out, l, Line::new());
            }
            let span = cols * col_w - (col_w - bw);
            let (first, last) = (
                format!("#{}", s.turns[from].n),
                format!("#{}", s.turns[n - 1].n),
            );
            let mut axis = first.clone();
            if span > first.len() + last.len() {
                axis.push_str(&blanks(span - first.len() - last.len()));
                axis.push_str(&last);
            }
            add(&mut out, line([plain("    "), dim(axis)]), Line::new());
        }
        // Runs of the same value, as "medium #1–4 · default #5–13".
        let runs = |vals: &[String], show: &dyn Fn(&str) -> Line| -> Line {
            let mut rs: Vec<(String, u32, u32)> = Vec::new();
            for (i, v) in vals.iter().enumerate() {
                let tn = s.turns[i].n;
                if let Some(last) = rs.last_mut()
                    && last.0 == *v
                {
                    last.2 = tn;
                    continue;
                }
                rs.push((v.clone(), tn, tn));
            }
            if rs.len() == 1 {
                return show(&rs[0].0);
            }
            let mut l = Line::new();
            for (i, (v, a, b)) in rs.iter().enumerate() {
                if i > 0 {
                    l.spans.push(dim(" · "));
                }
                if i == 5 && rs.len() > 6 && !verbose {
                    l.spans.push(dim(format!("{} more changes", rs.len() - 5)));
                    break;
                }
                join(&mut l, show(v));
                let at = if a == b {
                    format!("#{a}")
                } else {
                    format!("#{a}–{b}")
                };
                l.spans.push(plain(" "));
                l.spans.push(dim(at));
            }
            l
        };
        let mut ml = label("model");
        join(
            &mut ml,
            runs(&models, &|m: &str| {
                if m.is_empty() {
                    line([dim("unknown")])
                } else {
                    line([sp("■ ", model_colour(m)), text(m.to_string())])
                }
            }),
        );
        add(&mut out, ml, Line::new());
        let mut el = label("effort");
        join(
            &mut el,
            runs(&efforts, &|e: &str| line([text(e.to_string())])),
        );
        add(&mut out, el, Line::new());
    }

    // --- tools ---
    let mut ts: Vec<(&str, &rush_convo::ToolStat)> = s
        .tools
        .iter()
        .filter(|(_, v)| v.calls > 0)
        .map(|(k, v)| (&**k, v))
        .collect();
    ts.sort_by(|a, b| b.1.calls.cmp(&a.1.calls).then(a.0.cmp(b.0)));
    if !ts.is_empty() {
        sect(&mut out, "Tools", &plural(t.tool_calls as usize, "call"));
        let name_w = ts
            .iter()
            .map(|(n, _)| rush_core::text::width(n).min(26))
            .fold(8, usize::max);
        let top = f64::from(ts[0].1.calls).sqrt();
        let bar_w = 10.max(30.min(w.saturating_sub(name_w + 30)));
        for (i, (name, v)) in ts.iter().enumerate() {
            if i >= 12 && !verbose {
                add(
                    &mut out,
                    line([
                        plain("    "),
                        dim(format!("… {} more tools · ctrl+o shows all", ts.len() - i)),
                    ]),
                    Line::new(),
                );
                break;
            }
            let mut right = Line::new();
            if v.failed > 0 {
                right.spans.push(sp(format!("{} failed", v.failed), RED));
                right.spans.push(plain("   "));
            }
            if v.time >= 100 {
                right.spans.push(dim(dur(v.time)));
            }
            let mut l = line([plain("    ")]);
            join(&mut l, fit_to(line([text(name.to_string())]), name_w));
            l.spans.push(plain("  "));
            join(&mut l, bar(f64::from(v.calls).sqrt() / top, bar_w, SUB));
            l.spans.push(plain(" "));
            l.spans.push(text(v.calls.to_string()));
            add(&mut out, l, right);
        }
    }

    // --- cache ---
    if t.requests > 0 {
        let cold = cold_starts(s);
        let (mut expected, mut idle, mut odd) = (0, 0, 0);
        let mut kinds: HashMap<String, usize> = HashMap::new();
        for c in &cold {
            if !c.expected {
                odd += 1;
            } else if c.reason.starts_with("idle") {
                idle += 1;
            } else {
                expected += 1;
                let k = c.reason.split(':').next().unwrap_or("").to_string();
                *kinds.entry(k).or_default() += 1;
            }
        }
        let mut meta = format!("{:.0}% of input read from cache", hit * 100.0);
        if !cold.is_empty() {
            meta.push_str(&format!(" · {} cold starts", cold.len()));
            if odd == 0 {
                meta.push_str(", all expected");
            }
        }
        sect(&mut out, "Cache", &meta);
        if let Some((at, is_cold)) = cache_cold(s, now) {
            // What the next message costs to re-read the context: a cache
            // read while it's warm, a full rewrite once it's cold.
            let price = |u: TokenUsage| {
                let c = ag.cost(&model, &u);
                if c > 0.0 {
                    format!(" ≈ {}", rush_convo::render::money(c))
                } else {
                    String::new()
                }
            };
            let read = || TokenUsage {
                cache_read: s.context,
                ..TokenUsage::default()
            };
            let write = || TokenUsage {
                cache_write_1h: s.context,
                ..TokenUsage::default()
            };
            let mut l = label("now");
            if is_cold {
                l.spans.push(sp("cold", YELLOW));
                l.spans.push(dim(" since "));
                l.spans.push(text(clock(at)));
                l.spans.push(dim(format!(", {} ago", dur(now - at))));
                add(&mut out, l, Line::new());
                if s.context > 0 {
                    let mut l = label("next message");
                    l.spans.push(dim("rewrites "));
                    l.spans.push(text(tokens(s.context)));
                    l.spans.push(dim(price(write())));
                    add(&mut out, l, Line::new());
                }
            } else {
                let left = at - now;
                let col = if left < 600_000 { YELLOW } else { GREEN };
                l.spans.push(sp("warm", col));
                l.spans.push(dim(" till "));
                l.spans.push(text(clock(at)));
                l.spans.push(dim(", "));
                l.spans.push(sp(dur(left), col));
                l.spans.push(dim(" left   "));
                join(&mut l, bar(left as f64 / CACHE_HOUR as f64, 12, col));
                add(&mut out, l, Line::new());
                if s.context > 0 {
                    let mut l = label("next message");
                    l.spans.push(dim("reads "));
                    l.spans.push(text(tokens(s.context)));
                    l.spans.push(dim(price(read())));
                    l.spans
                        .push(dim(format!(" · after {} it rewrites it", clock(at))));
                    l.spans.push(dim(price(write())));
                    add(&mut out, l, Line::new());
                }
            }
        }
        let mut l = label("hit rate");
        join(&mut l, bar(hit, 20, GREEN));
        l.spans.push(plain(" "));
        l.spans.push(text(format!("{:.0}%", hit * 100.0)));
        add(&mut out, l, Line::new());
        let mut l = label("read");
        l.spans.push(text(tokens(t.cache_read)));
        l.spans.push(dim("   written "));
        l.spans.push(text(tokens(t.cache_out)));
        add(&mut out, l, Line::new());
        if expected > 0 {
            let mut parts = Vec::new();
            for k in [
                "new subagent",
                "session start",
                "model changed, and each model has its own cache",
            ] {
                if let Some(&n) = kinds.get(k).filter(|n| **n > 0) {
                    let name = k.split(',').next().unwrap_or(k);
                    parts.push(format!("{n} × {name}"));
                }
            }
            let mut l = label("expected");
            l.spans.push(dim(format!(
                "{} — a new cache is how these begin",
                parts.join(" · ")
            )));
            add(&mut out, l, Line::new());
        }
        let mut shown = 0;
        for c in &cold {
            if c.expected && !c.reason.starts_with("idle") {
                continue;
            }
            if shown >= 6 && !verbose {
                add(
                    &mut out,
                    line([plain("    "), dim(format!("… {} more", idle + odd - shown))]),
                    Line::new(),
                );
                break;
            }
            shown += 1;
            let who = if c.agent.is_empty() { "main" } else { &c.agent };
            let col = if c.expected { DIM } else { YELLOW };
            let mut l = label(&clock(c.at));
            l.spans.push(sp(c.reason.clone(), col));
            l.spans
                .push(dim(format!(" · {} idle · {who} · rewrote ", dur(c.gap))));
            l.spans.push(text(tokens(c.written)));
            add(&mut out, l, Line::new());
        }
        if cold.is_empty() {
            let mut l = label("went cold");
            l.spans.push(dim("never"));
            add(&mut out, l, Line::new());
        }
    }

    // --- subagents ---
    let mut subs: HashMap<String, (usize, usize, String)> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    for st in s.steps() {
        if &*st.tool != "Task" && &*st.tool != "Agent" {
            continue;
        }
        let kind = agent_name(st);
        let e = subs.entry(kind.clone()).or_insert_with(|| {
            names.push(kind.clone());
            (0, 0, String::new())
        });
        e.0 += 1;
        e.1 += st.children.len();
    }
    if !names.is_empty() {
        for r in &s.requests {
            if let Some(e) = subs.get_mut(&*r.agent)
                && !r.model.is_empty()
            {
                e.2 = r.model.to_string();
            }
        }
        names.sort();
        sect(&mut out, "Subagents", "");
        let name_w = names
            .iter()
            .map(|n| rush_core::text::width(n).min(30))
            .fold(8, usize::max);
        for n in &names {
            let (runs, steps, model) = &subs[n];
            let right = if *steps > 0 {
                line([dim(plural(*steps, "step"))])
            } else {
                Line::new()
            };
            let mut l = line([plain("    ")]);
            join(&mut l, fit_to(line([text(n.clone())]), name_w));
            l.spans.push(dim(format!("  ×{runs:<4} ")));
            if !model.is_empty() {
                l.spans.push(text(pretty_model(model)));
            }
            add(&mut out, l, right);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_read_as_people_say_them() {
        for (m, want) in [
            ("claude-opus-5-5[1m]", "opus 5.5 · 1M"),
            ("claude-sonnet-4-5-20250929", "sonnet 4.5"),
            ("gpt-6", "gpt 6"),
            ("", "?"),
        ] {
            assert_eq!(pretty_model(m), want, "{m}");
        }
        assert_eq!(model_colour("opus 5.5"), ORANGE);
        assert_eq!(model_colour("haiku"), GREEN);
    }
}
