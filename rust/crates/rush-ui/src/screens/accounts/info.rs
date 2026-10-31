//! The info sheet: Claude Code's /status, /usage and /stats done rush's
//! way, as tabs of one sheet over the open conversation: the agent and
//! what it runs on, how full its context is, the plan's limits and what's
//! been spent, the account's history, and where its settings are. It
//! reads the open conversation every frame, so it follows the agent while
//! it's open; what's on disk is read off the UI thread when it opens.
//! After Go's internal/ui/infosheet.go (infoSheet :28, tabs :79,
//! openInfo :102, step :163, infoRow :186, key :193, body :226,
//! statusLines :282, bigMeter :409, usageLines :453, billedRow :660) and
//! infotabs.go (contextLines :24, historyLines :103, bigTokens :168,
//! short :176, hourSpark :189, settingsCounts :219, readSettingsCounts
//! :226, add :245, settingsLinks :280, settingsLines :354), caps.go:21
//! (screenNeeds), bars.go:339 (usageColor), view.go:3050 (ctxFill,
//! ctxLine) and convo/overview.go (Totals :117, PrettyModel :147).
//!
//! The Context tab draws the host's count of the window as Claude Code's
//! grid with its legend, what's inside each part and how it grew
//! (infotabs.go:24-98, session_core's context.rs); without one it says
//! why. The Why tab is what rush did to a session it hosts in the last
//! hour, as rush why says it (infosheet.go:105 and :131, infotabs.go:366
//! whyLines), read from rush's events log off the UI thread.

use std::collections::HashMap;
use std::time::Duration;

use rush_core::adapter::{AdapterRef, Feature, Fill, Profile, Registry, Stats, fill_of};
use rush_core::agent::Agent;
use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_core::usage::{Billing, Quota};

use super::look::{LOGINS_KIND, harness_name};
use super::rows::{clock, current_login_name, login_name, roughly};
use super::state::{LoginView, peek};
use crate::frame::sheet::{Sheet, sheet_row, sheet_tabs, sheet_title};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

pub const STATUS: usize = 0;
pub const CONTEXT: usize = 1;
pub const USAGE: usize = 2;
pub const HISTORY: usize = 3;
pub const WHY: usize = 4;
pub const SETTINGS: usize = 5;
const TABS: usize = 6;

const TAB_NAMES: [&str; TABS] = ["Status", "Context", "Usage", "History", "Why", "Settings"];

/// The screen each tab needs; Status is every agent's (infoTabNeeds).
const TAB_NEEDS: [&str; TABS] = ["", "context", "usage", "stats", "", "permissions"];

/// The features a screen needs of the agent (screenNeeds).
fn screen_needs(screen: &str) -> &'static [Feature] {
    match screen {
        "context" => &[Feature::Context],
        "usage" => &[Feature::Quota],
        "stats" => &[Feature::Stats],
        "skills" => &[Feature::Commands],
        "plugin" => &[Feature::Plugins],
        "hooks" => &[Feature::Hooks],
        "permissions" => &[Feature::Settings],
        "memory" => &[Feature::Memory],
        "statusline" => &[Feature::StatusLine],
        "mcp" => &[Feature::Mcp, Feature::Screen],
        _ => &[],
    }
}

/// Whether an agent of kind can open screen (canScreen).
pub fn can_screen(reg: &Registry, kind: &str, screen: &str) -> bool {
    screen_needs(screen).iter().all(|f| reg.supports(kind, *f))
}

/// What the sheet reads from disk when it opens (infoRead).
#[derive(Default)]
pub struct InfoRead {
    /// The agent's record of the account's use; Err(None) when it keeps
    /// none, Err(Some(why)) when it couldn't be read.
    pub stats: Option<Result<Stats, Option<String>>>,
    pub counts: Option<SettingsCounts>,
    /// The limits of an agent whose accounts aren't rush's logins, as last
    /// read into quotas.json.
    pub quota: Option<Quota>,
    /// What rush did to the session in the last hour (events.Why); Err
    /// when rush's log couldn't be read.
    pub why: Option<Result<Vec<rush_events::Line>, String>>,
}

/// How far back the Why tab looks (whyWindow).
const WHY_WINDOW: Ms = 60 * 60 * 1000;

/// The rush id of the session key and the one that started it, when rush
/// hosts it or did (Info.ID, Meta spawnedBy).
pub(crate) fn rush_id(m: &Model, key: &str) -> (String, String) {
    m.kept
        .peek::<crate::screens::session_core::Core>()
        .and_then(|k| k.conns.get(key))
        .map(|c| (c.info.began.id.clone(), c.info.began.spawned_by.clone()))
        .unwrap_or_default()
}

/// The sheet.
pub struct InfoSheet {
    /// The agent's key.
    pub key: String,
    pub tab: usize,
    scroll: [usize; TABS],
    /// The Settings tab's row.
    cur: usize,
    /// The tabs the agent has.
    pub shown: Vec<usize>,
    pub(super) read: InfoRead,
    settings: Vec<SettingsLink>,
    pub(super) loaded: bool,
}

/// The agent the open conversation is, its kind and the profile it runs
/// on (sessionAgent, a.Acct).
fn open_agent(m: &Model) -> Option<(&Agent, Option<Profile>)> {
    let v = m.session.as_ref()?;
    let a = m.agents.iter().find(|a| a.key == v.key)?;
    Some((a, profile(&m.reg, a)))
}

/// The profile an agent runs on: its adapter's home of the agent's
/// account's name, else its first.
fn profile(reg: &Registry, a: &Agent) -> Option<Profile> {
    let ad: AdapterRef = reg.get(a.kind.as_str())?;
    let ps = reg.profiles_of(&ad);
    ps.iter()
        .find(|p| p.name == a.job.account)
        .or(ps.first())
        .cloned()
}

