//! Opening the agent you were in when rush last closed (reopen.rs): a
//! recent session in Active opens again; one closed too long ago, gone,
//! hibernated, a list, the setting off or rush opened on something else
//! doesn't; a reload keeps nothing as it goes.

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;
use rush_state::lastview::{self, LastView};

use super::reopen::{self, opened_explicitly, reopen, save_on_close};
use crate::model::Model;
use crate::screens::social::services::Reload;

const KEY: &str = "claude:x:a";
const MINUTE: Ms = 60_000;

/// A rush session at work: Active's Working section.
fn working() -> Agent {
    let mut a = Agent {
        key: KEY.into(),
        display_name: "fix the parser".into(),
        pid: 7,
        rush: true,
        ..Default::default()
    };
    a.job.state = State::Working;
    a
}

/// A model with the list read: agents in it.
fn listed(agents: Vec<Agent>) -> Model {
    let mut m = crate::kit::model(120, 40);
    let roots = vec![String::new(); agents.len()];
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots,
        ..Default::default()
    });
    m
}

/// What a view that closed `ago` before the model's clock left: KEY's
/// session open.
fn closed(m: &Model, ago: Ms) -> LastView {
    LastView {
        place: "Agents".into(),
        session: KEY.into(),
        view: "conversation".into(),
        at: m.now - ago,
        pid: 999_999_999,
        ..Default::default()
    }
}

#[test]
fn reopens_a_recent_active_session() {
    let mut m = listed(vec![working()]);
    let rec = closed(&m, 5 * MINUTE);
    assert!(reopen(&mut m, Some(rec), 0));
    assert_eq!(m.sel, KEY);
    assert_eq!(m.session.as_ref().map(|s| s.key.as_str()), Some(KEY));
}

#[test]
fn not_when_closed_over_half_an_hour_ago() {
    let mut m = listed(vec![working()]);
    let rec = closed(&m, 31 * MINUTE);
    assert!(!reopen(&mut m, Some(rec), 0));
    assert!(m.session.is_none());
    // Its own setting, longer, still reopens it.
    m.cfg.reopen_minutes = 60;
    let rec = closed(&m, 31 * MINUTE);
    assert!(reopen(&mut m, Some(rec), 0));
}

#[test]
fn not_when_the_session_is_gone() {
    let mut other = working();
    other.key = "claude:x:other".into();
    let mut m = listed(vec![other]);
    let rec = closed(&m, MINUTE);
    assert!(!reopen(&mut m, Some(rec), 0));
    assert!(m.session.is_none());
}

#[test]
fn not_when_the_session_is_hibernated() {
    // Finished 40 minutes ago, its process stopped: still listed, but
    // hibernated after 15.
    let mut a = working();
    a.pid = 0;
    a.job.state = State::Done;
    let mut m = crate::kit::model(120, 40);
    a.job.updated_at = m.clock.unwrap() - 40 * MINUTE;
    m.set_fleet(crate::worker::Fleet {
        agents: vec![a],
        roots: vec![String::new()],
        ..Default::default()
    });
    let rec = closed(&m, MINUTE);
    assert!(!reopen(&mut m, Some(rec), 15));
    assert!(m.session.is_none());
}

#[test]
fn not_when_put_away() {
    let mut a = working();
    a.done = true;
    let mut m = listed(vec![a]);
    let rec = closed(&m, MINUTE);
    assert!(!reopen(&mut m, Some(rec), 0));
}

#[test]
fn not_when_the_setting_is_off() {
    let mut m = listed(vec![working()]);
    m.cfg.reopen_minutes = -1;
    let rec = closed(&m, MINUTE);
    assert!(!reopen(&mut m, Some(rec), 0));
    assert!(m.session.is_none());
}

#[test]
fn not_when_it_was_a_list() {
    let mut m = listed(vec![working()]);
    let mut rec = closed(&m, MINUTE);
    rec.session.clear();
    assert!(!reopen(&mut m, Some(rec.clone()), 0));
    rec.session = KEY.into();
    rec.place = "Projects".into();
    assert!(!reopen(&mut m, Some(rec), 0));
    assert!(m.session.is_none());
}

#[test]
fn opening_rush_on_something_wins() {
    let mut m = listed(vec![working()]);
    opened_explicitly(&mut m);
    let rec = closed(&m, MINUTE);
    assert!(!reopen(&mut m, Some(rec), 0));
    assert!(m.session.is_none());
}

#[test]
fn not_once_you_have_started_on_something() {
    let mut m = listed(vec![working()]);
    m.last_input = m.now;
    let rec = closed(&m, MINUTE);
    assert!(!reopen(&mut m, Some(rec), 0));
}

/// Closing keeps where the view was in rush's folder; a reload, which
/// hands its place on itself, keeps nothing.
#[test]
fn closing_keeps_the_place_and_a_reload_does_not() {
    let d = tempfile::tempdir().unwrap();
    let mut m = listed(vec![working()]);
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let rec = closed(&m, MINUTE);
    assert!(reopen(&mut m, Some(rec), 0));
    save_on_close(&mut m);
    let got = lastview::load().expect("kept");
    assert_eq!(got.session, KEY);
    assert_eq!(got.place, "Agents");
    assert_eq!(got.view, "conversation");
    assert!(!got.open);
    assert_eq!(reopen::snapshot(&m).session, KEY);

    std::fs::remove_file(lastview::path()).unwrap();
    m.kept::<Reload>().reloading = true;
    save_on_close(&mut m);
    assert!(lastview::load().is_none(), "a reload isn't a close");
    rush_state::set_thread_dir(None);
}
