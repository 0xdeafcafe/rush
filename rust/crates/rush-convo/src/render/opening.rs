//! The card the conversation opens on, saying what the session is: when
//! it started and who started it (you, a plugin, the session that started
//! it beside it, or another agent it works for as a subagent), what it
//! runs as and where, what rush added to its prompt, and the commands and
//! skills it came with. Its start hooks follow, as they always have.
//! Port of internal/convo/opening.go (opening :18, added :65, startedBy
//! :86, startedAt :106, agentLabel :118, homeDir :123, openingPrint :132).

use rush_core::time::{Ms, format_rfc3339};

use super::GUTTER;
use super::Options;
use super::draw::Drawer;
use super::style::*;
use super::txt::{Txt, blanks, wrap};
use crate::model::Session;
use rush_core::text::Color;

impl Drawer<'_> {
    /// The card above the first turn, when it's that turn (opening).
    pub(crate) fn opening(&mut self) {
        let s = self.s;
        if s.turns.first().is_none_or(|t| !std::ptr::eq(t, self.t)) {
            return;
        }
        let i = &s.info;
        if i.started_at == 0 && s.model.is_empty() {
            return; // nothing known of how it began: a transcript read cold
        }
        let pad = || Txt::raw(" ").and(blanks(GUTTER - 3));
        let mut head = paint(C_TEXT.bold(), "◆ session");
        if i.started_at != 0 {
            head.push(dim(&format!(
                " · started {}",
                started_at(i.started_at, self.o.now, self.o.zone)
            )));
        }
        head.push(dim(" · "));
        head.push(self.started_by());
        self.add("", Color::Default, pad().and(head), Txt::new());
        let model = if i.model.is_empty() {
            &s.model
        } else {
            &i.model
        };
        let cwd = if i.cwd.is_empty() { &s.cwd } else { &i.cwd };
        let runs: Vec<String> = [
            agent_label(&i.kind),
            pretty_model(model),
            i.effort.clone(),
            i.permission_mode.clone(),
            home_dir(cwd),
        ]
        .into_iter()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .collect();
        if !runs.is_empty() {
            let l = pad().and_raw("  ").and(dim(&runs.join(" · ")));
            self.add("", Color::Default, l, Txt::new());
        }
        if self.o.hosted {
            self.added();
        }
        let n = s.commands.len();
        if n > 0 {
            let r = format!("{}:opening:skills", self.r);
            let what = format!("{} and skills ", plural(n, "command"));
            if !self.o.open.get(&r).copied().unwrap_or(false) {
                let l = pad().and_raw("  ").and(dim(&what)).and(faint("▸ list"));
                self.add(&r, Color::Default, l, Txt::new());
            } else {
                let l = pad().and_raw("  ").and(dim(&what)).and(faint("▾ hide"));
                self.add(&r, Color::Default, l, Txt::new());
                let names: Vec<String> = s.commands.iter().map(|c| format!("/{c}")).collect();
                let w = 10.max(self.cw.saturating_sub(GUTTER + 3).min(self.o.prose_cap()));
                for row in wrap(&dim(&names.join("  ")), w) {
                    let l = pad().and_raw("    ").and(row);
                    self.add(&r, Color::Default, l, Txt::new());
                }
            }
        }
        self.add("", Color::Default, Txt::new(), Txt::new());
    }

    /// What was added to the agent's prompt: rush's own, and the brief
    /// the session was started with, opened to read it whole (added).
    fn added(&mut self) {
        let pad = || Txt::raw(" ").and(blanks(GUTTER - 3));
        let b = self.o.brief.clone();
        if b.is_empty() {
            let l = pad().and_raw("  ").and(dim("prompt: rush's own"));
            self.add("", Color::Default, l, Txt::new());
            return;
        }
        let r = format!("{}:opening:brief", self.r);
        let n = b.matches('\n').count() + 1;
        if !self.o.open.get(&r).copied().unwrap_or(false) {
            let l = pad()
                .and_raw("  ")
                .and(dim(&format!(
                    "prompt: rush's own, and its brief · {} ",
                    plural(n, "line")
                )))
                .and(faint("▸ show"));
            self.add(&r, Color::Default, l, Txt::new());
            return;
        }
        let l = pad()
            .and_raw("  ")
            .and(dim("prompt: rush's own, and its brief "))
            .and(faint("▾ hide"));
        self.add(&r, Color::Default, l, Txt::new());
        let w = 10.max(self.cw.saturating_sub(GUTTER + 3).min(self.o.prose_cap()));
        for row in wrap(&paint(C_SUB, &b), w) {
            let l = pad().and_raw("    ").and(row);
            self.add(&r, Color::Default, l, Txt::new());
        }
    }

    /// Who began the session (startedBy).
    fn started_by(&self) -> Txt {
        let i = &self.s.info;
        if !self.o.parent.is_empty() {
            return dim("by ")
                .and(paint(C_TEXT, &self.o.parent))
                .and(dim(", as its subagent"));
        }
        if !i.spawned_by.is_empty() {
            return dim("by another session, as its subagent");
        }
        if !self.o.started_from.is_empty() {
            return dim("by ").and(paint(C_TEXT, &self.o.started_from));
        }
        if !i.started_from.is_empty() {
            return dim("by another session");
        }
        if !i.started_by.is_empty() {
            return dim("by the ")
                .and(paint(C_TEXT, &i.started_by))
                .and(dim(" plugin"));
        }
        if !self.t.from.is_empty() {
            return dim("by ").and(paint(C_TEXT, &self.t.from));
        }
        dim("by you")
    }
}

