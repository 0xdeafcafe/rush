//! The agent header: what's right of an agent's name at the top of its
//! Session and on the row under it, built in /statusline, with what it
//! runs as and how it's connected; the permission mode in it a click to
//! change. After Go's internal/ui/bars.go:169 (agentSegs),
//! header_context.go (headerContext :11, modeAt :39, segAt :48; Go
//! 42172936), usagebar.go (agentLabel :117, lowNote :147, lowNoteNow
//! :163), rushmode.go:2854 paneHeader's rows one and two (the low note,
//! the bar's two lines, the connection, the label) and model.go:1677 (a
//! click on the mode opens what changes it). Its rows one and two are
//! drawn here whole: the name, its @handle (mention.go:47 nameHandle) and
//! its state in Go's words (paused, retrying, offline, API error, cache
//! expired, compacting; rushmode.go:2858), the cache while a limit or an
//! error holds it, headerGit's commits and pull request (rushmode.go:1517),
//! the login the session runs on (logins.go:752 sessionLogin), billing and
//! an older rush from its host's info (bars.go:188, :210; stale.go:41),
//! and the agent label lit under the pointer, a click on it opening the
//! switch sheet (setup.go:570 clickLabel, startsheet.go:204, setup.go:301
//! sessionStart). The tabs and the rule are the Session's (session.rs,
//! session_core).

use rush_convo::Session;
use rush_core::agent::Agent;
use rush_core::text::{Line, width};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use super::line::{Ctx, Seg, bar_line};
use super::top::{in_use_rows, meter_of, now};
use super::{BAR_AGENT, State, layout};
use crate::frame::Out;
use crate::frame::location::{branch_words, folder, session_location};
use crate::header::Drops;
use crate::model::Model;
use crate::screens::accounts::info::pretty_model;
use crate::screens::accounts::look::{LOGINS_KIND, glyph, harness_name, model_word, show_profile};
use crate::screens::accounts::rows::{account_of, account_rows, switches};
use crate::screens::accounts::setup::StartOver;
use crate::screens::accounts::state::{cfg, peek};
use crate::screens::list_extras::meters::{MeterRow, switch_note};
use crate::screens::projects_machine::{TEMP_SHOWN, temp_of};
use crate::screens::session_core::{self as core, Core};
use crate::screens::social::parts::disk;
use crate::style::*;
use rush_draw::{Button, MouseKind};

/// The Session's header rows: the name's, and the one under it.
const NAME_ROW: usize = 1;
const META_ROW: usize = 2;

/// What an agent's header is drawn from: the agent, its conversation
/// when it's the one open, and its host's word on it (barCtx's a and c).
pub struct Of<'a> {
    pub a: &'a Agent,
    pub key: String,
    pub sess: Option<&'a Session>,
    /// rush hosts it and it's connected.
    pub hosted: bool,
    /// Its permission mode, in the agent's words.
    pub mode: String,
    /// Messages waiting for the turn to end.
    pub queued: usize,
    /// Lines added and removed, and files changed, in its project.
    pub stat: (i64, i64),
    pub files: usize,
    /// The pointer is over its mode, or its context.
    pub hover_mode: bool,
    pub hover_context: bool,
    /// Its host's word on it, when rush hosts it and it's connected.
    pub info: Option<&'a core::host::Info>,
    /// Its host sleeps, the session kept.
    pub sleeping: bool,
    /// Where its host last said it is, for a stopped rush session whose
    /// host's info was read from disk (openSavedHost's c.sess.Info.Cwd).
    pub saved_cwd: &'a str,
    /// The pointer is over its agent label.
    pub hover_label: bool,
}

