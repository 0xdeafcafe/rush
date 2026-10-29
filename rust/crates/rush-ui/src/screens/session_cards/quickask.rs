//! The quick ask: a question for rush's guide, a fast, cheap model that
//! belongs to no chat. It reads the sessions as the list has them and
//! rush's guide, and opens the session it points at. It floats over the
//! top right of whatever is showing, keeps its thread for follow-ups,
//! and can go into a chat's box later as one paste. Its threads are
//! btw's; only who answers and where it's drawn differ. After Go's
//! internal/ui/quickask.go (quickAsk :28, quickSystem :41, guideFleet
//! :57, guideOpens :103, openGuide :117, askCheap :144, newThread :186,
//! toggleQuick :199, quickThread :214, askQuick :226, insertQuick :259,
//! quickKey :281, clickQuick :337, quickOver :356).
//!
//! The keymap's chords (ctrl+] q, ctrl+] i) reach it through
//! [`super::run_action`]; alt+/ is bound here.

use std::time::{Duration, Instant};

use rush_core::agent::State;
use rush_core::text::Line;
use rush_draw::{Button, MouseKind};

use super::btw::{BTW_BG, Qa, Thread};
use super::sel::{Sel, paint_cols};
use crate::frame::dialog::paste_at;
use crate::frame::editor::edit;
use crate::frame::sheet::text_field;
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// rush's guide, as the quick ask reads it (rush.Guide).
pub const GUIDE: &str = include_str!("../../../../../../docs/guide.md");

/// How many threads are kept, in memory only (quickKeep).
pub const QUICK_KEEP: usize = 5;

/// What the guide is told it is (quickSystem).
pub const QUICK_SYSTEM: &str = "You are rush's guide. rush is the terminal app the user runs and watches their coding agents in; each message brings their sessions as rush's list has them, a number each. Help them find their way: what needs them next, which session is doing what, where something is, how to do something in rush. Sessions marked NEEDS YOU come first, then ones waiting on them, then ones that finished. Don't make decisions for them: point, summarise in a few lines, and say what they'd do there. For a fuller summary of one session, tell them to open it and ask its own agent. When your answer is about one session, end with a line of its own: open: N, and rush opens it for them. They may ask something unrelated to rush too: answer it briefly. Be brief and direct, plain text or light markdown, no preamble. Answer rush questions from its guide, and say when it doesn't cover something.";

/// Answers prompt: the model that did, and what it said.
pub type Asker = fn(&str) -> Result<(String, String), String>;

/// The quick ask's float and its threads (quickAsk).
#[derive(Default)]
pub struct QuickAsk {
    /// The last few, the one in use at cur.
    pub threads: Vec<Thread>,
    pub cur: usize,
    pub shown: bool,
    /// The model that answered last, for the title.
    pub model: String,
    /// Who answers: askCheap with the guide, unless a test set its own.
    pub asker: Option<Asker>,
}

impl QuickAsk {
    /// The thread in use, None before the first (thread).
    pub fn thread(&mut self) -> Option<&mut Thread> {
        self.threads.get_mut(self.cur)
    }

    /// Whether the float has the keys (focused).
    pub fn focused(&self) -> bool {
        self.shown && self.threads.get(self.cur).is_some_and(|t| t.focused)
    }

    /// Starts a thread, dropping the oldest past QUICK_KEEP; an empty one
    /// in use is kept as it is (newThread).
    pub fn new_thread(&mut self) -> &mut Thread {
        if self.threads.get(self.cur).is_some_and(|t| t.qa.is_empty()) {
            return &mut self.threads[self.cur];
        }
        self.threads.push(Thread::default());
        if self.threads.len() > QUICK_KEEP {
            self.threads.remove(0);
        }
        self.cur = self.threads.len() - 1;
        &mut self.threads[self.cur]
    }
}

fn quick(m: &mut Model) -> &mut QuickAsk {
    m.kept::<QuickAsk>()
}

/// Whether the list says agent a needs you now (NeedsYou), roughly: it's
/// blocked and alive.
fn needs_you(a: &rush_core::agent::Agent) -> bool {
    a.job.state == State::Blocked && !a.checking && a.pid != 0
}

