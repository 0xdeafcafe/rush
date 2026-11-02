//! A small dropdown over the screen: one of a list, under headings, with
//! typing that narrows it when asked. The frame of Go's picker (the PRs,
//! the folder a session starts in, what to do with a link); what each
//! lists is its screen's. After Go's internal/ui/picker.go (picker :24,
//! size :303, pickerKey :345, dirPickerKey :375, pickerBody :411) and
//! fleetslash.go:361 (roundMove).

use rush_core::text::Line;

use super::sheet::Sheet;
use super::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// One row of a picker.
#[derive(Clone, Debug, Default)]
pub struct Pick {
    /// The row as drawn.
    pub label: Line,
    /// What typing matches against (the label's words when empty).
    pub find: String,
    /// The heading it's listed under; rows of one heading go together.
    pub section: String,
    /// What the row stands for, for on_pick.
    pub value: String,
}

impl Pick {
    pub fn new(text: impl Into<String>) -> Pick {
        let t: String = text.into();
        Pick {
            label: line([sp(t.clone(), TEXT)]),
            value: t,
            ..Pick::default()
        }
    }

    pub fn under(mut self, section: impl Into<String>) -> Pick {
        self.section = section.into();
        self
    }

    fn words(&self) -> String {
        if self.find.is_empty() {
            self.label.plain()
        } else {
            self.find.clone()
        }
    }
}

/// What a pick does: given the row picked (None when what's typed is
/// taken as it is), and what was typed. The picker has closed by then.
pub type OnPick = Box<dyn FnMut(&mut Model, Option<&Pick>, &str) -> Out>;
/// Called as what's typed changes, to look for more rows (on disk, off
/// the UI thread): the picker's rows are set through `Model::picker`.
pub type OnQuery = Box<dyn FnMut(&mut Model, &str)>;

/// Typed text as a row of its own, when it is one.
pub type Typed = Box<dyn Fn(&str) -> Option<Pick>>;

/// A dropdown: a title, what it's about, its rows and a cursor.
pub struct Picker {
    pub title: String,
    /// After the title, dimmed.
    pub note: String,
    /// Under the title, wrapped.
    pub about: String,
    pub rows: Vec<Pick>,
    /// Typing narrows the rows, after this label ("Folder ❯ "); None for
    /// a list typing doesn't touch.
    pub query: Option<(String, Vec<char>)>,
    /// What shows when nothing matches.
    pub empty: String,
    /// What typed text is taken as when it's a row of its own: the first
    /// row, "Path" (a folder typed).
    pub typed: Option<Typed>,
    pub cursor: usize,
    /// The keys row; enter's word by default.
    pub keys: Vec<(String, String)>,
    /// Extra keys that close it as esc does ("ctrl+l" for the folder
    /// picker).
    pub closes: Vec<String>,
    pub on_pick: OnPick,
    pub on_query: Option<OnQuery>,
    /// Its own width, past the usual 100 (pickerWide).
    pub wide: bool,
}

impl Picker {
    pub fn new(title: impl Into<String>, rows: Vec<Pick>, on_pick: OnPick) -> Picker {
        Picker {
            title: title.into(),
            note: String::new(),
            about: String::new(),
            rows,
            query: None,
            empty: String::new(),
            typed: None,
            cursor: 0,
            keys: vec![
                ("↑↓".into(), "choose".into()),
                ("enter".into(), "pick".into()),
                ("esc".into(), "close".into()),
            ],
            closes: Vec::new(),
            on_pick,
            on_query: None,
            wide: false,
        }
    }

    /// The rows what's typed matches: a typed row first, then the rest in
    /// their order (shownDirs).
    pub fn shown(&self) -> Vec<Pick> {
        let q = match &self.query {
            Some((_, q)) => q.iter().collect::<String>(),
            None => return self.rows.clone(),
        };
        let q = q.trim().to_string();
        let mut out = Vec::new();
        if let Some(t) = self.typed.as_ref().and_then(|f| f(&q)) {
            out.push(t);
        }
        let lq = q.to_lowercase();
        for r in &self.rows {
            if r.words().to_lowercase().contains(&lq)
                && !out.iter().any(|o: &Pick| o.value == r.value)
            {
                out.push(r.clone());
            }
        }
        out
    }

