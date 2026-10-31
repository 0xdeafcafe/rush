//! The sheets opened from settings and sessions that read and write
//! rush's or an agent's settings: the layout asked for the first time
//! (viewsheet.go), the menu bar icon offered once (menubarsheet.go), the
//! network (#network, netsheet.go), a session's skills and commands
//! (/skills, skillsheet.go), and its permission rules and hooks
//! (/permissions, /hooks, rulesheet.go).
//!
//! After Go's internal/ui/viewsheet.go (viewSheet :11, askView :19, body
//! :25, key :34, pick :53, openView :64), menubarsheet.go (menuBarSheet
//! :12, askMenuBar :21, body :31, key :40, pick :56), netsheet.go
//! (netRate :48, netSeg :65, netSheet :80, key :84, body :94, netWaiting
//! :222), skillsheet.go (skillSheet :20, sourceRank :51, shown :64, key
//! :90, body :137, detail :186, sourceTitle :209, approxTokens :224) and
//! rulesheet.go (settingsFiles :30, permSheet :53, readRules :87, change
//! :127, key :156, body :223, hookSheet :270, openHooks :285, parseHooks
//! :349, key :372, body :399).

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use rush_core::adapter::{Command, Profile};
use rush_core::text::Line;
use rush_fleet::Predicates;
use rush_net::netwatch;
use rush_state::settingsfile::File;

use super::common::reg;
use super::files::edit_file;
use super::paint::{first_non_empty, trunc};
use super::{config, config_mut};
use crate::frame::confirm::Confirm;
use crate::frame::editor::edit;
use crate::frame::picker::round_move;
use crate::frame::sheet::{Sheet, sheet_row, sheet_tabs, sheet_title, text_field, window};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// Runs work off the UI and hands what it found to the open sheet of
/// type S. With no loop (tests, `--render`) it lands at once, while the
/// sheet that asked is still out of the model taking its key: then it's
/// handed back to the caller instead.
pub(crate) fn later_to<S: Sheet, T: Send + 'static>(
    m: &mut Model,
    work: impl FnOnce() -> T + Send + 'static,
    adopt: fn(&mut S, T),
) -> Option<T> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    let at_once = Arc::new(AtomicBool::new(true));
    let landed: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
    let (a2, l2) = (at_once.clone(), landed.clone());
    m.later(work, move |m, v| {
        if a2.load(Ordering::Relaxed) {
            *l2.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        } else if let Some(s) = m.sheet_as::<S>() {
            adopt(s, v);
        }
        Out::Draw
    });
    at_once.store(false, Ordering::Relaxed);
    landed.lock().unwrap_or_else(|e| e.into_inner()).take()
}

// ---- the layout ----

/// The layouts, as kept and as offered (viewChoices).
const VIEW_CHOICES: [(&str, &str, &str); 3] = [
    (
        "split",
        "Side by side",
        "Agents, and the picked agent's Session beside them",
    ),
    (
        "agent",
        "The Session alone",
        "one agent with the whole screen · esc for Agents",
    ),
    (
        "list",
        "Agents alone",
        "every agent at a glance · { } opens one",
    ),
];

/// Asks, the first time rush opens, which layout to keep (viewSheet).
#[derive(Default)]
pub struct ViewSheet {
    pub cur: usize,
}

/// Opens on the layout you kept, or asks which, the first time; then
/// offers the menu bar (startView). Agents alone kept before there was a
/// choice counts as picking it.
pub fn start_view(m: &mut Model) {
    let c = config_mut(m);
    if c.view.is_empty() && c.list_only {
        c.view = "list".into();
    }
    if c.view.is_empty() {
        m.open_sheet(Box::new(ViewSheet::default()));
        return;
    }
    open_view(m);
    ask_menu_bar(m);
}

/// Puts the kept layout on screen (openView): the Session alone opens on
/// the picked agent, with the whole screen; the others lay out on their
/// own.
pub fn open_view(m: &mut Model) {
    m.full = false;
    if config(m).view != "agent" {
        return;
    }
    let Some(i) = m.selected() else {
        return;
    };
    m.full = true;
    if m.session.as_ref().is_none_or(|v| v.key != m.agents[i].key) {
        m.open(i);
    }
}

impl ViewSheet {
    /// Keeps the layout chosen and puts it on screen (pick).
    fn pick(&self, m: &mut Model) -> Out {
        config_mut(m).set_view(VIEW_CHOICES[self.cur].0);
        super::changed(m);
        m.sheet = None;
        open_view(m);
        ask_menu_bar(m);
        if m.sheet.is_some() {
            return Out::Draw;
        }
        Out::Close
    }
}

