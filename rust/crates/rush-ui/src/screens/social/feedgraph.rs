//! The feed's graph: who's been talking to whom. Each agent that has
//! messaged another is a mark set round an ellipse, and a line joins each
//! pair that have, labelled with how many messages went between them,
//! brighter the busier, orange while fresh. g in the feed switches to it
//! and back; it keeps to the open hashtag tab. Port of Go's feedgraph.go.

use std::collections::HashMap;

use rush_core::text::{Color, Line, Span, Style};
use rush_core::time::Ms;

use super::feed::FeedSheet;
use super::ink::{cells, handle_tint, keys_controls, truncate};
use super::stream::{Post, Stream};
use crate::model::Model;
use crate::style::*;

/// How long a pair's line stays orange after a message.
const FRESH: Ms = 2 * 60_000;

/// A pair of agents that have messaged each other.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    /// Their @handles.
    pub a: String,
    pub b: String,
    /// Messages between them, both ways.
    pub n: usize,
    pub last: Ms,
}

/// The agents in posts that have messaged another, busiest first, and
/// each pair of them that has (feedGraph).
pub fn feed_graph(posts: &[Post]) -> (Vec<String>, Vec<Edge>) {
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    let mut sent: HashMap<String, usize> = HashMap::new();
    let mut nodes: Vec<String> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    for p in posts.iter().filter(|p| !p.to.is_empty()) {
        let (mut a, mut b) = (p.username(), p.to.clone());
        if b < a {
            std::mem::swap(&mut a, &mut b);
        }
        let i = *at.entry((a.clone(), b.clone())).or_insert_with(|| {
            edges.push(Edge {
                a: a.clone(),
                b: b.clone(),
                n: 0,
                last: 0,
            });
            edges.len() - 1
        });
        edges[i].n += 1;
        edges[i].last = edges[i].last.max(p.at);
        for h in [a, b] {
            if !sent.contains_key(&h) {
                nodes.push(h.clone());
            }
            *sent.entry(h).or_default() += 1;
        }
    }
    nodes.sort_by(|x, y| sent[y].cmp(&sent[x]));
    (nodes, edges)
}

/// The graph sheet's body (graphBody).
pub fn graph_body(s: &mut FeedSheet, m: &Model, w: usize, h: usize) -> Vec<Line> {
    let all = m
        .kept
        .peek::<Stream>()
        .map_or(&[][..], |s| s.posts.as_slice());
    let mut out = vec![crate::frame::sheet::sheet_title(
        "the feed 🐓",
        "who's been talking to whom",
        w,
    )];
    let tags = s.tags(all);
    s.tab_row = None;
    if tags.len() > 1 {
        s.tab_row = Some(out.len() + 1);
        let cur = tags.iter().position(|t| *t == s.tag).unwrap_or(0);
        out.push(Line::new());
        out.push(super::feed::feed_tabs(&tags, cur, w));
    }
    let (nodes, edges) = feed_graph(&s.shown(all));
    let mut footer = vec![Line::new()];
    footer.extend(keys_controls(
        w,
        &[("g", "Timeline"), ("[ ]", "Hashtag"), ("esc", "Close")],
    ));
    s.rows.clear(); // nothing here to click
    if edges.is_empty() {
        out.push(Line::new());
        out.push(line([sp("No agents have messaged each other yet.", TEXT)]));
        out.push(line([sp(
            "Their messages to each other land here as they send them.",
            DIM,
        )]));
    } else {
        let total: usize = edges.iter().map(|e| e.n).sum();
        let mut busiest = &edges[0];
        for e in &edges {
            if e.n > busiest.n {
                busiest = e;
            }
        }
        out.push(Line::new());
        out.push(line([sp(
            format!(
                "{} agents · {total} messages · busiest {} ↔ {}, {}",
                nodes.len(),
                busiest.a,
                busiest.b,
                busiest.n
            ),
            DIM,
        )]));
        out.push(Line::new());
        let gh = h.saturating_sub(out.len() + footer.len()).max(3);
        let now = m.clock.unwrap_or_else(rush_core::time::now);
        out.extend(draw_graph(&nodes, &edges, w, gh, now));
    }
    while out.len() + footer.len() < h {
        out.push(Line::new());
    }
    out.extend(footer);
    out.into_iter().map(|l| fit(l, w)).collect()
}

