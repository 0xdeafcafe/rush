//! The Overview's own rows beside its numbers: the agent's @handle after
//! its name, the permission mode on the now line, the Context section
//! (how the window grew turn by turn) and the Scratch section. After Go's
//! rushmode.go:2451 (the overview's first rows), convo/overview.go:286
//! (the now line) and :300-347 (Context), growth.go and
//! overviewsections.go:75 (Scratch).

use rush_core::event::{Message, Part, PartKind};

use super::*;

/// The row of the screen whose text has s.
fn row_with(m: &mut Model, s: &str) -> Option<usize> {
    screen(m).iter().position(|l| l.contains(s))
}

/// Two turns, each a request: 74k, then 75k in context.
fn two_turns() -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 50);
    let now = m.now;
    let said = |m: &mut Model, id: &str, ctx: i64, at: Ms| {
        sess(m).apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: id.into(),
                model: "claude-opus-5-5".into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: format!("answer {id}"),
                    ..Part::default()
                }],
                tokens: Some(rush_core::agent::TokenUsage {
                    input: 1_000,
                    output: 300,
                    cache_read: ctx - 1_000,
                    ..Default::default()
                }),
                ..Message::default()
            }),
            at,
        );
    };
    sess(&mut m).sent("one".into(), Vec::new(), now - 600_000);
    said(&mut m, "a1", 74_000, now - 590_000);
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 580_000);
    sess(&mut m).sent("two".into(), Vec::new(), now - 500_000);
    said(&mut m, "a2", 75_000, now - 490_000);
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 480_000);
    m
}

#[test]
fn overview_has_the_handle_the_mode_and_how_the_context_grew() {
    let mut m = two_turns();
    sess(&mut m).info.permission_mode = "auto".into();
    press(&mut m, "]");
    let s = screen(&mut m);
    let y = row_with(&mut m, "spent").unwrap();
    assert!(s[y - 2].contains(" @"), "the handle: {:?}", s[y - 2]);
    let now = row_with(&mut m, "now  opus 5.5").unwrap();
    assert!(
        s[now].contains("now  opus 5.5 · effort default · permissions auto   in "),
        "{:?}",
        s[now]
    );
    let c = row_with(&mut m, "▾ Context").expect("the Context section");
    assert!(
        s[c + 1].trim().chars().all(|ch| "▁▂▃▄▅▆▇█".contains(ch)),
        "{:?}",
        s[c + 1]
    );
    assert_eq!(s[c + 2].trim(), "#1 74k → #2 75k · peak 75k");
    // The context goes before the turns' costs, as Go's.
    let p = row_with(&mut m, "▾ Per turn").unwrap();
    assert!(c < p);
}

#[test]
fn overview_has_its_scratch() {
    let mut m = two_turns();
    m.agents[0].temp = 3 << 20;
    let key = m.agents[0].key.clone();
    conn(&mut m, &key).info.began.temp_dir = "/tmp/rush-steer".into();
    press(&mut m, "]");
    let s = screen(&mut m);
    let y = row_with(&mut m, "▾ Scratch").expect("the Scratch section");
    assert!(
        s[y].contains("TMPDIR for it and everything it runs"),
        "{:?}",
        s[y]
    );
    assert_eq!(s[y + 1].trim(), "/tmp/rush-steer · 3M");
}

/// The Overview lists the agent's pull requests and the pages it
/// published, before its own sections; enter on a page opens it, on a
/// pull request GitHub has said more of, its sheet, else the page on
/// GitHub (overviewsections.go:52-67, prs.go:797, rushmode.go:4243).
#[test]
fn overview_has_pull_requests_and_artifacts() {
    use rush_core::tool::{Call, Kind, Output};
    let mut m = two_turns();
    let now = m.now;
    let call = Call {
        id: "art1".into(),
        name: "Artifact".into(),
        kind: Kind::Other,
        raw: rush_core::wire::RawJson::new(
            r#"{"action":"publish","file_path":"/tmp/report.html","description":"The report"}"#,
        )
        .ok(),
        ..Call::default()
    };
    sess(&mut m).sent("publish it".into(), Vec::new(), now - 400_000);
    sess(&mut m).apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "m-art1".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        now - 390_000,
    );
    sess(&mut m).apply(
        &Event::Message(Message {
            role: "user".into(),
            id: "r-art1".into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: "art1".into(),
                    text: "Published (Version 2): https://claude.ai/a/x1".into(),
                    ..Output::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        }),
        now - 380_000,
    );
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 370_000);
    let pr = |n: i64, title: &str| rush_core::adapter::Pr {
        url: format!("https://github.com/o/r/pull/{n}"),
        title: title.into(),
        number: n,
        state: "OPEN".into(),
        ..Default::default()
    };
    m.agents[0].prs = vec![pr(118, "fix the tax test"), pr(119, "a second go")];
    // GitHub's word on 118, read with the list's readings; never asked here.
    let root = crate::list::folder_key(&m.agents[0], &m.roots[0]).to_string();
    {
        let d = m.kept::<crate::screens::projects_machine::data::Data>();
        d.frozen = true;
        let mut repo = rush_gh::ghpr::Repo::default();
        repo.root.clone_from(&root);
        repo.prs = vec![rush_gh::ghpr::PR {
            number: 118,
            title: "fix the tax test".into(),
            url: "https://github.com/o/r/pull/118".into(),
            state: "OPEN".into(),
            ..Default::default()
        }];
        d.prs.insert(root.clone(), repo);
    }
    press(&mut m, "]");
    let s = screen(&mut m).join("\n");
    for want in [
        "▾ Pull requests  2 · space opens one",
        "⇡ #118",
        "⇡ #119",
        "▾ Artifacts  1 published · enter opens one",
        "◆ report",
        "https://claude.ai/a/x1",
    ] {
        assert!(s.contains(want), "no {want:?} in\n{s}");
    }
    let (prs, arts) = (
        s.find("Pull requests").unwrap(),
        s.find("Artifacts").unwrap(),
    );
    assert!(prs < arts, "pull requests first\n{s}");

    let pick = |m: &mut Model, r: &str| m.session.as_mut().unwrap().sel = r.into();
    pick(&mut m, "art:https://claude.ai/a/x1");
    screen(&mut m);
    press(&mut m, "enter");
    assert_eq!(
        m.flash.as_ref().map(|f| f.0.as_str()),
        Some("opened https://claude.ai/a/x1")
    );
    // Only the transcript knows 119: it opens on GitHub.
    pick(&mut m, "pr:119");
    screen(&mut m);
    press(&mut m, "space");
    assert_eq!(
        m.flash.as_ref().map(|f| f.0.as_str()),
        Some("opened https://github.com/o/r/pull/119")
    );
    assert!(m.sheet.is_none());
    // GitHub has said of 118: its sheet.
    pick(&mut m, "pr:118");
    screen(&mut m);
    press(&mut m, "enter");
    assert!(
        m.sheet_as::<crate::screens::projects_machine::prsheet::PrSheet>()
            .is_some(),
        "the PR sheet"
    );
}
