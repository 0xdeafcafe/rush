//! A form page's rows: sections of settings, each a value ←→ go through or
//! one you type, or a line of its own; the keys on the highlighted row,
//! and how a row and its explanation are drawn. After Go's
//! internal/ui/settings.go:262-527 (section, setting, shown, now, flat,
//! cycle, formKey, rowAt, formRows, formKeys, settingRow, about,
//! choiceSetting).

use std::collections::HashMap;
use std::rc::Rc;

use rush_core::text::{Line, width};

use super::paint::{first_non_empty, trunc};
use super::{Answer, ask, changed, dialog};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// Sets a value as kept.
pub type SetFn = Rc<dyn Fn(&mut Model, &str)>;
/// Sets a value with work to start.
pub type RunFn = Rc<dyn Fn(&mut Model, &str) -> Out>;
/// A row's own keys, before ←→: Pass when it isn't one.
pub type KeyFn = Rc<dyn Fn(&mut Model, &str) -> Out>;
/// Draws a row itself, w wide.
pub type LineFn = Rc<dyn Fn(usize) -> Line>;
/// About, when it's more than what and means: title, what, now.
pub type AboutFn = Rc<dyn Fn() -> (String, String, String)>;
/// What a setting changes, as it is now, w wide in at most h rows.
pub type PreviewFn = Rc<dyn Fn(usize, usize) -> Vec<Line>>;

/// A titled run of settings on a form page (section).
#[derive(Clone, Default)]
pub struct Section {
    pub title: String,
    /// After the title, quieter.
    pub note: String,
    pub rows: Vec<Setting>,
    /// Folded under one line until you open them.
    pub advanced: bool,
}

impl Section {
    pub fn new(title: &str, rows: Vec<Setting>) -> Section {
        Section {
            title: title.into(),
            rows,
            ..Section::default()
        }
    }
}

/// One line of a form (setting): a value that ←→ go through or you type,
/// or, with line and key, a line of its own.
#[derive(Clone, Default)]
pub struct Setting {
    pub label: String,
    /// As kept; "" is the default.
    pub value: String,
    /// What ←→ go through.
    pub choices: Vec<String>,
    pub set: Option<SetFn>,
    /// Set instead, for a change with work to start.
    pub run: Option<RunFn>,
    /// What it does, for About.
    pub what: String,
    /// What each value means.
    pub means: HashMap<String, String>,
    /// How "" shows; "default" when not given.
    pub unset: String,
    /// How other values show, when not as kept.
    pub names: HashMap<String, String>,
    /// enter types a value rather than choosing one.
    pub typed: bool,
    /// Shown after label in a catalog's list.
    pub tag: Line,
    pub line: Option<LineFn>,
    pub key: Option<KeyFn>,
    /// Its own keys, for the key line.
    pub keys: Vec<String>,
    pub about: Option<AboutFn>,
    pub preview: Option<PreviewFn>,
}

impl Setting {
    /// How the value is shown (shown).
    pub fn shown(&self) -> String {
        if self.value.is_empty() {
            return first_non_empty(&[&self.unset, "default"]).to_string();
        }
        self.names
            .get(&self.value)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| self.value.clone())
    }

    /// What the value means, in a sentence (now).
    pub fn now(&self) -> String {
        if self.value.is_empty() && self.choices.is_empty() && !self.typed {
            return String::new();
        }
        match self.means.get(&self.value) {
            Some(s) if !s.is_empty() => format!("{}: {s}", self.shown()),
            _ => format!("{}.", self.shown()),
        }
    }

    /// Its title, what it does and what it's set to now, for About.
    pub fn explained(&self) -> (String, String, String) {
        match &self.about {
            Some(f) => f(),
            None => (self.label.clone(), self.what.clone(), self.now()),
        }
    }
}

/// A setting from a list of values and what each means (choiceSetting).
pub fn choice(
    label: &str,
    value: &str,
    what: &str,
    choices: &[(&str, &str)],
    set: Option<SetFn>,
) -> Setting {
    let mut st = Setting {
        label: label.into(),
        value: value.into(),
        what: what.into(),
        set,
        ..Setting::default()
    };
    for (v, means) in choices {
        st.choices.push(v.to_string());
        st.means.insert(v.to_string(), means.to_string());
    }
    st
}