impl<'a> Of<'a> {
    /// Agent key's header, as known now; None when it's gone.
    pub fn read(m: &'a Model, key: &str) -> Option<Of<'a>> {
        let a = m.agents.iter().find(|a| a.key == key)?;
        let sess = m.session.as_ref().filter(|v| v.key == key).map(|v| &v.sess);
        let c = m.kept.peek::<Core>().and_then(|c| c.conns.get(key));
        // Only the project's files: not its memory or plans (bars.go:257,268).
        let (stat, files) = sess.map_or(((0, 0), 0), |s| core::changes::project_count_of(m, s));
        Some(Of {
            a,
            key: key.to_string(),
            sess,
            hosted: c.is_some_and(|c| c.hosted()),
            mode: c
                .map(|c| c.info.permission_mode.clone())
                .unwrap_or_default(),
            queued: c.map_or(0, |c| c.info.queue.len()),
            stat,
            files,
            hover_mode: m
                .kept
                .peek::<State>()
                .is_some_and(|s| s.hover_mode == key && !key.is_empty()),
            hover_context: c.is_some_and(|c| c.vs.hover_context),
            info: c.filter(|c| c.hosted()).map(|c| &c.info),
            sleeping: c.is_some_and(|c| c.sleeping || c.info.sleeping),
            saved_cwd: c
                .filter(|c| !c.hosted())
                .map_or("", |c| c.info.began.cwd.as_str()),
            hover_label: m
                .kept
                .peek::<State>()
                .is_some_and(|s| s.hover_label == key && !key.is_empty()),
        })
    }

    fn model(&self) -> &str {
        let s = self.sess.map_or("", |s| {
            if s.model.is_empty() {
                &s.info.model
            } else {
                &s.model
            }
        });
        if s.is_empty() { &self.a.spend.model } else { s }
    }

    /// Where its host says it is: only a hosted session's word counts
    /// over the list's, connected or read from its info on disk
    /// (sessionLocation's c.sess.Info.Cwd, location.go:16).
    fn live_cwd(&self) -> Option<&str> {
        if !self.hosted && !self.saved_cwd.is_empty() {
            return Some(self.saved_cwd);
        }
        self.sess
            .filter(|_| self.hosted)
            .map(|s| s.info.cwd.as_str())
    }
}

fn of<'a>(x: &'a Ctx) -> &'a Of<'a> {
    x.a.expect("the agent header is drawn of an agent")
}

fn dim(s: impl Into<String>) -> rush_core::text::Span {
    sp(s, DIM)
}

