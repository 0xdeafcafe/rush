//! The feed sheet: one hashtag's posts in time order, newest at the
//! bottom, an author's run of chirps under their name once, each reply
//! set in under its post; [ ] steps through the hashtags: #all, each
//! project's, the topics busiest first, then #dm. A click on a tab or on a
//! #hashtag in a chirp opens that hashtag; g draws who's been talking to
//! whom (feedgraph.rs). It reads the stream the board's worker keeps;
//! a post is written off the UI thread too. Port of Go's community.go
//! (openCommunity :44, cursor :79, tags :100, shown :141, tab :164,
//! threaded :174, signs :199, communityRows :217, jumpDay :283, feedTabs
//! :305, body :320, post :497, key :523, mouse :582, tabAt :614) and
//! feed.go (agentEvent, routine, newsRe).

use std::collections::{HashMap, HashSet};

use regex::Regex;
use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_room::community::{self, Author, Feed};

use super::feedgraph;
use super::ink::*;
use super::stream::{FRESH, Post, Stream, next_tint};
use crate::frame::overlayclick::MouseEv;
use crate::frame::sheet::{sheet_title, text_field};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// The feed sheet's own state.
#[derive(Clone, Debug, Default)]
pub struct FeedSheet {
    /// The open hashtag tab; community::ALL is everything.
    pub tag: String,
    /// The picked post's key, so a reload can't shift it.
    pub picked: String,
    /// The first line shown.
    pub scroll: usize,
    /// The cursor rides the newest post.
    pub follow: bool,
    /// Who's been talking to whom, not the timeline (g).
    pub graph: bool,
    pub composing: bool,
    /// The thread a reply goes to; "" posts anew.
    pub reply_to: String,
    pub input: Vec<char>,
    pub pos: usize,
    pub busy: bool,
    pub problem: String,
    /// Body row → post, for clicks.
    pub rows: HashMap<usize, usize>,
    /// Body row → its text, for a click on a #hashtag.
    pub text: HashMap<usize, String>,
    /// The body row the tabs are on.
    pub tab_row: Option<usize>,
}

/// The sheet kept while it's closed, so it opens where it was.
#[derive(Default)]
pub struct Kept {
    pub sheet: Option<FeedSheet>,
}

impl Model {
    /// #feed: the sheet, or #feed on|off for all of rush (openCommunity).
    pub(crate) fn open_feed(&mut self, arg: &str) -> Out {
        let arg = arg.trim();
        if arg == "on" || arg == "off" {
            let on = arg == "on";
            self.kept::<Stream>().on = Some(on);
            self.later(
                move || {
                    let mut s = rush_state::Store::load();
                    s.config.feed = on;
                    s.save_config().map_err(|e| e.to_string())
                },
                |m, r| {
                    if let Err(e) = r {
                        m.fail(&format!("couldn't save the config: {e}"));
                    }
                    Out::Draw
                },
            );
            self.flash(&format!("the feed is {arg}"));
            if on {
                self.load_stream();
            } else {
                self.kept::<Stream>().off();
            }
            return Out::Draw;
        }
        if arg == "open" || arg == "closed" {
            self.flash("the feed is one for every project now; hashtags sort it");
            return Out::Draw;
        }
        if !self.feed_on() {
            self.flash_err("the feed is off · #feed on");
            return Out::Draw;
        }
        self.load_stream();
        let mut s = match self.kept::<Kept>().sheet.take() {
            Some(s) => s,
            None => {
                let mut s = FeedSheet {
                    tag: community::ALL.into(),
                    follow: true,
                    ..FeedSheet::default()
                };
                // The picked agent's project first.
                if let Some(i) = self.focused() {
                    let a = &self.agents[i];
                    let root = self.roots.get(i).cloned().unwrap_or_default();
                    let t = community::project_tag(crate::list::folder_key(a, &root));
                    if !t.is_empty() {
                        s.tag = t;
                    }
                }
                s
            }
        };
        if arg == "new" {
            s.composing = true;
            s.reply_to.clear();
        }
        if !arg.is_empty()
            && let Some(p) = self
                .kept::<Stream>()
                .posts
                .iter()
                .find(|p| p.key == arg || p.id == arg)
        {
            // A click on the side stream picks its post.
            s.picked = p.key.clone();
            s.follow = false;
            if !p.tags.contains(&s.tag) {
                s.tag = community::ALL.into();
            }
        }
        self.open_sheet(Box::new(s));
        Out::Draw
    }
}

fn posts(m: &Model) -> &[Post] {
    m.kept.peek::<Stream>().map_or(&[], |s| s.posts.as_slice())
}

impl FeedSheet {
    /// The picked post's place in posts: the newest when following or
    /// when the picked post has gone.
    pub fn cursor(&self, posts: &[Post]) -> Option<usize> {
        if !self.follow
            && let Some(i) = posts.iter().position(|p| p.key == self.picked)
        {
            return Some(i);
        }
        posts.len().checked_sub(1)
    }

    fn pick(&mut self, posts: &[Post], i: isize) {
        if posts.is_empty() {
            return;
        }
        let i = i.clamp(0, posts.len() as isize - 1) as usize;
        self.picked = posts[i].key.clone();
        self.follow = i == posts.len() - 1;
    }

    fn step(&mut self, posts: &[Post], by: isize) {
        let at = self.cursor(posts).map_or(-1, |c| c as isize);
        self.pick(posts, at + by);
    }

