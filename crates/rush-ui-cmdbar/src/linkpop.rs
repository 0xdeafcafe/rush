//! A GitHub link's popover: a small box by the link clicked (or in the
//! middle, opened from a key) with one line of what it is and where it
//! stands, one quiet line of facts, its links out first, then what can be
//! done with it in groups, folded, each showing what's in it as a hint.
//! Rush-only: Go's link menu (internal/ui/linkmenu.go:93 openLinkMenu,
//! linkkinds.go:53 openPRLink) opened a pull request's whole sheet; that
//! sheet is still here, as In rush's PR sheet.
//!
//! Nothing is done without a key: merging and approving ask first, through
//! the PR sheet's own questions (prsheet's ask_merge, ask_approve).

use rush_convo::linkinfo::{
    LINK_GITHUB_ISSUE, LINK_GITHUB_PR, LINK_GITHUB_RUN, forget_link, link_info,
};
use rush_core::text::{Color, Line, Span};
use rush_gh::ghpr::{self, Gh, GhLink, Issue, PR, Run};

use crate::frame::confirm::Confirm;
use crate::frame::overlayclick::MouseEv;
use crate::frame::sheet::Popover;
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;
use rush_ui_projects_machine::{place, prsheet};

/// The popover's width, edges and all, on a screen wide enough.
pub const POP_WIDTH: usize = 52;

/// What a link's lookup said of what it points at.
// One at a time, while a popover is open: not worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Info {
    Pr(PR),
    Issue(Issue),
    Run(Run),
}

/// What a row does once picked.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
enum Do {
    Browse(String),
    Copy(String),
    Approve(PR),
    /// Opens the PR sheet writing "comment" or "changes".
    Write(PR, &'static str),
    /// Asks how to merge, this way first.
    Merge(PR, &'static str),
    Sheet(PR),
    /// Runs the run's jobs again: only those that failed, or all.
    Rerun(Run, bool),
}

/// One thing the popover offers.
#[derive(Clone)]
struct Item {
    glyph: &'static str,
    label: String,
    act: Do,
}

/// Actions under one name, folded until opened.
struct Group {
    name: &'static str,
    items: Vec<Item>,
}

/// What the popover shows of a link.
struct Shown {
    title: String,
    state: Option<(String, Color)>,
    /// The facts line's parts, dropped from the end until they fit.
    facts: Vec<(String, Color)>,
    links: Vec<Item>,
    groups: Vec<Group>,
}

/// A place in the popover the cursor can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Link(usize),
    Group(usize),
    Member(usize, usize),
}

/// Where a slot was drawn: its row, and its cells [x0, x1).
#[derive(Clone, Copy, Debug)]
struct Hit {
    row: usize,
    x0: usize,
    x1: usize,
    slot: Slot,
}

/// A GitHub link's popover.
pub struct LinkPop {
    /// The link clicked.
    pub url: String,
    pub link: GhLink,
    /// What its lookup last said; None until it has.
    pub info: Option<Info>,
    /// The screen cell it was opened from; None centres it.
    pub at: Option<(usize, usize)>,
    /// The group unfolded.
    pub open: Option<usize>,
    cur: Slot,
    /// The inside's width as last drawn, and where each slot landed.
    w: usize,
    hits: Vec<Hit>,
}

/// What link_info says of u, typed by its kind.
fn looked_up(u: &str) -> Option<Info> {
    let (kind, _, v) = link_info(u)?;
    match kind {
        LINK_GITHUB_PR => v.downcast_ref::<PR>().cloned().map(Info::Pr),
        LINK_GITHUB_ISSUE => v.downcast_ref::<Issue>().cloned().map(Info::Issue),
        LINK_GITHUB_RUN => v.downcast_ref::<Run>().cloned().map(Info::Run),
        _ => None,
    }
}

/// Opens the popover on a GitHub link of a kind rush knows, by the cell
/// clicked when a click opened it; false for any other link.
pub fn open_link_pop(m: &mut Model, u: &str) -> bool {
    let info = looked_up(u);
    open_link_pop_with(m, u, info)
}

/// open_link_pop with what's known of the link given: for tests, and
/// for what knows it already.
pub fn open_link_pop_with(m: &mut Model, u: &str, info: Option<Info>) -> bool {
    let Some(link) = ghpr::github_link(u) else {
        return false;
    };
    let at = m.click_at;
    m.open_sheet(Box::new(LinkPop {
        url: u.to_string(),
        link,
        info,
        at,
        open: None,
        cur: Slot::Link(0),
        w: POP_WIDTH - 4,
        hits: Vec::new(),
    }));
    true
}

fn item(glyph: &'static str, label: &str, act: Do) -> Item {
    Item {
        glyph,
        label: label.to_string(),
        act,
    }
}

fn browse(label: &str, u: String) -> Item {
    item("↗", label, Do::Browse(u))
}

/// A pull request's checks in a few words, as its badge says them.
fn checks_word(pr: &PR) -> Option<(String, Color)> {
    let (ok, failed, pending) = pr.counts();
    if failed > 0 {
        Some((format!("✗ {failed} failing"), RED))
    } else if pending > 0 {
        Some((format!("{pending} running"), SUB))
    } else if ok > 0 {
        Some(("✓ checks".into(), GREEN))
    } else {
        None
    }
}

impl LinkPop {
    /// What the lookup says now, kept should it be forgotten.
    fn refresh(&mut self) {
        if let Some(i) = looked_up(&self.url) {
            self.info = Some(i);
        }
    }