/// The sessions as the guide reads them, numbered from 1, and the keys
/// those numbers stand for (guideFleet).
pub fn guide_fleet(m: &Model) -> (String, Vec<String>) {
    let now = super::now(m);
    let mut b = String::new();
    let mut keys = Vec::new();
    for &i in &m.order {
        if keys.len() == 80 {
            break;
        }
        let a = &m.agents[i];
        keys.push(a.key.clone());
        let state = if needs_you(a) {
            "NEEDS YOU".to_string()
        } else if a.past {
            "past".to_string()
        } else {
            a.job.state.as_str().to_string()
        };
        let t = a.job.updated_at.max(a.job.mod_time);
        b.push_str(&format!(
            "[{}] {} · {} · {} ago",
            keys.len(),
            a.display_name,
            state,
            super::clock::round_min(now.saturating_sub(t))
        ));
        if !a.repo.is_empty() {
            let base = std::path::Path::new(&a.repo)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            b.push_str(&format!(" · {base}"));
        }
        if !a.branch.is_empty() {
            b.push_str(&format!(" @{}", a.branch));
        }
        let todos = a.job.todos.len();
        if todos > 0 {
            let done = a.job.todos.iter().filter(|t| t.done).count();
            b.push_str(&format!(" · todos {done}/{todos}"));
        }
        for x in [&a.job.needs, &a.job.intent, &a.job.detail] {
            let x = one_line(x);
            if !x.is_empty() {
                b.push_str(&format!(" · {}", fit_str(&x, 200).trim_end()));
            }
        }
        b.push('\n');
    }
    if keys.is_empty() {
        return ("The user has no sessions in rush yet.".into(), Vec::new());
    }
    if let Some(i) = m.focused() {
        b.push_str(&format!(
            "\nThe one selected now: {}\n",
            m.agents[i].display_name
        ));
    }
    (format!("Their sessions:\n{b}"), keys)
}

/// Takes the guide's `open: N` off its answer, and the key N stood for,
/// "" when it pointed at none (guideOpens).
pub fn guide_opens(answer: &str, keys: &[String]) -> (String, String) {
    let re = regex::Regex::new(r"(?mi)^\s*open:\s*\[?(\d+)\]?\s*$\n?").expect("a regex");
    let Some(last) = re.captures_iter(answer).last() else {
        return (answer.to_string(), String::new());
    };
    let n: usize = last[1].parse().unwrap_or(0);
    let answer = re.replace_all(answer, "").trim().to_string();
    if n < 1 || n > keys.len() {
        return (answer, String::new());
    }
    (answer, keys[n - 1].clone())
}

/// Asks the cheapest model rush can run without a session, the first of
/// its models whose name has prefer when one does (askCheap).
pub fn ask_cheap(
    reg: &rush_core::adapter::Registry,
    system: &str,
    prompt: &str,
    prefer: &str,
) -> Result<(String, String), String> {
    for k in ["claude", "ollama"] {
        let Some(a) = reg.get(k) else { continue };
        let Some(sm) = a.summarizer() else { continue };
        if !reg.installed(k) {
            continue;
        }
        let Ok(ms) = sm.summary_models() else {
            continue;
        };
        let Some(first) = ms.first() else { continue };
        let pick = ms
            .iter()
            .find(|x| !prefer.is_empty() && x.contains(prefer))
            .unwrap_or(first)
            .clone();
        let out = sm
            .summarize(&pick, system, prompt)
            .map_err(|e| e.to_string())?;
        return Ok((pick, out));
    }
    Err("no model to ask: it runs on Claude Code or Ollama".into())
}

/// The guide's answer to prompt: with the guide, else, for a local model
/// that can't hold it, without (quickAsker).
fn guide_asker(
    reg: rush_core::adapter::Registry,
    prompt: String,
) -> Result<(String, String), String> {
    let system = format!("{QUICK_SYSTEM}\n\n<rush-guide>\n{GUIDE}\n</rush-guide>");
    match ask_cheap(&reg, &system, &prompt, "") {
        Err(e) if e.contains("too small") => ask_cheap(&reg, QUICK_SYSTEM, &prompt, ""),
        r => r,
    }
}

