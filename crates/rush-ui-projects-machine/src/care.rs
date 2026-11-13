//! Disk and memory: a cleanup agent, offered when they run low. Plain code
//! watches the free disk and the memory pressure; when either passes what
//! Settings › Disk and memory says, rush asks whether to start a cleanup
//! agent, and no agent runs until you say yes. Go: internal/ui/
//! machinecare.go (careReason :48, watchMachine :67, short :125,
//! careSheet :164, reads :179, andList :188, body :197, key :234, pick
//! :253, its start :313-322, careStart :348, careScanWait :356, autoMode
//! :360, careKnown :372, careReports :417, carePrompt :434) and
//! model.go:2024 (notifyLater).
//!
//! The session the cleanup agent runs in is started by whoever can start
//! hosted sessions: they set [`CareStarter`] (m.kept; session_core's
//! start::install sets [`start_with_hosts`]); until one does, yes says so
//! and starts nothing. It's started as rush's own: StartedBy the cleanup
//! plugin, with why in its meta, so the list says so and finds it again
//! after a reload. Once its turn is over and has settled, the watch marks
//! it done, stops its host and says so (watchMachine :80-115): only in
//! the rush that leads.

use rush_core::text::Line;
use rush_core::time::Ms;
use rush_plugin::{CleanupReport, Ctx, SERVICE_CLEANUP_REPORT, SERVICE_TIMEOUT};

use super::data::*;
use super::rows::*;
use crate::frame::sheet::{Sheet, sheet_row, sheet_title};
use crate::frame::{Out, PlaceId, Press};
use crate::model::Model;
use crate::style::*;

/// The free disk asked about by default, in GB.
pub const CARE_DISK_GB: i64 = 20;
/// The time under memory pressure asked about by default, in minutes.
pub const CARE_RAM_MINUTES: i64 = 15;
/// The least time between offers.
pub const CARE_SNOOZE: Ms = 6 * 3600 * 1000;
/// How long rush has run before it offers (Go's 120 ticks).
const CARE_WARMUP: Ms = 120_000;
/// How often the pressure is read (Go's every 5 ticks).
pub const CARE_EVERY: Ms = 5_000;
/// The plugin rush recommends turning on (bundled/cleanup).
pub const CARE_PLUGIN: &str = rush_core::adapter::STARTED_BY_CLEANUP;

/// A cleanup agent to start: its task, where, and its name. auto runs it
/// in its agent's auto mode where there is one (Go's, for nobody watching
/// to answer its prompts); rush's own start leaves it off, so it asks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CareStart {
    pub prompt: String,
    pub cwd: String,
    pub name: String,
    pub auto: bool,
    /// The plugin that starts it (cleanup): rush's own, as the list says,
    /// and found again after a reload.
    pub started_by: String,
    /// Why, in a few words, for its row (meta "why"): "18G free on disk".
    pub why: String,
    /// What it may do without asking, its session's alone (care_allow).
    pub allow: Vec<String>,
}

/// The read-only commands the cleanup agent runs without asking, as Claude
/// Code's rules: a prefix rule (`Bash(du:*)`) takes the command and its
/// arguments; one without `:*` only that command as written. Claude Code
/// matches each command of a chain (&&, ;, |) on its own, so `du x && rm
/// y` still asks for the rm. Only commands with no form that deletes or
/// writes: no find (-delete, -exec), no git log (--output), git branch
/// only in its listing forms (never -d/-D). Rust-only.
pub const CARE_READS: [&str; 13] = [
    "Bash(du:*)",
    "Bash(df:*)",
    "Bash(ls:*)",
    "Bash(stat:*)",
    "Bash(git status:*)",
    "Bash(git worktree list:*)",
    "Bash(git rev-list:*)",
    "Bash(git branch)",
    "Bash(git branch --list)",
    "Bash(git branch -a)",
    "Bash(git branch -v)",
    "Bash(git branch -vv)",
    "Bash(xcrun simctl list:*)",
];

/// What the cleanup session alone may do without asking: CARE_READS, and
/// write its plan and script in its own scratch folder ("$TMPDIR", which
/// its host makes the folder: `//` is Claude Code's absolute path).
/// Running the script, rm, git worktree remove, pnpm store prune and
/// simctl delete always ask. Never written to anyone's settings: it goes
/// as the session's --allowedTools. Rust-only.
pub fn care_allow() -> Vec<String> {
    let mut v: Vec<String> = CARE_READS.iter().map(|r| r.to_string()).collect();
    v.extend(["Write(/$TMPDIR/**)".into(), "Edit(/$TMPDIR/**)".into()]);
    v
}

/// How long Start waits on rush's look at the worktrees and the plugins'
/// reports before the agent starts with what rush has.
const CARE_SCAN_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Starts a hosted session for the cleanup agent. Set by the group that
/// runs hosted sessions; None says it isn't here yet.
#[derive(Default)]
pub struct CareStarter(pub Option<StartCare>);

/// Starts the cleanup agent's session off the UI thread, then hands its
/// id, or why not, to [`care_started`].
pub type StartCare = fn(&mut Model, CareStart) -> Out;

/// What a started cleanup agent has before its end counts: a turn that
/// ends at once is still starting.
pub const CARE_SETTLE: Ms = 2 * 60_000;

/// What the watch remembers.
#[derive(Default)]
pub struct Care {
    /// When the memory came under pressure; 0 while it isn't.
    pub pressed_since: Ms,
    /// When rush last offered.
    pub asked: Ms,
    /// When the pressure was last read, and whether a read is out.
    pub read_at: Ms,
    pub reading: bool,
    /// When this rush first read the list.
    pub started: Ms,
    /// Cleanup sessions this rush started, by id, and when.
    pub agents: Vec<(String, Ms)>,
    /// A cleanup agent is at work, or just finished: nothing is offered.
    pub busy: bool,
}

