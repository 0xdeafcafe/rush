//! Clean up: every worktree the agents' repositories have, what's stale in
//! /tmp and sessions' temp folders, as a checklist. What can go without
//! losing anything comes ticked; one confirmation (the Delete sheet) takes
//! the lot. Go: internal/ui/cleansheet.go (cleanSheet :21, openCleanSheet
//! :46, ticked :125, body :151, group :182, tempsSize :193, itemLine :201,
//! key :258, safe :285, confirm :297, nudgeClean :322). What agents left
//! outside their projects (fleet.LeftItem) isn't read yet, so that group
//! never shows. Rust-only: what's in rush's scratch root that no session
//! owns (rush_fleet::find_temp_orphans), ticked once untouched for days
//! and no running session's id starts its name.
//!
//! Rust-only too: caches that rebuild themselves, measured off the UI once
//! the sheet is open (rush_fleet::survey), so rush offers them itself
//! rather than starting an agent to find them. The folders cleanup rules
//! name (rush_fleet::cache_rules: built in, and learned from cleanup
//! agents' plans), ticked when the rule says so (some only once untouched
//! for a while); the build caches of each worktree nobody works in that
//! isn't going whole (rush_fleet::caches_in: its top, apps/*/node_modules),
//! ticked once the worktree, outside node_modules, sat untouched for
//! rush_fleet::WT_IDLE and no process works in it; then the commands every
//! run ends with (pnpm store prune, docker's prunes). Each worktree shows
//! its group and why (rush_fleet::Triage: SAFE, CACHE-ONLY, NOISE-ONLY,
//! KEEP), and last, what rush never touches, biggest first, for you to
//! decide on: never ticked.

use rush_core::text::Line;
use rush_core::time::Ms;
use rush_fleet::{Class, How, VerdictKind, Worktree};

