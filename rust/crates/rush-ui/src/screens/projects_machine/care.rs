//! Disk and memory: a cleanup agent, offered when they run low. Plain code
//! watches the free disk and the memory pressure; when either passes what
//! Settings › Disk and memory says, rush asks whether to start a cleanup
//! agent, and no agent runs until you say yes. Go: internal/ui/
//! machinecare.go (careReason :46, watchMachine :67, careSheet :146,
//! reads :157, andList :166, body :175, key :201, pick :221, careKnown
//! :291, careReports :340, carePrompt :359).
//!
//! The session the cleanup agent runs in is started by whoever can start
//! hosted sessions: they set [`CareStarter`] (m.kept); until one does, yes
//! says so and starts nothing. Marking a finished cleanup agent done and
//! stopping its host (watchMachine :86) waits on that too.

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
pub(crate) const CARE_DISK_GB: i64 = 20;
/// The time under memory pressure asked about by default, in minutes.
pub(crate) const CARE_RAM_MINUTES: i64 = 15;
/// The least time between offers.
pub(crate) const CARE_SNOOZE: Ms = 6 * 3600 * 1000;
/// How long rush has run before it offers (Go's 120 ticks).
const CARE_WARMUP: Ms = 120_000;
/// How often the pressure is read (Go's every 5 ticks).
const CARE_EVERY: Ms = 5_000;
/// The plugin rush recommends turning on (bundled/cleanup).
const CARE_PLUGIN: &str = "cleanup";

/// A cleanup agent to start: its task, where, and its name. It runs in
/// auto mode where its agent has one: nobody is watching to answer its
/// permission prompts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CareStart {
    pub prompt: String,
    pub cwd: String,
    pub name: String,
    pub auto: bool,
}

/// Starts a hosted session for the cleanup agent, returning its id. Set by
/// the group that runs hosted sessions; None says it isn't here yet.
#[derive(Default)]
pub struct CareStarter(pub Option<StartCare>);

/// Starts the cleanup agent's session: its id, or why not.
pub type StartCare = fn(&mut Model, CareStart) -> Result<String, String>;

/// What the watch remembers.
#[derive(Default)]
pub(crate) struct Care {
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
}

