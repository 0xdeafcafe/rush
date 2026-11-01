//! The header: clanker, the counts, where new agents start, the places
//! and the pages under them. After Go's internal/ui/view.go (tally :74,
//! mood :99, headH :128, underHead :146, header :165, the places hint's
//! keys :240, withTabHint :314,
//! pagesWidth :361, tabs :395, narrowHeader :412, shortPath :450,
//! nextKey :855; newsHead's place :192). The places' tabs and the pages
//! row are the frame's (frame/places.rs); the top bar down its right is
//! Ext::top_bar's (screens/bars, after view.go:165-305 header and
//! topRights).

use rush_core::text::{Line, Span, Style, width};
use rush_fleet::Predicates;

use crate::clanker::{self, Mood};
use crate::model::Model;
use crate::style::*;

/// Below this the header drops clanker's body for his face.
pub const NARROW: usize = 60;

/// The row of clanker's head: the text's first row is level with it.
pub const TEXT_AT: usize = 1;

#[derive(Default, Clone, Copy)]
pub struct Tally {
    pub blocked: usize,
    pub working: usize,
    pub busy: usize,
    pub done: usize,
    pub today: f64,
}

impl Model {
    pub fn tally(&self) -> Tally {
        let mut t = Tally::default();
        // A hosted view counts every agent, not only its one (tally).
        for a in self.ext.calls.all_agents()(self) {
            if a.needs_you() {
                t.blocked += 1;
            } else if a.job.live() {
                t.working += 1;
            } else if a.busy() {
                t.busy += 1;
            } else {
                t.done += 1;
            }
            t.today += a.spend.today;
        }
        t
    }

    /// clanker's reading of the fleet.
    pub fn mood(&self, t: Tally) -> Mood {
        if t.blocked > 0 {
            Mood::NeedsYou
        } else if t.working > 0 {
            Mood::Working
        } else if !(7..23).contains(&crate::model::local_hour(self.now)) {
            Mood::Sleepy
        } else {
            Mood::Idle
        }
    }

    /// clanker as the header draws him: his mood, his second, a dear day
    /// and what he's reacting to (clkState).
    pub fn robot(&self, md: Mood, t: Tally) -> Vec<Line> {
        clanker::draw_fx(md, self.tick, t.today >= 500.0, clanker::reaction(self))
    }

    /// The rows above the body: the header and a rule. With clanker the
    /// header is as tall as he is and the two rows of text sit beside his
    /// head and body (`TEXT_AT`); without him it is the two rows. The small
    /// screens keep the taller one, its places and pages a row each.
    pub fn top_h(&self) -> usize {
        if self.w < NARROW {
            5
        } else if self.cfg.hide_logo {
            3
        } else {
            clanker::H + 1
        }
    }

    /// The header row the first of the two rows of text is on: level with
    /// clanker's head, the second beside his body.
    pub fn text_at(&self) -> usize {
        if self.w < NARROW || self.cfg.hide_logo {
            0
        } else {
            TEXT_AT
        }
    }

    /// The counts along the header's left: what needs you, what is
    /// working, what runs in the background and what the guard holds. With
    /// `short` the words go and the glyphs keep the meaning.
    fn counts(&self, t: Tally, short: bool) -> Vec<Line> {
        let mut c = Vec::new();
        if t.blocked > 0 {
            let say = if short { "needs" } else { "needs you" };
            c.push(line([spb(format!("● {} {say}", t.blocked), YELLOW)]));
        }
        if t.working > 0 {
            let say = if short { "" } else { " working" };
            c.push(line([sp(format!("✻ {}{say}", t.working), ORANGE)]));
        }
        if t.busy > 0 {
            let say = if short { "" } else { " background" };
            c.push(line([sp(format!("◌ {}{say}", t.busy), SUB)]));
        }
        let (held, _) = self.ext.calls.guard_state()(self);
        if held > 0 {
            let say = if short { "" } else { " held" };
            c.push(line([sp(format!("‖ {held}{say}"), YELLOW)]));
        }
        c
    }