/// Opens the sheet over the open conversation on tab, with the tabs its
/// agent has (openInfo).
pub fn open_info(m: &mut Model, tab: usize) -> Out {
    let Some((a, prof)) = open_agent(m) else {
        m.flash_err("open an agent's Session first");
        return Out::Draw;
    };
    let kind = a.kind.as_str().to_string();
    let context = m.session.as_ref().map_or(0, |v| v.sess.context);
    let mut shown = Vec::new();
    for (t, need) in TAB_NEEDS.iter().enumerate() {
        if t == WHY && rush_id(m, &a.key).0.is_empty() {
            continue; // rush logs only what it hosts
        }
        if need.is_empty() || can_screen(&m.reg, &kind, need) || (t == CONTEXT && context > 0) {
            shown.push(t);
        }
    }
    let key = a.key.clone();
    let cwd = a.job.cwd.clone();
    let mut k = InfoSheet {
        key: key.clone(),
        tab: if shown.contains(&tab) { tab } else { STATUS },
        scroll: [0; TABS],
        cur: 0,
        shown,
        read: InfoRead::default(),
        settings: Vec::new(),
        loaded: false,
    };
    let stats = k.shown.contains(&HISTORY);
    let settings = k.shown.contains(&SETTINGS);
    let quota = k.shown.contains(&USAGE) && kind != LOGINS_KIND;
    let (id, by) = rush_id(m, &key);
    let since = m.now - WHY_WINDOW;
    if quota {
        k.read.quota = Some(Quota {
            problem: "reading its last reading…".into(),
            ..Quota::default()
        });
    }
    m.open_sheet(Box::new(k));
    let ad = m.reg.get(&kind);
    let dir = rush_state::dir();
    m.later(
        move || {
            let mut r = InfoRead::default();
            if !id.is_empty() {
                let linked: Vec<String> = [by].into_iter().filter(|b| !b.is_empty()).collect();
                r.why = Some(
                    rush_events::why(&rush_events::path(), &id, &linked, since)
                        .map_err(|e| e.to_string()),
                );
            }
            let Some(p) = prof else { return r };
            if stats {
                r.stats = Some(match ad.as_ref().and_then(|a| a.stats_reader()) {
                    None => Err(None),
                    Some(s) => s.stats(&p).map_err(|e| {
                        let gone = e
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound);
                        (!gone).then(|| e.to_string())
                    }),
                });
            }
            if settings && let Some(ad) = ad.as_ref() {
                r.counts = Some(read_settings_counts(ad, &p, &cwd));
            }
            if quota {
                let key = format!("{}:{}", p.kind.as_str(), p.dir);
                r.quota = Some(
                    rush_core::usage::load(&dir.join("quotas.json"))
                        .remove(&key)
                        .unwrap_or_default(),
                );
            }
            r
        },
        move |m, r| {
            let reg = m.reg.clone();
            let mcp = m
                .session
                .as_ref()
                .map(|v| v.sess.mcp.clone())
                .unwrap_or_default();
            let kind = m
                .agents
                .iter()
                .find(|a| a.key == key)
                .map(|a| a.kind.as_str().to_string())
                .unwrap_or_default();
            let Some(k) = m.sheet_as::<InfoSheet>().filter(|k| k.key == key) else {
                return Out::Idle;
            };
            k.loaded = true;
            if let Some(n) = r.counts.as_ref() {
                k.settings = settings_links(&reg, &kind, n, &mcp);
            }
            k.read = r;
            m.answered();
            Out::Draw
        },
    );
    Out::Draw
}

impl InfoSheet {
    /// The Why tab: what rush did to the session in the last hour and
    /// why, each event under the one that caused it (whyLines).
    fn why_lines(&self, m: &Model, key: &str, w: usize) -> Vec<Line> {
        let Some(why) = self.read.why.as_ref().filter(|_| self.loaded) else {
            return vec![dim("  reading rush's log…")];
        };
        let lines = match why {
            Err(e) => return vec![dim(format!("  couldn't read rush's log: {e}"))],
            Ok(l) => l,
        };
        let id = rush_id(m, key).0;
        let nodes = rush_events::tree(lines);
        if nodes.is_empty() {
            return vec![
                dim("  nothing in the last hour: rush hasn't woken, held, moved or stopped it"),
                Line::new(),
                faint(format!("  rush why {id} --last 24h looks further back")),
            ];
        }
        let mut out = vec![
            then(
                info_head("The last hour"),
                dim(" · each event under the one that caused it"),
            ),
            Line::new(),
        ];
        for n in &nodes {
            let one = rush_events::Node {
                line: n.line,
                depth: 0,
            };
            let row = rush_events::format(&[one], m.now, rush_events::BUDGET, false, "");
            let row = format!(
                "{}{}",
                "  ".repeat(n.depth.min(8)),
                row.strip_suffix('\n').unwrap_or(&row)
            );
            let row = rush_draw::truncate(&row, w.saturating_sub(2), "…").into_owned();
            let c = if n.line.failed {
                RED
            } else if n.depth == 0 {
                TEXT
            } else {
                DIM
            };
            out.push(line([plain("  "), sp(row, c)]));
        }
        out.push(Line::new());
        out.push(faint(format!(
            "  as rush why {id} says it, as of when this opened"
        )));
        out
    }

    /// Moves to the next tab shown, or the one before (step).
    fn step(&mut self, by: isize) {
        let n = self.shown.len() as isize;
        let i = self.shown.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        self.tab = self.shown[((i + by + n) % n) as usize];
    }
}

/// A label and its value, lined up (infoRow).
pub fn info_row(label: &str, value: Line, w: usize) -> Line {
    let value = if value.width() == 0 {
        line([sp("–", FAINT)])
    } else {
        value
    };
    let mut l = line([plain("  "), sp(fit_str(label, 14), DIM), plain(" ")]);
    join(&mut l, fit(value, w.saturating_sub(18)));
    l
}