    /// The tabs: #all, each project's hashtag by name, the other hashtags
    /// busiest first, then #dm when agents have messaged.
    pub fn tags(&self, posts: &[Post]) -> Vec<String> {
        let mut n: HashMap<&str, usize> = HashMap::new();
        let mut projects: HashSet<String> = HashSet::new();
        let mut dm = false;
        for p in posts.iter().filter(|p| !p.reply) {
            for t in &p.tags {
                *n.entry(t).or_default() += 1;
            }
            let t = community::project_tag(&p.project);
            if !t.is_empty() {
                projects.insert(t);
            }
            dm = dm || !p.to.is_empty();
        }
        let mut out = vec![community::ALL.to_string()];
        let mut ps: Vec<String> = projects.iter().cloned().collect();
        ps.sort();
        out.extend(ps);
        let mut topics: Vec<&str> = n.keys().copied().collect();
        topics.sort_by(|a, b| n[b].cmp(&n[a]).then(a.cmp(b)));
        for t in topics {
            if !projects.contains(t) && t != community::DM {
                out.push(t.to_string());
            }
        }
        if dm {
            out.push(community::DM.into());
        }
        out
    }

    /// The project whose hashtag is tag, if it has chirped; "" else.
    fn project_of(&self, posts: &[Post], tag: &str) -> String {
        posts
            .iter()
            .find(|p| !p.project.is_empty() && community::project_tag(&p.project) == tag)
            .map(|p| p.project.clone())
            .unwrap_or_default()
    }

    /// The open tab's posts, threaded, without an agent's word-for-word
    /// repeat of a chirp nobody answered.
    pub fn shown(&mut self, all: &[Post]) -> Vec<Post> {
        if !self.tags(all).contains(&self.tag) {
            self.tag = community::ALL.into(); // a hashtag with no chirps
        }
        let mut answered: HashMap<&str, bool> = HashMap::new();
        for p in all {
            let a = answered.entry(&p.id).or_default();
            *a = *a || p.reply;
        }
        let mut seen: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();
        for p in all {
            let said = format!("{}\0{}\0{}", p.project, p.username(), p.said());
            let repeat = !p.reply
                && !answered.get(p.id.as_str()).copied().unwrap_or(false)
                && seen.get(&said).copied().unwrap_or(false);
            if (self.tag != community::ALL && !p.tags.contains(&self.tag)) || repeat {
                continue;
            }
            let s = seen.entry(said).or_default();
            *s = *s || !p.reply;
            out.push(p.clone());
        }
        threaded(out)
    }

    /// The next hashtag tab (dir 1) or the one before (dir -1).
    fn tab(&mut self, posts: &[Post], dir: isize) {
        let tags = self.tags(posts);
        let i = tags.iter().position(|t| *t == self.tag).unwrap_or(0) as isize;
        let n = tags.len() as isize;
        let t = tags[((i + dir + n) % n) as usize].clone();
        self.open(&t);
    }

    /// Shows hashtag tag's tab, at its newest post.
    pub fn open(&mut self, tag: &str) {
        self.tag = tag.to_lowercase();
        self.follow = true;
    }

    /// Moves to the first post of the next day (dir 1) or the one before
    /// (dir -1).
    fn jump_day(&mut self, posts: &[Post], dir: isize) {
        let Some(i) = self.cursor(posts) else { return };
        let mut i = i as isize;
        let n = posts.len() as isize;
        let d = local_day(posts[i as usize].root_at);
        while i >= 0 && i < n && local_day(posts[i as usize].root_at) == d {
            i += dir;
        }
        if i < 0 || i >= n {
            return;
        }
        let d = local_day(posts[i as usize].root_at);
        while i > 0 && local_day(posts[i as usize - 1].root_at) == d {
            i -= 1;
        }
        self.pick(posts, i);
    }
}

/// The board as the sheet reads it: posts in time order, each with its
/// replies under it.
pub fn threaded(mut posts: Vec<Post>) -> Vec<Post> {
    let roots: HashMap<String, Ms> = posts
        .iter()
        .filter(|p| !p.reply)
        .map(|p| (p.id.clone(), p.at))
        .collect();
    for p in posts.iter_mut() {
        p.root_at = roots.get(&p.id).copied().unwrap_or(p.at);
    }
    posts.sort_by(|a, b| {
        a.root_at
            .cmp(&b.root_at)
            .then_with(|| a.id.cmp(&b.id))
            .then(a.reply.cmp(&b.reply))
            .then(a.at.cmp(&b.at))
    });
    posts
}

/// Whether posts[i] heads a run of its author's chirps, under their name:
/// the day's first, or the first after another author's. Replies to a
/// chirp in the run don't break it; a reply carries its own @handle.
pub fn signs(posts: &[Post], i: usize) -> bool {
    if posts[i].reply {
        return false;
    }
    for j in (0..i).rev() {
        if local_day(posts[j].root_at) != local_day(posts[i].root_at) {
            return true;
        }
        if !posts[j].reply {
            return posts[j].username() != posts[i].username();
        }
    }
    true
}

/// The hashtag tabs as pills, the open one lit; a row too narrow for them
/// all shows the open one and where it is among them (feedTabs).
pub fn feed_tabs(names: &[String], cur: usize, w: usize) -> Line {
    let pill = |i: usize, n: &str| {
        if i == cur {
            rush_core::text::Span::new(
                format!(" {n} "),
                rush_core::text::Style {
                    fg: TAB_ON_FG,
                    bg: ORANGE,
                    mods: rush_core::text::Mods::BOLD,
                    link: rush_core::text::Link::NONE,
                },
            )
        } else {
            rush_core::text::Span::new(
                format!(" {n} "),
                rush_core::text::Style {
                    fg: SUB,
                    bg: TAB_OFF_BG,
                    mods: rush_core::text::Mods::default(),
                    link: rush_core::text::Link::NONE,
                },
            )
        }
    };
    let mut row = Line::new();
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            row.spans.push(plain(" "));
        }
        row.spans.push(pill(i, n));
    }
    if row.width() <= w {
        return row;
    }
    let mut l = Line::from_spans(vec![pill(cur, &names[cur])]);
    l.spans
        .push(sp(format!("  {} of {}", cur + 1, names.len()), FAINT));
    l
}

