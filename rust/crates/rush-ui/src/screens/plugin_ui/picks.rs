//! The plugins that are off until you want them. Optional bundled
//! plugins are useful but not for everyone all the time, so they start
//! off. rush asks which you want once, on the first run, and again for
//! any that's new since; each is asked about once. After Go's
//! internal/ui/pluginpicks.go (optPick :23, checkOptional :38,
//! optionalFound :53, gotOptional :74, markAsked :85, openOptional :96,
//! optionalSheet :106, body :116, key :181, apply :207).

use std::collections::BTreeMap;

use rush_core::text::Line;
use rush_plugin::{Bundle, CATEGORIES};

use super::{settled, sheet_do};
use crate::frame::sheet::{sheet_row, sheet_title, window};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// One optional plugin to ask about (optPick).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptPick {
    pub name: String,
    pub about: String,
    pub category: String,
    /// Ticked in the sheet.
    pub on: bool,
}

/// What [`found`] made of the bundles: the plugins to ask about, and the
/// ones already on, which count as asked (optionalMsg).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub picks: Vec<OptPick>,
    pub already_on: Vec<String>,
}

/// How long after start optional plugins are looked for (Go's fifth
/// tick).
pub const LOOK_AFTER: rush_core::time::Ms = 5000;

/// Looks, off the UI, for optional plugins not asked about yet, once a
/// few seconds after start; until the sheet can open, each look tries
/// again (checkOptional).
pub fn check(m: &mut Model) {
    if m.plugins().opt_picks.is_some() {
        open(m);
        return;
    }
    let started = m.plugins().started;
    if m.plugins().opt_checked || started == 0 || rush_core::time::now() - started < LOOK_AFTER {
        return;
    }
    m.plugins().opt_checked = true;
    let disk = m.plugins().disk.clone();
    m.later(
        move || {
            let (asked, steps) = disk.onboarding();
            let f = found(&asked, &disk.bundles(), &disk.bundles_on());
            if !f.already_on.is_empty() {
                disk.mark_asked(&f.already_on);
            }
            let n = asked.len() + f.already_on.len();
            (f, n, steps.len())
        },
        |m, (f, asked, steps)| {
            if !f.picks.is_empty() {
                m.plugins().opt_picks = Some(f.picks);
                m.kept::<Asked>().0 = Some((asked, steps));
                open(m);
            }
            Out::Draw
        },
    );
}

/// How many plugins were asked about before, and onboarding steps done,
/// for the sheet's words.
#[derive(Default)]
struct Asked(Option<(usize, usize)>);

/// The optional plugins not in asked that run here, by category, and the
/// ones you have on already (optionalFound).
pub fn found(asked: &[String], bundles: &[Bundle], on: &BTreeMap<String, bool>) -> Found {
    let mut out = Found::default();
    for b in bundles {
        let n = &b.manifest.name;
        if !b.optional || asked.contains(n) || !b.manifest.unmet().is_empty() {
            continue;
        }
        if on.get(n).copied().unwrap_or(false) {
            out.already_on.push(n.clone());
            continue;
        }
        out.picks.push(OptPick {
            name: n.clone(),
            about: b.manifest.description.clone(),
            category: b.manifest.category_of().title.to_string(),
            on: false,
        });
    }
    let order = |c: &str| CATEGORIES.iter().position(|x| x.title == c);
    out.picks.sort_by_key(|p| order(&p.category));
    out
}

/// Opens the sheet once the screen is free (openOptional).
pub fn open(m: &mut Model) {
    if !m.screen_free() {
        return;
    }
    let Some(picks) = m.plugins().opt_picks.take() else {
        return;
    };
    if picks.is_empty() {
        return;
    }
    let (asked, steps) = m.kept::<Asked>().0.unwrap_or((0, 0));
    m.open_sheet(Box::new(OptionalSheet {
        picks,
        never: asked == 0,
        first: asked == 0 && steps == 0,
        cur: 0,
        busy: false,
    }));
}

/// Ticks which optional plugins to turn on (optionalSheet).
pub struct OptionalSheet {
    pub picks: Vec<OptPick>,
    /// The first run.
    pub first: bool,
    /// Never asked before, on the first run or not.
    pub never: bool,
    pub cur: usize,
    pub busy: bool,
}