fn one(s: rush_core::text::Span) -> Line {
    line([s])
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Whether what's counted needs the whole conversation, still being read.
fn counting(x: &Ctx) -> Option<Line> {
    of(x)
        .sess
        .filter(|s| s.partial)
        .map(|_| one(dim("counting…")))
}

/// The agent header's segments (agentSegs).
pub static SEGS: &[Seg] = &[
    Seg {
        id: "context",
        name: "Context",
        about: "context fullness; click the header readout for token counts and breakdown",
        draw: |x| {
            let o = of(x);
            if o.sess.is_none() {
                return Line::new();
            }
            core::views::header_context(x.m, o.hover_context).unwrap_or_default()
        },
    },
    Seg {
        id: "context-detail",
        name: "Context details",
        about: "tokens used, context capacity and fullness gauge",
        draw: |x| {
            let o = of(x);
            let Some(s) = o.sess.filter(|s| s.context > 0) else {
                return Line::new();
            };
            let win = if s.window > 0 {
                s.window
            } else {
                core::overview::agentry(x.m, &o.key).window(o.model())
            };
            let f = rush_core::adapter::fill_of(s.context, win, Default::default());
            let mut l = line([dim("ctx ")]);
            join(&mut l, crate::screens::list_extras::wall::ctx_bar(f.pct()));
            l.spans.push(plain(" "));
            l.spans.push(sp(
                format!("{}/{} {:.0}%", tokens(f.used), tokens(f.window), f.pct()),
                SUB,
            ));
            l
        },
    },
    Seg {
        id: "cost",
        name: "Cost",
        about: "what the agent has cost so far",
        draw: |x| {
            let c = of(x).sess.map_or(0.0, |s| s.info.cost);
            if c > 0.0 {
                return one(spb(money(c), TEXT));
            }
            Line::new()
        },
    },
    Seg {
        id: "billing",
        name: "Billing",
        about: "how it's paid for, when not out of a plan: extra usage in yellow, an API key in orange",
        draw: |x| match of(x).info.map_or("", |i| i.billing.as_str()) {
            "overage" => one(sp("extra usage", YELLOW)),
            "metered" => one(sp("API billed", ORANGE)),
            _ => Line::new(),
        },
    },
    Seg {
        id: "folder",
        name: "Folder",
        about: "the folder it works in",
        draw: |x| {
            let o = of(x);
            one(dim(folder(o.a, &o.a.root, o.live_cwd(), x.room)))
        },
    },
    Seg {
        id: "branch",
        name: "Git branch",
        about: "the branch checked out there",
        draw: |x| {
            let o = of(x);
            let l = session_location(o.a, &o.a.root, o.live_cwd());
            // A worktree named for its branch is only its branch here.
            let b = branch_words(l.repo, l.root, l.branch);
            let b = b.strip_suffix(" (worktree)").unwrap_or(&b);
            one(sp(rush_draw::truncate(b, x.room, "…").into_owned(), SUB))
        },
    },
    Seg {
        id: "model",
        name: "Model",
        about: "the model it runs",
        draw: |x| {
            let md = of(x).model();
            if md.is_empty() {
                return Line::new();
            }
            one(dim(pretty_model(md)))
        },
    },
    Seg {
        id: "rush",
        name: "Older rush",
        about: "shown when its host runs an older rush than the one installed",
        draw: |x| {
            let installed = x.m.kept.peek::<State>().map_or(0, |s| s.installed);
            if of(x).info.is_some_and(|i| i.stale(installed)) {
                return one(sp("older rush", YELLOW));
            }
            Line::new()
        },
    },
    Seg {
        id: "effort",
        name: "Effort",
        about: "the effort level",
        draw: |x| one(dim(of(x).sess.map_or("", |s| s.effort()).to_string())),
    },
    Seg {
        id: "mode",
        name: "Permissions",
        about: "the permission mode: plan in blue, the ones that don't ask in orange; click it to change it",
        draw: |x| {
            let o = of(x);
            let mode = &o.mode;
            if mode.is_empty() {
                return Line::new();
            }
            let c = if mode == "plan" {
                BLUE
            } else if rush_core::adapter::trust(mode) > 0 {
                ORANGE
            } else {
                SUB
            };
            let out = one(sp(mode.clone(), c));
            if o.hover_mode {
                return hover_line(out, width(mode));
            }
            out
        },
    },
    Seg {
        id: "tmp",
        name: "Temp files",
        about: "its scratch left on disk, when there's much",
        draw: |x| {
            let o = of(x);
            let t = temp_of(x.m, &o.key);
            if t < TEMP_SHOWN {
                return Line::new();
            }
            let c = if t >= 1 << 30 { YELLOW } else { SUB };
            let mut l = one(sp(format!("{} tmp", disk(t)), c));
            if o.a.pid == 0 {
                l.spans.push(dim(" /clean"));
            }
            l
        },
    },
    Seg {
        id: "lines",
        name: "Lines changed",
        about: "lines it added and removed",
        draw: |x| {
            if let Some(c) = counting(x) {
                return c;
            }
            let (add, del) = of(x).stat;
            if add + del == 0 {
                return Line::new();
            }
            line([
                sp(format!("+{add}"), GREEN),
                plain(" "),
                sp(format!("−{del}"), RED),
            ])
        },
    },
    Seg {
        id: "files",
        name: "Files changed",
        about: "how many files it changed",
        draw: |x| {
            if let Some(c) = counting(x) {
                return c;
            }
            let n = of(x).files;
            if n == 0 {
                return Line::new();
            }
            one(dim(format!("{n} file{}", plural(n))))
        },
    },
    Seg {
        id: "turns",
        name: "Turns",
        about: "how many of your messages it has had",
        draw: |x| {
            if let Some(c) = counting(x) {
                return c;
            }
            let n = of(x).sess.map_or(0, |s| s.turns.len());
            if n == 0 {
                return Line::new();
            }
            one(dim(format!("{n} turn{}", plural(n))))
        },
    },
    Seg {
        id: "time",
        name: "Time",
        about: "how long since its first turn",
        draw: |x| {
            if let Some(c) = counting(x) {
                return c;
            }
            let Some(start) = of(x)
                .sess
                .and_then(|s| s.turns.first())
                .map(|t| t.start)
                .filter(|&t| t != 0)
            else {
                return Line::new();
            };
            let d = (now(x.m) - start + 30_000) / 60_000 * 60_000;
            one(dim(dur(d)))
        },
    },
    Seg {
        id: "cache",
        name: "Prompt cache",
        about: "whether the cache is warm, and for how long: a message after that re-reads everything",
        draw: |x| {
            let Some(s) = of(x).sess else {
                return Line::new();
            };
            let t = now(x.m);
            match core::overview::cache_cold(s, t) {
                None => Line::new(),
                Some((at, true)) => line([
                    sp("cache cold", YELLOW),
                    dim(format!(" {} ago", dur(t - at))),
                ]),
                Some((at, false)) => {
                    let left = at - t;
                    let c = if left < 10 * 60_000 { YELLOW } else { GREEN };
                    line([dim("cache warm "), sp(dur(left), c)])
                }
            }
        },
    },
    Seg {
        id: "queue",
        name: "Queue",
        about: "messages waiting for the turn to end",
        draw: |x| {
            let n = of(x).queued;
            if n == 0 {
                return Line::new();
            }
            one(sp(format!("{n} queued"), ORANGE))
        },
    },
    Seg {
        id: "account",
        name: "Account",
        about: "the agent it runs, and the login it runs on",
        draw: |x| {
            let o = of(x);
            let k = o.a.kind.as_str();
            let name = crate::screens::accounts::look::agent_name(&x.m.reg, k);
            let login =
                session_login(x.m, o).map(|l| crate::screens::accounts::rows::login_name(&l));
            match login {
                Some(l) => one(dim(format!("{name} · {l}"))),
                None => one(dim(name)),
            }
        },
    },
    Seg {
        id: "session",
        name: "Session id",
        about: "the conversation's short id",
        draw: |x| {
            let id = &of(x).a.job.session_id;
            match id.get(..8) {
                Some(s) => one(sp(s, FAINT)),
                None => Line::new(),
            }
        },
    },
    Seg {
        id: "clock",
        name: "Clock",
        about: "the time of day",
        draw: |x| one(dim(crate::screens::accounts::rows::clock(now(x.m), false))),
    },
];

/// The login a rush session's agent runs on, when it says
/// (logins.go:752 sessionLogin).
pub fn session_login(m: &Model, o: &Of) -> Option<crate::screens::accounts::state::LoginView> {
    let id = o.info.map(|i| i.login.as_str()).filter(|l| !l.is_empty())?;
    let found = peek(m).logins.iter().find(|l| l.id == id).cloned();
    Some(found.unwrap_or_else(|| {
        let mut l = crate::screens::accounts::state::LoginView::default();
        l.login.id = id.to_string();
        l
    }))
}

/// What session key runs as: its profile when you made one, else
/// harness · account, and its model unless the header's own model
/// segment shows it (agentLabel).
pub fn agent_label(m: &Model, o: &Of) -> Line {
    let mut k = o.a.kind.as_str().to_string();
    if k.is_empty() {
        k = LOGINS_KIND.into();
    }
    let start = peek(m).session_start.and_then(|f| f(m, &o.key));
    let (account, mut profile) = start.map(|s| (s.account, s.profile)).unwrap_or_default();
    if !show_profile(cfg(m), &profile) {
        profile.clear();
    }
    let mut name = harness_name(&m.reg, &k);
    let account = if account.is_empty() {
        account_of(m, &k)
    } else {
        account
    };
    if !account.is_empty() && account != "default" {
        name += &format!(" · {account}");
    }
    if !profile.is_empty() {
        name += &format!(" · {profile}");
    }
    let mut s = line([glyph(&m.reg, &k), plain(" "), sp(name, TEXT)]);
    let model = o.model();
    if !model.is_empty() && !layout(m, BAR_AGENT).shown("model") {
        s.spans
            .push(dim(format!(" · {}", model_word(&m.reg, &k, model))));
    }
    s
}

/// What a session on provider k's account in use says once that account
/// is running low: how full its tightest window is, and the account, of
/// any provider, with the most room (lowNote, lowNoteNow).
pub fn low_note(m: &Model, k: &str) -> String {
    let t = now(m);
    let Some(cur) = in_use_rows(m).into_iter().rfind(|r| r.kind == k) else {
        return String::new();
    };
    // Most accounts have no reading or enough room: no alternatives made
    // just to say nothing.
    if switch_note(&cur.q, &[], t).is_empty() {
        return String::new();
    }
    let others: Vec<MeterRow> = account_rows(m)
        .iter()
        .filter(|r| {
            // Not the account itself, nor a provider's line over its
            // accounts.
            !(r.kind == k && (r.current || r.head) || r.head && switches(m, &r.kind))
        })
        .map(|r| meter_of(m, r))
        .collect();
    switch_note(&cur.q, &others, t)
}

/// How a session rush doesn't host is reached: nothing for one it does,
/// whose connection is the usual, nor for one whose host sleeps
/// (paneHeader's conn, rushmode.go:2945-2947).
fn conn_words(o: &Of) -> Line {
    if o.hosted || o.sleeping {
        return Line::new();
    }
    let a = o.a;
    if a.rush {
        return one(dim("send to resume"));
    }
    if a.past {
        return one(dim("past conversation · a message resumes it in rush mode"));
    }
    if a.headless {
        return one(dim(format!("Claude Code · {}", core::where_(a))));
    }
    if a.interactive {
        return line([
            dim(format!("Claude Code · {} · ", core::where_(a))),
            sp("/rush", ORANGE),
            dim(" copies it here"),
        ]);
    }
    line([
        dim("Claude Code · "),
        sp("/rush", ORANGE),
        dim(" moves it here"),
    ])
}

/// left, then right at the far end of w; left alone, cut, when there's
/// no room between (rushmode.go:2292 spread).
fn pane_spread(left: Line, right: Line, w: usize) -> Line {
    let (lw, rw) = (left.width(), right.width());
    if lw + rw + 2 > w {
        return fit(left, w);
    }
    let mut out = left;
    out.spans.push(plain(blanks(w - lw - rw)));
    join(&mut out, right);
    out
}

/// The column in row where word stands on its own in part, part being
/// row's end (or its start, last false): where a header segment is, for a
/// click on it (segAt).
pub fn seg_at(row: &str, part: &str, word: &str, last: bool) -> Option<usize> {
    if word.is_empty() || part.is_empty() {
        return None;
    }
    let off = if last {
        row.rfind(part)
    } else {
        row.find(part)
    }?;
    let b = part.as_bytes();
    let mut i = 0;
    while i + word.len() <= part.len() {
        let Some(j) = part[i..].find(word).map(|j| j + i) else {
            break;
        };
        let end = j + word.len();
        if (j == 0 || b[j - 1] == b' ') && (end == part.len() || b[end] == b' ') {
            return Some(width(&row[..off + j]));
        }
        i = j + 1;
    }
    None
}

/// Where the header's clickable parts landed, cells into the row's
/// content: the context label (from, width) and the mode (row, from,
/// width).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Labels {
    pub context: (usize, usize),
    pub mode: (usize, usize, usize),
}