    fn shown(&self) -> Shown {
        let repo = self.link.repo().to_string();
        let copy = item("⧉", "Copy link", Do::Copy(self.url.clone()));
        match &self.link {
            GhLink::Pr { n, .. } => {
                let base = ghpr::pr_url(&repo, *n);
                let pr = match &self.info {
                    Some(Info::Pr(p)) => Some(p.clone()),
                    _ => None,
                };
                let mut facts = vec![(repo.clone(), DIM)];
                let mut groups = Vec::new();
                let (title, state) = match &pr {
                    Some(p) => {
                        if !p.head.is_empty() {
                            let to = if p.base.is_empty() {
                                "its base"
                            } else {
                                p.base.as_str()
                            };
                            facts.push((format!("{} → {to}", p.head), DIM));
                        }
                        facts.extend(checks_word(p));
                        let state = match (p.state.as_str(), p.draft) {
                            ("MERGED", _) => ("merged", BLUE),
                            ("CLOSED", _) => ("closed", RED),
                            (_, true) => ("draft", DIM),
                            _ => ("open", GREEN),
                        };
                        (
                            format!("#{n} {}", one_line(&p.title)),
                            Some((state.0.to_string(), state.1)),
                        )
                    }
                    None => (format!("#{n} pull request"), None),
                };
                let sheet_pr = pr.clone().unwrap_or_else(|| PR {
                    number: *n,
                    url: base.clone(),
                    ..PR::default()
                });
                if let Some(p) = pr.filter(|p| p.state == "OPEN") {
                    let mut review = Vec::new();
                    if !p.mine {
                        review.push(item("", "approve", Do::Approve(p.clone())));
                        review.push(item("", "request changes", Do::Write(p.clone(), "changes")));
                    }
                    review.push(item("", "comment", Do::Write(p.clone(), "comment")));
                    groups.push(Group {
                        name: "Review",
                        items: review,
                    });
                    groups.push(Group {
                        name: "Merge",
                        items: vec![
                            item("", "squash", Do::Merge(p.clone(), "squash")),
                            item("", "merge commit", Do::Merge(p.clone(), "merge")),
                            item("", "rebase", Do::Merge(p.clone(), "rebase")),
                        ],
                    });
                }
                groups.push(Group {
                    name: "In rush",
                    items: vec![item("", "PR sheet", Do::Sheet(sheet_pr))],
                });
                Shown {
                    title,
                    state,
                    facts,
                    links: vec![
                        browse("Open on GitHub", base.clone()),
                        browse("Checks", format!("{base}/checks")),
                        browse("Files", format!("{base}/files")),
                        copy,
                    ],
                    groups,
                }
            }
            GhLink::Issue { n, .. } => {
                let base = ghpr::issue_url(&repo, *n);
                let (title, state) = match &self.info {
                    Some(Info::Issue(i)) => {
                        let state = if i.state != "CLOSED" {
                            ("open", GREEN)
                        } else {
                            match i.reason.as_str() {
                                "NOT_PLANNED" => ("not planned", DIM),
                                "DUPLICATE" => ("duplicate", DIM),
                                _ => ("closed", BLUE),
                            }
                        };
                        (
                            format!("#{n} {}", one_line(&i.title)),
                            Some((state.0.to_string(), state.1)),
                        )
                    }
                    _ => (format!("#{n} issue"), None),
                };
                Shown {
                    title,
                    state,
                    facts: vec![(repo, DIM)],
                    links: vec![
                        browse("Open on GitHub", base.clone()),
                        browse("Comments", format!("{base}#new_comment_field")),
                        copy,
                    ],
                    groups: Vec::new(),
                }
            }
            GhLink::Run { id, job, .. } => {
                let base = ghpr::run_url(&repo, *id);
                let mut links = vec![browse("Open on GitHub", base.clone())];
                let mut facts = vec![(repo.clone(), DIM)];
                let mut groups = Vec::new();
                let (title, state) = match &self.info {
                    Some(Info::Run(r)) => {
                        if !r.branch.is_empty() {
                            facts.push((r.branch.clone(), DIM));
                        }
                        let word = r.word();
                        if let Some(j) = r.failed_job() {
                            facts.push((format!("✗ {}", j.name), RED));
                            links.push(browse("Failed job", j.url.clone()));
                        } else if *job > 0 {
                            links.push(browse("Job", format!("{base}/job/{job}")));
                        }
                        if r.status == "completed" {
                            let mut items = Vec::new();
                            if r.failed_job().is_some() {
                                items.push(item("", "failed jobs", Do::Rerun(r.clone(), true)));
                            }
                            items.push(item("", "all jobs", Do::Rerun(r.clone(), false)));
                            groups.push(Group {
                                name: "Re-run",
                                items,
                            });
                        }
                        let col = match word {
                            "passed" => GREEN,
                            "failed" => RED,
                            "running" | "queued" => ORANGE,
                            _ => DIM,
                        };
                        let name = if r.workflow.is_empty() {
                            "run".to_string()
                        } else {
                            r.workflow.clone()
                        };
                        let t = one_line(&r.title);
                        let title = if t.is_empty() {
                            name
                        } else {
                            format!("{name} · {t}")
                        };
                        (title, Some((word.to_string(), col)))
                    }
                    _ => {
                        if *job > 0 {
                            links.push(browse("Job", format!("{base}/job/{job}")));
                        }
                        (format!("Actions run {id}"), None)
                    }
                };
                links.push(copy);
                Shown {
                    title,
                    state,
                    facts,
                    links,
                    groups,
                }
            }
            GhLink::Commit { sha, .. } => {
                let base = format!("https://github.com/{repo}/commit/{sha}");
                let short: String = sha.chars().take(7).collect();
                Shown {
                    title: format!("commit {short}"),
                    state: None,
                    facts: vec![(repo, DIM)],
                    links: vec![
                        browse("Open on GitHub", base.clone()),
                        browse("Checks", format!("{base}/checks")),
                        copy,
                        item("⧉", "Copy SHA", Do::Copy(sha.clone())),
                    ],
                    groups: Vec::new(),
                }
            }
            GhLink::Repo { .. } => {
                let base = format!("https://github.com/{repo}");
                Shown {
                    title: repo,
                    state: None,
                    facts: Vec::new(),
                    links: vec![
                        browse("Open on GitHub", base.clone()),
                        browse("Pull requests", format!("{base}/pulls")),
                        browse("Actions", format!("{base}/actions")),
                        browse("Issues", format!("{base}/issues")),
                        copy,
                    ],
                    groups: Vec::new(),
                }
            }
        }
    }

