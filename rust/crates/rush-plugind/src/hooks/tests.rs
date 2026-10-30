//! A window's side of the broker, against a fake broker.
//! Ported from internal/hooks/hooks_test.go.

use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_plugin::rpc::Handler;
use rush_plugin::{Conn, INTERCEPT_BUDGET, Intercept, UiDo, UiEvent, UiPlugin, UiState};

use super::{Client, EVENT_QUEUE, Msg};

/// A broker answering a client with state s, and answer for anything else;
/// each connection's broker end comes on the channel.
fn fake_broker(s: UiState, answer: Option<Handler>) -> (Arc<Client>, mpsc::Receiver<Conn>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let scope = crate::Scope::with_events(Arc::new(rush_events::Logger::new(None)));
    let c = Client::with_dial_in(
        Box::new(move |h| {
            let (a, b) = UnixStream::pair().map_err(|e| e.to_string())?;
            let s = s.clone();
            let answer = answer.clone();
            let srv = Conn::new(
                b,
                Some(Arc::new(move |c, ctx, method, params| {
                    if method == "ui.attach" {
                        return Ok(Some(crate::to_raw(&s)));
                    }
                    match &answer {
                        Some(a) => a(c, ctx, method, params),
                        None => Ok(Some(crate::raw("{}"))),
                    }
                })),
            )
            .map_err(|e| e.to_string())?;
            let _ = tx.lock().unwrap().send(srv);
            Conn::new(a, Some(h)).map_err(|e| e.to_string())
        }),
        scope,
    );
    (c, rx)
}

/// Waits for c to have attached: the state it was given comes first.
fn wait_connected(c: &Client) {
    assert_eq!(
        c.next(),
        Some(Msg::State),
        "attach should say the state changed"
    );
    assert!(c.connected(), "never connected");
}

fn events_and_intercept() -> UiState {
    UiState {
        plugins: vec![UiPlugin {
            name: "p".into(),
            ui: vec!["events".into(), "input".into(), "intercept".into()],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn kind(k: &str) -> UiEvent {
    UiEvent {
        kind: k.into(),
        ..Default::default()
    }
}

#[test]
fn emit_never_waits() {
    // A broker that never reads what it's sent.
    let (block_tx, block) = mpsc::channel::<()>();
    let block = Arc::new(Mutex::new(block));
    let h: Handler = Arc::new(move |_, _, _, _| {
        let _ = block.lock().unwrap().recv();
        Ok(None)
    });
    let (c, _brokers) = fake_broker(events_and_intercept(), Some(h));
    c.start();
    wait_connected(&c);
    let start = Instant::now();
    for _ in 0..EVENT_QUEUE * 4 {
        c.emit(kind(rush_plugin::EV_TURN_ENDED));
    }
    let d = start.elapsed();
    assert!(d < Duration::from_millis(50), "emit took {d:?}");
    assert!(
        c.dropped.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "a full queue should drop"
    );
    drop(block_tx);
    c.close();
}

#[test]
fn emit_skips_what_nobody_hears() {
    let s = UiState {
        plugins: vec![UiPlugin {
            name: "p".into(),
            ui: vec!["overview".into()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let (c, _brokers) = fake_broker(s, None);
    c.start();
    wait_connected(&c);
    c.emit(UiEvent {
        kind: rush_plugin::EV_INPUT_CHANGED.into(),
        text: "secret".into(),
        ..Default::default()
    });
    assert_eq!(
        c.queued_events(),
        0,
        "an input event went to a broker with no plugin allowed to see it"
    );
    c.close();
}

#[test]
fn intercept_times_out_to_allow() {
    let h: Handler = Arc::new(|_, ctx, _, _| {
        ctx.wait();
        Ok(Some(crate::raw(r#"{"action":"block"}"#)))
    });
    let (c, _brokers) = fake_broker(events_and_intercept(), Some(h));
    c.start();
    wait_connected(&c);
    let start = Instant::now();
    let r = c.intercept(Intercept {
        hook: "before-send".into(),
        text: "hi".into(),
        ..Default::default()
    });
    assert_eq!(r.action, "allow", "got {r:?}");
    let d = start.elapsed();
    assert!(
        d < INTERCEPT_BUDGET + Duration::from_secs(1),
        "waited {d:?}"
    );
    c.close();
}

#[test]
fn intercept_rewrite() {
    let h: Handler =
        Arc::new(|_, _, _, _| Ok(Some(crate::raw(r#"{"action":"rewrite","text":"HI"}"#))));
    let (c, _brokers) = fake_broker(events_and_intercept(), Some(h));
    c.start();
    wait_connected(&c);
    assert!(c.intercepts(), "should intercept");
    let r = c.intercept(Intercept {
        hook: "before-send".into(),
        text: "hi".into(),
        ..Default::default()
    });
    assert!(r.action == "rewrite" && r.text == "HI", "got {r:?}");
    c.close();
}

#[test]
fn state_and_do_reach_the_ui() {
    let (c, brokers) = fake_broker(events_and_intercept(), None);
    c.start();
    let srv = brokers.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_connected(&c);
    let d = |ui: &str, text: &str| UiDo {
        plugin: "p".into(),
        ui: ui.into(),
        kind: "notify".into(),
        text: text.into(),
        ..Default::default()
    };
    srv.notify("ui.do", &d("", "hello")).unwrap();
    srv.notify("ui.do", &d("someone-else", "not you")).unwrap();
    srv.notify("ui.do", &d(c.id(), "you")).unwrap();
    for want in ["hello", "you"] {
        match c.next() {
            Some(Msg::Do(d)) => assert_eq!(d.text, want),
            other => panic!("got {other:?}"),
        }
    }
    // The broker gone, the window is told nothing is added any more.
    srv.close();
    assert_eq!(c.next(), Some(Msg::State));
    c.close();
    assert_eq!(c.next(), None, "closed, next ends");
}

#[test]
fn fixed_never_connects() {
    let c = Client::fixed(events_and_intercept());
    assert_eq!(c.state().plugins.len(), 1);
    assert!(!c.wants("turn.ended") && !c.intercepts(), "not connected");
    assert_eq!(c.id().len(), 8);
}