/// Where the agent label landed on the row under the name: from and
/// width, cells into the row's content (hostConn.label).
type LabelAt = (usize, usize);

/// The name's row and the one under it, hw wide: the name, its @handle
/// and state (None: the name alone, for /statusline's preview), the bar's
/// first line at their right, and under them its second line, what the
/// agent runs as and how it's connected (paneHeader).
fn rows_with(
    m: &Model,
    o: &Of,
    state: Option<Line>,
    hw: usize,
    drops: &mut Drops,
) -> (Line, Line, Labels, LabelAt) {
    let mut labels = Labels::default();
    let k = if o.a.kind.as_str().is_empty() {
        LOGINS_KIND
    } else {
        o.a.kind.as_str()
    };
    let lead = "  ";
    let full = one_line(&o.a.display_name);
    let left1 = match state {
        None => line([plain(lead), spb(full, BRIGHT)]),
        Some(st) => {
            let mut state = line([sp("·", DIM), plain("  ")]);
            join(&mut state, st);
            let low = low_note(m, k);
            if !low.is_empty() {
                state.spans.push(plain("   "));
                state.spans.push(sp(low, YELLOW));
            }
            if let Some(away) = o.info.and_then(|i| i.away.as_ref()) {
                // In full only where it leaves the name and the bar their
                // room (away.go:220 awayChip).
                let wide = hw.saturating_sub(state.width()) >= 120;
                state.spans.push(plain("   "));
                join(
                    &mut state,
                    crate::screens::list_extras::away::away_chip(away, now(m), wide),
                );
            }
            // A name is often the whole first message; it's cut short so
            // the state and what's on the right keep their room.
            let room = 12.max(56.min(hw.saturating_sub(lead.len() + state.width()) / 2));
            let name = rush_draw::truncate(&full, room, "…").into_owned();
            let mut l = line([plain(lead), spb(name, BRIGHT), plain("   ")]);
            l.spans.push(name_handle(o.a));
            l.spans.push(plain("  "));
            join(&mut l, state);
            l
        }
    };
    let mut x = Ctx::new(m);
    x.a = Some(o);
    let mut right = bar_line(
        m,
        BAR_AGENT,
        0,
        &mut x,
        hw.saturating_sub(left1.width() + 4),
        drops,
    );
    let right_plain = right.plain();
    right.spans.push(plain(" "));
    let row1 = pane_spread(left1, right, hw);
    let plain1 = row1.plain();
    if o.sess.is_some()
        && let Some(value) = core::views::header_context(m, false)
    {
        let metric = value.plain();
        if let Some(i) = plain1.rfind(&metric)
            && right_plain.contains(&metric)
        {
            labels.context = (width(&plain1[..i]), width(&metric));
        }
    }
    if let Some(at) = seg_at(&plain1, &right_plain, &o.mode, true) {
        labels.mode = (NAME_ROW, at, width(&o.mode));
    }
    let indent = "  ";
    let mut conn = conn_words(o);
    if let Some(mut l) = cache_words(o, now(m)) {
        if conn.width() > 0 {
            l.spans.push(plain("   "));
            join(&mut l, conn);
        }
        conn = l;
    }
    let mut tag = agent_label(m, o);
    let tag_w = tag.width();
    if o.hover_label {
        tag = hover_line(tag, tag_w);
    }
    let shown = hw.saturating_sub(conn.width() + tag_w) > 40;
    if shown {
        let mut c = tag;
        if conn.width() > 0 {
            c.spans.push(plain("   "));
            join(&mut c, conn);
        }
        conn = c;
    }
    let meta = bar_line(
        m,
        BAR_AGENT,
        1,
        &mut x,
        hw.saturating_sub(indent.len() + conn.width() + 4),
        drops,
    );
    let meta_plain = meta.plain();
    let mut left2 = line([plain(indent)]);
    join(&mut left2, meta);
    let more = header_git(m, o.a);
    if more.width() > 0 && left2.width() + more.width() + conn.width() + 6 < hw {
        left2.spans.push(sp("  ·  ", DIM));
        join(&mut left2, more);
    }
    conn.spans.push(plain(" "));
    // Where the label is, for a click on it to open the switch sheet.
    let mut label = (0, 0);
    let at = hw.saturating_sub(conn.width());
    if shown && at >= left2.width() + 2 {
        label = (at, tag_w);
    }
    let row2 = pane_spread(left2, conn, hw);
    if labels.mode.2 == 0
        && let Some(at) = seg_at(&row2.plain(), &meta_plain, &o.mode, false)
    {
        labels.mode = (META_ROW, at, width(&o.mode));
    }
    (row1, row2, labels, label)
}