    /// The slots there are, in order: the links, then each group and,
    /// unfolded, what's in it.
    fn slots(&self, sh: &Shown) -> Vec<Slot> {
        let mut out: Vec<Slot> = (0..sh.links.len()).map(Slot::Link).collect();
        for (g, grp) in sh.groups.iter().enumerate() {
            out.push(Slot::Group(g));
            if self.open == Some(g) {
                out.extend((0..grp.items.len()).map(|i| Slot::Member(g, i)));
            }
        }
        out
    }

    fn label(sh: &Shown, s: Slot) -> &str {
        match s {
            Slot::Link(i) => &sh.links[i].label,
            Slot::Group(g) => sh.groups[g].name,
            Slot::Member(g, i) => &sh.groups[g].items[i].label,
        }
    }

    /// Each slot's key: a letter of its label, its words' first letters
    /// tried first. An unfolded group's rows choose first, then the links,
    /// then the groups.
    fn letters(&self, sh: &Shown) -> Vec<(Slot, usize, char)> {
        let slots = self.slots(sh);
        let mut order: Vec<Slot> = slots
            .iter()
            .copied()
            .filter(|s| matches!(s, Slot::Member(..)))
            .collect();
        order.extend(slots.iter().copied().filter(|s| matches!(s, Slot::Link(_))));
        order.extend(
            slots
                .iter()
                .copied()
                .filter(|s| matches!(s, Slot::Group(_))),
        );
        let mut taken: Vec<char> = Vec::new();
        let mut out = Vec::new();
        for s in order {
            let label = Self::label(sh, s);
            let chars: Vec<char> = label.chars().collect();
            let starts = (0..chars.len()).filter(|&i| i == 0 || chars[i - 1] == ' ');
            let rest = 0..chars.len();
            let pick = starts.chain(rest).find(|&i| {
                let c = chars[i].to_ascii_lowercase();
                c.is_ascii_lowercase() && !taken.contains(&c)
            });
            if let Some(i) = pick {
                let c = chars[i].to_ascii_lowercase();
                taken.push(c);
                out.push((s, i, c));
            }
        }
        out
    }