/// A section's heading (infoHead).
pub fn info_head(s: &str) -> Line {
    line([spb(format!("  {s}"), SUB)])
}

fn dim(s: impl Into<String>) -> Line {
    line([sp(s, DIM)])
}

fn faint(s: impl Into<String>) -> Line {
    line([sp(s, FAINT)])
}

fn text(s: impl Into<String>) -> Line {
    line([sp(s, TEXT)])
}

fn then(mut a: Line, b: Line) -> Line {
    join(&mut a, b);
    a
}

impl Sheet for InfoSheet {
    fn width(&self, _m: &Model) -> usize {
        100
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" => return Out::Close,
            "]" | "right" => self.step(1),
            "[" | "left" => self.step(-1),
            "up" => {
                if self.tab == SETTINGS {
                    self.cur = crate::frame::picker::round_move(self.cur, -1, self.settings.len());
                } else {
                    self.scroll[self.tab] = self.scroll[self.tab].saturating_sub(1);
                }
            }
            "down" => {
                if self.tab == SETTINGS {
                    self.cur = crate::frame::picker::round_move(self.cur, 1, self.settings.len());
                } else {
                    self.scroll[self.tab] += 1;
                }
            }
            "pgup" => self.scroll[self.tab] = self.scroll[self.tab].saturating_sub(10),
            "pgdown" => self.scroll[self.tab] += 10,
            "enter" => {
                if self.tab == SETTINGS
                    && let Some(l) = self.settings.get(self.cur)
                {
                    // Claude Code's own screen, with the terminal handed
                    // to it (infotabs.go:333).
                    if l.claude && l.screen == "mcp" {
                        let key = self.key.clone();
                        crate::screens::session_cards::claudescreen::open_screen(m, &key, "mcp");
                        return Out::Close;
                    }
                    m.flash_err(&format!("{} isn't in this rush yet", l.name));
                }
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let found = open_agent(m).filter(|(a, _)| a.key == self.key);
        let name = found.as_ref().map_or("Status".to_string(), |(a, _)| {
            [a.display_name.as_str(), a.job.name.as_str(), "this agent"]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("")
                .to_string()
        });
        let names: Vec<&str> = self.shown.iter().map(|t| TAB_NAMES[*t]).collect();
        let program = found.as_ref().map_or("its program".to_string(), |(a, _)| {
            harness_name(&m.reg, a.kind.as_str())
        });
        let cur = self.shown.iter().position(|t| *t == self.tab).unwrap_or(0);
        let mut out = vec![
            sheet_title(&name, &format!("the agent, its account, and {program}"), w),
            Line::new(),
            sheet_tabs(&names, cur),
            Line::new(),
        ];
        let (Some((a, prof)), Some(v)) = (found, m.session.as_ref()) else {
            out.push(dim("  this agent's Session is closed"));
            out.push(Line::new());
            out.push(keys_fit(w, &[("esc", "close")]));
            return out;
        };
        let s = &v.sess;
        let lines = match self.tab {
            CONTEXT => context_lines(m, a, s, w),
            USAGE => usage_lines(m, a, prof.as_ref(), s, self.read.quota.as_ref(), w),
            HISTORY => self.history_lines(m, a, prof.as_ref(), w, h.saturating_sub(out.len() + 3)),
            SETTINGS => self.settings_lines(w),
            WHY => self.why_lines(m, &a.key, w),
            _ => status_lines(m, a, prof.as_ref(), s, w),
        };
        // Long tabs scroll; the tab row and keys stay put.
        let room = 3.max(h.saturating_sub(out.len() + 2));
        let sc = &mut self.scroll[self.tab];
        *sc = (*sc).min(lines.len().saturating_sub(room));
        let n = lines.len();
        out.extend(lines.into_iter().skip(*sc).take(room));
        out.push(Line::new());
        out.push(if self.tab == SETTINGS {
            keys_fit(
                w,
                &[
                    ("↑↓", "choose"),
                    ("enter", "open"),
                    ("[ ]", "tabs"),
                    ("esc", "close"),
                ],
            )
        } else if n >= room {
            keys_fit(w, &[("[ ]", "tabs"), ("↑↓", "scroll"), ("esc", "close")])
        } else {
            keys_fit(w, &[("[ ]", "tabs"), ("esc", "close")])
        });
        out
    }
}

/// A model id as people say it: claude-opus-5-5[1m] is "opus 5.5 · 1M"
/// (convo.PrettyModel).
pub fn pretty_model(m: &str) -> String {
    if m.is_empty() {
        return "?".into();
    }
    let long = m.contains("[1m]");
    let m = m.replace("[1m]", "");
    let mut m = m.strip_prefix("claude-").unwrap_or(&m).to_string();
    // A date at the end: -20250929.
    if let Some(i) = m.rfind('-')
        && m.len() - i == 9
        && m[i + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        m.truncate(i);
    }
    let mut parts = m.split('-');
    let mut out = parts.next().unwrap_or("").to_string();
    let rest: Vec<&str> = parts.collect();
    if !rest.is_empty() {
        out += &format!(" {}", rest.join("."));
    }
    if long {
        out += " · 1M";
    }
    out
}

/// A context row's bar (view.go ctxBar).
fn ctx_bar(pct: f64) -> Line {
    let n = ((pct / 12.5 + 0.5) as usize).min(8);
    let c = if pct >= 80.0 {
        RED
    } else if pct >= 50.0 {
        YELLOW
    } else {
        SUB
    };
    line([sp("━".repeat(n), c), sp("━".repeat(8 - n), FAINT)])
}

/// A context row's value: its bar, percentage and what fills it (ctxLine).
pub fn ctx_line(f: Fill) -> Line {
    let mut l = ctx_bar(f.pct());
    l.spans.push(plain(" "));
    l.spans.push(sp(format!("{:.0}%", f.pct()), SUB));
    l.spans.push(sp(format!(" · {}", f.of(tokens)), DIM));
    l
}

/// How full s's context is against its window (ctxFill): the agent's own
/// compaction isn't known here, so the model's window counts.
fn ctx_fill(m: &Model, a: &Agent, s: &rush_convo::Session) -> Fill {
    let win = if s.window > 0 {
        s.window
    } else {
        let model = if s.model.is_empty() {
            &s.info.model
        } else {
            &s.model
        };
        m.reg.context_window(a.kind.as_str(), model)
    };
    fill_of(s.context, win, a.compaction)
}

fn first<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|s| !s.is_empty()).unwrap_or("")
}

