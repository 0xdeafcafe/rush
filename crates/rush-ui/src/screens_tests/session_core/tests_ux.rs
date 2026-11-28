//! The session screen at a glance (Rust-only): one busy session drawn
//! whole, a turn under way with a chain of commands at its second part,
//! a failed step behind it, a subagent and a background shell beside
//! it, and what the screen says of all that. What runs is said once, on
//! the live line; the keys are in the footer; a part of the column with
//! nothing in it isn't drawn.
//!
//! UX_SHOTS=<folder> writes the frames there (busy_200x56.txt and
//! .ansi, busy_260x60), to look at.

use rush_convo::Subagent;
use rush_core::adapter::Registry;
use rush_core::agent::State;
use rush_core::event::{Compacted, Event, Message, Part, PartKind, TaskKind, TaskStarted, TurnEnd};
use rush_core::time::Ms;
use rush_core::tool::{Call, Input, Kind, Output, Patch};
use rush_core::usage::TokenUsage;

use super::conn;
use super::tests_subs::NOW;
use crate::model::Model;
use crate::session::{SessionView, Source};

pub(super) const KEY: &str = "default/k1";

const CHAIN: &str = "cd /src/halo/d3d-frame-tail && \\\ncargo clippy -p d3d -p render-bridge -p game -- -D warnings && \\\nupdate-buck --check && \\\ngit merge origin/master && \\\ngit push origin HEAD && \\\ncargo build --release -p game && \\\n./boot.sh 03a_oldmombasa && \\\n./boot.sh 05a_citadel && \\\n./boot.sh 07a_highcharity";

fn said(s: &mut rush_convo::Session, id: &str, parts: Vec<Part>, at: Ms) {
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: id.into(),
            model: "claude-opus-5-5".into(),
            parts,
            tokens: Some(TokenUsage {
                input: 1200,
                output: 312,
                ..TokenUsage::default()
            }),
            ..Message::default()
        }),
        at,
    );
}

fn text(t: &str) -> Part {
    Part {
        kind: PartKind::Text,
        text: t.into(),
        ..Part::default()
    }
}

fn call(id: &str, kind: Kind, name: &str, input: Input) -> Part {
    Part {
        kind: PartKind::ToolCall,
        call: Some(Box::new(Call {
            id: id.into(),
            name: name.into(),
            kind,
            input,
            ..Call::default()
        })),
        ..Part::default()
    }
}

fn shell(id: &str, cmd: &str, why: &str) -> Part {
    call(
        id,
        Kind::Shell,
        "Bash",
        Input {
            command: cmd.into(),
            description: why.into(),
            ..Input::default()
        },
    )
}

fn result(s: &mut rush_convo::Session, out: Output, at: Ms) {
    s.apply(
        &Event::Message(Message {
            role: "user".into(),
            id: format!("r-{}", out.call_id),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(out)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        at,
    );
}

fn ok(s: &mut rush_convo::Session, id: &str, at: Ms) {
    result(
        s,
        Output {
            call_id: id.into(),
            text: "ok".into(),
            ..Output::default()
        },
        at,
    );
}

fn end(s: &mut rush_convo::Session, cost: f64, at: Ms) {
    s.apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            cost,
            ..TurnEnd::default()
        }),
        at,
    );
}

