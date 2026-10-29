//! A plugin a project you're working in ships: Install or Not now.
//! Installing runs the project's install.sh, as you; what it builds then
//! waits on approval like any plugin. After Go's
//! internal/ui/pluginoffer.go (checkProjectPlugins :24, openPluginOffer
//! :49, pluginOfferSheet :61, body :70, key :115, pick :134).

use std::collections::BTreeSet;
use std::path::Path;

use rush_core::text::Line;
use rush_plugin::{Offer, short_commit};

use super::{approval, choice, settled, sheet_do};
use crate::frame::sheet::{sheet_row, sheet_title};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// Looks, off the UI, in the repositories open sessions work in whenever
/// that set changes, and again every few minutes (a pull can bring a
/// newer commit of a plugin); what's found is offered once the screen is
/// free (checkProjectPlugins).
pub fn check(m: &mut Model) {
    open(m);
    let mut seen = BTreeSet::new();
    for a in m.agents.iter().filter(|a| !a.past) {
        seen.insert(a.repo.clone());
    }
    seen.remove("");
    let roots: Vec<String> = seen.into_iter().collect();
    let key = roots.join("\x00");
    let now = m.now;
    let p = m.plugins();
    if key == p.offer_roots && now - p.offer_at < 3 * 60_000 {
        return;
    }
    p.offer_roots = key;
    p.offer_at = now;
    let disk = p.disk.clone();
    m.later(
        move || disk.in_projects(&roots),
        |m, offers| {
            m.plugins().offers = offers;
            open(m);
            Out::Draw
        },
    );
}

/// Offers the first waiting, unless something already has the screen
/// (openPluginOffer).
pub fn open(m: &mut Model) {
    if !m.screen_free() {
        return;
    }
    let Some(o) = m.plugins().offers.first().cloned() else {
        return;
    };
    // It comes unasked and Install runs the project's code: an enter
    // meant for the list mustn't pick it.
    m.open_sheet(Box::new(OfferSheet::new(o)));
}

/// Install or Not now for a plugin a project ships (pluginOfferSheet).
pub struct OfferSheet {
    pub o: Offer,
    /// 0 Install, 1 Not now.
    pub cur: usize,
    pub busy: bool,
    pub err: String,
}

impl OfferSheet {
    pub fn new(o: Offer) -> OfferSheet {
        OfferSheet {
            o,
            cur: 1,
            busy: false,
            err: String::new(),
        }
    }
}

fn base(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

impl Sheet for OfferSheet {
    fn width(&self, _m: &Model) -> usize {
        100
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let o = &self.o;
        let mut what = if o.update {
            "ships a new version of"
        } else {
            "ships a plugin"
        };
        let mut title = o.name.clone();
        if !o.version.is_empty() {
            title += &format!(" {}", o.version);
        }
        if !o.was.is_empty() {
            what = "has changed";
            title += &format!(" · {} → {}", short_commit(&o.was), short_commit(&o.commit));
            if o.commits > 0 {
                let s = if o.commits != 1 { "s" } else { "" };
                title += &format!(" ({} commit{s})", o.commits);
            }
        }
        let project = base(&o.project);
        let mut out = vec![
            sheet_title("Plugin", &format!("{title} · in {project}"), w),
            Line::new(),
        ];
        let mut text = format!("{project} {what} {}. {}", o.name, o.description);
        if !o.was.is_empty() {
            text = format!(
                "{} was installed at commit {}; {project} is now at {}, and the plugin's files changed in between. {}",
                o.name,
                short_commit(&o.was),
                short_commit(&o.commit),
                o.description
            );
        }
        text += &format!(
            "\n\nInstalling runs this, as you and outside the sandbox:\n  sh {}\n\nThen rush shows what the plugin may do, and asks you to approve it.",
            tildify(&o.install_script())
        );
        for l in wrap(&text, w.saturating_sub(2)) {
            out.push(line([plain("  "), plain(l)]));
        }
        out.push(Line::new());
        let choices = [
            ("Install", "build it, then review what it may do"),
            ("Not now", "asks again when a newer commit changes it"),
        ];
        for (i, (what, about)) in choices.iter().enumerate() {
            out.push(sheet_row(choice(what, about), i == self.cur, w));
        }
        if !self.err.is_empty() {
            out.push(Line::new());
            out.push(line([sp(self.err.clone(), RED)]));
        }
        let k = if self.busy {
            line([sp("⋯ installing", YELLOW)])
        } else {
            keys_fit(
                w,
                &[("↑↓", "choose"), ("enter", "pick"), ("esc", "not now")],
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
            "up" | "shift+tab" | "down" | "tab" => {
                self.cur = 1 - self.cur;
                Out::Draw
            }
            "y" => {
                self.cur = 0;
                self.pick(m)
            }
            "n" | "esc" | "ctrl+c" => {
                self.cur = 1;
                self.pick(m)
            }
            "enter" => self.pick(m),
            _ => Out::Idle,
        }
    }
}

impl OfferSheet {
    fn pick(&mut self, m: &mut Model) -> Out {
        self.busy = true;
        self.err.clear();
        let disk = m.plugins().disk.clone();
        let (o, install) = (self.o.clone(), self.cur == 0);
        sheet_do(
            m,
            move || {
                if install {
                    disk.install(&o)
                } else {
                    disk.skip(&o)
                }
            },
            OfferSheet::done,
        );
        settled(self, m, OfferSheet::done).unwrap_or(Out::Draw)
    }

    fn done(&mut self, m: &mut Model, r: Result<(), String>) -> Out {
        if let Err(e) = r {
            self.busy = false;
            self.err = e;
            return Out::Draw;
        }
        let name = self.o.name.clone();
        m.plugins().offers.retain(|x| x.name != name);
        m.sheet = None;
        if self.cur == 0 {
            m.flash(&format!("{name} installed · approve it to run it"));
            approval::check(m);
        } else {
            m.flash(&format!("left {name} uninstalled"));
            open(m);
        }
        if m.sheet.is_some() {
            Out::Draw
        } else {
            Out::Close
        }
    }
}
