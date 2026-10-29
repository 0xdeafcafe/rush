//! The Session's views against Go's: the tabs and the rule under them,
//! [ ] between them, Tasks, Overview and Changes as Go draws them, the
//! page and wheel steps, the map from 96 wide, the dragged-over text and
//! #expand/#collapse. After Go's sessionviews_test.go, minimap_test.go,
//! textsel_test.go and pane_pointer_test.go.

use rush_core::event::{Context, Message, Part, PartKind, Plan};
use rush_core::tool::{Input, Kind as ToolKind, Output, Patch, TodoItem};
use rush_draw::{Button, KeyMods, MouseKind};

use super::*;
use crate::frame::Mouse;

/// A turn with an edit to a file, a task list and the context used.
fn worked(m: &mut Model) {
    let now = m.now;
    sess(m).sent("fix the parser".into(), Vec::new(), now - 60_000);
    let call = Call {
        id: "c1".into(),
        name: "Edit".into(),
        kind: ToolKind::Edit,
        input: Input {
            path: "/src/checkout/src/parse.rs".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    let msg = |role: &str, part: Part| {
        Event::Message(Message {
            role: role.into(),
            id: format!("{role}-{:?}", part.kind),
            parts: vec![part],
            ..Message::default()
        })
    };
    let s = sess(m);
    s.apply(
        &msg(
            "assistant",
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            },
        ),
        now - 50_000,
    );
    s.apply(
        &msg(
            "user",
            Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: "c1".into(),
                    text: "ok".into(),
                    patches: vec![Patch {
                        old_start: 3,
                        old_lines: 2,
                        new_start: 3,
                        new_lines: 2,
                        lines: vec![
                            " let a = 1;".into(),
                            "-let b = 2;".into(),
                            "+let b = 3;".into(),
                        ],
                        ..Patch::default()
                    }],
                    ..Output::default()
                })),
                ..Part::default()
            },
        ),
        now - 40_000,
    );
    s.apply(
        &Event::Plan(Plan {
            todos: vec![
                TodoItem {
                    label: "Read the parser".into(),
                    status: "completed".into(),
                    ..TodoItem::default()
                },
                TodoItem {
                    label: "Fix the bug".into(),
                    active: "Fixing the bug".into(),
                    status: "in_progress".into(),
                },
                TodoItem {
                    label: "Run the tests".into(),
                    status: "pending".into(),
                    ..TodoItem::default()
                },
            ],
        }),
        now - 30_000,
    );
    s.apply(
        &Event::Context(Context {
            tokens: 84_000,
            window: 100_000,
        }),
        now - 30_000,
    );
    s.apply(
        &msg(
            "assistant",
            Part {
                kind: PartKind::Text,
                text: "Fixed: b is 3 now.".into(),
                ..Part::default()
            },
        ),
        now - 20_000,
    );
    s.apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            cost: 0.42,
            ..TurnEnd::default()
        }),
        now - 10_000,
    );
}

fn opened(w: usize, h: usize) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, w, h);
    worked(&mut m);
    m
}

fn click(m: &mut Model, kind: MouseKind, x: usize, y: usize) -> Out {
    m.mouse_event(&Mouse {
        kind,
        x: x as u16,
        y: y as u16,
        mods: KeyMods::NONE,
    })
}

/// The column s starts at in row, in cells.
fn col_of(row: &str, s: &str) -> usize {
    rush_core::text::width(&row[..row.find(s).expect("in the row")])
}

/// The row of the screen whose text has s.
fn row_with(m: &mut Model, s: &str) -> Option<usize> {
    screen(m).iter().position(|l| l.contains(s))
}

/// Many turns, each a question and an answer: a conversation longer
/// than the screen.
fn long(m: &mut Model, n: usize) {
    let now = m.now;
    for i in 0..n {
        let at = now - (n - i) as Ms * 60_000;
        sess(m).sent(format!("question {i}"), Vec::new(), at);
        sess(m).apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: format!("a{i}"),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: format!("answer {i}"),
                    ..Part::default()
                }],
                ..Message::default()
            }),
            at + 1000,
        );
        sess(m).apply(
            &Event::TurnEnd(TurnEnd {
                reason: "done".into(),
                ..TurnEnd::default()
            }),
            at + 2000,
        );
    }
}

/// The tabs row, Go's paneHeader: a tab a view, the stats beside them,
/// [ ] views after, esc at the right; the rule lit under the one showing
/// and [ ] going round them both ways.
#[test]
fn tabs_and_the_rule_under_them() {
    let mut m = opened(120, 40);
    let s = screen(&mut m);
    let tabs = row_with(&mut m, "Conversation").expect("a tabs row");
    assert_eq!(
        s[tabs].trim_end(),
        "    Conversation   Overview   Changes +1 −1   Tasks 1 of 3    [ ] views                           esc back to the list"
    );
    let lit = |m: &mut Model| -> (usize, usize) {
        let r = screen(m)[tabs + 1].clone();
        let cs: Vec<char> = r.chars().collect();
        let from = cs.iter().position(|c| *c == '━').unwrap();
        let n = cs.iter().filter(|c| **c == '━').count();
        (from, n)
    };
    assert_eq!(lit(&mut m), (3, " Conversation ".len()));
    press(&mut m, "]");
    assert_eq!(lit(&mut m), (18, " Overview ".len()));
    press(&mut m, "[");
    press(&mut m, "[");
    let col = col_of(&s[tabs], "Tasks");
    assert_eq!(lit(&mut m), (col - 1, " Tasks 1 of 3 ".chars().count()));
    // Typing, [ is text.
    press(&mut m, "]");
    typed(&mut m, "a[");
    assert_eq!(input(&mut m), "a[");
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "conversation"
    );
}

