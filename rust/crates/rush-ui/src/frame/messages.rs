//! What rush says, each kind in its own place (Go's internal/ui/
//! messages.go, as of 381b0aec): an answer to a key you pressed (said,
//! or refused) on the line under the box in plain text, gone in a few
//! seconds (flash :55, flashFor :52); something that broke (failed) on
//! the same line in red until the next key (fail :63, keyClearsFailure
//! :103, statusShown :94); something rush did on its own (news :70,
//! noted :76) never on that line but as a count in the header (newsHead
//! :146) and under the feed at the foot of the list (newsDock :158, its
//! clicks clickNews :185), until seen (unseenNews :112, seeNews :123,
//! openNewsRef :136); never beside a conversation, since it isn't that
//! agent's (8f2c1c99). Everything said goes in the log, the last
//! 50 (logKept :47), which #log opens newest first (logSheet :200-268).

use rush_core::text::{Line, width};
use rush_core::time::Ms;

use super::places::PlaceId;
use super::sheet::{Sheet, sheet_row, sheet_title, window};
use super::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// What kind of thing rush said (sayKind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SayKind {
    Said,
    Refused,
    Failed,
    News,
}

/// One thing rush said, as the log keeps it (logLine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub at: Ms,
    pub kind: SayKind,
    pub text: String,
    /// Where a click on it goes (NEWS_REF_PROJECTS), for news.
    pub r#ref: String,
    pub seen: bool,
}

/// How many things the log keeps (logKept).
pub const LOG_KEPT: usize = 50;
/// How long an answer stays on the line under the box (flashFor).
pub const FLASH_FOR: Ms = 6_000;
/// A news item's ref that opens a project in Projects, then its folder.
pub const NEWS_REF_PROJECTS: &str = "projects:";
/// A row of the foot of the list's news: this, then its index or "all".
pub const NEWS_KEY_PREFIX: &str = "rushnews:";

impl Model {
    /// Says s on the line under the box for a few seconds: an answer to
    /// a key (flash(s, false)).
    pub fn flash(&mut self, s: &str) {
        self.say(SayKind::Said, s, "");
    }

    /// Says why a key couldn't do what was asked, with the way forward in
    /// its words (flash(s, true)): plain, as an answer is, not red.
    pub fn flash_err(&mut self, s: &str) {
        self.say(SayKind::Refused, s, "");
    }

    /// Says something broke: in red, until the next key (fail).
    pub fn fail(&mut self, s: &str) {
        self.say(SayKind::Failed, s, "");
    }

    /// Says something rush did on its own, r where a click on it goes
    /// (news). It doesn't take the line under the box: you may be typing
    /// there.
    pub fn news(&mut self, s: &str, r: &str) {
        self.say(SayKind::News, s, r);
    }

    /// Logs something rush did on its own that's expected, such as a
    /// queue going out: #log has it, the header and the foot of the list
    /// don't (noted).
    pub fn noted(&mut self, s: &str) {
        self.news(s, "");
        if let Some(l) = self.said.last_mut() {
            l.seen = true;
        }
    }

    /// A plugin's word: a failure when its tone is bad (flashTone).
    pub fn flash_tone(&mut self, s: &str, bad: bool) {
        if bad {
            self.fail(s);
        } else {
            self.flash(s);
        }
    }

    fn say(&mut self, k: SayKind, s: &str, r: &str) {
        let s = rush_convo::render::key_word(s);
        let at = rush_core::time::now();
        if k != SayKind::News {
            self.flash = Some((s.clone(), at, k == SayKind::Failed));
        }
        self.said.push(LogLine {
            at,
            kind: k,
            text: s,
            r#ref: r.to_string(),
            seen: k != SayKind::News,
        });
        if self.said.len() > LOG_KEPT {
            let n = self.said.len() - LOG_KEPT;
            self.said.drain(..n);
        }
    }

    /// Everything rush has said, oldest first, the last LOG_KEPT.
    pub fn said(&self) -> &[LogLine] {
        &self.said
    }

    /// Whether the line under the box shows what rush said rather than
    /// its keys: an answer for a few seconds, a failure until a key
    /// (statusShown).
    pub fn status_shown(&self) -> bool {
        match &self.flash {
            Some((s, at, failed)) => !s.is_empty() && (*failed || self.now - at < FLASH_FOR),
            None => false,
        }
    }

    /// Takes a failure off the line under the box: you've pressed a key
    /// since, so you've seen it. The log still has it (keyClearsFailure).
    pub fn key_clears_failure(&mut self) {
        if self.flash.as_ref().is_some_and(|f| f.2) {
            self.flash = None;
        }
    }

    /// The news you haven't seen yet, newest first, by index into said
    /// (unseenNews).
    pub fn unseen_news(&self) -> Vec<usize> {
        (0..self.said.len())
            .rev()
            .filter(|&i| self.said[i].kind == SayKind::News && !self.said[i].seen)
            .collect()
    }

    /// Marks all news seen (seeNews).
    pub fn see_news(&mut self) {
        for l in &mut self.said {
            l.seen = true;
        }
    }

    /// Goes where a news item points (openNewsRef): a project's folder
    /// opens Projects at it.
    pub fn open_news_ref(&mut self, r: &str) -> Out {
        if let Some(dir) = r.strip_prefix(NEWS_REF_PROJECTS) {
            let dir = dir.to_string();
            self.set_place(PlaceId::Projects);
            crate::screens::projects_machine::pick_in_projects(self, &dir);
            return Out::Draw;
        }
        Out::Idle
    }

