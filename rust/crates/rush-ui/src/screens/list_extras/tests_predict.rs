//! Reading ahead what the list shows next (predict.rs): shown at once
//! when picked, topped up when written to since, let go when the list
//! moves away, and nothing read with no loop to show it.

use rush_core::agent::State;
use rush_draw::{App, Canvas, Loop, Msg, Step};

use super::predict::{reads, ready};
use super::tests::{agent, model_of};
use crate::frame::Press;
use crate::model::Model;
use crate::worker::Note;

fn ask(i: usize) -> String {
    format!(
        r#"{{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/src/checkout","message":{{"role":"user","content":"ask {i}"}}}}"#
    ) + "\n"
}

/// Agents a0..a3, each with a two-message conversation kept in dir.
fn readable(dir: &std::path::Path) -> Model {
    let mut agents: Vec<_> = (0..4)
        .map(|i| agent(&format!("a{i}"), &format!("task {i}"), State::Idle, 0))
        .collect();
    for (i, a) in agents.iter_mut().enumerate() {
        let p = dir.join(format!("{}.jsonl", a.job.id));
        std::fs::write(&p, ask(0) + &ask(1)).unwrap();
        a.kind = rush_core::adapter::Kind::from("claude");
        a.job.transcript_path = p.to_string_lossy().into_owned();
        a.job.updated_at -= i as i64 * 1000;
    }
    let mut m = model_of(agents, 120, 40);
    let reg = rush_core::adapter::Registry::new();
    reg.register(rush_claude::Claude::new());
    m.reg = reg;
    m
}

fn press(m: &mut Model, s: &str) {
    m.press_key(&Press::of(s).unwrap());
}

/// Runs the model's loop until done says so.
fn run_until(m: &mut Model, lp: Loop<Note>, done: impl Fn(&mut Model) -> bool) {
    struct Until<'a, F>(&'a mut Model, F);
    impl<F: Fn(&mut Model) -> bool> App<Note> for Until<'_, F> {
        fn update(&mut self, msg: Msg<Note>) -> Step {
            let s = self.0.update(msg);
            if (self.1)(self.0) { Step::Quit } else { s }
        }
        fn draw(&mut self, c: &mut Canvas) {
            self.0.draw(c);
        }
    }
    if done(m) {
        return;
    }
    lp.run_with(&mut Until(m, done), |_| Ok(())).unwrap();
}

/// The rows' keys in the list's order.
fn keys(m: &Model) -> Vec<String> {
    m.items()
        .into_iter()
        .filter(|k| !k.starts_with('§'))
        .collect()
}

#[test]
fn what_is_read_ahead_shows_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let lp: Loop<Note> = Loop::new();
    let mut m = readable(dir.path());
    m.tx = Some(lp.sender());
    let k = keys(&m);
    m.sel = k[0].clone();
    // Down: the one picked (enter opens it) and the next way it moved.
    press(&mut m, "down");
    let want = [k[1].clone(), k[2].clone()];
    run_until(&mut m, lp, |m| {
        let r = ready(m);
        want.iter().all(|k| r.contains(k))
    });
    press(&mut m, "enter");
    let v = m.session.as_ref().expect("opened");
    assert_eq!(v.key, k[1]);
    assert!(v.read, "shown with the key, not read after it");
    assert_eq!(v.sess.turns.len(), 2);
    assert!(!ready(&mut m).contains(&k[1]), "taken");
}

#[test]
fn one_written_to_since_is_topped_up_first() {
    let dir = tempfile::tempdir().unwrap();
    let lp: Loop<Note> = Loop::new();
    let mut m = readable(dir.path());
    m.tx = Some(lp.sender());
    let k = keys(&m);
    m.sel = k[0].clone();
    press(&mut m, "down");
    let next = k[2].clone();
    let lp2: Loop<Note> = Loop::new();
    let tx2 = lp2.sender();
    run_until(&mut m, lp, |m| ready(m).contains(&next));
    m.tx = Some(tx2);
    let i = m.agents.iter().position(|a| a.key == next).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&m.agents[i].job.transcript_path)
        .unwrap()
        .write_all(ask(2).as_bytes())
        .unwrap();
    press(&mut m, "down");
    press(&mut m, "enter");
    let v = m.session.as_ref().expect("opened");
    assert_eq!(v.key, next);
    assert!(!v.read, "not shown as it was read");
    run_until(&mut m, lp2, |m| m.session.as_ref().is_some_and(|v| v.read));
    assert_eq!(m.session.as_ref().unwrap().sess.turns.len(), 3);
}

