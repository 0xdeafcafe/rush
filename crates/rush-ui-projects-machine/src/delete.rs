//! Delete: exactly what a removal takes, before anything goes — each path,
//! whether git removes it as a worktree or it's deleted as a folder, the
//! branch that stays, who worked there, and loudly, what's lost for good.
//! Each is looked at afresh off the UI; y goes only once that's in, and
//! never while an agent runs in what would go. Go: internal/ui/
//! deletesheet.go (deleteSheet :20, doomed :29, openDelete :43, size
//! :102, losing :125, blocked :130, lines :150, head :175, found :210,
//! worktreeLoss :236, listed :270, body :282, key :309, remove :330),
//! cleanup.go (askRemoveWorktree :487, askClearScratch :497, clearScratch
//! :505, onScratchCleared :513, removeWorktree :539, onRemoved :547),
//! tempwork.go (askClean :102, askCleanAll :120, cleanTemp :136,
//! onCleaned :165) and fleet/temp.go:72 (CleanTemp's guard). Rust-only:
//! caches that rebuild themselves, as Clean up measured them (CacheFound),
//! then pnpm store prune after a node_modules.

use rush_core::adapter::TempDir;
use rush_core::text::{Color, Line};
use rush_fleet::{ScratchItem, clear_scratch, stale_scratch};
use rush_fleet::{Worktree, remove_worktree};

use super::data::*;
use super::rows::*;
use crate::frame::sheet::{Sheet, sheet_title};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// One thing a removal takes: a worktree, finished agents' temp work, or
/// what's untouched in /tmp.
#[derive(Clone, Debug, Default)]
pub struct Doomed {
    pub wt: Option<Worktree>,
    /// Whose temp work, by key and name.
    pub agents: Vec<(String, String)>,
    pub tmp: bool,
    /// What an agent left outside its project (agents is that agent).
    pub left: Option<rush_fleet::LeftItem>,
    /// What's in rush's scratch root that no session owns, as Clean up
    /// saw it (Rust-only).
    pub orphan: Option<rush_fleet::TempOrphan>,
    /// A cache that rebuilds itself, as Clean up measured it (Rust-only).
    pub cache: Option<CacheFound>,
    // Looked at off the UI.
    pub looked: bool,
    /// The orphan changed, or went, since Clean up saw it.
    pub changed: bool,
    pub files: Vec<String>,
    pub commits: Vec<String>,
    pub dirs: Vec<TempDir>,
    pub stale: Vec<ScratchItem>,
}

/// A cache Clean up offers: a folder a cleanup rule names
/// (rush_fleet::cache_rules), a build cache in a worktree nobody works in
/// (wt is that worktree), or a command every run ends with. Rust-only.
pub use rush_fleet::CacheFound;

/// What a run of Clean up's caches did: the free disk before and after,
/// as df says it (not a sum of what du measured), the commands run, and
/// the first thing that failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cleaned {
    pub before: u64,
    pub after: u64,
    pub ran: Vec<String>,
    pub failed: Option<String>,
    /// docker runs in colima's VM, which gives space back only once it
    /// trims its disk.
    pub colima: bool,
}

/// What a run of Clean up's caches did, in a few words: what df shows it
/// freed, and the free space before and after.
pub fn cleaned_words(c: &Cleaned) -> String {
    let freed = c.after as i64 - c.before as i64;
    let mut s = if freed > 0 {
        format!(
            "freed {} as df measures it ({} → {} free)",
            disk(freed),
            disk(c.before as i64),
            disk(c.after as i64)
        )
    } else {
        format!("df shows nothing freed yet ({} free)", disk(c.after as i64))
    };
    if !c.ran.is_empty() {
        s += &format!(" · ran {}", c.ran.join(", "));
    }
    if c.colima && c.ran.iter().any(|r| r.starts_with("docker")) {
        s += " · colima gives docker's space back once its VM trims its disk";
    }
    if let Some(e) = &c.failed {
        s += &format!(" · {e}");
    }
    s
}

