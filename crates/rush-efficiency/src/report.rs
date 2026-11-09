//! What the figures say, for a person: each agent's totals, the sessions
//! that processed most tokens, the findings, and what Emeritus's handovers
//! saved. `rush efficiency` prints it and Settings › Usage shows it.
//! Rust-only: Go had no Efficiency CLI or screen in the end.

use std::collections::HashMap;
use std::io::Write;

use rush_core::adapter::Profile;
use rush_state::Time;
use serde_json::{Value, json};

use crate::aggregate::{AgentPart, Query};
use crate::catalog::CATALOG;
use crate::detect::{Env, Found};
use crate::events::{Event, HANDOVER, Handover, load_events};
use crate::findings::{Finding, findings, money, tokens};
use crate::price::cache_read_price_of;
use crate::store::Store;
use crate::top::SessionCost;
use crate::when::{DAY, add_secs, local, secs, since};

/// How many sessions the report lists.
pub const TOP: usize = 5;

/// How a handover's saving is worked out, said wherever it's shown.
pub const SAVING_METHOD: &str = "An estimate: the context the retiring session had at the handover, less what its successor started with, times the successor's requests. That is what each request would have re-read had the first carried on with the same work; it leaves out a compaction the first might have made, and what writing the baton cost. Priced at the cache-read rate.";

/// A successor's transcript starts at most this long before its handover
/// is logged (the log waits for its first request), or a little after.
const MATCH_BEFORE: i64 = 30 * 60;
const MATCH_AFTER: i64 = 5 * 60;

/// What one handover saved, by estimate (SAVING_METHOD).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saving {
    pub at: Time,
    pub h: Handover,
    /// The successor's transcript, when it was found, and its requests.
    pub transcript: String,
    pub requests: i64,
    /// The context each of its requests didn't re-read: A's at the
    /// handover less B's at its start.
    pub per_request: i64,
    /// per_request × requests, and that at the cache-read price (0 where
    /// the agent has no prices).
    pub tokens: i64,
    pub usd: f64,
}

/// Everything the report says for one query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub from: Time,
    pub to: Time,
    /// Each agent's totals, by kind ("claude", "codex").
    pub agents: Vec<(String, AgentPart)>,
    /// The transcripts that processed most tokens, most first.
    pub top: Vec<SessionCost>,
    /// Biggest cost first, in the Efficiency place's words.
    pub findings: Vec<Finding>,
    pub handovers: Vec<Saving>,
}

impl Report {
    /// The report of q, from what store holds, the savers found and the
    /// event log.
    pub fn build(
        store: &Store,
        q: &Query,
        found: &HashMap<String, Found>,
        events: &[Event],
    ) -> Report {
        let v = store.view(q);
        let mut top = store.top_sessions(q, usize::MAX);
        top.sort_by(|a, b| {
            processed(b)
                .cmp(&processed(a))
                .then_with(|| a.path.cmp(&b.path))
        });
        top.truncate(TOP);
        Report {
            from: q.from,
            to: q.to,
            agents: v
                .agents
                .iter()
                .map(|(k, a)| (k.clone(), a.clone()))
                .collect(),
            top,
            findings: findings(&v, found),
            handovers: savings(store, q, events),
        }
    }

    /// Whether there's anything to say.
    pub fn is_empty(&self) -> bool {
        self.agents.iter().all(|(_, a)| a.b.req == 0) && self.handovers.is_empty()
    }

    /// The handovers' estimated saving, in tokens and dollars.
    pub fn saved(&self) -> (i64, f64) {
        self.handovers
            .iter()
            .fold((0, 0.0), |(t, u), s| (t + s.tokens, u + s.usd))
    }
}

/// What a session processed: every token it sent and wrote.
pub fn processed(s: &SessionCost) -> i64 {
    s.b.context() + s.b.out
}

fn agent_of(kind: &str) -> &str {
    if kind.is_empty() { crate::AGENT } else { kind }
}

