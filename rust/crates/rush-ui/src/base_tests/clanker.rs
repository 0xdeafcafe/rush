//! clanker.rs's tests (rush-ui-base) that read his reaction: it's kept
//! by session_core's finish, through Ext::calls.

use crate::clanker::*;

/// Working, a light passes over him at second 8 of every 15; idle, at
/// 25 of every 45; never over a reaction running (clkBeat).
#[test]
fn beat_shimmers_now_and_then() {
    let mut m = crate::new_model("t");
    m.clock = Some(1_790_000_000_000);
    m.tick = 24;
    let idle = m.mood(m.tally());
    beat(&mut m);
    assert_eq!(reaction(&m).kind, Fx::None);
    m.tick = 25;
    beat(&mut m);
    let want = if idle == Mood::Idle {
        Fx::Shimmer
    } else {
        Fx::None
    };
    assert_eq!(reaction(&m).kind, want, "{idle:?}");
    m.kept::<crate::finish::Finish>().fx = Reaction {
        kind: Fx::Error,
        frame: 3,
    };
    m.tick = 25 + CYCLE;
    beat(&mut m);
    assert_eq!(
        reaction(&m),
        Reaction {
            kind: Fx::Error,
            frame: 3
        }
    );
}

/// Now and then a light passes over him (clkBeat).
#[test]
fn shimmers_on_his_second() {
    let mut m = crate::new_model("t");
    m.clock = Some(1_790_000_000_000);
    m.tick = 8;
    let md = m.mood(m.tally());
    m.tick = if md == Mood::Working { 8 } else { 25 };
    beat(&mut m);
    assert_eq!(reaction(&m).kind, Fx::Shimmer, "{md:?}");
}

/// With nothing to react to, drawing him asks for no wake-up: idle
/// costs nothing. One running asks for its next frame.
#[test]
fn only_a_reaction_wakes() {
    let mut m = crate::new_model("t");
    step(&mut m);
    assert!(m.kept.peek::<FxClock>().is_none_or(|c| c.last.is_none()));
    m.kept::<crate::finish::Finish>().fx = Reaction {
        kind: Fx::Done,
        frame: 0,
    };
    step(&mut m);
    assert!(m.kept::<FxClock>().last.is_some());
    assert_eq!(reaction(&m).kind, Fx::Done);
}