/// A SetFn from a closure.
pub fn set(f: impl Fn(&mut Model, &str) + 'static) -> Option<SetFn> {
    Some(Rc::new(f))
}

/// A form's rows in order (flat).
pub fn flat(secs: &[Section]) -> Vec<Setting> {
    secs.iter().flat_map(|s| s.rows.iter().cloned()).collect()
}

/// A form's row i, or none (rowAt).
pub fn row_at(secs: &[Section], i: usize) -> Setting {
    secs.iter()
        .flat_map(|s| s.rows.iter())
        .nth(i)
        .cloned()
        .unwrap_or_default()
}

/// Moves s to the choice dir away, round the ends (cycle).
pub fn cycle(m: &mut Model, s: &Setting, dir: isize) -> Out {
    if s.choices.is_empty() {
        return Out::Idle;
    }
    let mut i = 0;
    for (j, c) in s.choices.iter().enumerate() {
        if *c == s.value {
            i = j;
        }
    }
    let n = s.choices.len() as isize;
    let v = s.choices[((i as isize + dir + n) % n) as usize].clone();
    if let Some(run) = &s.run {
        return run(m, &v);
    }
    if let Some(set) = &s.set {
        set(m, &v);
    }
    Out::Draw
}

/// A key on a form's highlighted row (formKey).
pub fn form_key(m: &mut Model, secs: &[Section], s: &str) -> Out {
    let rows = flat(secs);
    let Some(st) = rows.get(dialog(m).cursor).cloned() else {
        return Out::Idle;
    };
    if let Some(k) = &st.key {
        let o = k(m, s);
        if o.used() {
            return o;
        }
    }
    if !st.typed && st.choices.len() < 2 {
        return Out::Idle;
    }
    let done = |m: &mut Model, o: Out| {
        changed(m);
        if o == Out::Pass { Out::Draw } else { o }
    };
    match s {
        "enter" | "right" | "l" | "space" => {
            if st.typed && (s == "enter" || st.choices.is_empty()) {
                let label = st.label.clone();
                let value = st.value.clone();
                let answer: Answer = Rc::new(move |m: &mut Model, v: &str| {
                    let o = match (&st.run, &st.set) {
                        (Some(run), _) => run(m, v),
                        (None, Some(set)) => {
                            set(m, v);
                            Out::Draw
                        }
                        _ => Out::Draw,
                    };
                    changed(m);
                    o
                });
                ask(m, &label, &value, answer);
                return Out::Draw;
            }
            let o = cycle(m, &st, 1);
            done(m, o)
        }
        "left" | "h" => {
            let o = cycle(m, &st, -1);
            done(m, o)
        }
        _ => Out::Idle,
    }
}

/// A form's sections drawn, row cur highlighted (formRows).
pub fn form_rows(secs: &[Section], cur: usize, w: usize) -> Vec<Line> {
    // One label and one value column for the page, so the › line up.
    let (mut label_w, mut value_w) = (0usize, 12usize);
    for st in secs.iter().flat_map(|s| s.rows.iter()) {
        if st.line.is_none() {
            label_w = label_w.max(width(&st.label) + 2);
            if st.choices.len() > 1 || st.typed {
                value_w = value_w.max(width(&st.shown()));
            }
        }
    }
    let label_w = label_w.min(44);
    let room = (w as isize - label_w as isize - 24).clamp(i32::MIN as isize, 40);
    let value_w = value_w.min(room.max(12) as usize);
    let mut out = Vec::new();
    let mut i = 0;
    for sec in secs {
        out.push(Line::new());
        if !sec.title.is_empty() {
            let mut title = line([sp(sec.title.clone(), DIM)]);
            if !sec.note.is_empty() {
                title.spans.push(sp(format!(" · {}", sec.note), FAINT));
            }
            out.push(trunc(title, w));
        }
        for st in &sec.rows {
            out.push(setting_row(i == cur, st, label_w, value_w, w));
            i += 1;
        }
    }
    out
}

