//! A plugin's list to choose from, as the plugin gave it: tabs that [ and
//! ] go through, filtered as you type, each action on its key. What's
//! chosen goes back to the plugin; the sheet only draws. After Go's
//! internal/ui/pluginpick.go (pickSheet :17, openPick :27, shown :38,
//! body :62, key :112, picked :159).

use std::collections::HashSet;

use rush_core::text::Line;
use rush_plugin::{Pick, PickItem, Picked};

use super::{STASH_PLUGIN, hooks, short_images};
use crate::frame::sheet::{sheet_row, sheet_tabs, sheet_title, text_field, window};
use crate::frame::{Out, Press, Sheet, editor};
use crate::model::Model;
use crate::style::*;

/// A plugin's pick on screen (pickSheet).
pub struct PickSheet {
    pub plugin: String,
    pub p: Pick,
    pub tab: i64,
    pub cur: usize,
    pub filter: Vec<char>,
    /// Taken out by an action that stays.
    pub gone: HashSet<String>,
}

/// Shows a plugin's pick, if nothing else has the screen (openPick).
pub fn open(m: &mut Model, plugin: &str, p: Pick) {
    if m.sheet.is_some() || m.confirm.is_some() {
        return;
    }
    m.open_sheet(Box::new(PickSheet::new(plugin, p)));
}

impl PickSheet {
    pub fn new(plugin: &str, p: Pick) -> PickSheet {
        PickSheet {
            plugin: plugin.to_string(),
            tab: p.tab,
            p,
            cur: 0,
            filter: Vec::new(),
            gone: HashSet::new(),
        }
    }

    /// The tab's items that match the filter, as the plugin ordered them
    /// (shown).
    pub fn shown(&self) -> Vec<&PickItem> {
        let q = self.filter.iter().collect::<String>().trim().to_lowercase();
        self.p
            .items
            .iter()
            .filter(|it| it.tab == self.tab && !self.gone.contains(&it.id))
            .filter(|it| {
                q.is_empty()
                    || it.text.to_lowercase().contains(&q)
                    || it.meta.to_lowercase().contains(&q)
            })
            .collect()
    }

    fn count(&self, tab: i64) -> usize {
        self.p
            .items
            .iter()
            .filter(|it| it.tab == tab && !self.gone.contains(&it.id))
            .count()
    }
}