/// Deletes caches, each through rush_fleet's checked removals
/// (remove_found: a rule's only when a rule names it and still says it
/// goes, a worktree's only when nothing in it changed since Clean up
/// measured it and no process works there), then runs the commands every
/// run ends with, once each (pnpm store prune frees what node_modules
/// linked). The free disk is read before and after. Rust-only.
fn remove_caches(m: &mut Model, list: Vec<CacheFound>) {
    if list.is_empty() {
        return;
    }
    let total: i64 = list.iter().map(|c| c.found.size).sum();
    m.flash(&format!("deleting {} of caches…", disk(total)));
    m.later(
        move || {
            let why = "ticked in Clean up";
            let before = rush_sys::sysinfo::read_disk().free;
            let rules = rush_fleet::cache_rules();
            let cwds = rush_fleet::process_cwds();
            let (cmds, folders): (Vec<CacheFound>, Vec<CacheFound>) = list
                .into_iter()
                .partition(|c| matches!(c.found.how, rush_fleet::How::Run(_)));
            let mut done = Cleaned {
                before,
                colima: rush_fleet::colima(&std::env::var("HOME").unwrap_or_default()),
                ..Cleaned::default()
            };
            let mut gone = Vec::new();
            for c in folders.iter().chain(cmds.iter()) {
                match rush_fleet::remove_found(c, &rules, &cwds, why) {
                    Ok(_) if matches!(c.found.how, rush_fleet::How::Run(_)) => {
                        done.ran.push(c.found.path.clone());
                    }
                    Ok(_) => gone.push(c.found.path.clone()),
                    Err(e) => {
                        done.failed.get_or_insert(e);
                    }
                }
            }
            done.after = rush_sys::sysinfo::read_disk().free;
            let mut f = serde_json::Map::new();
            f.insert("why".into(), why.into());
            f.insert("removed".into(), gone.into());
            f.insert("ran".into(), done.ran.clone().into());
            f.insert("freeBefore".into(), done.before.into());
            f.insert("freeAfter".into(), done.after.into());
            if let Some(e) = &done.failed {
                f.insert("err".into(), e.clone().into());
            }
            rush_events::log("clean-up", f);
            done
        },
        |m, done| {
            let text = cleaned_words(&done);
            match done.failed {
                Some(_) => m.fail(&text),
                None => m.flash(&text),
            }
            Out::Draw
        },
    );
}

/// Deletes what an agent left, looked at afresh first, then looks at
/// the worktrees again (cleanup.go:532 removeLeft).
fn remove_left(m: &mut Model, it: rush_fleet::LeftItem) {
    let path = tildify(&it.path);
    m.later(
        move || rush_fleet::remove_left(&it),
        move |m, r| {
            match r {
                Ok(_) => m.flash(&format!("deleted {path}")),
                Err(e) => m.fail(&e),
            }
            super::scan::scan_worktrees(m);
            Out::Draw
        },
    );
}

/// Deletes what's in rush's scratch root that no session owns, as Clean up
/// saw it: refused when a running session's id starts its name or it
/// changed since (rush_fleet::remove_temp_orphan). Rust-only.
fn remove_orphan(m: &mut Model, o: rush_fleet::TempOrphan) {
    let path = tildify(&o.path);
    let live = running_ids(m);
    m.later(
        move || rush_fleet::remove_temp_orphan(&o, &live),
        move |m, r| {
            match r {
                Ok(_) => m.flash(&format!("deleted {path}")),
                Err(e) => m.fail(&e),
            }
            super::scan::scan_worktrees(m);
            Out::Draw
        },
    );
}

/// The Delete sheet.
pub struct DeleteSheet {
    pub items: Vec<Doomed>,
    /// The look afresh is still out.
    pub reading: bool,
    /// The sheet it was opened from, back on esc.
    pub back: Option<Box<dyn Sheet>>,
    pub scroll: usize,
}

fn own_roots(m: &Model) -> rush_core::adapter::Registry {
    m.reg.clone()
}

