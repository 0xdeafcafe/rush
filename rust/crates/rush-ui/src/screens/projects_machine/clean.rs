//! Clean up: every worktree the agents' repositories have, what's stale in
//! /tmp and sessions' temp folders, as a checklist. What can go without
//! losing anything comes ticked; one confirmation (the Delete sheet) takes
//! the lot. Go: internal/ui/cleansheet.go (cleanSheet :21, openCleanSheet
//! :46, ticked :125, body :151, group :182, tempsSize :193, itemLine :201,
//! key :258, safe :285, confirm :297, nudgeClean :322). What agents left
//! outside their projects (fleet.LeftItem) isn't read yet, so that group
//! never shows.

use rush_core::text::Line;
use rush_fleet::{VerdictKind, Worktree};

use super::data::*;
use super::delete::{Doomed, open_delete};
use super::rows::*;
use super::scan;
use crate::frame::picker::round_move;
use crate::frame::sheet::{Sheet, sheet_row, sheet_title, window};
use crate::frame::{Out, Press};
use crate::list::FINISHED;
use crate::model::Model;
use crate::style::*;

/// Kinds of sessions' temp folders, as Clean up lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Temps {
    Done,
    Stopped,
    Running,
}

impl Temps {
    fn word(self) -> &'static str {
        match self {
            Temps::Done => "done",
            Temps::Stopped => "stopped",
            Temps::Running => "running",
        }
    }
}

/// A worktree, sessions' temp folders of one kind, or /tmp's untouched
/// scratch when neither.
#[derive(Clone, Debug)]
pub(crate) struct CleanItem {
    pub wt: Option<Worktree>,
    pub on: bool,
    /// An agent still running in it: it can't be ticked.
    pub busy: String,
    /// Sessions' temp folders (key, name, bytes), all of one kind.
    pub temps: Vec<(String, String, i64)>,
    pub kind: Option<Temps>,
}

impl CleanItem {
    fn group(&self) -> &'static str {
        if self.wt.is_some() {
            "Worktrees"
        } else {
            "Temporary"
        }
    }

    fn temps_size(&self) -> i64 {
        self.temps.iter().map(|t| t.2).sum()
    }

    fn safe(&self) -> bool {
        if let Some(k) = self.kind {
            return k == Temps::Done;
        }
        self.busy.is_empty()
            && self
                .wt
                .as_ref()
                .is_none_or(|w| w.own && w.verdict().can_go())
    }
}

#[derive(Clone)]
pub(crate) struct CleanSheet {
    pub items: Vec<CleanItem>,
    pub cur: usize,
}

/// Opens Clean up, once git has looked at the worktrees.
pub(crate) fn open_clean_sheet(m: &mut Model) {
    let d = data(m);
    if d.checked == 0 {
        m.flash("still looking at worktrees with git · a moment");
        scan::scan_worktrees(m);
        return;
    }
    let mut items: Vec<CleanItem> = Vec::new();
    for w in &d.wts {
        // Ticked as the rule says it can go; one rush didn't make, only by
        // hand.
        let mut it = CleanItem {
            wt: Some(w.clone()),
            on: w.own && w.verdict().can_go(),
            busy: String::new(),
            temps: Vec::new(),
            kind: None,
        };
        if let Some(g) = running(m, &w.agents) {
            let n = one_line(&g.a.display_name);
            it.busy = if n.is_empty() { "an agent".into() } else { n };
            it.on = false;
        }
        items.push(it);
    }
    // Ticked first, the biggest of them on top; then the rest by size.
    items.sort_by(|a, b| {
        b.on.cmp(&a.on).then(
            b.wt.as_ref()
                .map_or(0, |w| w.size)
                .cmp(&a.wt.as_ref().map_or(0, |w| w.size)),
        )
    });
    if d.tmp.stale_items > 0 {
        items.push(CleanItem {
            wt: None,
            on: true,
            busy: String::new(),
            temps: Vec::new(),
            kind: None,
        });
    }
    // Sessions' own temp folders: a done one's goes, ticked; a stopped
    // one's may be wanted when it's resumed; a running one's is in use.
    let mut by: [Vec<(String, String, i64)>; 3] = Default::default();
    for g in agents(m) {
        let t = d.temp_of(&g.a.key);
        if t < TEMP_SHOWN {
            continue;
        }
        let k = if g.a.pid != 0 || g.busy() {
            2
        } else if m.group_of.get(&g.a.key).map(String::as_str) == Some(FINISHED) {
            0
        } else {
            1
        };
        by[k].push((g.a.key.clone(), g.a.display_name.clone(), t));
    }
    for (k, temps) in [Temps::Done, Temps::Stopped, Temps::Running]
        .into_iter()
        .zip(by)
    {
        if temps.is_empty() {
            continue;
        }
        items.push(CleanItem {
            wt: None,
            on: k == Temps::Done,
            busy: if k == Temps::Running {
                "in use".into()
            } else {
                String::new()
            },
            temps,
            kind: Some(k),
        });
    }
    if items.is_empty() {
        m.flash(
            "no worktrees, nothing left outside projects and nothing stale in /tmp · nothing to clean",
        );
        return;
    }
    m.open_sheet(Box::new(CleanSheet { items, cur: 0 }));
}

