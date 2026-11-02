//! The feed docked at the very foot of the Agents list: every chirp and
//! reply, newest at the bottom, one a row, the older ones pushed up and
//! off as posts come in; faded until a post is new or the pointer is on
//! it. A hovered post's @handle, when it's cut, scrolls in its own room
//! like a marquee (stream.rs marquee); nothing else on the row moves. A
//! click opens the feed at that post. Port of Go's stream.go:17-25 (the
//! constants), :102-228 (streamDock, streamLines, streamColsOf,
//! streamRow), view.go:2130 (fadeText), view.go:1159 (where it
//! docks) and model.go:800 (the click); what rush did on its own under
//! it (view.go:1163, messages.go:158 newsDock, model.go:812 its click).

use std::time::{Duration, Instant};

use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};

use super::ink::{cells, clean_text, fields, tagged, tint_beside};
use super::stream::{FRESH, Post, Stream, marquee, name_room, next_tint};
use crate::feed::ModelFeed;
use crate::frame::{Mouse, Out};
use crate::model::Model;
use crate::stream::ModelStream;
use crate::style::*;

/// A row's key: this, then its post's key.
pub const KEY_PREFIX: &str = "community:";
/// How far a quiet dock fades to the ground.
const FADE: f64 = 0.55;
/// The most posts the docked timeline shows.
pub const DOCK_POSTS: usize = 6;

/// Which row the pointer came onto, and when: where a hovered handle's
/// scrolling starts from. Another row, or none, starts it again.
#[derive(Default)]
struct Hovered {
    key: String,
    at: Option<Instant>,
}

/// Where a row's columns sit: authors padded to one width, the text after
/// them, the age right-aligned in a column of its own (streamCols).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cols {
    pub handle: usize,
    pub text: usize,
    pub age: usize,
}

/// Model's side of this module: Model is rush-ui-base's, so its
/// methods here are a trait's.
pub trait ModelDock {
    fn stream_lines(&mut self, w: usize, room: usize) -> Vec<(Line, String)>;
    fn stream_cols(&self, posts: &[Post], w: usize, now: Ms) -> Cols;
}

impl ModelDock for Model {
    /// The docked timeline in at most room rows: none when the feed is
    /// off, quiet or the list too narrow (streamDock, streamLines).
    fn stream_lines(&mut self, w: usize, room: usize) -> Vec<(Line, String)> {
        if self.kept::<Stream>().on != Some(true)
            || room < 2
            || w < 24
            || self.kept::<Stream>().posts.is_empty()
        {
            return Vec::new();
        }
        let now = self.now;
        let inner = w - 2; // one column in, one clear of the divider
        let mut kept: Vec<Post> = Vec::with_capacity(room);
        let mut seen = std::collections::HashSet::new();
        for p in self.kept::<Stream>().posts.iter().rev() {
            if kept.len() >= room - 1 {
                break;
            }
            // The same agent saying the same thing again.
            if seen.insert(format!("{}\0{}", p.username(), p.said())) {
                kept.push(p.clone());
            }
        }
        kept.reverse(); // oldest first, as drawn
        let cols = self.stream_cols(&kept, inner, now);
        let hover = self.hover.clone();
        let since = {
            let h = self.kept::<Hovered>();
            if h.key != hover {
                h.key = hover.clone();
                h.at = Some(Instant::now());
            }
            h.at.map_or(Duration::ZERO, |t| t.elapsed())
        };
        let mut hot = hover.starts_with(KEY_PREFIX);
        let mut out = Vec::with_capacity(kept.len() + 1);
        let title = " the feed 🐓 ";
        out.push((
            line([
                sp("──", FAINT),
                spb(title, TEXT),
                sp("─".repeat(w.saturating_sub(3 + cells(title))), FAINT),
            ]),
            KEY_PREFIX.to_string(),
        ));
        let mut tint: Option<Color> = None;
        let mut fresh_until: Option<Ms> = None;
        let mut next: Option<Duration> = None;
        for (i, p) in kept.iter().enumerate() {
            if now - p.at < FRESH {
                hot = true;
                fresh_until = Some(fresh_until.map_or(p.at + FRESH, |t: Ms| t.max(p.at + FRESH)));
            }
            // A run of one author's posts signs once, in a colour not the
            // run above's.
            let key = format!("{KEY_PREFIX}{}", p.key);
            let hovered = (hover == key).then_some(since);
            let mut who = Line::new();
            if i == 0 || kept[i - 1].username() != p.username() {
                let t = tint_beside(
                    &p.username(),
                    tint,
                    next_tint(&kept[i + 1..], &p.username()),
                );
                tint = Some(t);
                who = self.stream_who(p, t, cols.handle, hovered);
            } else if hovered.is_some() {
                // Hovered, a post in a run says whose it is too, in the
                // room its run's name has.
                who = self.stream_who(p, tint.unwrap_or(TEXT), cols.handle, hovered);
            }
            // A hovered handle that's cut scrolls: the next frame is due
            // when it next moves, and only while the pointer is on it.
            if let Some(t) = hovered {
                next = marquee(&p.whole_handle(), name_room(p, cols.handle), t).1;
            }
            let row = stream_row(p, cols, who, now);
            let row = if hover == key {
                hover_line(row, inner)
            } else {
                row
            };
            out.push((join_to(line([plain(" ")]), fit(row, inner)), key));
        }
        if !hot {
            for (l, _) in &mut out {
                fade_text(l, FADE);
            }
        }
        // Only what moves asks for a wake: a hovered handle scrolling, and
        // a new post fading once it's no longer new.
        if let Some(d) = next {
            self.ask_wake(Instant::now() + d);
        }
        if let Some(t) = fresh_until {
            self.ask_wake(Instant::now() + Duration::from_millis((t - now).max(0) as u64));
        }
        out
    }