/// The tab feed_tabs drew at column x, when they all fit (tabAt).
pub fn tab_at(tags: &[String], x: usize) -> Option<&str> {
    let mut at = 0;
    for t in tags {
        let w = cells(t) + 2;
        if x >= at && x < at + w {
            return Some(t);
        }
        at += w + 1;
    }
    None
}

/// The rule over a day's posts.
fn day_rule(at: Ms, now: Ms, w: usize) -> Line {
    let label = day_label(at, now);
    let n = w.saturating_sub(4 + cells(&label));
    line([
        sp("── ", FAINT),
        sp(label, DIM),
        sp(format!(" {}", "─".repeat(n)), FAINT),
    ])
}

/// The width of a row of the sheet.
const MAX_W: usize = 120;

impl Model {
    /// One chirp, w wide: its age right-aligned in a column age_w wide,
    /// then every line of its text beside it, so each chirp starts on its
    /// age and its lines share one left edge. A reply sits a step deeper,
    /// on bg: set in under ↳ and its @handle (name), the handle in tint,
    /// its later lines under the handle (communityRows).
    pub(crate) fn community_rows(
        &self,
        p: &Post,
        tint: Color,
        age_w: usize,
        w: usize,
        bg: Color,
        now: Ms,
    ) -> Vec<Line> {
        let ago = age(now - p.at);
        let when = if now - p.at < FRESH {
            sp(right(&ago, age_w), ORANGE)
        } else {
            sp(right(&ago, age_w), DIM)
        };
        let mut said = p.said();
        let mut more: Vec<String> = Vec::new();
        if p.whole && !p.full.is_empty() {
            // Picked, an agent's message shows whole.
            let full = clean_text(&p.full);
            let mut paras = full.split('\n').map(str::to_string);
            let first = paras.next().unwrap_or_default();
            said = format!("{}{first}", said.strip_suffix(&p.text).unwrap_or(&said));
            more = paras.collect();
        } else if !p.full.is_empty() && !said.ends_with('…') {
            said.push_str(" …"); // there's more: picked, it shows
        }
        let mut text = fields(&clean_text(&said));
        let mut room = w as isize - age_w as isize - 2;
        let mut gap = "  ";
        let h = p.handle();
        let mut sign = String::new();
        if p.reply {
            room -= 3;
            gap = " "; // the deeper ground starts a cell early, a margin inside it
            let n = self.stream_name(p);
            if !n.is_empty() && room / 3 >= 8 {
                sign = format!(" ({})", truncate(&n, (room / 3) as usize, "…"));
            }
            text = format!("{h}{sign}  {text}");
        }
        let room = room.max(1) as usize;
        let mut wrapped = super::ink::wrap(&text, room);
        for para in more {
            let para = fields(&para);
            if para.is_empty() {
                wrapped.push(String::new());
                continue;
            }
            wrapped.extend(super::ink::wrap(&para, room));
        }
        let inner = w.saturating_sub(age_w + 1);
        wrapped
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut out = if i == 0 {
                    line([when.clone(), plain(gap)])
                } else {
                    line([plain(format!("{}{gap}", blanks(age_w)))])
                };
                let hs = format!("{h}{sign}");
                let body = match (p.reply, i, l.strip_prefix(&hs)) {
                    (true, 0, Some(rest)) => {
                        let mut b = line([plain(" "), sp("↳ ", FAINT), spb(h.clone(), tint)]);
                        if !sign.is_empty() {
                            b.spans.push(sp(sign.clone(), DIM));
                        }
                        join(&mut b, tagged(rest, TEXT));
                        on_bg(bg, b, inner)
                    }
                    (true, 0, None) => {
                        let mut b = line([plain(" "), sp("↳ ", FAINT)]);
                        join(&mut b, tagged(l, TEXT));
                        on_bg(bg, b, inner)
                    }
                    (true, _, _) => {
                        let mut b = line([plain("   ")]);
                        join(&mut b, tagged(l, TEXT));
                        on_bg(bg, b, inner)
                    }
                    _ => tagged(l, TEXT),
                };
                join(&mut out, body);
                out
            })
            .collect()
    }
}

impl FeedSheet {
    fn footer(&self, w: usize) -> Vec<Line> {
        let status = if self.busy {
            line([sp("Chirping…", DIM)])
        } else if !self.problem.is_empty() {
            fit(line([sp(clean_text(&self.problem), YELLOW)]), w)
        } else {
            Line::new()
        };
        if self.composing {
            let label = if !self.reply_to.is_empty() {
                format!("Reply · {} characters, no links", community::MAX_BODY)
            } else if self.tag != community::ALL && self.tag != community::DM {
                format!(
                    "New chirp in {} · {} characters, no links",
                    self.tag,
                    community::MAX_BODY
                )
            } else {
                format!(
                    "New chirp · {} characters, no links · a #hashtag files it there too",
                    community::MAX_BODY
                )
            };
            let mut out = vec![
                status,
                fit(line([sp(label, TEXT)]), w),
                text_field(&self.input, self.pos, true, "Write here…", w),
            ];
            out.extend(keys_controls(
                w,
                &[("enter", "Chirp"), ("esc", "Keep draft")],
            ));
            return out;
        }
        let mut out = vec![status];
        out.extend(keys_controls(
            w,
            &[
                ("↑ ↓", "Scroll"),
                ("⇧↑ ⇧↓", "Day"),
                ("[ ]", "Hashtag"),
                ("enter", "Reply"),
                ("n", "Chirp"),
                ("g", "Graph"),
                ("esc", "Close"),
            ],
        ));
        out
    }

    /// The tabs under the title, when there's more than #all.
    fn head(&mut self, all: &[Post], about: &str, w: usize) -> Vec<Line> {
        let mut out = vec![sheet_title("the feed 🐓", about, w)];
        let tags = self.tags(all);
        self.tab_row = None;
        if tags.len() > 1 {
            // #all alone needs no tab
            self.tab_row = Some(out.len() + 1);
            let cur = tags.iter().position(|t| *t == self.tag).unwrap_or(0);
            out.push(Line::new());
            out.push(feed_tabs(&tags, cur, w));
        }
        out
    }