/// #guide: the float with the keys, asking q when there is one
/// (openGuide).
pub fn open_guide(m: &mut Model, q: &str) -> Out {
    let qa = quick(m);
    if qa.thread().is_none() || !q.is_empty() {
        qa.new_thread();
    }
    qa.shown = true;
    qa.thread().expect("a thread").focused = true;
    let q = q.trim();
    if !q.is_empty() {
        ask_quick(m, q);
    }
    Out::Draw
}

/// quick.ask: opens the float with the keys, else moves the keys between
/// it and what's under it (toggleQuick).
pub fn toggle_quick(m: &mut Model) -> Out {
    let q = quick(m);
    if q.thread().is_none() {
        q.new_thread();
    }
    if !q.shown {
        q.shown = true;
        q.thread().expect("a thread").focused = true;
        return Out::Draw;
    }
    let t = q.thread().expect("a thread");
    t.focused = !t.focused;
    t.sel = Sel::default();
    Out::Draw
}

/// A thread's answered questions as text (quickThread).
pub fn quick_thread(qa: &[Qa]) -> String {
    qa.iter()
        .filter(|x| !x.response.is_empty())
        .map(|x| format!("Q: {}\nA: {}\n\n", x.question, x.response))
        .collect()
}

/// Asks q in the thread in use, with the thread so far as context
/// (askQuick).
fn ask_quick(m: &mut Model, q: &str) {
    let (sessions, keys) = guide_fleet(m);
    let now = super::now(m);
    let qa = quick(m);
    let cur = qa.cur;
    let asker = qa.asker;
    let t = qa.thread().expect("a thread");
    let mut prompt = q.to_string();
    let before = quick_thread(&t.qa);
    if !before.is_empty() {
        prompt = format!("Earlier in this thread:\n\n{before}Now: {q}");
    }
    let prompt = format!("{sessions}\n{prompt}");
    t.qa.push(Qa {
        question: q.into(),
        response: String::new(),
    });
    t.waiting = now;
    t.err.clear();
    t.scroll = 0;
    t.sel = Sel::default();
    let reg = m.reg.clone();
    m.later(
        move || match asker {
            Some(f) => f(&prompt),
            None => guide_asker(reg, prompt),
        },
        move |m, r| {
            let mut open = String::new();
            let qa = quick(m);
            let Some(t) = qa.threads.get_mut(cur) else {
                return Out::Idle;
            };
            t.waiting = 0;
            // The rows under it moved.
            t.sel = Sel::default();
            match r {
                Err(e) => t.err = e,
                Ok((_, out)) if out.trim().is_empty() => {
                    t.err = "no answer came back: ask again".into()
                }
                Ok((model, out)) => {
                    let (answer, key) = guide_opens(out.trim(), &keys);
                    if let Some(last) = t.qa.last_mut() {
                        last.response = answer;
                    }
                    qa.model = model;
                    open = key;
                }
            }
            if let Some(i) = m.order.iter().copied().find(|&i| m.agents[i].key == open) {
                let name = m.agents[i].display_name.clone();
                if let Some(last) = quick(m).threads.get_mut(cur).and_then(|t| t.qa.last_mut()) {
                    last.response.push_str(&format!("\n\n→ opened {name}"));
                }
                m.open(i);
            }
            Out::Draw
        },
    );
}

/// session.quickask.insert: the thread in use into the open chat's box as
/// one paste, to add a line to and send (insertQuick).
pub fn insert_quick(m: &mut Model) -> Out {
    let Some(key) = super::open_key(m) else {
        m.flash_err("open a chat first: the quick ask goes into its box");
        return Out::Draw;
    };
    let text = quick(m)
        .thread()
        .map(|t| quick_thread(&t.qa))
        .unwrap_or_default();
    if text.is_empty() {
        m.flash_err("nothing answered yet: alt+/ asks");
        return Out::Draw;
    }
    super::act(
        m,
        &key,
        super::Act::Paste(format!(
            "A quick question I asked on the side:\n\n{}\n",
            text.trim()
        )),
    );
    let q = quick(m);
    q.shown = false;
    if let Some(t) = q.thread() {
        t.focused = false;
        t.sel = Sel::default();
    }
    m.flash("the quick ask is in the box: add a line and send");
    Out::Draw
}