/// One cell of the graph as it's drawn.
#[derive(Clone, Copy, PartialEq)]
struct Cell {
    r: char,
    ink: Option<(Color, bool)>,
}

/// nodes laid round an ellipse in a w×h box, each edge's pair joined by a
/// line and its count, and each agent's name by its mark: before it on
/// the left half, after it on the right (drawGraph).
pub fn draw_graph(nodes: &[String], edges: &[Edge], w: usize, h: usize, now: Ms) -> Vec<Line> {
    let blank = Cell { r: ' ', ink: None };
    let mut grid = vec![vec![blank; w]; h];
    let set =
        |grid: &mut Vec<Vec<Cell>>, x: isize, y: isize, r: char, ink: Option<(Color, bool)>| {
            if y >= 0 && (y as usize) < h && x >= 0 && (x as usize) < w {
                grid[y as usize][x as usize] = Cell { r, ink };
            }
        };
    // The longest name, up to a quarter of the width.
    let name_w = nodes.iter().map(|n| cells(n).min(w / 4)).max().unwrap_or(0);
    // The marks: round an ellipse from the top, clockwise; two face each
    // other.
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    let (rx, ry) = ((cx - name_w as f64 - 3.0).max(1.0), (cy - 1.0).max(0.0));
    let start = if nodes.len() == 2 {
        std::f64::consts::PI
    } else {
        -std::f64::consts::PI / 2.0
    };
    let mut pos: HashMap<&str, (isize, isize)> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        let a = start + 2.0 * std::f64::consts::PI * i as f64 / nodes.len() as f64;
        pos.insert(
            n,
            (
                (cx + rx * a.cos()).round() as isize,
                (cy + ry * a.sin()).round() as isize,
            ),
        );
    }
    let busy = edges.iter().map(|e| e.n).max().unwrap_or(0);
    // Quiet pairs first, so the busy ones are drawn over them.
    let mut order: Vec<&Edge> = edges.iter().collect();
    order.sort_by_key(|e| e.n);
    for e in order {
        let ink = if now - e.last < FRESH {
            ORANGE
        } else if e.n * 2 >= busy {
            SUB
        } else if e.n * 4 >= busy {
            DIM
        } else {
            FAINT
        };
        let (p, q) = (pos[e.a.as_str()], pos[e.b.as_str()]);
        let l = graph_line(p, q);
        if l.len() > 2 {
            for c in &l[1..l.len() - 1] {
                set(&mut grid, c.0, c.1, stroke(p, q), Some((ink, false)));
            }
        }
        let mid = l[l.len() / 2];
        let label = format!(" {} ", e.n);
        let half = (label.len() / 2) as isize;
        for (i, r) in label.chars().enumerate() {
            set(
                &mut grid,
                mid.0 - half + i as isize,
                mid.1,
                r,
                Some((ink, true)),
            );
        }
    }
    for n in nodes {
        let p = pos[n.as_str()];
        let tint = handle_tint(n);
        let name = truncate(n, name_w, "…");
        set(&mut grid, p.0, p.1, '●', Some((tint, false)));
        let mut x = p.0 + 2;
        if (p.0 as f64) < cx {
            x = p.0 - 1 - cells(&name) as isize;
        }
        set(&mut grid, x - 1, p.1, ' ', None);
        for r in name.chars() {
            set(&mut grid, x, p.1, r, Some((tint, true)));
            x += 1;
        }
        set(&mut grid, x, p.1, ' ', None);
    }
    grid.into_iter()
        .map(|row| {
            let mut l = Line::new();
            let mut i = 0;
            while i < row.len() {
                let ink = row[i].ink;
                let mut end = i;
                let mut run = String::new();
                while end < row.len() && row[end].ink == ink {
                    run.push(row[end].r);
                    end += 1;
                }
                l.spans.push(match ink {
                    None => Span::plain(run),
                    Some((c, b)) => {
                        let st = if b { Style::fg(c).bold() } else { Style::fg(c) };
                        Span::new(run, st)
                    }
                });
                i = end;
            }
            l
        })
        .collect()
}

