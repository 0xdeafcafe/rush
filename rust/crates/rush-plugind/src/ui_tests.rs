//! The broker between plugins and rush's windows.
//! Ported from internal/plugind/ui_test.go.

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use rush_plugin::rpc::Handler;
use rush_plugin::{
    CommandSpec, Conn, INTERCEPT_BUDGET, INTERCEPT_STRIKE_OUT, Intercept, InterceptResult,
    MAX_SECTIONS, Manifest, SettingSpec, UI_EVENTS, UI_INPUT, UI_INTERCEPT, UI_NOTIFY, UI_OVERVIEW,
    UI_SEND, UI_VIEWS, UiEvent,
};
use serde_json::{Value, json};

use crate::testutil::{Home, add_plugin, attach_ui, limited, sess, test_broker};
use crate::ui::{NOTIFY_BURST, Outbox, PLUGIN_QUEUE, SENDS_PER_MIN};

fn m(name: &str, ui: &[&str]) -> Manifest {
    Manifest {
        name: name.into(),
        command: vec!["bin".into()],
        ui: ui.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn ev(kind: &str, s: Option<rush_plugin::UiSession>, text: &str) -> UiEvent {
    UiEvent {
        kind: kind.into(),
        ui: "main".into(),
        session: s,
        text: text.into(),
        ..Default::default()
    }
}

fn denied(e: &rush_plugin::CallError) -> bool {
    e.rpc()
        .is_some_and(|e| e.code == rush_plugin::rpc::CODE_DENIED)
}

fn cmd(name: &str) -> CommandSpec {
    CommandSpec {
        name: name.into(),
        description: "Go.".into(),
        ..Default::default()
    }
}

#[test]
fn events_go_only_where_they_may() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let evp = add_plugin(&b, m("ev", &[UI_EVENTS]), None);
    let inp = add_plugin(&b, m("in", &[UI_INPUT]), None);
    let none = add_plugin(&b, m("none", &[UI_OVERVIEW]), None);
    let u = attach_ui(&b, "main");
    let s = sess("s1", "/tmp");
    u.events(&[
        ev(rush_plugin::EV_TURN_STARTED, s.clone(), "leaked"),
        ev(rush_plugin::EV_INPUT_CHANGED, s.clone(), "my secret"),
        ev(rush_plugin::EV_TURN_ENDED, s, ""),
        ev(rush_plugin::EV_NETWORK_UP, None, ""),
    ]);
    let got = |f: &crate::testutil::FakePlugin, n: usize| -> Vec<UiEvent> {
        (0..n)
            .map(|_| {
                f.events
                    .recv_timeout(Duration::from_secs(2))
                    .expect("too few events")
            })
            .collect()
    };
    let evs = got(&evp, 3);
    assert_eq!(evs[0].kind, rush_plugin::EV_TURN_STARTED);
    assert_eq!(evs[0].text, "");
    assert_eq!(evs[0].session.as_ref().unwrap().id, "s1");
    assert_eq!(evs[1].kind, rush_plugin::EV_TURN_ENDED);
    assert_eq!(evs[2].kind, rush_plugin::EV_NETWORK_UP);
    let ins = got(&inp, 1);
    assert_eq!(ins[0].kind, rush_plugin::EV_INPUT_CHANGED);
    assert_eq!(ins[0].text, "my secret");
    // Each plugin's events go in order, so with the last one in, nothing
    // more is on its way to the ones that may hear; and none was ever sent
    // to the one that may not.
    for f in [&evp, &inp, &none] {
        assert!(f.events.try_recv().is_err(), "{} heard more", f.r.name);
    }
}

#[test]
fn ui_calls_need_their_capability() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let mut bare = m("bare", &[]);
    bare.commands = vec![cmd("go")];
    let f = add_plugin(&b, bare, None);
    for (method, params) in [
        ("ui.overview.set", json!({"session": "s1", "sections": []})),
        ("ui.status.set", json!({"session": "s1", "text": "hi"})),
        ("ui.notify", json!({"text": "hi"})),
        (
            "ui.input.set",
            json!({"ui": "main", "session": "s1", "text": "hi"}),
        ),
        ("ui.send", json!({"session": "s1", "text": "hi"})),
        (
            "ui.view.set",
            json!({"session": "s1", "view": {"name": "ci", "title": "CI"}}),
        ),
        ("ui.view.clear", json!({"session": "s1", "name": "ci"})),
    ] {
        let e = f.call(method, &params).unwrap_err();
        assert!(denied(&e), "{method} without its capability: {e:?}");
    }
    let out = f.call("ui.settings.get", &()).unwrap();
    assert!(out["values"].is_object(), "ui.settings.get = {out}");
}