impl Sheet for ViewSheet {
    fn width(&self, _m: &Model) -> usize {
        76
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title("Layout", "how should rush open?", w),
            Line::new(),
        ];
        for (i, (_, what, about)) in VIEW_CHOICES.iter().enumerate() {
            out.push(sheet_row(
                line([spb(*what, TEXT), plain("  "), sp(*about, DIM)]),
                i == self.cur,
                w,
            ));
        }
        out.extend([
            Line::new(),
            line([sp(
                "#view or Settings › General changes it · shift+← → too",
                DIM,
            )]),
            Line::new(),
            keys_fit(
                w,
                &[("↑↓", "choose"), ("enter", "pick"), ("esc", "side by side")],
            ),
        ]);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "up" | "shift+tab" => self.cur = round_move(self.cur, -1, VIEW_CHOICES.len()),
            "down" | "tab" => self.cur = round_move(self.cur, 1, VIEW_CHOICES.len()),
            "1" | "2" | "3" => {
                self.cur = (k.s.as_bytes()[0] - b'1') as usize;
                return self.pick(m);
            }
            "enter" => return self.pick(m),
            "esc" | "ctrl+c" => {
                self.cur = 0;
                return self.pick(m);
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

// ---- the menu bar icon ----

const MENU_BAR_CHOICES: [(&str, &str); 2] = [
    (
        "Put it in the menu bar",
        "usage, what's working, and questions to answer from their notification",
    ),
    ("Not now", "rush notifies from the terminal as it does now"),
];

/// Offers, once, on a Mac, to put rush in the menu bar (menuBarSheet).
#[derive(Default)]
pub struct MenuBarSheet {
    pub cur: usize,
}

/// Asks, the first time rush opens on a Mac (after the layout), whether
/// to run the menu bar icon (askMenuBar).
pub fn ask_menu_bar(m: &mut Model) {
    let c = config(m);
    if !cfg!(target_os = "macos") || c.menu_bar || c.menu_bar_asked || m.sheet.is_some() {
        return;
    }
    m.open_sheet(Box::new(MenuBarSheet::default()));
}

impl MenuBarSheet {
    fn pick(&self, m: &mut Model) -> Out {
        let c = config_mut(m);
        c.menu_bar_asked = true;
        c.menu_bar = self.cur == 0;
        let on = c.menu_bar;
        super::changed(m);
        if on {
            m.flash("putting rush in the menu bar…");
            start_menu_bar(m);
        }
        Out::Close
    }
}

/// Opens the menu bar icon when it's on, building it first if rush
/// changed since; one left running by an older rush is replaced
/// (startMenuBar, model.go:602). Off the UI; a failure is said. Only the
/// rush that leads starts it (a following one, a test or `--render`
/// never does: it's an app on the Mac, not this window's).
pub fn start_menu_bar(m: &mut Model) {
    if !super::store(m).config.menu_bar || m.sender().is_none() || !m.leads() {
        return;
    }
    m.later(rush_menubar::start, |m, r| match r {
        Ok(()) => Out::Idle,
        Err(e) => {
            m.fail(&e);
            Out::Draw
        }
    });
}

/// Quits the menu bar icon, every copy of it (menubar.Stop): it reads its
/// lock file and runs pkill, so off the UI. Only the rush that leads.
pub fn stop_menu_bar(m: &mut Model) {
    if m.sender().is_none() || !m.leads() {
        return;
    }
    m.later(rush_menubar::stop, |_, ()| Out::Idle);
}

/// What the menu bar row and sheet do once the choice is kept: the icon
/// started or stopped at once (settings_general.go:126-136); a rush that
/// follows says it leaves it to the one that leads.
pub fn menu_bar_changed(m: &mut Model, on: bool) {
    if !m.leads() {
        m.flash("kept · this rush follows another, so it leaves the menu bar icon alone");
        return;
    }
    if on {
        start_menu_bar(m);
    } else {
        stop_menu_bar(m);
    }
}

impl Sheet for MenuBarSheet {
    fn width(&self, _m: &Model) -> usize {
        76
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title("Menu bar", "keep rush in the macOS menu bar?", w),
            Line::new(),
        ];
        for (i, (what, about)) in MENU_BAR_CHOICES.iter().enumerate() {
            out.push(sheet_row(
                line([spb(*what, TEXT), plain("  "), sp(*about, DIM)]),
                i == self.cur,
                w,
            ));
        }
        out.extend([
            Line::new(),
            line([sp(
                "Settings › General or rush menubar [off] changes it",
                DIM,
            )]),
            Line::new(),
            keys_fit(
                w,
                &[("↑↓", "choose"), ("enter", "pick"), ("esc", "not now")],
            ),
        ]);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "up" | "shift+tab" | "down" | "tab" => {
                self.cur = 1 - self.cur;
                Out::Draw
            }
            "y" => {
                self.cur = 0;
                self.pick(m)
            }
            "n" | "esc" | "ctrl+c" => {
                self.cur = 1;
                self.pick(m)
            }
            "enter" => self.pick(m),
            _ => Out::Idle,
        }
    }
}

// ---- the network ----

/// Bytes a second, short: 0B, 40K, 1.2M (netRate).
pub fn net_rate(b: f64) -> String {
    const K: f64 = 1024.0;
    if b >= K * K * K {
        format!("{:.1}G", b / (K * K * K))
    } else if b >= 10.0 * K * K {
        format!("{:.0}M", b / (K * K))
    } else if b >= K * K {
        format!("{:.1}M", b / (K * K))
    } else if b >= K {
        format!("{:.0}K", b / K)
    } else {
        format!("{b:.0}B")
    }
}

/// The top bar's network: whether it's been steady (netSeg).
pub fn net_seg() -> Line {
    let s = netwatch::now();
    if !s.known {
        Line::new()
    } else if !s.up {
        line([sp("offline", RED)])
    } else if !s.steady {
        line([sp("net shaky", YELLOW)])
    } else {
        line([sp("net steady", DIM)])
    }
}

