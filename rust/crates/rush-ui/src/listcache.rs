//! Rust-only: the agent list as last read, kept in rush's cache and drawn
//! as soon as rush opens, so it opens on your agents rather than "Reading
//! your agents…" while the first reading of every session runs. The live
//! reading replaces it whole when it lands (Model::set_fleet).
//!
//! Until then the kept list is only shown: its rows are drawn as they
//! were, but for each agent's mark that says what it's doing now
//! (working, waiting on you, just finished): drawn neutral and still, so a
//! row that was working when rush last looked doesn't claim to be now.
//! Nothing acts on it: no host is dialled and no reading's hooks run. The
//! top bar's segments as last drawn are kept with it and stand in for
//! each until its own reading is in (TopKept), so the start only gains
//! details. Kept a week at most; one from another version of rush is
//! left unread.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rush_core::agent::Agent;
use rush_core::text::{Color, Line, Mods, Span, Style};
use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

use crate::frame::Out;
use crate::model::Model;
use crate::worker::Note;

const FILE: &str = "list-kept.json";
/// What's kept older than this isn't shown.
const MOST_AGE: Ms = 7 * 24 * 3600 * 1000;
/// Saved at most this often while rush runs, and once more on quitting.
const EVERY: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Kept {
    version: String,
    saved_at: Ms,
    agents: Vec<Agent>,
    roots: Vec<String>,
    /// The top bar's segments as last drawn once what they show was in,
    /// by id, and the plan's rows when it had a block of them.
    top: BTreeMap<String, KeptLine>,
    plan: Vec<KeptLine>,
}

