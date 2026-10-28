//! A session opened read-only: its header, the conversation as rush-convo
//! draws it, and the keys that scroll it and fold its rows. After Go's
//! internal/ui/rushmode.go (paneHeader :2721, the conversation's keys
//! :3954-4120, isOpen :4661) and panegeometry.go:13 (paneFrameRow).
//! The transcript is read and parsed on a worker (worker::spawn_tail);
//! this side only applies what it sends.

use std::collections::HashMap;
use std::path::PathBuf;

use rush_claude::fleet::Predicates;
use rush_convo::{Fresh, Options, Renderer, Session, Tail};
use rush_core::agent::Agent;
use rush_core::text::{Line, Span, Style, width};
use rush_core::time::Ms;

use crate::style::*;
use crate::worker::TailHandle;

/// The pane's chrome: a blank row, the name, where it works, its tabs,
/// and a rule.
pub const HEAD_H: usize = 5;

pub struct SessionView {
    pub key: String,
    pub path: PathBuf,
    pub sess: Session,
    pub r: Renderer,
    pub open: HashMap<String, bool>,
    /// The row picked, by ref; "" for none.
    pub sel: String,
    /// Rows scrolled up from the end; 0 follows the conversation.
    pub from_bottom: usize,
    pub verbose: bool,
    pub r#gen: u64,
    pub tail: Option<TailHandle>,
    pub err: String,
    pub read: bool,
    /// The ref on each screen row last drawn, and where they start.
    pub shown: Vec<String>,
    pub top: usize,
}

impl SessionView {
    pub fn new(key: String, path: PathBuf, r#gen: u64) -> SessionView {
        SessionView {
            key,
            path,
            sess: Session::new(),
            r: Renderer::new(),
            open: HashMap::new(),
            sel: String::new(),
            from_bottom: 0,
            verbose: false,
            r#gen,
            tail: None,
            err: String::new(),
            read: false,
            shown: Vec::new(),
            top: 0,
        }
    }

    /// Reads it here and now, for a one-off frame (rush --render).
    pub fn read_now(&mut self) {
        let mut t = Tail::from_end(&self.path, 2 << 20, rush_claude::messages);
        if let Err(e) = t.read(&mut self.sess) {
            self.err = e.to_string();
        }
        self.read = true;
    }

    /// Takes what the worker read.
    pub fn take(&mut self, f: Fresh) {
        self.read = true;
        f.apply(&mut self.sess);
    }

    pub fn options(&self, w: usize, now: Ms, tick: usize) -> Options {
        Options {
            width: w,
            now,
            tick,
            open: self.open.clone(),
            verbose: self.verbose,
            selected: self.sel.clone(),
            focused: true,
            zone: crate::model::local_zone(now),
            ..Options::default()
        }
    }

    /// The conversation's rows in view, `rows` tall; notes their refs.
    pub fn body(&mut self, o: &Options, rows: usize) -> Vec<Line> {
        let mut got = self
            .r
            .render_tail(&self.sess, o, self.from_bottom, rows)
            .len();
        // Scrolled past the top: as far up as there is.
        if got < rows && self.from_bottom > 0 {
            self.from_bottom = self.from_bottom.saturating_sub(rows - got);
            got = rows;
        }
        let _ = got;
        let lines = self.r.render_tail(&self.sess, o, self.from_bottom, rows);
        self.shown = lines.iter().map(|l| l.r#ref.clone()).collect();
        lines.iter().map(|l| l.line.clone()).collect()
    }

    /// Picks the next row up (d < 0) or down with a ref of its own, and
    /// scrolls it into view.
    pub fn move_sel(&mut self, d: isize, o: &Options, rows: usize) {
        let lo = self.from_bottom.saturating_sub(rows);
        let refs: Vec<String> = self
            .r
            .render_tail(&self.sess, o, lo, rows * 3)
            .iter()
            .map(|l| l.r#ref.clone())
            .collect();
        let n = refs.len();
        if n == 0 {
            return;
        }
        // The view's rows within the window.
        let end = n.saturating_sub(self.from_bottom - lo);
        let start = end.saturating_sub(rows);
        let at = refs[start..end]
            .iter()
            .position(|r| !self.sel.is_empty() && *r == self.sel)
            .map(|i| i + start);
        let mut j: Option<usize> = None;
        match (at, d < 0) {
            (None, true) => j = (start..end).rev().find(|&i| !refs[i].is_empty()),
            (None, false) => j = (start..end).find(|&i| !refs[i].is_empty()),
            (Some(i), true) => {
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    if !refs[k].is_empty() && refs[k] != self.sel {
                        // The top row of that ref.
                        let r = &refs[k];
                        while k > 0 && refs[k - 1] == *r {
                            k -= 1;
                        }
                        j = Some(k);
                        break;
                    }
                }
            }
            (Some(i), false) => {
                j = (i + 1..n).find(|&k| !refs[k].is_empty() && refs[k] != self.sel);
            }
        }
        let Some(j) = j else {
            // Nothing more that way: scroll instead.
            self.scroll(if d < 0 {
                rows as isize / 2
            } else {
                -(rows as isize / 2)
            });
            return;
        };
        self.sel = refs[j].clone();
        if j < start {
            self.from_bottom += start - j;
        } else if j >= end {
            self.from_bottom = self.from_bottom.saturating_sub(j - end + 1);
        }
    }

    /// Up by d rows (down when negative).
    pub fn scroll(&mut self, d: isize) {
        self.from_bottom = (self.from_bottom as isize + d).max(0) as usize;
    }

    /// Whether the row picked shows open: folded rows show ▸.
    fn is_open(&mut self, r: &str, o: &Options) -> bool {
        if let Some(&v) = self.open.get(r) {
            return v;
        }
        let mut seen = false;
        // The rows near the view say it; a ref out of view is closed.
        let lo = self.from_bottom.saturating_sub(64);
        let lines = self.r.render_tail(&self.sess, o, lo, 256);
        for l in lines {
            if l.r#ref == r {
                seen = true;
                if l.line.plain().contains('▸') {
                    return false;
                }
            } else if seen {
                break;
            }
        }
        seen
    }