/// When, as you'd say it: the time today, else the day too, in the zone
/// `zone` seconds east of UTC (startedAt).
pub(crate) fn started_at(t: Ms, now: Ms, zone: i64) -> String {
    let local = |ms: Ms| ms + zone * 1000;
    let day = |ms: Ms| local(ms).div_euclid(86_400_000);
    let clock = clock(t, zone);
    if now != 0 && day(t) == day(now) {
        return clock;
    }
    let date = format_rfc3339(local(t)); // 2026-10-28T…
    let (m, d) = (&date[5..7], &date[8..10]);
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let wd = DAYS[(day(t) + 4).rem_euclid(7) as usize]; // 1970-01-01 was a Thursday
    let mon = MONTHS[m.parse::<usize>().unwrap_or(1).clamp(1, 12) - 1];
    format!("{wd} {} {mon} {clock}", d.trim_start_matches('0'))
}

/// The harness a session runs, by name: empty is Claude Code's, as older
/// rushes wrote no kind for it (agentLabel).
fn agent_label(kind: &str) -> String {
    let k = rush_core::adapter::migrated(kind);
    rush_core::adapter::global().harness_label(k.as_str())
}

/// claude-opus-5-5[1m] as "opus 5.5 · 1M" (overview.go:150 PrettyModel).
pub(crate) fn pretty_model(m: &str) -> String {
    if m.is_empty() {
        return "?".into();
    }
    let long = m.contains("[1m]");
    let m = m.replace("[1m]", "");
    let mut m = m.strip_prefix("claude-").unwrap_or(&m).to_string();
    if let Some(i) = m.rfind('-')
        && m.len() - i == 9
        && m[i + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        m.truncate(i); // its date: -20250929
    }
    let parts: Vec<&str> = m.split('-').collect();
    let mut out = parts[0].to_string();
    if parts.len() > 1 {
        out.push(' ');
        out.push_str(&parts[1..].join("."));
    }
    if long {
        out.push_str(" · 1M");
    }
    out
}

/// p with your home as ~ (homeDir).
fn home_dir(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

/// What the card reads, for the first turn's key: when any of it
/// changes, the turn is drawn again (openingPrint).
pub(crate) fn opening_print(s: &Session, o: &Options) -> String {
    let i = &s.info;
    let today = (o.now + o.zone * 1000).div_euclid(86_400_000);
    format!(
        "{}|{}{}|{}|{}|{}|{}{}|{}{}{}|{}{}|{}{}|{}|{}",
        i.started_at / 1000,
        i.model,
        s.model,
        i.effort,
        i.permission_mode,
        i.kind,
        i.cwd,
        s.cwd,
        i.started_by,
        i.spawned_by,
        i.started_from,
        o.parent,
        o.started_from,
        o.hosted,
        o.brief.len(),
        s.commands.len(),
        today
    )
}
