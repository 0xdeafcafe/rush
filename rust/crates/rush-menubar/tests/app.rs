//! The app drawn off screen, on the main thread (this test runs without
//! the test harness, so main is the main thread): the icon's poses, the
//! menu's rows of every kind, and the answers a menu item sends. Nothing
//! is shown and no feed starts.

fn main() {
    rush_testenv::assert_sandboxed(); // every test in a throwaway home
    #[cfg(target_os = "macos")]
    run();
}

#[cfg(target_os = "macos")]
fn run() {
    use rush_menubar::wire::{Account, Line, Waiting, Window, Working};
    let state = Line {
        accounts: Some(vec![
            Account {
                name: "work".into(),
                current: true,
                live: 2,
                five_hour: Window {
                    present: true,
                    percent: 95.0,
                    label: "5h".into(),
                    ..Window::default()
                },
                seven_day: Window {
                    present: true,
                    percent: 40.0,
                    name: "weekly".into(),
                    ..Window::default()
                },
                ..Account::default()
            },
            Account {
                name: "old".into(),
                problem: "sign-in expired".into(),
                ..Account::default()
            },
        ]),
        working: Some(vec![Working {
            key: "w".into(),
            name: "tests".into(),
            doing: "running pnpm test".into(),
            since: 1,
            ..Working::default()
        }]),
        more: Some(2),
        waiting: Some(vec![Waiting {
            key: "q".into(),
            name: "db".into(),
            needs: "asks: Which database?".into(),
            kind: "question".into(),
            req: "req-q".into(),
            text: "Which database?".into(),
            options: vec!["Postgres".into(), "SQLite".into()],
            since: 1,
            ..Waiting::default()
        }]),
        error: None,
    };
    let out = rush_menubar::app::check(state).expect("the app draws");
    let has = |s: &str| out.iter().any(|l| l.starts_with(s));
    for want in [
        "image 0",
        "image 1",
        "image 2",
        "view \"lost touch with rush; trying again…\"",
        "view \"db\\nWhich database?\"",
        "item \"Postgres\"",
        "item \"SQLite\"",
        "view \"tests\\nrunning pnpm test\"",
        "view \"and 2 more\"",
        "item \"Usage\"",
        "item \"Open rush\"",
        "item \"Open at Login\"",
        "item \"Quit\"",
        "answer op \"answer\"",
    ] {
        assert!(has(want), "no {want} in {out:#?}");
    }
    assert!(
        out.iter()
            .filter(|l| l.starts_with("view"))
            .all(|l| !l.ends_with(": 0 bytes")),
        "a row drew nothing: {out:#?}"
    );
    println!("app: {} things drawn", out.len());
}
