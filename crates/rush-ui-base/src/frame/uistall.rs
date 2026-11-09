//! The UI thread, watched. Nothing that waits (the disk, the network,
//! another process) belongs on it: every key waits behind it. Each key,
//! click, message, tick and frame is timed; one that held the thread
//! longer than STALL_AFTER is counted, and logged to rush's events log as
//! kind `ui-stall` (`rush events -k ui-stall`), so whatever still does is
//! found by name rather than by feel. The Network sheet shows the count.
//! After Go's internal/ui/uistall.go (stallAfter :26, uiBusy :52,
//! watchUI :98, stallReport :182). Unlike Go, no watchdog thread ticks to
//! catch the stack mid-stall: idle costs nothing, so a stall is measured
//! when it ends and logged with what it was. Rust-only: the line goes to
//! the events log (not stalls.log), names its phase, what ran and the
//! session open, and is rate-limited (a storm can't flood the log).

use std::cell::Cell;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use rush_draw::{Key, KeyMods, MouseKind, key_string};
use serde_json::Value;

/// How long the UI thread may be held before it's a stall: about a frame
/// and a half at 60 Hz; typing feels it past this.
pub const STALL_AFTER: Duration = Duration::from_millis(25);

/// How many stall lines the log takes in a burst, and how often one more
/// is allowed after: a storm logs BURST, then one each REFILL, the rest
/// counted in the next line's `dropped`.
const BURST: u32 = 10;
const REFILL: Duration = Duration::from_secs(6);

/// One time the UI thread was held up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stall {
    /// What it was doing: "key ctrl+k", "msg subs::took", "draw".
    pub what: String,
    pub took: Duration,
    pub at: Option<SystemTime>,
}

/// Which of the loop's turns held it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Key,
    Mouse,
    Msg,
    Tick,
    Draw,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Key => "key",
            Phase::Mouse => "mouse",
            Phase::Msg => "msg",
            Phase::Tick => "tick",
            Phase::Draw => "draw",
        }
    }
}

/// What a turn handled, kept cheap (no text made) until it's a stall.
#[derive(Clone, Copy, Debug)]
pub enum What {
    Key(Key, KeyMods),
    Mouse(MouseKind),
    /// A message, by its kind.
    Named(&'static str),
}

impl What {
    fn text(self) -> String {
        match self {
            // A letter typed is "char": the log doesn't keep what you type.
            What::Key(Key::Char(c), m)
                if c != ' '
                    && !m.contains(KeyMods::CTRL)
                    && !m.contains(KeyMods::ALT)
                    && !m.contains(KeyMods::SUPER) =>
            {
                "char".into()
            }
            What::Key(k, m) => key_string(k, m),
            What::Mouse(k) => format!("{k:?}").to_lowercase(),
            What::Named(s) => s.to_string(),
        }
    }
}

struct Stalls {
    /// Watched: the live UI only, so tests' frames stay as they are.
    on: bool,
    n: usize,
    longest: Stall,
    last: Stall,
    /// Lines the log may take now, and when the last was given back.
    tokens: u32,
    refilled: Option<Instant>,
    /// Stalls not logged since the last line that was.
    dropped: u32,
}

const NONE: Stall = Stall {
    what: String::new(),
    took: Duration::ZERO,
    at: None,
};

static STALLS: Mutex<Stalls> = Mutex::new(Stalls {
    on: false,
    n: 0,
    longest: NONE,
    last: NONE,
    tokens: BURST,
    refilled: None,
    dropped: 0,
});

fn lock() -> std::sync::MutexGuard<'static, Stalls> {
    STALLS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts watching (the live UI does, as it starts); safe to call more
/// than once.
pub fn watch_ui() {
    lock().on = true;
}

thread_local! {
    /// The longest named piece run in this turn (a later's apply, a
    /// timer): what a stall is put down to.
    static PIECE: Cell<Option<(&'static str, Duration)>> = const { Cell::new(None) };
}

/// Runs f as a named piece of the turn: the turn's stall, if it is one,
/// names the longest piece rather than the message that carried it.
pub fn piece<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    let t = Instant::now();
    let r = f();
    let d = t.elapsed();
    PIECE.with(|p| {
        if p.get().is_none_or(|(_, pd)| d > pd) {
            p.set(Some((name, d)));
        }
    });
    r
}

/// A closure's name, for [`piece`]: its function's path, the crate and
/// `{{closure}}`s cut ("subs::refresh").
pub fn name_of<F>(_: &F) -> &'static str {
    let all = std::any::type_name::<F>();
    // Its generics cut, unless it's of a trait's impl ("<A as B>::f").
    let mut n = match all.split_once('<') {
        Some((head, _)) if !head.is_empty() => head.trim_end_matches("::"),
        _ => all,
    };
    while let Some(s) = n.strip_suffix("::{{closure}}") {
        n = s;
    }
    match n.split_once("::") {
        Some((krate, rest)) if krate.starts_with("rush_") => rest,
        _ => n,
    }
}