/// The network watched for the TUI's whole life, as Go's Init starts it
/// (model.go:560 watchNet): the top bar's "net steady / net shaky /
/// offline" needs the API checked on its timings, as Go's netwatch always
/// does.
#[derive(Default)]
struct NetWatched(Option<netwatch::ChecksGuard<'static>>);

/// Starts netwatch and waits for the network to change (watchNet): each
/// time the API comes back or the machine joins another network, what
/// waited on it is asked again at once (onNet). Only with a loop: a test
/// or `--render` never watches.
pub fn watch_net(m: &mut Model) {
    let Some(tx) = m.sender() else {
        return;
    };
    if m.kept::<NetWatched>().0.is_some() {
        return;
    }
    netwatch::start();
    m.kept::<NetWatched>().0 = Some(netwatch::want_checks());
    let tx = tx.lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-net".into())
        .spawn(move || {
            loop {
                let rx = netwatch::changes();
                if rx.recv().is_err() {
                    return;
                }
                let apply: crate::frame::work::Apply = Box::new(on_net);
                if !tx.send(crate::worker::Note::Apply(apply)) {
                    return;
                }
            }
        });
}

/// Whether this rush watches the network (a TUI, after watch_net).
pub fn net_watched(m: &Model) -> bool {
    m.kept.peek::<NetWatched>().is_some_and(|n| n.0.is_some())
}

/// The API came back or the network changed: usage, logins and every
/// other account's limits asked for again at once rather than at the
/// next fetch, unless it's down (onNet).
pub fn on_net(m: &mut Model) -> Out {
    let s = netwatch::now();
    let down = netwatch::down();
    let mut f = serde_json::Map::new();
    f.insert("up".into(), s.up.into());
    f.insert("known".into(), s.known.into());
    f.insert("target".into(), s.target.clone().into());
    f.insert("why".into(), s.why.clone().into());
    f.insert("refetch".into(), (!down).into());
    rush_events::log("network-changed", f);
    if !down {
        crate::screens::accounts::logins::fetch_usage(m);
        crate::screens::accounts::logins::find_logins(m);
        crate::screens::bars::fetch::fetch_now(m);
    }
    Out::Draw
}

/// #network: whether the API answers, the network rush is on, what of
/// rush's waits for it, and the sessions it stopped (netSheet). While
/// it's open the network is checked and its rates read.
pub struct NetSheet {
    _checks: netwatch::ChecksGuard<'static>,
    _rates: netwatch::RatesGuard<'static>,
    /// When it's drawn again: a second after it last was.
    next: Option<std::time::Instant>,
}

impl NetSheet {
    pub fn new() -> NetSheet {
        netwatch::start();
        NetSheet {
            _checks: netwatch::want_checks(),
            _rates: netwatch::watch_rates(),
            next: None,
        }
    }
}

#[cfg(test)]
impl NetSheet {
    /// The sheet with netwatch not started: a test never asks the
    /// network.
    pub(crate) fn unstarted() -> NetSheet {
        NetSheet {
            _checks: netwatch::want_checks(),
            _rates: netwatch::watch_rates(),
            next: None,
        }
    }
}

impl Default for NetSheet {
    fn default() -> Self {
        NetSheet::new()
    }
}

/// d to the millisecond, as Go's Duration prints it rounded: "41ms",
/// "1.2s".
fn go_dur(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let s = d.as_secs_f64();
    if s < 60.0 {
        let t = format!("{:.3}", (s * 1000.0).round() / 1000.0);
        let t = t.trim_end_matches('0').trim_end_matches('.');
        return format!("{t}s");
    }
    format!("{}m{}s", d.as_secs() / 60, d.as_secs() % 60)
}

/// Whether an agent waits on the network or the API, and why
/// (netWaiting).
fn net_waiting(a: &rush_core::agent::Agent, now: rush_core::time::Ms) -> Option<String> {
    if a.job.detail.starts_with("offline") {
        return Some(a.job.detail.clone());
    }
    if a.halted() && a.continues(now) {
        return Some(a.halt_reason(now));
    }
    None
}

fn in_where(w: &str) -> String {
    if w.is_empty() || w == "?" {
        String::new()
    } else {
        format!(" in {w}")
    }
}

fn or_dash(s: &str) -> String {
    if s.is_empty() { "–".into() } else { s.into() }
}

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl Sheet for NetSheet {
    fn width(&self, _m: &Model) -> usize {
        96
    }

    fn key(&mut self, _m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "c" | "r" => {
                netwatch::check();
                Out::Draw
            }
            "esc" | "q" | "enter" | "ctrl+c" => Out::Close,
            _ => Out::Idle,
        }
    }

    fn wake_at(&self, _m: &Model) -> Option<std::time::Instant> {
        // Its ages and rates move while it's open: drawn a second after
        // it last was (a time fixed when drawn, so the wake comes due).
        self.next
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.next = Some(std::time::Instant::now() + Duration::from_secs(1));
        let s = netwatch::now();
        let now = m.now;
        let label = |l: &str| sp(format!("{l:<13}"), SUB);
        let mut out = vec![
            sheet_title(
                "Network",
                "whether the API answers, the network rush is on, and what waits for it",
                w,
            ),
            Line::new(),
        ];
        // The API.
        let latency = go_dur(Duration::from_millis(s.latency.as_millis() as u64));
        let api = if !s.known {
            line([
                sp("● checking", YELLOW),
                sp(format!(" · {}", s.target), DIM),
            ])
        } else if s.up && s.latency > Duration::from_secs(1) {
            line([
                sp("● slow", YELLOW),
                sp(format!(" · {} · connects in {latency}", s.target), DIM),
            ])
        } else if s.up {
            line([
                sp("● answers", GREEN),
                sp(
                    format!(
                        " · {} · connects in {latency} · up {}",
                        s.target,
                        dur(now - s.since)
                    ),
                    DIM,
                ),
            ])
        } else {
            line([
                sp("● can't be reached", RED),
                sp(
                    format!(" · {} · {} · down {}", s.target, s.why, dur(now - s.since)),
                    DIM,
                ),
            ])
        };
        let mut l = line([label("API")]);
        join(&mut l, api);
        out.push(l);
        if s.known {
            let next = if !s.up {
                "every 5–30s until it answers"
            } else if !s.steady {
                "every 30s until it's steady"
            } else {
                "every 2m while it's steady"
            };
            let checking = if s.checking { " · checking now" } else { "" };
            out.push(line([
                label(""),
                sp(
                    format!(
                        "checked {} ago · {} checks · {next}{checking}",
                        age(now - s.checked),
                        s.checks
                    ),
                    FAINT,
                ),
            ]));
        }
        // Whether it's been steady.
        let window_ms = s.window.as_millis() as rush_core::time::Ms;
        let steady = if !s.known {
            line([sp("–", FAINT)])
        } else if !s.steady {
            line([
                sp("● shaky", YELLOW),
                sp(
                    format!(
                        " · over the last {}: {}",
                        dur(window_ms),
                        s.shaky.join(" · ")
                    ),
                    DIM,
                ),
            ])
        } else {
            line([
                sp("● steady", GREEN),
                sp(
                    format!(
                        " · every check answered, promptly, on one network over the last {}",
                        dur(window_ms)
                    ),
                    DIM,
                ),
            ])
        };
        let mut l = line([label("Steadiness")]);
        join(&mut l, steady);
        out.push(l);
        // The network, and how fast it moves.
        let speed = if s.rated {
            sp(
                format!("↓{}/s  ↑{}/s", net_rate(s.rx_rate), net_rate(s.tx_rate)),
                TEXT,
            )
        } else {
            sp("–", FAINT)
        };
        out.push(Line::new());
        out.push(line([
            label("Network"),
            sp(or_dash(&s.network), TEXT),
            sp("  ·  ", DIM),
            speed,
        ]));
        // Noticing another network.
        if s.changes == 0 {
            out.push(line([
                label("New network"),
                sp("none since rush opened", DIM),
                sp(
                    " · interfaces looked at every 5s; a change checks the API at once",
                    FAINT,
                ),
            ]));
        } else {
            let n = format!(
                "{} change{} · last {} ago",
                s.changes,
                plural(s.changes),
                age(now - s.changed)
            );
            let how = if s.noticed > Duration::ZERO {
                let ms = (s.noticed.as_millis() as f64 / 10.0).round() * 10.0;
                sp(
                    format!(
                        "API checked {} after it changed",
                        go_dur(Duration::from_millis(ms as u64))
                    ),
                    GREEN,
                )
            } else {
                sp("checking the API on it…", YELLOW)
            };
            out.push(line([
                label("New network"),
                sp(format!("{n} · "), DIM),
                how,
            ]));
        }
        // rush's own jobs.
        out.push(Line::new());
        out.push(line([spb("rush's jobs that use it", TEXT)]));
        if s.jobs.is_empty() {
            out.push(line([plain("  "), sp("none has run yet", FAINT)]));
        }
        for j in &s.jobs {
            let (mark, what) = if j.paused {
                (
                    sp("⏸", YELLOW),
                    sp(
                        format!("waiting for the network · held back {}×", j.skipped),
                        YELLOW,
                    ),
                )
            } else if !j.err.is_empty() {
                (
                    sp("✗", RED),
                    sp(format!("failed: {}", one_line(&j.err)), RED),
                )
            } else if j.runs > 0 {
                (
                    sp("✓", GREEN),
                    sp(
                        format!("ran {}× · last {} ago", j.runs, age(now - j.last)),
                        DIM,
                    ),
                )
            } else {
                (sp("✓", GREEN), sp("not run yet", DIM))
            };
            out.push(trunc(
                line([
                    plain("  "),
                    mark,
                    plain(" "),
                    sp(format!("{:<22}", j.name), TEXT),
                    what,
                ]),
                w,
            ));
        }
        // Sessions it stopped.
        out.push(Line::new());
        out.push(line([spb("Sessions waiting on it", TEXT)]));
        let mut waiting = 0;
        for a in &m.agents {
            let Some(why) = net_waiting(a, now) else {
                continue;
            };
            waiting += 1;
            let name = trunc(line([plain(a.display_name.clone())]), 22).plain();
            out.push(trunc(
                line([
                    plain("  "),
                    sp("⟳", YELLOW),
                    plain(" "),
                    sp(format!("{name:<22}"), TEXT),
                    sp(why, DIM),
                ]),
                w,
            ));
        }
        if waiting == 0 {
            out.push(line([plain("  "), sp("none", FAINT)]));
        }
        // The UI's own thread.
        let (n, longest, last, path) = crate::frame::uistall::report();
        let after = go_dur(crate::frame::uistall::STALL_AFTER);
        out.push(Line::new());
        out.push(line([spb("Keys and frames", TEXT)]));
        if n == 0 {
            out.push(line([
                plain("  "),
                sp("✓", GREEN),
                plain(" "),
                sp(
                    format!("nothing has held the UI up over {after} since rush opened"),
                    DIM,
                ),
            ]));
        } else {
            let ago = last
                .at
                .and_then(|t| t.elapsed().ok())
                .map_or_else(String::new, |d| age(d.as_millis() as rush_core::time::Ms));
            out.push(line([
                plain("  "),
                sp("!", YELLOW),
                plain(" "),
                sp(format!("held up {n}× over {after}"), TEXT),
                sp(
                    format!(
                        " · longest {} by {}{} · last {ago} ago",
                        go_dur(Duration::from_millis(longest.took.as_millis() as u64)),
                        longest.what,
                        in_where("")
                    ),
                    DIM,
                ),
            ]));
            out.push(line([
                plain("    "),
                sp(
                    format!(
                        "what it was doing: {}",
                        tildify(
                            &path
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        )
                    ),
                    FAINT,
                ),
            ]));
        }
        out.push(Line::new());
        out.push(keys_fit(w, &[("c", "check now"), ("esc", "close")]));
        out.truncate(h);
        out
    }
}