/// A line as kept: its spans' text and look (links aren't kept).
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
struct KeptLine(Vec<(String, KeptColor, KeptColor, u8)>);

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
enum KeptColor {
    #[default]
    Default,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

impl KeptColor {
    fn of(c: Color) -> KeptColor {
        match c {
            Color::Default => KeptColor::Default,
            Color::Rgb(r, g, b) => KeptColor::Rgb(r, g, b),
            Color::Indexed(i) => KeptColor::Indexed(i),
        }
    }
    fn color(self) -> Color {
        match self {
            KeptColor::Default => Color::Default,
            KeptColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
            KeptColor::Indexed(i) => Color::Indexed(i),
        }
    }
}

impl KeptLine {
    fn of(l: &Line) -> KeptLine {
        KeptLine(
            l.spans
                .iter()
                .map(|s| {
                    let st = s.style;
                    (
                        s.text.clone(),
                        KeptColor::of(st.fg),
                        KeptColor::of(st.bg),
                        st.mods.0,
                    )
                })
                .collect(),
        )
    }
    fn line(&self) -> Line {
        Line::from_spans(
            self.0
                .iter()
                .map(|(t, fg, bg, mods)| {
                    let mut st = Style::fg(fg.color()).on(bg.color());
                    st.mods = Mods(*mods);
                    Span::new(t.clone(), st)
                })
                .collect(),
        )
    }
}

/// How long after the first live reading the top bar may still draw
/// what it said last time, for a segment whose own reading isn't in yet:
/// the readings it waits on start with that reading.
const TOP_FOR: Duration = Duration::from_secs(3);

/// The top bar as kept, drawn for each segment until its own reading is
/// in: while the kept list shows, then for TOP_FOR at most (until).
#[derive(Default)]
struct TopKept {
    segs: std::collections::HashMap<String, Line>,
    plan: Vec<Line>,
    until: Option<Instant>,
}

/// The top bar's segments as last drawn with their readings in, to keep
/// with the list (drawing has only &Model).
#[derive(Default)]
pub struct TopDrawn {
    segs: RefCell<BTreeMap<String, KeptLine>>,
    plan: RefCell<Vec<KeptLine>>,
}

/// A priced reading of the list has landed: today's spend is in.
#[derive(Default)]
struct Priced(bool);

/// The account new agents start on, beside the folder when the top bar
/// doesn't show usage: kept as a top bar segment is.
pub const ACCOUNT: &str = "@account";

/// Makes m stand in the kept top bar's segs and plan rows, as a kept list
/// read at start would, and show a kept list.
#[cfg(test)]
pub(crate) fn stand_in(m: &mut Model, segs: &[(&str, Line)], plan: Vec<Line>) {
    m.kept::<TopDrawn>();
    m.kept::<Showing>().0 = true;
    *m.kept::<TopKept>() = TopKept {
        segs: segs
            .iter()
            .map(|(k, l)| (k.to_string(), l.clone()))
            .collect(),
        plan,
        until: None,
    };
}

/// Whether what top bar segment id shows has been read since rush
/// started: until then what it said last time stands in for it.
fn seg_in(m: &Model, id: &str) -> bool {
    let sys = || rush_sys::sysinfo::now().1.total != 0;
    let net = || rush_net::netwatch::now().known;
    let accts = || m.ext.calls.accounts_settled()(m);
    let limits = || accts() && m.ext.calls.limits_fetched()(m);
    match id {
        "cpu" | "ram" | "memory" => m.ext.calls.machine_read()(m),
        "disk" | "battery" => sys(),
        "net" => net(),
        "system" => sys() && net(),
        "today" => m.kept.peek::<Priced>().is_some_and(|p| p.0),
        "plan" | "usage" | "accounts" => limits(),
        "account" | ACCOUNT => accts(),
        _ => true,
    }
}

/// Whether the top bar may still draw what it said last time.
fn top_kept(m: &Model) -> Option<&TopKept> {
    let k = m.kept.peek::<TopKept>()?;
    let Some(until) = k.until else {
        return Some(k); // the kept list still shows
    };
    let left = until.checked_duration_since(Instant::now())?;
    crate::style::turns_in(left.as_millis() as Ms + 1);
    Some(k)
}

/// Top bar segment id as drawn now: live, its reading in, kept for the
/// list; else what it said last time, while that may stand in.
pub fn top_seg(m: &Model, id: &str, live: Line) -> Line {
    if !seg_in(m, id) {
        if let Some(l) = top_kept(m).and_then(|k| k.segs.get(id)) {
            return l.clone();
        }
        return live;
    }
    if let Some(d) = m.kept.peek::<TopDrawn>() {
        d.segs
            .borrow_mut()
            .insert(id.to_string(), KeptLine::of(&live));
    }
    live
}

/// The plan's rows as last time, while its reading isn't in.
pub fn top_plan(m: &Model) -> Option<Vec<Line>> {
    if seg_in(m, "plan") {
        return None;
    }
    top_kept(m)
        .map(|k| k.plan.clone())
        .filter(|p| !p.is_empty())
}

/// The plan's rows drawn live, kept for the list.
pub fn drew_plan(m: &Model, rows: &[Line]) {
    if seg_in(m, "plan")
        && let Some(d) = m.kept.peek::<TopDrawn>()
    {
        *d.plan.borrow_mut() = rows.iter().map(KeptLine::of).collect();
    }
}

/// Whether the list on screen is the kept one, not yet read again.
#[derive(Default)]
pub struct Showing(pub bool);

/// Whether the list on screen is the kept one, not yet read again.
pub fn showing(m: &Model) -> bool {
    m.kept.peek::<Showing>().is_some_and(|s| s.0)
}

/// When the list was last saved.
#[derive(Default)]
struct Saved(Option<Instant>);

fn path() -> PathBuf {
    rush_state::cache_dir().join(FILE)
}

/// The kept list being read, till the first frame takes it.
#[derive(Default)]
struct Reading(Option<std::sync::mpsc::Receiver<Kept>>);

/// How long the first frame waits for the kept list, so rush opens on it
/// rather than on "Reading your agents…" a moment before it.
pub const FIRST_WAIT: Duration = Duration::from_millis(50);

/// Reads the kept list off the UI thread, for [`wait_first`].
pub fn load(m: &mut Model) {
    m.kept::<TopDrawn>();
    if m.sender().is_none() {
        return;
    }
    let version = m.version.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    m.kept::<Reading>().0 = Some(rx);
    let _ = std::thread::Builder::new()
        .name("rush-list-kept".into())
        .spawn(move || {
            let Ok(b) = std::fs::read(path()) else { return };
            let Ok(k) = serde_json::from_slice::<Kept>(&b) else {
                return;
            };
            let now = rush_core::time::now();
            if k.version != version || now - k.saved_at > MOST_AGE || k.agents.is_empty() {
                return;
            }
            rush_draw::perf_note(|| format!("list kept read agents={}", k.agents.len()));
            let _ = tx.send(k);
        });
}

/// Shows the kept list in the first frame if it's read within
/// FIRST_WAIT, else as soon as it is, unless a live reading has come
/// first.
pub fn wait_first(m: &mut Model) {
    let Some(rx) = m.kept::<Reading>().0.take() else {
        return;
    };
    match rx.recv_timeout(FIRST_WAIT) {
        Ok(k) => {
            take(m, k);
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let Some(tx) = m.sender() else { return };
            let tx = tx.lane(1);
            let _ = std::thread::Builder::new()
                .name("rush-list-kept".into())
                .spawn(move || {
                    if let Ok(k) = rx.recv() {
                        tx.send(Note::Apply(Box::new(move |m: &mut Model| take(m, k))));
                    }
                });
        }
        Err(_) => {}
    }
}

fn take(m: &mut Model, k: Kept) -> Out {
    if m.loaded || !m.agents.is_empty() {
        return Out::Idle; // the live reading came first
    }
    m.agents = k.agents;
    m.roots = k.roots;
    m.kept::<Showing>().0 = true;
    let d = m.kept::<TopDrawn>();
    *d.segs.borrow_mut() = k.top.clone();
    *d.plan.borrow_mut() = k.plan.clone();
    *m.kept::<TopKept>() = TopKept {
        segs: k.top.iter().map(|(id, l)| (id.clone(), l.line())).collect(),
        plan: k.plan.iter().map(KeptLine::line).collect(),
        until: None,
    };
    m.rebuild();
    Out::Draw
}

/// After a live reading: the kept list is no longer what's shown, and
/// the reading is saved, at most once a minute.
pub fn took_reading(m: &mut Model, priced: bool) {
    m.kept::<Showing>().0 = false;
    if m.kept.peek::<TopKept>().is_some_and(|k| k.until.is_none()) {
        m.kept::<TopKept>().until = Some(Instant::now() + TOP_FOR);
    }
    if !priced {
        return;
    }
    m.kept::<Priced>().0 = true;
    let s = m.kept::<Saved>();
    if s.0.is_some_and(|t| t.elapsed() < EVERY) {
        return;
    }
    s.0 = Some(Instant::now());
    let k = kept(m);
    let _ = std::thread::Builder::new()
        .name("rush-list-save".into())
        .spawn(move || write(&k));
}

/// Saves the list now, as rush quits.
pub fn save_now(m: &mut Model) {
    if !m.loaded || m.kept::<Showing>().0 {
        return;
    }
    write(&kept(m));
}

fn kept(m: &Model) -> Kept {
    let d = m.kept.peek::<TopDrawn>();
    Kept {
        version: m.version.clone(),
        saved_at: rush_core::time::now(),
        agents: m.agents.clone(),
        roots: m.roots.clone(),
        top: d.map(|d| d.segs.borrow().clone()).unwrap_or_default(),
        plan: d.map(|d| d.plan.borrow().clone()).unwrap_or_default(),
    }
}

fn write(k: &Kept) {
    let Ok(b) = serde_json::to_vec(k) else { return };
    let p = path();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, b).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kept list reads back as it was saved.
    #[test]
    fn kept_reads_back_whole() {
        let mut a = Agent {
            key: "claude:alex:abc".into(),
            display_name: "A name".into(),
            pid: 42,
            rush: true,
            ..Agent::default()
        };
        a.job.id = "abc".into();
        a.spend.cost = 1.5;
        let mut top = BTreeMap::new();
        let l = Line::from_spans(vec![
            Span::new("3.3G", Style::fg(Color::Rgb(1, 2, 3)).bold()),
            Span::new(" ram", Style::fg(Color::Indexed(8)).on(Color::Rgb(4, 5, 6))),
        ]);
        top.insert("ram".to_string(), KeptLine::of(&l));
        let k = Kept {
            version: "v".into(),
            saved_at: 7,
            agents: vec![a.clone()],
            roots: vec!["/r".into()],
            top,
            plan: vec![KeptLine::of(&l)],
        };
        let b = serde_json::to_vec(&k).unwrap();
        let back: Kept = serde_json::from_slice(&b).unwrap();
        assert_eq!(back.agents, vec![a]);
        assert_eq!(back.roots, vec!["/r".to_string()]);
        assert_eq!((back.version.as_str(), back.saved_at), ("v", 7));
        assert_eq!(back.top["ram"].line(), l);
        assert_eq!(back.plan[0].line(), l);
    }