/// The two rows for an agent not open, for /statusline's preview: its
/// name alone on the first.
pub fn rows(m: &Model, o: &Of, w: usize, drops: &mut Drops) -> (Line, Line, Labels) {
    let (r1, r2, l, _) = rows_with(m, o, None, w, drops);
    (r1, r2, l)
}

/// A header row: two cells in, on the chrome's ground, a cell after.
fn chrome(l: Line, iw: usize) -> Line {
    use rush_core::text::{Span, Style};
    let mut out = line([Span::new("  ", Style::default().on(CHROME_BG))]);
    join(&mut out, on_ground(l, iw, CHROME_BG));
    out.spans
        .push(Span::new(" ", Style::default().on(CHROME_BG)));
    out
}

/// The open Session's header rows one and two drawn again with the agent
/// header: the bar's first line right of the name and its state, the low
/// note after them, and under them its second line, what the agent runs
/// as and how it's connected (paneHeader). Where the context and the
/// mode landed is kept for the pointer.
pub fn head(m: &mut Model, rows: &mut [Line], w: usize) {
    let Some(key) = core::open_key(m) else {
        return;
    };
    if rows.len() <= META_ROW || m.agents.iter().all(|a| a.key != key) {
        return; // gone: the Session says so
    }
    let hw = w.saturating_sub(3);
    // What's counted against its project, read once a step changes.
    let (stat, files) = core::changes::project_count(m);
    let mut drops = Drops::new();
    let (r1, r2, labels, label) = {
        let Some(mut o) = Of::read(m, &key) else {
            return;
        };
        o.stat = stat;
        o.files = files;
        let state = state_words(&o, now(m));
        rows_with(m, &o, Some(state), hw, &mut drops)
    };
    rows[NAME_ROW] = chrome(r1, hw);
    rows[META_ROW] = chrome(r2, hw);
    core::conn(m, &key).vs.context = labels.context;
    let st = m.kept::<State>();
    st.mode_at = labels.mode;
    st.label_at = label;
    super::line::keep_drops(&mut st.drops, drops);
}

