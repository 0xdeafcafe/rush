//! Opening the agent you were in when rush last closed: where the view was
//! is kept as it closes (and every so often while it runs, so a crash or
//! a closed terminal leaves it too), in rush's folder's lastview.json
//! (rush_state::lastview). The next rush opens that session again once
//! the list is read, if rush closed within Settings › General's "Reopen
//! the last agent if closed within" (30 minutes unless set), the view was
//! on a session rather than a list, and the session is still in Active or
//! Just left: not put away, cleaned up or hibernated.
//!
//! A restart rush does itself (#reload, `rush reload`, an update) isn't a
//! close: that rush hands the next its place through RUSH_RESTORE
//! (services.rs), which wins, and nothing is kept as it goes. Opening rush
//! on a session (`rush open <id>`, rush with an argument) wins too.

use std::time::{Duration, Instant};

use rush_core::agent::State;
use rush_core::time::Ms;
use rush_fleet::Predicates;
use rush_state::lastview::{self, LastView};
use rush_ui_social::services::{RESTORE_ENV, Reload};

use crate::frame::PlaceId;
use crate::list;
use crate::model::Model;
use crate::session::Pos;

/// How often a running view writes where it is, when that hasn't changed.
const EVERY: Duration = Duration::from_secs(30);

/// How long a scroll position waits for its turn to be read.
const POS_WAIT: Duration = Duration::from_secs(20);

/// The list's sections that are Active, and Just left.
const IN_PLAY: [&str; 8] = [
    list::NEEDS,
    list::FINISHED,
    list::STUCK,
    list::WORKING,
    list::TO_LAND,
    list::PINNED,
    list::IDLE,
    list::JUST_LEFT,
];

/// What this view keeps of it.
#[derive(Default)]
pub struct Reopen {
    /// The first reading has been and the last view looked at.
    took: bool,
    /// rush was opened on something: nothing is reopened.
    explicit: bool,
    /// A view that isn't yours (`rush --soak`): it neither reopens nor
    /// keeps anything.
    quiet: bool,
    /// When this view first read the list, in ms.
    started: Ms,
    /// What was last written, and when.
    wrote: Option<(LastView, Instant)>,
    /// The scroll to put the reopened session's conversation back to,
    /// once its turn is read: its key, turn, rows into it, since when,
    /// and your last input then.
    pos: Option<(String, u32, usize, Instant, Ms)>,
}

/// Adds the hook after each reading of the list.
pub fn register(x: &mut crate::frame::Ext) {
    x.on_fleet.push(on_fleet);
}

/// rush was opened on something (an argument): it wins over reopening.
pub fn opened_explicitly(m: &mut Model) {
    m.kept::<Reopen>().explicit = true;
}

/// This view is a measuring run, not yours: it leaves the last view
/// alone.
pub fn leave_alone(m: &mut Model) {
    let r = m.kept::<Reopen>();
    r.quiet = true;
    r.explicit = true;
}

/// Where the view is now.
pub fn snapshot(m: &Model) -> LastView {
    let mut v = LastView {
        place: m.place.name().to_string(),
        page: m.agents_page,
        at: rush_core::time::now(),
        pid: std::process::id(),
        ..Default::default()
    };
    if let Some(s) = &m.session {
        v.session = s.key.clone();
        v.view = crate::views::view_name(m, &s.key).to_string();
        if let Pos::At { n, off } = s.pos {
            v.turn = Some(n);
            v.off = off;
        }
    }
    v
}

/// Whether this rush is one a restart (#reload) is replacing, or hosted
/// on one session, or a measuring run: none keeps where it was.
fn keeps_nothing(m: &Model) -> bool {
    m.kept.peek::<Reload>().is_some_and(|r| r.reloading)
        || m.kept.peek::<Reopen>().is_some_and(|r| r.quiet)
        || m.ext.calls.hosted_id()(m).is_some()
}

/// The view is closing (quit, or the terminal gone): where it was is kept
/// for the next rush. A restart rush does itself isn't a close.
pub fn save_on_close(m: &mut Model) {
    if keeps_nothing(m) || !m.loaded {
        return;
    }
    let _ = lastview::save_closed(&snapshot(m));
}

