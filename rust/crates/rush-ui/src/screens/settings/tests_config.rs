//! config.rs's saves as Settings sees them: a change made through the
//! frame's save or hold reaches the copy of the config Settings holds.

use crate::config::{hold, hold_writers, queue, save, write_waiting};
use crate::screens::accounts::askrush::on_config;
use crate::screens::settings::{config, config_mut, use_store};

/// A change is made in Settings' copy of the config too, so a setting
/// changed there afterwards doesn't save the old value back.
#[test]
fn a_change_reaches_settings_copy() {
    let mut m = crate::new_model("t");
    use_store(&mut m, rush_state::Store::load());
    let mark = "settings-copy-test".to_string();
    let f = mark.clone();
    save(&mut m, "couldn't", move |c| {
        c.onboarding.steps.push(f.clone())
    });
    let held = &config_mut(&mut m).onboarding.steps;
    assert!(held.contains(&mark), "{held:?}");
    let disk = rush_state::Store::load().config.onboarding.steps;
    assert!(disk.contains(&mark), "{disk:?}");
}

/// A change kept off the UI is held in rush's one config too, so a
/// whole save after doesn't write the old value back.
#[test]
fn a_kept_change_is_held() {
    let mut m = crate::new_model("t");
    use_store(&mut m, rush_state::Store::load());
    hold(&mut m, |c| c.set_view("agent"));
    assert_eq!(config(&m).view, "agent");
}

/// config.json taken in from outside while a change of rush's waits
/// to be written keeps that change.
#[test]
fn an_outside_reload_keeps_waiting_changes() {
    let mut m = crate::new_model("t");
    use_store(&mut m, rush_state::Store::load());
    {
        // No writer takes it meanwhile (other tests write too).
        let _g = hold_writers();
        queue(|c| c.onboarding.steps.push("reload-test-waiting".into()));
        on_config(&mut m, br#"{"groupBy":"agent"}"#);
    }
    let c = config(&m);
    assert_eq!(c.group_by, "agent");
    assert!(
        c.onboarding
            .steps
            .iter()
            .any(|s| s == "reload-test-waiting")
    );
    write_waiting().unwrap();
}