#[test]
fn overview_goes_with_the_plugin() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let f = add_plugin(&b, m("ov", &[UI_OVERVIEW]), None);
    let u = attach_ui(&b, "main");
    let sections = json!([{"id": "ci", "title": "CI",
        "lines": [{"text": "\x1b[31mred\x1b[0m", "tone": "bad", "url": "javascript:x"}]}]);
    f.call(
        "ui.overview.set",
        &json!({"session": "s1", "sections": sections}),
    )
    .unwrap();
    f.call(
        "ui.status.set",
        &json!({"session": "s1", "text": "passing", "tone": "loud"}),
    )
    .unwrap();
    let st = u.wait_state("the section and status", |st| {
        st.sections.get("s1").is_some_and(|s| s.len() == 1)
            && st.statuses.get("s1").is_some_and(|s| s.len() == 1)
    });
    let s = &st.sections["s1"][0];
    let l = &s.section.lines[0];
    assert!(
        s.plugin == "ov" && !l.text.contains('\x1b') && l.url.is_empty() && l.tone == "bad",
        "section = {s:?}"
    );
    let status = &st.statuses["s1"][0];
    assert!(
        status.status.text == "passing" && status.status.tone.is_empty(),
        "{status:?}"
    );
    assert!(
        st.plugins.len() == 1 && st.plugins[0].name == "ov",
        "{:?}",
        st.plugins
    );
    let many: Vec<Value> = (0..=MAX_SECTIONS)
        .map(|i| json!({"id": format!("s{i}"), "title": "T"}))
        .collect();
    assert!(
        f.call(
            "ui.overview.set",
            &json!({"session": "s1", "sections": many})
        )
        .is_err(),
        "took too many sections"
    );

    let conn = f.r.live().1.unwrap();
    f.r.down(&conn);
    u.wait_state("nothing of the stopped plugin", |st| {
        st.sections.is_empty() && st.statuses.is_empty() && st.plugins.is_empty()
    });
}

/// A plugin allowed views draws them beside the session's conversation,
/// named as its own: it can't take the agent's, nor say another drew it,
/// and keeps to its share.
#[test]
fn views() {
    let home = Home::new();
    let (b, hosts) = test_broker(&home);
    hosts.views.lock().unwrap().insert(
        "s1".into(),
        vec![
            json!({"name": "ci", "title": "The agent's"})
                .as_object()
                .unwrap()
                .clone(),
        ],
    );
    let f = add_plugin(&b, m("ci", &[UI_VIEWS]), None);
    let set = |name: &str| {
        f.call(
            "ui.view.set",
            &json!({"session": "s1", "view": {"name": name, "title": "Checks", "by": "someone"}}),
        )
    };
    set("ci").unwrap();
    let vs = hosts.views.lock().unwrap()["s1"].clone();
    assert_eq!(vs.len(), 2, "{vs:?}");
    assert_eq!(vs[0]["name"], "p-ci--ci");
    assert_eq!(vs[0]["by"], "ci");
    assert_eq!(vs[1]["title"], "The agent's");
    assert!(set("Bad Name").is_err(), "took a bad name");
    set("two").unwrap();
    assert!(set("three").is_err(), "kept more than its share");
    set("ci").expect("updating one it has");
    f.call("ui.view.clear", &json!({"session": "s1", "name": "ci"}))
        .unwrap();
    assert_eq!(hosts.views.lock().unwrap()["s1"].len(), 2);
}

#[test]
fn overview_clears() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let f = add_plugin(&b, m("ov", &[UI_OVERVIEW]), None);
    let u = attach_ui(&b, "main");
    f.call(
        "ui.overview.set",
        &json!({"session": "s1", "sections": [{"id": "a", "title": "A"}]}),
    )
    .unwrap();
    u.wait_state("the section", |st| {
        st.sections.get("s1").is_some_and(|s| s.len() == 1)
    });
    f.call("ui.overview.set", &json!({"session": "s1", "sections": []}))
        .unwrap();
    u.wait_state("no section", |st| st.sections.is_empty());
}

