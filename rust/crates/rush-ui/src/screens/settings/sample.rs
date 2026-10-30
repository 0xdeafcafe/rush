//! A short made-up conversation with an edit in it, for Settings to show a
//! change to how a Session looks: its diff indented with tabs, and a line
//! whose only change is spaces for a tab. After Go's
//! internal/ui/settings_sample.go (sampleSession :17, sessionPreview :41).

use rush_convo::{Options, Renderer, Session};
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::text::Line;
use rush_core::tool::{Call, Input, Kind, Output, Patch};

/// The sample conversation, as of 2026-09-30 09:00 UTC (sampleSession).
fn sample() -> Session {
    let mut s = Session::new();
    s.info.cwd = "/work/app".into();
    let at = 1_790_758_800_000; // 2026-09-30T09:00:00Z
    s.sent("retry the upload when it times out".into(), Vec::new(), at);
    let call = Call {
        id: "e1".into(),
        name: "Edit".into(),
        kind: Kind::Edit,
        input: Input {
            path: "/work/app/upload.go".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    let out = Output {
        call_id: "e1".into(),
        text: "ok".into(),
        patches: vec![Patch {
            path: String::new(),
            old_start: 12,
            old_lines: 3,
            new_start: 12,
            new_lines: 6,
            lines: [
                " \tfor try := range 3 {",
                "-        err = send(ctx, f)",
                "+\t\terr = send(ctx, f)",
                "+\t\tif err == nil {",
                "+\t\t\tbreak",
                "+\t\t}",
                " \t}",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }],
        ..Output::default()
    };
    let evs = [
        Event::Message(Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        Event::Message(Message {
            role: "user".into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(out)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        Event::Message(Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: "Uploads now retry up to **three** times.".into(),
                ..Part::default()
            }],
            ..Message::default()
        }),
        Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }),
    ];
    for (i, ev) in evs.iter().enumerate() {
        s.apply(ev, at + (i as i64 + 1) * 1000);
    }
    s
}

/// The sample as a Session draws it now, w wide, its last rows lines at
/// most: no blank line first, nor two together (sessionPreview).
pub fn session_preview(w: usize, rows: usize) -> Vec<Line> {
    let s = sample();
    let o = Options {
        width: w,
        now: rush_core::time::now(),
        ..Options::default()
    };
    let mut out: Vec<Line> = Vec::new();
    let mut blank = true;
    for l in Renderer::new().render(&s, &o) {
        let b = l.line.plain().trim().is_empty();
        if !b || !blank {
            out.push(l.line);
        }
        blank = b;
    }
    if blank && !out.is_empty() {
        out.pop();
    }
    if out.len() > rows {
        out.drain(..out.len() - rows);
    }
    out
}
