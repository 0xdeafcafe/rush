//! The Agents box under the list ("new agent in …") and the row of keys
//! under it. After Go's internal/ui/view.go (promptBoxAt :2662,
//! promptLines :2683, dirLabel :2491, shortDir :2502, keysFit :800,
//! folderKey :813, fewKeys :829, needingYou :835, needsLabel :845),
//! model.go (startDirs :627, readStartDirs :666, startDir :708, followDir
//! :721, workDir :735, inTree :746, pickStartDir :760, pickDir :769),
//! picker.go:205 (setStartDir), picker.go:125
//! (agentDir) and fleetslash.go (isHashCmd :86, typingHash :92); the
//! shelf on its top edge is shelf.rs, the split's key back splitHint
//! (split.rs); plugins' notes on its bottom edge, the stash's "stashed ·
//! back after you send" among them (view.go:2722, pluginui.go:472
//! boxNote). Not here: the # picker over it (fleetslash.go).

use rush_core::text::{Line, width};
use rush_draw::{Button, MouseKind};
use rush_fleet::Predicates;

use crate::accounts::StartOver;
use crate::frame::{InputBox, Mouse, Out};
use crate::model::Model;
use crate::style::*;

/// What the box keeps between frames: the first wrapped row shown, and
/// where it was drawn, for clicks.
#[derive(Default)]
pub struct PromptState {
    top: usize,
    /// The box's first row on screen, and the box as drawn.
    y: usize,
    drawn: Option<InputBox>,
    /// What a click takes on the box's top edge, by column: the folder,
    /// its own worktree chip, the shelf's setups (shelfHits), the parts
    /// of its chip.
    hits: Vec<(usize, usize, EdgeAct)>,
    /// What a click takes on its bottom edge: a repository's offer.
    foot_hits: Vec<(usize, usize, EdgeAct)>,
    /// The part of an edge under the pointer, lit: the bottom one or the
    /// top, from and to.
    lit: Option<(bool, usize, usize)>,
}

/// What a click on a part of the box's top edge does.
#[derive(Clone, Debug)]
pub enum EdgeAct {
    /// The folder new sessions start in: ctrl+l's picker.
    Dir,
    /// Another setup on the shelf: the next session starts as it
    /// (shelfClick).
    Pick(StartOver),
    /// A part of the chip: the Start sheet on that row, as #model and
    /// #effort open it.
    Sheet(usize),
    /// The own worktree chip: switches whether a session started in the
    /// project gets a worktree of its own (#worktree on|off).
    Worktree(String),
    /// A repository's address typed: what tab does with it (gitlink).
    Repo,
}

/// Label parts: from and to, cells into the label, and what a click does.
type Parts = Vec<(usize, usize, EdgeAct)>;

/// Whether text is a # command: # and a letter, so a heading (# Plan) or
/// an issue (#123) is still a message (isHashCmd).
pub fn is_hash_cmd(text: &str) -> bool {
    let mut c = text.chars();
    c.next() == Some('#') && c.next().is_some_and(char::is_alphabetic)
}

/// A # command is being typed, # alone included (typingHash).
pub fn typing_hash(text: &str) -> bool {
    text == "#" || is_hash_cmd(text)
}

// Where new sessions start is the base's: every screen asks (crate::startdir).
pub use crate::startdir::{
    StartPick, agent_dir, follow_dir, in_tree, start_dir, start_dirs, start_in_tree, work_dir,
};

/// alt+l, or ctrl+] l, in the list: new sessions from an agent in a
/// worktree start in its main checkout or the worktree itself
/// (keys.go:437).
pub fn worktree_key(m: &mut Model, k: &crate::frame::Press) -> Out {
    if k.s != "alt+l" {
        return Out::Pass;
    }
    let p = m.kept::<StartPick>();
    p.in_tree = !p.in_tree;
    let d = tildify(&start_dir(m));
    m.flash(&format!("new sessions start in {d}"));
    Out::Draw
}

/// Folder i of start_dirs picked for new sessions: it holds while the
/// agent selected now stays selected (pickStartDir).
pub fn pick_start_dir(m: &mut Model, i: usize) {
    let for_key = m
        .focused()
        .map(|f| m.agents[f].key.clone())
        .unwrap_or_default();
    let p = m.kept::<StartPick>();
    p.idx = i;
    p.for_key = for_key;
    p.set = true;
}