/// The login a Claude Code agent runs on: the one new sessions run on,
/// as the session doesn't say (sessionLogin).
fn current_login(m: &Model) -> Option<&LoginView> {
    peek(m).logins.iter().find(|l| l.current)
}

/// The Status tab: the session, its model, the account and the program
/// (statusLines).
fn status_lines(
    m: &Model,
    a: &Agent,
    prof: Option<&Profile>,
    s: &rush_convo::Session,
    w: usize,
) -> Vec<Line> {
    let k = a.kind.as_str();
    let name = harness_name(&m.reg, k);
    let mut out = vec![info_head("Session")];
    out.push(info_row(
        "name",
        line([spb(first(&[&a.display_name, &a.job.name]), TEXT)]),
        w,
    ));
    out.push(info_row("state", text(a.job.state.as_str()), w));
    out.push(info_row("session id", dim(a.job.session_id.clone()), w));
    let mut folder = line([plain(tildify(first(&[&s.info.cwd, &s.cwd, &a.job.cwd])))]);
    if !a.branch.is_empty() {
        folder.spans.push(sp(" on ", DIM));
        folder.spans.push(sp(a.branch.clone(), SUB));
    }
    out.push(info_row("folder", folder, w));
    let conn = if a.interactive {
        line([
            sp(name.clone(), SUB),
            sp(" · a terminal session, read from its transcript", DIM),
        ])
    } else {
        line([
            sp(name.clone(), SUB),
            sp(" · its daemon, read from the transcript", DIM),
        ])
    };
    out.push(info_row("connection", conn, w));
    if !a.job.transcript_path.is_empty() {
        out.push(info_row("history", dim(tildify(&a.job.transcript_path)), w));
    }
    let id = rush_id(m, &a.key).0;
    if !id.is_empty() {
        out.push(info_row(
            "why",
            dim(format!(
                "what rush did to it: the Why tab, or rush why {id}"
            )),
            w,
        ));
    }

    out.push(Line::new());
    out.push(info_head("Model"));
    // A transcript says its model on each reply.
    let last = s.requests.last().map_or("", |r| &*r.model);
    out.push(info_row(
        "model",
        text(pretty_model(first(&[&s.model, &s.info.model, last]))),
        w,
    ));
    out.push(info_row("effort", text(s.effort()), w));
    out.push(info_row("permissions", Line::new(), w));
    if s.context > 0 {
        out.push(info_row("context", ctx_line(ctx_fill(m, a, s)), w));
    }

    out.push(Line::new());
    out.push(info_head("Account"));
    let acct = prof.map_or("", |p| p.name.as_str());
    let mut who = line([spb(acct, TEXT)]);
    let mut plan: Vec<String> = Vec::new();
    if k == LOGINS_KIND
        && let Some(lv) = current_login(m)
    {
        who = line([spb(login_name(lv), TEXT)]);
        let e = first(&[&lv.usage.email, &lv.email]);
        if !e.is_empty() {
            who.spans.push(sp(format!(" · {e}"), DIM));
        }
        if !lv.current {
            who.spans.push(sp(
                format!(
                    " · new sessions run on {}",
                    current_login_name(&peek(m).logins)
                ),
                DIM,
            ));
        }
        plan = [&lv.usage.plan, &lv.usage.org, &lv.usage.role]
            .into_iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect();
    }
    out.push(info_row("login", who, w));
    out.push(info_row("plan", text(plan.join(" · ")), w));
    out.push(info_row(
        "config",
        dim(tildify(prof.map_or("", |p| p.dir.as_str()))),
        w,
    ));

    out.push(Line::new());
    out.push(info_head(&name));
    let ver = if s.version.is_empty() {
        faint("not known from a transcript")
    } else {
        text(s.version.clone())
    };
    out.push(info_row("version", ver, w));
    if s.ntools > 0 {
        out.push(info_row("tools", text(s.ntools.to_string()), w));
    }
    let mut servers = s.mcp.clone();
    servers.sort_by_key(|s| s.status == "connected");
    for (i, sv) in servers.iter().enumerate() {
        let col = match sv.status.as_str() {
            "connected" => GREEN,
            "pending" => YELLOW,
            _ => RED,
        };
        let row = line([
            sp("● ", col),
            sp(sv.name.clone(), TEXT),
            plain(" "),
            sp(sv.status.clone(), DIM),
        ]);
        out.push(info_row(if i == 0 { "mcp" } else { "" }, row, w));
    }
    out
}