// ---- /skills ----

/// Your own first, then the project's, claude.ai's, and the plugins'
/// (sourceRank).
fn source_rank(s: &str) -> u8 {
    match s {
        "yours" => 0,
        "project" => 1,
        "claude.ai" => 2,
        _ => 3,
    }
}

/// Whether a skill or command is yours to change (editable).
fn editable(c: &Command) -> bool {
    c.source == "yours" || c.source == "project"
}

/// Where a skill or command comes from, as a heading (sourceTitle).
pub fn source_title(s: &str) -> String {
    match s {
        "yours" => "Yours".into(),
        "project" => "This project's".into(),
        "claude.ai" => "From claude.ai".into(),
        "" => "Other".into(),
        s => format!("Plugin · {s}"),
    }
}

/// A file's size as tokens, roughly (approxTokens).
pub fn approx_tokens(size: i64) -> String {
    if size <= 0 {
        return String::new();
    }
    let t = size / 4;
    if t >= 1000 {
        return format!("~{:.1}k tok", t as f64 / 1000.0);
    }
    format!("~{t} tok")
}

/// What using a skill does: the session's message box gets "/name ".
pub type UseIt = Rc<dyn Fn(&mut Model, &str)>;

/// The skills and custom commands a session can use, grouped by where
/// they come from: search, see one whole, use it or edit your own
/// (skillSheet).
pub struct SkillSheet {
    pub all: Vec<Command>,
    /// 0 skills, 1 commands.
    pub tab: usize,
    pub cur: [usize; 2],
    pub query: Vec<char>,
    pub query_pos: usize,
    pub use_it: UseIt,
}