    /// The kept list's rows are drawn as they were but for a mark that
    /// says what an agent is doing now: neutral and still till the live
    /// reading lands.
    #[test]
    fn kept_rows_keep_their_look_but_not_their_state() {
        use rush_core::agent::State as JobState;
        let mut m = Model::new("0.1.0");
        m.w = 120;
        m.h = 30;
        let now = rush_core::time::now();
        m.clock = Some(now);
        m.now = now;
        let mut a = Agent {
            key: "claude:alex:abc".into(),
            display_name: "Working one".into(),
            pid: 42,
            ..Agent::default()
        };
        a.job.id = "abc".into();
        a.job.state = JobState::Working;
        m.agents = vec![a];
        m.roots = vec!["/r".into()];
        m.loaded = true;
        m.rebuild();
        let live: Vec<Line> = m.frame();
        let row = |f: &[Line]| {
            f.iter()
                .find(|l| l.plain().contains("Working one"))
                .cloned()
                .expect("its row")
        };
        let (lr, mark) = (row(&live), |l: &Line| {
            l.spans.iter().find(|s| !s.text.trim().is_empty()).cloned()
        });
        // Kept: the same row, its mark neutral.
        m.kept::<Showing>().0 = true;
        m.rebuild();
        let kept = m.frame();
        let kr = row(&kept);
        let km = kr
            .spans
            .iter()
            .find(|s| s.text == "◌")
            .expect("a still mark");
        assert_eq!(km.style.fg, crate::style::FAINT);
        let name = |l: &Line| {
            l.spans
                .iter()
                .find(|s| s.text.contains("Working one"))
                .cloned()
        };
        assert_eq!(name(&kr), name(&lr), "the name keeps its look");
        assert_ne!(mark(&lr), Some(km.clone()), "live, it says it's working");
    }
}