    /// An item drawn: its glyph, its label with its key's letter lit, on
    /// the selection's ground when the cursor's on it.
    fn item_line(glyph: &str, label: &str, key: Option<usize>, on: bool, col: Color) -> Line {
        let mut l = Line::new();
        if !glyph.is_empty() {
            l.spans.push(sp(
                format!("{glyph} "),
                if glyph == "↗" { BLUE } else { DIM },
            ));
        }
        for (i, c) in label.chars().enumerate() {
            let s = c.to_string();
            if key == Some(i) {
                l.spans.push(spb(s, ORANGE));
            } else {
                l.spans.push(sp(s, col));
            }
        }
        // Runs of one colour joined again.
        let mut spans: Vec<Span> = Vec::new();
        for s in l.spans {
            match spans.last_mut() {
                Some(p) if p.style == s.style => p.text.push_str(&s.text),
                _ => spans.push(s),
            }
        }
        let mut l = Line::from_spans(spans);
        if on {
            for s in &mut l.spans {
                s.style.bg = SEL_BG;
            }
        }
        l
    }

    /// The rows w wide, at most h, and where each slot landed.
    fn layout(&self, sh: &Shown, w: usize, h: usize) -> (Vec<Line>, Vec<Hit>) {
        let keys = self.letters(sh);
        let key_of = |s: Slot| keys.iter().find(|k| k.0 == s).map(|k| k.1);
        let mut rows: Vec<Line> = Vec::new();
        let mut hits: Vec<Hit> = Vec::new();

        // The links out, ↗ first, then what copies, each flowing on as
        // many rows as the width needs.
        let flow = |rows: &mut Vec<Line>, hits: &mut Vec<Hit>, which: &[usize]| {
            let mut row = Line::new();
            let mut x = 0;
            for &i in which {
                let it = &sh.links[i];
                let s = Slot::Link(i);
                let l = Self::item_line(it.glyph, &it.label, key_of(s), self.cur == s, TEXT);
                let iw = l.width();
                if x > 0 && x + 3 + iw > w {
                    rows.push(std::mem::take(&mut row));
                    x = 0;
                }
                if x > 0 {
                    row.spans.push(plain("   "));
                    x += 3;
                }
                hits.push(Hit {
                    row: rows.len(),
                    x0: x,
                    x1: x + iw,
                    slot: s,
                });
                join(&mut row, l);
                x += iw;
            }
            if x > 0 {
                rows.push(row);
            }
        };
        let outs: Vec<usize> = (0..sh.links.len())
            .filter(|&i| sh.links[i].glyph == "↗")
            .collect();
        let copies: Vec<usize> = (0..sh.links.len())
            .filter(|&i| sh.links[i].glyph != "↗")
            .collect();
        flow(&mut rows, &mut hits, &outs);
        flow(&mut rows, &mut hits, &copies);

        // The groups, each folded to its name and what's in it, quiet.
        let name_w = sh
            .groups
            .iter()
            .map(|g| rush_core::text::width(g.name))
            .max()
            .unwrap_or(0)
            + 3;
        for (g, grp) in sh.groups.iter().enumerate() {
            let open = self.open == Some(g);
            let s = Slot::Group(g);
            let mark = if open { "▾" } else { "▸" };
            let head = Self::item_line(mark, grp.name, key_of(s), self.cur == s, TEXT);
            let hw = head.width();
            hits.push(Hit {
                row: rows.len(),
                x0: 0,
                x1: hw,
                slot: s,
            });
            let mut row = head;
            if !open {
                let hint = grp
                    .items
                    .iter()
                    .map(|i| i.label.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let lead = (2 + name_w).max(hw + 1);
                let room = w.saturating_sub(lead);
                if room >= 8 {
                    row.spans.push(plain(blanks(lead - hw)));
                    row.spans.push(sp(
                        rush_draw::truncate(&hint, room, "…").into_owned(),
                        FAINT,
                    ));
                }
            }
            rows.push(row);
            if open {
                for (i, it) in grp.items.iter().enumerate() {
                    let s = Slot::Member(g, i);
                    let mut row = line([plain("    ")]);
                    let l = Self::item_line("", &it.label, key_of(s), self.cur == s, SUB);
                    hits.push(Hit {
                        row: rows.len(),
                        x0: 4,
                        x1: 4 + l.width(),
                        slot: s,
                    });
                    join(&mut row, l);
                    rows.push(row);
                }
            }
        }

        // The facts above, dropped first when the rows don't fit, and cut
        // from the end until they fit the width.
        let mut facts = sh.facts.clone();
        let facts_line = loop {
            let mut l = Line::new();
            for (i, (t, c)) in facts.iter().enumerate() {
                if i > 0 {
                    l.spans.push(sp(" · ", FAINT));
                }
                l.spans.push(sp(t.clone(), *c));
            }
            if l.width() <= w || facts.len() <= 1 {
                break l;
            }
            facts.pop();
        };
        if !facts_line.spans.is_empty() && rows.len() + 2 <= h {
            for hit in &mut hits {
                hit.row += 2;
            }
            rows.insert(0, Line::new());
            rows.insert(0, fit(facts_line, w));
        }
        rows.truncate(h.max(1));
        hits.retain(|x| x.row < rows.len());
        (rows, hits)
    }

    fn hits_now(&self, sh: &Shown) -> Vec<Hit> {
        if self.hits.is_empty() {
            self.layout(sh, self.w, usize::MAX).1
        } else {
            self.hits.clone()
        }
    }

    /// Does what a slot does: a group unfolds or folds, anything else is
    /// done (and the popover closes, but for what opens in its place).
    fn pick(&mut self, m: &mut Model, sh: &Shown, s: Slot) -> Out {
        self.cur = s;
        match s {
            Slot::Group(g) => {
                if self.open == Some(g) {
                    self.open = None;
                } else {
                    self.open = Some(g);
                    if !sh.groups[g].items.is_empty() {
                        self.cur = Slot::Member(g, 0);
                    }
                }
                self.hits.clear();
                Out::Draw
            }
            Slot::Link(i) => self.run(m, sh.links[i].act.clone()),
            Slot::Member(g, i) => self.run(m, sh.groups[g].items[i].act.clone()),
        }
    }

    fn run(&mut self, m: &mut Model, d: Do) -> Out {
        let pr_url = |p: &PR| match &self.link {
            GhLink::Pr { repo, n } => ghpr::pr_url(repo, *n),
            _ => p.url.clone(),
        };
        match d {
            Do::Browse(u) => place::browse(m, &u),
            Do::Copy(t) => m.copy_text(t),
            Do::Approve(p) => prsheet::ask_approve_link(m, &pr_url(&p), &p),
            Do::Merge(p, how) => prsheet::ask_merge_link(m, &pr_url(&p), &p, how),
            Do::Write(p, what) => {
                prsheet::open_pr_link_writing(m, &pr_url(&p), &p, what);
                return Out::Draw;
            }
            Do::Sheet(p) => {
                prsheet::open_pr_link(m, &pr_url(&p), &p);
                return Out::Draw;
            }
            Do::Rerun(r, failed) => ask_rerun(m, r, failed),
        }
        Out::Close
    }

    /// The cursor to the nearest slot on the next row with any up (dir
    /// -1) or down (+1).
    fn step_row(&mut self, hits: &[Hit], dir: isize) {
        let Some(me) = hits.iter().find(|h| h.slot == self.cur) else {
            if let Some(h) = hits.first() {
                self.cur = h.slot;
            }
            return;
        };
        let rows = hits.iter().map(|h| h.row);
        let next = if dir < 0 {
            rows.filter(|&r| r < me.row).max()
        } else {
            rows.filter(|&r| r > me.row).min()
        };
        let Some(r) = next else {
            return;
        };
        if let Some(h) = hits
            .iter()
            .filter(|h| h.row == r)
            .min_by_key(|h| h.x0.abs_diff(me.x0))
        {
            self.cur = h.slot;
        }
    }

    /// The cursor along its row: left (-1) or right (+1); false when
    /// there's nothing that way on it.
    fn step_along(&mut self, hits: &[Hit], dir: isize) -> bool {
        let Some(me) = hits.iter().find(|h| h.slot == self.cur).copied() else {
            return false;
        };
        let on_row = hits.iter().filter(|h| h.row == me.row);
        let next = if dir < 0 {
            on_row.filter(|h| h.x0 < me.x0).max_by_key(|h| h.x0)
        } else {
            on_row.filter(|h| h.x0 > me.x0).min_by_key(|h| h.x0)
        };
        match next {
            Some(h) => {
                self.cur = h.slot;
                true
            }
            None => false,
        }
    }

    /// Folds the open group, the cursor back on it.
    fn fold(&mut self) -> bool {
        let Some(g) = self.open.take() else {
            return false;
        };
        self.cur = Slot::Group(g);
        self.hits.clear();
        true
    }
}

/// Asks before running an Actions run's jobs again on GitHub.
fn ask_rerun(m: &mut Model, r: Run, failed: bool) {
    let what = if failed {
        "its failed jobs"
    } else {
        "all its jobs"
    };
    let title = one_line(&r.title);
    m.ask(
        Confirm::new(format!("Re-run {what} on GitHub?"), move |m: &mut Model| {
            let (repo, id) = (r.repo.clone(), r.id);
            let url = ghpr::run_url(&r.repo, r.id);
            m.later(
                move || Gh::shared().rerun(&repo, id, failed),
                move |m, res| {
                    match res {
                        Ok(()) => {
                            m.flash("asked GitHub to run it again");
                            forget_link(&url);
                        }
                        Err(e) => m.fail(&format!("GitHub said no: {e}")),
                    }
                    Out::Draw
                },
            );
            Out::Draw
        })
        .detail(title),
    );
}

impl Sheet for LinkPop {
    fn popover(&self, m: &Model) -> Option<Popover> {
        let sh = self.shown();
        let (state, col) = sh.state.clone().unwrap_or((String::new(), DIM));
        Some(Popover {
            title: line([spb(sh.title, TEXT)]),
            state: line([sp(state, col)]),
            width: POP_WIDTH.min(m.w),
            at: self.at,
        })
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.refresh();
        let sh = self.shown();
        let slots = self.slots(&sh);
        if !slots.contains(&self.cur) {
            self.cur = slots.first().copied().unwrap_or(Slot::Link(0));
        }
        let (rows, hits) = self.layout(&sh, w, h);
        self.w = w;
        self.hits = hits;
        rows
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let sh = self.shown();
        let hits = self.hits_now(&sh);
        match k.s.as_str() {
            "esc" => {
                if self.fold() {
                    return Out::Draw;
                }
                return Out::Close;
            }
            "up" => self.step_row(&hits, -1),
            "down" => self.step_row(&hits, 1),
            "tab" | "shift+tab" => {
                let slots = self.slots(&sh);
                let i = slots.iter().position(|s| *s == self.cur).unwrap_or(0);
                let n = slots.len().max(1);
                let j = if k.s == "tab" {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                if let Some(s) = slots.get(j) {
                    self.cur = *s;
                }
            }
            "left" => {
                if !self.step_along(&hits, -1) {
                    match self.cur {
                        Slot::Member(..) => {
                            self.fold();
                        }
                        Slot::Group(g) if self.open == Some(g) => {
                            self.fold();
                        }
                        _ => return Out::Idle,
                    }
                }
            }
            "right" => {
                if !self.step_along(&hits, 1) {
                    match self.cur {
                        Slot::Group(g) if self.open != Some(g) => {
                            return self.pick(m, &sh, Slot::Group(g));
                        }
                        _ => return Out::Idle,
                    }
                }
            }
            "enter" | "space" => return self.pick(m, &sh, self.cur),
            s => {
                let mut ch = s.chars();
                let (Some(c), None) = (ch.next(), ch.next()) else {
                    return Out::Idle;
                };
                let c = c.to_ascii_lowercase();
                let Some(&(slot, _, _)) = self.letters(&sh).iter().find(|x| x.2 == c) else {
                    return Out::Idle;
                };
                return self.pick(m, &sh, slot);
            }
        }
        Out::Draw
    }

    fn mouse(&mut self, m: &mut Model, ev: MouseEv, x: i32, y: i32) -> Out {
        if ev != MouseEv::Press {
            return Out::Idle;
        }
        let sh = self.shown();
        let hit = self.hits_now(&sh).into_iter().find(|h| {
            y >= 0 && x >= 0 && h.row == y as usize && (h.x0..h.x1).contains(&(x as usize))
        });
        match hit {
            Some(h) => self.pick(m, &sh, h.slot),
            None => Out::Idle,
        }
    }
}