impl Sheet for OptionalSheet {
    fn width(&self, _m: &Model) -> usize {
        100
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut text = if self.first {
            "Welcome to rush. These plugins come with it. "
        } else if self.never {
            "These plugins come with rush. "
        } else {
            "New plugins came with rush, or can run here now. "
        }
        .to_string();
        text += "They're off until you want them: useful, but not for everyone all the time. \
                 Tick the ones you want; Settings › Plugins turns any of them on or off later.";
        let mut out = vec![
            sheet_title("Plugins", "off until you want them", w),
            Line::new(),
        ];
        for l in wrap(&text, w.saturating_sub(2)) {
            out.push(line([plain("  "), plain(l)]));
        }
        out.push(Line::new());
        // Each row: its tick, its name and its description cut to the
        // line; the one under the cursor is described in full below the
        // list. The list and the description keep the height of the
        // longest, so the box doesn't grow and shrink as the cursor moves.
        let about = self
            .picks
            .get(self.cur)
            .map(|p| wrap(&p.about, w.saturating_sub(4)))
            .unwrap_or_default();
        let (mut about_h, mut cats) = (0, 0);
        for (i, p) in self.picks.iter().enumerate() {
            about_h = about_h.max(wrap(&p.about, w.saturating_sub(4)).len());
            if i == 0 || p.category != self.picks[i - 1].category {
                cats += 1;
            }
        }
        let room =
            3.max(h as isize - out.len() as isize - about_h as isize - 5 - cats as isize) as usize;
        let (from, to) = window(self.picks.len(), self.cur, room);
        let list_end = out.len() + self.picks.len().min(room) + cats;
        let mut cat = "";
        for i in from..to {
            let p = &self.picks[i];
            if p.category != cat {
                cat = &p.category;
                out.push(line([plain("  "), sp(cat, FAINT)]));
            }
            let tick = if p.on {
                sp("● ", GREEN)
            } else {
                sp("○ ", DIM)
            };
            let l = line([
                tick,
                spb(format!("{:<19}", p.name), TEXT),
                sp(
                    rush_draw::truncate(&p.about, w.saturating_sub(26), "…").into_owned(),
                    DIM,
                ),
            ]);
            out.push(sheet_row(l, i == self.cur, w));
        }
        while out.len() < list_end {
            out.push(Line::new());
        }
        out.push(Line::new());
        for l in &about {
            out.push(line([plain("    "), sp(l.clone(), DIM)]));
        }
        for _ in about.len()..about_h {
            out.push(Line::new());
        }
        let k = if self.busy {
            line([sp("⋯ turning them on", YELLOW)])
        } else {
            keys_fit(
                w,
                &[
                    ("↑↓", "move"),
                    ("space", "tick"),
                    ("enter", "turn on ticked"),
                    ("esc", "not now"),
                ],
            )
        };
        out.push(Line::new());
        out.push(k);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if self.busy {
            return Out::Idle;
        }
        match k.s.as_str() {
            "up" | "shift+tab" | "k" => self.cur = self.cur.saturating_sub(1),
            "down" | "tab" | "j" => {
                self.cur = (self.cur + 1).min(self.picks.len().saturating_sub(1))
            }
            "space" | " " | "x" => {
                if let Some(p) = self.picks.get_mut(self.cur) {
                    p.on = !p.on;
                }
            }
            "enter" => return self.apply(m),
            "esc" | "ctrl+c" | "n" => {
                for p in &mut self.picks {
                    p.on = false;
                }
                return self.apply(m);
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

impl OptionalSheet {
    /// Turns on what's ticked, and records every plugin shown as asked
    /// about, so none is asked about again (apply).
    fn apply(&mut self, m: &mut Model) -> Out {
        let shown: Vec<String> = self.picks.iter().map(|p| p.name.clone()).collect();
        let on: Vec<String> = self
            .picks
            .iter()
            .filter(|p| p.on)
            .map(|p| p.name.clone())
            .collect();
        let disk = m.plugins().disk.clone();
        if on.is_empty() {
            m.later(move || disk.mark_asked(&shown), |_, _| Out::Idle);
            m.flash("no plugins turned on · Settings › Plugins has them");
            return Out::Close;
        }
        self.busy = true;
        sheet_do(
            m,
            move || {
                disk.mark_asked(&shown);
                let mut failed = Vec::new();
                for n in &on {
                    if let Err(e) = disk.set_bundled(n, true) {
                        failed.push(format!("{n}: {e}"));
                    }
                }
                let _ = disk.reload();
                (on.len(), failed)
            },
            OptionalSheet::done,
        );
        settled(self, m, OptionalSheet::done).unwrap_or(Out::Draw)
    }

    fn done(&mut self, m: &mut Model, (n, failed): (usize, Vec<String>)) -> Out {
        if let Some(f) = failed.first() {
            m.flash_err(&format!("couldn't turn on {f}"));
        } else {
            m.flash(&format!("{n} plugins on · Settings › Plugins changes them"));
        }
        Out::Close
    }
}