/// The cleanup agents whose turn is over and has settled, each taken off
/// the watch, and whether another is still at work. One started before a
/// reload and still at it is watched from now; one finished before it, or
/// brought back by you, is yours (watchMachine :80-99).
pub fn finished_care(m: &mut Model, now: Ms) -> (Vec<rush_core::agent::Agent>, bool) {
    let (mut done, mut busy) = (Vec::new(), false);
    let agents: Vec<rush_core::agent::Agent> = m
        .agents
        .iter()
        .filter(|a| a.rush && a.started_by == CARE_PLUGIN && !a.done)
        .cloned()
        .collect();
    let c = m.kept::<Care>();
    for a in agents {
        let finished = a.job.state == rush_core::agent::State::Done;
        let at = match c.agents.iter().find(|(id, _)| *id == a.job.id) {
            Some(&(_, at)) => at,
            None if finished => continue,
            None => {
                c.agents.push((a.job.id.clone(), now));
                now
            }
        };
        if !finished || now - at < CARE_SETTLE {
            busy = true;
            continue;
        }
        c.agents.retain(|(id, _)| *id != a.job.id);
        done.push(a);
    }
    (done, busy)
}

/// Each finished cleanup agent marked done, its host stopped (its report
/// stays, and a message resumes it), and said (watchMachine :100-115).
pub fn end_care(m: &mut Model, done: Vec<rush_core::agent::Agent>) {
    for a in done {
        if rush_core::dryrun::would(
            "stop-cleanup-agent",
            &a.key,
            "the machine cleanup agent finished: put away in Earlier, its host stopped",
        ) {
            continue;
        }
        m.act(crate::frame::ext::Act::ToggleDone(a.key.clone()));
        m.news("machine cleanup finished: its report is in Earlier", "");
        notify_later("Cleanup finished", "its report is in rush's Earlier", true);
        let h = crate::host::hosts(m);
        let id = a.job.id.clone();
        m.later(
            move || h.stop(&id),
            |m, r| {
                if let Err(e) = r {
                    m.fail(&e);
                }
                Out::Draw
            },
        );
    }
}

/// Whether rush's menu bar app is running, which says what rush would:
/// it holds its lock (menubar.Running).
fn menubar_running() -> bool {
    use std::os::fd::AsRawFd;
    let lock = rush_state::dir().join("menubar").join("feed.lock");
    let Ok(f) = std::fs::File::open(lock) else {
        return false;
    };
    // SAFETY: flock on a file this owns, open for the call.
    unsafe {
        if libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 {
            return true;
        }
        libc::flock(f.as_raw_fd(), libc::LOCK_UN);
    }
    false
}

/// A notification, off the UI thread; unless_menu_bar leaves it to the
/// menu bar when that runs (notifyLater). Not from tests.
fn notify_later(title: &'static str, body: &'static str, unless_menu_bar: bool) {
    if rush_core::sandbox::active() {
        return;
    }
    std::thread::spawn(move || {
        let by = if unless_menu_bar && menubar_running() {
            "menu bar"
        } else {
            rush_sys::actions::notify(title, body);
            "rush"
        };
        let mut f = serde_json::Map::new();
        f.insert("title".into(), title.into());
        f.insert("body".into(), body.into());
        f.insert("by".into(), by.into());
        rush_events::log("notify", f);
    });
}

/// Why a cleanup is worth offering, in words, or "" when it isn't: free
/// disk under disk_gb (0 the default, -1 never), or the memory under
/// pressure for longer than ram_min minutes (the same).
pub fn care_reason(free: u64, total: u64, disk_gb: i64, pressed: Ms, ram_min: i64) -> String {
    let disk_gb = if disk_gb == 0 { CARE_DISK_GB } else { disk_gb };
    let ram_min = if ram_min == 0 {
        CARE_RAM_MINUTES
    } else {
        ram_min
    };
    if disk_gb > 0 && total > 0 && free < (disk_gb as u64) << 30 {
        return format!(
            "the disk has {} free, under the {disk_gb}G you set",
            disk(free as i64)
        );
    }
    if ram_min > 0 && pressed >= ram_min * 60_000 {
        return format!(
            "the memory has been under pressure for {} minutes",
            pressed / 60_000
        );
    }
    String::new()
}

/// why in a few words, for the cleanup agent's row: the free disk when the
/// disk is why, else the memory (machinecare.go:125).
pub fn care_short(why: &str, free: u64) -> String {
    if why.starts_with("the disk") {
        return format!("{} free on disk", disk(free as i64));
    }
    "memory under pressure".into()
}