/// Keys while the float has them: all but quitting and the command bar,
/// and alt+/ anywhere (quickKey).
pub fn quick_key_hook(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    if s == "alt+/" && m.confirm.is_none() {
        return toggle_quick(m);
    }
    if !quick(m).focused() || m.confirm.is_some() || s == "ctrl+]" {
        return Out::Pass;
    }
    match s {
        "ctrl+q" | "ctrl+k" | "super+k" => Out::Pass,
        "super+c" | "ctrl+c" => {
            let t = quick(m).thread().expect("focused");
            if let Some(txt) = t.sel_text() {
                if !txt.is_empty() {
                    m.copy_text(txt);
                }
                return Out::Draw;
            }
            if s == "super+c" { Out::Idle } else { Out::Pass }
        }
        "esc" => {
            // Hidden, the thread stays for alt+/.
            let q = quick(m);
            q.shown = false;
            let t = q.thread().expect("focused");
            t.focused = false;
            t.sel = Sel::default();
            Out::Draw
        }
        "tab" => {
            // The keys to what's under it.
            let t = quick(m).thread().expect("focused");
            t.focused = false;
            t.sel = Sel::default();
            Out::Draw
        }
        "enter" => {
            let t = quick(m).thread().expect("focused");
            let text: String = t.input.iter().collect::<String>().trim().to_string();
            if text.is_empty() {
                return Out::Idle;
            }
            if t.waiting != 0 {
                m.flash_err("one question at a time: the last one's still being answered");
                return Out::Draw;
            }
            t.input.clear();
            t.pos = 0;
            ask_quick(m, &text);
            Out::Draw
        }
        "ctrl+n" => {
            quick(m).new_thread().focused = true;
            Out::Draw
        }
        "ctrl+p" => {
            // The thread before, round to the latest.
            let q = quick(m);
            let n = q.threads.len();
            if n > 1 {
                let t = q.thread().expect("focused");
                t.focused = false;
                t.sel = Sel::default();
                q.cur = (q.cur + n - 1) % n;
                q.thread().expect("a thread").focused = true;
            }
            Out::Draw
        }
        _ => {
            let t = quick(m).thread().expect("focused");
            if !t.scroll_key(s)
                && let Some((b, p)) = edit(&t.input, t.pos, k)
            {
                t.input = b;
                t.pos = p;
            }
            // Nothing reaches the box under it.
            Out::Draw
        }
    }
}

