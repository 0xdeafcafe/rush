//! The places at the top (Agents, Projects, Harnesses, Settings), the
//! pages inside one, and going between them. After Go's
//! internal/ui/model.go (viewNames :1817, setView :1828, landOnNeeds
//! :1850), keys.go (placeStep :232), view.go (frame :651, statusOr :702,
//! pageTabs :324, clickTab :343, pagesWidth :361, tabs :395),
//! header_pointer.go:16 (headerTabAt) and workstreams.go:35
//! (setAgentsPage).

use std::time::Instant;

use rush_core::text::{Color, Line};

use super::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// The places, in the bar's order (model.go:1820).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaceId {
    Agents = 0,
    Projects = 1,
    Harnesses = 2,
    Settings = 3,
}

impl PlaceId {
    pub const ALL: [PlaceId; 4] = [
        PlaceId::Agents,
        PlaceId::Projects,
        PlaceId::Harnesses,
        PlaceId::Settings,
    ];

    pub fn name(self) -> &'static str {
        ["Agents", "Projects", "Harnesses", "Settings"][self as usize]
    }

    /// The place d steps on, round the bar.
    pub fn step(self, d: isize) -> PlaceId {
        let n = PlaceId::ALL.len() as isize;
        PlaceId::ALL[((self as isize + d) % n + n) as usize % n as usize]
    }
}

/// A top-level screen under the header: a place, or one of Agents' pages
/// past its list. It's taken out of the model while one of its methods
/// runs, so it gets the model whole.
pub trait Place {
    /// Its name: a page's in the pages row. A place's tab says its
    /// PlaceId's name.
    fn name(&self) -> &str;
    /// Its pages' names, in order; none for a place of one page. [ ] and
    /// tab go through them.
    fn pages(&self, _m: &Model) -> Vec<String> {
        Vec::new()
    }
    /// The page showing.
    fn page(&self) -> usize {
        0
    }
    fn set_page(&mut self, _m: &mut Model, _p: usize) {}
    /// The pages row when it draws its own (Settings' tabs); None draws
    /// pages() as Go's pagesWidth does. w is the room after the lead.
    fn pages_line(&self, _m: &Model, _w: usize) -> Option<Line> {
        None
    }
    /// You came here (setView).
    fn enter(&mut self, _m: &mut Model) {}
    /// You went to another place.
    fn leave(&mut self, _m: &mut Model) {}
    /// A key. Pass leaves it to the frame: tab and [ ] turn the page, esc
    /// goes back to Agents.
    fn key(&mut self, m: &mut Model, k: &Press) -> Out;
    /// The mouse, x and y within the body as drawn (0, 0 its first cell).
    fn mouse(&mut self, _m: &mut Model, _e: &Mouse, _x: usize, _y: usize) -> Out {
        Out::Pass
    }
    fn paste(&mut self, _m: &mut Model, _text: &str) -> Out {
        Out::Pass
    }
    /// The rows under the header, w wide (the frame indents them two
    /// cells). The frame shows h of them, scrolled so a row on the
    /// selection's ground (SEL_BG) stays in view; draw only what's seen.
    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line>;
    /// The bottom row's keys, when no flash is showing.
    fn hint(&self, _m: &Model, _w: usize) -> Line {
        Line::new()
    }
    /// A question is being typed: , . < > are text, not places.
    fn asking(&self) -> bool {
        false
    }
    /// When to be woken with nothing else happening (a spinner while
    /// drawn); None sleeps.
    fn wake_at(&self, _m: &Model) -> Option<Instant> {
        None
    }
}

