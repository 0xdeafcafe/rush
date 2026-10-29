//! A session's views beside its conversation: the panel, its blocks in
//! every style `rush view` has, the rows a click opens and their buttons.
//! After Go's internal/ui/agentviews.go, viewblocks.go,
//! viewblocks_flow.go, viewblocks_tabular.go, viewblocks_visual.go and
//! viewacts.go; the data is model.rs (internal/host/views.go).

pub mod acts;
pub mod blocks;
mod flow;
pub mod model;
pub mod panel;
pub mod quiet;
mod tabular;
mod visual;

use rush_core::text::{Color, Line, Span, Style, width};

use crate::style::*;

pub use acts::{Btn, ViewAct, ViewLine};
pub use model::*;
pub use panel::{Click, ViewsState, views_panel, views_width};

/// Whether the plugin runs with views in its ui (agentviews.go:68
/// pluginDrawsViews).
pub fn plugin_draws_views(m: &crate::model::Model, name: &str) -> bool {
    let Some(h) = m.hooks() else { return false };
    h.state()
        .plugins
        .iter()
        .find(|p| p.name == name)
        .is_some_and(|p| p.ui.iter().any(|u| u == rush_plugin::ui::UI_VIEWS))
}

/// A session's views as they show: a plugin's only while it runs allowed
/// to draw them, so one stopped or revoked leaves nothing behind
/// (agentviews.go:53, viewsOf's end).
pub fn shown_views(list: &[View], draws: impl Fn(&str) -> bool) -> Vec<View> {
    list.iter()
        .filter(|v| v.by.is_empty() || draws(&v.by))
        .cloned()
        .collect()
}

/// The line a session's newest view gives the list: the agent's own
/// before any a plugin drew (agentviews.go:82 viewSummary).
pub fn view_summary(vs: &[View]) -> &str {
    for plugins in [false, true] {
        if let Some(v) = vs
            .iter()
            .find(|v| !v.summary.is_empty() && v.by.is_empty() != plugins)
        {
            return &v.summary;
        }
    }
    ""
}

/// The ground of a button (bgBtw), and of one under the pointer (bgMark).
pub const BTN_BG: Color = Color::Rgb(36, 33, 30);
pub const BTN_LIT_BG: Color = Color::Rgb(74, 64, 54);

/// A row's mark and its colour (agentviews.go:110 toneMark).
pub fn tone_mark(tone: &str) -> (&'static str, Color) {
    match tone {
        "good" => ("✓", GREEN),
        "bad" => ("✗", RED),
        "warn" => ("▲", YELLOW),
        "busy" => ("◐", BLUE),
        "quiet" => ("·", DIM),
        _ => ("•", SUB),
    }
}

/// A number's colour for its tone (viewToneColour).
pub fn view_tone_colour(tone: &str) -> Color {
    if tone.is_empty() || tone == "quiet" {
        return BRIGHT;
    }
    tone_mark(tone).1
}

/// The colour of a tone's edges and bars: grey for none (toneEdge).
pub fn tone_edge(tone: &str) -> Color {
    match tone {
        "" => DIM,
        "quiet" => FAINT,
        t => tone_mark(t).1,
    }
}

/// A tone's colour, the view's accent for none (accentOr).
pub fn accent_or(tone: &str, accent: Color) -> Color {
    if tone.is_empty() {
        accent
    } else {
        tone_edge(tone)
    }
}

/// The colour a view took as its own, rush's orange if none (viewAccent).
pub fn view_accent(v: &View) -> Color {
    match v.accent.as_str() {
        "blue" => BLUE,
        "green" => GREEN,
        "yellow" => YELLOW,
        "purple" => QUEUE,
        "red" => RED,
        _ => ORANGE,
    }
}

/// A number as short as it reads: 14, not 14.000000 (num).
pub fn num(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e21 {
        return format!("{}", f as i128);
    }
    format!("{f}")
}