/// Opens Delete on what would go, and looks at each afresh.
pub fn open_delete(m: &mut Model, back: Option<Box<dyn Sheet>>, items: Vec<Doomed>) {
    let look = items.clone();
    let temps: Vec<Vec<TempDir>> = items
        .iter()
        .map(|d| {
            let st = data(m);
            d.agents
                .iter()
                .flat_map(|(k, _)| st.temp.get(k).map(|t| t.dirs.clone()).unwrap_or_default())
                .collect()
        })
        .collect();
    let own = own_roots(m);
    m.open_sheet(Box::new(DeleteSheet {
        items,
        reading: true,
        back,
        scroll: 0,
    }));
    m.later(
        move || {
            let mut look = look;
            for (d, dirs) in look.iter_mut().zip(temps) {
                if let Some(w) = d.wt.as_mut() {
                    (d.files, d.commits) = w.doomed();
                } else if let Some(l) = d.left.as_mut() {
                    *l = l.again();
                } else if let Some(o) = &d.orphan {
                    let n = o.again();
                    d.changed = n.gone || n.newest != o.newest;
                } else if let Some(c) = &d.cache {
                    // A worktree's cache that changed since may be in use;
                    // a rule's is a cache whatever it does.
                    if !c.wt.is_empty() {
                        let (_, newest) = rush_fleet::walk(&c.found.path);
                        d.changed = newest > c.found.newest;
                    }
                } else if d.tmp {
                    d.stale = stale_scratch(&own, "/tmp");
                } else {
                    d.dirs = dirs;
                }
                d.looked = true;
            }
            look
        },
        |m, look| {
            if let Some(s) = m.sheet_as::<DeleteSheet>() {
                s.items = look;
                s.reading = false;
            }
            Out::Draw
        },
    );
}

impl Doomed {
    /// How much disk it frees, as last measured.
    fn size(&self, m: &Model) -> i64 {
        let st = data(m);
        if let Some(w) = &self.wt {
            return w.size;
        }
        if let Some(l) = &self.left {
            return l.size;
        }
        if let Some(o) = &self.orphan {
            return o.size;
        }
        if let Some(c) = &self.cache {
            return c.found.size;
        }
        if self.tmp && !self.looked {
            return st.tmp.stale;
        }
        if self.tmp {
            return self.stale.iter().map(|i| i.size).sum();
        }
        self.agents.iter().map(|(k, _)| st.temp_of(k)).sum()
    }

    /// It throws away work nowhere else: what's uncommitted only noise
    /// (rush_fleet::noise: Claude Code's local settings, a lockfile) isn't.
    fn losing(&self) -> bool {
        self.wt.as_ref().is_some_and(|w| {
            let noise_only =
                w.files.len() == w.changed && w.files.iter().all(|f| rush_fleet::noise(f));
            w.changed > 0 && !noise_only
                || w.unpushed > 0 && w.branch.is_empty()
                || !w.err.is_empty()
        })
    }

    /// Why it can't go now: an agent running in it.
    fn blocked(&self, m: &Model) -> String {
        if let Some(w) = &self.wt
            && let Some(g) = running(m, &w.agents)
        {
            return format!("{} is running in it", one_line(&g.a.display_name));
        }
        if let Some(l) = &self.left
            && self.looked
            && !l.why().is_empty()
        {
            return l.why().to_string();
        }
        if let Some(o) = &self.orphan {
            if let Some(id) = o.used_by(&running_ids(m)) {
                return format!(
                    "{} is running; its id starts the name, so it may use it",
                    session_name(m, id)
                );
            }
            if self.looked && self.changed {
                return format!(
                    "{} changed since Clean up looked; it stays",
                    tildify(&o.path)
                );
            }
        }
        if let Some(c) = &self.cache
            && !c.wt.is_empty()
        {
            if let Some(w) = data(m).wts.iter().find(|w| w.path == c.wt)
                && let Some(g) = running(m, &w.agents)
            {
                return format!("{} is running in its worktree", one_line(&g.a.display_name));
            }
            if self.looked && self.changed {
                return format!(
                    "{} changed since Clean up looked; it stays",
                    tildify(&c.found.path)
                );
            }
        }
        for (k, _) in &self.agents {
            if let Some(g) = agent_by_key(m, k)
                && g.a.pid != 0
            {
                return format!(
                    "{} is running; its temp work may be in use",
                    one_line(&g.a.display_name)
                );
            }
        }
        String::new()
    }