/// A place no group has registered yet: its name, and esc back.
pub struct Placeholder(pub &'static str);

impl Place for Placeholder {
    fn name(&self) -> &str {
        self.0
    }
    fn key(&mut self, _m: &mut Model, _k: &Press) -> Out {
        Out::Pass
    }
    fn body(&mut self, _m: &Model, _w: usize, _h: usize) -> Vec<Line> {
        vec![
            Line::new(),
            line([sp(format!("{} isn't in this rush yet", self.0), DIM)]),
        ]
    }
    fn hint(&self, _m: &Model, w: usize) -> Line {
        keys_fit(w, &[("esc", "back")])
    }
}

/// A place's or page's pages, the one showing, for the pages row.
pub struct Pages {
    pub names: Vec<String>,
    pub cur: usize,
}

impl Model {
    /// Runs f on place id taken out of the model; None for Agents, or
    /// while it's already out (called from inside itself).
    pub fn with_place<R>(
        &mut self,
        id: PlaceId,
        f: impl FnOnce(&mut dyn Place, &mut Model) -> R,
    ) -> Option<R> {
        let mut p = self.ext.places.get_mut(id as usize)?.take()?;
        let r = f(p.as_mut(), self);
        self.ext.places[id as usize] = Some(p);
        Some(r)
    }

    /// Runs f on Agents' page i (1 the first past the list).
    pub fn with_agents_page<R>(
        &mut self,
        i: usize,
        f: impl FnOnce(&mut dyn Place, &mut Model) -> R,
    ) -> Option<R> {
        let slot = i.checked_sub(1)?;
        let mut p = self.ext.agents_pages.get_mut(slot)?.take()?;
        let r = f(p.as_mut(), self);
        self.ext.agents_pages[slot] = Some(p);
        Some(r)
    }

    /// The screen under the header is a Place's (another place, or a page
    /// of Agents past its list), not the list and its conversation.
    pub fn on_place(&self) -> bool {
        self.place != PlaceId::Agents || self.agents_page > 0
    }

    /// Runs f on whichever Place is showing; None on Agents' list.
    pub fn with_shown<R>(&mut self, f: impl FnOnce(&mut dyn Place, &mut Model) -> R) -> Option<R> {
        if self.place == PlaceId::Agents {
            let p = self.agents_page;
            self.with_agents_page(p, f)
        } else {
            self.with_place(self.place, f)
        }
    }

    /// Switches the screen to a place, on the page it was last on: what's
    /// over the screen closes and the Agents box empties (setView).
    pub fn set_place(&mut self, id: PlaceId) {
        let from = self.place;
        if from != id {
            self.with_place(from, |p, m| p.leave(m));
        }
        self.place = id;
        self.sheet = None;
        self.confirm = None;
        self.input.clear();
        if id == PlaceId::Agents {
            // esc from the Wall lands here, on the plain list, never back
            // on the page it came from.
            self.agents_page = 0;
        } else {
            self.with_place(id, |p, m| p.enter(m));
        }
    }

    /// Which way a key moves between places: -1 for , and <, +1 for . >
    /// and ctrl+\, 0 when it doesn't. , . < > move only with nothing
    /// typed, and never while a box has the keys, so they can always be
    /// typed there (placeStep).
    pub fn place_step(&self, k: &Press) -> isize {
        let d = match k.s.as_str() {
            "," | "<" => -1,
            "." | ">" | "ctrl+\\" => 1,
            _ => return 0,
        };
        if self.place_asking() {
            return 0;
        }
        if k.s == "ctrl+\\" || self.on_place() {
            return d;
        }
        if self.session.is_some() {
            // The Session's box types them: a message may start with a >
            // quote. ctrl+\ still moves.
            return 0;
        }
        if !self.input.is_empty() {
            return 0;
        }
        d
    }

    /// Whether the place showing is asking for text.
    fn place_asking(&self) -> bool {
        if self.place == PlaceId::Agents {
            return self
                .agents_page
                .checked_sub(1)
                .and_then(|i| self.ext.agents_pages.get(i))
                .and_then(|p| p.as_ref())
                .is_some_and(|p| p.asking());
        }
        self.ext.places[self.place as usize]
            .as_ref()
            .is_some_and(|p| p.asking())
    }

    /// Goes d places on, round the bar, putting the cursor on what needs
    /// you on coming back to Agents.
    pub fn step_place(&mut self, d: isize) {
        let from = self.place;
        self.set_place(from.step(d));
        self.land_on_needs(from);
    }

    /// Puts the cursor on the first agent that needs you, if one does,
    /// when you come to Agents from another place (landOnNeeds).
    pub fn land_on_needs(&mut self, from: PlaceId) {
        if from == PlaceId::Agents || self.place != PlaceId::Agents {
            return;
        }
        self.rebuild();
        let needs = self
            .order
            .iter()
            .find(|&&i| {
                self.group_of.get(&self.agents[i].key).map(String::as_str)
                    == Some(crate::list::NEEDS)
            })
            .map(|&i| self.agents[i].key.clone());
        if let Some(k) = needs {
            if !self.sel.is_empty() && !self.sel.starts_with('§') {
                self.shown = self.sel.clone();
            }
            self.sel = k;
        }
    }

    /// The pages of what's showing, and the one up; None where there are
    /// none (pageTabs).
    pub fn page_tabs(&mut self) -> Option<Pages> {
        if self.place == PlaceId::Agents {
            let mut names = vec!["Agents".to_string()];
            names.extend(
                self.ext
                    .agents_pages
                    .iter()
                    .flatten()
                    .map(|p| p.name().to_string()),
            );
            return Some(Pages {
                names,
                cur: self.agents_page,
            });
        }
        let id = self.place;
        let p = self.ext.places[id as usize].as_ref()?;
        let names = p.pages(self);
        (!names.is_empty()).then(|| Pages {
            names,
            cur: p.page(),
        })
    }

    /// Shows page i of what's showing.
    pub fn set_page(&mut self, i: usize) {
        if self.place == PlaceId::Agents {
            let n = 1 + self.ext.agents_pages.len();
            let was = self.agents_page;
            self.agents_page = i % n;
            if was != self.agents_page {
                if was > 0 {
                    self.with_agents_page(was, |p, m| p.leave(m));
                }
                let now = self.agents_page;
                self.with_agents_page(now, |p, m| p.enter(m));
            }
            return;
        }
        self.with_place(self.place, |p, m| p.set_page(m, i));
    }

    /// [ and ]: the next page of what's showing, round.
    pub fn step_page(&mut self, d: isize) -> bool {
        let Some(t) = self.page_tabs() else {
            return false;
        };
        let n = t.names.len() as isize;
        if n == 0 {
            return false;
        }
        self.set_page(((t.cur as isize + d) % n + n) as usize % n as usize);
        true
    }

    /// The place tabs, the one you're in filled orange (tabs).
    pub fn place_tabs(&self) -> Vec<Line> {
        PlaceId::ALL
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let text = format!(" {} ", v.name());
                if *v == self.place {
                    line([rush_core::text::Span::new(
                        text,
                        rush_core::text::Style::fg(TAB_ON_FG).on(ORANGE).bold(),
                    )])
                } else if self.top_hover == Some((i, false)) {
                    hover_line(line([sp(text, TEXT)]), v.name().len() + 2)
                } else {
                    line([rush_core::text::Span::new(
                        text,
                        rush_core::text::Style::fg(SUB).on(TAB_OFF_BG),
                    )])
                }
            })
            .collect()
    }

    /// The pages row's words, as many as width has room for, the page
    /// showing bright; "ctrl+z zen" where there are no pages
    /// (pagesWidth). Leading blanks are the caller's.
    pub fn pages_line(&mut self, width: usize) -> Line {
        if self.place != PlaceId::Agents
            && let Some(Some(p)) = self.ext.places.get(self.place as usize)
            && let Some(l) = p.pages_line(self, width.saturating_sub(3))
        {
            return l;
        }
        let Some(t) = self.page_tabs() else {
            // Go's leading blanks stay here: they sit inside the faint ink, where
            // its TrimLeft doesn't reach them.
            return line([sp("   ctrl+z zen", FAINT)]);
        };
        let mut out = Line::new();
        for (i, n) in t.names.iter().enumerate() {
            if i > 0 {
                out.spans.push(sp(" · ", DIM));
            }
            if i == t.cur {
                out.spans.push(spb(n.clone(), TEXT));
            } else if self.top_hover == Some((i, true)) {
                join(&mut out, hover_line(line([sp(n.clone(), TEXT)]), n.len()));
            } else {
                out.spans.push(sp(n.clone(), DIM));
            }
        }
        out.spans.push(sp("  [ ]", FAINT));
        if out.width() + 3 > width {
            let cur = t.names[t.cur.min(t.names.len() - 1)].clone();
            return line([
                sp("‹ ", FAINT),
                spb(cur, TEXT),
                sp(format!(" {}/{} › ", t.cur + 1, t.names.len()), FAINT),
                sp("[ ]", FAINT),
            ]);
        }
        out
    }

    /// The tab at (x, y) of the header as last drawn: (index, is a page)
    /// (headerTabAt).
    pub fn header_tab_at(&mut self, x: usize, y: usize) -> Option<(usize, bool)> {
        if self.sheet.is_some() || x >= self.w || y >= self.top_h() {
            return None;
        }
        let at = |line: &str, names: &[String]| {
            for (i, n) in names.iter().enumerate() {
                if let Some(j) = line.find(n.as_str()) {
                    let left = rush_core::text::width(&line[..j]);
                    let right = left + rush_core::text::width(n);
                    if x + 1 >= left && x <= right {
                        return Some(i);
                    }
                }
            }
            None
        };
        if y == self.top_h() - 2 {
            let names = self.page_tabs()?.names;
            return at(&self.header_text[1], &names).map(|i| (i, true));
        }
        if y == 2 {
            let names: Vec<String> = PlaceId::ALL
                .iter()
                .map(|p| format!(" {} ", p.name()))
                .collect();
            return at(&self.header_text[0], &names).map(|i| (i, false));
        }
        None
    }

    /// A click on the header's tabs: to the place or page clicked
    /// (clickTab). False when it wasn't on one.
    pub fn click_tab(&mut self, x: usize, y: usize) -> bool {
        let Some((i, page)) = self.header_tab_at(x, y) else {
            return false;
        };
        if page {
            self.set_page(i);
        } else {
            let from = self.place;
            self.set_place(PlaceId::ALL[i]);
            self.land_on_needs(from);
        }
        true
    }

    /// The frame of a place (frame): the header, its body indented, a
    /// rule, and the bottom row: a flash, else the place's keys.
    pub(crate) fn place_frame(&mut self) -> Vec<Line> {
        let w = self.w;
        let mut out = self.header();
        let avail = self.h.saturating_sub(out.len() + 2);
        let (mut body, hint) = self
            .with_shown(|p, m| {
                let b = p.body(m, w.saturating_sub(2), avail);
                (b, p.hint(m, w.saturating_sub(2)))
            })
            .unwrap_or_default();
        // Keep the cursor's row in view on a long body.
        let cur = body
            .iter()
            .position(|l| l.bg == SEL_BG || l.spans.iter().any(|s| s.style.bg == SEL_BG))
            .unwrap_or(0);
        let start = if cur >= avail { cur - avail + 2 } else { 0 };
        let shown = body.len().min(start + avail);
        for l in body.drain(start.min(shown)..shown) {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            out.push(fit(r, w));
        }
        while out.len() < self.h.saturating_sub(2) {
            out.push(Line::new());
        }
        out.push(line([sp("─".repeat(w), FAINT)]));
        out.push(self.status_or(hint));
        out
    }

    /// The bottom row: a flash for a few seconds, else hint (statusOr).
    pub fn status_or(&self, hint: Line) -> Line {
        let w = self.w;
        if let Some((s, at, err)) = &self.flash
            && self.now - at < 6000
        {
            let c: Color = if *err { RED } else { TEXT };
            return fit(line([plain("  "), sp(s.clone(), c)]), w);
        }
        let mut l = line([plain("  ")]);
        join(&mut l, hint);
        fit(l, w)
    }
}