/// The name's row says how much of the context is used, coloured as it
/// fills; the pointer over it lights it (headerContext, contextAt).
#[test]
fn context_on_the_names_row() {
    let mut m = opened(120, 40);
    let s = screen(&mut m);
    let y = row_with(&mut m, "context 84%").expect("context shown");
    assert!(s[y].starts_with("    tidy the parser"), "{:?}", s[y]);
    assert!(s[y].ends_with("context 84%  "), "{:?}", s[y]);
    let x = col_of(&s[y], "context");
    assert_eq!(click(&mut m, MouseKind::Moved, x, y), Out::Draw);
    assert!(conn(&mut m, KEY).vs.hover_context);
    assert_ne!(
        click(&mut m, MouseKind::Moved, x, y),
        Out::Draw,
        "nothing moved"
    );
}

/// A click on a tab shows its view; the pointer over one lights it
/// (paneTabAt, clickPaneTab, hoverPane).
#[test]
fn clicks_on_the_tabs() {
    let mut m = opened(120, 40);
    let s = screen(&mut m);
    let y = row_with(&mut m, "Conversation").unwrap();
    let x = col_of(&s[y], "Changes");
    assert_eq!(click(&mut m, MouseKind::Moved, x, y), Out::Draw);
    assert_eq!(conn(&mut m, KEY).vs.hover_tab, 3);
    assert_eq!(
        click(&mut m, MouseKind::Down(Button::Left), x, y),
        Out::Draw
    );
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "changes"
    );
    let x = col_of(&s[y], "[ ]");
    click(&mut m, MouseKind::Down(Button::Left), x + 1, y);
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "changes"
    );
}

/// The Tasks view as Go's taskLines draws it.
#[test]
fn tasks_view() {
    let mut m = opened(120, 40);
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "tasks"
    ));
    let s = screen(&mut m);
    let y = row_with(&mut m, "Tasks  1 of 3 done").unwrap();
    let want = [
        "    Tasks  1 of 3 done",
        &format!("    {}", "─".repeat(113)),
        "",
        "    Now  1",
        "      ■ Fixing the bug",
        "",
        "    Next  1",
        "      ☐ Run the tests",
        "",
        "    Done  1",
        "      ✓ Read the parser",
    ];
    for (i, w) in want.iter().enumerate() {
        assert_eq!(s[y + i].trim_end(), *w, "row {i}");
    }
}

/// With no task list there's no Tasks tab; the view itself says why.
#[test]
fn no_tasks_no_tab() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    assert_eq!(
        crate::screens::session_core::views::views(&m, KEY),
        vec!["conversation", "overview", "changes"]
    );
    assert!(!crate::screens::session_core::views::show_view(
        &mut m, KEY, "tasks"
    ));
    let rows = crate::screens::session_core::tasks::lines(&m, 80);
    assert_eq!(
        rows.iter()
            .map(|(l, _)| l.plain().trim_end().to_string())
            .collect::<Vec<_>>()[3],
        "    No task list yet. It appears when the agent plans its work."
    );
}

/// The Overview's tiles and sections (convo.Overview).
#[test]
fn overview_view() {
    let mut m = opened(120, 40);
    press(&mut m, "]");
    let s = screen(&mut m);
    let y = row_with(&mut m, "spent").unwrap();
    assert_eq!(
        s[y - 1].trim_end(),
        "     $0.42             50s               1                 1                 —                 0%"
    );
    // Go's tile asks the model's window, and this session names no model.
    assert_eq!(
        s[y].trim_end(),
        "     spent             working           turns             tool calls        context           from cache"
    );
    assert!(s[y - 2].trim_end().ends_with("tidy the parser"));
    let t = row_with(&mut m, "▾ Tools  1 call").unwrap();
    assert!(s[t + 1].starts_with("      Edit      ▰▰▰"), "{}", s[t + 1]);
    assert!(s[t + 1].trim_end().ends_with("10s"));
}

/// The Changes view: the session's file with its counts and turns, opened
/// to its hunk under the turn that made it; alt+r marks it reviewed; enter
/// on the hunk goes to the step in the conversation (ChangesView).
#[test]
fn changes_view() {
    let mut m = opened(120, 40);
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "changes"
    ));
    let s = screen(&mut m);
    let y = row_with(&mut m, "This session").unwrap();
    assert!(
        s[y].starts_with(&format!(
            "    ▾ This session  1 file · +1 −1 · 0 of 1 reviewed · {} marks one ─",
            rush_convo::render::key_word("alt+r")
        )),
        "{}",
        s[y]
    );
    assert!(s[y + 1].starts_with("    ▸   /src/checkout/src/parse.rs"));
    assert!(s[y + 1].trim_end().ends_with("+1 −1   #1"));
    press(&mut m, "down");
    assert_eq!(
        m.session.as_ref().unwrap().sel,
        "",
        "↓ from the box picks nothing"
    );
    press(&mut m, "up");
    assert_eq!(
        m.session.as_ref().unwrap().sel,
        "chg:/src/checkout/src/parse.rs"
    );
    let s = screen(&mut m);
    assert_eq!(
        s[s.len() - 1].trim(),
        "enter open or close  ·  ↑↓ pick  ·  esc back to message",
        "a changed file's keys"
    );
    press(&mut m, "enter");
    let s = screen(&mut m);
    assert!(
        s[y + 1].starts_with("  ▏ ▾   /src/"),
        "picked and open: {}",
        s[y + 1]
    );
    assert!(s[y + 2].contains("@ line 3  #1"), "{}", s[y + 2]);
    assert!(s[y + 2].trim_end().ends_with("enter goes to the step"));
    // Two in, seven for the hunk, the line number in five and a blank.
    assert_eq!(
        s[y + 3].trim_end(),
        format!("{}    3   let a = 1;", " ".repeat(9))
    );
    assert_eq!(
        s[y + 4].trim_end(),
        format!("{}    4 − let b = 2;", " ".repeat(9))
    );
    assert_eq!(
        s[y + 5].trim_end(),
        format!("{}    4 + let b = 3;", " ".repeat(9))
    );
    press(&mut m, "alt+r");
    let s = screen(&mut m);
    assert!(s[y].contains("1 of 1 reviewed"));
    assert!(s[y + 1].contains("✓ /src/checkout"), "{}", s[y + 1]);
    press(&mut m, "down");
    press(&mut m, "enter");
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "conversation"
    );
    assert_eq!(m.session.as_ref().unwrap().sel, "t1:s:c1");
}