    /// What it is, where, and how it goes.
    fn head(&self, m: &Model, w: usize) -> Vec<Line> {
        if let Some(wt) = &self.wt {
            let keeps = if wt.branch.is_empty() {
                ln([
                    plain("   "),
                    sp("! ", YELLOW),
                    sp(
                        "on no branch (detached): nothing keeps its commits once it goes",
                        DIM,
                    ),
                ])
            } else {
                ln([
                    plain("   "),
                    sp("✓ ", GREEN),
                    sp("branch ", DIM),
                    sp(wt.branch.clone(), SUB),
                    sp(format!(" stays, and {} with it", tildify(&wt.repo)), DIM),
                ])
            };
            let name = wt.path.rsplit('/').next().unwrap_or(&wt.path);
            return vec![
                kind_line(
                    Kind::Worktree,
                    name,
                    "worktree · git removes it",
                    &disk(wt.size),
                    w,
                ),
                ln([plain("   "), sp(tildify(&wt.path), FAINT)]),
                keeps,
            ];
        }
        if let Some(l) = &self.left {
            let what = format!("left by {} · deleted", one_line(&l.agent.display_name));
            let note = if l.temp {
                "scratch in a temp folder"
            } else {
                "outside any temp folder: only because you ticked it"
            };
            return vec![
                kind_line(Kind::Temp, &tildify(&l.path), &what, &disk(self.size(m)), w),
                ln([
                    plain("   "),
                    sp(
                        format!("{note}; looked at again first, and kept if it changed since"),
                        DIM,
                    ),
                ]),
            ];
        }
        if let Some(o) = &self.orphan {
            let what = if o.session.is_empty() {
                "rush's scratch, no session's · deleted".to_string()
            } else {
                format!(
                    "rush's scratch, from {}'s probably · deleted",
                    session_name(m, &o.session)
                )
            };
            return vec![
                kind_line(Kind::Temp, &tildify(&o.path), &what, &disk(self.size(m)), w),
                ln([
                    plain("   "),
                    sp(
                        "no session owns it; looked at again first, and kept if it changed since",
                        DIM,
                    ),
                ]),
            ];
        }
        if let Some(c) = &self.cache {
            let (what, note) = if matches!(c.found.how, rush_fleet::How::Run(_)) {
                (
                    "command · run once, after the rest".to_string(),
                    format!("{}; {}", c.found.why, c.found.note),
                )
            } else if c.found.how == rush_fleet::How::GoClean {
                (
                    "Go's module cache · go clean -modcache".to_string(),
                    format!("{}; its files are read-only, so go clears it", c.found.why),
                )
            } else if c.wt.is_empty() {
                (
                    "cache · deleted".to_string(),
                    format!("{}; made again by the tool that made it", c.found.why),
                )
            } else {
                let wt = c.wt.rsplit('/').next().unwrap_or(&c.wt);
                (
                    format!("build cache in {wt} · deleted"),
                    "an install or build puts it back; kept if it changed since Clean up looked"
                        .to_string(),
                )
            };
            return vec![
                kind_line(
                    Kind::Temp,
                    &tildify(&c.found.path),
                    &what,
                    &disk(self.size(m)),
                    w,
                ),
                ln([plain("   "), sp(note, DIM)]),
            ];
        }
        if self.tmp {
            let n = if self.looked {
                self.stale.len()
            } else {
                data(m).tmp.stale_items
            };
            return vec![
                kind_line(
                    Kind::Temp,
                    "/tmp",
                    &format!("{n} of yours untouched for a day · deleted"),
                    &disk(self.size(m)),
                    w,
                ),
                ln([
                    plain("   "),
                    sp(
                        "each is looked at again first; anything touched since stays",
                        DIM,
                    ),
                ]),
            ];
        }
        let what = if self.agents.len() > 1 {
            format!("temp work of {} finished agents", self.agents.len())
        } else {
            format!(
                "temp work of {}",
                one_line(self.agents.first().map_or("", |a| a.1.as_str()))
            )
        };
        vec![
            kind_line(
                Kind::Temp,
                &what,
                "scratch folders · deleted",
                &disk(self.size(m)),
                w,
            ),
            ln([
                plain("   "),
                sp("conversations, their files and any worktree stay", DIM),
            ]),
        ]
    }