/// Opens /skills for agent kind's session with these commands; use_it
/// puts one in its message box (openSkills).
pub fn open_skills(m: &mut Model, kind: &str, all: Vec<Command>, use_it: UseIt) {
    let lists = reg().get(kind).is_some_and(|a| a.commander().is_some());
    if !lists {
        m.flash_err(&format!(
            "{} has no skills or commands rush lists",
            reg().harness_label(kind)
        ));
        return;
    }
    m.open_sheet(Box::new(SkillSheet {
        all,
        tab: 0,
        cur: [0; 2],
        query: Vec::new(),
        query_pos: 0,
        use_it,
    }));
}

impl SkillSheet {
    /// The tab's list for the search, grouped by source (shown).
    pub fn shown(&self) -> Vec<Command> {
        let q: String = self.query.iter().collect::<String>().trim().to_lowercase();
        let mut out: Vec<Command> = self
            .all
            .iter()
            .filter(|c| c.skill == (self.tab == 0))
            .filter(|c| {
                q.is_empty()
                    || format!("{} {} {}", c.name, c.description, c.source)
                        .to_lowercase()
                        .contains(&q)
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            source_rank(&a.source)
                .cmp(&source_rank(&b.source))
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    fn detail(list: &[Command], cur: usize, w: usize) -> Vec<Line> {
        let Some(c) = list.get(cur) else {
            return Vec::new();
        };
        let mut head = line([plain("  "), spb(format!("/{}", c.name), TEXT)]);
        if !c.argument_hint.is_empty() {
            head.spans.push(plain(" "));
            head.spans.push(sp(c.argument_hint.clone(), DIM));
        }
        let mut out = vec![head];
        let desc = one_line(first_non_empty(&[&c.description, "no description"]));
        for l in wrap(&desc, w.saturating_sub(4)).into_iter().take(3) {
            out.push(line([plain("  "), sp(l, SUB)]));
        }
        let mut place = tildify(&c.path);
        if !editable(c) {
            place.push_str(&format!(" · {}'s, read-only", c.source));
        }
        out.push(line([
            plain("  "),
            trunc(
                line([sp(
                    format!("{place} · {} when used", approx_tokens(c.size)),
                    DIM,
                )]),
                w.saturating_sub(4),
            )
            .spans
            .into_iter()
            .next()
            .unwrap_or_else(|| plain("")),
        ]));
        out
    }
}

impl Sheet for SkillSheet {
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let list = self.shown();
        let n = list.len();
        let cur = &mut self.cur[self.tab];
        *cur = (*cur).min(n.saturating_sub(1));
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "[" | "]" => self.tab = 1 - self.tab,
            "up" => *cur = round_move(*cur, -1, n),
            "down" => *cur = round_move(*cur, 1, n),
            "pgup" => *cur = cur.saturating_sub(10),
            "pgdown" => *cur = (*cur + 10).min(n.saturating_sub(1)),
            "enter" => {
                let Some(c) = list.get(*cur) else {
                    return Out::Idle;
                };
                (self.use_it)(m, &c.name);
                return Out::Close;
            }
            "ctrl+e" | "ctrl+o" => {
                let Some(c) = list.get(*cur).filter(|c| !c.path.is_empty()) else {
                    return Out::Idle;
                };
                if !editable(c) {
                    m.flash_err(&format!(
                        "{} comes from {}: an update would overwrite changes to it",
                        c.name, c.source
                    ));
                    return Out::Draw;
                }
                return edit_file(m, &c.path);
            }
            _ => {
                let Some((q, pos)) = edit(&self.query, self.query_pos, k) else {
                    return Out::Idle;
                };
                self.query = q;
                self.query_pos = pos;
                self.cur[self.tab] = 0;
            }
        }
        Out::Draw
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let skills = self.all.iter().filter(|c| c.skill).count();
        let cmds = self.all.len() - skills;
        let (t1, t2) = (format!("Skills {skills}"), format!("Commands {cmds}"));
        let mut search = line([plain("  "), sp("⌕ ", ORANGE)]);
        join(
            &mut search,
            text_field(
                &self.query,
                self.query_pos,
                true,
                "search",
                w.saturating_sub(6),
            ),
        );
        let mut out = vec![
            sheet_title(
                "Skills",
                "what Claude picks up when a task calls for it, and your own / commands",
                w,
            ),
            Line::new(),
            sheet_tabs(&[&t1, &t2], self.tab),
            Line::new(),
            search,
            Line::new(),
        ];
        let list = self.shown();
        let cur = self.cur[self.tab].min(list.len().saturating_sub(1));
        let detail = SkillSheet::detail(&list, cur, w);
        let list_h = 3.max(h as isize - out.len() as isize - detail.len() as isize - 4) as usize;
        let mut rows = Vec::new();
        let (mut sel_at, mut last) = (0, None::<String>);
        for (i, c) in list.iter().enumerate() {
            if last.as_deref() != Some(c.source.as_str()) {
                rows.push(line([spb(format!("  {}", source_title(&c.source)), SUB)]));
                last = Some(c.source.clone());
            }
            if i == cur {
                sel_at = rows.len();
            }
            let mut name = c.name.as_str();
            if let Some((_, n)) = c.name.split_once(':')
                && !matches!(c.source.as_str(), "yours" | "project" | "claude.ai")
            {
                name = n; // the plugin's heading says whose it is
            }
            let mut l = line([
                spb(fit_str(name, 26), TEXT),
                plain(" "),
                sp(fit_str(&approx_tokens(c.size), 10), DIM),
                plain(" "),
            ]);
            join(
                &mut l,
                trunc(
                    line([sp(one_line(&c.description), FAINT)]),
                    w.saturating_sub(44),
                ),
            );
            rows.push(sheet_row(l, i == cur, w));
        }
        if list.is_empty() {
            rows.push(line([sp("  nothing matches", DIM)]));
        }
        let (from, to) = window(rows.len(), sel_at, list_h);
        out.extend(rows[from..to].iter().cloned());
        out.push(Line::new());
        out.push(line([sp("─".repeat(w), FAINT)]));
        out.extend(detail);
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[
                ("type", "search"),
                ("enter", "use it"),
                ("ctrl+e", "edit"),
                ("[ ]", "skills/commands"),
                ("esc", "close"),
            ],
        ));
        out
    }
}