/// pgup and pgdown go ten rows, as Go's; scrolled up, a pill says how
/// much is below; End follows the end again (paneKey :4235).
#[test]
fn paging_and_the_pill() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 90, 40);
    long(&mut m, 30);
    screen(&mut m);
    let start = |m: &Model| m.session.as_ref().unwrap().start;
    let end = start(&m);
    assert!(end > 20, "a long conversation starts well down");
    press(&mut m, "pgup");
    let s = screen(&mut m);
    // The pill takes a row: the view starts a row higher than ten up.
    assert_eq!(start(&m), end - 9);
    let pill = s
        .iter()
        .find(|l| l.contains("more · end follows"))
        .expect("the pill");
    assert!(pill.ends_with("↓ 10 more · end follows "), "{pill:?}");
    press(&mut m, "pgdown");
    screen(&mut m);
    assert_eq!(m.session.as_ref().unwrap().pos, crate::session::Pos::End);
    press(&mut m, "home");
    screen(&mut m);
    assert_eq!(start(&m), 0);
    press(&mut m, "end");
    let s = screen(&mut m);
    assert!(!s.iter().any(|l| l.contains("end follows")));
}

/// The wheel turns three rows; at the end it draws nothing (model.go
/// :1772).
#[test]
fn the_wheel() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 90, 40);
    long(&mut m, 30);
    screen(&mut m);
    let y = m.session.as_ref().unwrap().top + 2;
    assert_eq!(
        click(&mut m, MouseKind::ScrollDown, 10, y),
        Out::Idle,
        "at the end"
    );
    let end = m.session.as_ref().unwrap().start;
    assert_eq!(click(&mut m, MouseKind::ScrollUp, 10, y), Out::Draw);
    screen(&mut m);
    assert_eq!(
        m.session.as_ref().unwrap().start,
        end - 2,
        "three up, the pill a row"
    );
    press(&mut m, "home");
    screen(&mut m);
    assert_eq!(
        click(&mut m, MouseKind::ScrollUp, 10, y),
        Out::Idle,
        "at the top"
    );
}

/// The map shows from 96 wide, the conversation narrower by its width,
/// lit where the view is; a click on it scrolls there (minimapEnabled,
/// conversationWidth, pressMinimap).
#[test]
fn the_map() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 98, 40);
    long(&mut m, 30);
    screen(&mut m);
    assert!(!conn(&mut m, KEY).map.visible, "95 wide: no map");
    m.w = 99;
    let s = screen(&mut m);
    let mm = &conn(&mut m, KEY).map;
    assert!(mm.visible);
    assert_eq!(
        mm.x,
        2 + 96 - crate::screens::session_core::minimap::MINI_WIDTH
    );
    let (first, last) = mm.bounds();
    assert!(
        last == mm.height && first > 0,
        "the end is lit: {first}..{last}"
    );
    let top = m.session.as_ref().unwrap().top;
    assert_eq!(
        m.session.as_ref().unwrap().o.width,
        96 - crate::screens::session_core::minimap::MINI_WIDTH
    );
    let edge: String = s[top].chars().skip(2 + 84).take(2).collect();
    assert_eq!(edge, " │", "{:?}", s[top]);
    let lit: String = s[top + first].chars().skip(2 + 84).take(2).collect();
    assert_eq!(lit, " ▌");
    // The top row of the map goes to the top.
    let x = conn(&mut m, KEY).map.x + 3;
    assert_eq!(
        click(&mut m, MouseKind::Down(Button::Left), x, top),
        Out::Draw
    );
    assert_eq!(
        click(&mut m, MouseKind::Up(Button::Left), x, top),
        Out::Draw
    );
    screen(&mut m);
    assert_eq!(m.session.as_ref().unwrap().start, 0);
}

/// Text dragged over is copied on release, as written; a press that
/// never moved picks the row; esc drops the text (textsel.go).
#[test]
fn dragged_over_text() {
    let mut m = opened(90, 40);
    let s = screen(&mut m);
    let y = row_with(&mut m, "Fixed: b is 3 now.").unwrap();
    let x = col_of(&s[y], "Fixed");
    assert_eq!(
        click(&mut m, MouseKind::Down(Button::Left), x, y),
        Out::Idle
    );
    assert_eq!(
        click(&mut m, MouseKind::Drag(Button::Left), x + 10, y),
        Out::Draw
    );
    assert_eq!(
        click(&mut m, MouseKind::Up(Button::Left), x + 10, y),
        Out::Draw
    );
    assert_eq!(m.pending_copy.as_deref(), Some("Fixed: b is"));
    // Painted on the selection's ground.
    let f = m.frame();
    assert!(f[y].spans.iter().any(|sp| sp.style.bg
        == crate::screens::session_core::textsel::SEL_BLUE
        && sp.text == "Fixed: b is"));
    assert_eq!(press(&mut m, "ctrl+c"), Out::Draw, "copied again, not quit");
    assert_eq!(press(&mut m, "esc"), Out::Draw);
    assert!(!conn(&mut m, KEY).txt.on);
    // A click: the row it's on is picked.
    let h = row_with(&mut m, "parse.rs").unwrap();
    click(&mut m, MouseKind::Down(Button::Left), 10, h);
    assert_eq!(click(&mut m, MouseKind::Up(Button::Left), 10, h), Out::Draw);
    assert_eq!(m.session.as_ref().unwrap().sel, "t1:s:c1");
}