/// The Context tab: what fills the window, as a grid with its legend
/// beside it, then what's inside each part; without the host's count of
/// it, how full it is and why there's no breakdown (contextLines).
fn context_lines(m: &Model, a: &Agent, s: &rush_convo::Session, w: usize) -> Vec<Line> {
    use crate::screens::session_core::context as cx;
    let k = a.kind.as_str();
    let c = m
        .kept
        .peek::<crate::screens::session_core::Core>()
        .and_then(|core| core.conns.get(&a.key));
    let Some(u) = c.and_then(|c| c.context.as_deref()).filter(|u| u.max > 0) else {
        let mut out = Vec::new();
        if s.context > 0 {
            out.push(info_row("context", ctx_line(ctx_fill(m, a, s)), w));
            out.push(Line::new());
        }
        let name = harness_name(&m.reg, k);
        let note = m.reg.feature_of(k, Feature::Context).note;
        let mut why = if note.is_empty() {
            "the breakdown comes when this turn ends".to_string()
        } else {
            format!("{name} says {note}")
        };
        let (sleeping, hosted, proto, pid) = c.map_or((false, false, 0, 0), |c| {
            (
                c.sleeping || c.info.sleeping,
                c.hosted(),
                c.info.proto,
                c.info.agent_pid,
            )
        });
        if !can_screen(&m.reg, k, "context") {
            why = format!("{name} reports context usage without a detailed breakdown");
        } else if sleeping {
            why = "this session is idle; the breakdown refreshes when it resumes".into();
        } else if !hosted {
            why = "a breakdown needs a rush-mode session: sessions outside rush only say how full it is".into();
        } else if proto < 2 {
            why = "this agent's host is older than the breakdown: it comes once the host restarts (it does after resting idle)".into();
        } else if pid == 0 {
            why = format!("{name} is asleep: the breakdown comes when it next wakes");
        }
        out.push(dim(format!("  {why}")));
        out.extend(context_since(m, a, s, c, w));
        return out;
    };
    // Its own count of where it compacts wins over rush's.
    let f = fill_of(u.total, u.max, a.compaction);
    let mut meta = format!(
        "{} of {} · {:.0}%",
        crate::screens::session_core::paint::tokens(u.total),
        tokens(f.window),
        f.pct()
    );
    if u.auto_compact && u.auto_compact_at > 0 {
        meta.push_str(&format!(
            " · auto-compacts at {}",
            crate::screens::session_core::paint::tokens(u.auto_compact_at)
        ));
    }
    if f.compacts() {
        meta.push_str(&format!(" · model {}", tokens(f.model)));
    }
    let mut head = info_head(&pretty_model(&u.model));
    head.spans.push(plain("  "));
    head.spans.push(sp(meta, DIM));
    head.spans
        .push(sp("  · #slim drops what it never uses", FAINT));
    let mut out = vec![head, Line::new()];
    let grid = cx::grid(u, 20, 10);
    let legend = cx::legend(u, "");
    for i in 0..grid.len().max(legend.len()) {
        let mut l = line([plain("  ")]);
        match grid.get(i) {
            Some(g) => join(&mut l, g.clone()),
            None => l.spans.push(plain(" ".repeat(39))),
        }
        l.spans.push(plain("    "));
        if let Some(r) = legend.get(i) {
            join(&mut l, r.clone());
        }
        out.push(l);
    }
    let d = cx::detail(u, "  ", 6);
    if !d.is_empty() {
        out.push(Line::new());
        out.extend(d);
    }
    let ago = m.now - u.at;
    let when = if ago >= 60_000 {
        format!("{} ago", roughly(ago))
    } else {
        "just now".to_string()
    };
    out.push(Line::new());
    out.push(line([sp(
        format!("  counted {when}, from the last reply and local estimates"),
        FAINT,
    )]));
    out.extend(context_since(m, a, s, c, w));
    out
}

/// How the context grew turn by turn, and what changed at its last
/// start, each after a blank row (contextSince).
fn context_since(
    m: &Model,
    a: &Agent,
    s: &rush_convo::Session,
    c: Option<&crate::screens::session_core::Conn>,
    w: usize,
) -> Vec<Line> {
    use crate::screens::session_core::context as cx;
    let mut out = Vec::new();
    let change = c.and_then(|c| c.info.began.context_change.as_deref());
    for ls in [
        cx::growth_lines(&cx::growth(s), "  ", w.saturating_sub(4)),
        cx::change_lines(
            change,
            &harness_name(&m.reg, a.kind.as_str()),
            "  ",
            w,
            m.now,
        ),
    ] {
        if !ls.is_empty() {
            out.push(Line::new());
            out.extend(ls);
        }
    }
    out
}

/// What a session has spent and used, all told (convo Totals).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub cost: f64,
    pub turns: usize,
    pub working: Ms,
    pub tool_calls: u32,
    pub failed: u32,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_out: i64,
    pub requests: usize,
}

/// s's totals at now (Session.Totals).
pub fn totals(s: &rush_convo::Session, now: Ms) -> Totals {
    let mut t = Totals::default();
    for tn in &s.turns {
        t.turns += 1;
        t.cost += tn.cost;
        if tn.start != 0 {
            let end = if tn.live { now } else { tn.end };
            t.working += end - tn.start;
        }
    }
    if s.info.cost > t.cost {
        t.cost = s.info.cost;
    }
    for ts in s.tools.values() {
        t.tool_calls += ts.calls;
        t.failed += ts.failed;
    }
    for r in &s.requests {
        t.requests += 1;
        t.input += r.usage.input;
        t.output += r.usage.output;
        t.cache_read += r.usage.cache_read;
        t.cache_out += r.usage.cache_write_5m + r.usage.cache_write_1h;
    }
    t
}

/// A window's use's colour (usageColor).
pub fn usage_color(p: f64) -> Color {
    if p >= 85.0 {
        RED
    } else if p >= 60.0 {
        YELLOW
    } else {
        GREEN
    }
}