/// dir made where new sessions start, kept first in the list when it
/// wasn't in it (setStartDir).
pub fn set_start_dir(m: &mut Model, dir: &str) {
    loop {
        if let Some(i) = start_dirs(m).iter().position(|d| d == dir) {
            pick_start_dir(m, i);
            return;
        }
        let p = m.kept::<StartPick>();
        if p.dir == dir {
            return;
        }
        p.dir = dir.to_string();
    }
}

/// A path in at most n cells, its middle left out, so the folder's own
/// name always shows: ~/…/langwatch/.worktrees/go-ports (shortDir).
pub fn short_dir(p: &str, n: usize) -> String {
    if rush_core::text::width(p) <= n {
        return p.to_string();
    }
    let parts: Vec<&str> = p.split('/').collect();
    let mut tail = parts[parts.len() - 1].to_string();
    for i in (1..parts.len().saturating_sub(1)).rev() {
        let next = format!("{}/{tail}", parts[i]);
        if rush_core::text::width(&format!("{}/…/{next}", parts[0])) > n {
            break;
        }
        tail = next;
    }
    rush_draw::truncate(&format!("{}/…/{tail}", parts[0]), n, "…").into_owned()
}

/// A folder named by its repository, not a branch: the agent decides at
/// the start whether it works there or in a new worktree (dirLabel).
pub fn dir_label(m: &Model, dir: &str) -> String {
    for a in &m.agents {
        if a.job.cwd == dir && a.repo == dir {
            return a.repo.rsplit('/').next().unwrap_or(&a.repo).to_string();
        }
    }
    short_dir(&tildify(dir), 48)
}

/// The box's edge labels, left and right, what it says while empty, and
/// the parts of each a click takes, as the text calls for (promptLines'
/// switch).
fn labels(m: &mut Model, text: &str, w: usize) -> (Line, Line, String, Parts, Parts) {
    if let Some((top_l, holder)) = super::rename::labels(m) {
        return (top_l, Line::new(), holder, Vec::new(), Vec::new());
    }
    if typing_hash(text) {
        return (
            line([sp("rush command · enter runs it", DIM)]),
            // What #new starts, or why it can't (view.go:2741 newPreview).
            rush_ui_accounts::newcmd::new_preview(m, text).unwrap_or_default(),
            String::new(),
            Vec::new(),
            Vec::new(),
        );
    }
    let dir = start_dir(m);
    let lead = "new agent in ";
    let mut top_l = line([sp(lead, DIM), sp(dir_label(m, &dir), TEXT)]);
    // The folder: a click picks another, as ctrl+l does.
    let mut left = vec![(width(lead), top_l.width(), EdgeAct::Dir)];
    // In a repository, whether it gets a worktree of its own: a click
    // switches it for the project.
    let (project, repo) = super::gitlink::box_project(m);
    if repo {
        top_l.spans.push(plain(" "));
        let from = top_l.width();
        join(&mut top_l, worktree_chip(m, &project));
        left.push((from, top_l.width(), EdgeAct::Worktree(project)));
    }
    // What it starts as, so a model or profile is never a surprise,
    // beside the others it could: ⌥1…9 or a click takes one; a click on
    // the chip's name, model or effort opens the Start sheet there.
    let room = w.saturating_sub(top_l.width() + 10);
    let (top_r, hits) = super::shelf::shelf_line(m, &dir, room);
    let mut right = Vec::new();
    for h in hits {
        if h.chip {
            for (f, t, row) in super::shelf::chip_parts(m, &h.o) {
                right.push((h.x0 + f, (h.x0 + t).min(h.x1), EdgeAct::Sheet(row)));
            }
        } else {
            right.push((h.x0, h.x1, EdgeAct::Pick(h.o)));
        }
    }
    (top_l, top_r, "describe a task".into(), left, right)
}

/// The own worktree chip: on, sessions started from the box in project
/// get a worktree and branch of their own; off, they work in the
/// checkout itself.
pub fn worktree_chip(m: &Model, project: &str) -> Line {
    let on = crate::accounts::cfg(m).own_worktree(project);
    let mut l = if on {
        line([sp(" own worktree ", TEXT)])
    } else {
        line([sp(" in the checkout ", DIM)])
    };
    for s in &mut l.spans {
        s.style.bg = rush_ui_accounts::setup::BAR_CHIP;
    }
    l
}