#[test]
fn moving_away_lets_them_go() {
    let dir = tempfile::tempdir().unwrap();
    let lp: Loop<Note> = Loop::new();
    let mut m = readable(dir.path());
    m.tx = Some(lp.sender());
    let k = keys(&m);
    m.sel = k[0].clone();
    press(&mut m, "down");
    let next = k[2].clone();
    run_until(&mut m, lp, |m| ready(m).contains(&next));
    press(&mut m, "up");
    assert!(!ready(&mut m).contains(&next));
}

#[test]
fn nothing_is_read_ahead_without_a_loop() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = readable(dir.path());
    let k = keys(&m);
    m.sel = k[0].clone();
    press(&mut m, "down");
    press(&mut m, "down");
    assert!(ready(&mut m).is_empty());
}

/// ctrl+k's pick is read ahead as it's typed, so enter shows it at once.
#[test]
fn the_bars_pick_is_read_ahead() {
    let dir = tempfile::tempdir().unwrap();
    let lp: Loop<Note> = Loop::new();
    let mut m = readable(dir.path());
    m.tx = Some(lp.sender());
    let k = keys(&m);
    m.sel = k[0].clone();
    press(&mut m, "ctrl+k");
    for c in "task 3".chars() {
        let k = if c == ' ' {
            "space".into()
        } else {
            c.to_string()
        };
        press(&mut m, &k);
    }
    let want = m
        .agents
        .iter()
        .find(|a| a.display_name == "task 3")
        .unwrap()
        .key
        .clone();
    run_until(&mut m, lp, |m| ready(m).contains(&want));
    press(&mut m, "enter");
    let v = m.session.as_ref().expect("opened");
    assert_eq!(v.key, want);
    assert!(v.read, "shown with the key, not read after it");
}

/// The Session beside the list is kept as the pick moves off it, still
/// following its file: going back shows it as it was, not read again.
#[test]
fn the_one_just_left_is_kept_for_going_back() {
    let dir = tempfile::tempdir().unwrap();
    let lp: Loop<Note> = Loop::new();
    let mut m = readable(dir.path());
    m.w = 200;
    m.h = 50;
    m.tx = Some(lp.sender());
    let k = keys(&m);
    m.sel = k[0].clone();
    m.frame();
    let first = m.pane.as_ref().map(|v| (v.key.clone(), v.r#gen));
    assert_eq!(first.as_ref().map(|f| f.0.as_str()), Some(k[0].as_str()));
    press(&mut m, "down");
    m.frame();
    assert_eq!(m.pane.as_ref().map(|v| v.key.as_str()), Some(k[1].as_str()));
    press(&mut m, "up");
    m.frame();
    let back = m.pane.as_ref().map(|v| (v.key.clone(), v.r#gen));
    assert_eq!(back, first, "the same view, not read again");
}

/// A row that can't be read is read once, not over and over.
#[test]
fn one_that_cant_be_read_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let lp: Loop<Note> = Loop::new();
    let mut m = readable(dir.path());
    m.tx = Some(lp.sender());
    let k = keys(&m);
    m.sel = k[0].clone();
    for key in &k[1..3] {
        let a = m.agents.iter().find(|a| a.key == *key).unwrap();
        std::fs::remove_file(&a.job.transcript_path).unwrap();
    }
    press(&mut m, "down");
    run_until(&mut m, lp, |m| !reads(m).1);
    // The two that can't be read and the one after them, once each.
    assert_eq!(reads(&mut m), (3, false), "each tried once");
    assert_eq!(ready(&mut m), vec![k[3].clone()]);
}
