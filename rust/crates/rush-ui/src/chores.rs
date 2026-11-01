//! What rush does by itself on Go's one-second tick, at the same seconds
//! (model.go:1089 tickMsg, :1095 and :1103): the installed rush looked at
//! (reload.go:134 watchBinary, tick%30 == 3), and in the rush that leads,
//! work that landed settled (cleanup.go:660 settleLanded, tick%60 == 15),
//! done work tidied (cleanup.go:240 tidyDone, tick%600 == 30) and
//! sessions told of subagents gone quiet (stucksubs.go nudgeStuckSubs,
//! every tick in Go; every 30 here, at 20, as a run is quiet for minutes
//! before it's told). Go
//! ticks every second whatever happens; here the loop is woken only at
//! the next second a chore is due, the chore's work goes off the UI
//! thread, and a wake that only ran chores draws nothing, so an idle rush
//! costs a wake every half a minute and no frame. The battery and disk
//! are read again at the same second while shown (Go's sysinfo, read
//! every 30 seconds while drawn).

use std::time::{Duration, Instant};

use crate::model::Model;

/// A chore: run at Go's tick `at` and every `every` ticks after, in every
/// rush or only the one that leads. The screens push theirs into
/// Ext::chores at register (screens/mod.rs), in the order they run.
pub struct Chore {
    pub at: u64,
    pub every: u64,
    pub lead_only: bool,
    /// Whether it's wanted now; a chore not wanted isn't woken for.
    pub when: fn(&Model) -> bool,
    pub run: fn(&mut Model),
}

pub fn always(_: &Model) -> bool {
    true
}

/// The battery and disk read again, while the last frame showed them: Go
/// reads them every 30 seconds while drawn (sysinfo.go:27), and each
/// second draws them; here they're drawn only when they show differently.
pub fn read_sysinfo(_: &mut Model) {
    if rush_sys::sysinfo::asked() {
        rush_sys::sysinfo::read_again();
    }
}

/// When the tick started, and the last tick whose chores ran.
#[derive(Default)]
pub struct Clock {
    start: Option<Instant>,
    through: u64,
}

/// Starts the tick: from now, with a loop to wake.
pub fn start(m: &mut Model) {
    m.kept::<Clock>().start = Some(Instant::now());
}

/// The first tick after `after` at which c is due.
fn next_of(c: &Chore, after: u64) -> u64 {
    if after < c.at {
        return c.at;
    }
    c.at + ((after - c.at) / c.every + 1) * c.every
}

/// When the next chore is due, if any is this rush's to do.
pub fn due_at(m: &Model) -> Option<Instant> {
    let clock = m.kept.peek::<Clock>()?;
    let start = clock.start?;
    let lead = m.leads();
    m.ext
        .chores
        .iter()
        .filter(|c| (lead || !c.lead_only) && (c.when)(m))
        .map(|c| next_of(c, clock.through))
        .min()
        .map(|t| start + Duration::from_secs(t))
}

/// Runs the chores due by now, each once however many ticks went by;
/// whether any ran.
pub fn run_due(m: &mut Model, now: Instant) -> bool {
    let Some(start) = m.kept.peek::<Clock>().and_then(|c| c.start) else {
        return false;
    };
    let tick = now.saturating_duration_since(start).as_secs();
    let through = m.kept::<Clock>().through;
    if tick <= through {
        return false;
    }
    m.kept::<Clock>().through = tick;
    let lead = m.leads();
    let mut ran = false;
    for i in 0..m.ext.chores.len() {
        let c = &m.ext.chores[i];
        let (run, when) = (c.run, c.when);
        if (lead || !c.lead_only) && next_of(c, through) <= tick && when(m) {
            run(m);
            ran = true;
        }
    }
    ran
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_gos_seconds() {
        let m = Model::new("t");
        let c = &m.ext.chores[0];
        assert_eq!(next_of(c, 0), 3);
        assert_eq!(next_of(c, 3), 33);
        assert_eq!(next_of(c, 40), 63);
        let t = &m.ext.chores[3];
        assert_eq!(next_of(t, 29), 30);
        assert_eq!(next_of(t, 30), 630);
    }

    // Only the rush that leads settles and tidies; every one looks at the
    // installed rush. Nothing's due without a loop's tick started.
    #[test]
    fn due_by_lead() {
        let mut m = Model::new("t");
        assert!(due_at(&m).is_none());
        start(&mut m);
        let s = m.kept.peek::<Clock>().unwrap().start.unwrap();
        m.follows = true;
        m.kept::<Clock>().through = 3;
        assert_eq!(due_at(&m), Some(s + Duration::from_secs(33)));
        m.follows = false;
        assert_eq!(due_at(&m), Some(s + Duration::from_secs(15)));
    }

    // While a room the list read runs, the rooms are read each second
    // (Go's tick refreshes and reads them): no room running, no wake.
    #[test]
    fn rooms_each_second_while_one_runs() {
        use crate::rooms::RoomRows;
        let mut m = Model::new("t");
        start(&mut m);
        let s = m.kept.peek::<Clock>().unwrap().start.unwrap();
        m.follows = true;
        let lp: rush_draw::Loop<crate::worker::Note> = rush_draw::Loop::new();
        m.tx = Some(lp.sender());
        m.kept::<Clock>().through = 5;
        assert_eq!(due_at(&m), Some(s + Duration::from_secs(33)));
        let mut room = rush_room::room::Summary::default();
        m.kept::<RoomRows>().list = vec![room.clone()];
        assert_eq!(due_at(&m), Some(s + Duration::from_secs(33)));
        room.running = true;
        m.kept::<RoomRows>().list = vec![room];
        assert_eq!(due_at(&m), Some(s + Duration::from_secs(6)));
        // Drawn once (no loop): nothing to wake for.
        m.tx = None;
        assert_eq!(due_at(&m), Some(s + Duration::from_secs(33)));
    }
}
