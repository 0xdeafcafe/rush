//! The tests of rush-ui-social's feed.rs that need the whole UI.

use crate::frame::overlayclick::MouseEv;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;
use rush_core::text::Line;
use rush_room::community::{self, Author};
use rush_ui_social::feed::*;
use rush_ui_social::ink::*;
use rush_ui_social::stream::{Post, Stream};

fn author(name: &str) -> Author {
    Author {
        name: name.into(),
        ..Author::default()
    }
}

pub fn feed_model() -> Model {
    let mut m = crate::kit::model(100, 35);
    let now = m.clock.unwrap();
    m.kept::<Stream>().posts = vec![
        Post {
            key: "a/0".into(),
            id: "a".into(),
            title: "go test hangs on fswait".into(),
            author: author("Worker"),
            at: now - 60_000,
            ..Post::default()
        },
        Post {
            key: "b/0".into(),
            id: "b".into(),
            title: "flaky lint".into(),
            author: author("Other"),
            at: now - 30_000,
            ..Post::default()
        },
        Post {
            key: "a/1".into(),
            id: "a".into(),
            title: "go test hangs on fswait".into(),
            author: Author {
                session_id: "s1".into(),
                ..Author::default()
            },
            text: "-race and a 5s timeout found it".into(),
            at: now,
            reply: true,
            ..Post::default()
        },
    ];
    m
}

fn sheet(m: &mut Model) -> &mut FeedSheet {
    m.sheet_as::<FeedSheet>().expect("the feed is open")
}

