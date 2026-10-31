//! What fills a session's context window, drawn: by category as one bar
//! or as Claude Code's grid with its legend, what's inside each part, how
//! the context grew turn by turn, and what changed since the agent's last
//! start. The Overview's Context section and /context's tab read them.
//! After Go's internal/convo/context.go (ctxHues :20, ctxColour :34,
//! ctxParts :50, ContextBar :67, ContextLegend :93, ContextDetail :112,
//! ContextGrid :203) and growth.go (ContextGrowth :28, contextGrowth :40,
//! ContextGrowthLines :79, ContextChangeLines :129, toolCount :190,
//! deltaRow :197, fineTokens :213).

use rush_convo::{Item, Session};
use rush_core::text::{Color, Line, Span};
use rush_core::time::Ms;
use rush_core::usage::{Change, Context, ContextPart, Delta};

use super::paint::{dim, faint, fit_to, plural, text, tokens};
use crate::style::*;

/// Each category's colour, its own hue so neighbours in the bar stay
/// apart (ctxHues).
const HUES: [(&str, Color); 8] = [
    ("System prompt", Color::Rgb(86, 140, 230)),
    ("System tools", Color::Rgb(160, 130, 220)),
    ("MCP tools", Color::Rgb(127, 191, 138)),
    ("MCP server", Color::Rgb(70, 180, 170)),
    ("Custom agents", Color::Rgb(220, 120, 170)),
    ("Memory", Color::Rgb(235, 230, 220)),
    ("Skills", Color::Rgb(225, 190, 80)),
    ("Messages", Color::Rgb(217, 119, 87)),
];

/// A category's colour, the same in the overview and /context: the
/// longest name it starts with (ctxColour).
pub fn ctx_colour(name: &str) -> Color {
    HUES.iter()
        .filter(|(p, _)| name.starts_with(p))
        .max_by_key(|(p, _)| p.len())
        .map_or(DIM, |(_, c)| *c)
}

/// The categories in the window in Claude Code's order: what's used, then
/// the auto-compact buffer, then what's free; deferred tools aren't in it
/// (ctxParts).
fn parts(u: &Context) -> (Vec<&ContextPart>, i64, i64) {
    let (mut used, mut buffer, mut free) = (Vec::new(), 0, 0);
    for c in &u.parts {
        if c.deferred || c.kind == "deferred" {
            continue;
        }
        match c.kind.as_str() {
            "buffer" => buffer += c.tokens,
            "free" => free += c.tokens,
            _ => used.push(c),
        }
    }
    (used, buffer, free)
}

/// The window as one stacked bar w cells wide: each category in its
/// colour, then the auto-compact buffer, then what's free (ContextBar).
pub fn bar(u: &Context, w: usize) -> Line {
    let mut out = Line::new();
    if u.max <= 0 || w == 0 {
        return out;
    }
    let (used, buffer, _) = parts(u);
    let (wi, max) = (w as i64, u.max);
    let (mut n, mut at) = (0i64, 0i64);
    for c in used {
        at += c.tokens;
        let mut end = wi.min((at * wi + max / 2) / max);
        if c.tokens > 0 && end <= n && n < wi {
            end = n + 1; // everything there shows, if only as one cell
        }
        let k = (end - n).max(0) as usize;
        if k > 0 {
            out.spans.push(sp("█".repeat(k), ctx_colour(&c.name)));
        }
        n = n.max(end);
    }
    let buf_at = wi - (buffer * wi + max / 2) / max;
    if n < buf_at {
        out.spans.push(faint("─".repeat((buf_at - n) as usize)));
        n = buf_at;
    }
    let rest = (wi - n).max(0) as usize;
    if rest > 0 {
        out.spans.push(faint("░".repeat(rest)));
    }
    out
}

/// Text padded or cut to w cells.
fn fit(s: &str, w: usize) -> String {
    fit_to(line([plain(s)]), w).plain()
}