fn answer(f: impl Fn(Intercept) -> InterceptResult + Send + Sync + 'static) -> Handler {
    Arc::new(move |_, _, method, params| {
        if method != "ui.intercept" {
            return Ok(Some(crate::raw("{}")));
        }
        let i: Intercept = rush_plugin::rpc::from_raw(params).unwrap();
        Ok(Some(crate::to_raw(&f(i))))
    })
}

fn act(action: &str, text: &str, reason: &str) -> InterceptResult {
    InterceptResult {
        action: action.into(),
        text: text.into(),
        reason: reason.into(),
        ..Default::default()
    }
}

#[test]
fn intercept_chains_and_blocks() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let both = &[UI_INPUT, UI_INTERCEPT];
    let asked = Arc::new(AtomicI32::new(0));
    let _a = add_plugin(
        &b,
        m("a", both),
        Some(answer(|i| act("rewrite", &format!("{} [a]", i.text), ""))),
    );
    let _b = add_plugin(
        &b,
        m("b", both),
        Some(answer(|i| act("rewrite", &i.text.to_uppercase(), ""))), // sees a's
    );
    let _c = add_plugin(
        &b,
        m("c", both),
        Some(answer(|i| {
            if i.text.contains("STOP") {
                act("block", "", "no\x1b stopping")
            } else {
                act("allow", "", "")
            }
        })),
    );
    let n = asked.clone();
    let _d = add_plugin(
        &b,
        m("d", &[UI_INPUT]),
        Some(answer(move |_| {
            n.fetch_add(1, Ordering::SeqCst);
            act("block", "", "")
        })),
    );
    let u = attach_ui(&b, "main");
    let req = |text: &str| json!({"hook": "before-send", "ui": "main", "text": text});
    let res: InterceptResult =
        serde_json::from_value(u.call("ui.intercept", &req("hello")).unwrap()).unwrap();
    assert_eq!(
        (res.action.as_str(), res.text.as_str(), res.plugin.as_str()),
        ("rewrite", "HELLO [A]", "a, b"),
        "chained"
    );
    let res: InterceptResult =
        serde_json::from_value(u.call("ui.intercept", &req("stop")).unwrap()).unwrap();
    assert_eq!(
        (
            res.action.as_str(),
            res.plugin.as_str(),
            res.reason.as_str()
        ),
        ("block", "c", "no stopping"),
        "blocked"
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "a plugin without intercept was asked"
    );
}

#[test]
fn intercept_timeouts_strike_out() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let asked = Arc::new(AtomicI32::new(0));
    // It answers only once it's let go, which is after every ask here.
    let (release, held) = mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let n = asked.clone();
    let h: Handler = Arc::new(move |_, _, _, _| {
        n.fetch_add(1, Ordering::SeqCst);
        let _ = held.lock().unwrap().recv();
        Ok(Some(crate::to_raw(&act("block", "", ""))))
    });
    let _slow = add_plugin(&b, m("slow", &[UI_INPUT, UI_INTERCEPT]), Some(h));
    let u = attach_ui(&b, "main");
    for i in 0..=INTERCEPT_STRIKE_OUT {
        let began = Instant::now();
        let res: InterceptResult =
            serde_json::from_value(u.call("ui.intercept", &json!({"text": "hi"})).unwrap())
                .unwrap();
        assert_eq!(res.action, "allow", "a plugin that didn't answer held it");
        let took = began.elapsed();
        assert!(
            took <= INTERCEPT_BUDGET + Duration::from_millis(150),
            "call {i} took {took:?}"
        );
    }
    assert_eq!(
        asked.load(Ordering::SeqCst),
        INTERCEPT_STRIKE_OUT as i32,
        "asked before it's skipped"
    );
    u.wait_state("slow skipped", |st| {
        st.plugins.len() == 1 && !st.plugins[0].skipped.is_empty()
    });
    // Restarted, it's asked again.
    b.0.ui.gone("slow");
    let _ = u.call("ui.intercept", &json!({"text": "hi"}));
    assert_eq!(
        asked.load(Ordering::SeqCst),
        INTERCEPT_STRIKE_OUT as i32 + 1,
        "not asked again after a restart"
    );
    drop(release);
}