/// The own worktree chip clicked: the project's switched and saved.
fn switch_worktree(m: &mut Model, project: &str) -> Out {
    use crate::accounts::{cfg, cfg_mut, save};
    let on = !cfg(m).own_worktree(project);
    cfg_mut(m).own_worktrees.insert(project.to_string(), on);
    save(m);
    let name = project.rsplit('/').next().unwrap_or(project);
    m.flash(&if on {
        format!("new sessions in {name} get a worktree and branch of their own")
    } else {
        format!("new sessions in {name} work in the checkout itself")
    });
    Out::Draw
}

/// The box at width w, before it's drawn (promptBoxAt).
fn box_at(m: &mut Model, w: usize) -> InputBox {
    let max_rows = 6.min(1.max(m.h as isize - m.top_h() as isize - 4 - 5) as usize);
    let top = m.kept::<PromptState>().top;
    let text = m.input.string();
    let (top_l, top_r, mut holder, left, right) = labels(m, &text, w);
    let embedded = super::live::embedded(m);
    if m.keys_beside || embedded {
        // The Session has the keys; this box waits, and says how back.
        holder = if embedded {
            "ctrl+] or click here to come back".into()
        } else {
            "esc or ← to come back here".into()
        };
    }
    // What plugins put on its edge, while it's the Prompt (view.go:2722).
    let foot_r = if super::rename::renaming(m).is_none() {
        rush_ui_plugin_ui::hooks::box_note(m, "")
    } else {
        Line::new()
    };
    // A repository's address typed: what tab does with it, on the left.
    let foot_l = if super::rename::renaming(m).is_none() && !m.keys_beside && !embedded {
        super::gitlink::offer_line(m, w.saturating_sub(foot_r.width() + 8)).unwrap_or_default()
    } else {
        m.kept::<super::gitlink::GitState>().shown = None;
        Line::new()
    };
    let foot = if foot_l.spans.is_empty() {
        Vec::new()
    } else {
        vec![(0, foot_l.width(), EdgeAct::Repo)]
    };
    let b = InputBox {
        w,
        foot_l,
        foot_r,
        // The Session beside it has the keys: its edge goes quiet
        // (promptBoxAt's focused).
        focused: !m.keys_beside && !embedded,
        top_l,
        top_r,
        text: m.input.text.clone(),
        cursor: m.input.pos,
        anchor: m.input.anchor,
        holder,
        lead: line([sp("❯ ", ORANGE)]),
        max_rows,
        top,
        // An empty box with nothing asked of it shows no cursor: the
        // list has the keys until you type.
        idle: m.input.is_empty() && super::rename::renaming(m).is_none(),
        // The paste chip under the pointer (view.go:2680).
        hot: m.input.hot,
        ..InputBox::default()
    };
    // Where the parts landed, as the edge drew them: cut short, or with no
    // room, they take no clicks.
    let at = b.edge_at(false);
    let mut hits = at.place(false, &left);
    hits.extend(at.place(true, &right));
    let foot_hits = b.edge_at(true).place(false, &foot);
    let st = m.kept::<PromptState>();
    st.lit = st.lit.filter(|&(f, x0, x1)| {
        let on = if f { &foot_hits } else { &hits };
        on.iter().any(|h| (h.0, h.1) == (x0, x1))
    });
    let mut b = b;
    b.lit = st.lit;
    st.hits = hits;
    st.foot_hits = foot_hits;
    // A dictation under way shows at the cursor (view.go:2719).
    if b.focused
        && m.sheet.is_none()
        && m.confirm.is_none()
        && let Some(l) = rush_ui_social::dictation::look(m)
    {
        b.idle = false;
        b.live = Some(l);
    }
    b.scrolled()
}

/// The box under the list: a list_dock (promptLines' box).
pub fn dock(m: &mut Model, w: usize) -> Vec<Line> {
    if m.session.is_some() {
        return Vec::new();
    }
    let b = box_at(m, w);
    let rows = b.lines();
    // The docks sit at the foot of the body, over the row of keys.
    let y = m.h.saturating_sub(1 + rows.len());
    let st = m.kept::<PromptState>();
    st.top = b.top;
    st.y = y;
    st.drawn = Some(b);
    rows
}