/// A row per category: its colour, name, tokens and share
/// (ContextLegend).
pub fn legend(u: &Context, lead: &str) -> Vec<Line> {
    if u.max <= 0 {
        return Vec::new();
    }
    let (used, buffer, free) = parts(u);
    let pct = |n: i64| format!("{:5.1}%", n as f64 / u.max as f64 * 100.0);
    let mut out = Vec::new();
    for c in used {
        out.push(line([
            plain(lead),
            sp("█ ", ctx_colour(&c.name)),
            text(fit(&c.name, 20)),
            plain(" "),
            text(format!("{:>6}", tokens(c.tokens))),
            plain(" "),
            dim(pct(c.tokens)),
        ]));
    }
    if buffer > 0 {
        out.push(line([
            plain(lead),
            faint("░ "),
            dim(fit("Auto-compact buffer", 20)),
            plain(" "),
            dim(format!("{:>6}", tokens(buffer))),
            plain(" "),
            faint(pct(buffer)),
        ]));
    }
    out.push(line([
        plain(lead),
        faint("─ "),
        dim(fit("Free", 20)),
        plain(" "),
        dim(format!("{:>6}", tokens(free))),
        plain(" "),
        faint(pct(free)),
    ]));
    out
}

/// The window as rows of cells, as Claude Code's /context draws it: each
/// cell a share of the window in its category's colour, the free space
/// dotted and the auto-compact buffer shaded at the end (ContextGrid).
pub fn grid(u: &Context, cols: usize, rows: usize) -> Vec<Line> {
    if u.max <= 0 || cols == 0 || rows == 0 {
        return Vec::new();
    }
    let (used, buffer, _) = parts(u);
    let n = (cols * rows) as i64;
    let mut cells: Vec<Span> = Vec::with_capacity(n as usize);
    let mut at = 0i64;
    for c in used {
        at += c.tokens;
        let mut end = n.min((at * n + u.max / 2) / u.max);
        let len = cells.len() as i64;
        if c.tokens > 0 && end <= len && len < n {
            end = len + 1;
        }
        while (cells.len() as i64) < end {
            cells.push(sp("■", ctx_colour(&c.name)));
        }
    }
    let buf_at = n - (buffer * n + u.max / 2) / u.max;
    while (cells.len() as i64) < buf_at {
        cells.push(faint("·"));
    }
    while (cells.len() as i64) < n {
        cells.push(faint("░"));
    }
    (0..rows)
        .map(|r| {
            let mut l = Line::new();
            for (i, c) in cells[r * cols..(r + 1) * cols].iter().enumerate() {
                if i > 0 {
                    l.spans.push(plain(" "));
                }
                l.spans.push(c.clone());
            }
            l
        })
        .collect()
}

/// What's inside the categories: the biggest skills, MCP servers, agents,
/// memory files and tools' calls and results, each most first and each
/// at most `each` rows (ContextDetail).
pub fn detail(u: &Context, lead: &str, each: usize) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut group = |title: String, mut rows: Vec<(String, i64)>| {
        if rows.is_empty() {
            return;
        }
        // Most first, those even kept in their order (SliceStable).
        rows.sort_by_key(|r| std::cmp::Reverse(r.1));
        if !out.is_empty() {
            out.push(Line::new());
        }
        out.push(line([plain(lead), spb(title, SUB)]));
        for (i, (name, n)) in rows.iter().enumerate() {
            if i == each {
                out.push(line([
                    plain(lead),
                    plain("  "),
                    dim(format!("… {} more", rows.len() - each)),
                ]));
                break;
            }
            out.push(line([
                plain(lead),
                plain("  "),
                text(fit(name, 34)),
                plain(" "),
                dim(tokens(*n)),
            ]));
        }
    };
    let sk = &u.skills;
    if !sk.each.is_empty() {
        group(
            format!(
                "Skills · {} of {} listed · {}",
                sk.included,
                sk.total,
                tokens(sk.tokens)
            ),
            sk.each.iter().map(|s| (s.name.clone(), s.tokens)).collect(),
        );
    }
    if !u.mcp_tools.is_empty() {
        // By server, in the order first seen (a map in Go: sorted after).
        let mut by: Vec<(String, i64, usize)> = Vec::new();
        for t in &u.mcp_tools {
            match by.iter_mut().find(|b| b.0 == t.server) {
                Some(b) => {
                    b.1 += t.tokens;
                    b.2 += 1;
                }
                None => by.push((t.server.clone(), t.tokens, 1)),
            }
        }
        by.sort_by(|a, b| a.0.cmp(&b.0));
        group(
            "MCP servers".into(),
            by.into_iter()
                .map(|(s, t, n)| (format!("{s} ({n} tools)"), t))
                .collect(),
        );
    }
    if !u.agents.is_empty() {
        group(
            "Custom agents".into(),
            u.agents
                .iter()
                .map(|a| (a.kind.clone(), a.tokens))
                .collect(),
        );
    }
    if !u.memory_files.is_empty() {
        group(
            "Memory files".into(),
            u.memory_files
                .iter()
                .map(|f| (f.path.clone(), f.tokens))
                .collect(),
        );
    }
    let m = &u.messages;
    if m.tool_calls + m.tool_results + m.user + m.assistant + m.attachments > 0 {
        let xs = [
            ("tool results", m.tool_results),
            ("tool calls", m.tool_calls),
            ("Claude's words", m.assistant),
            ("your messages", m.user),
            ("attachments", m.attachments),
        ];
        group(
            "Messages".into(),
            xs.iter()
                .filter(|x| x.1 > 0)
                .map(|(n, t)| (n.to_string(), *t))
                .collect(),
        );
    }
    if !m.by_tool.is_empty() {
        group(
            "Messages by tool".into(),
            m.by_tool
                .iter()
                .map(|t| (t.name.clone(), t.calls + t.results))
                .collect(),
        );
    }
    out
}