    /// Where the dock's columns sit for posts in w cells (streamColsOf).
    fn stream_cols(&self, posts: &[Post], w: usize, now: Ms) -> Cols {
        let mut c = Cols {
            handle: 0,
            text: 0,
            age: 3,
        };
        let mut handles = 0;
        for p in posts {
            let h = cells(&p.handle());
            let mut who = h;
            let n = self.stream_name(p);
            if !n.is_empty() {
                who += cells(&n) + 3;
            }
            c.handle = c.handle.max(who);
            handles = handles.max(h);
            c.age = c.age.max(cells(&age(now - p.at)));
        }
        c.handle = c.handle.min(handles.max(w / 4)).min((w / 3).max(6));
        c.text = (w as isize - c.handle as isize - c.age as isize - 4).max(8) as usize;
        c
    }
}

/// One post on one row: who signed it, if anyone, the text cut with …,
/// and its age (streamRow). Hovered or not, every column keeps its
/// place.
fn stream_row(p: &Post, c: Cols, who: Line, now: Ms) -> Line {
    let ago = age(now - p.at);
    let when = if now - p.at < FRESH {
        sp(ago.clone(), ORANGE)
    } else {
        sp(ago.clone(), DIM)
    };
    let text = fields(&clean_text(&p.said()));
    let mut row = fit(who, c.handle);
    row.spans.push(plain("  "));
    row = join_to(row, fit(tagged(&text, SUB), c.text));
    row.spans.push(plain("  "));
    row.spans
        .push(plain(blanks(c.age.saturating_sub(cells(&ago)))));
    row.spans.push(when);
    row
}

/// l's coloured text by of the way to the ground; plain text stays
/// (fadeText).
pub fn fade_text(l: &mut Line, by: f64) {
    let ground = crate::frame::ground::DARK.bg.color();
    for s in &mut l.spans {
        if let Color::Rgb(..) = s.style.fg {
            s.style.fg = mix(s.style.fg, ground, by);
        }
    }
}

fn join_to(mut a: Line, b: Line) -> Line {
    join(&mut a, b);
    a
}

/// The dock at the foot of the Agents list: up to six posts, at most a
/// third of the body (streamDock); what rush did on its own sits under
/// them while the list keeps 12 rows (view.go:1163, messages.go newsDock).
pub fn foot(m: &mut Model, w: usize, body_h: usize) -> Vec<(Line, String)> {
    let mut rows = m.stream_lines(w, (DOCK_POSTS + 1).min(body_h / 3));
    if body_h.saturating_sub(rows.len()) >= 12 {
        rows.extend(m.news_dock(w));
    }
    rows
}

/// A click on a docked post opens the feed at it (model.go:800).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if e.kind != MouseKind::Down(Button::Left)
        || m.sheet.is_some()
        || m.session.is_some()
        || m.on_place()
    {
        return Out::Pass;
    }
    let Some(k) = (e.y as usize)
        .checked_sub(m.list_top)
        .and_then(|i| m.row_keys.get(i))
    else {
        return Out::Pass;
    };
    if k.starts_with(crate::frame::messages::NEWS_KEY_PREFIX) {
        let k = k.clone();
        return m.click_news(&k); // model.go:812
    }
    let Some(id) = k.strip_prefix(KEY_PREFIX) else {
        return Out::Pass;
    };
    let id = id.to_string();
    m.open_feed(&id)
}