    /// What the fresh look found.
    fn found(&self, m: &Model, w: usize) -> Vec<Line> {
        if self.wt.is_some() {
            return self.worktree_loss(m, w);
        }
        if self.left.is_some() || self.orphan.is_some() || self.cache.is_some() {
            return Vec::new();
        }
        if self.tmp {
            if self.stale.is_empty() {
                return vec![ln([
                    plain("   "),
                    sp("nothing is untouched any more; nothing goes", DIM),
                ])];
            }
            let items: Vec<String> = self
                .stale
                .iter()
                .map(|it| {
                    format!(
                        "{}{}",
                        fit_str(&tildify(&it.path), w.saturating_sub(18)),
                        right(&disk(it.size), 8)
                    )
                })
                .collect();
            return listed(&items, SUB, w);
        }
        let items: Vec<String> = self
            .dirs
            .iter()
            .map(|t| {
                if t.keep {
                    format!("{}  (emptied; the folder stays)", tildify(&t.path))
                } else {
                    tildify(&t.path)
                }
            })
            .collect();
        listed(&items, SUB, w)
    }

    fn worktree_loss(&self, m: &Model, w: usize) -> Vec<Line> {
        let wt = self.wt.as_ref().expect("a worktree");
        let mut out = Vec::new();
        if !wt.err.is_empty() {
            out.push(ln([
                plain("   "),
                spb(
                    format!(
                        "✗ {}: what it holds can't be told, and goes with it",
                        wt.err
                    ),
                    RED,
                ),
            ]));
        }
        let n = self.files.len();
        if n > 0 && self.files.iter().all(|f| rush_fleet::noise(f)) {
            out.push(ln([
                plain("   "),
                sp(
                    format!(
                        "! only noise uncommitted ({}): local settings or a lockfile, gone with it",
                        self.files.join(", ")
                    ),
                    YELLOW,
                ),
            ]));
        } else if n > 0 {
            out.push(ln([
                plain("   "),
                spb(
                    format!("✗ {n} uncommitted file{} LOST for good", plural(n)),
                    RED,
                ),
            ]));
            out.extend(listed(&self.files, RED, w));
        }
        let n = self.commits.len();
        if n > 0 {
            let c = format!("{n} commit{}", plural(n));
            if wt.branch.is_empty() {
                out.push(ln([
                    plain("   "),
                    spb(format!("✗ {c} on no branch LOST"), RED),
                ]));
            } else {
                out.push(ln([
                    plain("   "),
                    spb(format!("! {c} not pushed or in main"), YELLOW),
                    sp(format!(": only {} keeps them", wt.branch), YELLOW),
                ]));
            }
            out.extend(listed(&self.commits, YELLOW, w));
        }
        if wt.locked {
            out.push(ln([
                plain("   "),
                sp(
                    "! locked: someone put a lock on it, and removing it ignores that",
                    YELLOW,
                ),
            ]));
        }
        if wt.safe() {
            out.push(ln([
                plain("   "),
                sp(
                    "✓ nothing is lost: every change committed, every commit pushed or in main",
                    GREEN,
                ),
            ]));
        }
        for k in &wt.agents {
            if let Some(g) = agent_by_key(m, k)
                && g.a.pid == 0
            {
                out.push(ln([
                    plain("   "),
                    sp("worked here: ", DIM),
                    sp(one_line(&g.a.display_name), SUB),
                    sp(" · its conversation stays; resumed, it has no folder", DIM),
                ]));
            }
        }
        out
    }
}

/// A list under an item: the first few, and how many more.
fn listed(items: &[String], col: Color, w: usize) -> Vec<Line> {
    const SHOWN: usize = 8;
    let mut out: Vec<Line> = items
        .iter()
        .take(SHOWN)
        .map(|l| {
            ln([
                plain("      "),
                sp(
                    rush_draw::truncate(l, w.saturating_sub(8), "…").into_owned(),
                    col,
                ),
            ])
        })
        .collect();
    if items.len() > SHOWN {
        out.push(ln([
            plain("      "),
            sp(format!("… {} more", items.len() - SHOWN), FAINT),
        ]));
    }
    out
}