/// One turn's context: what its last request sent, and how big it was
/// when the agent compacted during it (ContextStep).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub turn: u32,
    pub tokens: i64,
    pub compacted: i64,
}

/// The context each turn ended with, from the main agent's requests;
/// turns that sent nothing and didn't compact are left out
/// (ContextGrowth).
pub fn growth(s: &Session) -> Vec<Step> {
    let n = s.turns.len();
    if n == 0 {
        return Vec::new();
    }
    let mut toks = vec![0i64; n];
    let mut sent = vec![false; n];
    for r in &s.requests {
        if !r.run.is_empty() || !r.agent.is_empty() {
            continue;
        }
        // The turn it went in: the last one started by then.
        let i = s
            .turns
            .partition_point(|t| t.start <= r.at)
            .saturating_sub(1);
        let u = &r.usage;
        toks[i] = u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h;
        sent[i] = true;
    }
    let mut out = Vec::new();
    for (i, t) in s.turns.iter().enumerate() {
        let mut st = Step {
            turn: t.n,
            tokens: toks[i],
            compacted: 0,
        };
        for it in &t.items {
            if let Item::Compact { compact, .. } = it
                && compact.before > st.compacted
            {
                st.compacted = compact.before;
            }
        }
        if sent[i] || st.compacted > 0 {
            out.push(st);
        }
    }
    out
}

/// growth in w cells: a bar per turn, the newest at the right, the turns
/// it compacted in yellow; then where it started and ended, its peak, and
/// the compactions with the size each compacted from
/// (ContextGrowthLines).
pub fn growth_lines(steps: &[Step], lead: &str, w: usize) -> Vec<Line> {
    if steps.len() < 2 || w < 8 {
        return Vec::new();
    }
    let from = steps.len().saturating_sub(w);
    let shown = &steps[from..];
    let peak = shown
        .iter()
        .map(|s| s.tokens.max(s.compacted))
        .max()
        .unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    let levels: Vec<char> = "▁▂▃▄▅▆▇█".chars().collect();
    let mut b = line([plain(lead)]);
    for st in shown {
        let v = st.tokens.max(st.compacted);
        let g = levels[((v * levels.len() as i64 / (peak + 1)) as usize).min(levels.len() - 1)];
        let c = if st.compacted > 0 {
            YELLOW
        } else {
            ctx_colour("Messages")
        };
        b.spans.push(sp(g.to_string(), c));
    }
    let (first, last) = (shown[0], shown[shown.len() - 1]);
    let mut caption = format!(
        "#{} {} → #{} {} · peak {}",
        first.turn,
        tokens(first.tokens),
        last.turn,
        tokens(last.tokens),
        tokens(peak)
    );
    if from > 0 {
        caption = format!("last {} turns · {caption}", shown.len());
    }
    let mut out = vec![b, line([plain(lead), dim(caption)])];
    let mut comp: Vec<String> = steps
        .iter()
        .filter(|s| s.compacted > 0)
        .map(|s| format!("#{} from {}", s.turn, tokens(s.compacted)))
        .collect();
    if !comp.is_empty() {
        let mut more = String::new();
        if comp.len() > 3 {
            more = format!(" · {} before", comp.len() - 3);
            comp = comp.split_off(comp.len() - 3);
        }
        out.push(line([
            plain(lead),
            sp("▮ ", YELLOW),
            dim(format!("compacted {}{more}", comp.join(" · "))),
        ]));
    }
    out
}

