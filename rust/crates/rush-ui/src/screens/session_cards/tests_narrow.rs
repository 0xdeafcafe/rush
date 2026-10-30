//! The cards, sheets and panels drawn from fixed fixtures: never a
//! panic or a row past its width however narrow or short, and, with
//! RUSH_ORACLE set, written in colour to compare with Go's own.

use std::collections::{HashMap, HashSet};

use rush_core::adapter::FileRewind;
use rush_core::event::{Approval, ApprovalOption, Ask, Choice, Event, OptionKind, Question};
use rush_core::text::{Color, Line, Mods};
use rush_core::tool::{Call, Input, Kind, Replace};
use rush_room::room::{Member, Room, Summary};

use super::asksheets::ExportSheet;
use super::btw::{Qa, Thread, thread_lines};
use super::compact::{CompactSheet, Summ};
use super::discuss::{Verdicts, chat_room_rows, verdict_rows};
use super::enter::EnterSheet;
use super::fork::{ForkSheet, ForkTurn};
use super::mention::mention_rows;
use super::message::{MessageSheet, UserMessage};
use super::question::*;
use super::rewind::{Branch, FilesPreview, RewindSheet, TurnStart};
use super::sidekick::{Notes, Sidekick, side_column};
use super::slim::SlimSheet;
use super::slim::Unused;
use super::tests::*;
use super::*;
use crate::frame::Sheet;
use rush_core::agent::Agent;

fn colour(out: &mut String, c: Color, ext: u8) {
    match c {
        Color::Default => {}
        Color::Rgb(r, g, b) => out.push_str(&format!(";{ext};2;{r};{g};{b}")),
        Color::Indexed(i) => out.push_str(&format!(";{ext};5;{i}")),
    }
}

