//! lead.rs's tests (rush-ui-base): whether a rush leads asks hosted.rs
//! whether it's a hosted view, through Ext::calls.

use crate::lead::*;

#[test]
fn leads_unless_it_follows() {
    let mut m = crate::new_model("t");
    m.follows = false;
    assert!(m.leads() && !m.following());
    m.follows = true;
    assert!(!m.leads() && m.following());
}

// A rush that follows never refreshes a sign-in on its own: an expired
// one reads as expired, for the rush that leads to refresh.
#[test]
fn following_never_renews() {
    let mut m = crate::new_model("t");
    m.follows = true;
    let leads = renews(&m);
    assert!(!leads());
    let got = rush_claude::oauth::or_renew_if(
        leads(),
        "login:test-following",
        || Err(rush_claude::oauth::OauthError::Expired),
        || panic!("a following rush renewed a sign-in"),
    );
    assert_eq!(got, Err(rush_claude::oauth::OauthError::Expired));
    m.follows = false;
    assert!(renews(&m)());
}