fn change_empty(c: &Change) -> bool {
    c.tools_added.is_empty()
        && c.tools_removed.is_empty()
        && c.servers_added.is_empty()
        && c.servers_removed.is_empty()
        && c.sections.is_empty()
        && c.parts.is_empty()
}

/// What changed since the agent's last start: tools and MCP servers
/// added and removed, rush's prompt sections (estimated) and the agent's
/// own categories (counted) (ContextChangeLines).
pub fn change_lines(c: Option<&Change>, harness: &str, lead: &str, w: usize, now: Ms) -> Vec<Line> {
    let Some(c) = c.filter(|c| !change_empty(c)) else {
        return Vec::new();
    };
    let mut head = line([plain(lead), spb("Since the last start", SUB)]);
    let at = c.at.ms();
    if at != 0 {
        let ago = (now - at).max(0) / 1000 * 1000;
        head.spans.push(dim(format!(
            " · restarted {} ago",
            rush_convo::render::dur(ago)
        )));
    }
    let mut out = vec![head];
    let names = |xs: &[String]| {
        rush_draw::truncate(
            &xs.join(", "),
            10.max(w.saturating_sub(lead.len() + 20)),
            "…",
        )
        .into_owned()
    };
    // A server's tools go with it, counted, rather than listed.
    let of = |tools: &[String], server: &str| -> (usize, Vec<String>) {
        let pre = format!("mcp__{server}__");
        let n = tools.iter().filter(|t| t.starts_with(&pre)).count();
        let rest = tools.iter().filter(|t| !t.starts_with(&pre)).cloned();
        (n, rest.collect())
    };
    let mark = |plus: bool| {
        if plus {
            sp("+ ", GREEN)
        } else {
            sp("− ", RED)
        }
    };
    let tool_count = |n: usize| {
        if n == 0 {
            String::new()
        } else {
            format!(" · {}", plural(n, "tool"))
        }
    };
    let (mut added, mut removed) = (c.tools_added.clone(), c.tools_removed.clone());
    for sv in &c.servers_added {
        let (n, rest) = of(&added, sv);
        added = rest;
        out.push(line([
            plain(lead),
            plain("  "),
            mark(true),
            text(format!("MCP server {sv}")),
            dim(tool_count(n)),
        ]));
    }
    for sv in &c.servers_removed {
        let (n, rest) = of(&removed, sv);
        removed = rest;
        out.push(line([
            plain(lead),
            plain("  "),
            mark(false),
            text(format!("MCP server {sv}")),
            dim(tool_count(n)),
        ]));
    }
    if !added.is_empty() {
        out.push(line([
            plain(lead),
            plain("  "),
            mark(true),
            text(plural(added.len(), "tool")),
            dim(format!("  {}", names(&added))),
        ]));
    }
    if !removed.is_empty() {
        out.push(line([
            plain(lead),
            plain("  "),
            mark(false),
            text(plural(removed.len(), "tool")),
            dim(format!("  {}", names(&removed))),
        ]));
    }
    for d in &c.sections {
        let mut l = line([plain(lead), plain("  ")]);
        join(
            &mut l,
            delta_row(d, &format!("rush's prompt: {}", d.name), None),
        );
        l.spans.push(faint(" · estimated"));
        out.push(l);
    }
    if !c.parts.is_empty() {
        out.push(line([
            plain(lead),
            plain("  "),
            dim(format!("as counted by {harness}:")),
        ]));
        for d in &c.parts {
            let mut l = line([plain(lead), plain("    ")]);
            join(
                &mut l,
                delta_row(d, &d.name, Some(sp("█ ", ctx_colour(&d.name)))),
            );
            out.push(l);
        }
    }
    out
}