fn ansi(ls: &[Line], w: usize) -> String {
    let mut out = String::new();
    for l in ls {
        let mut used = 0;
        for s in &l.spans {
            let bg = if s.style.bg == Color::Default {
                l.bg
            } else {
                s.style.bg
            };
            out.push_str("\x1b[0");
            for (bit, code) in [
                (Mods::BOLD, 1),
                (Mods::DIM, 2),
                (Mods::ITALIC, 3),
                (Mods::UNDERLINE, 4),
                (Mods::REVERSE, 7),
                (Mods::STRIKE, 9),
            ] {
                if s.style.mods.contains(bit) {
                    out.push_str(&format!(";{code}"));
                }
            }
            colour(&mut out, s.style.fg, 38);
            colour(&mut out, bg, 48);
            out.push('m');
            out.push_str(&s.text);
            used += s.width();
        }
        if l.bg != Color::Default && used < w {
            out.push_str("\x1b[0");
            colour(&mut out, l.bg, 48);
            out.push('m');
            out.push_str(&" ".repeat(w - used));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// RUSH_ORACLE=<dir> writes each in colour to <dir>/<name>_<screen>.txt,
/// to compare with what a throwaway Go test prints of the same fixtures
/// (ansi stripped of nothing: colours compare too).
#[test]
fn oracle() {
    let Ok(dir) = std::env::var("RUSH_ORACLE") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for sw in [80usize, 120, 200] {
        let mut put = |name: &str, screen: usize, ls: &[Line], w: usize| {
            std::fs::write(format!("{dir}/{name}_{screen}.txt"), ansi(ls, w)).unwrap();
        };
        draw_all(
            sw,
            34,
            &|want| sheet_w(sw, want),
            sw - 4,
            sw / 2 - 10,
            &mut put,
        );
    }
}

/// Nothing panics however narrow or short: sheets, cards and panels at
/// a few cells, and at none.
#[test]
fn narrow_and_short() {
    for w in [0usize, 1, 2, 3, 5, 8, 13, 21, 34, 47] {
        for h in [0usize, 1, 3, 9] {
            // Rows past the width are Go's too (max(20, w-6)): the frame
            // fits a sheet's to its box and the screen clips the rest.
            let mut put = |_: &str, _: usize, ls: &[Line], _: usize| {
                assert!(ls.iter().all(|l| l.width() < 10_000));
            };
            draw_all(w, h, &|_| w, w, w, &mut put);
        }
    }
}

fn sheet_w(screen: usize, want: usize) -> usize {
    40.max((screen - 6).min(want)) - 4
}

fn ask(m: &mut Model, tool: &str, kind: Kind, input: Input) {
    let call = Call {
        id: "b1".into(),
        name: tool.into(),
        kind,
        input,
        ..Call::default()
    };
    let options = vec![ApprovalOption {
        id: "y".into(),
        label: "Yes".into(),
        kind: OptionKind::AllowOnce,
    }];
    apply(
        m,
        Event::Approval(Box::new(Approval {
            id: "r1".into(),
            call,
            options,
            ..Approval::default()
        })),
    );
}

fn agent(key: &str, name: &str) -> Agent {
    Agent {
        key: key.into(),
        display_name: name.into(),
        ..Agent::default()
    }
}

const PERM_CMD: &str = r#"mkdir -p "$TMPDIR/xterm-src" && cd "$TMPDIR/xterm-src" && for f in cursor.c screen.c; do curl -fsSL "https://example.com/$f" -o "$f" & done; wait; ls -la"#;

/// Every card, sheet and panel of the fixtures below, drawn: screen sw,
/// sheets as sheet(want) wide, the dock dw, side panels pw. h is the
/// height sheets and cards get.
/// Where each drawing goes: its name, the screen, its rows, its width.
type Put<'a> = &'a mut dyn FnMut(&str, usize, &[Line], usize);

#[allow(clippy::too_many_lines)]
fn draw_all(sw: usize, h: usize, sheet: &dyn Fn(usize) -> usize, dw: usize, pw: usize, put: Put) {
    {
        let mut m = hosted(sw, 40);
        let w = sheet(76);
        let mut s = ExportSheet {
            conn: "k".into(),
            text: "line one\nline two\nline three".into(),
            filename: "conversation.txt".into(),
            dir: "/src/app".into(),
            cur: 1,
            ..ExportSheet::default()
        };
        put("export", sw, &s.body(&m, w, h), w);
        let mut s = ExportSheet {
            loading: true,
            ..ExportSheet::default()
        };
        put("export_loading", sw, &s.body(&m, w, h), w);

        let w = sheet(112);
        let ts = |n: u32, p: &str, u: &str| TurnStart {
            n,
            prompt: p.into(),
            uuid: u.into(),
            ..TurnStart::default()
        };
        let mut rw = RewindSheet {
            agent_key: "k".into(),
            agent: "fix login".into(),
            kind: "claude".into(),
            acct: Default::default(),
            sid: "s".into(),
            cwd: "/src/app".into(),
            path: String::new(),
            model: String::new(),
            turns: vec![
                ts(3, "make it faster", ""),
                ts(2, "add a test", ""),
                ts(1, "fix the login bug", "u1"),
            ],
            branches: vec![Branch {
                session_id: "b1".into(),
                left: now(&m) - 3 * 3_600_000,
                from: 2,
                turns: 4,
                last: "try redis".into(),
            }],
            sel: 0,
            put_back: false,
            recap: false,
            previews: HashMap::new(),
        };
        put("rewind0", sw, &rw.body(&m, w, h), w);
        rw.sel = 2;
        rw.put_back = true;
        rw.recap = true;
        rw.previews.insert(
            "u1".into(),
            FilesPreview {
                loading: false,
                r: FileRewind {
                    can_rewind: true,
                    error: String::new(),
                    files: vec!["/src/app/a.go".into(), "/src/app/b.go".into()],
                    insertions: 12,
                    deletions: 3,
                },
                err: String::new(),
            },
        );
        put("rewind2", sw, &rw.body(&m, w, h), w);
        rw.sel = 3;
        put("rewind_branch", sw, &rw.body(&m, w, h), w);

        let ft = |n: u32, p: &str| ForkTurn {
            n,
            prompt: p.into(),
            end: 0,
        };
        let name: Vec<char> = "fix login (fork)".chars().collect();
        let mut fk = ForkSheet {
            conn: "k".into(),
            agent: "fix login".into(),
            prof: Default::default(),
            kind: "claude".into(),
            br: None,
            sid: "s".into(),
            cwd: "/src/app".into(),
            path: String::new(),
            turns: vec![
                ft(3, "make it faster"),
                ft(2, "add a test"),
                ft(1, "fix the login bug"),
            ],
            repo: String::new(),
            row: 0,
            name_pos: name.len(),
            name,
            first: Vec::new(),
            first_pos: 0,
            up_to: 0,
            model: 0,
            effort: 0,
            perm: 0,
            worktree: false,
            models: vec!["".into(), "opus".into(), "sonnet".into()],
            efforts: vec!["".into(), "high".into()],
            perms: vec!["".into(), "plan".into()],
            now_model: String::new(),
            now_effort: String::new(),
            now_perm: String::new(),
            cache_warm: 0,
            looked: true,
        };
        put("fork0", sw, &fk.body(&m, w, h), w);
        fk.row = super::fork::FROM;
        fk.up_to = 1;
        fk.model = 2;
        put("fork1", sw, &fk.body(&m, w, h), w);

        let w = sheet(76);
        let mut cs = CompactSheet {
            opts: vec![
                Summ {
                    label: "native harness compaction".into(),
                    ..Summ::default()
                },
                Summ {
                    kind: "claude".into(),
                    label: "Claude Code".into(),
                    model: "claude-haiku-4-5".into(),
                    prune: false,
                },
            ],
            errs: vec!["Ollama: not running".into()],
            ..CompactSheet::default()
        };
        put("compact", sw, &cs.body(&m, w, h), w);

        let w = sheet(84);
        let mut sl = SlimSheet {
            sess: "nope".into(),
            items: vec![
                Unused {
                    what: "MCP server".into(),
                    name: "playwright".into(),
                    tokens: 1600,
                    rules: vec!["mcp__playwright".into()],
                },
                Unused {
                    what: "skill".into(),
                    name: "pdf".into(),
                    tokens: 120,
                    rules: vec!["Skill(pdf)".into()],
                },
            ],
            keep: HashSet::from([1]),
            cur: 0,
            was: vec!["mcp__old".into()],
        };
        put("slim", sw, &sl.body(&m, w, h), w);
        let mut sl = SlimSheet {
            sess: "nope".into(),
            items: vec![],
            keep: HashSet::new(),
            cur: 0,
            was: vec![],
        };
        put("slim_empty", sw, &sl.body(&m, w, h), w);

        let w = sheet(64);
        let mut en = EnterSheet {
            agent: "k".into(),
            cur: 0,
        };
        put("enter", sw, &en.body(&m, w, h), w);

        let w = sheet(usize::MAX);
        let mut ms = MessageSheet::new(
            vec![
                UserMessage {
                    r#ref: "t1".into(),
                    text: "first message".into(),
                    images: vec![],
                },
                UserMessage {
                    r#ref: "t2".into(),
                    text: format!("please look at this {}", "word ".repeat(30)),
                    images: vec!["/src/app/shot.png".into()],
                },
            ],
            1,
        );
        put("message", sw, &ms.body(&m, w, h), w);

        let v = m.kept::<Verdicts>();
        v.waiting.insert(
            "k".into(),
            Summary {
                room: Room {
                    topic: "which cache to use".into(),
                    ..Room::default()
                },
                final_: "alpha: use redis\nbeta: use memcached for now".into(),
                ..Summary::default()
            },
        );
        let mut last = HashMap::new();
        last.insert("alpha".to_string(), "redis is simplest".to_string());
        let member = |n: &str| Member {
            name: n.into(),
            ..Member::default()
        };
        v.rooms = vec![Summary {
            room: Room {
                id: "r1".into(),
                from: "k".into(),
                topic: "which cache".into(),
                members: vec![member("alpha"), member("beta")],
                ..Room::default()
            },
            round: 2,
            last,
            ..Summary::default()
        }];
        put("verdict", sw, &verdict_rows(&m, "k", dw), dw);
        put("chatroom", sw, &chat_room_rows(&m, "k", dw), dw);

        let mut t = Thread {
            focused: true,
            qa: vec![Qa {
                question: "what is it doing?".into(),
                response: String::new(),
            }],
            err: "no answer came back: ask again, or ask in the conversation".into(),
            ..Thread::default()
        };
        put("btw", sw, &thread_lines(&mut t, pw, 20, 0), pw);
        let mut t = Thread {
            qa: vec![
                Qa {
                    question: "first?".into(),
                    response: "one".into(),
                },
                Qa {
                    question: "what is it doing?".into(),
                    response: "It's **fixing** the login test.\n\n```go\nx := 1\n```".into(),
                },
            ],
            ..Thread::default()
        };
        put("btw_answer", sw, &thread_lines(&mut t, pw, 20, 0), pw);
        let mut t = Thread::default();
        put("btw_tucked", sw, &thread_lines(&mut t, pw, 20, 0), pw);

        let at = now(&m) - 500;
        m.kept::<Sidekick>().notes.insert(
            "k".into(),
            Notes {
                model: "sonnet".into(),
                at,
                decided: vec!["!use redis".into(), "keep the API".into()],
                grain: vec!["bypasses the store".into()],
                open: vec!["!who owns auth?".into()],
                ..Notes::default()
            },
        );
        put("recall", sw, &side_column(&mut m, "k", pw, 14, 0, 0), pw);

        let mut fix = agent("a", "fix login bug");
        fix.branch = "fix/login".into();
        let docs = agent("b", "docs");
        let mut term = agent("c", "login page");
        term.interactive = true;
        m.agents = vec![docs, fix, term];
        m.order = vec![0, 1, 2];
        m.group_of.insert("a".into(), "Working".into());
        m.group_of.insert("b".into(), "Done".into());
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        put(
            "mention_log",
            sw,
            &mention_rows(&m, &chars("@LOG"), 0, 0, dw),
            dw,
        );
        put(
            "mention_all",
            sw,
            &mention_rows(&m, &chars("@"), 0, 1, dw),
            dw,
        );

        for foc in [false, true] {
            let mut m = hosted(sw, 40);
            set_info(&mut m, |i| i.cwd = "/src/app".into());
            ask(
                &mut m,
                "Bash",
                Kind::Shell,
                Input {
                    command: PERM_CMD.into(),
                    ..Input::default()
                },
            );
            set_info(&mut m, |i| {
                i.permission_modes = vec![Mode {
                    id: "bypassPermissions".into(),
                    ..Mode::default()
                }];
                if !foc {
                    i.input = "x".into();
                }
            });
            let i = info(&m, "k").unwrap();
            let r = card_rows(&mut m, "k", &i, dw, h);
            put(&format!("perm_bash_{foc}"), sw, &r, dw);
        }
        let mut pm = hosted(sw, 40);
        set_info(&mut pm, |i| {
            i.cwd = "/src/app".into();
            i.input = "x".into();
            i.permission_modes = vec![Mode {
                id: "bypassPermissions".into(),
                ..Mode::default()
            }];
        });
        ask(
            &mut pm,
            "Edit",
            Kind::Edit,
            Input {
                path: "/src/app/main.go".into(),
                edits: vec![Replace {
                    old: "a := 1".into(),
                    new: "a := 2".into(),
                    ..Replace::default()
                }],
                ..Input::default()
            },
        );
        let i = info(&pm, "k").unwrap();
        let r = card_rows(&mut pm, "k", &i, dw, h);
        put("perm_edit", sw, &r, dw);
        let mut pm = hosted(sw, 40);
        set_info(&mut pm, |i| {
            i.cwd = "/src/app".into();
            i.input = "x".into();
            i.permission_modes = vec![Mode {
                id: "bypassPermissions".into(),
                ..Mode::default()
            }];
        });
        ask(
            &mut pm,
            "WebFetch",
            Kind::Fetch,
            Input {
                url: "https://example.com/docs".into(),
                prompt: "read it".into(),
                ..Input::default()
            },
        );
        let i = info(&pm, "k").unwrap();
        let r = card_rows(&mut pm, "k", &i, dw, h);
        put("perm_fetch", sw, &r, dw);

        let choice = |l: &str| Choice {
            label: l.into(),
            ..Choice::default()
        };
        let req = Question {
            id: "q9".into(),
            asks: vec![
                Ask {
                    text: "Which layout?".into(),
                    header: "Layout".into(),
                    options: vec![
                        Choice {
                            label: "Split (Recommended)".into(),
                            description: "Agents left, Session right".into(),
                            preview: "```\n┌────┬────────┐\n│ A  │ S      │\n└────┴────────┘\n```"
                                .into(),
                            ..Choice::default()
                        },
                        Choice {
                            label: "Stacked".into(),
                            preview: "┌────────┐\n│ A      │\n├────────┤\n│ S      │\n└────────┘"
                                .into(),
                            ..Choice::default()
                        },
                    ],
                    ..Ask::default()
                },
                Ask {
                    text: "Theme?".into(),
                    header: "Theme".into(),
                    options: vec![choice("dark"), choice("light")],
                    ..Ask::default()
                },
            ],
            ..Question::default()
        };
        let mut c = Conn {
            card_focus: true,
            ..Conn::default()
        };
        put(
            "question",
            sw,
            &question_card(&mut c, &req, None, dw, 0),
            dw,
        );
        question_key(&mut c, &req, None, "1", true, &mut String::new());
        question_key(&mut c, &req, None, "2", true, &mut String::new());
        put(
            "question_review",
            sw,
            &question_card(&mut c, &req, None, dw, 0),
            dw,
        );
        let req2 = Question {
            id: "q1".into(),
            asks: vec![
                Ask {
                    text: "Which rules first?".into(),
                    header: "Lint".into(),
                    options: vec![
                        choice("no-floating-promises"),
                        choice("explicit return types"),
                    ],
                    ..Ask::default()
                },
                Ask {
                    text: "Which packages?".into(),
                    multi: true,
                    options: vec![choice("mcp"), choice("skills"), choice("web")],
                    ..Ask::default()
                },
            ],
            ..Question::default()
        };
        let mut c = Conn::default();
        put(
            "question_unfocused",
            sw,
            &question_card(&mut c, &req2, None, dw, 0),
            dw,
        );

        for (n, qa) in [
            vec![],
            vec![
                Qa {
                    question: "which one needs me?".into(),
                    response: "**fix login** waits on you: it asks to run `pnpm test`.\n\n- open it with 2".into(),
                },
                Qa {
                    question: "and docs?".into(),
                    response: String::new(),
                },
            ],
        ]
        .into_iter()
        .enumerate()
        {
            let mut qm = crate::model::Model::new("0.1.0");
            qm.w = sw;
            qm.h = 40;
            let q = qm.kept::<super::quickask::QuickAsk>();
            let input: Vec<char> = "where".chars().collect();
            q.threads = vec![Thread {
                qa,
                focused: n == 0,
                pos: input.len(),
                input,
                err: if n == 1 {
                    "no model to ask: it runs on Claude Code or Ollama".into()
                } else {
                    String::new()
                },
                ..Thread::default()
            }];
            q.shown = true;
            q.model = if n == 0 { "haiku".into() } else { String::new() };
            let mut screen = vec![Line::new(); 40];
            super::quickask::quick_over(&mut qm, &mut screen);
            put(&format!("quick{n}"), sw, &screen, sw);
        }

        let long = "a reason that goes on ".repeat(20);
        let lc = |l: &str| Choice {
            label: l.into(),
            description: long.clone(),
            ..Choice::default()
        };
        let freq = Question {
            id: "q5".into(),
            asks: vec![Ask {
                text: format!("Go? {}", "context words here ".repeat(12)),
                header: "A very long header that goes on".into(),
                options: vec![lc("one"), lc("two 漢字ラベル"), lc("three")],
                ..Ask::default()
            }],
            ..Question::default()
        };
        let mut fc = Conn {
            card_focus: true,
            ..Conn::default()
        };
        put(
            "qfold16",
            sw,
            &question_card(&mut fc, &freq, None, dw, 16),
            dw,
        );
        put(
            "qfold_full",
            sw,
            &question_card(&mut fc, &freq, None, dw, 0),
            dw,
        );
        put(
            "qfold_narrow",
            sw,
            &question_card(&mut fc, &freq, None, 30, 12),
            30,
        );
        question_key(&mut fc, &freq, None, "down", true, &mut String::new());
        put(
            "qfold_down",
            sw,
            &question_card(&mut fc, &freq, None, dw, 16),
            dw,
        );
        let w = sheet(112);
        rw.sel = 0;
        put("rewind_short", sw, &rw.body(&m, w, 8.min(h)), w);
        use super::fork::{EFFORT, FIRST, FOLDER, GO, MODEL, PERM};
        for row in [MODEL, EFFORT, PERM, FOLDER, FIRST, GO] {
            fk.row = row;
            put(&format!("fork_row{row}"), sw, &fk.body(&m, w, h), w);
        }
        fk.first = "carry on with the tests".chars().collect();
        fk.first_pos = 5;
        fk.row = FIRST;
        put("fork_first", sw, &fk.body(&m, w, h), w);
        fk.worktree = true;
        fk.repo = "/src/app".into();
        put("fork_wt", sw, &fk.body(&m, w, h), w);
        let w = sheet(76);
        let mut cs = CompactSheet {
            loading: true,
            ..CompactSheet::default()
        };
        put("compact_loading", sw, &cs.body(&m, w, h), w);
        let mut ex = ExportSheet {
            conn: "k".into(),
            text: "x".into(),
            filename: "c.txt".into(),
            dir: "/src/app".into(),
            err: "it could not be written: permission denied".into(),
            ..ExportSheet::default()
        };
        put("export_err", sw, &ex.body(&m, w, h), w);
        let w = sheet(usize::MAX);
        ms.tab = 1;
        put("message_tab1", sw, &ms.body(&m, w, h), w);

        for (name, imgs, tab) in [
            ("pic_one", vec!["/tmp/sc-oracle/img/a.png"], 1),
            (
                "pic_strip",
                vec![
                    "/tmp/sc-oracle/img/a.png",
                    "/tmp/sc-oracle/img/b.png",
                    "/tmp/sc-oracle/img/c.png",
                ],
                2,
            ),
        ] {
            let mut ps = MessageSheet::new(
                vec![UserMessage {
                    r#ref: "t1".into(),
                    text: "look".into(),
                    images: imgs.into_iter().map(String::from).collect(),
                }],
                0,
            );
            ps.tab = tab;
            let w = sheet(usize::MAX);
            put(name, sw, &ps.body(&m, w, h), w);
        }

        let mut dm = hosted(sw, 40);
        let d = decisions_of(
            "Decisions:\n1. **Scope:** (a) the full recommendation; (b) the easier alternative.\n2. **Answer format:** (a) `1a. Topic: label` lines; (b) keep `Q:/A:`.\nMy picks: 1a 2b.",
            "d1",
        );
        set_info(&mut dm, |i| i.decisions = d);
        let i = info(&dm, "k").unwrap();
        card_key(&mut dm, "k", &i, "up", true);
        let r = card_rows(&mut dm, "k", &i, dw, 0);
        put("decisions", sw, &r, dw);
    }
}