// ---- /permissions and /hooks ----

/// One of the settings files a session reads (settingsFile).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsFile {
    pub label: String,
    pub path: String,
}

/// The files a session of k's on p in cwd reads, in the order it reads
/// them; none if k's settings aren't files rush edits (settingsFiles).
pub fn settings_files(k: &str, p: &Profile, cwd: &str) -> Vec<SettingsFile> {
    let Some(a) = reg().get(k) else {
        return Vec::new();
    };
    let Some(sf) = a.settings_filer() else {
        return Vec::new();
    };
    sf.settings_files(p, cwd)
        .into_iter()
        .map(|f| SettingsFile {
            label: f.label,
            path: f.path,
        })
        .collect()
}

/// The kinds of rule, as the tabs and the files name them (ruleKinds).
const RULE_KINDS: [&str; 3] = ["allow", "ask", "deny"];

/// One permission rule and the file it's in (permRule).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermRule {
    pub text: String,
    pub file: SettingsFile,
}

/// Every rule in the files, as read off the UI (permRead).
#[derive(Clone, Debug, Default)]
pub struct PermRead {
    pub files: Vec<SettingsFile>,
    pub rules: [Vec<PermRule>; 3],
    pub mode: String,
    pub err: String,
}

/// Every rule in the files (readRules). It reads them, so never on the
/// UI.
pub fn read_rules(files: Vec<SettingsFile>) -> PermRead {
    let mut r = PermRead {
        files,
        ..PermRead::default()
    };
    for f in r.files.clone() {
        let s = match File::load(&f.path) {
            Ok(s) => s,
            Err(e) => {
                r.err = e.to_string();
                continue;
            }
        };
        for (i, kind) in RULE_KINDS.iter().enumerate() {
            let list: Vec<String> = s.get(&format!("permissions.{kind}")).unwrap_or_default();
            for rule in list {
                r.rules[i].push(PermRule {
                    text: rule,
                    file: f.clone(),
                });
            }
        }
        let mode = s.string("permissions.defaultMode");
        if !mode.is_empty() {
            r.mode = format!("{mode} ({})", f.label);
        }
    }
    r
}

/// /permissions: the allow, ask and deny rules from every settings file,
/// each marked with its file; add one to any file, or take one out
/// (permSheet).
#[derive(Default)]
pub struct PermSheet {
    pub files: Vec<SettingsFile>,
    pub rules: [Vec<PermRule>; 3],
    pub mode: String,
    pub tab: usize,
    pub cur: [usize; 3],
    pub adding: bool,
    pub input: Vec<char>,
    pub pos: usize,
    /// Which file a new rule goes to.
    pub target: usize,
    /// The first read has landed.
    pub loaded: bool,
    pub err: String,
}

/// Opens /permissions for agent k's session on p in cwd
/// (openPermissions).
pub fn open_permissions(m: &mut Model, k: &str, p: &Profile, cwd: &str) {
    m.open_sheet(Box::new(PermSheet::default()));
    let files = settings_files(k, p, cwd);
    m.later(
        move || read_rules(files),
        |m, r| {
            if let Some(s) = m.sheet_as::<PermSheet>() {
                s.adopt(r);
            }
            Out::Draw
        },
    );
}

impl PermSheet {
    /// Takes in what was read (adopt).
    pub fn adopt(&mut self, r: PermRead) {
        if !self.loaded {
            self.target = r.files.len().saturating_sub(1);
        }
        self.loaded = true;
        self.files = r.files;
        self.rules = r.rules;
        self.mode = r.mode;
        if !r.err.is_empty() {
            self.err = r.err;
        }
    }
}

/// Edits one file's list for a kind of rule and reads everything again,
/// off the UI; one change waits for the one before (change). What's read
/// comes back here when it lands at once.
fn change_rules(
    m: &mut Model,
    f: SettingsFile,
    tab: usize,
    files: Vec<SettingsFile>,
    edit_list: impl FnOnce(Vec<String>) -> Vec<String> + Send + 'static,
) -> Option<PermRead> {
    let key = format!("permissions.{}", RULE_KINDS[tab]);
    later_to(
        m,
        move || {
            static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = ONE.lock();
            let res = File::load(&f.path).and_then(|mut s| {
                let list: Vec<String> = s.get(&key).unwrap_or_default();
                let list = edit_list(list);
                if list.is_empty() {
                    s.delete(&key);
                } else {
                    s.set(&key, &list)?;
                }
                s.save()
            });
            let mut r = read_rules(files);
            if let Err(e) = res {
                r.err = format!("couldn't save {}: {e}", tildify(&f.path));
            }
            r
        },
        PermSheet::adopt,
    )
}