/// A click in the box puts the cursor there (the prompt's click in
/// model.go's mouse).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.session.is_some() || m.on_place() || m.sheet.is_some() || m.confirm.is_some() {
        return Out::Pass;
    }
    let st = m.kept::<PromptState>();
    let Some(b) = st.drawn.clone() else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let foot_y = st.y + b.rows() + 1;
    let hit = if y == st.y {
        st.hits.iter().find(|h| x >= h.0 && x < h.1).cloned()
    } else if y == foot_y {
        st.foot_hits.iter().find(|h| x >= h.0 && x < h.1).cloned()
    } else {
        None
    };
    if e.kind == MouseKind::Moved {
        // A part of an edge a click takes, lit under the pointer.
        let lit = hit.as_ref().map(|h| (y == foot_y, h.0, h.1));
        let edge = st.lit != lit;
        st.lit = lit;
        // The paste chip under the pointer lit (hoverChip).
        let at =
            (y > st.y && y <= st.y + b.rows() && x < b.w).then(|| b.at(y - st.y - 1, x as isize));
        return if m.input.hover(at) || edge {
            Out::Draw
        } else {
            Out::Pass
        };
    }
    if e.kind != MouseKind::Down(Button::Left) {
        return Out::Pass;
    }
    if let Some((_, _, act)) = hit {
        st.lit = None;
        return edge_click(m, act);
    }
    let rows = b.rows();
    if y <= st.y || y > st.y + rows {
        return Out::Pass;
    }
    let p = b.at(y - st.y - 1, x as isize);
    // An image's chip opens the image, the draft as it was (draftImage).
    if let Some(sp) = crate::imagerefs::chip_on(&m.input.text, p) {
        let marker: String = m.input.text[sp.from..sp.to].iter().collect();
        let refs = m.imgs.clone();
        let o = m.ext.calls.draft_image()(m, &marker, &refs);
        if o.used() {
            return o;
        }
    }
    // A paste chip clicked opens in place (clickBox's openChip).
    if m.input.open_chip_at(p) {
        return Out::Draw;
    }
    m.input.pos = p.min(m.input.text.len());
    m.input.anchor = None;
    Out::Draw
}

/// What a click takes on the box's top edge as last drawn, and the row
/// it's on.
#[doc(hidden)]
pub fn edge_hits(m: &mut Model) -> (usize, Vec<(usize, usize, EdgeAct)>) {
    let st = m.kept::<PromptState>();
    (st.y, st.hits.clone())
}

/// A click on a part of the box's top edge.
fn edge_click(m: &mut Model, act: EdgeAct) -> Out {
    match act {
        EdgeAct::Dir => m.ext.calls.open_dir_picker()(m),
        // The shelf's other setups (shelfClick).
        EdgeAct::Pick(o) => {
            super::shelf::pick(m, o);
            Out::Draw
        }
        EdgeAct::Sheet(row) => rush_ui_accounts::startsheet::open_on(m, row, ""),
        EdgeAct::Worktree(project) => switch_worktree(m, &project),
        EdgeAct::Repo => match super::gitlink::offer(m) {
            Some(o) => super::gitlink::take(m, o),
            None => Out::Draw,
        },
    }
}

/// How many agents wait on you (needingYou).
pub fn needing_you(m: &Model) -> usize {
    m.order.iter().filter(|&&i| m.agents[i].needs_you()).count()
}

/// "1 needs you", "3 need you" (needsLabel).
pub fn needs_label(n: usize) -> String {
    if n == 1 {
        "1 needs you".into()
    } else {
        format!("{n} need you")
    }
}

/// The first n of pairs, then always, fitted to w: a row of keys says
/// what matters now and leaves the rest to ? (fewKeys).
pub fn few_keys(w: usize, n: usize, pairs: &[(String, String)], always: &[(&str, &str)]) -> Line {
    let mut all: Vec<(&str, &str)> = pairs
        .iter()
        .take(n)
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    all.extend_from_slice(always);
    keys_fit(w, &all)
}