/// The cells from p to q, both ends in (graphLine).
pub fn graph_line(p: (isize, isize), q: (isize, isize)) -> Vec<(isize, isize)> {
    let (dx, dy) = ((q.0 - p.0).abs(), -(q.1 - p.1).abs());
    let (sx, sy) = ((q.0 - p.0).signum(), (q.1 - p.1).signum());
    let mut out = Vec::new();
    let (mut x, mut y, mut e) = (p.0, p.1, dx + dy);
    loop {
        out.push((x, y));
        if x == q.0 && y == q.1 {
            return out;
        }
        let e2 = 2 * e;
        if e2 >= dy {
            e += dy;
            x += sx;
        }
        if e2 <= dx {
            e += dx;
            y += sy;
        }
    }
}

/// The box-drawing stroke for a line from p to q, a cell being about
/// twice as tall as it's wide (graphStroke).
pub fn stroke(p: (isize, isize), q: (isize, isize)) -> char {
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    if dx.abs() >= 4 * dy.abs() {
        '─'
    } else if dx.abs() <= dy.abs() {
        '│'
    } else if (dx > 0) == (dy > 0) {
        '╲'
    } else {
        '╱'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_room::community::Author;

    fn msg(from: &str, to: &str, at: Ms) -> Post {
        Post {
            author: Author {
                handle: from.into(),
                ..Author::default()
            },
            to: format!("@{to}"),
            at,
            ..Post::default()
        }
    }

    // Each agent that's messaged another is a mark with its name, each
    // pair a line with its count, and nothing runs past the box.
    #[test]
    fn draws_who_talks_to_whom() {
        let now = 10 * 3_600_000;
        let ago = now - 3_600_000;
        let mut posts = vec![
            msg("pepper-bean", "scout-dodo", ago),
            msg("scout-dodo", "pepper-bean", ago),
            msg("pepper-bean", "scout-dodo", ago),
            msg("pepper-bean", "waffle-bramble", ago),
            msg("waffle-bramble", "marmite-bramble", ago),
        ];
        posts.push(Post {
            title: "a chirp".into(),
            author: Author {
                name: "x".into(),
                ..Author::default()
            },
            ..Post::default()
        });
        let (nodes, edges) = feed_graph(&posts);
        assert!(
            nodes.len() == 4 && nodes[0] == "@pepper-bean" && edges.len() == 3 && edges[0].n == 3,
            "nodes {nodes:?} edges {edges:?}"
        );
        let rows = draw_graph(&nodes, &edges, 80, 16, now);
        let text: String = rows.iter().map(|l| l.plain() + "\n").collect();
        for n in &nodes {
            assert!(text.contains(n.as_str()), "{n} missing:\n{text}");
        }
        assert!(
            text.contains(" 3 ") && text.matches('●').count() == 4,
            "{text}"
        );
        assert!(rows.iter().all(|l| l.width() <= 80));
    }

    #[test]
    fn lines_and_strokes() {
        assert_eq!(
            graph_line((0, 0), (3, 0)),
            vec![(0, 0), (1, 0), (2, 0), (3, 0)]
        );
        assert_eq!(graph_line((0, 0), (0, 2)), vec![(0, 0), (0, 1), (0, 2)]);
        assert_eq!(stroke((0, 0), (8, 1)), '─');
        assert_eq!(stroke((0, 0), (1, 3)), '│');
        assert_eq!(stroke((0, 0), (3, 2)), '╲');
        assert_eq!(stroke((0, 0), (3, -2)), '╱');
    }
}