/// How long ago, in a word or two (ago); ms.
pub fn ago(ms: i64) -> String {
    let s = ms / 1000;
    if s < 10 {
        "just now".into()
    } else if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else {
        format!("{}h ago", s / 3600)
    }
}

/// s on one line, cut to w with … (prs.go:804 cellwTrunc).
pub fn trunc(s: &str, w: usize) -> String {
    rush_draw::truncate(&one_line(s), w, "…").into_owned()
}

/// A span of s in c.
pub fn paint(c: Color, s: impl Into<String>) -> Span {
    sp(s, c)
}

/// s in the quiet colour.
pub fn dim(s: impl Into<String>) -> Span {
    sp(s, DIM)
}

/// s in the faintest colour.
pub fn faint(s: impl Into<String>) -> Span {
    sp(s, FAINT)
}

/// Lines joined into one.
pub fn cat(parts: impl IntoIterator<Item = Line>) -> Line {
    let mut l = Line::new();
    for p in parts {
        l.spans.extend(p.spans);
    }
    l
}

/// Spans as a line, empty ones kept out.
pub fn ln(spans: impl IntoIterator<Item = Span>) -> Line {
    line(spans)
}

/// left and right w apart, or left fit to w when they don't both fit
/// (rushmode.go:2175 spread).
pub fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut l = left;
    l.spans.push(plain(blanks(gap as usize)));
    l.spans.extend(right.spans);
    l
}

/// done of total as a bar w wide in col (progressBar).
pub fn progress_bar(frac: f64, w: usize, col: Color) -> Line {
    let n = ((frac * w as f64 + 0.5) as isize).clamp(0, w as isize) as usize;
    ln([paint(col, "━".repeat(n)), faint("─".repeat(w - n))])
}

/// vals as a sparkline at most w wide, its newest last (spark).
pub fn spark(vals: &[f64], w: usize, col: Color) -> Line {
    let vals = &vals[vals.len().saturating_sub(w)..];
    if vals.is_empty() {
        return Line::new();
    }
    let (mut lo, mut hi) = (vals[0], vals[0]);
    for &v in vals {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let bars: Vec<char> = "▁▂▃▄▅▆▇█".chars().collect();
    let s: String = vals
        .iter()
        .map(|&v| {
            let i = if hi > lo {
                ((v - lo) / (hi - lo) * (bars.len() - 1) as f64) as usize
            } else {
                bars.len() / 2
            };
            bars[i.min(bars.len() - 1)]
        })
        .collect();
    ln([paint(col, s)])
}

/// The stats' numbers side by side as one bar w wide, each in its tone:
/// empty unless two or more are plain numbers above nought, each with a
/// tone (shareBar).
pub fn share_bar(ss: &[Stat], w: usize) -> Line {
    let mut vals = Vec::new();
    let mut sum = 0.0;
    for s in ss {
        match s.value.trim().parse::<f64>() {
            Ok(f) if f >= 0.0 && !s.tone.is_empty() => {
                vals.push(f);
                sum += f;
            }
            _ => return Line::new(),
        }
    }
    if vals.len() < 2 || sum == 0.0 {
        return Line::new();
    }
    let mut l = Line::new();
    let (mut used, mut acc) = (0usize, 0.0);
    for (i, f) in vals.iter().enumerate() {
        acc += f;
        let mut n = ((acc / sum * w as f64 + 0.5) as isize - used as isize).max(0) as usize;
        if *f > 0.0 && n == 0 && used < w {
            n = 1; // a number above nought always shows
        }
        n = n.min(w - used);
        used += n;
        l.spans.push(paint(tone_edge(&ss[i].tone), "━".repeat(n)));
    }
    l.spans.push(plain(blanks(w - used)));
    line(l.spans)
}

/// A styled bold span.
pub fn bolded(c: Color, s: impl Into<String>) -> Span {
    Span::new(s, Style::fg(c).bold())
}

/// Cells s takes.
pub fn cw(s: &str) -> usize {
    width(s)
}

#[cfg(test)]
mod tests;