#[test]
fn send_reaches_only_its_workspaces() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let ws = home.mkdir("ws");
    let app = home.mkdir("ws/app");
    let elsewhere = home.mkdir("elsewhere");
    let mut man = m("s", &[UI_EVENTS, UI_SEND, UI_NOTIFY, UI_INPUT]);
    man.workspaces = vec![ws];
    let f = add_plugin(&b, man, None);
    let u = attach_ui(&b, "main");
    u.events(&[
        ev(rush_plugin::EV_SESSION_OPENED, sess("in", &app), ""),
        ev(rush_plugin::EV_SESSION_OPENED, sess("out", &elsewhere), ""),
    ]);
    f.call("ui.send", &json!({"session": "in", "text": "continue"}))
        .unwrap();
    let d = u.wait_do();
    assert!(
        d.kind == "send" && d.session == "in" && d.text == "continue" && d.plugin == "s",
        "ui.do = {d:?}"
    );
    for id in ["out", "never-seen"] {
        let e = f
            .call("ui.send", &json!({"session": id, "text": "continue"}))
            .unwrap_err();
        assert!(denied(&e), "sent to {id}: {e:?}");
    }
    let e = f
        .call("ui.send", &json!({"session": "in", "text": "  "}))
        .unwrap_err();
    assert!(!denied(&e), "sent nothing: {e:?}");
    // It can't send without limit.
    let mut sent = 1;
    let mut err = None;
    for _ in 0..SENDS_PER_MIN {
        match f.call("ui.send", &json!({"session": "in", "text": "again"})) {
            Ok(_) => sent += 1,
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    assert!(
        err.as_ref().is_some_and(limited) && sent == SENDS_PER_MIN,
        "sent {sent} in a minute: {err:?}"
    );
    for _ in 0..sent - 1 {
        u.wait_do();
    }

    // Notices are cleaned and limited too.
    for i in 0..NOTIFY_BURST as usize {
        f.call(
            "ui.notify",
            &json!({"text": format!("hi\x1b{i}"), "tone": "good"}),
        )
        .unwrap();
    }
    let e = f
        .call("ui.notify", &json!({"text": "one more"}))
        .unwrap_err();
    assert!(limited(&e), "notices without limit: {e:?}");
    let d = u.wait_do();
    assert!(
        d.kind == "notify" && d.text == "hi0" && d.tone == "good",
        "notice = {d:?}"
    );
    f.call(
        "ui.input.set",
        &json!({"ui": "main", "session": "in", "text": "draft"}),
    )
    .unwrap();
}

#[test]
fn slow_plugin_holds_up_no_one() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    // a-slow's end stops reading at its first event, until let go.
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(Some(held));
    let (done_tx, done) = mpsc::channel::<usize>();
    let got = Arc::new(AtomicI32::new(0));
    let g = got.clone();
    let done_tx = Mutex::new(done_tx);
    let slow_h: Handler = Arc::new(move |_, _, _, params| {
        if let Some(h) = held.lock().unwrap().take() {
            let _ = h.recv();
        }
        let n = g.fetch_add(1, Ordering::SeqCst) + 1;
        let e: UiEvent = rush_plugin::rpc::from_raw(params).unwrap();
        if e.kind == rush_plugin::EV_NETWORK_UP {
            let _ = done_tx.lock().unwrap().send(n as usize);
        }
        Ok(None)
    });
    let slow = add_plugin(&b, m("a-slow", &[UI_EVENTS]), None);
    // Its end is replaced by one that answers as slow_h does.
    let (mine, theirs) = UnixStream::pair().unwrap();
    let pc = Conn::new(theirs, Some(slow_h)).unwrap();
    let me = Arc::downgrade(&slow.r);
    let bh: Handler = Arc::new(move |_, ctx, method, params| match me.upgrade() {
        Some(r) => r.answer_plugin(ctx, method, params),
        None => Err(crate::server("gone")),
    });
    let bc = Conn::new(mine, Some(bh)).unwrap();
    slow.r.set_running(slow.r.live().0, bc.clone());
    let fast = add_plugin(&b, m("b-fast", &[UI_EVENTS]), None);
    let u = attach_ui(&b, "main");

    // Far more than its queue, the socket and the plugin's own read buffer
    // hold between them.
    const TOTAL: usize = 5000;
    let began = Instant::now();
    for i in 0..TOTAL - 1 {
        u.conn
            .notify(
                "ui.event",
                &ev(
                    rush_plugin::EV_TURN_STARTED,
                    sess(&format!("s{i}"), "/tmp"),
                    "",
                ),
            )
            .unwrap();
    }
    u.conn
        .notify("ui.event", &ev(rush_plugin::EV_NETWORK_UP, None, ""))
        .unwrap();
    let took = began.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "handing over events took {took:?} with a plugin stuck"
    );
    loop {
        let e = fast
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("the fast plugin never heard the last event");
        if e.kind == rush_plugin::EV_NETWORK_UP {
            break;
        }
    }
    drop(release);
    let n = done
        .recv_timeout(Duration::from_secs(3))
        .expect("the slow plugin never caught up");
    // Go's test pipe holds nothing; here the socket and the plugin's read
    // buffer hold some too, so what's bounded is what waits for it.
    assert!(
        n < TOTAL / 2,
        "slow plugin got {n} events: its queue isn't bounded"
    );
    let out = slow.r.lock().out.clone().unwrap();
    assert!(out.lock().dropped > 0, "nothing was dropped");
    assert!(out.lock().q.len() <= PLUGIN_QUEUE);
    pc.close();
    bc.close();
}