fn body(m: &mut Model, w: usize, h: usize) -> String {
    let mut s = m.sheet.take().expect("a sheet");
    let out = s.body(m, w, h);
    m.sheet = Some(s);
    out.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

fn press(m: &mut Model, k: &str) {
    let mut s = m.sheet.take().expect("a sheet");
    let out = s.key(m, &Press::of(k).expect("a key"));
    if out != Out::Close && m.sheet.is_none() {
        m.sheet = Some(s);
    }
}

#[test]
fn switches() {
    let mut m = feed_model();
    m.kept::<Stream>().on = Some(false);
    m.open_feed("");
    assert!(m.sheet.is_none(), "the sheet opened while the feed is off");
    m.kept::<Stream>().on = Some(true);
    m.open_feed("");
    assert!(m.sheet.is_some(), "the sheet did not open once on");
}

#[test]
fn timeline_is_one_view() {
    let mut m = feed_model();
    m.open_feed("");
    let text = body(&mut m, 90, 30);
    let (first, reply, other) = (
        text.find("go test hangs").unwrap(),
        text.find("↳ @").unwrap(),
        text.find("flaky lint").unwrap(),
    );
    assert!(
        first < reply && reply < other && text.contains("-race and a 5s"),
        "{text}"
    );
    let posts = threaded(m.kept::<Stream>().posts.clone());
    assert_eq!(sheet(&mut m).cursor(&posts), Some(2));
    for key in ["enter", "space"] {
        let s = sheet(&mut m);
        s.composing = false;
        s.picked = "a/1".into();
        s.follow = false;
        press(&mut m, "up");
        press(&mut m, key);
        let s = sheet(&mut m);
        assert!(
            s.composing && s.reply_to == "a",
            "{key} replies to the picked post"
        );
    }
    m.paste("board draft");
    assert_eq!(
        sheet(&mut m).input.iter().collect::<String>(),
        "board draft"
    );
    press(&mut m, "esc");
    press(&mut m, "esc");
    assert!(m.sheet.is_none(), "esc closes the sheet");
}

#[test]
fn renders_within_bounds() {
    for (w, h) in [(44, 24), (140, 45)] {
        let mut m = feed_model();
        m.kept::<Stream>().posts[0].title = format!("question\x1b[2J{}", " long".repeat(60));
        m.open_feed("new");
        let mut s = m.sheet.take().unwrap();
        let lines = s.body(&m, w - 10, h - 6);
        assert!(lines.len() <= h - 6, "{w}x{h} clips controls");
        for l in &lines {
            assert!(
                l.width() <= w - 10 && !l.plain().contains('\x1b'),
                "bad line {:?}",
                l.plain()
            );
        }
    }
}

#[test]
fn day_separators_and_jump() {
    let mut m = feed_model();
    let now = m.clock.unwrap();
    m.kept::<Stream>().posts[0].at = now - 3 * 86_400_000;
    m.kept::<Stream>().posts[1].at = now - 86_400_000;
    m.open_feed("");
    let text = body(&mut m, 90, 30);
    let old = format!("── {} ──", day_label(now - 3 * 86_400_000, now));
    let (a, b) = (
        text.find(&old).unwrap(),
        text.find("── Yesterday ──").unwrap(),
    );
    assert!(a < b && !text.contains("── Today"), "{text}");
    assert!(text.contains("⇧↑ ⇧↓ Day"), "{text}");
    for (key, want) in [
        ("shift+up", "a/0"),
        ("shift+up", "a/0"),
        ("shift+down", "b/0"),
        ("shift+down", "b/0"),
        ("up", "a/1"),
        ("shift+down", "b/0"),
    ] {
        press(&mut m, key);
        assert_eq!(sheet(&mut m).picked, want, "{key}");
    }
}

#[test]
fn picks_and_wraps() {
    let mut m = feed_model();
    m.kept::<Stream>().posts[1].title = format!("{}the end", "a long flaky lint story ".repeat(12));
    m.open_feed("b/0");
    let text = body(&mut m, 90, 40);
    assert!(
        text.lines()
            .any(|l| l.starts_with('▍') && l.contains("the end")),
        "the picked post marked and wrapped in full:\n{text}"
    );
    assert!(text.starts_with("the feed 🐓"), "{text}");
}

#[test]
fn sorts_by_hashtag() {
    let mut m = feed_model();
    let now = m.clock.unwrap();
    {
        let ps = &mut m.kept::<Stream>().posts;
        for (i, p) in ["/src/rush", "/src/haven", "/src/rush"].iter().enumerate() {
            ps[i].project = p.to_string();
            ps[i].tags = vec![community::project_tag(p)];
        }
        ps[1].tags.push("#build".into());
        ps.push(Post {
            key: "c/0".into(),
            id: "c".into(),
            project: "/src/haven".into(),
            tags: vec!["#haven".into()],
            title: "flaky lint".into(),
            author: author("Other"),
            at: now,
            ..Post::default()
        }); // a lone repeat, hidden
    }
    m.open_feed("");
    sheet(&mut m).open("#rush");
    let text = body(&mut m, 90, 30);
    assert!(
        text.contains(" #all   #haven   #rush   #build ")
            && text.contains("go test hangs")
            && !text.contains("flaky lint"),
        "the #rush tab shows only rush's chirps, under its tabs:\n{text}"
    );
    press(&mut m, "]");
    let text = body(&mut m, 90, 30);
    assert!(
        sheet(&mut m).tag == "#build"
            && !text.contains("go test hangs")
            && text.matches("flaky lint").count() == 1,
        "{text}"
    );
    let row = sheet(&mut m).tab_row.unwrap() as i32;
    let mut s = m.sheet.take().unwrap();
    s.mouse(&mut m, MouseEv::Press, 2, row);
    m.sheet = Some(s);
    assert_eq!(
        sheet(&mut m).tag,
        community::ALL,
        "a click on #all opens it"
    );
    press(&mut m, "[");
    press(&mut m, "n");
    assert_eq!(sheet(&mut m).input.iter().collect::<String>(), "#build ");
}

// An author's run of chirps is signed once, @handle then their name;
// another author or a new day signs again, and a reply carries its own
// @handle.
#[test]
fn signs_each_run_once() {
    let mut m = feed_model();
    let now = m.clock.unwrap();
    let hub = Author {
        session_id: "s-hub".into(),
        name: "Hub Header Sims Integration".into(),
        ..Author::default()
    };
    let mk = |k: &str, title: &str, a: &Author, mins: i64, reply: bool| Post {
        key: k.into(),
        id: k[..1].into(),
        title: title.into(),
        text: if reply { title.into() } else { String::new() },
        author: a.clone(),
        at: now - mins * 60_000,
        reply,
        ..Post::default()
    };
    m.kept::<Stream>().posts = vec![
        mk("a/0", "first tip", &hub, 5, false),
        mk("b/0", "second tip", &hub, 4, false),
        mk("b/1", "thanks", &author("You"), 3, true),
        mk("c/0", "third tip", &hub, 2, false),
        mk("d/0", "a word from another", &author("Other"), 1, false),
        mk("e/0", "back again", &hub, 0, false),
    ];
    m.open_feed("");
    let text = body(&mut m, 90, 40);
    let handle = m.kept::<Stream>().posts[0].handle();
    let sign = format!("▎ {handle} (Hub Header Sims Integration)");
    assert_eq!(text.matches(&sign).count(), 2, "{text}");
    assert_eq!(text.matches("@other").count(), 1, "{text}");
    assert!(text.contains("↳ @you  thanks"), "{text}");
    assert!(
        text.find("second tip") < text.find("thanks")
            && text.find("thanks") < text.find("third tip")
    );
    let rows: Vec<&str> = text.lines().collect();
    let from = rows.iter().position(|l| l.starts_with("── ")).unwrap();
    let gaps: Vec<usize> = rows[from..]
        .iter()
        .take_while(|l| !l.contains("back again"))
        .enumerate()
        .filter(|(_, l)| l.trim().is_empty())
        .map(|(i, _)| i)
        .collect();
    assert!(
        gaps.len() == 2 && gaps[1] - gaps[0] == 3,
        "{gaps:?}\n{text}"
    );
}

// A chirp's wrapped lines share its first line's left edge; a reply's
// sit under its @handle. None runs past the row.
#[test]
fn rows_wrap_aligned() {
    let m = crate::kit::model(100, 30);
    let now = m.clock.unwrap();
    let long = "words that wrap ".repeat(8);
    for p in [
        Post {
            title: long.clone(),
            author: author("Worker"),
            at: now,
            ..Post::default()
        },
        Post {
            text: long.clone(),
            author: author("You"),
            at: now,
            reply: true,
            ..Post::default()
        },
    ] {
        let rows = m.community_rows(&p, BLUE, 3, 50, SEL_BG, now);
        let first = rows[0].plain();
        let edge = if p.reply {
            cells(&first[..first.find('@').unwrap()])
        } else {
            cells(&first[..first.find("words").unwrap()])
        };
        for r in &rows[1..] {
            let r = r.plain();
            let lead = cells(&r) - cells(r.trim_start_matches(' '));
            assert!(lead == edge && cells(&r) <= 50, "{edge} {r:?}");
        }
        assert!(rows.len() >= 3);
    }
}

// An agent's message to another says who to; it shows whole once
// picked.
#[test]
fn shows_an_agents_whole_message_when_picked() {
    let m = crate::kit::model(100, 30);
    let now = m.clock.unwrap();
    let mut p = Post {
        text: "Asks for the tests".into(),
        full: "Run the tests.\n\nThen the linter.".into(),
        to: "@pepper-bean".into(),
        author: author("Worker"),
        at: now,
        ..Post::default()
    };
    let plain = |m: &Model, p: &Post| {
        m.community_rows(p, BLUE, 3, 60, SEL_BG, now)
            .iter()
            .map(Line::plain)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let shut = plain(&m, &p);
    assert!(
        shut.contains("→ @pepper-bean Asks for the tests …") && !shut.contains("linter"),
        "{shut}"
    );
    p.whole = true;
    let open = plain(&m, &p);
    assert!(
        open.contains("→ @pepper-bean Run the tests.")
            && open.contains("Then the linter.")
            && !open.contains("Asks for"),
        "{open}"
    );
}

/// Cells from..to of a row, as text.
fn cols(l: &Line, from: usize, to: usize) -> String {
    crate::frame::dialog::cells(l, from, to).plain()
}

// The sheet in a frame 120×40, as Go's `rush --render 120x40 text=#feed
// enter` draws it with an empty board: the box 114 wide from column 3,
// its title, the empty board's words and its keys where Go has them.
#[test]
fn frame_at_go_s_width() {
    let mut m = crate::kit::model(120, 40);
    m.open_feed("");
    let f = m.frame();
    assert_eq!(f.len(), 40);
    let inner = |s: &str| format!("│ {} │", fit_str(s, 110));
    assert_eq!(cols(&f[1], 3, 117), format!("╭{}╮", "─".repeat(112)));
    assert_eq!(cols(&f[2], 3, 117), inner(""));
    assert_eq!(
        cols(&f[3], 3, 117),
        inner("the feed 🐓  one for every project · a #hashtag sorts it")
    );
    assert_eq!(cols(&f[5], 3, 117), inner("No chirps yet."));
    assert_eq!(
        cols(&f[6], 3, 117),
        inner("Agents chirp with: rush feed chirp \"…\"")
    );
    assert_eq!(
        cols(&f[36], 3, 117),
        inner("↑ ↓ Scroll · ⇧↑ ⇧↓ Day · [ ] Hashtag · enter Reply · n Chirp · g Graph · esc Close")
    );
    assert_eq!(cols(&f[38], 3, 117), format!("╰{}╯", "─".repeat(112)));
}

// Opened at a post, it's picked; the pick follows its post when the
// board shifts.
#[test]
fn opens_at_a_post() {
    let mut m = feed_model();
    m.open_feed("b/0");
    let posts = threaded(m.kept::<Stream>().posts.clone());
    assert_eq!(sheet(&mut m).cursor(&posts), Some(2));
    m.kept::<Stream>().posts.insert(
        0,
        Post {
            key: "z/0".into(),
            id: "z".into(),
            at: 1_790_000_000_000 - 3_600_000,
            ..Post::default()
        },
    );
    let posts = threaded(m.kept::<Stream>().posts.clone());
    let s = sheet(&mut m);
    assert!(s.picked == "b/0" && s.cursor(&posts) == Some(3));
}

// Scrolled to the newest, the newest sits on the footer and the view
// opens right under the tabs' gap on a name or a day: partway down a
// run, its name is pinned in that gap.
#[test]
fn view_pins_its_runs_name() {
    let mut m = feed_model();
    let now = m.clock.unwrap();
    let mut ps = Vec::new();
    for i in 0..14i64 {
        let id = ((b'a' + i as u8) as char).to_string();
        ps.push(Post {
            key: format!("{id}/0"),
            id: id.clone(),
            title: format!("{id} {}", "a long chirp that wraps ".repeat(4)),
            author: author(["Ann", "Bo"][(i / 3 % 2) as usize]),
            at: now + (i - 14) * 60_000,
            ..Post::default()
        });
        if i % 4 == 1 {
            ps.push(Post {
                key: format!("{id}/1"),
                id,
                text: "a long reply that wraps ".repeat(5),
                author: author("Cy"),
                at: now + (i - 14) * 60_000 + 1000,
                reply: true,
                ..Post::default()
            });
        }
    }
    m.kept::<Stream>().posts = ps;
    m.open_feed("");
    let all = m.kept::<Stream>().posts.clone();
    let posts = sheet(&mut m).shown(&all);
    let name = regex::Regex::new(r"^(▎ @|── )").unwrap();
    let pin = regex::Regex::new(r"^▎ (@|\s*\d+[smhd]\s+↳ @)").unwrap();
    let mut pinned = 0;
    for h in 14..24 {
        let mut s = m.sheet.take().unwrap();
        let lines = s.body(&m, 60, h);
        m.sheet = Some(s);
        let (gap, top) = (lines[1].plain(), lines[2].plain());
        let view: String = lines.iter().map(|l| l.plain() + "\n").collect();
        if gap == " ".repeat(60) {
            assert!(
                name.is_match(&top),
                "at {h} tall the view opens partway down a run without its name:\n{view}"
            );
        } else {
            pinned += 1;
            let s = sheet(&mut m);
            let (p, mut t) = (s.rows[&1], s.rows[&2]);
            while t > 0 && posts[t].reply && posts[t].key != posts[p].key {
                t -= 1; // a reply's first line pins its run's name
            }
            assert!(
                pin.is_match(&gap) && posts[p].author == posts[t].author,
                "at {h} tall the run's own name should be pinned under the tabs:\n{view}"
            );
        }
        let foot = lines
            .iter()
            .position(|l| l.plain().starts_with("↑ ↓"))
            .unwrap();
        let last = lines[foot - 2].plain();
        assert!(
            last.contains("wraps") && last.starts_with('▍'),
            "at {h} tall the newest sits on the footer:\n{view}"
        );
    }
    assert!(
        pinned > 0,
        "some height opens partway down a run, its name pinned"
    );
}

#[test]
fn routine_or_news() {
    let e = |said: &str| AgentEvent {
        glyph: "✓".into(),
        said: said.into(),
        ..AgentEvent::default()
    };
    assert!(e("all tests pass").routine());
    assert!(!e("turns out the cache was stale").routine());
    assert!(!e("Found it: a race").routine());
}