    fn typed_text(&self) -> String {
        self.query
            .as_ref()
            .map(|(_, q)| q.iter().collect())
            .unwrap_or_default()
    }
}

/// The cursor moved d rows, round the ends (roundMove).
pub fn round_move(cur: usize, d: isize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as isize;
    (((cur as isize + d) % n + n) % n) as usize
}

impl Sheet for Picker {
    fn width(&self, m: &Model) -> usize {
        if self.wide {
            m.w.saturating_sub(6)
        } else {
            100
        }
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let s = k.s.as_str();
        let n = self.shown().len();
        let filtering = self.query.is_some();
        if s == "esc" || self.closes.iter().any(|c| c == s) || (!filtering && s == "q") {
            return Out::Close;
        }
        match s {
            "up" | "shift+tab" => self.cursor = round_move(self.cursor, -1, n),
            "k" if !filtering => self.cursor = round_move(self.cursor, -1, n),
            "down" | "tab" => self.cursor = round_move(self.cursor, 1, n),
            "j" if !filtering => self.cursor = round_move(self.cursor, 1, n),
            "enter" => {
                let shown = self.shown();
                let typed = self.typed_text();
                if shown.is_empty() && !filtering {
                    return Out::Idle;
                }
                let pick = shown.get(self.cursor.min(shown.len().saturating_sub(1)));
                if pick.is_none() {
                    return Out::Idle;
                }
                let r = (self.on_pick)(m, pick, &typed);
                return if r == Out::Quit { r } else { Out::Close };
            }
            "backspace" | "ctrl+h" if filtering => {
                let (_, q) = self.query.as_mut().expect("filtering");
                if q.pop().is_none() {
                    return Out::Idle;
                }
                self.cursor = 0;
                self.queried(m);
            }
            "ctrl+u" | "super+backspace" if filtering => {
                self.query.as_mut().expect("filtering").1.clear();
                self.cursor = 0;
                self.queried(m);
            }
            _ => {
                let Some(c) = k.text().filter(|_| filtering) else {
                    return Out::Idle;
                };
                self.query.as_mut().expect("filtering").1.push(c);
                self.cursor = 0;
                self.queried(m);
            }
        }
        Out::Draw
    }

    fn body(&mut self, m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut title = line([spb(self.title.clone(), TEXT)]);
        if !self.note.is_empty() {
            title.spans.push(sp(format!(" · {}", self.note), DIM));
        }
        let mut out = vec![title, Line::new()];
        if !self.about.is_empty() {
            for l in wrap(&self.about, w) {
                out.push(line([sp(l, DIM)]));
            }
            out.push(Line::new());
        }
        if let Some((label, q)) = &self.query {
            out.push(line([
                sp(label.clone(), DIM),
                plain(q.iter().collect::<String>()),
                sp("▏", ORANGE),
            ]));
            out.push(Line::new());
        }
        let shown = self.shown();
        if shown.is_empty() && !self.empty.is_empty() {
            out.push(line([sp(format!(" {}", self.empty), FAINT)]));
        }
        // Room for the rows, and up to four headings.
        let room = 4.max(m.h.saturating_sub(18));
        let top = (self.cursor as isize - room as isize + 1)
            .min(shown.len() as isize - room as isize)
            .max(0) as usize;
        for i in top..shown.len().min(top + room) {
            let p = &shown[i];
            if !p.section.is_empty() && (i == top || p.section != shown[i - 1].section) {
                out.push(line([sp(format!(" {}", p.section), FAINT)]));
            }
            if i == self.cursor {
                let mut r = line([sp("▍", ORANGE)]);
                join(&mut r, p.label.clone());
                out.push(highlight(r, w));
            } else {
                let mut r = line([plain(" ")]);
                join(&mut r, p.label.clone());
                out.push(r);
            }
        }
        out.push(Line::new());
        let pairs: Vec<(&str, &str)> = self
            .keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        out.push(keys(&pairs));
        out
    }
}

impl Picker {
    fn queried(&mut self, m: &mut Model) {
        let q = self.typed_text();
        if let Some(f) = self.on_query.as_mut() {
            f(m, q.trim());
        }
    }
}

impl Model {
    /// The picker open, to change its rows (more found on disk).
    pub fn picker(&mut self) -> Option<&mut Picker> {
        self.sheet_as::<Picker>()
    }
}