/// Why a cleanup is worth offering, in words, or "" when it isn't: free
/// disk under disk_gb (0 the default, -1 never), or the memory under
/// pressure for longer than ram_min minutes (the same).
pub(crate) fn care_reason(
    free: u64,
    total: u64,
    disk_gb: i64,
    pressed: Ms,
    ram_min: i64,
) -> String {
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

/// "a", "a and b", "a, b and c".
pub(crate) fn and_list(l: &[String]) -> String {
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
pub(crate) fn watch_machine(m: &mut Model) {
    let now = m.now;
    let st = m.kept::<Care>();
    if st.started == 0 {
        st.started = now;
    }
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
            if now - c.started < CARE_WARMUP || (c.asked > 0 && now - c.asked < CARE_SNOOZE) {
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
            if why.is_empty() || !screen_free(m) {
                return Out::Idle;
            }
            m.kept::<Care>().asked = now;
            offer(m, why);
            Out::Idle
        },
    );
}

/// Reads whether the cleanup plugin is on and who reports to it, then
/// asks: unasked, so the cursor starts on Not now and an enter meant for
/// the list doesn't start anything.
fn offer(m: &mut Model, why: String) {
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
            m.open_sheet(Box::new(CareSheet {
                why,
                cur: 1,
                busy: false,
                off: !on,
                from,
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
pub(crate) struct CareSheet {
    pub why: String,
    /// 0 Start (or Turn on), 1 Not now.
    pub cur: usize,
    pub busy: bool,
    /// The cleanup plugin is off: the sheet offers turning it on.
    pub off: bool,
    /// The plugins that report to the cleanup agent.
    pub from: Vec<String>,
}

impl CareSheet {
    /// What the cleanup agent reads, in words.
    fn reads(&self) -> String {
        let what = "your worktrees, what agents left in /tmp".to_string();
        if self.from.is_empty() {
            return what;
        }
        format!("{what} and what {} report", and_list(&self.from))
    }

    fn pick(&mut self, m: &mut Model) -> Out {
        let hours = CARE_SNOOZE / 3_600_000;
        if self.cur == 1 {
            m.flash(&format!("no cleanup · asks again in {hours} hours"));
            return Out::Close;
        }
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
                        m.flash_err(&format!("couldn't turn on the cleanup plugin: {e}"));
                        return Out::Draw;
                    }
                    (s.off, s.cur) = (false, 1);
                    m.flash("cleanup plugin on");
                    Out::Draw
                },
            );
            return Out::Draw;
        }
        let known = care_known(m);
        let why = self.why.clone();
        m.later(care_reports, move |m, reports| {
            if m.sheet_as::<CareSheet>().is_none() {
                return Out::Idle;
            }
            m.sheet = None;
            let start = CareStart {
                prompt: care_prompt(&why, &known, &reports),
                cwd: std::env::var("HOME").unwrap_or_default(),
                name: "Cleanup".into(),
                auto: true,
            };
            let Some(f) = m.kept::<CareStarter>().0 else {
                m.flash_err("starting a cleanup agent isn't in this rush yet");
                return Out::Draw;
            };
            match f(m, start) {
                Ok(id) => {
                    let now = m.now;
                    m.kept::<Care>().agents.push((id, now));
                }
                Err(e) => m.flash_err(&format!("couldn't start a cleanup agent: {e}")),
            }
            Out::Draw
        });
        Out::Draw
    }
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
                     when this happens, which looks at {reads}, removes what can go without losing work and tells you what it did. \
                     Nothing starts until you say yes.",
                    self.why
                ),
                (
                    "Turn on the cleanup plugin",
                    "recommended · then asks whether to start a cleanup agent now",
                ),
            )
        } else {
            (
                format!(
                    "rush noticed that {}. A cleanup agent can look at {reads}, \
                     remove what can go without losing work (a worktree only once it's committed and pushed), stop what's holding memory that nobody works in, \
                     and tell you what it did. It's marked done when it finishes, which stops it.",
                    self.why
                ),
                (
                    "Start a cleanup agent",
                    "in auto mode, with the agent and model new sessions start with",
                ),
            )
        };
        for l in wrap(&text, w.saturating_sub(2)) {
            out.push(ln([plain(format!("  {l}"))]));
        }
        out.push(Line::new());
        let later = format!(
            "asks again in {} hours if it's still so",
            CARE_SNOOZE / 3_600_000
        );
        let choices = [first, ("Not now", later.as_str())];
        for (i, (what, about)) in choices.iter().enumerate() {
            let l = ln([spb(*what, TEXT), plain("  "), sp(*about, DIM)]);
            out.push(sheet_row(l, i == self.cur, w));
        }
        let k = match (self.busy, self.from.is_empty()) {
            (true, true) => ln([sp("⋯ starting", YELLOW)]),
            (true, false) => ln([sp(
                format!("⋯ asking {} for their reports", and_list(&self.from)),
                YELLOW,
            )]),
            _ => keys_fit(
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

/// What rush knows of the disk, the memory, worktrees and what agents
/// left, as lines for the cleanup agent. It reads nothing: the disk and
/// memory as last read by the place, and the worktrees as Clean up's scan
/// left them.
pub(crate) fn care_known(m: &Model) -> Vec<String> {
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
pub(crate) fn care_known_with(m: &Model, out: &mut Vec<String>, mem_level: i32) {
    if mem_level >= 0 {
        out.push(format!("Memory: {mem_level}% available."));
    }
    let st = data(m);
    if st.checked == 0 {
        out.push(
            "Worktrees: rush hasn't looked yet; find them with git worktree list in the repositories under ~/Source."
                .into(),
        );
        return;
    }
    out.push(String::new());
    out.push(format!(
        "Worktrees, as rush checked them {} (path · branch · size · verdict):",
        ago(m.now - st.checked)
    ));
    for w in &st.wts {
        let v = w.verdict();
        let mut l = format!(
            "- {} · {} · {} · {}",
            w.path,
            w.branch,
            disk(w.size),
            verdict_word(v.kind)
        );
        if !v.reason.is_empty() {
            l += &format!(" ({})", v.reason);
        }
        if running(m, &w.agents).is_some() {
            l += " · an agent is working in it now";
        }
        out.push(l);
    }
    if st.tmp.stale_items > 0 {
        out.push(String::new());
        out.push(format!(
            "/tmp: {} of your items untouched for days, {}.",
            st.tmp.stale_items,
            disk(st.tmp.stale)
        ));
    }
}

/// A verdict as Go's VerdictKind.String says it.
fn verdict_word(k: rush_fleet::VerdictKind) -> &'static str {
    use rush_fleet::VerdictKind::*;
    match k {
        KeepInUse => "keep-in-use",
        KeepPrOpen => "keep-pr-open",
        CanGo => "can-go",
        WouldLose => "would-lose",
    }
}

/// How long ago, in words (agentviews.go:891).
pub(crate) fn ago(d: Ms) -> String {
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
pub(crate) fn care_prompt(why: &str, known: &[String], reports: &[CleanupReport]) -> String {
    let mut b = String::new();
    b += &format!(
        "Free disk space and memory on this machine, then report and stop. rush started you because {why}. "
    );
    b += "Nobody is watching, so don't ask questions: do only what loses no work, and leave the rest.\n\n";
    b += "Rules:
- Remove a worktree only when it's can-go below, or git shows nothing uncommitted, unpushed or unmerged in it. Use git worktree remove and keep its branch. Never touch one an agent is working in, or a repository's main checkout.
- Remove what agents left in /tmp or outside their projects only when it's marked safe, or nothing has touched it for days and nothing running uses it.
- Build caches that rebuild themselves (node_modules, .next, .turbo, target, dist) may go from worktrees nobody works in.
- Never delete databases, ~/.claude, transcripts, anything in a project that isn't a cache or a worktree, or anything you're unsure of.
";
    for r in reports {
        if !r.rules.is_empty() {
            b += &format!("\n{}, as its plugin says:\n{}\n", r.title, r.rules.trim());
        }
    }
    b += "\nEnd with a short report, under 10 lines: what you removed or stopped, how much it freed, and what you left and why.\n\n";
    b += &format!("What rush knows:\n{}\n", known.join("\n"));
    for r in reports {
        b += &format!("\nWhat {} reports:\n{}\n", r.title, r.report.trim());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::sheet::Sheet;

    #[test]
    fn reasons() {
        const G: u64 = 1 << 30;
        let cases: &[(u64, u64, i64, Ms, i64, &str)] = &[
            (
                10 * G,
                500 * G,
                0,
                0,
                0,
                "the disk has 10.0G free, under the 20G you set",
            ),
            (30 * G, 500 * G, 0, 0, 0, ""),
            (10 * G, 500 * G, -1, 0, 0, ""),
            (10 * G, 0, 0, 0, 0, ""),
            (
                30 * G,
                500 * G,
                0,
                16 * 60_000,
                0,
                "the memory has been under pressure for 16 minutes",
            ),
            (30 * G, 500 * G, 0, 16 * 60_000, -1, ""),
            (30 * G, 500 * G, 0, 14 * 60_000, 0, ""),
            (
                30 * G,
                500 * G,
                40,
                0,
                0,
                "the disk has 30.0G free, under the 40G you set",
            ),
        ];
        for &(free, total, gb, pressed, min, want) in cases {
            assert_eq!(care_reason(free, total, gb, pressed, min), want);
        }
    }

    #[test]
    fn lists() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(and_list(&s(&[])), "");
        assert_eq!(and_list(&s(&["a"])), "a");
        assert_eq!(and_list(&s(&["a", "b"])), "a and b");
        assert_eq!(and_list(&s(&["a", "b", "c"])), "a, b and c");
    }

    #[test]
    fn prompt_has_rules_known_and_reports() {
        let r = CleanupReport {
            title: "Haven".into(),
            report: " 3 stacks idle \n".into(),
            rules: "Stop only idle stacks.".into(),
        };
        let p = care_prompt("the disk is low", &["Disk: 1G free of 2G.".into()], &[r]);
        assert!(p.starts_with("Free disk space and memory on this machine, then report and stop. rush started you because the disk is low. "));
        assert!(p.contains("\nHaven, as its plugin says:\nStop only idle stacks.\n"));
        assert!(p.contains("What rush knows:\nDisk: 1G free of 2G.\n"));
        assert!(p.ends_with("\nWhat Haven reports:\n3 stacks idle\n"));
    }

    fn sheet(off: bool) -> CareSheet {
        CareSheet {
            why: "the disk has 10.0G free, under the 20G you set".into(),
            cur: 1,
            busy: false,
            off,
            from: vec!["Haven".into(), "go-clean".into()],
        }
    }

    fn text(l: &[Line]) -> String {
        l.iter().map(text_of).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn sheet_says_what_it_reads() {
        let m = Model::new("t");
        let mut s = sheet(false);
        let t = text(&s.body(&m, 100, 30));
        assert!(t.starts_with("Cleanup  the disk has 10.0G free"), "{t}");
        let flat = t.replace("\n  ", " ");
        assert!(
            flat.contains("what agents left in /tmp and what Haven and go-clean report, remove"),
            "{t}"
        );
        assert!(t.contains("Start a cleanup agent  in auto mode"));
        assert!(t.contains("▍ Not now  asks again in 6 hours if it's still so"));
        assert!(t.contains("↑↓ choose"));
        let t = text(&sheet(true).body(&m, 100, 30));
        assert!(t.contains("Turn on the cleanup plugin  recommended"));
        assert!(
            t.replace("\n  ", " ")
                .contains("The cleanup plugin is off, and recommended now")
        );
    }

    #[test]
    fn sheet_keys() {
        let mut m = Model::new("t");
        let mut s = sheet(false);
        for (k, cur) in [("down", 0), ("tab", 1), ("up", 0), ("shift+tab", 1)] {
            assert_eq!(s.key(&mut m, &Press::of(k).unwrap()), Out::Draw);
            assert_eq!(s.cur, cur, "{k}");
        }
        for k in ["n", "esc", "ctrl+c"] {
            let mut s = sheet(false);
            s.cur = 0;
            assert_eq!(s.key(&mut m, &Press::of(k).unwrap()), Out::Close, "{k}");
            assert_eq!(s.cur, 1);
        }
        let mut s = sheet(false);
        s.busy = true;
        assert_eq!(s.key(&mut m, &Press::of("enter").unwrap()), Out::Idle);
    }

    #[test]
    fn known_before_and_after_a_look() {
        let mut m = Model::new("t");
        let mut out = Vec::new();
        care_known_with(&m, &mut out, 40);
        assert_eq!(out[0], "Memory: 40% available.");
        assert!(out[1].starts_with("Worktrees: rush hasn't looked yet"));
        m.now = 10 * 60_000;
        let d = m.kept::<Data>();
        d.checked = 5 * 60_000;
        let mut w = rush_fleet::Worktree::default();
        w.path = "/src/r/.claude/worktrees/a".into();
        w.branch = "rush/a".into();
        d.wts.push(w);
        d.tmp.stale_items = 2;
        d.tmp.stale = 3 << 20;
        let mut out = Vec::new();
        care_known_with(&m, &mut out, -1);
        assert_eq!(
            out,
            [
                "",
                "Worktrees, as rush checked them 5m ago (path · branch · size · verdict):",
                "- /src/r/.claude/worktrees/a · rush/a · – · would-lose (not looked at with git yet)",
                "",
                "/tmp: 2 of your items untouched for days, 3M.",
            ]
        );
    }
}