    /// What rush did on its own, unseen: a small badge (messages.go
    /// newsHead), the log is #log's.
    fn news_badge(&self) -> Option<Line> {
        let n = self.unseen_news().len();
        (n > 0).then(|| line([sp(format!("◇ {n}"), SUB)]))
    }

    fn tabs(&self) -> Vec<Line> {
        self.place_tabs()
    }

    fn acct(&self) -> Line {
        if let Some(tag) = self.ext.start_tag {
            return tag(self);
        }
        line([sp("✻", ORANGE), plain(" "), sp(self.account.clone(), DIM)])
    }

    /// The header's two rows, w wide: the counts and the account along the
    /// first, the places, the pages and the machine along the second, with
    /// clanker's face and feet down the left. Also the places' and the
    /// pages' words as drawn, for clicks (blanks where the other is), and
    /// what the top bar had no room for.
    pub fn head_rows(&self, w: usize) -> (Vec<Line>, [String; 2], Drops) {
        let t = self.tally();
        let md = self.mood(t);
        let (face, gap) = if self.cfg.hide_logo {
            (vec![Line::new(); 2], "")
        } else {
            let r = self.robot(md, t);
            (vec![r[TEXT_AT].clone(), r[TEXT_AT + 1].clone()], "   ")
        };
        let lead = |i: usize| {
            let mut l = line([plain("  ")]);
            join(&mut l, face[i].clone());
            l.spans.push(plain(gap));
            l
        };
        let tb = self.ext.top_bar;
        let news = self.news_badge();
        let gauges = tb.and_then(|tb| (tb.gauges)(self));
        let mut drops = Drops::new();

        // The first row's left: its counts, longest words first.
        let left0 = |short: bool| {
            let mut l = lead(0);
            l.spans
                .push(Span::new("rush", Style::fg(TEXT).bold().italic()));
            for c in self.counts(t, short) {
                l.spans.push(plain("   "));
                join(&mut l, c);
            }
            l
        };
        // The second row's left: the places, then the pages.
        let mut tabs = lead(1);
        let mut blank = tabs.width();
        for (i, tb) in self.tabs().into_iter().enumerate() {
            if i > 0 {
                tabs.spans.push(plain(" "));
                blank += 1;
            }
            blank += tb.width();
            join(&mut tabs, tb);
        }
        let tabs_w = tabs.width();
        let pages_at = |room: usize| self.pages_line(room);
        let full_pages = pages_at(1 << 16);
        let side = |l: &Line, p: &Line| {
            let mut l = l.clone();
            if p.width() > 0 {
                l.spans.push(plain("   "));
                join(&mut l, p.clone());
            }
            l
        };
        let join_dots = |ps: &[Line]| {
            let mut out = Line::new();
            for p in ps.iter().filter(|p| p.width() > 0) {
                if out.width() > 0 {
                    out.spans.push(sp(" · ", FAINT));
                }
                join(&mut out, p.clone());
            }
            out
        };

        let (mut right0, mut right1) = (Vec::new(), Vec::new());
        let custom;
        if let Some(g) = &gauges {
            // Dropped from the end: the pace, then the account, then the
            // badge; the machine's readings from the right.
            let mut sets: Vec<Line> = vec![join_dots(&[
                g.account.clone(),
                g.pace.clone(),
                news.clone().unwrap_or_default(),
            ])];
            if g.pace.width() > 0 {
                sets.push(join_dots(&[
                    g.account.clone(),
                    news.clone().unwrap_or_default(),
                ]));
            }
            if g.account.width() > 0 {
                sets.push(join_dots(&[news.clone().unwrap_or_default()]));
            }
            right0 = sets;
            right1 = (0..=g.machine.len())
                .rev()
                .map(|k| join_dots(&g.machine[..k]))
                .collect();
            custom = false;
        } else {
            custom = true;
        }
        // Right-aligned, a gap of at least three between.
        let fits = |left: &Line, r: &Line| {
            left.width() + if r.width() > 0 { r.width() + 5 } else { 0 } <= w
        };
        let place = |left: Line, r: Line| spread(left, r, w);
        let costing = !self.loaded && !crate::listcache::showing(self);

        let row0;
        let row1;
        if custom {
            // A top bar of your own is drawn as you built it, a line each.
            let l0 = left0(false);
            let l1 = side(&tabs, &full_pages);
            // While transcripts are costed the second row says so first.
            let say = if tb.is_some() && costing { 23 } else { 0 };
            let rooms = [
                w.saturating_sub(l0.width() + 4),
                w.saturating_sub(l1.width() + 4 + say),
            ];
            let mut rt = match tb {
                Some(tb) => (tb.rights)(self, &rooms, &mut drops),
                None => vec![line([sp(money(t.today), DIM), sp(" today", DIM)])],
            };
            rt.resize(2, Line::new());
            if tb.is_some() && costing {
                let mut c = line([sp("costing transcripts…   ", DIM)]);
                join(&mut c, std::mem::take(&mut rt[1]));
                rt[1] = c;
            }
            row0 = spread(l0, rt[0].clone(), w);
            row1 = spread(l1, rt[1].clone(), w);
        } else {
            let mut pick = None;
            'words: for short in [false, true] {
                let l = left0(short);
                for r in &right0 {
                    if fits(&l, r) {
                        pick = Some(place(l, r.clone()));
                        break 'words;
                    }
                }
            }
            row0 = pick.unwrap_or_else(|| fit(left0(true), w));
            let mut pick = None;
            for r in &mut right1 {
                if costing {
                    let mut c = line([sp("costing transcripts…   ", DIM)]);
                    join(&mut c, std::mem::take(r));
                    *r = c;
                }
                let l = side(&tabs, &full_pages);
                if pick.is_none() && fits(&l, r) {
                    pick = Some(place(l, r.clone()));
                }
            }
            row1 = pick.unwrap_or_else(|| {
                // The pages shorten once nothing of the right is left,
                // and go when even that doesn't fit ([ ] still turn them).
                let room = w.saturating_sub(tabs_w + 5);
                let p = pages_at(room + 3);
                let p = if p.width() > room { Line::new() } else { p };
                fit(side(&tabs, &p), w)
            });
        }
        let plain1 = row1.plain();
        // Where the pages' words are, the places' words are blanks.
        let mut pages_text: String = plain1.chars().map(|_| ' ').take(0).collect();
        let tab_cells = blank;
        let mut cells = 0;
        for ch in plain1.chars() {
            let cw = width(&ch.to_string());
            if cells < tab_cells {
                for _ in 0..cw {
                    pages_text.push(' ');
                }
            } else {
                pages_text.push(ch);
            }
            cells += cw;
        }
        (
            vec![fit(row0, w), fit(row1, w)],
            [plain1, pages_text],
            drops,
        )
    }

    /// The header's rows and the rule under them; on a small screen the
    /// places and the pages get a row each.
    pub fn header(&mut self) -> Vec<Line> {
        let w = self.w;
        crate::clanker::step(self);
        if w < NARROW {
            let t = self.tally();
            let md = self.mood(t);
            let mut counts = self.counts(t, true);
            counts.extend(self.news_badge());
            return self.narrow_header(md, counts);
        }
        let (text_rows, text, drops) = self.head_rows(w);
        if let Some(tb) = self.ext.top_bar {
            (tb.dropped)(self, drops);
        }
        self.header_text = text;
        // clanker whole, his rows above and below the text's two.
        let mut out = Vec::new();
        if self.cfg.hide_logo {
            out = text_rows;
        } else {
            let t = self.tally();
            let robot = self.robot(self.mood(t), t);
            let mut rows = text_rows.into_iter();
            for (i, r) in robot.into_iter().enumerate() {
                if (TEXT_AT..TEXT_AT + 2).contains(&i) {
                    out.extend(rows.next());
                } else {
                    let mut l = line([plain("  ")]);
                    join(&mut l, r);
                    out.push(fit(l, w));
                }
            }
        }
        out.push(line([sp("─".repeat(w), FAINT)]));
        out
    }

    fn narrow_header(&mut self, md: Mood, counts: Vec<Line>) -> Vec<Line> {
        let w = self.w;
        let face = if self.cfg.hide_logo {
            Line::new()
        } else {
            let t = self.tally();
            self.robot(md, t)[3].clone()
        };
        let fw = face.width();
        let indent = blanks(fw + 3);
        let room = w.saturating_sub(fw + 4);
        let mut title = line([spb("rush", TEXT)]);
        for n in (0..=counts.len()).rev() {
            let mut t = line([spb("rush", TEXT)]);
            for c in &counts[..n] {
                t.spans.push(plain("  "));
                join(&mut t, c.clone());
            }
            if t.width() <= room || n == 0 {
                title = t;
                break;
            }
        }
        let mut r1 = line([plain("  ")]);
        join(&mut r1, face);
        r1.spans.push(plain("  "));
        join(&mut r1, fit(title, room));
        let mut r2 = line([plain(format!("  {indent}"))]);
        join(&mut r2, self.acct());
        // As many places as fit around the one you're in.
        let tabs = self.tabs();
        let room = w.saturating_sub(indent.len() + 4);
        let tabs_w =
            |ts: &[Line]| ts.iter().map(Line::width).sum::<usize>() + ts.len().saturating_sub(1);
        let cur = self.place as usize;
        let mut start = 0;
        while start < cur && tabs_w(&tabs[start..=cur]) + 2 > room {
            start += 1;
        }
        let mut end = cur + 1;
        while end < tabs.len() && tabs_w(&tabs[start..end + 1]) + 2 <= room {
            end += 1;
        }
        let mut r3 = line([plain(format!("  {indent}"))]);
        if start > 0 {
            r3.spans.push(sp("‹ ", FAINT));
        }
        for (i, tb) in tabs[start..end].iter().enumerate() {
            if i > 0 {
                r3.spans.push(plain(" "));
            }
            join(&mut r3, tb.clone());
        }
        if end < tabs.len() {
            r3.spans.push(sp(" ›", FAINT));
        }
        let r3 = fit(r3, w);
        self.header_text[0] = r3.plain();
        let mut lead = line([plain(blanks(6 + clanker::W))]);
        let pw = w.saturating_sub(lead.width()) + 3;
        join(&mut lead, self.pages_line(pw));
        let lead = fit(lead, w);
        self.header_text[1] = lead.plain();
        vec![
            fit(r1, w),
            fit(r2, w),
            r3,
            lead,
            line([sp("─".repeat(w), FAINT)]),
        ]
    }
}

/// body on the left, rt on the right, the gap between; body alone, cut
/// to fit, when there's no room.
pub fn spread(body: Line, rt: Line, w: usize) -> Line {
    let gap = w as isize - body.width() as isize - rt.width() as isize - 2;
    if gap < 2 {
        return fit(body, w);
    }
    let mut out = body;
    out.spans.push(plain(blanks(gap as usize)));
    join(&mut out, rt);
    fit(out, w)
}

/// What each line of a bar left out for room, by line (barDrops).
pub type Drops = Vec<(usize, Vec<String>)>;

/// A path's last folders, as many as fit in w.
pub fn short_path(p: &str, w: usize) -> String {
    if width(p) <= w {
        return p.to_string();
    }
    let parts: Vec<&str> = p.split('/').collect();
    let mut s = parts[parts.len() - 1].to_string();
    for i in (0..parts.len() - 1).rev() {
        let next = format!("{}/{s}", parts[i]);
        if width(&next) + 2 > w {
            break;
        }
        s = next;
    }
    format!("…/{s}")
}