/// #collapse and #expand set how turns gone past are drawn, and say so
/// (setHistoryFold).
#[test]
fn history_folds() {
    let mut m = opened(90, 40);
    press(&mut m, "]");
    typed(&mut m, "#expand");
    press(&mut m, "enter");
    assert!(m.session.as_ref().unwrap().o.history_open);
    assert_eq!(flash(&m), "every turn shown whole");
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "conversation"
    );
    m.session.as_mut().unwrap().sel = "t1:s:c1".into();
    crate::screens::session_core::history::set_fold(&mut m, false);
    assert!(!m.session.as_ref().unwrap().o.history_open);
    assert_eq!(m.session.as_ref().unwrap().sel, "t1");
}

/// The pointer over a step lights every row of it (paintHover).
#[test]
fn the_pointer_lights_an_item() {
    let mut m = opened(90, 40);
    screen(&mut m);
    let y = row_with(&mut m, "parse.rs").unwrap();
    assert_eq!(click(&mut m, MouseKind::Moved, 10, y), Out::Draw);
    assert_eq!(conn(&mut m, KEY).vs.hover_ref, "t1:s:c1");
    let f = m.frame();
    let lit = |l: &Line| l.spans.iter().any(|s| s.style.bg == crate::style::HOVER_BG);
    assert!(lit(&f[y]));
    assert!(lit(&f[y + 2]), "its diff too");
    assert!(!lit(&f[y - 2]), "not the turn's heading");
}

/// An agent that says what it reads: two files, one not written yet.
struct Memo;

impl adapter::MemoryReader for Memo {
    fn memory(&self, _: &Profile, cwd: &str, _: &str) -> adapter::Memory {
        adapter::Memory {
            files: vec![
                adapter::MemoryFile {
                    group: "Project".into(),
                    path: format!("{cwd}/CLAUDE.md"),
                    name: "CLAUDE.md".into(),
                    about: "how to build it".into(),
                    up: 1200,
                    mod_time: NOW - 120_000,
                    ..adapter::MemoryFile::default()
                },
                adapter::MemoryFile {
                    group: "Project".into(),
                    path: format!("{cwd}/.claude/rules.md"),
                    name: "rules.md".into(),
                    missing: true,
                    ..adapter::MemoryFile::default()
                },
            ],
            ..adapter::Memory::default()
        }
    }
    fn forget_file(&self, _: &str) -> rush_core::adapter::Result<()> {
        Ok(())
    }
}

