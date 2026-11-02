//! The links the conversation draws besides markdown's and the files
//! steps name: your message's chips opening it (userbox.go chipRows,
//! pasteRows; render.go:4246 messageLinks), an @agent opening that agent
//! (handles.go), and a pull request's card opening it (cards.go:1559).

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::HashMap;

use rush_convo::handles::{Handle, set_handles};
use rush_convo::{Options, RenderedLine, Renderer, Session};
use rush_core::text::Color;
use serde_json::json;

mod common;

use common::*;

fn opts(w: usize) -> Options {
    Options {
        width: w,
        now: at(100),
        zone: ZONE,
        ..Options::default()
    }
}

/// Each run of the rows that links somewhere: its words and the link.
fn links(ls: &[RenderedLine]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for l in ls {
        let mut prev = None;
        for s in &l.line.spans {
            let Some(u) = s.style.link.url() else {
                prev = None;
                continue;
            };
            match out.last_mut() {
                Some((t, _)) if prev == Some(s.style.link) => t.push_str(&s.text),
                _ => out.push((s.text.clone(), u.to_string())),
            }
            prev = Some(s.style.link);
        }
    }
    out
}

fn turn_ref(ls: &[RenderedLine], words: &str) -> String {
    ls.iter()
        .find(|l| l.line.plain().contains(words))
        .map(|l| l.r#ref.clone())
        .expect(words)
}

#[test]
fn your_chips_open_your_message() {
    let mut s = Session::new();
    s.feed(
        sent_images(
            "look at [pasted text #1 · 3 lines: a b c] please",
            &["/tmp/shot.png", "/tmp/two.png"],
        ),
        at(1),
    );
    s.feed(say("ok"), at(2));
    s.feed(done(0.01), at(3));
    let ls = Renderer::new().render(&s, &opts(120));
    let r = turn_ref(&ls, "look at");
    let got = links(&ls);
    let want = [
        (
            "▤ pasted text #1 · 3 lines: a b c",
            format!("rush:message/{r}"),
        ),
        ("▣ shot.png", format!("rush:message/{r}/image/1")),
        ("▣ two.png", format!("rush:message/{r}/image/2")),
    ];
    for (w, u) in &want {
        assert!(
            got.iter().any(|(t, gu)| t == w && gu == u),
            "{w} → {u} not in {got:?}"
        );
    }
}

#[test]
fn a_long_paste_opens_its_message() {
    let body: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
    let mut s = Session::new();
    s.feed(
        sent(&format!(
            "<pasted_content>\n{}\n</pasted_content>",
            body.join("\n")
        )),
        at(1),
    );
    s.feed(done(0.01), at(2));
    let ls = Renderer::new().render(&s, &opts(120));
    let r = turn_ref(&ls, "pasted");
    let got = links(&ls);
    assert!(
        got.contains(&(
            "… 8 more lines · click to show all".into(),
            format!("rush:message/{r}")
        )),
        "{got:?}"
    );
}

#[test]
fn an_agent_tag_opens_the_agent() {
    let mut hs = HashMap::new();
    hs.insert(
        "lena".to_string(),
        Handle {
            name: "lena".into(),
            tint: Color::Indexed(3),
            title: "Lena".into(),
        },
    );
    set_handles(hs);
    let mut s = Session::new();
    s.feed(sent("ask @lena about it"), at(1));
    s.feed(say("I asked @Lena, and mailed bo@lena.dev."), at(2));
    s.feed(done(0.01), at(3));
    let ls = Renderer::new().render(&s, &opts(120));
    let got = links(&ls);
    assert_eq!(
        got,
        [
            ("@lena".to_string(), "rush:agent/lena".to_string()),
            ("@Lena".to_string(), "rush:agent/lena".to_string()),
        ]
    );
}

#[test]
fn a_pull_request_card_opens_it() {
    let mut s = Session::new();
    s.feed(sent("open a pr"), at(1));
    s.feed(
        tool_use(
            "b1",
            "Bash",
            json!({"command": "gh pr create --title \"Add it\" --body \"Why\""}),
        ),
        at(2),
    );
    s.feed(
        tool_result(
            "b1",
            "Creating pull request for feat into main in o/r\n\nhttps://github.com/o/r/pull/8\n",
            false,
            None,
        ),
        at(3),
    );
    s.feed(done(0.01), at(4));
    let ls = Renderer::new().render(&s, &opts(120));
    let got = links(&ls);
    let u = "https://github.com/o/r/pull/8".to_string();
    assert!(got.contains(&("#8".into(), u.clone())), "{got:?}");
    assert!(
        got.contains(&("github.com/o/r/pull/8".into(), u)),
        "{got:?}"
    );
}
