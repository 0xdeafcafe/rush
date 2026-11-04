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
    /// A folder picker: what's typed is a path. ↑↓ put the row chosen in
    /// the text to go on editing, tab completes it as a shell does, and
    /// the rows keep to what was last typed until you type again.
    pub path: bool,
    /// What the rows are narrowed by while the text is a row chosen, not
    /// typed, with the text that was put there: once the text is anything
    /// else, they're narrowed by it.
    filter: Option<(String, String)>,
    /// The text is the folder it opened on, as a whole: typing replaces
    /// it, as in an address bar; backspace or tab edits it.
    fresh: bool,
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
            path: false,
            filter: None,
            fresh: false,
        }
    }

    /// Starts the text at t, the rows all shown still: a folder picker
    /// open on the folder you're in, to edit (path mode).
    pub fn start_at(&mut self, t: &str) {
        if let Some((_, q)) = self.query.as_mut() {
            *q = t.chars().collect();
            self.filter = Some((String::new(), t.to_string()));
            self.fresh = true;
        }
    }

    /// What the rows are narrowed by: what was last typed.
    pub fn narrowed_by(&self) -> String {
        let text = self.typed_text();
        match &self.filter {
            Some((by, put)) if *put == text => by.clone(),
            _ => text,
        }
    }

    /// The text still the folder it opened on, untouched.
    fn is_fresh(&self) -> bool {
        self.fresh && matches!(&self.filter, Some((_, put)) if *put == self.typed_text())
    }

    /// The text set to t, as if typed: the rows narrow to it.
    fn set_text(&mut self, m: &mut Model, t: &str) {
        if let Some((_, q)) = self.query.as_mut() {
            *q = t.chars().collect();
        }
        self.filter = None;
        self.fresh = false;
        self.cursor = 0;
        self.queried(m);
    }

    /// The cursor moved d rows, the text following it to the row chosen.
    fn choose(&mut self, d: isize) {
        let shown = self.shown();
        self.cursor = round_move(self.cursor, d, shown.len());
        if let Some(r) = shown.get(self.cursor) {
            let t = tildify(&r.value);
            self.filter = Some((self.narrowed_by(), t.clone()));
            self.fresh = false;
            if let Some((_, q)) = self.query.as_mut() {
                *q = t.chars().collect();
            }
        }
    }

    /// Tab: the text completed as a shell does. A row chosen goes in
    /// whole, ending in /, for its folders; otherwise as much as the
    /// rows under what's typed share, all of it when there's one.
    fn complete(&mut self, m: &mut Model) {
        let shown = self.shown();
        let text = self.typed_text();
        let chosen = shown
            .get(self.cursor)
            .filter(|_| self.cursor > 0 || self.narrowed_by() != text)
            .map(|r| tildify(&r.value));
        let t = if let Some(c) = chosen {
            dir_slash(&c)
        } else {
            let under: Vec<String> = shown
                .iter()
                .map(|r| tildify(&r.value))
                .filter(|v| {
                    v.starts_with(&text) && v.trim_end_matches('/') != text.trim_end_matches('/')
                })
                .collect();
            match under.as_slice() {
                [] => match shown.first() {
                    Some(r) => dir_slash(&tildify(&r.value)),
                    None => return,
                },
                [one] => dir_slash(one),
                many => {
                    let p = shared_start(many);
                    if p.chars().count() <= text.chars().count() {
                        // Nothing more they share: tab goes through them.
                        self.choose(1);
                        return;
                    }
                    p
                }
            }
        };
        self.set_text(m, &t);
    }

    /// The rows what's typed matches: a typed row first, then the rest in
    /// their order (shownDirs).
    pub fn shown(&self) -> Vec<Pick> {
        if self.query.is_none() {
            return self.rows.clone();
        }
        let q = self.narrowed_by();
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

/// A folder ending in /, for what's in it.
fn dir_slash(p: &str) -> String {
    if p.ends_with('/') {
        p.to_string()
    } else {
        format!("{p}/")
    }
}

/// The start all of v share, whole characters.
fn shared_start(v: &[String]) -> String {
    let mut p: Vec<char> = v[0].chars().collect();
    for s in &v[1..] {
        let n = p
            .iter()
            .zip(s.chars())
            .take_while(|(a, b)| **a == *b)
            .count();
        p.truncate(n);
    }
    p.into_iter().collect()
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
        if self.path {
            match s {
                "up" | "shift+tab" => {
                    self.choose(-1);
                    return Out::Draw;
                }
                "down" => {
                    self.choose(1);
                    return Out::Draw;
                }
                "tab" => {
                    self.complete(m);
                    return Out::Draw;
                }
                _ => {}
            }
        }
        match s {
            "up" | "shift+tab" => self.cursor = round_move(self.cursor, -1, n),
            "k" if !filtering => self.cursor = round_move(self.cursor, -1, n),
            "down" | "tab" => self.cursor = round_move(self.cursor, 1, n),
            "j" if !filtering => self.cursor = round_move(self.cursor, 1, n),
            "enter" if self.path => {
                // The text, when it's been edited past the row chosen.
                let typed = self.typed_text();
                let t = typed.trim().trim_end_matches('/');
                let shown = self.shown();
                let row = shown.get(self.cursor).cloned();
                let same = row
                    .as_ref()
                    .is_some_and(|r| tildify(&r.value).trim_end_matches('/') == t);
                let pick = if same || t.is_empty() {
                    row
                } else {
                    self.typed.as_ref().and_then(|f| f(typed.trim())).or(row)
                };
                let Some(pick) = pick else {
                    return Out::Idle;
                };
                let r = (self.on_pick)(m, Some(&pick), &typed);
                return if r == Out::Quit { r } else { Out::Close };
            }
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
                self.filter = None;
                self.fresh = false;
                self.cursor = 0;
                self.queried(m);
            }
            "ctrl+u" | "super+backspace" if filtering => {
                self.query.as_mut().expect("filtering").1.clear();
                self.filter = None;
                self.fresh = false;
                self.cursor = 0;
                self.queried(m);
            }
            _ => {
                let Some(c) = k.text().filter(|_| filtering) else {
                    return Out::Idle;
                };
                if self.is_fresh() {
                    self.query.as_mut().expect("filtering").1.clear();
                }
                self.fresh = false;
                self.query.as_mut().expect("filtering").1.push(c);
                self.filter = None;
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
            let mut t = plain(q.iter().collect::<String>());
            if self.is_fresh() {
                // Typing replaces it: drawn as a selection.
                t.style.bg = SEL_BG;
            }
            out.push(line([sp(label.clone(), DIM), t, sp("▏", ORANGE)]));
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
        // A path typed: the rows under it show it dim, what's beyond bright.
        let under = Some(self.narrowed_by())
            .filter(|q| self.path && (q.starts_with('/') || q.starts_with('~')));
        for i in top..shown.len().min(top + room) {
            let mut p = shown[i].clone();
            if let Some(q) = &under {
                let t = p.label.plain();
                if t.len() > q.len() && t.starts_with(q.as_str()) {
                    p.label = line([sp(q.clone(), DIM), sp(t[q.len()..].to_string(), TEXT)]);
                }
            }
            let p = &p;
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
        let q = self.narrowed_by();
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