/// "a", "a and b", "a, b and c".
pub fn and_list(l: &[String]) -> String {
    match l {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Whether nothing else is on the screen: the list showing, no sheet, no
/// question, no conversation open.
fn screen_free(m: &Model) -> bool {
    m.sheet.is_none() && m.confirm.is_none() && m.place == PlaceId::Agents && m.session.is_none()
}

/// The watch, with each reading of the list: it follows the memory
/// pressure and offers a cleanup once the screen is free. What it reads
/// (the pressure, the disk, whether the plugin is on) is read off the UI.
/// Only the rush that leads watches (model.go:1100): one that follows
/// never offers the cleanup agent that tidies worktrees.
pub fn watch_machine(m: &mut Model) {
    if !m.leads() {
        return;
    }
    let now = m.now;
    // Ending one stops its host: only with a loop running, never from a
    // frame drawn once (--render).
    if m.sender().is_some() {
        let (done, busy) = finished_care(m, now);
        let ended = !done.is_empty();
        end_care(m, done);
        m.kept::<Care>().busy = busy || ended;
    }
    let st = m.kept::<Care>();
    if st.started == 0 {
        st.started = now;
    }
    // Read while a cleanup agent is at work too, for the dire watch (its
    // plan may wait on you); the offer still holds off (below).
    if st.reading || now - st.read_at < CARE_EVERY || data(m).frozen || m.sender().is_none() {
        return;
    }
    let st = m.kept::<Care>();
    st.reading = true;
    st.read_at = now;
    m.later(
        || {
            let pressed =
                rush_gate::guard::pressure(&rush_gate::Real, 0) >= rush_gate::share::PRESSURE_WARN;
            (pressed, rush_sys::sysinfo::read_disk())
        },
        |m, (pressed, d)| {
            let now = m.now;
            let c = m.kept::<Care>();
            c.reading = false;
            if !pressed {
                c.pressed_since = 0;
            } else if c.pressed_since == 0 {
                c.pressed_since = now;
            }
            super::dire::watch_dire(m, d.free, d.total);
            let c = m.kept::<Care>();
            if c.busy
                || now - c.started < CARE_WARMUP
                || (c.asked > 0 && now - c.asked < CARE_SNOOZE)
            {
                return Out::Idle;
            }
            let pressed = if c.pressed_since > 0 {
                now - c.pressed_since
            } else {
                0
            };
            let cfg = &data(m).cfg;
            let why = care_reason(
                d.free,
                d.total,
                cfg.cleanup_disk_gb,
                pressed,
                cfg.cleanup_ram_minutes,
            );
            let short = care_short(&why, d.free);
            if why.is_empty() || !screen_free(m) {
                return Out::Idle;
            }
            m.kept::<Care>().asked = now;
            offer(m, why, short);
            Out::Idle
        },
    );
}

/// Reads whether the cleanup plugin is on and who reports to it, then
/// asks: unasked, so the cursor starts on Not now and an enter meant for
/// the list doesn't start anything.
fn offer(m: &mut Model, why: String, short: String) {
    m.later(
        || {
            (
                rush_plugin::bundled_on(CARE_PLUGIN),
                rush_plugin::providers(SERVICE_CLEANUP_REPORT),
            )
        },
        move |m, (on, from)| {
            if !screen_free(m) {
                return Out::Idle;
            }
            let (title, body) = if on {
                (
                    format!("rush: {why}"),
                    "Start a cleanup agent? rush is asking in its view.",
                )
            } else {
                (
                    format!("rush: {why}"),
                    "Turn on the cleanup plugin? rush is asking in its view.",
                )
            };
            let sheet = CareSheet {
                why,
                short,
                cur: 0,
                busy: false,
                looking: false,
                off: !on,
                from,
                asked: false,
            };
            m.open_sheet(Box::new(CareSheet {
                cur: sheet.not_now(),
                ..sheet
            }));
            m.later(
                move || rush_sys::actions::notify(&title, body),
                |_, ()| Out::Idle,
            );
            // The agent is told what's there, fresh.
            if m.now - data(m).checked > 30 * 60_000 {
                super::scan::scan_worktrees(m);
            }
            Out::Draw
        },
    );
}

/// Start a cleanup agent, or Not now; with the cleanup plugin off, it
/// recommends turning it on first.
pub struct CareSheet {
    pub why: String,
    /// why in a few words, for the agent's row: "18G free on disk".
    pub short: String,
    /// 0 Start (or Turn on), 1 Not now.
    pub cur: usize,
    pub busy: bool,
    /// rush checking the worktrees before it starts the agent.
    pub looking: bool,
    /// The cleanup plugin is off: the sheet offers turning it on.
    pub off: bool,
    /// The plugins that report to the cleanup agent.
    pub from: Vec<String>,
    /// You asked for it (#cleanup), not rush: nothing is snoozed by Not
    /// now, and the cursor starts on Start. Beyond Go.
    pub asked: bool,
}

/// What each choice on the cleanup sheet does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CareChoice {
    /// Open Clean up, rush's own list by its rules: no agent (Rust-only).
    CleanUp,
    TurnOn,
    Start,
    /// Start one that also looks at the rest of the disk (Beyond Go).
    Further,
    NotNow,
}

impl CareSheet {
    /// The choices, top to bottom: Clean up first, rush's own list that
    /// needs no agent; then Turn on and Not now while the plugin is off,
    /// else Start, Start and look further, and Not now.
    pub fn choices(&self) -> &'static [CareChoice] {
        use CareChoice::*;
        if self.off {
            &[CleanUp, TurnOn, NotNow]
        } else {
            &[CleanUp, Start, Further, NotNow]
        }
    }

    /// Where a choice is, when it's offered.
    pub fn at(&self, c: CareChoice) -> Option<usize> {
        self.choices().iter().position(|x| *x == c)
    }

    /// Where Not now is: the last choice.
    pub fn not_now(&self) -> usize {
        self.choices().len() - 1
    }

    /// What the cleanup agent reads, in words.
    fn reads(&self) -> String {
        let what = "your worktrees, rush's scratch folders, what agents left in /tmp".to_string();
        if self.from.is_empty() {
            return what;
        }
        format!("{what} and what {} report", and_list(&self.from))
    }

    fn pick(&mut self, m: &mut Model) -> Out {
        let hours = CARE_SNOOZE / 3_600_000;
        let choice = self.choices()[self.cur.min(self.not_now())];
        if choice == CareChoice::NotNow {
            if self.asked {
                m.flash("no cleanup");
            } else {
                m.flash(&format!("no cleanup · asks again in {hours} hours"));
            }
            return Out::Close;
        }
        if choice == CareChoice::CleanUp {
            // rush's own list, by its rules: what it knows can go, ticked,
            // and the caches it knows of measured; nothing goes until you
            // confirm it there.
            super::clean::open_clean_sheet(m);
            return Out::Close;
        }
        let further = choice == CareChoice::Further;
        self.busy = true;
        if self.off {
            m.later(
                || {
                    rush_plugin::set_bundled(CARE_PLUGIN, true)?;
                    rush_plugin::client::reload()
                },
                |m, r| {
                    let Some(s) = m.sheet_as::<CareSheet>() else {
                        return Out::Idle;
                    };
                    s.busy = false;
                    if let Err(e) = r {
                        m.sheet = None;
                        m.fail(&format!("couldn't turn on the cleanup plugin: {e}"));
                        return Out::Draw;
                    }
                    s.off = false;
                    // Unasked, the cursor stays off Start, so an enter
                    // meant for something else starts nothing.
                    s.cur = if s.asked {
                        s.at(CareChoice::Start).unwrap_or(0)
                    } else {
                        s.not_now()
                    };
                    m.flash("cleanup plugin on");
                    Out::Draw
                },
            );
            return Out::Draw;
        }
        // The agent is told what rush's own check of the worktrees says,
        // which knows a squash-merged branch by its pull request; git alone
        // doesn't, and an agent left to look for itself spends its time
        // finding them.
        let checked = data(m).checked;
        let scan = if checked == 0 || rush_core::time::now() - checked > 30 * 60_000 {
            self.looking = true;
            Some(super::scan::worktree_scan(m, None, false))
        } else {
            None
        };
        let why = self.why.clone();
        let short = self.short.clone();
        // What rush's own rules decide, measured beside the rest: the agent
        // is told, and looks only at what they can't.
        let (wts, free) = super::clean::survey_paths(m);
        let (now, frozen) = (m.now, data(m).frozen);
        m.later(
            move || {
                let survey = (!frozen).then(|| {
                    std::thread::spawn(move || super::clean::survey_all(&wts, &free, now))
                });
                let st = care_start(scan, care_reports, CARE_SCAN_WAIT);
                (st, survey.and_then(|h| h.join().ok()))
            },
            move |m, (st, survey)| {
                if let Some((s, never)) = survey {
                    let d = m.kept::<Data>();
                    d.wt_facts.extend(s.facts);
                    d.survey = s.caches;
                    d.never = never;
                }
                if let Some(late) = st.late {
                    // The scan lands when it's done, whatever became of the sheet.
                    m.later(
                        move || late.recv().ok(),
                        |m, f| {
                            let Some(f) = f else { return Out::Idle };
                            super::scan::on_worktrees(m, f);
                            Out::Draw
                        },
                    );
                }
                if m.sheet_as::<CareSheet>().is_none() {
                    return Out::Idle;
                }
                m.sheet = None;
                if let Some(f) = st.scan {
                    super::scan::on_worktrees(m, f);
                }
                let known = care_known(m);
                start_care(m, &why, &short, &known, &st.reports, further)
            },
        );
        Out::Draw
    }
}

