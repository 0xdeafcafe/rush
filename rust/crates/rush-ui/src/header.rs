//! The header: clanker, the counts, where new agents start, the places
//! and the pages under them. After Go's internal/ui/view.go (tally :74,
//! mood :99, headH :128, underHead :146, header :165, withTabHint :314,
//! pagesWidth :361, tabs :395, narrowHeader :412, shortPath :450,
//! nextKey :855). Not ported: the top bar's gauges (statusline), the
//! header's wash and hover.

use rush_claude::fleet::Predicates;
use rush_core::text::{Line, Span, Style, width};

use crate::clanker::{self, Mood};
use crate::model::Model;
use crate::style::*;

/// Below this the header drops clanker's body for his face.
pub const NARROW: usize = 60;

const PLACES: [&str; 4] = ["Agents", "Projects", "Harnesses", "Settings"];

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
        for a in &self.agents {
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
        c
    }

    fn tabs(&self) -> Vec<Line> {
        PLACES
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let s = if i == 0 {
                    Style::fg(TAB_ON_FG).on(ORANGE).bold()
                } else {
                    Style::fg(SUB).on(TAB_OFF_BG)
                };
                line([Span::new(format!(" {v} "), s)])
            })
            .collect()
    }

    fn acct(&self) -> Line {
        line([sp("✻", ORANGE), plain(" "), sp(self.account.clone(), DIM)])
    }

    /// The header's rows, then the pages row and the rule under it.
    pub fn header(&self) -> Vec<Line> {
        let w = self.w;
        let t = self.tally();
        let md = self.mood(t);
        let counts = self.counts(t);
        if w < NARROW {
            return self.narrow_header(md, counts);
        }
        let robot = if self.cfg.hide_logo {
            vec![Line::new(); clanker::H]
        } else {
            clanker::draw(md, self.tick)
        };
        let gap = if self.cfg.hide_logo { "" } else { "   " };
        let mut left1 = line([Span::new("rush", Style::fg(TEXT).bold().italic())]);
        for c in counts {
            left1.spans.push(plain("   "));
            join(&mut left1, c);
        }
        let mut left2 = self.acct();
        left2
            .spans
            .push(sp(format!(" · {}", tildify(&self.launch_dir)), DIM));
        let row = |r: &Line, l: Line, rt: Line| {
            let mut body = line([plain("  ")]);
            join(&mut body, r.clone());
            body.spans.push(plain(gap));
            join(&mut body, l);
            spread(body, rt, w)
        };
        let mut rt0 = line([sp(money(t.today), DIM), sp(" today", DIM)]);
        rt0.spans.push(sp(" · ", FAINT));
        rt0.spans.push(sp(format!("rush {}", self.version), FAINT));
        let mut out = vec![
            row(&robot[0], left1, rt0),
            row(&robot[1], left2, Line::new()),
        ];
        let mut tabs = line([plain("  ")]);
        join(&mut tabs, robot[2].clone());
        tabs.spans.push(plain(gap));
        for (i, tb) in self.tabs().into_iter().enumerate() {
            if i > 0 {
                tabs.spans.push(plain(" "));
            }
            join(&mut tabs, tb);
        }
        out.push(fit(with_tab_hint(tabs, "< >", "places", w), w));
        if !self.cfg.hide_logo {
            let mut tail = line([plain("  ")]);
            join(&mut tail, robot[3].clone());
            out.push(fit(tail, w));
        }
        // The pages row, level with the tabs, its lead clanker's foot.
        let mut lead = line([plain("  ")]);
        if !self.cfg.hide_logo {
            let foot = robot[clanker::H - 1].clone();
            let fw = foot.width();
            join(&mut lead, foot);
            lead.spans.push(plain(blanks(
                (6 + clanker::W - 2).saturating_sub(fw).max(1),
            )));
        } else {
            lead = line([plain("   ")]);
        }
        join(&mut lead, self.pages());
        out.push(fit(lead, w));
        out.push(line([sp("─".repeat(w), FAINT)]));
        out
    }

    /// The pages of Agents, the one showing bright.
    fn pages(&self) -> Line {
        line([
            spb("Agents", TEXT),
            sp(" · ", DIM),
            sp("Wall", DIM),
            sp("  [ ]", FAINT),
        ])
    }

    fn narrow_header(&self, md: Mood, counts: Vec<Line>) -> Vec<Line> {
        let w = self.w;
        let face = clanker::draw(md, self.tick)[3].clone();
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
        let mut end = 1;
        while end < tabs.len() && tabs_w(&tabs[..end + 1]) + 2 <= room {
            end += 1;
        }
        let mut r3 = line([plain(format!("  {indent}"))]);
        for (i, tb) in tabs[..end].iter().enumerate() {
            if i > 0 {
                r3.spans.push(plain(" "));
            }
            join(&mut r3, tb.clone());
        }
        if end < tabs.len() {
            r3.spans.push(sp(" ›", FAINT));
        }
        let mut lead = line([plain(blanks(6 + clanker::W))]);
        join(&mut lead, self.pages());
        vec![
            fit(r1, w),
            fit(r2, w),
            fit(r3, w),
            fit(lead, w),
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