impl DeleteSheet {
    /// Everything it lists, and what it all comes to.
    fn lines(&self, m: &Model, w: usize) -> (Vec<Line>, i64, usize, bool) {
        let (mut out, mut size, mut losing, mut blocked) = (Vec::new(), 0, 0, false);
        for (i, d) in self.items.iter().enumerate() {
            size += d.size(m);
            if d.losing() {
                losing += 1;
            }
            if i > 0 {
                out.push(Line::new());
            }
            out.extend(d.head(m, w));
            let why = d.blocked(m);
            if !why.is_empty() {
                blocked = true;
                out.push(ln([
                    plain("   "),
                    spb(format!("✗ can't go: {why}"), RED),
                    sp(" · stop it or mark it done first", DIM),
                ]));
            }
            if !d.looked {
                out.push(ln([
                    plain("   "),
                    sp(SPINNER[m.tick % SPINNER.len()], ORANGE),
                    sp(" looking at it again…", DIM),
                ]));
                continue;
            }
            out.extend(d.found(m, w));
        }
        (out, size, losing, blocked)
    }

    /// Removes everything listed, as looked at: a worktree found safe is
    /// checked with git once more, one that isn't is forced.
    fn remove(&mut self, m: &mut Model) -> Out {
        if self.reading {
            m.flash("still looking · a moment");
            return Out::Draw;
        }
        for d in &self.items {
            let why = d.blocked(m);
            if !why.is_empty() {
                m.flash_err(&format!("{why} · stop it or mark it done first"));
                return Out::Draw;
            }
        }
        let mut caches = Vec::new();
        for d in std::mem::take(&mut self.items) {
            if let Some(c) = d.cache {
                caches.push(c);
            } else if let Some(w) = d.wt {
                let force = !w.safe();
                remove_wt(m, w, force);
            } else if let Some(l) = d.left {
                remove_left(m, l);
            } else if let Some(o) = d.orphan {
                remove_orphan(m, o);
            } else if d.tmp {
                clear_tmp(m);
            } else {
                clean_temp(m, d.agents);
            }
        }
        remove_caches(m, caches);
        Out::Close
    }
}

impl Sheet for DeleteSheet {
    fn width(&self, _m: &Model) -> usize {
        118
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let (lines, size, losing, blocked) = self.lines(m, w);
        let n = self.items.len();
        let about = format!("{n} thing{} · frees {}", plural(n), disk(size));
        let mut out = vec![sheet_title("Delete", &about, w)];
        if losing > 0 {
            out.push(ln([spb(
                format!(
                    "✗ {losing} would lose work that exists nowhere else · this can't be undone"
                ),
                RED,
            )]));
        }
        out.push(Line::new());
        let room = 1.max(h as isize - out.len() as isize - 3) as usize;
        self.scroll = self.scroll.min(lines.len().saturating_sub(room));
        let end = lines.len().min(self.scroll + room);
        out.extend(lines[self.scroll..end].iter().cloned());
        if lines.len() > room {
            out.push(ln([sp(
                format!("  ↑↓ {} more lines", lines.len() - room),
                FAINT,
            )]));
        }
        out.push(Line::new());
        let k = if blocked {
            keys_fit(w, &[("↑↓", "scroll"), ("esc", "back")])
        } else if self.reading {
            keys_fit(w, &[("↑↓", "scroll"), ("esc", "cancel")])
        } else if losing > 0 {
            keys_fit(
                w,
                &[
                    ("↑↓", "scroll"),
                    ("y", "delete, losing it"),
                    ("esc", "cancel"),
                ],
            )
        } else {
            keys_fit(w, &[("↑↓", "scroll"), ("y", "delete"), ("esc", "cancel")])
        };
        out.push(k);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" | "n" => {
                if let Some(b) = self.back.take() {
                    m.open_sheet(b);
                }
                Out::Close
            }
            "up" | "k" => {
                self.scroll = self.scroll.saturating_sub(1);
                Out::Draw
            }
            "down" | "j" => {
                self.scroll += 1;
                Out::Draw
            }
            "pgup" => {
                self.scroll = self.scroll.saturating_sub(10);
                Out::Draw
            }
            "pgdown" => {
                self.scroll += 10;
                Out::Draw
            }
            "y" | "enter" => self.remove(m),
            _ => Out::Idle,
        }
    }
}