impl Adapter for Memo {
    fn kind(&self) -> Kind {
        Kind::from("memo")
    }
    fn name(&self) -> String {
        "Memo Code".into()
    }
    fn features(&self) -> Features {
        [(Feature::Memory, Support::YES)].into_iter().collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn memory_reader(&self) -> Option<&dyn adapter::MemoryReader> {
        Some(self)
    }
}

/// The Memory view: a tab when the agent says what it reads, the files
/// under their group with what each costs, ↑↓ round them, the picked one
/// in the bottom half (memoryLines, memRows, docPane).
#[test]
fn memory_view() {
    let dir = std::env::temp_dir().join(format!("rush-memview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("CLAUDE.md"), "# Build\n\ncargo build\n").unwrap();
    let fake = Fake::new();
    let mut a = agent("memo", State::Idle);
    a.job.cwd = dir.to_string_lossy().into_owned();
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.now = NOW;
    let reg = Registry::new();
    reg.register(Memo);
    m.reg = reg;
    m.w = 120;
    m.h = 40;
    let src = Source::History {
        adapter: m.reg.get("memo").unwrap(),
        session: Box::new(adapter::Session::default()),
    };
    let mut v = SessionView::new(a.key.clone(), src, 1);
    v.read = true;
    m.agents = vec![a];
    m.session = Some(v);
    set_hosts(&mut m, Arc::new(fake.clone()));
    let vs = crate::screens::session_core::views::views(&m, KEY);
    assert_eq!(vs.last(), Some(&"memory"));
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "memory"
    ));
    let s = screen(&mut m);
    let y = row_with(&mut m, "what Memo Code reads").unwrap();
    assert!(s[y].starts_with("    Memory   what Memo Code reads for this project, and when"));
    assert!(
        s[y].trim_end()
            .ends_with("≈1.2k tokens every session · 2 files"),
        "{}",
        s[y]
    );
    assert!(s[y + 2].starts_with("    Project"));
    assert!(
        s[y + 3].starts_with("      ◆ CLAUDE.md  how to build it"),
        "{}",
        s[y + 3]
    );
    assert!(
        s[y + 3].trim_end().ends_with("≈1.2k · 2m ago"),
        "{}",
        s[y + 3]
    );
    assert!(s[y + 4].starts_with("      ◇ rules.md"));
    assert!(s[y + 4].trim_end().ends_with("not written yet"));
    assert!(
        s.iter()
            .any(|l| l.contains("Pick a file above to see it here · space edits it"))
    );
    assert_eq!(s[s.len() - 1].trim(), "↑↓ pick a file  ·  [ ] views");
    press(&mut m, "down");
    let s = screen(&mut m);
    assert!(
        s[y + 3].starts_with("  ▍   ◆ CLAUDE.md"),
        "picked: {}",
        s[y + 3]
    );
    let r = row_with(&mut m, "── CLAUDE.md").expect("its title");
    assert!(s[r].trim_end().ends_with("markdown"));
    assert_eq!(s[r + 1].trim_end(), "   # Build");
    assert_eq!(s[r + 3].trim_end(), "   cargo build");
    press(&mut m, "down");
    press(&mut m, "down");
    assert_eq!(
        m.session.as_ref().unwrap().sel,
        format!("mem:{}/CLAUDE.md", dir.display()),
        "round"
    );
    // The Overview links to it, with what the files cost
    // (overviewSections' Memory).
    crate::screens::session_core::views::show_view(&mut m, KEY, "overview");
    if let Some(v) = m.session.as_mut() {
        v.sel.clear();
    }
    let s = screen(&mut m);
    let y = row_with(&mut m, "→ 2 files").expect("the Memory link");
    assert!(s[y - 1].starts_with("    ▾ Memory ─"), "{:?}", s[y - 1]);
    assert!(
        s[y].starts_with("      → 2 files · ≈1.2k tokens every session"),
        "{:?}",
        s[y]
    );
    assert!(s[y].trim_end().ends_with("enter opens the Memory tab"));
    press(&mut m, "up");
    assert_eq!(m.session.as_ref().unwrap().sel, "go:memory");
    press(&mut m, "enter");
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "memory"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// From 96 wide the box and the keys under it give the map its width,
/// and the map runs on down beside them (rushmode.go:3224, :2656).
#[test]
fn the_map_beside_the_box() {
    let (mut m, _) = hosted("steer", Vec::new());
    m.w = 120;
    let s = screen(&mut m);
    let n = s.len();
    let mm_x = conn(&mut m, KEY).map.x;
    assert_eq!(
        mm_x,
        2 + 117 - crate::screens::session_core::minimap::MINI_WIDTH
    );
    // The box's right edge stops short of the map, which carries on beside
    // it and the keys.
    let edge = s[n - 2].chars().position(|c| c == '╯').unwrap();
    assert_eq!(edge, mm_x - 1, "{:?}", s[n - 2]);
    for row in &s[n - 4..] {
        let side: String = row.chars().skip(mm_x).take(2).collect();
        assert!(side == " │" || side == " ▌", "{row:?}");
    }
    // Another view has no map.
    press(&mut m, "]");
    let s = screen(&mut m);
    assert!(s[s.len() - 2].trim_end().ends_with('╯'));
    assert!(!s.iter().any(|r| r.chars().nth(mm_x + 1) == Some('│')));
}

/// An agent that prices its tokens and knows its model's window.
struct Priced;

impl rush_core::adapter::Adapter for Priced {
    fn kind(&self) -> Kind {
        Kind::from("priced")
    }
    fn name(&self) -> String {
        "Priced Code".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn pricer(&self) -> Option<&dyn rush_core::adapter::Pricer> {
        Some(self)
    }
    fn context_windower(&self) -> Option<&dyn rush_core::adapter::ContextWindower> {
        Some(self)
    }
}

impl rush_core::adapter::Pricer for Priced {
    fn cost(&self, _: &str, u: &rush_core::agent::TokenUsage) -> Option<f64> {
        Some(u.output as f64 / 1000.0 + u.cache_read as f64 / 100_000.0)
    }
}

impl rush_core::adapter::ContextWindower for Priced {
    fn context_window(&self, _: &str) -> i64 {
        200_000
    }
}

/// A transcript carries no totals: each request is priced, a turn's cost
/// is its requests' unless it says its own, the context tile and the
/// header use the model's window and the next message's price is said
/// (turnCosts, Session.cost, ContextWindow, the cache section).
#[test]
fn priced_turns_and_the_models_window() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    m.reg.register(Priced);
    m.agents[0].kind = "priced".into();
    let now = m.now;
    let said = |m: &mut Model, id: &str, out: i64, at: Ms| {
        sess(m).apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: id.into(),
                model: "big-1".into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: format!("answer {id}"),
                    ..Part::default()
                }],
                tokens: Some(rush_core::agent::TokenUsage {
                    output: out,
                    cache_read: 50_000,
                    ..Default::default()
                }),
                ..Message::default()
            }),
            at,
        );
    };
    sess(&mut m).sent("one".into(), Vec::new(), now - 600_000);
    said(&mut m, "a1", 2000, now - 590_000);
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 580_000);
    sess(&mut m).sent("two".into(), Vec::new(), now - 500_000);
    said(&mut m, "a2", 500, now - 490_000);
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 480_000);
    press(&mut m, "]");
    let s = screen(&mut m);
    let y = row_with(&mut m, "spent").unwrap();
    // 2.5 + 2 × 0.5 for the cache read.
    assert!(s[y - 1].trim_start().starts_with("$3.50"), "{:?}", s[y - 1]);
    assert!(s[y].contains("context of 200k"), "{:?}", s[y]);
    let p = row_with(&mut m, "▾ Per turn").unwrap();
    assert!(s[p].contains("2 turns · most $2.50, on #1"), "{:?}", s[p]);
    let n = row_with(&mut m, "next message").unwrap();
    // 50.5k in context, read from the cache; a rewrite this agent doesn't
    // price says no price.
    assert!(s[n].contains("reads 50k ≈ $0.51"), "{:?}", s[n]);
    assert!(s[n].trim_end().ends_with("it rewrites it"), "{:?}", s[n]);
    let h = row_with(&mut m, "context 25%").expect("the header asks the model's window");
    assert!(s[h].starts_with("    tidy the parser"), "{:?}", s[h]);
}