/// Its state in words, coloured by what it asks of you: no glyphs, the
/// list's marks already say it at a glance (paneHeader's state).
pub fn state_words(o: &Of, now: Ms) -> Line {
    let a = o.a;
    let asks = a.needs_you() || a.waiting();
    if o.sleeping {
        return one(dim("idle"));
    }
    let Some(info) = o.info else {
        return one(if asks {
            spb("needs you", YELLOW)
        } else if a.halted() {
            spb("stopped mid-turn", RED)
        } else if a.job.live() {
            spb("working", ORANGE)
        } else if a.pid != 0 {
            dim("idle")
        } else {
            dim("stopped")
        });
    };
    let sess = o.sess;
    if let Some(l) = &info.limit {
        return one(sp(format!("paused · {}", l.text()), YELLOW));
    }
    if let Some(r) = &info.retry {
        return if r.gave_up {
            line([spb("API error", RED), sp(format!(" · {}", r.why), RED)])
        } else if r.offline {
            one(sp("offline · goes on when the network is back", YELLOW))
        } else if r.proof {
            one(sp(
                "cache expired · goes on once the connection holds",
                YELLOW,
            ))
        } else {
            // time.Until(next).Round(time.Second)
            let left = (r.next - now + 500).div_euclid(1000) * 1000;
            one(sp(
                format!("retrying · {} of {} in {}", r.attempt, r.max, dur(left)),
                YELLOW,
            ))
        };
    }
    if sess.is_some_and(|s| !s.pending().is_empty()) {
        return one(spb("needs you", YELLOW));
    }
    if let Some(t) = sess.and_then(|s| s.live()) {
        return line([
            spb("working", ORANGE),
            sp(format!(" · {}", dur(now - t.start)), ORANGE),
        ]);
    }
    if sess.is_some_and(|s| s.compacting().is_some()) {
        return one(spb("compacting", ORANGE));
    }
    if !info.error.is_empty() || a.halted() {
        return one(spb("stopped mid-turn", RED));
    }
    if asks {
        // Asked in its last message, not on a card: the list has it
        // under Needs you, so the pane says so rather than "idle".
        return line([
            spb("needs you", YELLOW),
            sp(" · its last message asks", YELLOW),
        ]);
    }
    if !a.job.background.is_empty() {
        // Its turn ended with a workflow, subagents or shells still at
        // work: the list has it under Working, so the pane isn't "idle".
        // Its host's background tasks, as the fleet read them from its
        // info (rushmode.go paneHeader's info.Background).
        return line([
            spb("working", ORANGE),
            sp(
                format!(" · {}", crate::screens::list_extras::wall::lanes_line(a)),
                ORANGE,
            ),
        ]);
    }
    one(dim("idle"))
}