impl CleanSheet {
    /// How many are ticked, how many of those would lose work, and how much
    /// they free.
    pub fn ticked(&self, m: &Model) -> (usize, usize, i64) {
        let (mut n, mut losing, mut size) = (0, 0, 0);
        for it in self.items.iter().filter(|it| it.on) {
            n += 1;
            if it.kind.is_some() {
                size += it.temps_size();
                continue;
            }
            match &it.wt {
                None => size += data(m).tmp.stale,
                Some(w) => {
                    size += w.size;
                    if !w.safe() {
                        losing += 1;
                    }
                }
            }
        }
        (n, losing, size)
    }

    fn item_line(&self, m: &Model, it: &CleanItem, w: usize) -> Line {
        let mut bx = if it.on {
            sp("✓ ", GREEN)
        } else {
            sp("○ ", FAINT)
        };
        if let Some(k) = it.kind {
            let what = match k {
                Temps::Done => sp("sessions done", GREEN),
                Temps::Stopped => sp("stopped, not done: may be resumed", YELLOW),
                Temps::Running => sp("in use", DIM),
            };
            if !it.busy.is_empty() {
                bx = sp("· ", FAINT);
            }
            let n = it.temps.len();
            let name = format!("$TMPDIR of {n} {} session{}", k.word(), plural(n));
            return fit(
                ln([
                    bx,
                    sp(pad_to(&name, 44), TEXT),
                    sp(format!("{:>7}  ", disk(it.temps_size())), FAINT),
                    what,
                ]),
                w,
            );
        }
        let Some(wt) = &it.wt else {
            let t = data(m).tmp.clone();
            return fit(
                ln([
                    bx,
                    sp(pad_to("/tmp", 28), TEXT),
                    sp(pad_to("", 16), DIM),
                    sp(format!("{:>7}  ", disk(t.stale)), FAINT),
                    sp(
                        format!("{} things untouched for a day", t.stale_items),
                        GREEN,
                    ),
                ]),
                w,
            );
        };
        let name = rush_draw::truncate(base(&wt.path), 27, "…").into_owned();
        let repo = rush_draw::truncate(base(&wt.repo), 15, "…").into_owned();
        let v = wt.verdict();
        let why = if !it.busy.is_empty() {
            bx = sp("· ", FAINT);
            ln([sp(format!("{} is running in it", it.busy), DIM)])
        } else if !wt.err.is_empty() {
            ln([sp(wt.err.clone(), RED)])
        } else if v.kind == VerdictKind::WouldLose {
            ln([sp(format!("would lose {}", v.reason), YELLOW)])
        } else if v.kind == VerdictKind::CanGo && !wt.own {
            ln([
                sp(v.reason.clone(), GREEN),
                sp(" · not rush's: yours to tick", FAINT),
            ])
        } else if v.kind == VerdictKind::CanGo {
            ln([sp(v.reason.clone(), GREEN)])
        } else {
            ln([sp(v.reason.clone(), DIM)])
        };
        fit(
            cat(
                ln([
                    bx,
                    sp(pad_to(&name, 28), TEXT),
                    sp(pad_to(&repo, 16), DIM),
                    sp(format!("{:>7}  ", disk(wt.size)), FAINT),
                ]),
                why,
            ),
            w,
        )
    }