#[test]
fn commands_and_settings() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let mut man = m("cmd", &[]);
    man.commands = vec![cmd("go")];
    man.settings = vec![SettingSpec {
        key: "on".into(),
        title: "On".into(),
        kind: "bool".into(),
        ..Default::default()
    }];
    let dir = rush_plugin::root().join("cmd");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        rush_state::json::to_vec(&man).unwrap(),
    )
    .unwrap();
    let p = rush_plugin::load(&dir).unwrap();
    rush_plugin::approve(&p).unwrap();
    let f = add_plugin(&b, man, None);
    let u = attach_ui(&b, "main");

    u.call(
        "ui.command",
        &json!({"plugin": "cmd", "command": "go", "ui": "main", "session": sess("s1", "/tmp")}),
    )
    .unwrap();
    let got = f.other.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        got.starts_with("ui.command ") && got.contains(r#""s1""#),
        "plugin got {got}"
    );
    assert!(
        u.call("ui.command", &json!({"plugin": "cmd", "command": "rm"}))
            .is_err(),
        "ran a command it doesn't declare"
    );

    assert!(
        u.call(
            "ui.settings.set",
            &json!({"plugin": "cmd", "key": "on", "value": "maybe"})
        )
        .is_err(),
        "took a bad value"
    );
    u.call(
        "ui.settings.set",
        &json!({"plugin": "cmd", "key": "on", "value": "true"}),
    )
    .unwrap();
    let got = f
        .other
        .recv_timeout(Duration::from_secs(2))
        .expect("plugin wasn't told its settings");
    assert_eq!(got, r#"ui.settings {"values":{"on":"true"}}"#);
    u.wait_state("the new value", |st| {
        st.plugins.len() == 1
            && st.plugins[0].values.get("on").is_some_and(|v| v == "true")
            && st.plugins[0].commands.len() == 1
    });

    // initialize tells it its ui, commands and settings.
    let (mine, theirs) = UnixStream::pair().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let pc = Conn::new(
        theirs,
        Some(Arc::new(move |_, _, _, params| {
            let _ = tx
                .lock()
                .unwrap()
                .send(params.map(|p| p.get().to_string()).unwrap_or_default());
            Ok(Some(crate::raw("{}")))
        })),
    )
    .unwrap();
    let bc = Conn::new(mine, None).unwrap();
    f.r.handshake(&p, &bc).unwrap();
    let init: Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
    assert_eq!(init["commands"], json!(["go"]));
    assert_eq!(init["settings"]["on"], "true");
    assert_eq!(init["protocol"], 1);
    pc.close();
    bc.close();
}