/// The key that changes where a new agent starts, and what it does
/// (folderKey): a worktree's agent offers its worktree or the checkout it
/// came from, by name; otherwise the folder picker, when there's a choice.
fn folder_key(m: &Model, a: Option<usize>) -> Option<(String, String)> {
    if let Some(i) = a {
        let ag = &m.agents[i];
        let root = m.roots.get(i).map_or("", String::as_str);
        let picked = m
            .kept
            .peek::<StartPick>()
            .is_some_and(|p| p.set && p.for_key == ag.key);
        if in_tree(ag, root) && !picked {
            let dir = if start_in_tree(m) { root } else { &ag.repo };
            let other = dir.rsplit('/').next().unwrap_or(dir);
            return Some((
                rush_convo::render::key_word("ctrl+] l"),
                format!("start in {other}"),
            ));
        }
    }
    (start_dirs(m).len() > 1).then(|| ("ctrl+l".to_string(), "folder".to_string()))
}

/// The row of keys under the box: what you can do right now, ? the rest
/// (promptLines' hint).
pub fn hint(m: &Model, w: usize) -> Option<Line> {
    if m.session.is_some() || m.on_place() {
        return None;
    }
    let w = w.saturating_sub(4);
    let text = m.input.string();
    let a = m.selected();
    let needs = needing_you(m);
    let embedded = super::live::embedded(m);
    let keys = if m.keys_beside || embedded {
        // The keys are the Session's: only what reaches back here.
        let label = needs_label(needs);
        let mut pairs = Vec::new();
        if needs > 0 {
            pairs.push(("ctrl+n", label.as_str()));
        }
        let back = if embedded { "ctrl+]" } else { "esc · ←" };
        pairs.push((back, "back to Agents"));
        keys_fit(w, &pairs)
    } else if super::rename::renaming(m).is_some() {
        super::rename::hint(w)
    } else if typing_hash(&text) {
        keys_fit(w, &[("enter", "run it"), ("esc", "clear"), ("?", "guide")])
    } else if !text.is_empty() {
        let mut pairs = vec![
            ("enter".to_string(), "start".to_string()),
            ("shift+tab".to_string(), "model".to_string()),
        ];
        // A repository's address typed: tab moves there, or clones it.
        match m
            .kept
            .peek::<super::gitlink::GitState>()
            .and_then(|g| g.shown.clone())
        {
            Some(super::gitlink::Offer::Move { .. }) => {
                pairs.insert(1, ("tab".into(), "move there".into()));
            }
            Some(super::gitlink::Offer::Clone { .. }) => {
                pairs.insert(1, ("tab".into(), "clone it".into()));
            }
            None => {}
        }
        if let Some(fk) = folder_key(m, a) {
            pairs.push(fk);
        }
        few_keys(w, 3, &pairs, &[("esc", "clear"), ("?", "more")])
    } else if super::peek::peeking(m) {
        super::peek::hint(m, w)
    } else {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        if let Some(i) = a {
            pairs.push(p("enter", "open"));
            if !m.agents[i].rush {
                pairs.push(p("ctrl+o", "reply"));
            }
        }
        if needs > 0 {
            pairs.push(p("ctrl+n", &needs_label(needs)));
        }
        pairs.push(p("ctrl+k", "go anywhere"));
        if a.is_some() {
            pairs.push(p("ctrl+r", "rename"));
        }
        if let Some(i) = a {
            let ag = &m.agents[i];
            if ag.halted() || ag.your_turn(m.now) {
                pairs.insert(0, p("ctrl+b", ag.continue_text()));
            }
        }
        let n = rush_ui_settings::updates::count(m);
        if n > 0 {
            pairs.push((format!("{n} updates"), "in Settings".into()));
        }
        if m.ext.calls.hosted_id()(m).is_some() {
            // Hosted's list: no command bar, and the way back to the
            // session (view.go:2842).
            pairs.retain(|(k, _)| k != "ctrl+k");
            pairs.insert(0, p("esc · ctrl+6", "hide Agents"));
        }
        // With one side on screen, how to have both is kept in view.
        if let Some(v) = super::split::hint(m) {
            let at = pairs.len().min(1);
            pairs.insert(at, v);
        }
        pairs.push(p("?", "more"));
        let all: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        keys_fit(w, &all)
    };
    let mut l = line([plain("  ")]);
    join(&mut l, keys);
    Some(l)
}