    /// The header's count of news unseen, None with none (newsHead).
    pub fn news_head(&self) -> Option<Line> {
        let n = self.unseen_news().len();
        if n == 0 {
            return None;
        }
        Some(line([
            sp(format!("◇ {n} from rush"), SUB),
            sp(" · #log", DIM),
        ]))
    }

    /// What rush did on its own that you haven't seen, under the feed at
    /// the foot of the list, newest at the bottom like the feed, one row
    /// each, each with its key for a click to where it's about (newsDock).
    pub fn news_dock(&self, w: usize) -> Vec<(Line, String)> {
        let mut idx = self.unseen_news();
        if idx.is_empty() || w < 24 {
            return Vec::new();
        }
        const MOST: usize = 3;
        let title = " what rush did ";
        let all = format!("{NEWS_KEY_PREFIX}all");
        let mut out = vec![(
            line([
                sp("──", FAINT),
                spb(title, TEXT),
                sp("─".repeat(w.saturating_sub(3 + width(title))), FAINT),
            ]),
            all.clone(),
        )];
        if idx.len() > MOST {
            let more = sp(format!("and {} more · #log", idx.len() - MOST), FAINT);
            out.push((indent(fit(line([more]), w.saturating_sub(2))), all));
            idx.truncate(MOST);
        }
        for &i in idx.iter().rev() {
            let (what, why) = split_why(&self.said[i].text);
            let mut row = line([sp("◇ ", SUB), sp(what, TEXT)]);
            if !why.is_empty() {
                row.spans.push(sp(format!(" · {why}"), DIM));
            }
            out.push((
                indent(fit(row, w.saturating_sub(2))),
                format!("{NEWS_KEY_PREFIX}{i}"),
            ));
        }
        out
    }

    /// Takes a click on a row of news_dock (clickNews): its
    /// head opens #log, an item goes where it points and is seen.
    pub fn click_news(&mut self, key: &str) -> Out {
        let what = key.strip_prefix(NEWS_KEY_PREFIX).unwrap_or(key);
        if what == "all" {
            self.open_log();
            return Out::Draw;
        }
        let Some(i) = what.parse::<usize>().ok().filter(|&i| i < self.said.len()) else {
            return Out::Idle;
        };
        self.said[i].seen = true;
        let r = self.said[i].r#ref.clone();
        match self.open_news_ref(&r) {
            Out::Idle => Out::Draw,
            o => o,
        }
    }

    /// Opens #log: the last things rush said, newest first (openLog).
    pub fn open_log(&mut self) {
        self.open_sheet(Box::new(LogSheet::default()));
    }
}

/// What before the first ": ", and what after.
fn split_why(s: &str) -> (&str, &str) {
    s.split_once(": ").unwrap_or((s, ""))
}

fn indent(l: Line) -> Line {
    let mut r = line([plain(" ")]);
    join(&mut r, l);
    r
}

/// "15:04" at ms, here.
fn clock(ms: Ms) -> String {
    let h = crate::model::local_hour(ms);
    let m = (ms / 1000 + crate::model::local_zone(ms)).rem_euclid(3600) / 60;
    format!("{h:02}:{m:02}")
}

/// #log: the last things rush said, newest first: answers, failures and
/// what it did on its own. Enter on one with somewhere to go goes there
/// (logSheet).
#[derive(Default)]
pub struct LogSheet {
    cur: usize,
}

impl LogSheet {
    fn newest(m: &Model) -> Vec<&LogLine> {
        m.said.iter().rev().collect()
    }
}

impl Sheet for LogSheet {
    fn width(&self, m: &Model) -> usize {
        110.min(m.w.saturating_sub(4))
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let lines = Self::newest(m);
        let about = format!("the last {} things rush said", lines.len());
        let mut out = vec![sheet_title("Log", &about, w), Line::new()];
        if lines.is_empty() {
            out.push(line([sp("nothing yet", DIM)]));
        }
        self.cur = self.cur.min(lines.len().saturating_sub(1));
        let (from, to) = window(lines.len(), self.cur, h.saturating_sub(5).max(1));
        for (i, s) in lines[from..to].iter().enumerate() {
            let (mark, c) = match s.kind {
                SayKind::Failed => (sp("✗ ", RED), RED),
                SayKind::News => (sp("◇ ", SUB), TEXT),
                _ => (sp("· ", DIM), TEXT),
            };
            let text = if s.kind == SayKind::News && !s.seen {
                spb(s.text.clone(), c)
            } else {
                sp(s.text.clone(), c)
            };
            let mut l = line([sp(format!("{}  ", clock(s.at)), FAINT), mark, text]);
            if !s.r#ref.is_empty() {
                l.spans.push(sp("  ›", DIM));
            }
            out.push(sheet_row(l, from + i == self.cur, w));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[("↑↓", "choose"), ("enter", "go to it"), ("esc", "close")],
        ));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let n = m.said.len();
        match k.s.as_str() {
            "up" | "k" | "shift+tab" => self.cur = self.cur.saturating_sub(1),
            "down" | "j" | "tab" => self.cur = (self.cur + 1).min(n.saturating_sub(1)),
            "enter" => {
                let r = Self::newest(m)
                    .get(self.cur)
                    .map(|l| l.r#ref.clone())
                    .unwrap_or_default();
                if !r.is_empty() {
                    m.see_news();
                    m.open_news_ref(&r);
                    return Out::Close;
                }
            }
            "esc" | "ctrl+c" | "q" => {
                m.see_news();
                return Out::Close;
            }
            _ => {}
        }
        Out::Draw
    }
}

/// #log opens the log (keys.go:899).
pub(crate) fn log_command(m: &mut Model, _: &str) -> Out {
    m.open_log();
    Out::Draw
}