/// Opens Delete on a worktree, once it's been looked at.
pub fn ask_remove_worktree(m: &mut Model, wt: Worktree) {
    if wt.checked == 0 || wt.repo.is_empty() {
        let name = wt.path.rsplit('/').next().unwrap_or(&wt.path).to_string();
        m.flash(&format!("{name} hasn't been looked at yet · a moment"));
        return;
    }
    open_delete(
        m,
        None,
        vec![Doomed {
            wt: Some(wt),
            ..Doomed::default()
        }],
    );
}

/// Opens Delete on what's yours in /tmp untouched for a day.
pub fn ask_clear_scratch(m: &mut Model) {
    if data(m).tmp.stale_items == 0 {
        m.flash("everything of yours in /tmp was touched in the last day; it stays");
        return;
    }
    open_delete(
        m,
        None,
        vec![Doomed {
            tmp: true,
            ..Doomed::default()
        }],
    );
}

/// Opens Delete on an agent's temp work.
pub fn ask_clean(m: &mut Model, key: &str) {
    let Some(g) = agent_by_key(m, key) else {
        m.flash_err("select an agent first");
        return;
    };
    let name = g.a.display_name.clone();
    if g.a.pid != 0 {
        m.flash_err(&format!(
            "{name} is still running; its temp work may be in use · stop it first (ctrl+x)"
        ));
        return;
    }
    if data(m).temp_of(key) < TEMP_SHOWN {
        m.flash(&format!("{name} has no temp work to clean"));
        return;
    }
    open_delete(
        m,
        None,
        vec![Doomed {
            agents: vec![(key.to_string(), name)],
            ..Doomed::default()
        }],
    );
}

/// Opens Delete on the temp work of every agent that has finished.
pub fn ask_clean_all(m: &mut Model) {
    let d = data(m);
    let list: Vec<(String, String)> = agents(m)
        .filter(|g| g.a.pid == 0 && d.temp_of(&g.a.key) >= TEMP_SHOWN)
        .map(|g| (g.a.key.clone(), g.a.display_name.clone()))
        .collect();
    if list.is_empty() {
        m.flash("no finished agent has temp work to clean");
        return;
    }
    open_delete(
        m,
        None,
        vec![Doomed {
            agents: list,
            ..Doomed::default()
        }],
    );
}

fn remove_wt(m: &mut Model, wt: Worktree, force: bool) {
    let name = wt.path.rsplit('/').next().unwrap_or(&wt.path).to_string();
    m.flash(&format!("removing {name}…"));
    let (path, freed) = (wt.path.clone(), wt.size);
    m.later(
        move || remove_worktree(&wt, force),
        move |m, r| {
            match r {
                Err(e) => m.fail(&e),
                Ok(()) => {
                    m.kept::<Data>().wts.retain(|w| w.path != path);
                    m.flash(&format!("removed {name} · freed {}", disk(freed)));
                }
            }
            Out::Draw
        },
    );
}

fn clear_tmp(m: &mut Model) {
    m.flash("clearing /tmp…");
    let own = own_roots(m);
    m.later(
        move || clear_scratch(&own, "/tmp"),
        |m, (freed, n, err)| {
            let s = &mut m.kept::<Data>().tmp;
            s.items = s.items.saturating_sub(n);
            s.stale_items = s.stale_items.saturating_sub(n);
            s.size -= freed;
            s.stale -= freed;
            s.checked = 0; // and look again at what's left
            let text = format!("cleared {n} things in /tmp · freed {}", disk(freed));
            match err {
                Some(e) => m.fail(&format!("{text} · {e}")),
                None => m.flash(&text),
            }
            super::scan::scan_worktrees(m);
            Out::Draw
        },
    );
}