impl Sheet for PickSheet {
    fn width(&self, _m: &Model) -> usize {
        112
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut title = sheet_title(
            &self.p.title,
            &self.p.about,
            w.saturating_sub(self.plugin.len() + 3),
        );
        if self.plugin == STASH_PLUGIN {
            title = sheet_title(&self.p.title, &self.p.about, w); // rush's own
        } else {
            title.spans.push(sp(format!(" · {}", self.plugin), FAINT));
        }
        let mut out = vec![title, Line::new()];
        if !self.p.tabs.is_empty() {
            let tabs: Vec<String> = self
                .p
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| format!("{t} {}", self.count(i as i64)))
                .collect();
            let names: Vec<&str> = tabs.iter().map(String::as_str).collect();
            out.push(sheet_tabs(&names, self.tab.max(0) as usize));
            out.push(Line::new());
        }
        let mut find = line([sp("find ", DIM)]);
        join(
            &mut find,
            text_field(
                &self.filter,
                self.filter.len(),
                true,
                "type to search",
                w.saturating_sub(5),
            ),
        );
        out.push(find);
        out.push(Line::new());
        let n = self.shown().len();
        self.cur = self.cur.min(n.saturating_sub(1));
        let list = self.shown();
        if list.is_empty() {
            let mut msg = "nothing here".to_string();
            if let Some(e) = self.p.empty.get(self.tab.max(0) as usize)
                && !e.is_empty()
            {
                msg = e.clone();
            }
            if !self.filter.is_empty() {
                msg = "none of these has that".into();
            }
            out.push(line([plain("  "), sp(msg, FAINT)]));
        }
        let (from, to) = window(
            list.len(),
            self.cur,
            3.max(h as isize - out.len() as isize - 4) as usize,
        );
        for (i, it) in list.iter().enumerate().take(to).skip(from) {
            let meta = it.meta.as_str();
            let first = it.text.trim().split('\n').next().unwrap_or("");
            let mw = rush_core::text::width(meta);
            let room = 8.max(w as isize - 4 - mw as isize - 4) as usize;
            let text = rush_draw::truncate(&short_images(first), room, "…").into_owned();
            let pad = 1.max(w as isize - 4 - rush_core::text::width(&text) as isize - mw as isize);
            let row = line([sp(text, TEXT), plain(blanks(pad as usize)), sp(meta, FAINT)]);
            out.push(sheet_row(row, i == self.cur, w));
        }
        if let Some(it) = list.get(self.cur) {
            let lines: Vec<&str> = it.text.trim().split('\n').collect();
            if lines.len() > 1 {
                let more = lines[1..lines.len().min(3)].join(" ⏎ ");
                out.push(Line::new());
                out.push(line([sp(
                    rush_draw::truncate(&more, w, "…").into_owned(),
                    FAINT,
                )]));
            }
        }
        let mut pairs: Vec<(String, String)> = vec![("↑↓".into(), "choose".into())];
        for a in &self.p.actions {
            pairs.push((a.key.clone(), a.name.clone()));
        }
        if self.p.tabs.len() > 1 {
            pairs.push(("[ ]".into(), self.p.tabs.join(" · ").to_lowercase()));
        }
        pairs.push(("esc".into(), "close".into()));
        let pairs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        out.push(Line::new());
        out.push(keys_fit(w, &pairs));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let n = self.shown().len();
        let s = k.s.as_str();
        match s {
            "esc" | "ctrl+c" => return Out::Close,
            "[" | "]" => {
                let t = self.p.tabs.len() as i64;
                if t > 1 {
                    self.tab = (self.tab + if s == "]" { 1 } else { t - 1 }) % t;
                    self.cur = 0;
                }
                return Out::Draw;
            }
            "up" | "ctrl+p" => {
                self.cur = self.cur.saturating_sub(1);
                return Out::Draw;
            }
            "down" | "ctrl+n" => {
                self.cur = (self.cur + 1).min(n.saturating_sub(1));
                return Out::Draw;
            }
            "pgup" => {
                self.cur = self.cur.saturating_sub(10);
                return Out::Draw;
            }
            "pgdown" => {
                self.cur = (self.cur + 10).min(n.saturating_sub(1));
                return Out::Draw;
            }
            _ => {}
        }
        if let Some(a) = self.p.actions.iter().find(|a| a.key == s).cloned() {
            let Some(id) = self.shown().get(self.cur).map(|it| it.id.clone()) else {
                return Out::Idle;
            };
            picked(m, &self.plugin, &self.p, &id, &a.key);
            if a.stay {
                self.gone.insert(id);
                return Out::Draw;
            }
            return Out::Close;
        }
        if let Some((buf, pos)) = editor::edit(&self.filter, self.filter.len(), k)
            && pos == buf.len()
        {
            self.filter = buf;
            self.cur = 0;
        }
        Out::Draw
    }
}

/// Tells the plugin what was chosen, with the box it's about when that
/// box is the one with the keys (picked).
pub fn picked(m: &mut Model, plugin: &str, p: &Pick, item: &str, action: &str) {
    let Some(h) = m.hooks() else { return };
    let mut ch = Picked {
        plugin: plugin.to_string(),
        pick: p.id.clone(),
        item: item.to_string(),
        action: action.to_string(),
        box_id: p.session.clone(),
        ..Picked::default()
    };
    if let Some((b, who)) = hooks::box_state(m)
        && who == p.session
    {
        ch.input = Some(b);
    }
    let name = plugin.to_string();
    m.later(
        move || h.picked(ch),
        move |m, r| {
            if let Err(e) = r {
                m.flash_err(&format!("{name}: {e}"));
                return Out::Draw;
            }
            Out::Idle
        },
    );
}