/// An agent's @handle painted to go after its name (mention.go:47
/// nameHandle).
pub fn name_handle(a: &Agent) -> rush_core::text::Span {
    use crate::screens::session_cards::mention::{handle_tint, mention_name};
    let h = format!("@{}", mention_name(a, a.rush.then_some(a.job.id.as_str())));
    let c = handle_tint(&h);
    sp(h, c)
}

/// While a limit or an API error holds it, whether its prompt cache is
/// still warm (paneHeader's conn).
fn cache_words(o: &Of, now: Ms) -> Option<Line> {
    let i = o.info?;
    if i.limit.is_none() && i.retry.is_none() || i.cache_warm == 0 {
        return None;
    }
    Some(if now < i.cache_warm {
        line([
            dim("cache warm until "),
            sp(
                crate::screens::accounts::rows::clock(i.cache_warm, false),
                GREEN,
            ),
        ])
    } else {
        one(sp("cache cold", YELLOW))
    })
}

/// What the header's second line adds after where the session is:
/// commits to push or pull, and its pull request, as the list says them
/// (rushmode.go:1517 headerGit).
pub fn header_git(m: &Model, a: &Agent) -> Line {
    use crate::screens::list_extras::landing::{ListFolders, tree_of};
    let f = m
        .kept
        .peek::<ListFolders>()
        .and_then(|f| f.by_root.get(crate::list::folder_key(a, &a.root)));
    let t = tree_of(a);
    let st = f.and_then(|f| {
        if t.is_empty() {
            Some(&f.git)
        } else {
            f.trees.get(t)
        }
    });
    let mut parts = Vec::new();
    if let Some(st) = st {
        if st.ahead > 0 {
            parts.push(sp(format!("↑{} to push", st.ahead), ORANGE));
        }
        if st.behind > 0 {
            parts.push(sp(format!("↓{} to pull", st.behind), YELLOW));
        }
    }
    if let Some(pr) = &a.pr {
        parts.push(crate::list::status_short(pr));
    } else if let Some(pr) = a.prs.last() {
        parts.push(crate::list::pr_short(pr));
    }
    let mut out = Line::new();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.spans.push(dim("  ·  "));
        }
        out.spans.push(p);
    }
    out
}

