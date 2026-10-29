//! The pieces an agent's views (the panels a session keeps beside its
//! conversation) are drawn with: tones, accents, bars and sparklines.
//! After Go's internal/ui/agentviews.go (viewsWidth :101, toneMark :110,
//! viewToneColour :127, toneEdge :136, accentOr :149, viewAccent :157,
//! viewsStrip :178, shareBar :181, progressBar :210, spark :216). Not
//! here yet: the views themselves (host.View, the panel, its folds and
//! clicks), which wait on rush hosting sessions.

use rush_core::text::{Color, Line};

use crate::style::*;

/// How wide the folded sidebar is (viewsStrip).
pub const STRIP: usize = 3;

/// How wide the sidebar is beside a conversation w wide, 0 when the
/// conversation would be left too narrow (viewsWidth).
pub fn views_width(w: usize) -> usize {
    let side = (w * 3 / 8).clamp(42, 64);
    if (w as isize) - (side as isize) < 72 {
        0
    } else {
        side
    }
}

/// A row's mark and its colour (toneMark).
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
pub fn tone_colour(tone: &str) -> Color {
    if tone.is_empty() || tone == "quiet" {
        BRIGHT
    } else {
        tone_mark(tone).1
    }
}

/// The colour of a tone's edges and bars: grey for none (toneEdge).
pub fn tone_edge(tone: &str) -> Color {
    match tone {
        "" => DIM,
        "quiet" => FAINT,
        _ => tone_mark(tone).1,
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
pub fn accent(name: &str) -> Color {
    match name {
        "blue" => BLUE,
        "green" => GREEN,
        "yellow" => YELLOW,
        "purple" => QUEUE,
        "red" => RED,
        _ => ORANGE,
    }
}

/// Stats' numbers side by side as one bar w wide, each in its tone; None
/// unless two or more are plain numbers at or above nought, each with a
/// tone, as parts of one whole are; stats are (value, tone) (shareBar).
pub fn share_bar(stats: &[(&str, &str)], w: usize) -> Option<Line> {
    let mut vals = Vec::with_capacity(stats.len());
    let mut sum = 0.0;
    for (v, tone) in stats {
        let f: f64 = v.trim().parse().ok()?;
        if f < 0.0 || tone.is_empty() {
            return None;
        }
        vals.push(f);
        sum += f;
    }
    if vals.len() < 2 || sum == 0.0 {
        return None;
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
        l.spans.push(sp("━".repeat(n), tone_edge(stats[i].1)));
    }
    l.spans.push(plain(blanks(w - used)));
    Some(l)
}

/// frac done as a bar w wide in col (progressBar).
pub fn progress_bar(frac: f64, w: usize, col: Color) -> Line {
    let n = ((frac * w as f64 + 0.5) as isize).clamp(0, w as isize) as usize;
    line([sp("━".repeat(n), col), sp("─".repeat(w - n), FAINT)])
}

/// vals as a sparkline at most w wide, its newest last (spark).
pub fn spark(vals: &[f64], w: usize, col: Color) -> Line {
    let vals = &vals[vals.len().saturating_sub(w)..];
    if vals.is_empty() {
        return Line::new();
    }
    let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let s: String = vals
        .iter()
        .map(|v| {
            let i = if hi > lo {
                ((v - lo) / (hi - lo) * (BARS.len() - 1) as f64) as usize
            } else {
                BARS.len() / 2
            };
            BARS[i]
        })
        .collect();
    line([sp(s, col)])
}