/// The handovers logged inside q, each with what it saved: its successor
/// is the transcript of the same account and agent, not the retiring
/// conversation, that began about when the handover was logged and
/// started nearest the context the handover says it did.
pub fn savings(store: &Store, q: &Query, events: &[Event]) -> Vec<Saving> {
    let inner = store.read();
    let mut out = Vec::new();
    for e in events {
        let Some(h) = e.handover.as_ref().filter(|_| e.kind == HANDOVER) else {
            continue;
        };
        let agent = agent_of(&h.agent);
        if e.at < q.from
            || e.at > q.to
            || !(q.accounts.is_empty() || q.accounts.contains(&e.account))
            || !(q.agents.is_empty() || q.agents.iter().any(|a| a == agent))
        {
            continue;
        }
        let mut best: Option<(&String, &crate::scan::File, i64)> = None;
        for (p, f) in &inner.files {
            let gap = since(f.first, e.at);
            if f.account != e.account
                || f.agent() != agent
                || (!h.from_session.is_empty() && f.session == h.from_session)
                || gap < -MATCH_BEFORE
                || gap > MATCH_AFTER
            {
                continue;
            }
            let off = (f.start() - h.to_context).abs();
            if off > (h.to_context / 4).max(5_000) {
                continue;
            }
            if best.is_none_or(|(bp, _, bo)| off < bo || (off == bo && p < bp)) {
                best = Some((p, f, off));
            }
        }
        let per_request = (h.from_context - h.to_context).max(0);
        let mut s = Saving {
            at: e.at,
            h: h.clone(),
            per_request,
            ..Saving::default()
        };
        if let Some((p, f, _)) = best {
            s.transcript = p.clone();
            s.requests = f.totals().req;
            s.tokens = per_request * s.requests;
            let model = if f.model.is_empty() {
                &h.model
            } else {
                &f.model
            };
            s.usd = s.tokens as f64 * cache_read_price_of(agent, model) / 1e6;
        }
        out.push(s);
    }
    out
}

/// The savers found in every Claude Code profile, the furthest each is
/// set up in any: what the findings point at.
pub fn found_in(profiles: &[Profile]) -> HashMap<String, Found> {
    let mut out: HashMap<String, Found> = HashMap::new();
    for p in profiles {
        if agent_of(&p.kind.0) != crate::AGENT {
            continue;
        }
        let env = Env::load(p);
        for s in CATALOG.iter() {
            let f = env.detect(s);
            let better = out
                .get(s.id)
                .is_none_or(|was| (f.status as i64) > (was.status as i64));
            if better {
                out.insert(s.id.to_string(), f);
            }
        }
    }
    out
}

/// A count with its thousands marked: 12,345.
pub fn count(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// A time, short: 7 Nov 09:12.
fn at(t: Time) -> String {
    local(t, "2 Jan 15:04")
}

/// first → last, the day said once when it's the same.
pub fn span(first: Time, last: Time) -> String {
    let (a, b) = (at(first), at(last));
    if local(first, "2 Jan") == local(last, "2 Jan") {
        format!("{a} → {}", local(last, "15:04"))
    } else {
        format!("{a} → {b}")
    }
}

/// A project as a line has room for: its last folder.
pub fn project_name(p: &str) -> String {
    let p = p.trim_end_matches('/');
    match p.rsplit('/').next() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => "?".into(),
    }
}

/// An agent's dollars, or that it has no prices.
pub fn cost_of(c: f64) -> String {
    if c > 0.0 {
        money(c)
    } else {
        "not priced".into()
    }
}

/// One top session's facts after its tokens: where, when, how big.
pub fn session_line(s: &SessionCost) -> String {
    format!(
        "{}  {}  peak {}  {} req{}",
        project_name(&s.project),
        span(s.first, s.last),
        tokens(s.peak_ctx),
        count(s.b.req),
        if s.sub { "  subagent" } else { "" }
    )
}

/// One handover in a line: who to whom, its contexts, what it saved.
pub fn handover_line(s: &Saving) -> String {
    let h = &s.h;
    let mut l = format!(
        "{} → {}, {}: {} → {}",
        h.from,
        h.to,
        at(s.at),
        tokens(h.from_context),
        tokens(h.to_context)
    );
    if h.subagent {
        l += ", subagent";
    }
    if s.transcript.is_empty() {
        l += &format!(
            ", ≈{} a request less (its successor's transcript isn't read yet)",
            tokens(s.per_request)
        );
    } else {
        l += &format!(
            ", ≈{} a request less × {} requests ≈ {} tokens",
            tokens(s.per_request),
            count(s.requests),
            tokens(s.tokens)
        );
        if s.usd > 0.0 {
            l += &format!(", ≈{}", money(s.usd));
        }
    }
    l
}