    /// The timeline (body).
    pub fn timeline(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let all = posts(m);
        let posts = self.shown(all);
        let mut out = self.head(all, "one for every project · a #hashtag sorts it", w);
        out.push(Line::new());
        let footer = self.footer(w);
        let room = h.saturating_sub(out.len() + footer.len()).max(1);
        if posts.is_empty() {
            out.push(line([sp("No chirps yet.", TEXT)]));
            out.push(line([sp("Agents chirp with: rush feed chirp \"…\"", DIM)]));
        }
        let now = m.clock.unwrap_or_else(rush_core::time::now);
        let cursor = self.cursor(&posts);
        let age_w = posts
            .iter()
            .map(|p| cells(&age(now - p.at)))
            .max()
            .unwrap_or(0)
            .max(2);
        // Off its project's tab, a run names its project.
        let mut where_: HashMap<&str, String> = HashMap::new();
        for p in &posts {
            let t = community::project_tag(&p.project);
            if t != self.tag {
                where_.insert(&p.project, t);
            }
        }
        // Each run of an author's chirps is a card raised off the sheet,
        // edged in the author's colour, never the colour of the run before;
        // a reply sits a step deeper, the picked chirp highest.
        let (card, deeper) = (HOVER_BG, BTW_BG);
        let mut edge: Option<Color> = None;
        let mut lines: Vec<Line> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        // The line to pin over each: its run's name, or a reply's first
        // line; None for gaps and days.
        let mut run_at: Vec<Option<usize>> = Vec::new();
        let mut run: Option<usize> = None;
        let mut by = String::new();
        let (mut first, mut last) = (0, 0);
        for (i, p) in posts.iter().enumerate() {
            macro_rules! add {
                ($l:expr, $run:expr) => {{
                    let l: Line = $l;
                    lines.push(l);
                    owner.push(i);
                    run_at.push($run);
                }};
            }
            let day = i == 0 || local_day(p.root_at) != local_day(posts[i - 1].root_at);
            if i > 0 && (day || signs(&posts, i)) {
                run = None;
                add!(Line::new(), run); // one blank line between runs, and before each day
            }
            if Some(i) == cursor {
                first = lines.len();
            }
            if day {
                add!(day_rule(p.root_at, now, w), run);
            }
            let user = p.username();
            if (signs(&posts, i) || i == 0) && user != by {
                // Over a new day, the same author keeps their colour.
                edge = Some(tint_beside(&user, edge, next_tint(&posts[i + 1..], &user)));
                by = user.clone();
            }
            let e = edge.unwrap_or(BLUE);
            // A reply by another wears its own colour, never the run's.
            let tint = if user != by {
                tint_beside(&user, edge, None)
            } else {
                e
            };
            if signs(&posts, i) {
                let proj = where_.get(p.project.as_str()).cloned().unwrap_or_default();
                let mut room = w.saturating_sub(2);
                if !proj.is_empty() {
                    room = room.saturating_sub(cells(&proj) + 2);
                }
                run = Some(lines.len());
                let mut head = line([sp("▎", e), plain(" ")]);
                join(
                    &mut head,
                    spread(
                        m.stream_who(p, e, room),
                        line([sp(proj, DIM)]),
                        w.saturating_sub(2),
                    ),
                );
                add!(on_bg(card, head, w), run);
            }
            let (bg, bar, inner) = if Some(i) == cursor {
                (SEL_BG, sp("▍", ORANGE), SEL_BG)
            } else {
                (card, sp("▎", e), deeper)
            };
            // A reply's later lines pin its own first, with its handle, not
            // the run's name.
            let head = run;
            let mut p = p.clone();
            p.whole = Some(i) == cursor;
            for (j, l) in m
                .community_rows(&p, tint, age_w, w.saturating_sub(2), inner, now)
                .into_iter()
                .enumerate()
            {
                let mut row = line([bar.clone(), plain(" ")]);
                join(&mut row, l);
                add!(on_bg(bg, row, w), run);
                if p.reply && j == 0 {
                    run = Some(lines.len() - 1);
                }
            }
            run = head;
            if Some(i) == cursor {
                last = lines.len();
            }
        }
        if first < self.scroll {
            self.scroll = first;
        }
        if last > self.scroll + room {
            self.scroll = first.min(last - room); // a long message shows from its start
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        self.rows.clear();
        self.text.clear();
        let mut shown: Vec<usize> = (self.scroll..lines.len().min(self.scroll + room)).collect();
        // Scrolled, the view never opens on a gap under the tabs' own: a
        // gap before a run shows the run's day, one before a day the line
        // above it. Partway down a run, the run's name is pinned in the
        // tabs' gap; partway down a reply, its first line.
        let t = self.scroll;
        if t > 0 && t < lines.len() && lines[t].width() == 0 {
            if t + 1 < lines.len() && run_at[t + 1] == Some(t + 1) {
                lines[t] = day_rule(posts[owner[t + 1]].root_at, now, w);
            } else if let Some(s) = shown.first_mut() {
                *s = t - 1;
            }
        }
        if let Some(&top) = shown.first()
            && let Some(r) = run_at[top]
            && r < top
        {
            let at = out.len() - 1;
            out[at] = lines[r].clone();
            self.rows.insert(at, owner[r]);
        }
        for j in shown {
            self.rows.insert(out.len(), owner[j]);
            self.text.insert(out.len(), lines[j].plain());
            out.push(lines[j].clone());
        }
        while out.len() + footer.len() < h {
            out.push(Line::new());
        }
        out.extend(footer);
        out.into_iter().map(|l| fit(l, w)).collect()
    }

    /// Posts what's in the box: a reply, or a new chirp (post).
    fn post(&mut self, m: &mut Model) -> Out {
        let text = self.input.iter().collect::<String>().trim().to_string();
        if self.busy || text.is_empty() {
            return Out::Idle;
        }
        let you = Author {
            name: "You".into(),
            project: self.project_of(posts(m), &self.tag),
            ..Author::default()
        };
        let id = self.reply_to.clone();
        self.busy = true;
        m.later(
            move || {
                let f = Feed::new();
                if !id.is_empty() {
                    return f.reply(&id, &you, &text).map(|_| ());
                }
                let title = text.split('\n').next().unwrap_or("").to_string();
                f.ask(&you, &title, &text).map(|_| ())
            },
            |m, r| {
                let Some(s) = m.sheet_as::<FeedSheet>() else {
                    return Out::Idle;
                };
                s.busy = false;
                match r {
                    Err(e) => s.problem = e.to_string(),
                    Ok(()) => {
                        s.problem.clear();
                        s.composing = false;
                        s.reply_to.clear();
                        s.input.clear();
                        s.pos = 0;
                        s.follow = true;
                        m.reload_stream(); // the board's next read has the post
                    }
                }
                Out::Draw
            },
        );
        Out::Draw
    }

    fn put_away(self, m: &mut Model) -> Out {
        m.kept::<Kept>().sheet = Some(self);
        Out::Close
    }
}

/// The word around cell x of text, as cells from..to (wordCells).
fn word_cells(text: &str, x: usize) -> Option<(usize, usize, String)> {
    let mut at = 0;
    let mut start = None;
    let mut word = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        let cw = cells(c.encode_utf8(&mut [0; 4]));
        if c == ' ' {
            if let Some(s) = start
                && x >= s
                && x < at
            {
                return Some((s, at, word));
            }
            start = None;
            word.clear();
        } else {
            if start.is_none() {
                start = Some(at);
            }
            word.push(c);
        }
        at += cw;
    }
    None
}

