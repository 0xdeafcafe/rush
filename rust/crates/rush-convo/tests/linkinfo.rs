//! Links rush knows the kind of: after internal/convo/linkinfo_test.go.
//! Lookups are on for this test binary alone, and only test kinds are
//! drawn: nothing asks GitHub.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_convo::linkinfo::{LinkKind, forget_links, link_info, look_links, register_link};
use rush_convo::render::inline;
use rush_core::text::{Line, Span, Style};

/// One test at a time: lookups are the process's.
static ONE: Mutex<()> = Mutex::new(());

fn draw(s: &str) -> String {
    inline(s, Style::default()).plain()
}

/// Waits for a link's lookup to answer.
fn wait_link(u: &str) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if link_info(u).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("{u} never answered");
}

// A kind registered once draws its badge after every link it knows, bare
// links and markdown ones alike, once its lookup has answered: never while
// the frame that first draws it is drawn (TestLinkKindBadge).
#[test]
fn link_kind_badge() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    look_links(true);
    let asked = Arc::new(AtomicU32::new(0));
    let a = asked.clone();
    register_link(LinkKind::<String> {
        name: "test-ticket",
        key: |u| {
            u.strip_prefix("https://tickets.test/")
                .map_or_else(String::new, |k| k.split('#').next().unwrap_or("").into())
        },
        look: Arc::new(move |key| {
            a.fetch_add(1, Ordering::SeqCst);
            Ok(format!("state of {key}"))
        }),
        badge: |v| Line::from_spans(vec![Span::new(format!("[{v}]"), Style::default())]),
        fresh: None,
    });
    let u = "https://tickets.test/T-1";
    let got = draw(&format!("see {u}"));
    assert!(
        !got.contains('['),
        "drawn with a badge before its lookup answered: {got:?}"
    );
    wait_link(u);
    assert_eq!(
        draw(&format!("see {u} now")),
        format!("see {u} [state of T-1] now")
    );
    assert_eq!(
        draw(&format!("see [the ticket]({u}#top)")),
        "see the ticket [state of T-1]"
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "asked more than once for one key"
    );
    look_links(false);
    forget_links();
}

// With lookups off nothing is asked or drawn (TestLinkKindOff).
#[test]
fn link_kind_off() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    look_links(false);
    let u = "https://github.com/acme/terrafied/pull/33";
    assert!(link_info(u).is_none(), "answered with lookups off");
    assert_eq!(draw(u), u);
}

// A failed lookup draws the link bare and isn't asked again at once
// (TestLinkKindFailed).
#[test]
fn link_kind_failed() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    look_links(true);
    let asked = Arc::new(AtomicU32::new(0));
    let a = asked.clone();
    register_link(LinkKind::<i32> {
        name: "test-broken",
        key: |u| {
            u.strip_prefix("https://broken.test/")
                .unwrap_or("")
                .to_string()
        },
        look: Arc::new(move |_| {
            a.fetch_add(1, Ordering::SeqCst);
            Err("signed out".into())
        }),
        badge: |_| Line::from_spans(vec![Span::new("never", Style::default())]),
        fresh: None,
    });
    let u = "https://broken.test/x";
    link_info(u);
    let end = Instant::now() + Duration::from_secs(5);
    while asked.load(Ordering::SeqCst) == 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(10));
    for _ in 0..3 {
        assert!(link_info(u).is_none(), "a failure answered");
    }
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    look_links(false);
    forget_links();
}