/// What a cleanup agent is started with: a fresh look at the worktrees
/// when rush's is old, and the plugins' reports (machinecare.go:348).
pub struct CareBegin<F> {
    pub scan: Option<F>,
    /// The scan, still running past the wait.
    pub late: Option<std::sync::mpsc::Receiver<F>>,
    pub reports: Vec<CleanupReport>,
}

/// Runs the scan (when there is one) and asks for the reports at once,
/// off the UI. The scan measures every worktree's size at a gentle pace,
/// which takes minutes on a full disk, and a plugin may be slow to report:
/// past wait the agent starts with what rush has, and the scan lands when
/// it's done.
pub fn care_start<F, S, R>(scan: Option<S>, reports: R, wait: std::time::Duration) -> CareBegin<F>
where
    F: Send + 'static,
    S: FnOnce() -> F + Send + 'static,
    R: FnOnce() -> Vec<CleanupReport> + Send + 'static,
{
    use std::sync::mpsc;
    let deadline = std::time::Instant::now() + wait;
    let mut st = CareBegin {
        scan: None,
        late: None,
        reports: Vec::new(),
    };
    // The scan has a channel of its own, so one past the wait is still
    // there to land; the reports go into one that's dropped with this.
    let (rtx, rrx) = mpsc::sync_channel(1);
    let mut out = 1;
    if let Some(scan) = scan {
        let (tx, rx) = mpsc::sync_channel(1);
        let wake = rtx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scan());
            let _ = wake.send(None);
        });
        st.late = Some(rx);
        out += 1;
    }
    std::thread::spawn(move || {
        let _ = rtx.send(Some(reports()));
    });
    while out > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rrx.recv_timeout(left) {
            Ok(Some(r)) => st.reports = r,
            Ok(None) => {
                st.scan = st.late.take().and_then(|rx| rx.recv().ok());
            }
            Err(_) => break,
        }
        out -= 1;
    }
    st
}

/// Starts the cleanup agent, as rush's own.
pub fn start_care(
    m: &mut Model,
    why: &str,
    short: &str,
    known: &[String],
    reports: &[CleanupReport],
    further: bool,
) -> Out {
    let start = CareStart {
        prompt: care_prompt(why, known, reports, further),
        cwd: std::env::var("HOME").unwrap_or_default(),
        name: "Cleanup".into(),
        // Beyond Go, which started it in auto mode: it asks before every
        // deletion, and deletes only what you approved.
        auto: false,
        // rush's own: the list says so, and finds it again after a reload.
        started_by: CARE_PLUGIN.into(),
        why: short.into(),
        allow: care_allow(),
    };
    let Some(f) = m.kept::<CareStarter>().0 else {
        m.flash_err("starting a cleanup agent isn't in this rush yet");
        return Out::Draw;
    };
    f(m, start)
}

/// A cleanup agent's session started, or why not: watched from now, so
/// its end is noticed (machinecare.go:322).
pub fn care_started(m: &mut Model, r: Result<String, String>) -> Out {
    match r {
        Ok(id) => {
            let now = m.now;
            m.kept::<Care>().agents.push((id, now));
        }
        Err(e) => m.fail(&format!("couldn't start a cleanup agent: {e}")),
    }
    Out::Draw
}

/// The mode of an agent's modes that asks, for a cleanup agent that
/// mustn't delete on its own (rush_core's asking_mode). Rust-only.
pub use rush_core::adapter::asking_mode;