/// A plugin with "input" sets a box whole, only if it still holds what it
/// expects, and puts a note on its edge; a command shows the box only to a
/// plugin that may see it.
#[test]
fn box_whole() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let mut bm = m("box", &[UI_INPUT]);
    bm.commands = vec![cmd("go")];
    let f = add_plugin(&b, bm, None);
    let mut blind = m("blind", &[]);
    blind.commands = vec![cmd("go")];
    let g = add_plugin(&b, blind, None);
    let u = attach_ui(&b, "main");

    let bx = json!({"text": "see [Image #1] [Pasted text #2 +4 lines]", "cursor": 3,
        "pastes": {"2": "a\nb\nc\nd"}, "images": {"1": "/tmp/a.png"}});
    f.call(
        "ui.input.set",
        &json!({"ui": "main", "session": "s1", "box": bx, "if": "draft"}),
    )
    .unwrap();
    let d = u.wait_do();
    let bx = d.input_box.as_ref().expect("no box");
    assert!(
        d.kind == "input.set"
            && bx.cursor == 3
            && bx.images[&1] == "/tmp/a.png"
            && !bx.pastes[&2].is_empty()
            && d.if_text.as_deref() == Some("draft"),
        "do: {d:?}"
    );
    let bad = json!({"text": "x", "images": {"1": "a.png"}});
    assert!(
        f.call(
            "ui.input.set",
            &json!({"ui": "main", "session": "s1", "box": bad})
        )
        .is_err(),
        "an image that isn't an absolute path was taken"
    );

    f.call(
        "ui.box.note",
        &json!({"session": "", "text": "stashed · alt+s brings it back", "tone": "warn"}),
    )
    .unwrap();
    u.wait_state("the Prompt's note", |st| {
        st.notes
            .get("")
            .is_some_and(|n| n.len() == 1 && n[0].plugin == "box" && n[0].status.tone == "warn")
    });
    assert!(
        g.call("ui.box.note", &json!({"session": "s1", "text": "hi"}))
            .is_err(),
        "a plugin without input put a note on a box"
    );

    let input = json!({"text": "secret", "cursor": 6});
    for name in ["box", "blind"] {
        u.call(
            "ui.command",
            &json!({"plugin": name, "command": "go", "ui": "main", "box": "s1", "input": input}),
        )
        .unwrap();
    }
    let got = f.other.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        got.contains(r#""secret""#),
        "the plugin with input wasn't shown the box: {got}"
    );
    let got = g.other.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        !got.contains("secret") && got.contains(r#""box":"s1""#),
        "the plugin without input: {got}"
    );
}

/// A plugin shows a pick only in answer to its command in that window, and
/// hears what was chosen.
#[test]
fn pick_answers_a_command() {
    let home = Home::new();
    let (b, _) = test_broker(&home);
    let mut pm = m("pk", &[]);
    pm.commands = vec![cmd("go")];
    let f = add_plugin(&b, pm, None);
    let u = attach_ui(&b, "main");
    let pick = json!({"id": "history", "title": "Drafts", "tabs": ["Sent", "Cleared"],
        "items": [{"id": "a", "text": "one\x1b[31m\ntwo", "tab": 1}],
        "actions": [{"key": "enter", "name": "put back"}, {"key": "ctrl+d", "name": "forget", "stay": true}]});
    assert!(
        f.call("ui.pick", &json!({"ui": "main", "pick": pick}))
            .is_err(),
        "a pick shown out of the blue"
    );
    u.call(
        "ui.command",
        &json!({"plugin": "pk", "command": "go", "ui": "main"}),
    )
    .unwrap();
    f.other.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        f.call("ui.pick", &json!({"ui": "other", "pick": pick}))
            .is_err(),
        "a pick shown in a window that didn't ask"
    );
    f.call("ui.pick", &json!({"ui": "main", "pick": pick}))
        .unwrap();
    let d = u.wait_do();
    let p = d.pick.as_ref().expect("no pick");
    assert!(
        d.kind == "pick"
            && p.id == "history"
            && !p.items[0].text.contains('\x1b')
            && p.items[0].text.contains('\n'),
        "do: {d:?}"
    );
    let bad_key =
        json!({"id": "x", "title": "X", "items": [], "actions": [{"key": "q", "name": "quit"}]});
    assert!(
        f.call("ui.pick", &json!({"ui": "main", "pick": bad_key}))
            .is_err(),
        "an action on a key that types was taken"
    );
    u.call(
        "ui.picked",
        &json!({"plugin": "pk", "pick": "history", "item": "a", "action": "enter", "ui": "main",
            "input": {"text": "secret"}}),
    )
    .unwrap();
    let got = f.other.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        got.starts_with("ui.picked ") && got.contains(r#""item":"a""#) && !got.contains("secret"),
        "plugin got {got}"
    );
}

#[test]
fn outbox_sends_in_order_and_stops() {
    let (a, z) = UnixStream::pair().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let peer = Conn::new(
        z,
        Some(Arc::new(move |_, _, method, _| {
            let _ = tx.lock().unwrap().send(method.to_string());
            Ok(None)
        })),
    )
    .unwrap();
    let c = Conn::new(a, None).unwrap();
    let o = Outbox::new(c.clone(), 8, &crate::Scope::here());
    o.put("one", &1, "");
    o.put("two", &2, "");
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "one");
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "two");
    o.close();
    o.put("three", &3, "");
    assert_eq!(o.lock().q.len(), 1, "closed, it sends nothing more");
    peer.close();
    c.close();
}