/// A turn begun: its start, and the piece slate wiped.
pub fn begin() -> Instant {
    PIECE.with(|p| p.set(None));
    Instant::now()
}

/// A turn ended: logged when it held the thread past STALL_AFTER. The
/// session open, when there is one, is asked for only then.
pub fn end<'a>(
    began: Instant,
    phase: Phase,
    what: What,
    session: impl FnOnce() -> Option<&'a str>,
) {
    let took = began.elapsed();
    if took < STALL_AFTER {
        return;
    }
    let piece = PIECE.with(|p| p.take());
    note(phase, what, piece.map(|p| p.0), session(), took);
}

/// Counts a turn that held the thread for took and logs it, at most
/// BURST at once and one each REFILL after.
pub fn note(phase: Phase, what: What, piece: Option<&str>, session: Option<&str>, took: Duration) {
    if took < STALL_AFTER {
        return;
    }
    let name = what.text();
    let label = match piece {
        Some(p) => format!("{} {name} ({p})", phase.as_str()),
        None => format!("{} {name}", phase.as_str()),
    };
    let now = Instant::now();
    let dropped = {
        let mut s = lock();
        if !s.on {
            return;
        }
        let st = Stall {
            what: label,
            took,
            at: Some(SystemTime::now()),
        };
        s.n += 1;
        if took > s.longest.took {
            s.longest = st.clone();
        }
        s.last = st;
        let since = s.refilled.map_or(Duration::ZERO, |r| now.duration_since(r));
        if s.refilled.is_none() {
            s.refilled = Some(now);
        } else if since >= REFILL {
            let back = (since.as_secs_f64() / REFILL.as_secs_f64()) as u32;
            s.tokens = (s.tokens + back).min(BURST);
            s.refilled = Some(now);
        }
        if s.tokens == 0 {
            s.dropped += 1;
            return;
        }
        s.tokens -= 1;
        std::mem::take(&mut s.dropped)
    };
    let mut f = rush_events::Fields::new();
    f.insert("phase".into(), Value::from(phase.as_str()));
    f.insert("what".into(), Value::from(name));
    if let Some(p) = piece {
        f.insert("piece".into(), Value::from(p));
    }
    if let Some(s) = session.filter(|s| !s.is_empty()) {
        f.insert("open".into(), Value::from(s));
    }
    f.insert("ms".into(), Value::from(took.as_millis() as u64));
    if dropped > 0 {
        f.insert("dropped".into(), Value::from(dropped));
    }
    // The log's writer is a thread of its own: this only queues.
    rush_events::log("ui-stall", f);
}

/// What the watch has seen, for the Network sheet (stallReport): how
/// many, the longest and the last.
pub fn report() -> (usize, Stall, Stall) {
    let s = lock();
    (s.n, s.longest.clone(), s.last.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_closures_by_their_function() {
        fn outer() -> &'static str {
            let f = || 1;
            name_of(&f)
        }
        let n = outer();
        assert!(n.ends_with("outer"), "{n}");
        assert!(!n.contains("{{closure}}"), "{n}");
    }

    #[test]
    fn counts_only_stalls() {
        let before = report().0;
        watch_ui();
        note(
            Phase::Draw,
            What::Named("frame"),
            None,
            None,
            Duration::from_millis(1),
        );
        note(
            Phase::Key,
            What::Key(Key::Char('x'), KeyMods::NONE),
            Some("cmdbar::rebuild"),
            Some("s1"),
            Duration::from_millis(80),
        );
        let (n, longest, _) = report();
        assert!(n > before);
        assert!(longest.took >= Duration::from_millis(80));
        assert_eq!(What::Key(Key::Char('x'), KeyMods::NONE).text(), "char");
        assert_eq!(What::Key(Key::Char('k'), KeyMods::CTRL).text(), "ctrl+k");
    }
}