/// The starter session_core sets once hosts can start sessions: the
/// cleanup agent in c.cwd as the next session starts there, named
/// Cleanup, as rush's own (StartedBy, Meta's why) (pick's startHosted
/// :313-322, autoMode :360). Beyond Go: unless c.auto, in the mode of its
/// agent's that asks (asking_mode), never the one new sessions start in,
/// so nothing is deleted without you.
pub fn start_with_hosts(m: &mut Model, c: CareStart) -> Out {
    use crate::host::hosts;
    let calls = &m.ext.calls;
    let (next_start, start_profile, config_as) =
        (calls.next_start(), calls.start_profile(), calls.config_as());
    let next = next_start(m, &c.cwd);
    let mut s = config_as(m, &next, &c.cwd);
    s.prompt = c.prompt;
    s.name = c.name;
    s.started_by = c.started_by;
    s.why = c.why;
    s.allow = c.allow;
    s.profile = if next.profile.is_empty() {
        start_profile(m, &c.cwd).name
    } else {
        next.profile.clone()
    };
    let modes = m
        .reg
        .choices_of(&s.kind)
        .map(|ch| ch.modes)
        .unwrap_or_default();
    if c.auto && modes.iter().any(|x| x.id == "auto") {
        s.permission_mode = "auto".into();
    } else if !c.auto {
        // Never the mode new sessions start in, which may trust more.
        s.permission_mode = asking_mode(&modes);
    }
    // rush's own that ask first (the cleanup agent) keep to asking, as
    // their host keeps them.
    s.permission_mode = rush_core::adapter::mode_for(&s.started_by, &s.permission_mode, &modes);
    let h = hosts(m);
    m.later(move || h.start(&s).map(|st| st.id), care_started);
    Out::Draw
}

impl Sheet for CareSheet {
    fn width(&self, _m: &Model) -> usize {
        100
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![sheet_title("Cleanup", &self.why, w), Line::new()];
        let reads = self.reads();
        let (text, first) = if self.off {
            (
                format!(
                    "rush noticed that {}. The cleanup plugin is off, and recommended now: with it on, rush offers a cleanup agent \
                     when this happens, which looks at {reads} and proposes a list of what can go without losing work for you to approve. \
                     Nothing starts until you say yes. Clean up now needs no plugin: it's rush's own list, by its rules.",
                    self.why
                ),
                (
                    "Turn on the cleanup plugin",
                    "recommended · then asks whether to start a cleanup agent now",
                ),
            )
        } else {
            let noticed = if self.asked {
                String::new()
            } else {
                format!("rush noticed that {}. ", self.why)
            };
            (
                format!(
                    "{noticed}Clean up now opens rush's own list, with no agent: worktrees that can go, done sessions' scratch, \
                     and caches that rebuild themselves (ones rush knows, and ones cleanup agents' plans taught it), ticked for you to confirm. \
                     A cleanup agent can look at {reads}, \
                     deleting nothing, and propose a list of what can go without losing work (a worktree only once it's committed and pushed) \
                     and what holds memory that nobody works in, with each path, its size and why, for you to approve: all, some or none. \
                     It removes only what you approve, in one command you see first, and tells you what it freed. \
                     Told to look further, it also hunts the rest of the disk for caches that rebuild themselves, \
                     and lists the big things it leaves for you. It's marked done when it finishes, which stops it.",
                ),
                (
                    "Start a cleanup agent",
                    "asks before it deletes anything, with the agent and model new sessions start with",
                ),
            )
        };
        for l in wrap(&text, w.saturating_sub(2)) {
            out.push(ln([plain(format!("  {l}"))]));
        }
        out.push(Line::new());
        let later = if self.asked {
            "starts nothing".to_string()
        } else {
            format!(
                "asks again in {} hours if it's still so",
                CARE_SNOOZE / 3_600_000
            )
        };
        for (i, c) in self.choices().iter().enumerate() {
            let (what, about) = match c {
                CareChoice::CleanUp => (
                    "Clean up now",
                    "rush's own list, no agent: what it knows can go and caches that rebuild, ticked",
                ),
                CareChoice::TurnOn | CareChoice::Start => first,
                CareChoice::Further => (
                    "Start one that looks further",
                    "also hunts caches across the disk, and lists what's big",
                ),
                CareChoice::NotNow => ("Not now", later.as_str()),
            };
            let l = ln([spb(what, TEXT), plain("  "), sp(about, DIM)]);
            out.push(sheet_row(l, i == self.cur, w));
        }
        let k = match self.busy {
            true => {
                let mut doing = Vec::new();
                if self.looking {
                    doing.push("looking at your worktrees".to_string());
                }
                if !self.from.is_empty() {
                    doing.push(format!("asking {} for their reports", and_list(&self.from)));
                }
                let doing = match and_list(&doing) {
                    d if d.is_empty() => "starting".to_string(),
                    d => d,
                };
                ln([sp(format!("⋯ {doing}"), YELLOW)])
            }
            false => keys_fit(
                w,
                &[("↑↓", "choose"), ("enter", "pick"), ("esc", "not now")],
            ),
        };
        out.push(Line::new());
        out.push(k);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if self.busy {
            return Out::Idle;
        }
        let n = self.choices().len();
        match k.s.as_str() {
            "down" | "tab" => {
                self.cur = (self.cur + 1) % n;
                Out::Draw
            }
            "up" | "shift+tab" => {
                self.cur = (self.cur + n - 1) % n;
                Out::Draw
            }
            "c" => {
                self.cur = 0;
                self.pick(m)
            }
            "y" => {
                self.cur = 1;
                self.pick(m)
            }
            "f" if !self.off => {
                self.cur = self.at(CareChoice::Further).unwrap_or(n - 1);
                self.pick(m)
            }
            "n" | "esc" | "ctrl+c" => {
                self.cur = n - 1;
                self.pick(m)
            }
            "enter" => self.pick(m),
            _ => Out::Idle,
        }
    }
}

