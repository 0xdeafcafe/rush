//! The group's screens at every width from nothing up, with wide
//! chars and empty boards: none of them may panic or run past the screen.
use super::stream::{Post, Stream};
use rush_core::agent::{Agent, Job, State};
use rush_draw::{Key, KeyMods};
use rush_room::community::Author;

fn posts(now: i64) -> Vec<Post> {
    let a = |n: &str, h: &str| Author {
        name: n.into(),
        handle: h.into(),
        ..Author::default()
    };
    let p = |key: &str, title: &str, au: Author, at: i64| Post {
        key: key.into(),
        id: key.split('/').next().unwrap().into(),
        title: title.into(),
        author: au,
        at,
        ..Post::default()
    };
    let m = 60_000;
    vec![
        Post {
            tags: vec!["#build".into()],
            ..p(
                "y/0",
                "yesterday's note about the #build cache",
                a("Ann", "ann-owl"),
                now - 26 * 60 * m,
            )
        },
        Post {
            project: "/work/rush".into(),
            tags: vec!["#rush".into()],
            ..p(
                "a/0",
                "go test hangs on fswait when the folder is gone and comes back again later",
                a("Worker", "pepper-bean"),
                now - 3 * m,
            )
        },
        Post {
            text: "-race and a 5s timeout found it".into(),
            reply: true,
            ..p(
                "a/1",
                "go test hangs on fswait",
                a("Other", "scout-dodo"),
                now - 2 * m,
            )
        },
        Post {
            project: "/work/rush".into(),
            tags: vec!["#rush".into()],
            ..p(
                "b/0",
                "flaky lint 🐓 wide chars ✓",
                a("Other", "scout-dodo"),
                now - 90_000,
            )
        },
        Post {
            to: "@scout-dodo".into(),
            full: "can you look at the cache? it never gets written".into(),
            tags: vec!["#dm".into()],
            ..p(
                "d/0",
                "can you look at the cache?",
                a("Worker", "pepper-bean"),
                now - m,
            )
        },
        Post {
            to: "@pepper-bean".into(),
            text: "on it".into(),
            reply: true,
            ..p(
                "d/1",
                "can you look at the cache?",
                a("Other", "scout-dodo"),
                now - 30_000,
            )
        },
    ]
}

fn agents(now: i64) -> Vec<Agent> {
    (0..30)
        .map(|i| {
            let (state, pid, needs) = match i % 3 {
                0 => (State::Working, 100 + i, ""),
                1 => (State::Done, 0, ""),
                _ => (State::Blocked, 100 + i, "which way?"),
            };
            Agent {
                key: format!("default/a:{i}"),
                display_name: format!("agent number {i} doing things"),
                repo: "/work/rush".into(),
                branch: "main".into(),
                pid,
                job: Job {
                    id: format!("{i:08x}"),
                    state,
                    needs: needs.into(),
                    cwd: "/work/rush".into(),
                    detail: "Looking at how the pane draws its rows and where the wrap happens"
                        .into(),
                    updated_at: now - i as i64 * 60_000,
                    ..Job::default()
                },
                ..Agent::default()
            }
        })
        .collect()
}

#[test]
fn never_panic_at_any_size() {
    let now = 1_790_000_000_000i64;
    let k = |s: &str| match s {
        "up" => (Key::Up, KeyMods::NONE),
        "tab" => (Key::Tab, KeyMods::NONE),
        "enter" => (Key::Enter, KeyMods::NONE),
        s => (Key::Char(s.chars().next().unwrap()), KeyMods::NONE),
    };
    let mut ps = posts(now);
    ps.push(Post {
        key: "w/0".into(),
        id: "w".into(),
        title: "日本語のタイトル🐓🐓🐓 #wide".into(),
        tags: vec!["#wide".into()],
        author: Author {
            name: "日本".into(),
            handle: "日本語".into(),
            ..Author::default()
        },
        to: "@x".into(),
        at: now,
        ..Post::default()
    });
    for w in [0u16, 1, 2, 5, 8, 12, 20, 33, 47, 60] {
        for h in [0u16, 1, 3, 6, 10, 24] {
            for keys in [
                vec![],
                vec!["up", "up"],
                vec!["n"],
                vec!["g"],
                vec!["[", "g"],
                vec!["]", "]"],
                vec!["[", "up", "enter"],
            ] {
                for empty in [false, true] {
                    let mut m = super::feed::tests::model(w as usize, h as usize);
                    m.clock = Some(now);
                    m.set_fleet(crate::worker::Fleet {
                        agents: agents(now),
                        roots: vec!["/work/rush".into(); 30],
                        ..Default::default()
                    });
                    if !empty {
                        m.kept::<Stream>().posts = ps.clone();
                    }
                    m.open_feed("");
                    let keys: Vec<_> = keys.iter().map(|s| k(s)).collect();
                    let f = crate::frame(&mut m, w, h, &keys, false);
                    assert!(
                        f.lines().all(|l| rush_core::text::width(l) <= w as usize),
                        "{w}x{h} {keys:?}:\n{f}"
                    );
                }
            }
            for hover in ["", "community:w/0", "community:a/0"] {
                let mut m = super::feed::tests::model(w as usize, h as usize);
                m.clock = Some(now);
                m.set_fleet(crate::worker::Fleet {
                    agents: agents(now),
                    roots: vec!["/work/rush".into(); 30],
                    ..Default::default()
                });
                m.kept::<Stream>().posts = ps.clone();
                m.hover = hover.into();
                let _ = crate::frame(&mut m, w, h, &[], true);
                let _ = m.stream_lines(w as usize, h as usize);
            }
        }
    }
}