/// What session key runs as now, as a start: the hosting screens' word
/// when they give one, else what its header knows (setup.go:301
/// sessionStart).
pub fn session_start(m: &Model, key: &str) -> Option<StartOver> {
    if let Some(o) = peek(m).session_start.and_then(|f| f(m, key)) {
        return Some(o);
    }
    let o = Of::read(m, key)?;
    let kind = if o.a.kind.as_str().is_empty() {
        LOGINS_KIND.to_string()
    } else {
        o.a.kind.as_str().to_string()
    };
    let mut s = StartOver {
        account: account_of(m, &kind),
        model: o.model().to_string(),
        effort: o.sess.map_or("", |s| s.effort()).to_string(),
        mode: o.mode.clone(),
        mode_set: true,
        kind,
        ..StartOver::default()
    };
    if o.info.is_some_and(|i| i.billing == "metered") {
        s.billing = rush_state::profiles::BILLING_KEY.into();
        s.account.clear();
    }
    Some(s)
}

/// The pointer over the permission mode lights it; a click on it opens
/// what changes it (model.go:1677, pane_pointer.go:54).
pub fn mouse(m: &mut Model, e: &crate::frame::Mouse) -> Out {
    if m.session.is_none() || m.on_place() || m.sheet.is_some() || m.confirm.is_some() {
        return Out::Pass;
    }
    let Some(key) = core::open_key(m) else {
        return Out::Pass;
    };
    let Some(top) = m.session.as_ref().map(|v| v.top) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let (row, at, mw) = m
        .kept
        .peek::<State>()
        .map(|s| s.mode_at)
        .unwrap_or_default();
    // The header's rows end where the body starts.
    let head_top = top.checked_sub(crate::session::HEAD_H);
    let on = mw > 0 && head_top.is_some_and(|t| y == t + row) && x >= 2 + at && x < 2 + at + mw;
    // The agent label, on the row under the name (setup.go:570 clickLabel).
    let (lx, lw) = m
        .kept
        .peek::<State>()
        .map(|s| s.label_at)
        .unwrap_or_default();
    let on_label =
        lw > 0 && head_top.is_some_and(|t| y == t + META_ROW) && x >= 2 + lx && x < 2 + lx + lw;
    match e.kind {
        MouseKind::Down(Button::Left) if on => {
            crate::screens::session_cards::permissions::permission_command(m, &key, "", false)
        }
        MouseKind::Down(Button::Left) if on_label => {
            // A click on it opens the sheet to switch it (model.go:1683).
            match session_start(m, &key) {
                Some(o) => crate::screens::accounts::startsheet::open_switch_sheet(m, &key, o),
                None => Out::Pass,
            }
        }
        MouseKind::Moved => {
            let st = m.kept::<State>();
            let want = if on { key.clone() } else { String::new() };
            let want_label = if on_label { key } else { String::new() };
            if st.hover_mode != want || st.hover_label != want_label {
                st.hover_mode = want;
                st.hover_label = want_label;
                return Out::Draw;
            }
            Out::Pass
        }
        _ => Out::Pass,
    }
}