/// What rush knows of the disk, the memory, worktrees and what agents
/// left, as lines for the cleanup agent. It reads nothing: the disk and
/// memory as last read by the place, and the worktrees as Clean up's scan
/// left them.
pub fn care_known(m: &Model) -> Vec<String> {
    let d = rush_sys::sysinfo::now().1;
    let mut out = vec![format!(
        "Disk: {} free of {}.",
        disk(d.free as i64),
        disk(d.total as i64)
    )];
    let lvl = rush_gate::Machine::memory_level(&rush_gate::Real);
    care_known_with(m, &mut out, lvl);
    out
}

/// care_known past the disk: the memory level given (-1 unknown).
pub fn care_known_with(m: &Model, out: &mut Vec<String>, mem_level: i32) {
    if mem_level >= 0 {
        out.push(format!("Memory: {mem_level}% available."));
    }
    let st = data(m);
    if st.checked == 0 {
        out.push(
            "Worktrees: rush hasn't looked yet; find them with git worktree list in the repositories under ~/Source."
                .into(),
        );
        scratch_known(m, out);
        return;
    }
    out.push(String::new());
    out.push(format!(
        "Worktrees, as rush checked them {} (path · branch · size · group (why)), grouped by rush's rules: SAFE goes whole and loses nothing; NOISE-ONLY too, all that's uncommitted is local settings or a lockfile; CACHE-ONLY holds work, so it stays, but nobody touched it for a day: its build caches can go; KEEP stays as it is:",
        ago(m.now - st.checked)
    ));
    for w in &st.wts {
        let t = w.triage(st.wt_facts.get(&w.path).copied(), m.now);
        let mut l = format!(
            "- {} · {} · {} · {}",
            w.path,
            w.branch,
            disk(w.size),
            t.class.as_str()
        );
        if !t.reason.is_empty() {
            l += &format!(" ({})", t.reason);
        }
        if running(m, &w.agents).is_some() {
            l += " · an agent is working in it now";
        }
        out.push(l);
    }
    rules_known(m, out);
    if st.tmp.stale_items > 0 {
        out.push(String::new());
        out.push(format!(
            "/tmp: {} of your items untouched for days, {}.",
            st.tmp.stale_items,
            disk(st.tmp.stale)
        ));
    }
    scratch_known(m, out);
}

/// How long ago, in words, in days past one: "3h ago", "4 days ago".
fn touched(d: Ms) -> String {
    if d >= 24 * 3_600_000 {
        format!("{} ago", super::clean::untouched_for(d))
    } else {
        ago(d)
    }
}

/// A session's scratch folder this big or more is named in the cleanup
/// agent's brief.
pub const CARE_SCRATCH_SHOWN: i64 = 100 << 20;