/// The key line for row cur of a form (formKeys).
pub fn form_keys(cur: &Setting, page_keys: &[String], w: usize) -> Line {
    let mut keys: Vec<String> = Vec::new();
    if cur.line.is_none() && cur.choices.len() > 1 {
        let change = if page_keys.first().is_some_and(|k| k.starts_with('←')) {
            "→ space" // the page's ← goes back
        } else {
            "←→"
        };
        keys.push(change.into());
        keys.push("change".into());
    }
    if cur.typed {
        keys.push("enter".into());
        keys.push("type it".into());
    }
    keys.extend(cur.keys.iter().cloned());
    keys.extend(page_keys.iter().cloned());
    keys.extend(PAGES_KEYS.iter().map(|s| s.to_string()));
    let mut seen = std::collections::HashSet::new();
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for p in keys.chunks(2) {
        if p.len() == 2 && seen.insert(p[0].as_str()) {
            pairs.push((&p[0], &p[1]));
        }
    }
    keys_fit(w, &pairs)
}

/// The keys every page ends its key line with (pagesKeys).
pub const PAGES_KEYS: [&str; 4] = ["[ ]", "page", "esc", "back"];

/// A row, always one line, the highlighted one too, so moving the
/// highlight never shifts the page (settingRow).
pub fn setting_row(on: bool, st: &Setting, label_w: usize, value_w: usize, w: usize) -> Line {
    let wi = w as isize;
    let label_w = label_w.min(((wi - 10) / 2).max(8) as usize);
    let value_w = value_w.min((wi - label_w as isize - 8).max(1) as usize);
    let mut l;
    if let Some(f) = &st.line {
        l = f(w.saturating_sub(4));
    } else if st.choices.len() < 2 && !st.typed {
        if st.key.is_some() {
            let key = st.keys.first().map(String::as_str).unwrap_or("enter");
            l = line([sp(st.label.clone(), TEXT)]);
            if !st.value.is_empty() {
                l = line([
                    sp(fit_str(&st.label, label_w), TEXT),
                    sp(st.value.clone(), TEXT),
                ]);
            }
            l.spans.push(plain("  "));
            l.spans.push(sp(format!("[{key}]"), ORANGE));
        } else {
            let value = if st.choices.len() == 1 {
                st.shown()
            } else {
                st.value.clone()
            };
            l = line([sp(fit_str(&st.label, label_w), SUB), sp(value, TEXT)]);
        }
    } else {
        l = line([
            plain(fit_str(&st.label, label_w.saturating_sub(1).max(1))),
            plain(" "),
            sp("‹ ", FAINT),
            sp(fit_str(&st.shown(), value_w), TEXT),
            sp(" › ", FAINT),
        ]);
    }
    let l = trunc(l, w.saturating_sub(2));
    if on {
        let mut r = line([sp("▍", ORANGE), plain(" ")]);
        join(&mut r, l);
        return highlight(r, w);
    }
    let mut r = line([plain("  ")]);
    join(&mut r, l);
    r
}

/// How many rows About keeps, whatever is highlighted (aboutLines).
pub const ABOUT_LINES: usize = 4;

/// The highlighted row explained in a section of fixed height, so the
/// page keeps its shape (about).
pub fn about(st: &Setting, w: usize) -> Vec<Line> {
    let (title, what, now) = st.explained();
    let mut body = Vec::new();
    let mut add = |c, text: &str| {
        for l in wrap(text, w.saturating_sub(4)) {
            body.push(line([plain("  "), sp(l, c)]));
        }
    };
    if !what.is_empty() {
        add(SUB, &what);
    }
    if !now.is_empty() {
        add(TEXT, &now);
    }
    let mut out = vec![
        Line::new(),
        trunc(rule(&format!("About {title}"), "", w), w),
    ];
    for i in 0..ABOUT_LINES {
        out.push(body.get(i).cloned().unwrap_or_default());
    }
    if let Some(p) = &st.preview {
        out.push(rule("Preview", "as it looks now · ←→ to change it", w));
        for l in p(w.saturating_sub(2), 12) {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            out.push(r);
        }
    }
    out
}