/// One size that changed: + added, − gone, ~ changed, with its tokens
/// before and after (deltaRow).
fn delta_row(d: &Delta, name: &str, swatch: Option<Span>) -> Line {
    let mut l = Line::new();
    let named = |l: &mut Line, sign: Span| {
        l.spans.push(sign);
        if let Some(s) = swatch.clone() {
            l.spans.push(s);
        }
        l.spans.push(text(fit(name, 30)));
        l.spans.push(plain(" "));
    };
    if d.added {
        named(&mut l, sp("+ ", GREEN));
        l.spans.push(dim(format!("{:>6}", fine_tokens(d.now))));
        return l;
    }
    if d.gone {
        named(&mut l, sp("− ", RED));
        l.spans.push(dim(format!("{:>6}", fine_tokens(d.was))));
        return l;
    }
    let change = d.now - d.was;
    let (sign, col) = if change < 0 {
        ("−", GREEN)
    } else {
        ("+", YELLOW)
    };
    named(&mut l, faint("~ "));
    l.spans.push(dim(format!(
        "{:>6} → {} ",
        fine_tokens(d.was),
        fine_tokens(d.now)
    )));
    l.spans
        .push(sp(format!("{sign}{}", fine_tokens(change.abs())), col));
    l
}

/// tokens with a decimal under 10k, where a change of a few hundred would
/// otherwise round away (fineTokens).
fn fine_tokens(n: i64) -> String {
    if (1000..10_000).contains(&n) {
        let s = format!("{:.1}", n as f64 / 1e3);
        return format!("{}k", s.trim_end_matches(".0"));
    }
    tokens(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::usage::ContextPart;

    fn part(name: &str, tokens: i64, kind: &str) -> ContextPart {
        ContextPart {
            name: name.into(),
            tokens,
            kind: kind.into(),
            deferred: false,
        }
    }

    fn usage() -> Context {
        Context {
            parts: vec![
                part("System prompt", 3_000, ""),
                part("System tools", 12_000, ""),
                part("Messages", 45_000, ""),
                part("Deferred", 9_000, "deferred"),
                part("Autocompact buffer", 40_000, "buffer"),
                part("Free space", 100_000, "free"),
            ],
            total: 60_000,
            max: 200_000,
            ..Context::default()
        }
    }

    // context.go ContextBar: each part a share of w, the buffer shaded at
    // the end, what's left in between ruled.
    #[test]
    fn bar_is_the_window_in_shares() {
        let b = bar(&usage(), 20);
        assert_eq!(b.plain(), "██████──────────░░░░");
        assert_eq!(b.width(), 20);
    }

    #[test]
    fn grid_and_legend() {
        let g = grid(&usage(), 10, 2);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].plain(), "■ ■ ■ ■ ■ ■ · · · ·");
        assert_eq!(g[1].plain(), "· · · · · · ░ ░ ░ ░");
        let l: Vec<String> = legend(&usage(), "").iter().map(Line::plain).collect();
        assert_eq!(
            l,
            [
                "█ System prompt            3k   1.5%",
                "█ System tools            12k   6.0%",
                "█ Messages                45k  22.5%",
                "░ Auto-compact buffer     40k  20.0%",
                "─ Free                   100k  50.0%",
            ]
        );
    }

    // growth.go ContextGrowthLines: where it started and ended, its peak,
    // and the compactions.
    #[test]
    fn growth_caption() {
        let steps = [
            Step {
                turn: 1,
                tokens: 74_000,
                compacted: 0,
            },
            Step {
                turn: 2,
                tokens: 75_000,
                compacted: 0,
            },
        ];
        let l: Vec<String> = growth_lines(&steps, "    ", 80)
            .iter()
            .map(Line::plain)
            .collect();
        assert_eq!(l, ["    ██", "    #1 74k → #2 75k · peak 75k"]);
        assert!(growth_lines(&steps[..1], "", 80).is_empty());
    }

    #[test]
    fn fine_tokens_keep_a_decimal_under_10k() {
        assert_eq!(fine_tokens(1_200), "1.2k");
        assert_eq!(fine_tokens(2_000), "2k");
        assert_eq!(fine_tokens(12_400), "12k");
        assert_eq!(fine_tokens(800), "800");
    }
}