/// A file the session edited through a symlinked folder is still its own
/// in the working tree git lists by the folder's real path (realPath,
/// changes.go:218): the temp folder is /var/… on macOS, /private/var/…
/// to git.
#[test]
fn ours_through_a_symlink() {
    let dir = std::env::temp_dir().join(format!("rush-ours-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "one",
    ]);
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    let path = dir.join("a.txt").to_string_lossy().into_owned();
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    let now = m.now;
    sess(&mut m).info.cwd = dir.to_string_lossy().into_owned();
    sess(&mut m).sent("edit a".into(), Vec::new(), now - 60_000);
    let call = Call {
        id: "c1".into(),
        name: "Edit".into(),
        kind: ToolKind::Edit,
        input: Input {
            path: path.clone(),
            ..Input::default()
        },
        ..Call::default()
    };
    sess(&mut m).apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "a1".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            }],
            ..Message::default()
        }),
        now - 50_000,
    );
    sess(&mut m).apply(
        &Event::Message(Message {
            role: "user".into(),
            id: "u1".into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: "c1".into(),
                    text: "ok".into(),
                    patches: vec![Patch {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        lines: vec!["-one".into(), "+two".into()],
                        ..Patch::default()
                    }],
                    ..Output::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        }),
        now - 40_000,
    );
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 30_000);
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "changes"
    ));
    screen(&mut m); // with no loop, git and the paths are read at once
    let s = screen(&mut m);
    let y = row_with(&mut m, "▾ Working tree").expect("git read");
    assert!(s[y].contains("1 file changed against"), "{:?}", s[y]);
    assert!(!s[y].contains("not from this session"), "{:?}", s[y]);
    assert!(s[y + 1].contains("this session"), "{:?}", s[y + 1]);
    std::fs::remove_dir_all(&dir).ok();
}

/// Scrolled into a long conversation, the prompt you're reading under is
/// pinned over the top row once it's scrolled off; at the end nothing is;
/// a click on the pin goes to its turn (promptpins_test.go).
#[test]
fn prompt_pins() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 123, 36);
    long(&mut m, 30);
    let s = screen(&mut m);
    assert!(!s.iter().any(|l| l.contains("current question")));
    press(&mut m, "pgup");
    let s = screen(&mut m);
    let top = m.session.as_ref().unwrap().top;
    assert!(
        s[top].starts_with("  ▍ current question 25"),
        "{:?}",
        s[top]
    );
    assert_eq!(m.session.as_ref().unwrap().shown[0], "pin:t26");
    click(&mut m, MouseKind::Down(Button::Left), 10, top);
    click(&mut m, MouseKind::Up(Button::Left), 10, top);
    let v = m.session.as_ref().unwrap();
    assert_eq!(v.sel, "t26");
    let s = screen(&mut m);
    // Its turn from the top: the blank above its heading under the pin
    // of the one before, as Go's.
    assert!(s[top + 1].contains("▏ You"), "picked: {:?}", s[top + 1]);
    assert!(s[top + 2].contains("question 25"), "{:?}", s[top + 2]);
}

/// In a read conversation what's stacked on the box (the queue, a row of
/// room) goes on the transcript's end and scrolls with it; one row of
/// room stays put over the box; a click on the queue there picks it
/// (paneDock's auxRows, rushmode.go:2216, :3455).
#[test]
fn the_queue_scrolls_with_the_conversation() {
    let (mut m, _) = super::hosted("steer", vec![super::queue_of(&["first", "second"])]);
    m.session.as_mut().unwrap().read = true;
    long(&mut m, 30);
    let f = screen(&mut m);
    let n = f.len();
    let card = f
        .iter()
        .position(|l| l.contains("queue ·2"))
        .expect("the card");
    assert!(
        f[card - 1].contains("answer 29") || f[card - 2].contains("answer 29"),
        "{f:?}"
    );
    let room: String = f[n - 5].chars().take(100).collect();
    assert_eq!(room.trim(), "", "a row of room stays over the box");
    assert!(f[n - 4].trim_start().starts_with('╭'), "{f:?}");
    assert!(f[card + 2].contains("2  second"), "{f:?}");
    // A click on it picks it.
    click(&mut m, MouseKind::Down(Button::Left), 10, card + 2);
    assert_eq!(m.session.as_ref().unwrap().sel, "q:1");
    // Scrolled up, it goes with the transcript.
    if let Some(v) = m.session.as_mut() {
        v.sel.clear();
    }
    press(&mut m, "pgup");
    let f = screen(&mut m);
    assert!(!f.iter().any(|l| l.contains("queue ·2")), "{f:?}");
    assert!(f.iter().any(|l| l.contains("end follows")));
    // Where it was, a click is the conversation's now.
    click(&mut m, MouseKind::Down(Button::Left), 10, card + 2);
    assert_ne!(m.session.as_ref().unwrap().sel, "q:1");
}