/// A window's use as a wide bar, with the share of the window gone
/// marked, and when it resets (bigMeter).
pub fn big_meter(pct: f64, resets: Ms, now: Ms, window: Duration, w: usize) -> Line {
    let w = w.max(10);
    let win = window.as_millis() as f64;
    let pace = if resets != 0 && resets > now && win > 0.0 {
        1.0 - (resets - now) as f64 / win
    } else {
        -1.0
    };
    let mut col = usage_color(pct);
    if pace >= 0.0 && pct < 60.0 && pct / 100.0 > pace + 0.1 {
        col = YELLOW; // burning faster than the window allows
    }
    let mut fill = ((pct / 100.0 * w as f64 + 0.5) as isize).clamp(0, w as isize) as usize;
    if pct > 0.0 && fill == 0 {
        fill = 1;
    }
    let tick = (pace >= 0.0).then(|| ((pace * w as f64) as usize).min(w - 1));
    let mut l = Line::new();
    for i in 0..w {
        l.spans.push(if Some(i) == tick && i < fill {
            sp("╋", TEXT)
        } else if Some(i) == tick {
            sp("┼", SUB)
        } else if i < fill {
            sp("━", col)
        } else {
            sp("─", FAINT)
        });
    }
    l.spans.push(plain(" "));
    l.spans.push(spb(format!("{pct:3.0}%"), col));
    if resets != 0 {
        if resets > now {
            let at = clock(resets, resets - now > 20 * 3_600_000);
            l.spans.push(sp(
                format!(" · resets in {}, {at}", roughly(resets - now)),
                DIM,
            ));
        } else {
            l.spans.push(sp(" · has reset since", DIM));
        }
    }
    l
}

/// How an agent's requests are paid for; nothing until the agent has said
/// (billedRow).
pub fn billed_row(b: &Billing, w: usize) -> Option<Line> {
    let v = match b {
        Billing::Plan => line([sp("plan", TEXT), sp(" · out of the allowance", DIM)]),
        Billing::Overage => line([
            sp("extra usage", YELLOW),
            sp(" · the allowance is spent: each request is paid for", DIM),
        ]),
        Billing::Metered => line([sp("API key", ORANGE), sp(" · every token is paid for", DIM)]),
        _ => return None,
    };
    Some(info_row("billed", v, w))
}

/// The Usage tab: the plan's limits, and what this agent has spent
/// (usageLines). other is the limits of an agent whose accounts aren't
/// rush's logins, as last read.
fn usage_lines(
    m: &Model,
    a: &Agent,
    prof: Option<&Profile>,
    s: &rush_convo::Session,
    other: Option<&Quota>,
    w: usize,
) -> Vec<Line> {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let mut out = Vec::new();
    let (q, plan, extra) = match other {
        Some(o) => (o.clone(), first(&[&o.plan, &o.balance]).to_string(), false),
        None => match current_login(m).filter(|_| a.kind.as_str() == LOGINS_KIND) {
            Some(lv) => (lv.quota.clone(), lv.usage.plan.clone(), lv.usage.extra),
            None => (Quota::default(), String::new(), false),
        },
    };
    let mut head = format!("Plan · {}", prof.map_or("", |p| p.name.as_str()));
    if !plan.is_empty() {
        head += &format!(" · {plan}");
    }
    out.push(info_head(&head));
    let meter_w = 40.min(10.max(w.saturating_sub(60)));
    for win in &q.windows {
        out.push(info_row(
            &win.name,
            big_meter(win.percent, win.resets_at, now, win.span, meter_w),
            w,
        ));
    }
    if q.windows.is_empty() {
        out.push(info_row(
            "limits",
            faint(first(&[
                &q.problem,
                "no reading yet: this account's limits come in with the next refresh",
            ])),
            w,
        ));
    } else {
        if extra {
            out.push(info_row(
                "extra usage",
                line([sp("on", TEXT), sp(" · past the limits, it's billed", DIM)]),
                w,
            ));
        }
        let mut note = Vec::new();
        if q.fetched_at != 0 {
            note.push(format!("read {}", clock(q.fetched_at, false)));
        }
        if !q.problem.is_empty() {
            note.push(q.problem.clone());
        }
        if !note.is_empty() {
            out.push(info_row(
                "",
                faint(format!(
                    "{} · ┼ marks how much of the window has gone",
                    note.join(" · ")
                )),
                w,
            ));
        }
    }

    let t = totals(s, now);
    out.push(Line::new());
    out.push(info_head("This agent"));
    let mut cost = line([spb(money(t.cost), TEXT)]);
    if t.turns > 0 {
        cost.spans.push(sp(
            format!(" · {} turns · {} working", t.turns, dur(t.working)),
            DIM,
        ));
    }
    if s.partial {
        cost = dim("counting…");
    }
    out.push(info_row("cost", cost, w));
    if t.requests > 0 && !s.partial {
        out.push(info_row(
            "requests",
            then(
                text(t.requests.to_string()),
                dim(format!(" · {} tool calls", t.tool_calls)),
            ),
            w,
        ));
        let read = t.input + t.cache_read + t.cache_out;
        let mut tin = text(tokens(read));
        if read > 0 {
            tin.spans.push(sp(
                format!(
                    " · {:.0}% from cache",
                    t.cache_read as f64 / read as f64 * 100.0
                ),
                DIM,
            ));
        }
        out.push(info_row("tokens in", tin, w));
        out.push(info_row(
            "",
            dim(format!(
                "{} new · {} cache read · {} cache written",
                tokens(t.input),
                tokens(t.cache_read),
                tokens(t.cache_out)
            )),
            w,
        ));
        out.push(info_row("tokens out", text(tokens(t.output)), w));
        // By model, when there's more than one (subagents, /model).
        let mut per: HashMap<String, (i64, i64)> = HashMap::new();
        for r in &s.requests {
            let e = per.entry(pretty_model(&r.model)).or_default();
            let u = &r.usage;
            e.0 += u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h;
            e.1 += u.output;
        }
        if per.len() > 1 {
            let mut rows: Vec<_> = per.into_iter().collect();
            rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
            for (i, (name, (tin, tout))) in rows.iter().enumerate() {
                out.push(info_row(
                    if i == 0 { "by model" } else { "" },
                    then(
                        text(fit_str(name, 18)),
                        dim(format!("{} in · {} out", tokens(*tin), tokens(*tout))),
                    ),
                    w,
                ));
            }
        }
    }
    out
}

