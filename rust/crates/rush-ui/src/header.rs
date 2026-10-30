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
        for a in crate::hosted::all_agents(self) {
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

    /// The rows above the body: the header, the pages and a rule.
    pub fn top_h(&self) -> usize {
        let head = if self.w < NARROW || self.cfg.hide_logo {
            3
        } else {
            clanker::H - 1
        };
        head + 2
    }

    fn counts(&self, t: Tally) -> Vec<Line> {
        let mut c = Vec::new();
        if t.blocked > 0 {
            c.push(line([
                spb(format!("● {} needs you", t.blocked), YELLOW),
                plain(" "),
                spb("[ctrl+n]", YELLOW),
                plain(" "),
                sp("next", YELLOW),
            ]));
        }
        if t.working > 0 {
            c.push(line([sp(format!("✻ {} working", t.working), ORANGE)]));
        }
        if t.busy > 0 {
            c.push(line([sp(format!("◌ {} in background", t.busy), SUB)]));
        }
        c.push(line([sp(format!("{} finished", t.done), DIM)]));
        if let Some(n) = self.news_head() {
            c.push(n); // what rush did on its own, unseen (messages.go newsHead)
        }
        c
    }

    /// A Session has the keys on Agents and is open: its host reached, or
    /// its conversation read without a failure. While it's still
    /// connecting, or couldn't be opened, there's no box to type in
    /// (view.go:240's m.paneFocus && m.host != nil && m.mode == modeList
    /// && m.dialog == nil; m.host is set by onHostOpen, rushmode.go:2136).
    fn in_box(&self) -> bool {
        let Some(v) = self.session.as_ref().filter(|_| !self.on_place()) else {
            return false;
        };
        let linked = self
            .kept
            .peek::<crate::screens::session_core::Core>()
            .and_then(|c| c.conns.get(&v.key))
            .is_some_and(|c| c.hosted());
        // A hosted one's conversation is its host's: only reaching it opens it.
        let hosted = matches!(v.source, crate::session::Source::Hosted);
        linked || !hosted && v.read && v.err.is_empty()
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

    /// The header's own rows, w wide, above the pages row: clanker, the
    /// counts, where new agents start and the places, with the top bar
    /// down the right (view.go:165 header). Also the tabs row's words,
    /// for clicks, and what the top bar had no room for.
    pub fn head_rows(&self, w: usize) -> (Vec<Line>, String, Drops) {
        let t = self.tally();
        let md = self.mood(t);
        let mut counts = self.counts(t);
        let tb = self.ext.top_bar;
        if let Some(o) = tb.and_then(|tb| (tb.orphans)(self)) {
            // Agents' leftovers, ahead of what rush said (view.go:189).
            let at = counts.len() - usize::from(self.news_head().is_some());
            counts.insert(at, o);
        }
        let robot = if self.cfg.hide_logo {
            vec![Line::new(); clanker::H]
        } else {
            self.robot(md, t)
        };
        let gap = if self.cfg.hide_logo { "" } else { "   " };
        let mut left1 = line([Span::new("rush", Style::fg(TEXT).bold().italic())]);
        for c in counts {
            left1.spans.push(plain("   "));
            join(&mut left1, c);
        }
        // Which provider, account and profile new sessions start on:
        // beside the usage it reads when the top bar shows usage, here
        // when it doesn't.
        let usage_shown = tb.is_some_and(|tb| (tb.usage_shown)(self));
        let left2 = if usage_shown {
            line([sp(tildify(&self.launch_dir), DIM)])
        } else {
            let mut l = self.acct();
            l.spans
                .push(sp(format!(" · {}", tildify(&self.launch_dir)), DIM));
            l
        };
        let lead = |r: &Line, l: Line| {
            let mut body = line([plain("  ")]);
            join(&mut body, r.clone());
            body.spans.push(plain(gap));
            join(&mut body, l);
            body
        };
        // < > go between places; in a Session's box they're text, so it's
        // ctrl+\ there (view.go:240).
        let keys = if crate::hosted::id(self).is_some() || self.in_box() {
            "ctrl+\\"
        } else {
            "< >"
        };
        let mut strip = line([plain("  ")]);
        join(&mut strip, robot[2].clone());
        strip.spans.push(plain(gap));
        for (i, tb) in self.tabs().into_iter().enumerate() {
            if i > 0 {
                strip.spans.push(plain(" "));
            }
            join(&mut strip, tb);
        }
        let tabs = with_tab_hint(strip, keys, "places", w);
        let mut tail = line([plain("  ")]);
        join(&mut tail, robot[3].clone());
        let row0 = lead(&robot[0], left1.clone());
        let row1 = lead(&robot[1], left2.clone());
        let mut drops = Drops::new();
        let Some(tb) = tb else {
            // No top bar: today's spend and this rush.
            let mut rt0 = line([sp(money(t.today), DIM), sp(" today", DIM)]);
            rt0.spans.push(sp(" · ", FAINT));
            rt0.spans.push(sp(format!("rush {}", self.version), FAINT));
            let mut out = vec![spread(row0, rt0, w), fit(row1, w), fit(tabs, w)];
            if !self.cfg.hide_logo {
                out.push(fit(tail, w));
            }
            let text = out[2].plain();
            return (out, text, drops);
        };
        // The top bar's lines go down the right a row each: beside the
        // counts, the folder, the tabs and, with clanker, his feet.
        let room = |r: &Line, l: &Line| w.saturating_sub(2 + r.width() + gap.len() + l.width() + 4);
        let mut rooms = vec![room(&robot[1], &left1), room(&robot[2], &left2)];
        let mut rights = |rows: &[&Line], drops: &mut Drops| {
            for r in rows {
                rooms.push(w.saturating_sub(r.width() + 4));
            }
            let mut out = (tb.rights)(self, &rooms, drops);
            out.resize(rooms.len(), Line::new());
            if !self.loaded {
                let mut l = line([sp("costing transcripts…   ", DIM)]);
                join(&mut l, std::mem::take(&mut out[1]));
                out[1] = l;
            }
            out
        };
        if self.cfg.hide_logo {
            let rt = rights(&[&tabs], &mut drops);
            let out = vec![
                spread(row0, rt[0].clone(), w),
                spread(row1, rt[1].clone(), w),
                spread(tabs, rt[2].clone(), w),
            ];
            let text = out[2].plain();
            return (out, text, drops);
        }
        // The default top bar is a block of gauges down the right, a row
        // each, rather than one line beside the counts.
        let lead_w = 2 + robot[2].width() + gap.len();
        let gw = w.saturating_sub(
            left1
                .width()
                .max(left2.width())
                .max(tabs.width().saturating_sub(lead_w))
                + clanker::W
                + 12,
        );
        if let Some(mut g) = (tb.gauges)(self, t.today, gw) {
            let col = w.saturating_sub(2 + g.iter().map(Line::width).max().unwrap_or(0));
            let at = |row: Line, r: Line| {
                if r.width() == 0 {
                    return row;
                }
                let mut f = fit(trim_end(row), col);
                let fw = f.width();
                f.spans.push(plain(blanks(col.saturating_sub(fw))));
                join(&mut f, r);
                f
            };
            g.resize(4, Line::new());
            let mut g = g.into_iter();
            let mut nx = || g.next().unwrap_or_default();
            let out = vec![
                at(fit(row0, w), nx()),
                at(fit(row1, w), nx()),
                at(tabs, nx()),
                at(tail, nx()),
            ];
            let text = out[2].plain();
            return (out, text, drops);
        }
        let rt = rights(&[&tabs, &tail], &mut drops);
        let tail = if rt[3].width() > 0 {
            spread(tail, rt[3].clone(), w)
        } else {
            tail
        };
        let out = vec![
            spread(row0, rt[0].clone(), w),
            spread(row1, rt[1].clone(), w),
            spread(tabs, rt[2].clone(), w),
            tail,
        ];
        let text = out[2].plain();
        (out, text, drops)
    }

    /// The header's rows, then the pages row and the rule under it.
    pub fn header(&mut self) -> Vec<Line> {
        let w = self.w;
        crate::clanker::step(self);
        if w < NARROW {
            let t = self.tally();
            let md = self.mood(t);
            let counts = self.counts(t);
            return self.narrow_header(md, counts);
        }
        let (mut out, text, drops) = self.head_rows(w);
        if let Some(tb) = self.ext.top_bar {
            (tb.dropped)(self, drops);
        }
        self.header_text[0] = text;
        // The pages row, level with the tabs, its lead clanker's foot.
        let mut lead = line([plain("  ")]);
        if !self.cfg.hide_logo {
            let t = self.tally();
            let foot = self.robot(self.mood(t), t)[clanker::H - 1].clone();
            let fw = foot.width();
            join(&mut lead, foot);
            lead.spans.push(plain(blanks(
                (6 + clanker::W - 2).saturating_sub(fw).max(1),
            )));
        } else {
            lead = line([plain("   ")]);
        }
        let pw = w.saturating_sub(lead.width()) + 3;
        join(&mut lead, self.pages_line(pw));
        let lead = fit(lead, w);
        self.header_text[1] = lead.plain();
        out.push(lead);
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
        let acct = self.acct();
        let aw = acct.width();
        let mut r2 = line([plain(format!("  {indent}"))]);
        join(&mut r2, acct);
        r2.spans.push(sp(
            format!(
                " · {}",
                short_path(
                    &tildify(&self.launch_dir),
                    w.saturating_sub(indent.len() + aw + 5)
                )
            ),
            DIM,
        ));
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

/// A line with its trailing blanks gone (strings.TrimRight(row, " ")).
fn trim_end(mut l: Line) -> Line {
    while let Some(last) = l.spans.last_mut() {
        let t = last.text.trim_end_matches(' ');
        if t.is_empty() {
            l.spans.pop();
            continue;
        }
        let n = t.len();
        last.text.truncate(n);
        break;
    }
    l
}

/// A strip of tabs with a quiet hint after it, when it fits.
fn with_tab_hint(mut strip: Line, keys: &str, what: &str, w: usize) -> Line {
    let hint = line([sp(format!("   {keys}"), DIM), sp(format!(" {what}"), FAINT)]);
    if strip.width() + hint.width() > w {
        return strip;
    }
    join(&mut strip, hint);
    strip
}

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