/// The task list sits on the box: one row with the task under way until
/// its head is clicked, then the tasks around it; a click elsewhere on it
/// opens the Tasks view; ↓ from the box picks nothing, ↑ the last row
/// (taskRow, clickTasks, taskrow_test.go).
#[test]
fn the_task_row_on_the_box() {
    let mut m = opened(120, 40);
    let s = screen(&mut m);
    let y = row_with(&mut m, "▸ Tasks").expect("the task row");
    assert!(
        s[y].starts_with("    ▸ Tasks  1 of 3 done  ▰▰▱  Fixing the bug"),
        "{:?}",
        s[y]
    );
    assert!(s[y].contains("all tasks ›"));
    assert!(s[y + 1].trim_start().starts_with('╭'), "on the box");
    click(&mut m, MouseKind::Down(Button::Left), 10, y);
    let s = screen(&mut m);
    let y = row_with(&mut m, "▾ Tasks").expect("opened");
    assert!(s[y + 1].contains("✓ Read the parser"), "{:?}", s[y + 1]);
    assert!(s[y + 2].contains("■ Fixing the bug"));
    assert!(s[y + 3].contains("☐ Run the tests"));
    assert!(s[y + 4].trim_start().starts_with('╭'));
    click(&mut m, MouseKind::Down(Button::Left), 10, y + 2);
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "tasks"
    );
}
/// With room right of the conversation's text (from 171 wide less the
/// map), the tasks and what the session changed sit in a column there,
/// at the foot beside the newest turn, and leave the box; a click on a task opens Tasks, on a file Changes
/// with it picked (sideTasks, sideChanges, TestTasksBeside).
#[test]
fn the_column_beside_a_wide_conversation() {
    let mut m = opened(200, 40);
    let s = screen(&mut m);
    assert!(
        !s.iter().any(|l| l.contains("all tasks ›")),
        "not on the box"
    );
    let y = row_with(&mut m, "▌ Tasks  1 of 3  ▰▰▱").expect("the column");
    let x = col_of(&s[y], "▌ Tasks");
    // The text is 185 wide, the column 58 from a cell short of 126.
    assert_eq!(x, 2 + 125, "{:?}", s[y]);
    assert!(s[y + 1].contains("✓ Read the parser"));
    assert!(s[y + 2].contains("■ Fixing the bug"));
    assert!(s[y + 3].contains("☐ Run the tests"));
    let c = row_with(&mut m, "▌ Changes  +1 −1").expect("the changes");
    assert_eq!(c, y + 5, "a row apart");
    assert!(s[c + 1].contains("/src/checkout/src/parse.rs"));
    // At the foot, beside the newest turn: its last row is the turn's.
    assert!(s[c + 1].contains("1 step"), "{:?}", s[c + 1]);
    click(&mut m, MouseKind::Down(Button::Left), x + 4, c + 1);
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "changes"
    );
    assert_eq!(
        m.session.as_ref().unwrap().sel,
        "chg:/src/checkout/src/parse.rs"
    );
    press(&mut m, "[");
    press(&mut m, "[");
    screen(&mut m);
    click(&mut m, MouseKind::Down(Button::Left), x + 4, y + 2);
    assert_eq!(
        crate::screens::session_core::views::view_name(&mut m, KEY),
        "tasks"
    );
    // Narrower, they sit on the box again.
    let mut m = opened(170, 40);
    let s = screen(&mut m);
    assert!(s.iter().any(|l| l.contains("all tasks ›")));
}

/// Every view, at every width from nothing to wide and short heights,
/// with wide characters in the turns and tasks, and empty: nothing
/// panics, and no row runs past the screen once the dock has its 20
/// cells (narrower, the terminal cuts it).
#[test]
fn narrow_wide_and_empty_never_panic() {
    let wide = |m: &mut Model| {
        let now = m.now;
        sess(m).sent(
            "界界界 👩‍💻 e\u{301} 修正してください".into(),
            Vec::new(),
            now - 5000,
        );
        sess(m).apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: "w1".into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: "長い答え ".repeat(40),
                    ..Part::default()
                }],
                ..Message::default()
            }),
            now - 4000,
        );
        sess(m).apply(
            &Event::Plan(Plan {
                todos: vec![TodoItem {
                    label: "タスク".repeat(30),
                    active: "作業中".repeat(30),
                    status: "in_progress".into(),
                }],
            }),
            now - 3000,
        );
    };
    for fixture in 0..3 {
        for w in (0..32).chain([40, 60, 95, 96, 97, 120, 171, 172, 200, 260]) {
            for h in [0, 1, 3, 8, 14, 40] {
                let fake = Fake::new();
                let mut m = open(agent("steer", State::Idle), &fake, w, h);
                match fixture {
                    0 => worked(&mut m),
                    1 => wide(&mut m),
                    _ => {}
                }
                m.session.as_mut().unwrap().read = fixture != 2 || w % 2 == 0;
                // A drag over every column of a few rows, wide characters
                // cut in two at its ends, and a double-click.
                screen(&mut m);
                let top = m.session.as_ref().unwrap().top;
                for (x0, x1) in [(0, w), (3, 4), (w / 2, 1)] {
                    click(&mut m, MouseKind::Down(Button::Left), x0, top + 1);
                    click(&mut m, MouseKind::Drag(Button::Left), x1, top + 3);
                    click(&mut m, MouseKind::Drag(Button::Left), x1, top + h);
                    click(&mut m, MouseKind::Up(Button::Left), x1, top + 2);
                    screen(&mut m);
                }
                for k in ["]", "]", "]", "]", "]", "pgup", "up", "home", "down", "end"] {
                    press(&mut m, k);
                    let s = screen(&mut m);
                    for l in s.iter().filter(|_| w >= 24) {
                        assert!(
                            rush_core::text::width(l) <= w,
                            "{w}x{h} fixture {fixture} after {k}: {l:?}"
                        );
                    }
                }
            }
        }
    }
}

/// The Memory view and the Overview's link to it, narrow and short: no
/// panic.
#[test]
fn memory_narrow_never_panics() {
    for w in (0..40).chain([80, 120]) {
        for h in [0, 1, 5, 12, 30] {
            let mut m = Model::new("0.1.0");
            m.clock = Some(NOW);
            m.now = NOW;
            let reg = Registry::new();
            reg.register(Memo);
            m.reg = reg;
            m.w = w;
            m.h = h;
            let a = agent("memo", State::Idle);
            let src = Source::History {
                adapter: m.reg.get("memo").unwrap(),
                session: Box::new(adapter::Session::default()),
            };
            let mut v = SessionView::new(a.key.clone(), src, 1);
            v.read = true;
            m.agents = vec![a];
            m.session = Some(v);
            for view in ["memory", "overview"] {
                crate::screens::session_core::views::show_view(&mut m, KEY, view);
                for k in ["up", "up", "down", "enter", "pgdown"] {
                    press(&mut m, k);
                    screen(&mut m);
                }
            }
        }
    }
}