/// A session hard at work, w×h: its second turn failed a long step, its
/// third woke itself and folded five steps, its fourth runs a chain of
/// nine commands, at the second; a subagent and a shell run beside it.
pub(super) fn busy(w: usize, h: usize) -> Model {
    const MIN: Ms = 60_000;
    let reg = Registry::new();
    reg.register(rush_claude::Claude::new());
    let mut a = rush_core::agent::Agent {
        key: KEY.into(),
        kind: "claude".into(),
        display_name: "Drive Faster Coordination to Web Fixes Escalation".into(),
        repo: "/src/halo".into(),
        branch: "d3d-frame-tail".into(),
        pid: 4242,
        ..Default::default()
    };
    a.job.id = "k1".into();
    a.job.state = State::Working;
    a.job.cwd = "/src/halo".into();
    // The machine's battery and disk as the frames show them, whatever
    // this one's.
    rush_sys::sysinfo::pin(
        rush_sys::sysinfo::Battery {
            present: true,
            percent: 80,
            charging: false,
        },
        rush_sys::sysinfo::Disk {
            free: 37 << 30,
            total: 500 << 30,
        },
    );
    let mut m = crate::new_model("0.1.0");
    m.reg = reg;
    m.w = w;
    m.h = h;
    m.clock = Some(NOW);
    m.now = NOW;
    let mut v = SessionView::new(KEY.into(), Source::Hosted, 1);
    let s = &mut v.sess;
    s.info.cwd = "/src/halo".into();
    // 1: what you asked, a long failed step, an edit.
    s.sent(
        "firefight is still stuttering on the frame tail: fix it, then check every boot map".into(),
        Vec::new(),
        NOW - 40 * MIN,
    );
    said(
        s,
        "m1",
        vec![
            text("I'll fix the frame tail, then boot all three."),
            call(
                "e1",
                Kind::Edit,
                "Edit",
                Input {
                    path: "/src/halo/d3d/src/frame_tail.rs".into(),
                    ..Input::default()
                },
            ),
        ],
        NOW - 40 * MIN + 2_000,
    );
    result(
        s,
        Output {
            call_id: "e1".into(),
            text: "ok".into(),
            patches: vec![Patch {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 3,
                lines: vec!["-a".into(), "+b".into(), "+c".into(), "+d".into()],
                ..Patch::default()
            }],
            ..Output::default()
        },
        NOW - 40 * MIN + 3_000,
    );
    said(
        s,
        "m2",
        vec![shell(
            "f1",
            "cargo clippy && git push && ./boot.sh all",
            "Clippy, push, rebuild, boot all three",
        )],
        NOW - 39 * MIN,
    );
    result(
        s,
        Output {
            call_id: "f1".into(),
            text: "booting 03a_oldmombasa\nerror: collision: location: float (03a_oldmombasa boot)"
                .into(),
            stderr: "error: collision: location: float (03a_oldmombasa boot)".into(),
            is_error: true,
            exit: Some(1),
            ..Output::default()
        },
        NOW - 39 * MIN + 20 * MIN + 37_000,
    );
    said(
        s,
        "m3",
        vec![text(
            "The boot of 03a_oldmombasa failed on a collision location. Looking.",
        )],
        NOW - 18 * MIN,
    );
    end(s, 1.10, NOW - 18 * MIN + 1_000);
    s.apply(
        &Event::Compacted(Compacted {
            trigger: "auto".into(),
            before: 180_000,
            after: 24_000,
        }),
        NOW - 17 * MIN,
    );
    // 2: it woke with no message and ran five quick steps.
    let mut parts = Vec::new();
    for (i, (kind, name, path)) in [
        (Kind::Read, "Read", "/src/halo/game/src/collision.rs"),
        (Kind::Read, "Read", "/src/halo/game/src/boot.rs"),
        (Kind::Search, "Grep", ""),
        (Kind::Glob, "Glob", ""),
        (Kind::Read, "Read", "/src/halo/maps/03a.toml"),
    ]
    .into_iter()
    .enumerate()
    {
        parts.push(call(
            &format!("q{i}"),
            kind,
            name,
            Input {
                path: path.into(),
                pattern: "location".into(),
                ..Input::default()
            },
        ));
    }
    said(s, "m4", parts, NOW - 10 * MIN);
    for i in 0..5 {
        ok(s, &format!("q{i}"), NOW - 10 * MIN + 700);
    }
    said(
        s,
        "m5",
        vec![text(
            "The location is read as a float where the map has an int.",
        )],
        NOW - 9 * MIN,
    );
    end(s, 0.79, NOW - 9 * MIN + 1_000);
    // 3: the turn under way: a shell in the background, a subagent, and
    // the chain, at its second command.
    s.sent(
        "clippy the crates you touched, then push".into(),
        Vec::new(),
        NOW - 27_000,
    );
    said(
        s,
        "m6",
        vec![
            call(
                "b1",
                Kind::Shell,
                "Bash",
                Input {
                    command: "cargo build --release -p game".into(),
                    description: "Build the game".into(),
                    background: true,
                    ..Input::default()
                },
            ),
            call(
                "a1",
                Kind::Subagent,
                "Task",
                Input {
                    description: "Rust Code Review Required".into(),
                    prompt: "review the frame tail change".into(),
                    agent: "general-purpose".into(),
                    ..Input::default()
                },
            ),
        ],
        NOW - 26_000,
    );
    for (id, call, kind, label, agent) in [
        (
            "j1",
            "b1",
            TaskKind::Shell,
            "cargo build --release -p game",
            "",
        ),
        (
            "s1",
            "a1",
            TaskKind::Subagent,
            "Rust Code Review Required",
            "general-purpose",
        ),
    ] {
        s.apply(
            &Event::TaskStarted(TaskStarted {
                id: id.into(),
                call_id: call.into(),
                kind,
                label: label.into(),
                agent: agent.into(),
                background: true,
            }),
            NOW - 25_000,
        );
    }
    ok(s, "b1", NOW - 25_000);
    said(
        s,
        "m7",
        vec![shell("c1", CHAIN, "Clippy touched crates, then push")],
        NOW - 16_000,
    );
    v.read = true;
    m.sel = KEY.into();
    m.agents = vec![a];
    m.roots = vec![String::new()];
    m.session = Some(v);
    m.full = true;
    let subs = vec![Subagent {
        id: "s1".into(),
        typ: "general-purpose".into(),
        description: "Rust Code Review Required".into(),
        model: "claude-opus-5-5".into(),
        tool_use_id: "a1".into(),
        path: "/s/subagents/agent-s1.jsonl".into(),
        born: NOW - 25_000,
        modified: NOW - 1_000,
        ..Subagent::default()
    }];
    let r = &mut conn(&mut m, KEY).runs;
    r.listed = subs.clone();
    r.subs = subs;
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    m
}

/// The frame, plain: drawn twice, as what a frame's dock takes in shows
/// on the next.
pub(super) fn drawn(m: &mut Model) -> String {
    let (w, h) = (m.w as u16, m.h as u16);
    let _ = crate::frame(m, w, h, &[], false);
    crate::frame(m, w, h, &[], false)
}

/// Keeps the frames where UX_SHOTS says, to look at.
fn keep(name: &str, m: &mut Model) {
    let Some(dir) = std::env::var_os("UX_SHOTS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (m.w as u16, m.h as u16);
    let plain = drawn(m);
    std::fs::write(dir.join(format!("{name}_{w}x{h}.txt")), plain).unwrap();
    let ansi = crate::frame(m, w, h, &[], true);
    std::fs::write(dir.join(format!("{name}_{w}x{h}.ansi")), ansi).unwrap();
}

#[test]
fn the_busy_session_draws() {
    for (w, h) in [(200, 56), (260, 60), (120, 40)] {
        let mut m = busy(w, h);
        keep("busy", &mut m);
        let f = drawn(&mut m);
        assert!(f.contains("Drive Faster Coordination"), "{f}");
    }
}