/// Words wrapped at w, for the findings' details.
fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The report as `rush efficiency` prints it.
pub fn text(r: &Report) -> String {
    let mut b = String::new();
    let mut p = |s: String| {
        b.push_str(&s);
        b.push('\n');
    };
    p(format!("Efficiency, {} to {}", at(r.from), at(r.to)));
    if r.is_empty() {
        p("No requests in that time.".into());
        return b;
    }
    p(String::new());
    p(format!(
        "  {:<8} {:>9} {:>9} {:>12} {:>11} {:>8} {:>11}",
        "", "requests", "uncached", "cache write", "cache read", "output", "cost"
    ));
    for (k, a) in &r.agents {
        let t = &a.b;
        p(format!(
            "  {:<8} {:>9} {:>9} {:>12} {:>11} {:>8} {:>11}",
            k,
            count(t.req),
            tokens(t.input),
            tokens(t.cw),
            tokens(t.cr),
            tokens(t.out),
            cost_of(t.cost())
        ));
    }
    if !r.top.is_empty() {
        p(String::new());
        p("Most tokens processed".into());
        for s in &r.top {
            p(format!(
                "  {:>6}  {}",
                tokens(processed(s)),
                session_line(s)
            ));
        }
    }
    if !r.findings.is_empty() {
        p(String::new());
        p("Findings, biggest cost first".into());
        for f in &r.findings {
            let c = if f.cost > 0.0 {
                money(f.cost)
            } else {
                String::new()
            };
            p(format!("  {c:>7}  {}", f.title));
            for l in wrap(&f.detail, 86) {
                p(format!("           {l}"));
            }
        }
    }
    if !r.handovers.is_empty() {
        let (t, u) = r.saved();
        p(String::new());
        let mut head = format!("Handovers, saving ≈{} tokens by estimate", tokens(t));
        if u > 0.0 {
            head += &format!(" (≈{})", money(u));
        }
        p(head);
        for s in &r.handovers {
            p(format!("  {}", handover_line(s)));
        }
        for l in wrap(SAVING_METHOD, 93) {
            p(format!("  {l}"));
        }
    }
    b
}

fn ts(t: Time) -> String {
    t.format()
}

/// The report as --json prints it.
pub fn to_json(r: &Report) -> Value {
    let bucket = |t: &crate::scan::Bucket| {
        json!({
            "requests": t.req,
            "uncached": t.input,
            "cacheWrite": t.cw,
            "cacheRead": t.cr,
            "output": t.out,
            "costUsd": t.cost(),
        })
    };
    let (t, u) = r.saved();
    json!({
        "from": ts(r.from),
        "to": ts(r.to),
        "agents": r.agents.iter().map(|(k, a)| {
            let mut v = bucket(&a.b);
            v["agent"] = json!(k);
            v["sessions"] = json!(a.sessions);
            v["subagents"] = json!(a.subagents);
            v
        }).collect::<Vec<_>>(),
        "top": r.top.iter().map(|s| json!({
            "transcript": s.path,
            "agent": s.agent,
            "project": s.project,
            "model": s.model,
            "subagent": s.sub,
            "first": ts(s.first),
            "last": ts(s.last),
            "processed": processed(s),
            "peakContext": s.peak_ctx,
            "requests": s.b.req,
            "costUsd": s.b.cost(),
        })).collect::<Vec<_>>(),
        "findings": r.findings.iter().map(|f| json!({
            "title": f.title,
            "detail": f.detail,
            "costUsd": f.cost,
            "fix": f.fix,
        })).collect::<Vec<_>>(),
        "handovers": r.handovers.iter().map(|s| json!({
            "at": ts(s.at),
            "from": s.h.from,
            "to": s.h.to,
            "agent": agent_of(&s.h.agent),
            "model": s.h.model,
            "fromContext": s.h.from_context,
            "fromCostUsd": s.h.from_cost_usd,
            "toContext": s.h.to_context,
            "threshold": s.h.threshold,
            "lineage": s.h.lineage,
            "subagent": s.h.subagent,
            "transcript": s.transcript,
            "requests": s.requests,
            "savedPerRequest": s.per_request,
            "savedTokens": s.tokens,
            "savedUsd": s.usd,
        })).collect::<Vec<_>>(),
        "saved": { "tokens": t, "usd": u, "estimate": true, "method": SAVING_METHOD },
    })
}