/// Files edited outside the project, such as plans, are the session's
/// own: Changes says where they are, and the Overview lists them under
/// Session files (TestProjectChanges; overviewsections.go Session files).
#[test]
fn own_files_stay_out_of_changes() {
    let mut m = opened(140, 40);
    let now = m.now;
    sess(&mut m).info.cwd = "/src/checkout".into();
    let call = Call {
        id: "p1".into(),
        name: "Write".into(),
        kind: ToolKind::Write,
        input: Input {
            path: "/u/.claude/plans/fix-parser.md".into(),
            content: "# Plan\nfix it\n".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    for (role, part) in [
        (
            "assistant",
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            },
        ),
        (
            "user",
            Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: "p1".into(),
                    created: true,
                    ..Output::default()
                })),
                ..Part::default()
            },
        ),
    ] {
        sess(&mut m).apply(
            &Event::Message(Message {
                role: role.into(),
                id: format!("{role}-plan"),
                parts: vec![part],
                ..Message::default()
            }),
            now - 15_000,
        );
    }
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "changes"
    ));
    let s = screen(&mut m);
    let y = row_with(&mut m, "This session").unwrap();
    assert!(
        s[y].contains("1 file · +1 −1 ·")
            && s[y].contains("1 file outside the project on Overview"),
        "{}",
        s[y]
    );
    assert!(
        row_with(&mut m, "fix-parser.md").is_none(),
        "a plan isn't Changes"
    );
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "overview"
    ));
    let y = row_with(&mut m, "Session files").expect("the Session files section");
    let s = screen(&mut m);
    assert!(
        s[y].contains("Session files  1 edited outside the project · +2 −0"),
        "{}",
        s[y]
    );
    assert!(
        s[y + 1].starts_with("    fix-parser.md  /u/.claude/plans")
            || s[y + 1].contains("fix-parser.md  /u/.claude/plans"),
        "{}",
        s[y + 1]
    );
    assert!(
        s[y + 1].trim_end().ends_with("new · 2 lines"),
        "{}",
        s[y + 1]
    );
}

/// A file a shell command wrote is in Changes, marked by shell, its lines
/// not counted; opened, it names the command and its turn
/// (TestChangesByShellAndSubagents's view).
#[test]
fn shell_writes_in_changes() {
    let mut m = opened(140, 40);
    let now = m.now;
    sess(&mut m).info.cwd = "/src/checkout".into();
    let call = Call {
        id: "b1".into(),
        name: "Bash".into(),
        kind: ToolKind::Shell,
        input: Input {
            command: "sed -i '' 's/a/b/' src/lex.rs".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    for (role, part) in [
        (
            "assistant",
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            },
        ),
        (
            "user",
            Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: "b1".into(),
                    ..Output::default()
                })),
                ..Part::default()
            },
        ),
    ] {
        sess(&mut m).apply(
            &Event::Message(Message {
                role: role.into(),
                id: format!("{role}-sed"),
                parts: vec![part],
                ..Message::default()
            }),
            now - 15_000,
        );
    }
    assert!(crate::screens::session_core::views::show_view(
        &mut m, KEY, "changes"
    ));
    let s = screen(&mut m);
    let y = row_with(&mut m, "This session").unwrap();
    assert!(
        s[y].contains("2 files · +1 −1 · 1 by shell, their lines not counted"),
        "{}",
        s[y]
    );
    let f = row_with(&mut m, "src/lex.rs").expect("the shell's file");
    assert!(s[f].trim_end().ends_with("by shell   #2"), "{}", s[f]);
    conn(&mut m, KEY)
        .vs
        .open
        .insert("chg:/src/checkout/src/lex.rs".into(), true);
    let s = screen(&mut m);
    assert!(
        s[f + 1].contains("@ shell · sed -i '' 's/a/b/' src/lex.rs  #2"),
        "{}",
        s[f + 1]
    );
}

/// A finished turn whose last message wants something of you ends with a
/// Needs-you section, laid out by the model that read it; a click on a
/// reply puts it in the box (turnwants.go needsOf, rushmode.go
/// actOnNeeds).
#[test]
fn needs_you_under_the_last_answer() {
    use crate::screens::session_core::turnwants::{TurnState, TurnWants, needs_of};
    use rush_convo::render::NeedsItem;
    let fake = Fake::new();
    let mut a = agent("steer", State::Done);
    a.job.updated_at = NOW - 60_000;
    let mut m = open(a.clone(), &fake, 120, 40);
    worked(&mut m);
    let item = |k: &str, t: &str| NeedsItem {
        kind: k.into(),
        text: t.into(),
    };
    m.kept::<TurnState>().got.insert(
        KEY.into(),
        TurnWants {
            at: NOW - 60_000,
            wants: "you".into(),
            line: "approve PR #52".into(),
            ui: vec![item("run", "make ship"), item("reply", "yes, ship it")],
            ..TurnWants::default()
        },
    );
    assert_eq!(needs_of(&mut m, &a).reason, "approve PR #52");
    let s = screen(&mut m);
    let y = row_with(&mut m, "Needs you · approve PR #52").expect("the section");
    assert!(s[y].contains("╭ ! Needs you"), "{}", s[y]);
    let r = row_with(&mut m, "yes, ship it").expect("the reply");
    assert!(s[r].contains("↩ yes, ship it") && s[r].contains("click to reply"));
    let x = s[r].find("yes").unwrap();
    let x = rush_core::text::width(&s[r][..x]);
    click(&mut m, MouseKind::Down(Button::Left), x, r);
    click(&mut m, MouseKind::Up(Button::Left), x, r);
    assert_eq!(input(&mut m), "yes, ship it");
    assert_eq!(flash(&m), "in your message box · enter sends it");
    // A turn after it: what was read is of the one before.
    m.agents[0].job.updated_at = NOW;
    let a = m.agents[0].clone();
    assert_eq!(needs_of(&mut m, &a).reason, "");
}