use super::data::*;
use super::delete::{CacheFound, Doomed, open_delete};
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
pub enum Temps {
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

/// A worktree, something an agent left outside its project, sessions'
/// temp folders of one kind, or /tmp's untouched scratch when none.
#[derive(Clone, Debug, Default)]
pub struct CleanItem {
    pub wt: Option<Worktree>,
    pub left: Option<rush_fleet::LeftItem>,
    pub on: bool,
    /// An agent still running in it: it can't be ticked.
    pub busy: String,
    /// Sessions' temp folders (key, name, bytes), all of one kind.
    pub temps: Vec<(String, String, i64)>,
    pub kind: Option<Temps>,
    /// What's in rush's scratch root that no session owns, and whether
    /// it's sat untouched for days (Rust-only).
    pub orphan: Option<rush_fleet::TempOrphan>,
    pub stale: bool,
    /// A cache that rebuilds itself, or a command every run ends with
    /// (Rust-only).
    pub cache: Option<CacheFound>,
    /// A worktree's group and why, once its facts are in (Rust-only).
    pub triage: Option<rush_fleet::Triage>,
    /// Something big rush never touches, for you to decide on: never
    /// ticked (Rust-only).
    pub never: Option<rush_fleet::FoundCache>,
}

impl CleanItem {
    fn group(&self) -> &'static str {
        if self.never.is_some() {
            "Yours to decide: rush never touches these"
        } else if self
            .cache
            .as_ref()
            .is_some_and(|c| matches!(c.found.how, How::Run(_)))
        {
            "Run last, once"
        } else if self.cache.is_some() {
            "Caches that rebuild themselves"
        } else if self.wt.is_some() {
            "Worktrees"
        } else if self.left.is_some() {
            "Left outside projects"
        } else if self.orphan.is_some() {
            "Rush's scratch no session owns"
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
        if let Some(l) = &self.left {
            return self.busy.is_empty() && l.safe();
        }
        if self.never.is_some() {
            return false;
        }
        if let Some(c) = &self.cache {
            return self.busy.is_empty() && c.found.ok;
        }
        if self.orphan.is_some() {
            return self.busy.is_empty() && self.stale;
        }
        let noise_only = self
            .triage
            .as_ref()
            .is_some_and(|t| t.class == Class::NoiseOnly);
        self.busy.is_empty()
            && self
                .wt
                .as_ref()
                .is_none_or(|w| w.own && (w.verdict().can_go() || noise_only))
    }
}

/// A worktree's group, as Clean up colours it.
fn class_color(c: Class) -> rush_core::text::Color {
    match c {
        Class::Safe => GREEN,
        Class::NoiseOnly => GREEN,
        Class::CacheOnly => YELLOW,
        Class::Keep => DIM,
    }
}

#[derive(Clone)]
pub struct CleanSheet {
    pub items: Vec<CleanItem>,
    pub cur: usize,
    /// The caches are still being measured.
    pub measuring: bool,
}

/// The caches as Clean up lists them: ticked when the rules say so, those
/// ticked first, biggest first; the commands every run ends with last, in
/// their order.
pub fn cache_items(found: Vec<CacheFound>) -> Vec<CleanItem> {
    let item = |c: CacheFound| {
        let mut it = CleanItem {
            stale: c.found.ok,
            cache: Some(c),
            ..CleanItem::default()
        };
        it.on = it.safe();
        it
    };
    let (cmds, caches): (Vec<CacheFound>, Vec<CacheFound>) = found
        .into_iter()
        .partition(|c| matches!(c.found.how, How::Run(_)));
    let mut items: Vec<CleanItem> = caches.into_iter().map(item).collect();
    items.sort_by(|a, b| {
        let size = |it: &CleanItem| it.cache.as_ref().map_or(0, |c| c.found.size);
        b.on.cmp(&a.on).then(size(b).cmp(&size(a)))
    });
    items.extend(cmds.into_iter().map(item));
    items
}

/// What rush never touches, as Clean up lists it: never ticked, and can't
/// be.
pub fn never_items(never: Vec<rush_fleet::FoundCache>) -> Vec<CleanItem> {
    never
        .into_iter()
        .map(|f| CleanItem {
            busy: f.note.clone(),
            never: Some(f),
            ..CleanItem::default()
        })
        .collect()
}

/// What rush never touches is listed from this size up, at most so many.
const NEVER_SHOWN: i64 = 1 << 30;
const NEVER_MAX: usize = 8;

/// The worktrees a survey reads the facts of (all but main checkouts), and
/// those whose build caches it measures: nobody works in them, git could
/// look, and they aren't ones rush removes whole.
pub fn survey_paths(m: &Model) -> (Vec<String>, Vec<String>) {
    let d = data(m);
    let wts: Vec<&Worktree> = d.wts.iter().filter(|w| w.path != w.repo).collect();
    let free = wts
        .iter()
        .filter(|w| {
            !(w.own && w.verdict().can_go())
                && running(m, &w.agents).is_none()
                && w.err.is_empty()
                && w.in_use.is_empty()
        })
        .map(|w| w.path.clone())
        .collect();
    (wts.iter().map(|w| w.path.clone()).collect(), free)
}

/// Everything a survey measures, and the biggest of what rush never
/// touches. Off the UI.
pub fn survey_all(
    wts: &[String],
    free: &[String],
    now: Ms,
) -> (rush_fleet::Survey, Vec<rush_fleet::FoundCache>) {
    let home = std::env::var("HOME").unwrap_or_default();
    (
        rush_fleet::survey(&rush_fleet::cache_rules(), wts, free, now),
        rush_fleet::never_touch_big(&home, NEVER_SHOWN, NEVER_MAX),
    )
}

/// Each worktree's group in the sheet, by the facts measured.
fn triage_items(
    items: &mut [CleanItem],
    facts: &std::collections::HashMap<String, rush_fleet::WtFacts>,
    now: Ms,
) {
    for it in items.iter_mut() {
        let Some(w) = &it.wt else { continue };
        let t = w.triage(facts.get(&w.path).copied(), now);
        let was = it.on;
        it.triage = Some(t);
        // An own worktree that's only noise is ticked once that's known.
        it.on = was || it.safe();
    }
}

/// Learns from cleanup agents' plans (rush_fleet::learn), then measures
/// the caches and the worktrees' facts for the Clean up sheet that's open
/// (rush_fleet::survey), and adds them when that's done; closes it, saying
/// so, when there's nothing at all. What rush never touches is measured
/// after, and listed last.
fn measure_into_sheet(
    m: &mut Model,
    wts: Vec<String>,
    free_wts: Vec<String>,
    plans: Vec<std::path::PathBuf>,
) {
    let now = m.now;
    m.later(
        move || {
            let learned: usize = plans
                .iter()
                .filter_map(|p| std::fs::read_to_string(p).ok())
                .filter_map(|t| rush_fleet::learn(&t).ok())
                .map(|n| n.len())
                .sum();
            let s = rush_fleet::survey(&rush_fleet::cache_rules(), &wts, &free_wts, now);
            (s, learned)
        },
        |m, (survey, learned)| {
            let found = survey.caches.clone();
            let d = m.kept::<Data>();
            d.wt_facts.extend(survey.facts.clone());
            d.survey = survey.caches;
            m.later(
                || {
                    let home = std::env::var("HOME").unwrap_or_default();
                    rush_fleet::never_touch_big(&home, NEVER_SHOWN, NEVER_MAX)
                },
                |m, never| {
                    m.kept::<Data>().never = never.clone();
                    if let Some(s) = m.sheet_as::<CleanSheet>() {
                        s.items.extend(never_items(never));
                    }
                    Out::Draw
                },
            );
            if learned > 0 {
                m.news(
                    &format!(
                        "rush learned {learned} cache{} from a cleanup agent's plan · Clean up offers {} from now on",
                        plural(learned),
                        if learned == 1 { "it" } else { "them" }
                    ),
                    "",
                );
            }
            let now = m.now;
            let facts = data(m).wt_facts.clone();
            let Some(s) = m.sheet_as::<CleanSheet>() else {
                return Out::Idle;
            };
            s.measuring = false;
            triage_items(&mut s.items, &facts, now);
            s.items.extend(cache_items(found));
            let empty = s.items.is_empty();
            if empty {
                m.sheet = None;
                m.flash(
                    "no worktrees, no caches, nothing left outside projects and nothing stale in /tmp · nothing to clean",
                );
            }
            Out::Draw
        },
    );
}

/// Opens Clean up, once git has looked at the worktrees.
pub fn open_clean_sheet(m: &mut Model) {
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
            ..CleanItem::default()
        };
        if let Some(g) = running(m, &w.agents) {
            let n = one_line(&g.a.display_name);
            it.busy = if n.is_empty() { "an agent".into() } else { n };
            it.on = false;
        }
        items.push(it);
    }
    // Each one's group by what the last survey measured; measured afresh
    // below.
    triage_items(&mut items, &d.wt_facts, m.now);
    // Ticked first, the biggest of them on top; then the rest by size.
    items.sort_by(|a, b| {
        b.on.cmp(&a.on).then(
            b.wt.as_ref()
                .map_or(0, |w| w.size)
                .cmp(&a.wt.as_ref().map_or(0, |w| w.size)),
        )
    });
    // What agents left outside their projects: ticked only when it's
    // scratch in a temp folder nothing has touched since; the rest you may
    // tick, unless it's held back (there before, changed since, git's).
    for l in d.outside.iter().filter(|l| !l.gone) {
        let mut it = CleanItem {
            left: Some(l.clone()),
            on: l.safe(),
            busy: l.why().to_string(),
            ..CleanItem::default()
        };
        if let Some(g) = agent_by_key(m, &l.agent.key)
            && g.a.pid != 0
        {
            let n = one_line(&g.a.display_name);
            let n = if n.is_empty() { "an agent".into() } else { n };
            it.busy = format!("{n} is running");
            it.on = false;
        }
        items.push(it);
    }
    if d.tmp.stale_items > 0 {
        items.push(CleanItem {
            on: true,
            ..CleanItem::default()
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
            on: k == Temps::Done,
            busy: if k == Temps::Running {
                "in use".into()
            } else {
                String::new()
            },
            temps,
            kind: Some(k),
            ..CleanItem::default()
        });
    }
    // Rush's scratch no session owns (a sibling a script left, a deleted
    // session's folder): ticked once untouched for days, unless a running
    // session's id starts its name; that one can't be ticked.
    let live = running_ids(m);
    for o in d.scratch_orphans.iter().filter(|o| !o.gone) {
        let busy = o
            .used_by(&live)
            .map(|id| format!("{} is running", session_name(m, id)))
            .unwrap_or_default();
        let stale = o.idle(m.now);
        items.push(CleanItem {
            on: stale && busy.is_empty(),
            busy,
            orphan: Some(o.clone()),
            stale,
            ..CleanItem::default()
        });
    }
    // The worktrees whose build caches are offered: nobody works in them,
    // git could look, and they aren't going whole; never a main checkout.
    let free_wts: Vec<String> = items
        .iter()
        .filter_map(|it| it.wt.as_ref().map(|w| (it, w)))
        .filter(|(it, w)| {
            !it.on
                && it.busy.is_empty()
                && w.err.is_empty()
                && w.in_use.is_empty()
                && w.path != w.repo
        })
        .map(|(_, w)| w.path.clone())
        .collect();
    let wts: Vec<String> = data(m)
        .wts
        .iter()
        .filter(|w| w.path != w.repo)
        .map(|w| w.path.clone())
        .collect();
    let measuring = !data(m).frozen;
    if items.is_empty() && !measuring {
        m.flash(
            "no worktrees, nothing left outside projects and nothing stale in /tmp · nothing to clean",
        );
        return;
    }
    m.open_sheet(Box::new(CleanSheet {
        items,
        cur: 0,
        measuring,
    }));
    if measuring {
        // Cleanup agents' plans, done or waiting on you: what they mark
        // conservative is learned before the rules are read.
        let plans: Vec<std::path::PathBuf> = m
            .agents
            .iter()
            .filter(|a| a.rush && a.started_by == super::care::CARE_PLUGIN && !a.job.id.is_empty())
            .map(|a| {
                std::path::Path::new(&rush_fleet::temp_dir(&a.job.id))
                    .join(super::care::CARE_PLAN_FILE)
            })
            .collect();
        measure_into_sheet(m, wts, free_wts, plans);
    }
}