/// rush efficiency -h.
pub const USAGE: &str = "rush efficiency: where your agents' tokens went

  rush efficiency [--since 7d] [--agent claude|codex] [--json]

  Each agent's requests, input (uncached, cache writes, cache reads),
  output and dollars where rush has prices; the sessions that processed
  most tokens; what's worth changing, biggest cost first; and what
  Emeritus's handovers saved, by estimate. --since takes 90m, 24h, 2d or
  1w back from now.
";

/// A while back as --since says it: 90m, 24h, 2d, 1w; in seconds.
pub fn parse_since(s: &str) -> Option<i64> {
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = n.parse().ok().filter(|&n| n > 0)?;
    let per = match unit {
        "m" => 60,
        "h" => 3600,
        "d" => DAY,
        "w" => 7 * DAY,
        _ => return None,
    };
    Some(n * per)
}

/// The flags rush efficiency read.
#[derive(Debug, PartialEq)]
pub struct Flags {
    pub since: i64,
    pub agent: String,
    pub json: bool,
    pub help: bool,
}

/// Reads rush efficiency's flags: -x or --x, a value after it or after =.
pub fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags {
        since: 7 * DAY,
        agent: String::new(),
        json: false,
        help: false,
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let name = a.trim_start_matches('-');
        let (name, inline) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (name, None),
        };
        if a == "help" || name == "h" || name == "help" {
            f.help = true;
            i += 1;
            continue;
        }
        if !a.starts_with('-') {
            return Err(format!("{a} isn't a flag"));
        }
        let mut value = || -> Result<String, String> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("--{name} wants a value"))
        };
        match name {
            "json" => f.json = true,
            "since" => {
                let v = value()?;
                f.since = parse_since(&v).ok_or_else(|| {
                    format!("--since {v}: want a while back, as 90m, 24h, 2d or 1w")
                })?;
            }
            "agent" => {
                let v = value()?;
                if !crate::source::sources().iter().any(|s| s.kind() == v) {
                    return Err(format!("--agent {v}: rush reads claude's and codex's"));
                }
                f.agent = v;
            }
            _ => return Err(format!("no flag {a}")),
        }
        i += 1;
    }
    Ok(f)
}

/// rush efficiency: reads what's new in profiles' transcripts and prints
/// the report of the time --since asks for, back from now. Its exit code.
pub fn efficiency_cmd(
    profiles: &[Profile],
    args: &[String],
    now: Time,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let f = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            let _ = writeln!(stderr, "rush efficiency: {e}");
            let _ = stderr.write_all(USAGE.as_bytes());
            return 2;
        }
    };
    if f.help {
        let _ = stdout.write_all(USAGE.as_bytes());
        return 0;
    }
    let store = Store::open();
    if store.is_empty() {
        let _ = writeln!(
            stderr,
            "rush efficiency: reading transcripts the first time…"
        );
    }
    store.refresh(profiles);
    store.save();
    let q = query(now, f.since, &f.agent);
    let r = Report::build(&store, &q, &found_in(profiles), &load_events());
    let out = if f.json {
        format!("{}\n", to_json(&r))
    } else {
        text(&r)
    };
    match stdout.write_all(out.as_bytes()) {
        Ok(()) => 0,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            let _ = writeln!(stderr, "rush efficiency: {e}");
            1
        }
    }
}

/// The query of span seconds back from now, of one agent or all.
pub fn query(now: Time, span: i64, agent: &str) -> Query {
    Query {
        from: add_secs(now, -span),
        to: now,
        agents: if agent.is_empty() {
            Vec::new()
        } else {
            vec![agent.to_string()]
        },
        ..Query::default()
    }
}

/// The span the report covers, in words for a note: 7 days, 24 hours.
pub fn span_words(r: &Report) -> String {
    let s = secs(r.to) - secs(r.from);
    if s % DAY == 0 && s >= DAY {
        let d = s / DAY;
        if d == 1 {
            "24 hours".into()
        } else {
            format!("{d} days")
        }
    } else {
        format!("{} hours", (s + 1800) / 3600)
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
