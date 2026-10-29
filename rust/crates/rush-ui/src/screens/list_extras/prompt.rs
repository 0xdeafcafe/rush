//! The Agents box under the list ("new agent in …") and the row of keys
//! under it. After Go's internal/ui/view.go (promptBoxAt :2662,
//! promptLines :2683, dirLabel :2491, shortDir :2502, keysFit :800,
//! folderKey :813, fewKeys :829, needingYou :835, needsLabel :845),
//! model.go (startDirs :627, readStartDirs :666, startDir :708, followDir
//! :721, workDir :735, inTree :746, pickDir :758), picker.go:125
//! (agentDir) and fleetslash.go (isHashCmd :86, typingHash :92). Not
//! here: the shelf of what a new agent starts as (shelf.go, the start
//! sheet's), plugins' notes on the box's edge (boxNote), the # picker
//! over it (fleetslash.go) and the split view's keys (splitHint).

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_draw::{Button, MouseKind};
use rush_fleet::Predicates;

use crate::frame::{InputBox, Mouse, Out};
use crate::list::is_temp;
use crate::model::Model;
use crate::style::*;

/// What the box keeps between frames: the first wrapped row shown, and
/// where it was drawn, for clicks.
#[derive(Default)]
pub(super) struct PromptState {
    top: usize,
    /// The box's first row on screen, and the box as drawn.
    y: usize,
    drawn: Option<InputBox>,
}

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

/// An agent working in a linked worktree (inTree): its checkout's root
/// isn't its repository.
pub fn in_tree(a: &Agent, root: &str) -> bool {
    !root.is_empty() && !a.repo.is_empty() && a.repo != root
}

/// Where an agent works now, as far as its transcript says (agentDir).
pub fn agent_dir(a: &Agent) -> &str {
    if a.spend.dir.is_empty() {
        &a.job.cwd
    } else {
        &a.spend.dir
    }
}

/// Whether d is somewhere to start work: not a temp folder, nor rush's
/// or an agent's own state (workDir).
pub fn work_dir(d: &str) -> bool {
    if d.is_empty() || is_temp(d) {
        return false;
    }
    let state = crate::config::dir();
    if std::path::Path::new(d).starts_with(&state) {
        return false;
    }
    !d.contains("/projects/-")
}

/// The folder a new session takes from agent i: the top of its
/// repository, not the subfolder it's in; for one in a linked worktree,
/// the checkout it was made from (followDir).
pub fn follow_dir(m: &Model, i: usize) -> String {
    let a = &m.agents[i];
    let root = m.roots.get(i).map_or("", String::as_str);
    if in_tree(a, root) {
        return root.to_string();
    }
    [a.repo.as_str(), agent_dir(a), a.job.cwd.as_str()]
        .into_iter()
        .find(|d| work_dir(d))
        .unwrap_or("")
        .to_string()
}

/// The folders a new session can start in: where rush was opened, then
/// those of agents open, then of the latest, twelve at most
/// (readStartDirs).
pub fn start_dirs(m: &Model) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if !m.launch_dir.is_empty() {
        out.push(m.launch_dir.clone());
    }
    let mut agents: Vec<&Agent> = m.agents.iter().collect();
    agents.sort_by(|a, b| {
        b.job
            .open()
            .cmp(&a.job.open())
            .then(b.job.updated_at.cmp(&a.job.updated_at))
    });
    for a in agents {
        if out.len() >= 12 {
            break;
        }
        if (a.interactive || a.past) && is_temp(&a.job.cwd) {
            continue;
        }
        let d = &a.job.cwd;
        if !d.is_empty() && !out.contains(d) {
            out.push(d.clone());
        }
    }
    out
}

/// Where a new session starts: the picked agent's folder, else where
/// rush was opened (startDir).
pub fn start_dir(m: &Model) -> String {
    if let Some(i) = m.focused() {
        let d = follow_dir(m, i);
        if !d.is_empty() {
            return d;
        }
    }
    start_dirs(m).into_iter().next().unwrap_or_default()
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

/// The box's edge label, what it says while empty, as the text calls
/// for (promptLines' switch).
fn labels(m: &Model, text: &str) -> (Line, String) {
    if typing_hash(text) {
        return (
            line([sp("rush command · enter runs it", DIM)]),
            String::new(),
        );
    }
    let dir = start_dir(m);
    (
        line([sp("new agent in ", DIM), sp(dir_label(m, &dir), TEXT)]),
        "describe a task".into(),
    )
}

/// The box at width w, before it's drawn (promptBoxAt).
fn box_at(m: &mut Model, w: usize) -> InputBox {
    let max_rows = 6.min(1.max(m.h as isize - m.top_h() as isize - 4 - 5) as usize);
    let top = m.kept::<PromptState>().top;
    let text = m.input.string();
    let (top_l, holder) = labels(m, &text);
    let b = InputBox {
        w,
        focused: true,
        top_l,
        text: m.input.text.clone(),
        cursor: m.input.pos,
        anchor: m.input.anchor,
        holder,
        lead: line([sp("❯ ", ORANGE)]),
        max_rows,
        top,
        // An empty box with nothing asked of it shows no cursor: the
        // list has the keys until you type.
        idle: m.input.is_empty(),
        ..InputBox::default()
    };
    b.scrolled()
}

/// The box under the list: a list_dock (promptLines' box).
pub(super) fn dock(m: &mut Model, w: usize) -> Vec<Line> {
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
pub(super) fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.session.is_some()
        || m.on_place()
        || m.sheet.is_some()
        || m.confirm.is_some()
        || e.kind != MouseKind::Down(Button::Left)
    {
        return Out::Pass;
    }
    let st = m.kept::<PromptState>();
    let Some(b) = st.drawn.clone() else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let rows = b.rows();
    if y <= st.y || y > st.y + rows {
        return Out::Pass;
    }
    let p = b.at(y - st.y - 1, x as isize);
    m.input.pos = p.min(m.input.text.len());
    m.input.anchor = None;
    Out::Draw
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
/// (folderKey): a worktree's agent offers the checkout it came from;
/// otherwise the folder picker, when there's a choice.
fn folder_key(m: &Model, a: Option<usize>) -> Option<(String, String)> {
    if let Some(i) = a {
        let ag = &m.agents[i];
        let root = m.roots.get(i).map_or("", String::as_str);
        if in_tree(ag, root) {
            let other = ag.repo.rsplit('/').next().unwrap_or(&ag.repo);
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
pub(super) fn hint(m: &Model, w: usize) -> Option<Line> {
    if m.session.is_some() || m.on_place() {
        return None;
    }
    let w = w.saturating_sub(4);
    let text = m.input.string();
    let a = m.selected();
    let needs = needing_you(m);
    let keys = if typing_hash(&text) {
        keys_fit(w, &[("enter", "run it"), ("esc", "clear"), ("?", "guide")])
    } else if !text.is_empty() {
        let mut pairs = vec![
            ("enter".to_string(), "start".to_string()),
            ("shift+tab".to_string(), "model".to_string()),
        ];
        if let Some(fk) = folder_key(m, a) {
            pairs.push(fk);
        }
        few_keys(w, 3, &pairs, &[("esc", "clear"), ("?", "more")])
    } else {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        if a.is_some() {
            pairs.push(p("enter", "open"));
            pairs.push(p("ctrl+o", "reply"));
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