/// Rush's own scratch, for the cleanup agent (Rust-only): sessions'
/// scratch folders of CARE_SCRATCH_SHOWN or more, biggest first, and
/// what's in the scratch root that no session owns.
fn scratch_known(m: &Model, out: &mut Vec<String>) {
    let st = data(m);
    let mut rows: Vec<(i64, String)> = Vec::new();
    for g in agents(m).filter(|g| g.a.rush && !g.a.job.id.is_empty()) {
        let size = st.temp_of(&g.a.key);
        if size < CARE_SCRATCH_SHOWN {
            continue;
        }
        let state = if g.a.pid != 0 || g.busy() {
            "running"
        } else if m.group_of.get(&g.a.key).map(String::as_str) == Some(crate::list::FINISHED) {
            "done"
        } else {
            "stopped"
        };
        let name = one_line(&g.a.display_name);
        // Rush's own folder is a session's first (rush_fleet::temp_dirs).
        let path = st
            .temp
            .get(&g.a.key)
            .and_then(|t| t.dirs.first())
            .map(|d| d.path.clone())
            .unwrap_or_else(|| rush_fleet::temp_dir(&g.a.job.id));
        rows.push((
            size,
            format!(
                "- {path} · {} · {} · {state} · last touched {}",
                if name.is_empty() { &g.a.job.id } else { &name },
                disk(size),
                touched(untouched(st, &g, m.now)),
            ),
        ));
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    if !rows.is_empty() {
        out.push(String::new());
        out.push(format!(
            "Sessions' scratch folders (their $TMPDIR) of {} or more (path · session · size · done/stopped/running · last touched):",
            disk(CARE_SCRATCH_SHOWN)
        ));
        out.extend(rows.into_iter().map(|r| r.1));
    }
    let live = running_ids(m);
    let orphans: Vec<&rush_fleet::TempOrphan> =
        st.scratch_orphans.iter().filter(|o| !o.gone).collect();
    if orphans.is_empty() {
        return;
    }
    out.push(String::new());
    out.push(
        "In rush's scratch folders but no session's (path · size · last touched · the session it probably came from):"
            .into(),
    );
    for o in orphans {
        let mut l = format!(
            "- {} · {} · last touched {}",
            o.path,
            disk(o.size),
            touched(m.now - o.newest)
        );
        if !o.session.is_empty() {
            l += &format!(" · from {}", session_name(m, &o.session));
        }
        if let Some(id) = o.used_by(&live) {
            l += &format!(" · {} is running: keep it", session_name(m, id));
        }
        out.push(l);
    }
}

/// What rush's own rules found when Clean up last measured (Rust-only):
/// what they say goes (rush's Clean up removes it without an agent), what
/// they hold back and why, the commands every run ends with, and the
/// biggest of what rush never touches. The agent is told to leave the
/// first to rush and spend its time on what the rules can't decide.
pub fn rules_known(m: &Model, out: &mut Vec<String>) {
    let st = data(m);
    if st.survey.is_empty() && st.never.is_empty() {
        return;
    }
    let line = |c: &rush_fleet::CacheFound| {
        let mut l = format!(
            "- {} · {} · {}",
            c.found.path,
            disk(c.found.size),
            c.found.why
        );
        if !c.found.note.is_empty() {
            l += &format!(" ({})", c.found.note);
        }
        l
    };
    let (cmds, caches): (Vec<&rush_fleet::CacheFound>, Vec<&rush_fleet::CacheFound>) = st
        .survey
        .iter()
        .partition(|c| matches!(c.found.how, rush_fleet::How::Run(_)));
    let yes: Vec<String> = caches
        .iter()
        .filter(|c| c.found.ok)
        .map(|c| line(c))
        .collect();
    let not: Vec<String> = caches
        .iter()
        .filter(|c| !c.found.ok)
        .map(|c| line(c))
        .collect();
    if !yes.is_empty() {
        out.push(String::new());
        out.push("rush's rules say these go (path · size · what): rush's own Clean up removes them, with no agent; leave them out of your plan:".into());
        out.extend(yes);
    }
    if !not.is_empty() {
        out.push(String::new());
        out.push("rush's rules hold these back for now (path · size · what (why)): leave them too unless you find more than the rules know:".into());
        out.extend(not);
    }
    if !cmds.is_empty() {
        out.push(String::new());
        out.push(format!(
            "rush's Clean up ends every run with {}: don't run them yourself.",
            and_list(
                &cmds
                    .iter()
                    .map(|c| c.found.path.clone())
                    .collect::<Vec<_>>()
            )
        ));
    }
    if !st.never.is_empty() {
        out.push(String::new());
        out.push("rush never touches these, and neither do you: never in a script, only in your leave list for the user to decide on (path · size · what · last changed):".into());
        for f in &st.never {
            out.push(format!(
                "- {} · {} · {} · {}",
                f.path,
                disk(f.size),
                f.why,
                touched(m.now - f.newest)
            ));
        }
    }
}

/// How long ago, in words (agentviews.go:891).
pub fn ago(d: Ms) -> String {
    match d {
        d if d < 10_000 => "just now".into(),
        d if d < 60_000 => format!("{}s ago", d / 1000),
        d if d < 3_600_000 => format!("{}m ago", d / 60_000),
        d => format!("{}h ago", d / 3_600_000),
    }
}

/// Asks every plugin that provides cleanup.report for what it knows; one
/// that can't answer is left out. Off the UI.
fn care_reports() -> Vec<CleanupReport> {
    let ctx = Ctx::background().with_timeout(SERVICE_TIMEOUT + std::time::Duration::from_secs(5));
    let answers = rush_plugin::ask_service(&ctx, SERVICE_CLEANUP_REPORT).unwrap_or_default();
    answers
        .into_iter()
        .filter(|a| a.error.is_empty())
        .filter_map(|a| {
            let raw = a.result.as_ref()?;
            let mut r: CleanupReport = serde_json::from_str(raw.get()).ok()?;
            if r.report.trim().is_empty() {
                return None;
            }
            if r.title.is_empty() {
                r.title = a.plugin.clone();
            }
            Some(r)
        })
        .collect()
}

/// The cleanup agent's task.
pub fn care_prompt(
    why: &str,
    known: &[String],
    reports: &[CleanupReport],
    further: bool,
) -> String {
    let mut b = String::new();
    b += &format!(
        "Free disk space and memory on this machine, but only what the user approves. rush started you because {why}. "
    );
    b += "Never delete, remove or stop anything on your own: you work in two phases, and nothing goes before the user answers.\n\n";
    b += CARE_PHASES;
    b += "\nWhat may be in your plan:
- A worktree only when it's SAFE or NOISE-ONLY below, or git shows nothing uncommitted, unpushed or unmerged in it. SAFE is safe even when git calls its branch unmerged: rush found its pull request merged (a squash merge). A CACHE-ONLY one stays; only its build caches may go. It goes with git worktree remove, its branch kept. Never one an agent is working in, or a repository's main checkout.
- What agents left in /tmp or outside their projects only when it's marked safe, or nothing has touched it for days and nothing running uses it.
- Rush's scratch folders (each session's $TMPDIR, listed below): a done session's; a stopped one's only once nothing has touched it for days, as it may be resumed; a running one's never. What's in rush's scratch folders but no session's once nothing has touched it for days and no running session uses it (none whose id starts its name). What's in them, never the folders that hold them: rush's tmp folder and each project's folder in it.
- Build caches that rebuild themselves (node_modules, .next, .turbo, target, dist) from worktrees nobody works in.
- pnpm's node_modules are mostly hard links into its shared store, so removing them frees little until pnpm store prune runs: when the plan has any, it ends with pnpm store prune, once.
- Never databases, ~/.claude, transcripts, anything in a project that isn't a cache or a worktree, or anything you're unsure of: those go in the list you leave, with why.
- Never anything in ~/.colima, ~/.codex/sessions, ~/.claude, ~/Library/Application Support, ~/Library/Containers, ~/Library/Group Containers, ~/Pictures or ~/.ollama: rush never touches them. List the biggest in your leave list, each with its size, for the user to decide on.
- What rush's rules decide is listed below, with each worktree's group (SAFE, NOISE-ONLY, CACHE-ONLY, KEEP) and why: trust it, leave what they say goes to rush's own Clean up, and spend your time on what they can't decide.
- Go's module cache goes only through go clean -modcache, never rm (its files are read-only). A folder rm can't remove for Permission denied is made the owner's first (chmod -R u+rwx), in the script.
";
    if further {
        b += CARE_FURTHER;
    }
    for r in reports {
        if !r.rules.is_empty() {
            b += &format!("\n{}, as its plugin says:\n{}\n", r.title, r.rules.trim());
        }
    }
    b += "\nList the biggest wins first. Wait on a long command by running it in the foreground with a long timeout, or in the background until its notice comes; never sleep to wait.\n";
    if further {
        b += "\nEnd phase 2 with a short report, under 15 lines: what you removed or stopped, how much it freed as df measures it before and after, what you skipped or left and why, and the biggest things you found but left for the user to decide on, each with its path and size.\n\n";
    } else {
        b += "\nEnd phase 2 with a short report, under 10 lines: what you removed or stopped, how much it freed as df measures it before and after, and what you skipped or left and why.\n\n";
    }
    b += &format!("What rush knows:\n{}\n", known.join("\n"));
    for r in reports {
        b += &format!("\nWhat {} reports:\n{}\n", r.title, r.report.trim());
    }
    b
}