/// After each reading of the list: the first time, the last view's
/// session reopened if it should be; a scroll waiting put back; and with
/// a loop running, where the view is written now and then.
fn on_fleet(m: &mut Model) {
    let live = m.sender().is_some();
    if !m.kept::<Reopen>().took {
        let st = m.kept::<Reopen>();
        st.took = true;
        st.started = rush_core::time::now();
        if live && !keeps_nothing(m) {
            let rec = lastview::load().filter(lastview::left);
            let hib = if rec.as_ref().is_some_and(|r| !r.session.is_empty()) {
                rush_state::Store::load().config.hibernate.after_minutes
            } else {
                0
            };
            reopen(m, rec, hib);
        }
    }
    put_back_pos(m);
    if live && !keeps_nothing(m) {
        write_now_and_then(m);
    }
}

/// Opens rec's session if it should be (see the module's words);
/// hibernate_minutes is Settings › General's Hibernate after. Whether it
/// opened.
pub fn reopen(m: &mut Model, rec: Option<LastView>, hibernate_minutes: i64) -> bool {
    let Some(rec) = rec else { return false };
    if m.kept::<Reopen>().explicit
        || std::env::var_os(RESTORE_ENV).is_some_and(|v| !v.is_empty())
        || m.ext.calls.hosted_id()(m).is_some()
    {
        return false;
    }
    // You've started doing something since rush opened.
    if m.session.is_some() || m.last_input != 0 || m.place != PlaceId::Agents {
        return false;
    }
    let within = m.cfg.reopen_for();
    let now = m.now;
    if within <= 0 || now - rec.at >= within || rec.at > now + 60_000 {
        return false;
    }
    if rec.place != PlaceId::Agents.name() || rec.session.is_empty() {
        return false; // a list, or another place
    }
    let key = rec.session.clone();
    let Some(i) = m.agents.iter().position(|a| a.key == key) else {
        return false; // gone, or cleaned up
    };
    if !in_play(m, i, hibernate_minutes) || !m.items().contains(&key) {
        return false;
    }
    m.sel = key.clone();
    m.open(i);
    if m.session.as_ref().is_none_or(|s| s.key != key) {
        return false;
    }
    if !rec.view.is_empty() {
        crate::views::show_view(m, &key, &rec.view);
    }
    if let Some(n) = rec.turn {
        let input = m.last_input;
        m.kept::<Reopen>().pos = Some((key, n, rec.off, Instant::now(), input));
        put_back_pos(m);
    }
    true
}

/// Whether agent i is in Active or Just left, and not put away or
/// hibernated (a finished agent whose process stopped after Hibernate
/// after).
fn in_play(m: &Model, i: usize, hibernate_minutes: i64) -> bool {
    let a = &m.agents[i];
    if a.done {
        return false;
    }
    if hibernate_minutes > 0
        && a.pid == 0
        && a.job.state == State::Done
        && a.age(m.now) > hibernate_minutes * 60_000
    {
        return false;
    }
    m.group_of
        .get(&a.key)
        .is_some_and(|g| IN_PLAY.contains(&g.as_str()))
}

/// The reopened conversation scrolled back to where it was, once that
/// turn is read; given up once you've moved, or after POS_WAIT.
fn put_back_pos(m: &mut Model) {
    let Some((key, n, off, since, input)) = m.kept::<Reopen>().pos.clone() else {
        return;
    };
    let open = m.session.as_ref().is_some_and(|s| s.key == key);
    if !open || m.last_input != input || since.elapsed() > POS_WAIT {
        m.kept::<Reopen>().pos = None;
        return;
    }
    let Some(v) = m.session.as_mut() else { return };
    if v.sess.turns.iter().any(|t| t.n == n) {
        v.pos = Pos::At { n, off };
        m.kept::<Reopen>().pos = None;
    }
}

/// Writes where the view is when it has changed, or EVERY otherwise, off
/// the UI thread.
fn write_now_and_then(m: &mut Model) {
    let now = snapshot(m);
    let st = m.kept::<Reopen>();
    let due = match &st.wrote {
        None => true,
        Some((was, at)) => !same_place(was, &now) || at.elapsed() >= EVERY,
    };
    if !due {
        return;
    }
    let started = st.started;
    st.wrote = Some((now.clone(), Instant::now()));
    let _ = std::thread::Builder::new()
        .name("rush-lastview".into())
        .spawn(move || {
            let _ = lastview::save_running(&now, started);
        });
}

fn same_place(a: &LastView, b: &LastView) -> bool {
    (&a.place, a.page, &a.session, &a.view, a.turn, a.off)
        == (&b.place, b.page, &b.session, &b.view, b.turn, b.off)
}
