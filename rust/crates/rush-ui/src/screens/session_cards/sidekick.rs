//! Recall: a cheap model follows the open chat and notes what was
//! decided, what goes against the grain and what's open, at the foot of
//! the list. Off until turned on; notes live in memory only, one set per
//! agent. After Go's internal/ui/sidekick.go (sidekick :17, sideNotes
//! :30, sideSystem :46, parseSide :62, sideInput :97, sideTick :111,
//! toggleRecall :163, sideColumn :170, clickSide :234).
//!
//! Gap: Go draws the column at the foot of the list beside an open chat;
//! the Rust frame shows the list or a chat, not both yet, so
//! [`side_column`] is ready for that split and isn't drawn until it's
//! there.

use std::collections::HashMap;

use rush_convo::Session;
use rush_core::text::Line;
use rush_core::time::Ms;

use super::quickask::ask_cheap;
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// Rows it takes in the list, at most (recallMax).
pub const RECALL_MAX: usize = 22;
/// Exchanges it reads (sideTurns).
pub const SIDE_TURNS: usize = 12;
/// Items a section keeps (sideKeep).
pub const SIDE_KEEP: usize = 5;
/// How often it reads again, at most (sideEvery).
pub const SIDE_EVERY: Ms = 60_000;

/// What the model is told (sideSystem).
pub const SIDE_SYSTEM: &str = "You follow along with a developer's conversation with a coding agent and keep short architecture notes. Reply only with lines in this format, nothing else:\nD: <a decision made in the chat>\nG: <something going against the codebase's patterns or earlier decisions>\nQ: <an open question>\nWrite D!:, G!: or Q!: for the few that matter most right now (a decision that changes the design, a real risk, a question blocking work). At most 5 of each, each under 70 characters. Keep earlier notes that still hold, drop the ones that don't. Architecture-level only: no chit-chat.";

/// One agent's notes (sideNotes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    pub model: String,
    /// The last read.
    pub at: Ms,
    /// What that read was given.
    pub input: String,
    pub decided: Vec<String>,
    pub grain: Vec<String>,
    pub open: Vec<String>,
    pub err: String,
}

/// Recall's state (sidekick).
#[derive(Default)]
pub struct Sidekick {
    pub on: bool,
    pub notes: HashMap<String, Notes>,
    /// The agent key being read, "" when none.
    pub busy: String,
    /// Where it was drawn last: column, row, width, the item on each row.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub items: Vec<String>,
    /// The agent key it was drawn for, "" when hidden.
    pub shown_on: String,
}

/// The model's D:/G:/Q: lines; anything else is dropped; a ! marks the
/// few that matter, kept as a leading ! (parseSide).
pub fn parse_side(out: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut d, mut g, mut q) = (Vec::new(), Vec::new(), Vec::new());
    for l in out.split('\n') {
        let mut l = l
            .trim()
            .trim_start_matches(['-', '*', '•', ' '])
            .to_string();
        let mut mark = "";
        if l.len() > 2 && l.as_bytes()[1] == b'!' {
            mark = "!";
            l = format!("{}{}", &l[..1], &l[2..]);
        }
        if l.len() < 2 || l.as_bytes()[1] != b':' {
            continue;
        }
        let text = l[2..].trim();
        if text.is_empty() {
            continue;
        }
        let to = match l.as_bytes()[0] {
            b'D' | b'd' => &mut d,
            b'G' | b'g' => &mut g,
            b'Q' | b'q' => &mut q,
            _ => continue,
        };
        if to.len() < SIDE_KEEP {
            to.push(format!("{mark}{text}"));
        }
    }
    (d, g, q)
}

/// The latest exchanges of a chat, as the model reads them (sideInput).
pub fn side_input(s: &Session) -> String {
    let from = s.turns.len().saturating_sub(SIDE_TURNS);
    super::discuss::chat_turns_of(&s.turns[from..], s, false).join("\n")
}

/// session.recall: on and off (toggleRecall).
pub fn toggle_recall(m: &mut Model) -> Out {
    let on = {
        let s = m.kept::<Sidekick>();
        s.on = !s.on;
        s.on
    };
    m.flash(if on {
        "recall on: notes on this chat at the foot of the list"
    } else {
        "recall off"
    });
    Out::Draw
}