impl CleanSheet {
    /// How many are ticked, how many of those would lose work, and how much
    /// they free.
    pub fn ticked(&self, m: &Model) -> (usize, usize, i64) {
        let (mut n, mut losing, mut size) = (0, 0, 0);
        for it in self.items.iter().filter(|it| it.on) {
            n += 1;
            if let Some(l) = &it.left {
                size += l.size;
                continue;
            }
            if let Some(o) = &it.orphan {
                size += o.size;
                continue;
            }
            if let Some(c) = &it.cache {
                size += c.found.size;
                continue;
            }
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
        if let Some(l) = &it.left {
            let mut why = if !it.busy.is_empty() {
                bx = sp("· ", FAINT);
                ln([sp(it.busy.clone(), DIM)])
            } else if !l.temp {
                ln([sp("outside temp: may be meant to stay", YELLOW)])
            } else {
                ln([sp("scratch, untouched since", GREEN)])
            };
            if l.shell {
                why.spans.push(sp(" · from a shell command", FAINT));
            }
            let path = rush_draw::truncate(&tildify(&l.path), 43, "…").into_owned();
            return fit(
                cat(
                    ln([
                        bx,
                        sp(pad_to(&path, 44), TEXT),
                        sp(format!("{:>7}  ", disk(l.size)), FAINT),
                        sp(format!("{} · ", one_line(&l.agent.display_name)), DIM),
                    ]),
                    why,
                ),
                w,
            );
        }
        if let Some(f) = &it.never {
            let path = rush_draw::truncate(&tildify(&f.path), 43, "…").into_owned();
            return fit(
                ln([
                    sp("· ", FAINT),
                    sp(pad_to(&path, 44), TEXT),
                    sp(format!("{:>7}  ", disk(f.size)), FAINT),
                    sp(
                        format!(
                            "{} · changed {} ago · never ticked: yours to decide",
                            f.why,
                            untouched_for(m.now - f.newest)
                        ),
                        DIM,
                    ),
                ]),
                w,
            );
        }
        if let Some(c) = &it.cache {
            let col = if c.found.ok { GREEN } else { YELLOW };
            let mut words = if c.wt.is_empty() {
                c.found.why.clone()
            } else {
                "build cache".to_string()
            };
            if !c.found.note.is_empty() {
                words += &format!(" · {}", c.found.note);
            }
            let why = ln([sp(words, col)]);
            let (path, size) = if matches!(c.found.how, How::Run(_)) {
                (c.found.path.clone(), "?".to_string())
            } else {
                (
                    rush_draw::truncate(&tildify(&c.found.path), 43, "…").into_owned(),
                    disk(c.found.size),
                )
            };
            return fit(
                cat(
                    ln([
                        bx,
                        sp(pad_to(&path, 44), TEXT),
                        sp(format!("{size:>7}  "), FAINT),
                    ]),
                    why,
                ),
                w,
            );
        }
        if let Some(o) = &it.orphan {
            let ago = untouched_for(m.now - o.newest);
            let mut why = if !it.busy.is_empty() {
                bx = sp("· ", FAINT);
                ln([sp(it.busy.clone(), DIM)])
            } else if it.stale {
                ln([sp(format!("untouched for {ago}"), GREEN)])
            } else {
                ln([sp(format!("changed {ago} ago: may be in use"), YELLOW)])
            };
            if !o.session.is_empty() {
                let from = format!("from {}'s · ", session_name(m, &o.session));
                why.spans.insert(0, sp(from, DIM));
            }
            let path = rush_draw::truncate(&tildify(&o.path), 43, "…").into_owned();
            return fit(
                cat(
                    ln([
                        bx,
                        sp(pad_to(&path, 44), TEXT),
                        sp(format!("{:>7}  ", disk(o.size)), FAINT),
                    ]),
                    why,
                ),
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
        } else if let Some(t) = &it.triage {
            let mut l = ln([
                spb(format!("{} ", t.class.as_str()), class_color(t.class)),
                sp(t.reason.clone(), class_color(t.class)),
            ]);
            if !wt.own && matches!(t.class, Class::Safe | Class::NoiseOnly) {
                l.spans.push(sp(" · not rush's: yours to tick", FAINT));
            }
            l
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
            if let Some(l) = &it.left {
                picked.push(Doomed {
                    left: Some(l.clone()),
                    agents: vec![(l.agent.key.clone(), l.agent.display_name.clone())],
                    ..Doomed::default()
                });
                continue;
            }
            if let Some(o) = &it.orphan {
                picked.push(Doomed {
                    orphan: Some(o.clone()),
                    ..Doomed::default()
                });
                continue;
            }
            if let Some(c) = &it.cache {
                picked.push(Doomed {
                    cache: Some(c.clone()),
                    ..Doomed::default()
                });
                continue;
            }
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

/// How long something has sat, as Clean up says it: "3 days", "5h", "20m".
pub fn untouched_for(ms: rush_core::time::Ms) -> String {
    let (m, h, d) = (ms / 60_000, ms / 3_600_000, ms / 86_400_000);
    if d >= 1 {
        format!("{d} day{}", plural(d as usize))
    } else if h >= 1 {
        format!("{h}h")
    } else {
        format!("{}m", m.max(0))
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
        if self.measuring {
            out.push(ln([
                plain("  "),
                sp(SPINNER[m.tick % SPINNER.len()], ORANGE),
                sp(" measuring caches that rebuild themselves…", DIM),
            ]));
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
pub fn nudge_clean(m: &mut Model) {
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

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Ms = 1_793_190_896_789;

    fn found(path: &str, wt: &str, size: i64, ok: bool) -> CacheFound {
        CacheFound {
            found: rush_fleet::FoundCache {
                path: path.into(),
                why: "a cache".into(),
                size,
                newest: NOW,
                ok,
                ..rush_fleet::FoundCache::default()
            },
            wt: wt.into(),
        }
    }

    // What the rules say goes is ticked, the rest isn't: ticked first, then
    // by size; the commands every run ends with last, ticked, in their
    // order.
    #[test]
    fn ticks_caches_the_rules_say_go() {
        let mut prune = found("pnpm store prune", "", 0, true);
        prune.found.how = How::Run(rush_fleet::PNPM_PRUNE);
        let items = cache_items(vec![
            prune,
            found("/h/.cache/uv", "", 1 << 30, false),
            found("/h/Library/Caches/go-build", "", 6 << 30, true),
            found("/w/a/node_modules", "/w/a", 3 << 30, true),
            found("/w/b/node_modules", "/w/b", 9 << 30, false),
        ]);
        let got: Vec<(&str, bool, &str)> = items
            .iter()
            .map(|it| {
                (
                    it.cache.as_ref().unwrap().found.path.as_str(),
                    it.on,
                    it.group(),
                )
            })
            .collect();
        let c = "Caches that rebuild themselves";
        assert_eq!(
            got,
            [
                ("/h/Library/Caches/go-build", true, c),
                ("/w/a/node_modules", true, c),
                ("/w/b/node_modules", false, c),
                ("/h/.cache/uv", false, c),
                ("pnpm store prune", true, "Run last, once"),
            ]
        );
    }

    // What rush never touches is listed, never ticked, and can't be.
    #[test]
    fn never_ticks_what_rush_never_touches() {
        let mut items = never_items(vec![rush_fleet::FoundCache {
            path: "/h/.colima/_lima".into(),
            size: 40 << 30,
            note: "rush never touches ~/.colima: yours to decide".into(),
            ..rush_fleet::FoundCache::default()
        }]);
        let it = &mut items[0];
        assert!(!it.on && !it.safe() && !it.busy.is_empty());
        assert_eq!(it.group(), "Yours to decide: rush never touches these");
    }
}