impl Sheet for PermSheet {
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let s = k.s.as_str();
        if self.adding {
            match s {
                "esc" => self.adding = false,
                "tab" | "shift+tab" if !self.files.is_empty() => {
                    let n = self.files.len();
                    let d = if s == "shift+tab" { n - 1 } else { 1 };
                    self.target = (self.target + d) % n;
                }
                "enter" => {
                    let text: String = self.input.iter().collect::<String>().trim().to_string();
                    self.adding = false;
                    self.input.clear();
                    self.pos = 0;
                    if !text.is_empty()
                        && let Some(f) = self.files.get(self.target).cloned()
                    {
                        let r = change_rules(m, f, self.tab, self.files.clone(), move |mut l| {
                            if !l.contains(&text) {
                                l.push(text);
                            }
                            l
                        });
                        if let Some(r) = r {
                            self.adopt(r);
                        }
                    }
                }
                _ => {
                    if let Some((b, p)) = edit(&self.input, self.pos, k) {
                        self.input = b;
                        self.pos = p;
                    }
                }
            }
            return Out::Draw;
        }
        let n = self.rules[self.tab].len();
        let cur = &mut self.cur[self.tab];
        *cur = (*cur).min(n.saturating_sub(1));
        if !self.loaded || self.files.is_empty() {
            // Nothing to change until the files are read.
            return if matches!(s, "esc" | "ctrl+c" | "q") {
                Out::Close
            } else {
                Out::Idle
            };
        }
        match s {
            "esc" | "ctrl+c" | "q" => return Out::Close,
            "]" | "right" => self.tab = (self.tab + 1) % 3,
            "[" | "left" => self.tab = (self.tab + 2) % 3,
            "up" | "k" => *cur = round_move(*cur, -1, n),
            "down" | "j" => *cur = round_move(*cur, 1, n),
            "a" | "+" | "enter" => {
                self.adding = true;
                self.err.clear();
            }
            "x" | "delete" | "backspace" => {
                let Some(r) = self.rules[self.tab].get(*cur).cloned() else {
                    return Out::Idle;
                };
                let (tab, files) = (self.tab, self.files.clone());
                m.ask(
                    Confirm::new(format!("Take out {}?", r.text), move |m| {
                        let text = r.text.clone();
                        // The question has closed: the sheet is back.
                        if let Some(r) =
                            change_rules(m, r.file.clone(), tab, files, move |mut l| {
                                l.retain(|x| *x != text);
                                l
                            })
                            && let Some(s) = m.sheet_as::<PermSheet>()
                        {
                            s.adopt(r);
                        }
                        Out::Draw
                    })
                    .detail(format!("from {}", self.rules[self.tab][*cur].file.label)),
                );
            }
            "ctrl+e" | "e" => {
                let path = self.rules[self.tab]
                    .get(*cur)
                    .map(|r| r.file.path.clone())
                    .unwrap_or_else(|| self.files[0].path.clone());
                return edit_file(m, &path);
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title(
                "Permissions",
                "what Claude may do without asking, must ask about, and may never do",
                w,
            ),
            Line::new(),
        ];
        if !self.loaded {
            out.extend([
                line([sp("  reading the settings files…", DIM)]),
                Line::new(),
                keys_fit(w, &[("esc", "close")]),
            ]);
            return out;
        }
        let names = [
            format!("Allow {}", self.rules[0].len()),
            format!("Ask {}", self.rules[1].len()),
            format!("Deny {}", self.rules[2].len()),
        ];
        out.push(sheet_tabs(&[&names[0], &names[1], &names[2]], self.tab));
        if !self.mode.is_empty() {
            out.push(line([sp(format!("  default mode: {}", self.mode), DIM)]));
        }
        out.push(Line::new());
        let list = &self.rules[self.tab];
        let cur = self.cur[self.tab].min(list.len().saturating_sub(1));
        let list_h = 3.max(h as isize - out.len() as isize - 6) as usize;
        let mut rows: Vec<Line> = list
            .iter()
            .enumerate()
            .map(|(i, r)| {
                sheet_row(
                    line([
                        sp(fit_str(&r.text, 20.max(w.saturating_sub(28))), TEXT),
                        plain("  "),
                        sp(r.file.label.clone(), DIM),
                    ]),
                    i == cur,
                    w,
                )
            })
            .collect();
        if rows.is_empty() {
            rows.push(line([sp(
                "  none · a adds one, like Bash(npm test:*), Read(./secrets/**) or WebFetch(domain:example.com)",
                DIM,
            )]));
        }
        let (from, to) = window(rows.len(), cur, list_h);
        out.extend(rows[from..to].iter().cloned());
        out.push(Line::new());
        if self.adding {
            let mut l = line([
                plain("  "),
                sp(format!("{} ❯ ", RULE_KINDS[self.tab]), ORANGE),
            ]);
            join(
                &mut l,
                text_field(
                    &self.input,
                    self.pos,
                    true,
                    "Bash(git status:*)",
                    w.saturating_sub(40),
                ),
            );
            l.spans.push(plain("  "));
            l.spans.push(sp("into ", DIM));
            l.spans.push(sp(
                self.files
                    .get(self.target)
                    .map(|f| f.label.clone())
                    .unwrap_or_default(),
                TEXT,
            ));
            l.spans.push(sp(" · tab", DIM));
            out.push(l);
        } else if !self.err.is_empty() {
            out.push(line([
                plain("  "),
                trunc(line([sp(self.err.clone(), RED)]), w.saturating_sub(4))
                    .spans
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| plain("")),
            ]));
        } else if let Some(r) = list.get(cur) {
            out.push(line([sp(format!("  {}", tildify(&r.file.path)), DIM)]));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[
                ("a", "add"),
                ("x", "take out"),
                ("[ ]", "allow/ask/deny"),
                ("e", "edit the file"),
                ("esc", "close"),
            ],
        ));
        out
    }
}

/// One command a hook event runs (hook).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hook {
    pub event: String,
    pub matcher: String,
    pub command: String,
    pub file: SettingsFile,
}

/// /hooks: every hook by event, from each settings file and plugin, with
/// the file it's in; enter opens that file (hookSheet).
#[derive(Default)]
pub struct HookSheet {
    pub hooks: Vec<Hook>,
    /// disableAllHooks.
    pub off: bool,
    pub user: String,
    pub cur: usize,
    /// The settings files are being read, off the UI.
    pub loading: bool,
}