/// Reads agent key's chat again when it changed, at most once a minute,
/// and only while the column shows (sideTick).
pub fn side_tick(m: &mut Model, key: &str) {
    let now = super::now(m);
    let Some(sess) = super::session(m, key) else {
        return;
    };
    let input = side_input(sess);
    let s = m.kept::<Sidekick>();
    if s.shown_on != key || !s.busy.is_empty() {
        return;
    }
    let old = s.notes.get(key).cloned();
    if old
        .as_ref()
        .is_some_and(|n| now.saturating_sub(n.at) < SIDE_EVERY)
    {
        return;
    }
    if input.trim().is_empty() || old.as_ref().is_some_and(|n| n.input == input) {
        return;
    }
    let mut prompt = format!("The latest exchanges:\n\n{input}");
    if let Some(n) = &old {
        let mut b = String::new();
        for (k, items) in [("D", &n.decided), ("G", &n.grain), ("Q", &n.open)] {
            for it in items {
                match it.strip_prefix('!') {
                    Some(t) => b.push_str(&format!("{k}!: {t}\n")),
                    None => b.push_str(&format!("{k}: {it}\n")),
                }
            }
        }
        if !b.is_empty() {
            prompt = format!("Your notes so far:\n{b}\n{prompt}");
        }
    }
    s.busy = key.to_string();
    let reg = m.reg.clone();
    let k = key.to_string();
    m.later(
        move || ask_cheap(&reg, SIDE_SYSTEM, &prompt, "sonnet"),
        move |m, r| {
            let s = m.kept::<Sidekick>();
            s.busy.clear();
            let mut nn = s.notes.get(&k).cloned().unwrap_or_default();
            nn.at = now;
            nn.input = input;
            nn.err.clear();
            match r {
                Err(e) => nn.err = e,
                Ok((model, out)) => {
                    if !model.is_empty() {
                        nn.model = model;
                    }
                    (nn.decided, nn.grain, nn.open) = parse_side(&out);
                }
            }
            s.notes.insert(k, nn);
            Out::Draw
        },
    );
}

/// Recall for agent key, w wide and h tall, at screen (x, y)
/// (sideColumn).
pub fn side_column(m: &mut Model, key: &str, w: usize, h: usize, x: usize, y: usize) -> Vec<Line> {
    let now = super::now(m);
    let s = m.kept::<Sidekick>();
    s.x = x;
    s.y = y;
    s.w = w;
    s.shown_on = key.to_string();
    s.items.clear();
    let mut out: Vec<Line> = Vec::new();
    let add = |s: &mut Sidekick, l: Line, item: &str| {
        let mut r = line([plain(" ")]);
        join(&mut r, fit(l, w.saturating_sub(1)));
        s.items.push(item.to_string());
        r
    };
    let n = s.notes.get(key).cloned();
    let mut head = line([spb("◇ recall", ORANGE)]);
    if let Some(n) = &n {
        if !n.model.is_empty() {
            head.spans.push(sp(format!(" · {}", n.model), DIM));
        }
        head.spans
            .push(sp(format!(" · {}", dur(now.saturating_sub(n.at))), DIM));
    }
    out.push(add(s, head, ""));
    let empty = n
        .as_ref()
        .is_none_or(|n| n.decided.len() + n.grain.len() + n.open.len() == 0);
    if s.busy == key && empty {
        out.push(add(s, line([sp("reading the chat…", DIM)]), ""));
    } else if n.is_none() {
        out.push(add(
            s,
            line([sp("follows the chat once it says something", DIM)]),
            "",
        ));
    } else if let Some(n) = n.as_ref().filter(|n| !n.err.is_empty()) {
        out.push(add(s, line([sp(n.err.clone(), RED)]), ""));
    }
    if let Some(n) = &n {
        let kinds = [
            ("✓ ", GREEN, &n.decided),
            ("⚠ ", ORANGE, &n.grain),
            ("? ", DIM, &n.open),
        ];
        for important in [true, false] {
            let title = if important {
                spb("IMPORTANT", ORANGE)
            } else {
                spb("RECAP", DIM)
            };
            let mut titled = false;
            for (glyph, c, items) in kinds {
                for it in items.iter() {
                    let (text, mark) = match it.strip_prefix('!') {
                        Some(t) => (t, true),
                        None => (it.as_str(), false),
                    };
                    if mark != important {
                        continue;
                    }
                    if !titled {
                        out.push(add(s, Line::new(), ""));
                        out.push(add(s, line([title.clone()]), ""));
                        titled = true;
                    }
                    if important {
                        out.push(add(s, line([sp(glyph, c), sp(text, TEXT)]), text));
                    } else {
                        out.push(add(s, line([sp(glyph, FAINT), sp(text, DIM)]), text));
                    }
                }
            }
        }
    }
    while out.len() < h {
        out.push(add(s, Line::new(), ""));
    }
    out.truncate(h);
    out
}

/// A click on an item puts it into agent key's box, to say something
/// about it (clickSide).
pub fn click_side(m: &mut Model, key: &str, x: usize, y: usize) -> bool {
    let s = m.kept::<Sidekick>();
    if s.shown_on.is_empty() || s.shown_on != key || x < s.x || x >= s.x + s.w {
        return false;
    }
    let Some(item) = y
        .checked_sub(s.y)
        .and_then(|r| s.items.get(r))
        .filter(|i| !i.is_empty())
        .cloned()
    else {
        return false;
    };
    let input = super::info(m, key).map(|i| i.input).unwrap_or_default();
    super::set_input(m, key, &format!("{input}About: {item} "));
    true
}
