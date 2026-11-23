//! chores.rs's tests (rush-ui-base): they need the screens' chores,
//! which register pushes into Ext::chores.

use std::time::Duration;

use crate::chores::*;

#[test]
fn at_gos_seconds() {
    let m = crate::new_model("t");
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
    let mut m = crate::new_model("t");
    assert!(due_at(&m).is_none());
    start(&mut m);
    let s = m.kept.peek::<Clock>().unwrap().start.unwrap();
    m.follows = true;
    m.kept::<Clock>().through = 3;
    assert_eq!(due_at(&m), Some(s + Duration::from_secs(33)));
    m.follows = false;
    assert_eq!(due_at(&m), Some(s + Duration::from_secs(15)));
}