/// A click on the float gives it the keys and starts a drag over its
/// text; a click elsewhere hands the keys back (clickQuick); a drag and
/// its release as btw's.
pub fn quick_mouse(m: &mut Model, e: &Mouse) -> Out {
    let copies = super::btw::copies_on_select(m);
    let q = quick(m);
    if !q.shown {
        return Out::Pass;
    }
    let (x, y) = (e.x as usize, e.y as usize);
    let Some(t) = q.thread().filter(|t| t.at[2] > 0) else {
        return Out::Pass;
    };
    match e.kind {
        MouseKind::Down(Button::Left) => {
            if x < t.at[0] || x >= t.at[0] + t.at[2] || y + 1 < t.at[1] || y > t.at[1] + t.at[3] {
                t.focused = false;
                t.sel = Sel::default();
                return Out::Pass;
            }
            t.focused = true;
            let at = t.cell_at(x, y);
            t.sel = Sel::at(at);
            Out::Draw
        }
        MouseKind::Drag(Button::Left) if t.sel.drag => {
            let at = t.cell_at(x, y);
            t.sel.drag_to(at);
            Out::Draw
        }
        MouseKind::Up(Button::Left) if t.sel.drag => {
            t.sel.drag = false;
            if !t.sel.moved {
                t.sel = Sel::default();
                return Out::Draw;
            }
            if copies && let Some(txt) = t.sel_text().filter(|s| !s.is_empty()) {
                m.copy_text(txt);
            }
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// Draws the float over the screen's top right, while it's shown
/// (quickOver).
// The frame's Overlay hook hands the rows as a Vec.
#[allow(clippy::ptr_arg)]
pub fn quick_over(m: &mut Model, screen: &mut Vec<Line>) {
    let (w, h, now) = (m.w, m.h, super::now(m));
    let in_chat = super::open_key(m).is_some();
    let q = quick(m);
    let model = q.model.clone();
    let shown = q.shown;
    let Some(t) = q.thread() else { return };
    t.at = [0; 4];
    if !shown || w < 30 || h < 8 {
        return;
    }
    let pw = (w - 2).min(40.max(w * 2 / 5));
    // Inside the edges and a space either side.
    let iw = pw.saturating_sub(4);
    let mut thread: Vec<Line> = Vec::new();
    for (i, x) in t.qa.iter().enumerate() {
        if i > 0 {
            thread.push(Line::new());
        }
        for (j, l) in wrap(&x.question, iw.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            let lead = if j == 0 {
                sp("› ", ORANGE)
            } else {
                plain("  ")
            };
            thread.push(line([lead, spb(l, TEXT)]));
        }
        let last = i == t.qa.len() - 1;
        if !x.response.is_empty() {
            thread.extend(rush_convo::render::markdown_lines(&x.response, iw));
        } else if last && t.waiting != 0 {
            thread.push(line([sp(
                format!("thinking · {}", dur(now.saturating_sub(t.waiting))),
                DIM,
            )]));
        } else if last && !t.err.is_empty() {
            for l in wrap(&t.err, iw) {
                thread.push(line([sp(l, RED)]));
            }
        }
    }
    if t.qa.is_empty() {
        thread.push(line([sp(
            "ask the guide: what's next? which one needs me? where's the login work?",
            DIM,
        )]));
        thread.push(line([sp(
            "it sees your sessions, opens the one it means, and answers anything quick",
            DIM,
        )]));
    }
    let mut foot = Vec::new();
    if t.focused {
        foot.push(Line::new());
        let mut f = line([sp("› ", ORANGE)]);
        join(
            &mut f,
            text_field(&t.input, t.pos, true, "ask…", iw.saturating_sub(2)),
        );
        foot.push(f);
    }
    let room = 1.max((h / 2) as isize - 2 - foot.len() as isize) as usize;
    if thread.len() > room {
        // Anchored to its latest line; ↑ scrolls back.
        t.scroll = t.scroll.min(thread.len() - room);
        let end = thread.len() - t.scroll;
        thread = thread[end.saturating_sub(room)..end].to_vec();
    }
    t.shown = thread;
    t.shown.extend(foot);
    let edge = if t.focused { ORANGE } else { DIM };
    let mut title = line([spb("guide", ORANGE)]);
    if !model.is_empty() {
        title.spans.push(sp(format!(" · {model}"), DIM));
    }
    let hint = if t.focused && in_chat && !quick_thread(&t.qa).is_empty() {
        "ctrl+] i into a chat · esc"
    } else if t.focused {
        "enter asks · ctrl+n new · tab back · esc"
    } else {
        "alt+/ to type here · esc"
    };
    let edge_line = |l: &str, r: &str, mid: Line| -> Line {
        let room = pw.saturating_sub(rush_core::text::width(l) + 1);
        let mid = if mid.width() > room {
            fit(mid, room)
        } else {
            mid
        };
        let mw = mid.width();
        let mut out = line([sp(l, edge)]);
        join(&mut out, mid);
        out.spans.push(sp(
            format!("{}{r}", "─".repeat(room.saturating_sub(mw))),
            edge,
        ));
        out
    };
    let mut tl = title;
    tl.spans.push(plain(" "));
    let mut bx = vec![edge_line("╭─ ", "╮", tl)];
    for (i, l) in t.shown.iter().enumerate() {
        let mut inner = line([plain(" ")]);
        join(&mut inner, l.clone());
        let mut row = line([sp("│", edge)]);
        join(&mut row, on_ground(fit(inner, pw - 2), pw - 2, BTW_BG));
        row.spans.push(sp("│", edge));
        if let Some((from, to)) = t.sel.cols(i, iw) {
            // Past the edge and its space.
            row = paint_cols(&row, from + 2, to + 2);
        }
        bx.push(row);
    }
    bx.push(edge_line("╰ ", "╯", line([sp(hint, DIM), plain(" ")])));
    let (left, top) = (w - pw - 1, 1);
    t.at = [left, top + 1, pw, t.shown.len()];
    let waiting = t.waiting != 0;
    paste_at(screen, &bx, top, left);
    if waiting {
        m.ask_wake(Instant::now() + Duration::from_secs(1));
    }
}