/// Whether dir is somewhere temp work may be deleted from: a temp folder,
/// or two below the agent's own scratch root (fleet/temp.go:81).
pub fn deletable(dir: &str, roots: &[String]) -> bool {
    if crate::list::is_temp(dir) {
        return true;
    }
    let p = std::path::Path::new(dir);
    let gp = p.parent().and_then(|d| d.parent());
    roots
        .iter()
        .any(|r| !r.is_empty() && gp.is_some_and(|g| g == std::path::Path::new(r)))
}

/// Deletes agents' temp work, then measures it again.
fn clean_temp(m: &mut Model, list: Vec<(String, String)>) {
    let st = data(m);
    let total: i64 = list.iter().map(|(k, _)| st.temp_of(k)).sum();
    let work: Vec<(String, i64, Vec<TempDir>)> = list
        .iter()
        .map(|(k, _)| {
            (
                k.clone(),
                st.temp_of(k),
                st.temp.get(k).map(|t| t.dirs.clone()).unwrap_or_default(),
            )
        })
        .collect();
    let roots: Vec<String> = m
        .reg
        .all()
        .iter()
        .filter_map(|a| a.scratcher().map(|s| s.scratch_root()))
        .collect();
    m.flash(&format!("cleaning {} of temp work…", disk(total)));
    m.later(
        move || {
            let mut failed = None;
            let mut sizes = Vec::new();
            for (k, before, dirs) in work {
                for d in &dirs {
                    if !deletable(&d.path, &roots) {
                        failed.get_or_insert(format!("won't delete {}", d.path));
                        continue;
                    }
                    let r = if d.keep {
                        match std::fs::read_dir(&d.path) {
                            // Each goes, whatever another says; the first
                            // that couldn't is said.
                            Ok(rd) => {
                                let mut first = Ok(());
                                for e in rd.flatten() {
                                    let r = rush_fleet::remove_tree(&e.path().to_string_lossy());
                                    if first.is_ok() {
                                        first = r;
                                    }
                                }
                                first
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                            Err(e) => Err(e.to_string()),
                        }
                    } else {
                        rush_fleet::remove_tree(&d.path)
                    };
                    if let Err(e) = r {
                        failed.get_or_insert(e);
                    }
                }
                let paths: Vec<&str> = dirs.iter().map(|d| d.path.as_str()).collect();
                sizes.push((k, before, disk_usage(&paths)));
            }
            (sizes, failed)
        },
        |m, (sizes, failed)| {
            let mut freed = 0;
            let st = m.kept::<Data>();
            for (k, before, after) in sizes {
                freed += before - after;
                if let Some(t) = st.temp.get_mut(&k) {
                    (t.bytes, t.known) = (after, true);
                }
            }
            match failed {
                Some(e) => m.fail(&format!("freed {} · {e}", disk(freed))),
                None => m.flash(&format!("freed {} of temp work", disk(freed))),
            }
            Out::Draw
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1 << 30;

    // What a run says is what df shows, before and after; docker's prunes
    // under colima say the space comes later.
    #[test]
    fn says_what_df_shows() {
        let c = |before, after, ran: &[&str], colima, failed: Option<&str>| Cleaned {
            before,
            after,
            ran: ran.iter().map(|s| s.to_string()).collect(),
            failed: failed.map(String::from),
            colima,
        };
        for (got, want) in [
            (
                c(2 * G, 9 * G, &[], false, None),
                "freed 7.0G as df measures it (2.0G → 9.0G free)",
            ),
            (
                c(9 * G, 9 * G, &[], false, None),
                "df shows nothing freed yet (9.0G free)",
            ),
            (
                c(
                    2 * G,
                    3 * G,
                    &["pnpm store prune", "docker image prune -f"],
                    true,
                    None,
                ),
                "freed 1.0G as df measures it (2.0G → 3.0G free) · ran pnpm store prune, docker image prune -f · colima gives docker's space back once its VM trims its disk",
            ),
            (
                c(
                    2 * G,
                    3 * G,
                    &["pnpm store prune"],
                    true,
                    Some("x: changed lately; it stays"),
                ),
                "freed 1.0G as df measures it (2.0G → 3.0G free) · ran pnpm store prune · x: changed lately; it stays",
            ),
        ] {
            assert_eq!(cleaned_words(&got), want);
        }
    }
}