/// A token count to three figures, up to billions (bigTokens).
pub fn big_tokens(n: i64) -> String {
    if n >= 1_000_000_000 {
        return format!("{:.1}B", n as f64 / 1e9);
    }
    tokens(n)
}

/// A count to three figures: 950, 12.6k, 1.3M (short).
pub fn short(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// The sessions started in each hour, one bar per hour (hourSpark).
pub fn hour_spark(h: &[i64; 24]) -> Option<Line> {
    let top = h.iter().copied().max().unwrap_or(0);
    if top == 0 {
        return None;
    }
    const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let mut l = Line::new();
    for &n in h {
        if n == 0 {
            l.spans.push(sp("·", FAINT));
        } else {
            let i = ((n as usize * BARS.len()) / (top as usize + 1)).min(BARS.len() - 1);
            l.spans.push(sp(BARS[i], ORANGE));
        }
    }
    Some(l)
}

/// The local date at ms: "2 Jan 2006", "2 Jan", or "Mon 2 Jan".
fn date(ms: Ms, year: bool, day: bool) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only the tm it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    let mut s = format!("{} {}", tm.tm_mday, MONTHS[tm.tm_mon.clamp(0, 11) as usize]);
    if year {
        s += &format!(" {}", tm.tm_year + 1900);
    }
    if day {
        s = format!("{} {s}", DAYS[tm.tm_wday.clamp(0, 6) as usize]);
    }
    s
}

impl InfoSheet {
    /// The History tab: the agent's own record of the account's use, by
    /// day and by model, fitted to h (historyLines).
    fn history_lines(
        &self,
        m: &Model,
        a: &Agent,
        prof: Option<&Profile>,
        w: usize,
        h: usize,
    ) -> Vec<Line> {
        let name = harness_name(&m.reg, a.kind.as_str());
        let Some(st) = self.read.stats.as_ref().filter(|_| self.loaded) else {
            return vec![dim(format!("  reading {name}'s record…"))];
        };
        let st = match st {
            Ok(st) => st,
            Err(None) => {
                return vec![dim(format!(
                    "  {name} hasn't kept a record for this account yet"
                ))];
            }
            Err(Some(e)) => return vec![dim(format!("  couldn't read its record: {e}"))],
        };
        let mut out = vec![info_head(&format!(
            "All time · {}",
            prof.map_or("", |p| p.name.as_str())
        ))];
        let mut sessions = then(
            text(thousands(st.sessions)),
            dim(format!(" · {} messages", thousands(st.messages))),
        );
        if st.first != 0 {
            sessions
                .spans
                .push(sp(format!(" since {}", date(st.first, true, false)), DIM));
        }
        out.push(info_row("sessions", sessions, w));
        if !st.longest.is_zero() {
            out.push(info_row(
                "longest",
                then(
                    text(dur(st.longest.as_millis() as Ms)),
                    dim(format!(
                        " · {} messages · {}",
                        thousands(st.longest_msgs),
                        date(st.longest_at, false, false)
                    )),
                ),
                w,
            ));
        }
        if let Some(peak) = hour_spark(&st.hours) {
            out.push(info_row("by hour", peak, w));
            out.push(info_row("", faint("0     6     12    18   23"), w));
        }
        // Days: as many as fit, newest last, bars by messages.
        let room =
            5.max(h as isize - out.len() as isize - 4 - st.models.len().min(4) as isize) as usize;
        let days = &st.days[st.days.len().saturating_sub(room)..];
        if !days.is_empty() {
            out.push(Line::new());
            out.push(info_head(&format!("Last {} days", days.len())));
            let top = days.iter().map(|d| d.messages).max().unwrap_or(1).max(1);
            let bar_w = 8.max(30.min(w.saturating_sub(70)));
            for d in days {
                let mut fill = ((d.messages.max(0) as usize * bar_w) / top as usize).min(bar_w);
                if d.messages > 0 && fill == 0 {
                    fill = 1;
                }
                let nums = format!(
                    "{:>7} msgs  {:>3} sessions  {:>6} tools  {:>6} tok",
                    short(d.messages),
                    d.sessions,
                    short(d.tool_calls),
                    big_tokens(d.total_tokens())
                );
                out.push(line([
                    plain("  "),
                    sp(fit_str(&date(d.date, false, true), 12), DIM),
                    plain(" "),
                    sp("━".repeat(fill), ORANGE),
                    sp("─".repeat(bar_w - fill), FAINT),
                    plain(" "),
                    sp(nums, DIM),
                ]));
            }
        }
        if !st.models.is_empty() {
            out.push(Line::new());
            out.push(info_head("By model"));
            for t in &st.models {
                out.push(info_row(
                    &pretty_model(&t.model),
                    then(
                        text(fit_str(&big_tokens(t.total()), 9)),
                        dim(format!(
                            "{} out · {} from cache",
                            big_tokens(t.output),
                            big_tokens(t.cache_read)
                        )),
                    ),
                    w,
                ));
            }
        }
        if !st.computed.is_empty() {
            out.push(Line::new());
            out.push(faint(format!(
                "  as Claude Code last counted it, up to {}",
                st.computed
            )));
        }
        out
    }