/// A hooks block, by event, as each settings file and plugin writes it
/// (parseHooks).
pub fn parse_hooks(raw: &serde_json::Value, f: &SettingsFile) -> Vec<Hook> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct H {
        #[serde(rename = "type")]
        kind: String,
        command: String,
        prompt: String,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct G {
        matcher: String,
        hooks: Vec<H>,
    }
    let Ok(by_event) = serde_json::from_value::<BTreeMap<String, Vec<G>>>(raw.clone()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (ev, groups) in by_event {
        for g in groups {
            for h in g.hooks {
                out.push(Hook {
                    event: ev.clone(),
                    matcher: g.matcher.clone(),
                    command: first_non_empty(&[&h.command, &h.prompt, &h.kind]).to_string(),
                    file: f.clone(),
                });
            }
        }
    }
    out
}

/// Opens /hooks for agent k's session on p in cwd: the settings files'
/// hooks, then the enabled plugins', read-only (openHooks).
pub fn open_hooks(m: &mut Model, k: &str, p: &Profile, cwd: &str) {
    m.open_sheet(Box::new(HookSheet {
        loading: true,
        ..HookSheet::default()
    }));
    let files = settings_files(k, p, cwd);
    m.later(
        move || {
            let (mut hooks, mut off, mut user) = (Vec::new(), false, String::new());
            for (i, f) in files.iter().enumerate() {
                if i == 0 {
                    user = f.path.clone();
                }
                let Ok(s) = File::load(&f.path) else {
                    continue;
                };
                if s.get::<bool>("disableAllHooks") == Some(true) {
                    off = true;
                }
                if let Some(raw) = s.get::<serde_json::Value>("hooks") {
                    hooks.extend(parse_hooks(&raw, f));
                }
            }
            (hooks, off, user)
        },
        |m, (hooks, off, user)| {
            if let Some(hs) = m.sheet_as::<HookSheet>() {
                hs.loading = false;
                hs.off = off;
                hs.user = user;
                let more = std::mem::take(&mut hs.hooks); // the plugins' may have landed first
                hs.hooks = hooks;
                hs.hooks.extend(more);
                hs.sort();
            }
            Out::Draw
        },
    );
    // Enabled plugins' hooks come after, read-only.
    let Some(a) = reg().get(k) else {
        return;
    };
    if a.plugger().is_none() {
        return;
    }
    let (p, cwd) = (p.clone(), cwd.to_string());
    m.later(
        move || {
            let Some(plug) = a.plugger() else {
                return Vec::new();
            };
            let Ok((inst, _)) = plug.plugins(&p, &cwd) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for pl in inst.iter().filter(|p| p.enabled) {
                let path = std::path::Path::new(&pl.install_path)
                    .join("hooks")
                    .join("hooks.json");
                let Ok(b) = std::fs::read(&path) else {
                    continue;
                };
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) else {
                    continue;
                };
                if let Some(h) = v.get("hooks") {
                    out.extend(parse_hooks(
                        h,
                        &SettingsFile {
                            label: format!("plugin {}", pl.name),
                            path: path.to_string_lossy().into_owned(),
                        },
                    ));
                }
            }
            out
        },
        |m, more| {
            if let Some(hs) = m.sheet_as::<HookSheet>() {
                hs.hooks.extend(more);
                hs.sort();
            }
            Out::Draw
        },
    );
}

impl HookSheet {
    fn sort(&mut self) {
        self.hooks.sort_by(|a, b| a.event.cmp(&b.event));
    }
}

impl Sheet for HookSheet {
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let n = self.hooks.len();
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" => Out::Close,
            "up" | "k" => {
                self.cur = round_move(self.cur, -1, n);
                Out::Draw
            }
            "down" | "j" => {
                self.cur = round_move(self.cur, 1, n);
                Out::Draw
            }
            "enter" | "e" | "ctrl+e" => {
                let mut path = self.user.clone();
                if let Some(h) = self.hooks.get(self.cur) {
                    if h.file.label.starts_with("plugin ") {
                        m.flash_err(&format!(
                            "that hook is {}'s: /plugins turns the plugin off",
                            h.file.label
                        ));
                        return Out::Draw;
                    }
                    path = h.file.path.clone();
                }
                if path.is_empty() {
                    return Out::Idle;
                }
                edit_file(m, &path);
                Out::Close
            }
            _ => Out::Idle,
        }
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title(
                "Hooks",
                "commands Claude Code runs around tools, prompts and sessions",
                w,
            ),
            Line::new(),
        ];
        if self.off {
            out.push(line([sp(
                "  All hooks are off (disableAllHooks): Settings › Claude turns them back on.",
                YELLOW,
            )]));
            out.push(Line::new());
        }
        let list_h = 3.max(h as isize - out.len() as isize - 6) as usize;
        let mut rows = Vec::new();
        let (mut sel_at, mut last) = (0, None::<String>);
        for (i, hk) in self.hooks.iter().enumerate() {
            if last.as_deref() != Some(hk.event.as_str()) {
                rows.push(line([spb(format!("  {}", hk.event), SUB)]));
                last = Some(hk.event.clone());
            }
            if i == self.cur {
                sel_at = rows.len();
            }
            let matcher = if hk.matcher.is_empty() {
                "*"
            } else {
                &hk.matcher
            };
            rows.push(sheet_row(
                line([
                    sp(fit_str(matcher, 14), BLUE),
                    plain(" "),
                    sp(
                        fit_str(&one_line(&hk.command), 20.max(w.saturating_sub(44))),
                        TEXT,
                    ),
                    plain("  "),
                    sp(hk.file.label.clone(), DIM),
                ]),
                i == self.cur,
                w,
            ));
        }
        if rows.is_empty() {
            rows.push(line([sp(
                if self.loading {
                    "  reading the settings files…"
                } else {
                    "  no hooks · enter opens your settings.json to add one"
                },
                DIM,
            )]));
        }
        let (from, to) = window(rows.len(), sel_at, list_h);
        out.extend(rows[from..to].iter().cloned());
        out.push(Line::new());
        if let Some(hk) = self.hooks.get(self.cur) {
            out.push(line([
                plain("  "),
                sp(
                    trunc(line([plain(tildify(&hk.file.path))]), w.saturating_sub(4)).plain(),
                    DIM,
                ),
            ]));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[
                ("↑↓", "choose"),
                ("enter", "edit its file"),
                ("esc", "close"),
            ],
        ));
        out
    }
}