    /// Opens Delete on everything ticked, which says what each takes; esc
    /// there comes back here.
    fn confirm(&self, m: &mut Model) -> Out {
        let mut picked = Vec::new();
        for it in self.items.iter().filter(|it| it.on) {
            if it.kind.is_some() {
                picked.push(Doomed {
                    agents: it
                        .temps
                        .iter()
                        .map(|(k, n, _)| (k.clone(), n.clone()))
                        .collect(),
                    ..Doomed::default()
                });
                continue;
            }
            picked.push(Doomed {
                tmp: it.wt.is_none(),
                wt: it.wt.clone(),
                ..Doomed::default()
            });
        }
        if picked.is_empty() {
            m.flash("nothing ticked · space ticks one, a ticks all that's safe");
            return Out::Draw;
        }
        open_delete(m, Some(Box::new(self.clone())), picked);
        Out::Draw
    }
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// s padded with spaces to n cells (Go's %-ns).
fn pad_to(s: &str, n: usize) -> String {
    let w = rush_core::text::width(s);
    format!("{s}{}", " ".repeat(n.saturating_sub(w)))
}

impl Sheet for CleanSheet {
    fn width(&self, _m: &Model) -> usize {
        130
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let (n, losing, size) = self.ticked(m);
        let mut about = format!("{n} ticked · {}", disk(size));
        if losing > 0 {
            about += &format!(" · {losing} would lose work");
        }
        if losing == 0 && n > 0 {
            about += " · loses nothing";
        }
        let mut out = vec![sheet_title("Clean up", &about, w), Line::new()];
        let (from, to) = window(self.items.len(), self.cur, 1.max(h.saturating_sub(10)));
        let edge = sp("┃", BLUE);
        let mut group = "";
        for i in from..to {
            let it = &self.items[i];
            if it.group() != group {
                group = it.group();
                out.push(ln([
                    plain("  "),
                    edge.clone(),
                    plain(" "),
                    spb(group, TEXT),
                ]));
            }
            let last = i + 1 == self.items.len() || self.items[i + 1].group() != group;
            let branch = if last { "└─" } else { "├─" };
            let l = cat(
                ln([edge.clone(), plain(" "), sp(branch, FAINT), plain(" ")]),
                self.item_line(m, it, w.saturating_sub(8)),
            );
            out.push(sheet_row(l, i == self.cur, w));
        }
        out.extend([
            Line::new(),
            ln([sp(
                "on its own: when a PR merges, and 3h after a session is done · keeps the newest 3 per repo · worktrees rush didn't make only by hand · branches stay",
                DIM,
            )]),
            Line::new(),
            keys_fit(
                w,
                &[
                    ("↑↓", "move"),
                    ("space", "tick"),
                    ("a", "all safe / none"),
                    ("enter", "remove ticked"),
                    ("esc", "close"),
                ],
            ),
        ]);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let n = self.items.len();
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" => return Out::Close,
            "up" | "k" | "shift+tab" => self.cur = round_move(self.cur, -1, n),
            "down" | "j" | "tab" => self.cur = round_move(self.cur, 1, n),
            "space" | " " | "x" => {
                if let Some(it) = self.items.get_mut(self.cur)
                    && it.busy.is_empty()
                {
                    it.on = !it.on;
                }
            }
            "a" => {
                // All that's safe, or none when that's what's ticked already.
                let all = self.items.iter().all(|it| it.on == it.safe());
                for it in &mut self.items {
                    it.on = !all && it.safe();
                }
            }
            "enter" => return self.confirm(m),
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

/// Says, once a day at most, when there's a lot that could go: the prompt
/// to open Clean up. Remembered only while rush runs.
pub(crate) fn nudge_clean(m: &mut Model) {
    let d = data(m);
    if m.now - d.nudged < 24 * 3600 * 1000 || m.place == crate::frame::PlaceId::Projects {
        return;
    }
    let (mut n, mut size) = (0, 0);
    for w in &d.wts {
        if w.verdict().can_go() && running(m, &w.agents).is_none() {
            n += 1;
            size += w.size;
        }
    }
    size += d.tmp.stale;
    if n < 5 && size < 2 << 30 {
        return;
    }
    let items = d.tmp.stale_items;
    m.kept::<Data>().nudged = m.now;
    m.news(
        &format!(
            "{} could be cleaned up without losing anything: {n} worktrees merged and clean, {items} things in /tmp · Projects, c cleans up",
            disk(size)
        ),
        "",
    );
}