    /// The Settings tab (settingsLines).
    fn settings_lines(&self, w: usize) -> Vec<Line> {
        if !self.loaded {
            return vec![dim("  reading the settings files…")];
        }
        let mut out = Vec::new();
        for (i, l) in self.settings.iter().enumerate() {
            let value = if l.value.is_empty() { "–" } else { &l.value };
            let tag = if l.claude { "  claude code ↗" } else { "" };
            let about = rush_draw::truncate(
                &l.about,
                w.saturating_sub(48 + rush_core::text::width(tag)),
                "…",
            )
            .into_owned();
            let row = line([
                spb(fit_str(&l.name, 14), TEXT),
                plain(" "),
                sp(fit_str(value, 26), TEXT),
                plain(" "),
                sp(about, FAINT),
                sp(tag, SUB),
            ]);
            out.push(sheet_row(row, i == self.cur, w));
        }
        out
    }
}

/// What the Settings tab shows of the settings files and commands, read
/// off the UI thread (settingsCounts).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsCounts {
    pub allow: usize,
    pub ask: usize,
    pub deny: usize,
    pub hooks: usize,
    pub plugins: usize,
    pub env: usize,
    pub skills: usize,
    pub cmds: usize,
    pub model: String,
    pub mode: String,
    pub line: String,
}

/// Reads every settings file a session of ad on p in cwd reads, and its
/// commands and skills. It reads the disk (readSettingsCounts).
pub fn read_settings_counts(ad: &AdapterRef, p: &Profile, cwd: &str) -> SettingsCounts {
    let mut n = SettingsCounts::default();
    if let Some(sf) = ad.settings_filer() {
        for (i, f) in sf.settings_files(p, cwd).iter().enumerate() {
            if let Ok(s) = rush_state::settingsfile::File::load(&f.path) {
                n.add(&s, i == 0);
            }
        }
    }
    if let Some(cm) = ad.commander() {
        for c in cm.commands(p, cwd) {
            if c.skill {
                n.skills += 1;
            } else {
                n.cmds += 1;
            }
        }
    }
    n
}

impl SettingsCounts {
    /// Counts one settings file in; the first is the account's own, whose
    /// env block is the one shown (add).
    pub fn add(&mut self, s: &rush_state::settingsfile::File, first: bool) {
        for (key, n) in [
            ("permissions.allow", &mut self.allow),
            ("permissions.ask", &mut self.ask),
            ("permissions.deny", &mut self.deny),
        ] {
            if let Some(rules) = s.get::<Vec<serde_json::Value>>(key) {
                *n += rules.len();
            }
        }
        if let Some(hs) = s.get::<HashMap<String, Vec<serde_json::Value>>>("hooks") {
            self.hooks += hs.values().map(Vec::len).sum::<usize>();
        }
        if let Some(on) = s.get::<HashMap<String, bool>>("enabledPlugins") {
            self.plugins += on.values().filter(|v| **v).count();
        }
        if first {
            self.env = s.env().len();
        }
        let or = |a: String, b: &str| if a.is_empty() { b.to_string() } else { a };
        self.model = or(s.string("model"), &self.model);
        self.mode = or(s.string("permissions.defaultMode"), &self.mode);
        self.line = or(s.string("statusLine.command"), &self.line);
    }
}

/// A row of the Settings tab: an area of settings, what it holds now, and
/// the screen that edits it (settingsLink).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsLink {
    pub name: String,
    pub value: String,
    pub about: String,
    /// The screen it opens, whose features it needs.
    pub screen: &'static str,
    /// Opens Claude Code's own screen.
    pub claude: bool,
}

/// The Settings tab's rows, from what was read of the settings files the
/// agent's session reads (settingsLinks).
pub fn settings_links(
    reg: &Registry,
    kind: &str,
    n: &SettingsCounts,
    mcp: &[rush_core::event::McpServer],
) -> Vec<SettingsLink> {
    let join = |parts: &[String]| {
        parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let count = |n: usize, one: &str, many: &str| match n {
        0 => String::new(),
        1 => format!("1 {one}"),
        _ => format!("{n} {many}"),
    };
    let line = if n.line.contains("rush") {
        "rush's".to_string()
    } else if !n.line.is_empty() {
        "your own command".to_string()
    } else {
        String::new()
    };
    let connected = mcp.iter().filter(|s| s.status == "connected").count();
    let mcp_v = if mcp.is_empty() {
        String::new()
    } else {
        format!("{connected} of {} connected", mcp.len())
    };
    let link = |name: &str, screen: &'static str, value: String, about: &str| SettingsLink {
        name: name.into(),
        value,
        about: about.into(),
        screen,
        claude: false,
    };
    let all = [
        link(
            &harness_name(reg, kind),
            "",
            join(&[n.model.clone(), count(n.env, "env var", "env vars")]),
            "settings.json and the environment every session starts with",
        ),
        link(
            "Permissions",
            "permissions",
            join(&[
                n.mode.clone(),
                count(n.allow, "allow", "allow"),
                count(n.ask, "ask", "ask"),
                count(n.deny, "deny", "deny"),
            ]),
            "what tools may do without asking",
        ),
        link(
            "Hooks",
            "hooks",
            count(n.hooks, "hook", "hooks"),
            "commands run around tools, prompts and sessions",
        ),
        link(
            "Plugins",
            "plugin",
            count(n.plugins, "on", "on"),
            "installed plugins, discover more, marketplaces",
        ),
        link(
            "Skills",
            "skills",
            join(&[
                count(n.skills, "skill", "skills"),
                count(n.cmds, "command", "commands"),
            ]),
            "what Claude picks up, and your own / commands",
        ),
        link(
            "Status line",
            "statusline",
            line,
            "this header, rush's top bar, and Claude Code's",
        ),
        link(
            "Memory",
            "memory",
            String::new(),
            "CLAUDE.md and the other files Claude reads",
        ),
        SettingsLink {
            claude: true,
            ..link("MCP servers", "mcp", mcp_v, "connect, sign in, tools")
        },
    ];
    all.into_iter()
        .filter(|l| can_screen(reg, kind, l.screen))
        .collect()
}