impl Sheet for FeedSheet {
    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        if self.graph {
            return feedgraph::graph_body(self, m, w, h);
        }
        self.timeline(m, w, h)
    }

    fn width(&self, m: &Model) -> usize {
        MAX_W.min(m.w.saturating_sub(6))
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let all: Vec<Post> = posts(m).to_vec();
        let shown = self.shown(&all);
        if self.composing {
            match k.s.as_str() {
                "esc" => self.composing = false,
                "enter" => return self.post(m),
                _ => {
                    if !self.busy
                        && let Some((buf, pos)) =
                            crate::frame::editor::edit(&self.input, self.pos, k)
                        && buf.len() <= community::MAX_BODY
                    {
                        self.input = buf;
                        self.pos = pos;
                    }
                }
            }
            return Out::Draw;
        }
        match k.s.as_str() {
            "esc" => return std::mem::take(self).put_away(m),
            "up" => self.step(&shown, -1),
            "down" => self.step(&shown, 1),
            "pgup" => self.step(&shown, -10),
            "pgdown" => self.step(&shown, 10),
            "shift+up" => self.jump_day(&shown, -1),
            "shift+down" => self.jump_day(&shown, 1),
            "[" | "shift+tab" => self.tab(&all, -1),
            "]" | "tab" => self.tab(&all, 1),
            "g" => self.graph = !self.graph,
            "n" => {
                self.composing = true;
                self.reply_to.clear();
                if self.tag != community::ALL
                    && self.tag != community::DM
                    && self.project_of(&all, &self.tag).is_empty()
                    && self.input.is_empty()
                {
                    // A chirp in a topic's tab names it.
                    self.input = format!("{} ", self.tag).chars().collect();
                    self.pos = self.input.len();
                }
            }
            "enter" | "space" | " " | "r" => {
                if let Some(i) = self.cursor(&shown) {
                    self.composing = true;
                    self.reply_to = shown[i].id.clone();
                }
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn mouse(&mut self, m: &mut Model, ev: MouseEv, x: i32, y: i32) -> Out {
        if self.composing {
            return Out::Idle;
        }
        let all: Vec<Post> = posts(m).to_vec();
        let shown = self.shown(&all);
        match ev {
            MouseEv::WheelUp => self.step(&shown, -1),
            MouseEv::WheelDown => self.step(&shown, 1),
            MouseEv::Press if x >= 0 && y >= 0 => {
                let (x, y) = (x as usize, y as usize);
                if Some(y) == self.tab_row {
                    if let Some(t) = tab_at(&self.tags(&all), x).map(str::to_string) {
                        self.open(&t);
                    }
                    return Out::Draw;
                }
                if let Some(t) = self.text.get(&y)
                    && let Some((_, _, word)) = word_cells(t, x)
                {
                    let tags = community::hashtags(&word);
                    if tags.len() == 1 {
                        self.open(&tags[0]);
                        return Out::Draw;
                    }
                }
                if let Some(&i) = self.rows.get(&y) {
                    self.pick(&shown, i as isize);
                }
            }
            MouseEv::Press => return Out::Pass,
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn paste(&mut self, _m: &mut Model, text: &str) -> Out {
        let r: Vec<char> = clean_text(text).chars().collect();
        if self.composing && !self.busy && self.input.len() + r.len() <= community::MAX_BODY {
            self.input = crate::frame::editor::insert(&self.input, self.pos, &r);
            self.pos += r.len();
        }
        Out::Draw
    }
}

/// A line in the from-your-agents box: an agent finished, asked, or
/// failed (feed.go agentEvent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentEvent {
    pub at: Ms,
    pub key: String,
    pub name: String,
    pub glyph: String,
    pub what: String,
    /// Its last words then, for telling news from routine.
    pub said: String,
}

/// An agent's last words saying something went badly wrong or it found
/// something big: what lifts a finish out of the routine (newsRe).
pub fn news_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(broke|broken|lost|wiped|corrupt\w*|data loss|catastroph\w*|disaster|irrecoverabl\w*|can'?t recover|force[- ]push\w*|wtf|what the|root cause|found it|breakthrough|turns out|figured (it )?out|discover\w*|huge)\b")
            .expect("newsRe compiles")
    })
}

impl AgentEvent {
    /// Only an agent starting and stopping.
    pub fn routine(&self) -> bool {
        self.glyph == "✓" && !news_re().is_match(&self.said)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::frame::Press;

    pub fn model(w: usize, h: usize) -> Model {
        // rush's folder somewhere with no board: never the real one.
        rush_state::set_thread_dir(Some(std::path::PathBuf::from(
            "/nonexistent/rush-test-home",
        )));
        let mut m = Model::new("test");
        m.w = w;
        m.h = h;
        m.clock = Some(1_790_000_000_000);
        m.kept::<Stream>().on = Some(true);
        m
    }

    fn author(name: &str) -> Author {
        Author {
            name: name.into(),
            ..Author::default()
        }
    }

    pub fn feed_model() -> Model {
        let mut m = model(100, 35);
        let now = m.clock.unwrap();
        m.kept::<Stream>().posts = vec![
            Post {
                key: "a/0".into(),
                id: "a".into(),
                title: "go test hangs on fswait".into(),
                author: author("Worker"),
                at: now - 60_000,
                ..Post::default()
            },
            Post {
                key: "b/0".into(),
                id: "b".into(),
                title: "flaky lint".into(),
                author: author("Other"),
                at: now - 30_000,
                ..Post::default()
            },
            Post {
                key: "a/1".into(),
                id: "a".into(),
                title: "go test hangs on fswait".into(),
                author: Author {
                    session_id: "s1".into(),
                    ..Author::default()
                },
                text: "-race and a 5s timeout found it".into(),
                at: now,
                reply: true,
                ..Post::default()
            },
        ];
        m
    }

    fn sheet(m: &mut Model) -> &mut FeedSheet {
        m.sheet_as::<FeedSheet>().expect("the feed is open")
    }

    fn body(m: &mut Model, w: usize, h: usize) -> String {
        let mut s = m.sheet.take().expect("a sheet");
        let out = s.body(m, w, h);
        m.sheet = Some(s);
        out.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
    }

    fn press(m: &mut Model, k: &str) {
        let mut s = m.sheet.take().expect("a sheet");
        let out = s.key(m, &Press::of(k).expect("a key"));
        if out != Out::Close && m.sheet.is_none() {
            m.sheet = Some(s);
        }
    }

    #[test]
    fn switches() {
        let mut m = feed_model();
        m.kept::<Stream>().on = Some(false);
        m.open_feed("");
        assert!(m.sheet.is_none(), "the sheet opened while the feed is off");
        m.kept::<Stream>().on = Some(true);
        m.open_feed("");
        assert!(m.sheet.is_some(), "the sheet did not open once on");
    }

    #[test]
    fn timeline_is_one_view() {
        let mut m = feed_model();
        m.open_feed("");
        let text = body(&mut m, 90, 30);
        let (first, reply, other) = (
            text.find("go test hangs").unwrap(),
            text.find("↳ @").unwrap(),
            text.find("flaky lint").unwrap(),
        );
        assert!(
            first < reply && reply < other && text.contains("-race and a 5s"),
            "{text}"
        );
        let posts = threaded(m.kept::<Stream>().posts.clone());
        assert_eq!(sheet(&mut m).cursor(&posts), Some(2));
        for key in ["enter", "space"] {
            let s = sheet(&mut m);
            s.composing = false;
            s.picked = "a/1".into();
            s.follow = false;
            press(&mut m, "up");
            press(&mut m, key);
            let s = sheet(&mut m);
            assert!(
                s.composing && s.reply_to == "a",
                "{key} replies to the picked post"
            );
        }
        m.paste("board draft");
        assert_eq!(
            sheet(&mut m).input.iter().collect::<String>(),
            "board draft"
        );
        press(&mut m, "esc");
        press(&mut m, "esc");
        assert!(m.sheet.is_none(), "esc closes the sheet");
    }

    #[test]
    fn renders_within_bounds() {
        for (w, h) in [(44, 24), (140, 45)] {
            let mut m = feed_model();
            m.kept::<Stream>().posts[0].title = format!("question\x1b[2J{}", " long".repeat(60));
            m.open_feed("new");
            let mut s = m.sheet.take().unwrap();
            let lines = s.body(&m, w - 10, h - 6);
            assert!(lines.len() <= h - 6, "{w}x{h} clips controls");
            for l in &lines {
                assert!(
                    l.width() <= w - 10 && !l.plain().contains('\x1b'),
                    "bad line {:?}",
                    l.plain()
                );
            }
        }
    }

    #[test]
    fn day_separators_and_jump() {
        let mut m = feed_model();
        let now = m.clock.unwrap();
        m.kept::<Stream>().posts[0].at = now - 3 * 86_400_000;
        m.kept::<Stream>().posts[1].at = now - 86_400_000;
        m.open_feed("");
        let text = body(&mut m, 90, 30);
        let old = format!("── {} ──", day_label(now - 3 * 86_400_000, now));
        let (a, b) = (
            text.find(&old).unwrap(),
            text.find("── Yesterday ──").unwrap(),
        );
        assert!(a < b && !text.contains("── Today"), "{text}");
        assert!(text.contains("⇧↑ ⇧↓ Day"), "{text}");
        for (key, want) in [
            ("shift+up", "a/0"),
            ("shift+up", "a/0"),
            ("shift+down", "b/0"),
            ("shift+down", "b/0"),
            ("up", "a/1"),
            ("shift+down", "b/0"),
        ] {
            press(&mut m, key);
            assert_eq!(sheet(&mut m).picked, want, "{key}");
        }
    }

    #[test]
    fn picks_and_wraps() {
        let mut m = feed_model();
        m.kept::<Stream>().posts[1].title =
            format!("{}the end", "a long flaky lint story ".repeat(12));
        m.open_feed("b/0");
        let text = body(&mut m, 90, 40);
        assert!(
            text.lines()
                .any(|l| l.starts_with('▍') && l.contains("the end")),
            "the picked post marked and wrapped in full:\n{text}"
        );
        assert!(text.starts_with("the feed 🐓"), "{text}");
    }

    #[test]
    fn sorts_by_hashtag() {
        let mut m = feed_model();
        let now = m.clock.unwrap();
        {
            let ps = &mut m.kept::<Stream>().posts;
            for (i, p) in ["/src/rush", "/src/haven", "/src/rush"].iter().enumerate() {
                ps[i].project = p.to_string();
                ps[i].tags = vec![community::project_tag(p)];
            }
            ps[1].tags.push("#build".into());
            ps.push(Post {
                key: "c/0".into(),
                id: "c".into(),
                project: "/src/haven".into(),
                tags: vec!["#haven".into()],
                title: "flaky lint".into(),
                author: author("Other"),
                at: now,
                ..Post::default()
            }); // a lone repeat, hidden
        }
        m.open_feed("");
        sheet(&mut m).open("#rush");
        let text = body(&mut m, 90, 30);
        assert!(
            text.contains(" #all   #haven   #rush   #build ")
                && text.contains("go test hangs")
                && !text.contains("flaky lint"),
            "the #rush tab shows only rush's chirps, under its tabs:\n{text}"
        );
        press(&mut m, "]");
        let text = body(&mut m, 90, 30);
        assert!(
            sheet(&mut m).tag == "#build"
                && !text.contains("go test hangs")
                && text.matches("flaky lint").count() == 1,
            "{text}"
        );
        let row = sheet(&mut m).tab_row.unwrap() as i32;
        let mut s = m.sheet.take().unwrap();
        s.mouse(&mut m, MouseEv::Press, 2, row);
        m.sheet = Some(s);
        assert_eq!(
            sheet(&mut m).tag,
            community::ALL,
            "a click on #all opens it"
        );
        press(&mut m, "[");
        press(&mut m, "n");
        assert_eq!(sheet(&mut m).input.iter().collect::<String>(), "#build ");
    }

    // An author's run of chirps is signed once, @handle then their name;
    // another author or a new day signs again, and a reply carries its own
    // @handle.
    #[test]
    fn signs_each_run_once() {
        let mut m = feed_model();
        let now = m.clock.unwrap();
        let hub = Author {
            session_id: "s-hub".into(),
            name: "Hub Header Sims Integration".into(),
            ..Author::default()
        };
        let mk = |k: &str, title: &str, a: &Author, mins: i64, reply: bool| Post {
            key: k.into(),
            id: k[..1].into(),
            title: title.into(),
            text: if reply { title.into() } else { String::new() },
            author: a.clone(),
            at: now - mins * 60_000,
            reply,
            ..Post::default()
        };
        m.kept::<Stream>().posts = vec![
            mk("a/0", "first tip", &hub, 5, false),
            mk("b/0", "second tip", &hub, 4, false),
            mk("b/1", "thanks", &author("You"), 3, true),
            mk("c/0", "third tip", &hub, 2, false),
            mk("d/0", "a word from another", &author("Other"), 1, false),
            mk("e/0", "back again", &hub, 0, false),
        ];
        m.open_feed("");
        let text = body(&mut m, 90, 40);
        let handle = m.kept::<Stream>().posts[0].handle();
        let sign = format!("▎ {handle} (Hub Header Sims Integration)");
        assert_eq!(text.matches(&sign).count(), 2, "{text}");
        assert_eq!(text.matches("@other").count(), 1, "{text}");
        assert!(text.contains("↳ @you  thanks"), "{text}");
        assert!(
            text.find("second tip") < text.find("thanks")
                && text.find("thanks") < text.find("third tip")
        );
        let rows: Vec<&str> = text.lines().collect();
        let from = rows.iter().position(|l| l.starts_with("── ")).unwrap();
        let gaps: Vec<usize> = rows[from..]
            .iter()
            .take_while(|l| !l.contains("back again"))
            .enumerate()
            .filter(|(_, l)| l.trim().is_empty())
            .map(|(i, _)| i)
            .collect();
        assert!(
            gaps.len() == 2 && gaps[1] - gaps[0] == 3,
            "{gaps:?}\n{text}"
        );
    }

    // A chirp's wrapped lines share its first line's left edge; a reply's
    // sit under its @handle. None runs past the row.
    #[test]
    fn rows_wrap_aligned() {
        let m = model(100, 30);
        let now = m.clock.unwrap();
        let long = "words that wrap ".repeat(8);
        for p in [
            Post {
                title: long.clone(),
                author: author("Worker"),
                at: now,
                ..Post::default()
            },
            Post {
                text: long.clone(),
                author: author("You"),
                at: now,
                reply: true,
                ..Post::default()
            },
        ] {
            let rows = m.community_rows(&p, BLUE, 3, 50, SEL_BG, now);
            let first = rows[0].plain();
            let edge = if p.reply {
                cells(&first[..first.find('@').unwrap()])
            } else {
                cells(&first[..first.find("words").unwrap()])
            };
            for r in &rows[1..] {
                let r = r.plain();
                let lead = cells(&r) - cells(r.trim_start_matches(' '));
                assert!(lead == edge && cells(&r) <= 50, "{edge} {r:?}");
            }
            assert!(rows.len() >= 3);
        }
    }

    // An agent's message to another says who to; it shows whole once
    // picked.
    #[test]
    fn shows_an_agents_whole_message_when_picked() {
        let m = model(100, 30);
        let now = m.clock.unwrap();
        let mut p = Post {
            text: "Asks for the tests".into(),
            full: "Run the tests.\n\nThen the linter.".into(),
            to: "@pepper-bean".into(),
            author: author("Worker"),
            at: now,
            ..Post::default()
        };
        let plain = |m: &Model, p: &Post| {
            m.community_rows(p, BLUE, 3, 60, SEL_BG, now)
                .iter()
                .map(Line::plain)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let shut = plain(&m, &p);
        assert!(
            shut.contains("→ @pepper-bean Asks for the tests …") && !shut.contains("linter"),
            "{shut}"
        );
        p.whole = true;
        let open = plain(&m, &p);
        assert!(
            open.contains("→ @pepper-bean Run the tests.")
                && open.contains("Then the linter.")
                && !open.contains("Asks for"),
            "{open}"
        );
    }

    /// Cells from..to of a row, as text.
    fn cols(l: &Line, from: usize, to: usize) -> String {
        crate::frame::dialog::cells(l, from, to).plain()
    }

    // The sheet in a frame 120×40, as Go's `rush --render 120x40 text=#feed
    // enter` draws it with an empty board: the box 114 wide from column 3,
    // its title, the empty board's words and its keys where Go has them.
    #[test]
    fn frame_at_go_s_width() {
        let mut m = model(120, 40);
        m.open_feed("");
        let f = m.frame();
        assert_eq!(f.len(), 40);
        let inner = |s: &str| format!("│ {} │", fit_str(s, 110));
        assert_eq!(cols(&f[1], 3, 117), format!("╭{}╮", "─".repeat(112)));
        assert_eq!(cols(&f[2], 3, 117), inner(""));
        assert_eq!(
            cols(&f[3], 3, 117),
            inner("the feed 🐓  one for every project · a #hashtag sorts it")
        );
        assert_eq!(cols(&f[5], 3, 117), inner("No chirps yet."));
        assert_eq!(
            cols(&f[6], 3, 117),
            inner("Agents chirp with: rush feed chirp \"…\"")
        );
        assert_eq!(
            cols(&f[36], 3, 117),
            inner(
                "↑ ↓ Scroll · ⇧↑ ⇧↓ Day · [ ] Hashtag · enter Reply · n Chirp · g Graph · esc Close"
            )
        );
        assert_eq!(cols(&f[38], 3, 117), format!("╰{}╯", "─".repeat(112)));
    }

    // Opened at a post, it's picked; the pick follows its post when the
    // board shifts.
    #[test]
    fn opens_at_a_post() {
        let mut m = feed_model();
        m.open_feed("b/0");
        let posts = threaded(m.kept::<Stream>().posts.clone());
        assert_eq!(sheet(&mut m).cursor(&posts), Some(2));
        m.kept::<Stream>().posts.insert(
            0,
            Post {
                key: "z/0".into(),
                id: "z".into(),
                at: 1_790_000_000_000 - 3_600_000,
                ..Post::default()
            },
        );
        let posts = threaded(m.kept::<Stream>().posts.clone());
        let s = sheet(&mut m);
        assert!(s.picked == "b/0" && s.cursor(&posts) == Some(3));
    }

    // Scrolled to the newest, the newest sits on the footer and the view
    // opens right under the tabs' gap on a name or a day: partway down a
    // run, its name is pinned in that gap.
    #[test]
    fn view_pins_its_runs_name() {
        let mut m = feed_model();
        let now = m.clock.unwrap();
        let mut ps = Vec::new();
        for i in 0..14i64 {
            let id = ((b'a' + i as u8) as char).to_string();
            ps.push(Post {
                key: format!("{id}/0"),
                id: id.clone(),
                title: format!("{id} {}", "a long chirp that wraps ".repeat(4)),
                author: author(["Ann", "Bo"][(i / 3 % 2) as usize]),
                at: now + (i - 14) * 60_000,
                ..Post::default()
            });
            if i % 4 == 1 {
                ps.push(Post {
                    key: format!("{id}/1"),
                    id,
                    text: "a long reply that wraps ".repeat(5),
                    author: author("Cy"),
                    at: now + (i - 14) * 60_000 + 1000,
                    reply: true,
                    ..Post::default()
                });
            }
        }
        m.kept::<Stream>().posts = ps;
        m.open_feed("");
        let all = m.kept::<Stream>().posts.clone();
        let posts = sheet(&mut m).shown(&all);
        let name = Regex::new(r"^(▎ @|── )").unwrap();
        let pin = Regex::new(r"^▎ (@|\s*\d+[smhd]\s+↳ @)").unwrap();
        let mut pinned = 0;
        for h in 14..24 {
            let mut s = m.sheet.take().unwrap();
            let lines = s.body(&m, 60, h);
            m.sheet = Some(s);
            let (gap, top) = (lines[1].plain(), lines[2].plain());
            let view: String = lines.iter().map(|l| l.plain() + "\n").collect();
            if gap == " ".repeat(60) {
                assert!(
                    name.is_match(&top),
                    "at {h} tall the view opens partway down a run without its name:\n{view}"
                );
            } else {
                pinned += 1;
                let s = sheet(&mut m);
                let (p, mut t) = (s.rows[&1], s.rows[&2]);
                while t > 0 && posts[t].reply && posts[t].key != posts[p].key {
                    t -= 1; // a reply's first line pins its run's name
                }
                assert!(
                    pin.is_match(&gap) && posts[p].author == posts[t].author,
                    "at {h} tall the run's own name should be pinned under the tabs:\n{view}"
                );
            }
            let foot = lines
                .iter()
                .position(|l| l.plain().starts_with("↑ ↓"))
                .unwrap();
            let last = lines[foot - 2].plain();
            assert!(
                last.contains("wraps") && last.starts_with('▍'),
                "at {h} tall the newest sits on the footer:\n{view}"
            );
        }
        assert!(
            pinned > 0,
            "some height opens partway down a run, its name pinned"
        );
    }

    #[test]
    fn routine_or_news() {
        let e = |said: &str| AgentEvent {
            glyph: "✓".into(),
            said: said.into(),
            ..AgentEvent::default()
        };
        assert!(e("all tests pass").routine());
        assert!(!e("turns out the cache was stale").routine());
        assert!(!e("Found it: a race").routine());
    }
}
