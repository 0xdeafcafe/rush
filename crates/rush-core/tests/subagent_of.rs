//! Which hosted sessions are another's subagents, as their Meta says
//! (rush_core::agent::subagent_of).

use rush_core::agent::subagent_of;

#[test]
fn started_beside_is_never_a_subagent() {
    // (spawnedBy, startedFrom, the session it's a subagent of)
    let cases: [(Option<&str>, Option<&str>, Option<&str>); 7] = [
        (None, None, None),
        (Some("p1"), None, Some("p1")),
        (Some("p1"), Some(""), Some("p1")),
        (Some(""), None, None),
        (None, Some("p1"), None),
        // `rush session start --meta startedFrom=p1` from p1's shell.
        (Some("p1"), Some("p1"), None),
        (Some("p2"), Some("p1"), None),
    ];
    for (by, from, want) in cases {
        assert_eq!(
            subagent_of(by, from),
            want,
            "spawnedBy {by:?}, startedFrom {from:?}"
        );
    }
}