/// The plan's file in the cleanup agent's $TMPDIR: rush reads it to pick
/// what it may remove itself when the disk is dire and you're away
/// (dire.rs). Rust-only.
pub const CARE_PLAN_FILE: &str = "cleanup-plan.tsv";

/// The cleanup agent's two phases: a read-only plan the user approves,
/// then exactly what was approved, in one script (Rust-only: Go's ran in
/// auto mode and deleted as it went).
const CARE_PHASES: &str = "Phase 1, now, is read-only: survey (df -h, du, git status and git worktree list, the reports below) and delete, remove or stop nothing. These run without asking: du, df, ls, stat, git status, git worktree list, git rev-list, git branch (listing it only), xcrun simctl list, and writing files in $TMPDIR; run each plainly as its own command (git in the repository's folder rather than git -C, nothing chained after it, no redirection, a path with a space in it quoted (\"~/Library/Developer/Xcode/iOS DeviceSupport\") rather than its space escaped), or it asks. Anything else asks the user, so keep to these. End your turn with a plan:
- every path you would delete, each with its size and why it's safe to go (a can-go worktree, a done session's scratch, a cache that rebuilds itself, …) and conservative: yes when it's only something made or fetched again that loses no work (a done session's scratch untouched for days, a build cache in a worktree nobody works in, pnpm store prune), else conservative: no;
- each group's total, and a grand total of the space it frees;
- what you'd leave, and why;
- the same plan as $TMPDIR/cleanup-plan.tsv, one line per path: path, bytes, conservative (yes or no) and why, tab separated; it's the one file you write in phase 1. rush learns from it: a folder directly in ~/Library/Caches or ~/.cache marked yes becomes one of its rules, and its Clean up offers it from then on without an agent, so give each such line a why that says what makes it again;
- then a decisions block with one decision: (a) delete all of it; (b) only some, the user names which; (c) none.
Phase 2 starts only once the user answers. Delete exactly what they approved, nothing new: write the removals as one script, $TMPDIR/cleanup.sh, one line per approved path (rm -rf for a folder or file, git -C <repo> worktree remove for a worktree), then pnpm store prune if the plan has it, and run it once, so the user sees and approves one command. Run df -h before and after. Look at each path again first: one that changed since the plan (touched since, or now in use) stays out of the script, and you say so. With (c), delete nothing and say what you'd have freed.
";

/// What a cleanup agent told to look further may also do (Beyond Go).
const CARE_FURTHER: &str = "
Look further, too: past what rush knows, find what takes the most space on this disk (du -xhd1 on the home folder, ~/Library, ~/Library/Caches, ~/Library/Developer, ~/.cache and the package managers' folders) and put in your plan what rebuilds itself or is fetched again:
- applications' caches in ~/Library/Caches and ~/.cache;
- Xcode's DerivedData, device support for OS versions no device here runs, and unavailable simulators (xcrun simctl delete unavailable);
- package managers' caches through their own commands (brew cleanup, npm cache clean --force, pnpm store prune, pip cache purge, cargo's registry cache), and build caches (target, node_modules, .next) in projects untouched for weeks;
- Docker's dangling images and build cache (docker image prune, docker builder prune), never its volumes or containers;
- the copies of Chrome each launch leaves in $(getconf DARWIN_USER_TEMP_DIR)/../X/com.google.Chrome.code_sign_clone, except those a running Chrome has open (lsof): they share most of their blocks with Chrome, so du overstates them; say what df shows they freed.
Commands that free space (brew cleanup, docker image prune, …) go in the plan, and in phase 2's script once approved, as paths do.
Never propose documents, Downloads, Desktop, photos, music, mail, messages, or anything in ~/Library/Containers, ~/Library/Application Support or ~/Library/Group Containers: list the biggest of these in your plan's leave list for the user to decide on.
";

/// Opens the cleanup sheet because you asked (#cleanup): whatever the
/// disk and memory say, and in any rush. Beyond Go, which only offered it.
pub fn ask_care(m: &mut Model) -> Out {
    let at_work = m.agents.iter().any(|a| {
        a.rush
            && a.started_by == CARE_PLUGIN
            && !a.done
            && a.job.state != rush_core::agent::State::Done
    });
    if at_work {
        m.flash_err("a cleanup agent is already at work");
        return Out::Draw;
    }
    m.later(
        || {
            (
                rush_plugin::bundled_on(CARE_PLUGIN),
                rush_plugin::providers(SERVICE_CLEANUP_REPORT),
                rush_sys::sysinfo::read_disk(),
            )
        },
        |m, (on, from, d)| {
            if m.sheet.is_some() || m.confirm.is_some() {
                return Out::Idle;
            }
            let free = disk(d.free as i64);
            m.open_sheet(Box::new(CareSheet {
                why: format!("you asked for one, with {free} free on disk"),
                short: format!("asked, {free} free on disk"),
                // On Start (or Turn on): you asked for an agent, the
                // choice after Clean up.
                cur: 1,
                busy: false,
                looking: false,
                off: !on,
                from,
                asked: true,
            }));
            if m.now - data(m).checked > 30 * 60_000 {
                super::scan::scan_worktrees(m);
            }
            Out::Draw
        },
    );
    Out::Draw
}