    /// Opens or closes the row picked.
    pub fn toggle(&mut self, o: &Options) {
        if self.sel.is_empty() {
            return;
        }
        let v = !self.is_open(&self.sel.clone(), o);
        self.open.insert(self.sel.clone(), v);
    }

    /// The ref on screen row y, if any.
    pub fn ref_at(&self, y: usize) -> Option<&str> {
        let i = y.checked_sub(self.top)?;
        self.shown
            .get(i)
            .map(String::as_str)
            .filter(|r| !r.is_empty())
    }
}

/// The pane's chrome, w wide: blank, name and state, where it works and
/// what it is, the tab, a rule.
pub fn header(a: Option<&Agent>, v: &SessionView, w: usize, now: Ms) -> Vec<Line> {
    let iw = w.saturating_sub(3);
    let chrome = |l: Line| {
        let mut out = line([Span::new("  ", Style::default().on(CHROME_BG))]);
        join(&mut out, on_ground(l, iw, CHROME_BG));
        out.spans
            .push(Span::new(" ", Style::default().on(CHROME_BG)));
        out
    };
    let Some(a) = a else {
        return vec![
            chrome(Line::new()),
            chrome(line([sp("this session has gone", DIM)])),
            chrome(Line::new()),
            chrome(Line::new()),
            line([sp("─".repeat(w), FAINT)]),
        ];
    };
    let state = if a.needs_you() {
        spb("needs you", YELLOW)
    } else if a.halted() {
        spb("stopped mid-turn", RED)
    } else if a.job.live() {
        let mut s = spb("working", ORANGE);
        if let Some(t) = v.sess.live() {
            s.text.push_str(&format!(" · {}", dur(now - t.start)));
        }
        s
    } else if a.pid != 0 {
        sp("idle", DIM)
    } else {
        sp("stopped", DIM)
    };
    let mut name = one_line(&a.display_name);
    let room = 12.max(56.min(iw.saturating_sub(2 + width(&state.text) + 3) / 2));
    if width(&name) > room {
        name = rush_draw::truncate(&name, room, "…").into_owned();
    }
    let row1 = line([
        spb(name, BRIGHT),
        plain("   "),
        sp("·", DIM),
        plain("  "),
        state,
    ]);
    let mut meta = Vec::new();
    let cwd = if a.job.cwd.is_empty() {
        &a.repo
    } else {
        &a.job.cwd
    };
    if !cwd.is_empty() {
        meta.push(tildify(cwd));
    }
    if !a.branch.is_empty() {
        meta.push(a.branch.clone());
    }
    let model = if v.sess.model.is_empty() {
        &a.spend.model
    } else {
        &v.sess.model
    };
    if !model.is_empty() {
        meta.push(model.trim_start_matches("claude-").to_string());
    }
    let mut row2 = Line::new();
    for (i, m) in meta.into_iter().enumerate() {
        if i > 0 {
            row2.spans.push(sp(" · ", FAINT));
        }
        row2.spans.push(sp(m, DIM));
    }
    let conn = if a.past {
        "past conversation".to_string()
    } else if a.headless {
        // Agent.Where (fleet.go:1551): every agent here is Claude Code's.
        "Claude Code · run by another program (claude -p)".to_string()
    } else if a.interactive {
        "Claude Code · open in a terminal".to_string()
    } else {
        "Claude Code".to_string()
    };
    let row2 = crate::header::spread(row2, line([sp(conn, DIM), plain(" ")]), iw);
    // tabLabel (rushmode.go:1418): the view's name, capitalised.
    let tab = Span::new(" Conversation ", Style::fg(BRIGHT).bold().on(TAB_ON_BG));
    let tab_w = tab.width();
    let mut chips = Line::new();
    if v.verbose {
        chips.spans.push(sp("m3 all shown", ORANGE));
        chips.spans.push(plain("  "));
    }
    chips.spans.push(sp("esc", TEXT));
    chips.spans.push(sp(" back to the list", DIM));
    chips.spans.push(plain(" "));
    let row3 = crate::header::spread(line([tab]), chips, iw);
    let rule = line([
        sp("──", FAINT),
        sp("─", FAINT),
        sp("━".repeat(tab_w.min(iw)), SUB),
        sp("─".repeat(w.saturating_sub(3 + tab_w)), FAINT),
    ]);
    vec![
        chrome(Line::new()),
        chrome(row1),
        chrome(row2),
        chrome(row3),
        fit(rule, w),
    ]
}
