//! Rust-only: the checks every adapter's TailReader and HistoryWindow are
//! held to, run by each adapter's own tests on its own sessions: a
//! session's end, read alone, says what the end of the whole says; and a
//! session paged back a window at a time loses no message of the whole.

use super::{Adapter, Session, read_history};
use crate::event::{Event, Message, PartKind};

/// The messages of evs, from the first you typed (a user's message that
/// isn't a tool's result): what comes before it in an end may answer calls
/// made before the end began.
fn from_a_prompt(evs: &[Event]) -> Vec<&Message> {
    let ms: Vec<&Message> = evs
        .iter()
        .filter_map(|e| match e {
            Event::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    let first = ms.iter().position(|m| {
        m.role == "user"
            && m.parts.iter().any(|p| p.kind == PartKind::Text)
            && !m.parts.iter().any(|p| p.kind == PartKind::ToolResult)
    });
    ms[first.unwrap_or(ms.len())..].to_vec()
}

/// Holds ad's TailReader to its HistoryReader on session s, read with an
/// end of `most` bytes, which s is meant to be longer than: the end's
/// messages, from its first prompt, are the whole's last ones; and an end
/// as long as the whole leaves nothing out and is the whole. Panics with
/// what differs.
pub fn check_tail(ad: &dyn Adapter, s: &Session, most: i64) {
    let name = ad.name();
    let tr = ad
        .tail_reader()
        .unwrap_or_else(|| panic!("{name}: no TailReader"));
    let hr = ad
        .history_reader()
        .unwrap_or_else(|| panic!("{name}: no HistoryReader"));
    let whole = read_history(hr, s, 0).unwrap_or_else(|e| panic!("{name}: whole: {e}"));
    let all = from_a_prompt(&whole);
    assert!(!all.is_empty(), "{name}: the whole has no prompt");

    let (end, cut) = tr
        .history_tail(s, most)
        .unwrap_or_else(|e| panic!("{name}: end: {e}"));
    assert!(cut, "{name}: an end of {most} bytes left nothing out");
    let got = from_a_prompt(&end);
    assert!(!got.is_empty(), "{name}: the end has no prompt");
    assert!(got.len() < all.len(), "{name}: the end is all of it");
    assert_eq!(got, all[all.len() - got.len()..], "{name}: the end differs");

    let (end, cut) = tr
        .history_tail(s, i64::MAX / 4)
        .unwrap_or_else(|e| panic!("{name}: whole end: {e}"));
    assert!(!cut, "{name}: an end as long as it left some out");
    assert_eq!(from_a_prompt(&end), all, "{name}: the whole end differs");
}

/// A message said in evs: its role with its text parts, to tell one said
/// again from one lost.
fn said(evs: &[Event]) -> Vec<(String, String)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Message(m) if m.role == "user" || m.role == "assistant" => {
                let t = m
                    .parts
                    .iter()
                    .filter(|p| p.kind == PartKind::Text)
                    .map(|p| p.text.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                (!t.is_empty()).then(|| (m.role.clone(), t))
            }
            _ => None,
        })
        .collect()
}

/// Holds ad's HistoryWindow to its HistoryReader on session s, read with
/// windows of `most` bytes, which s is meant to be longer than: a window
/// over the whole says what the whole says; and the whole paged back one
/// window at a time — each window's first group naming where the next
/// reads down to, as the remote app pages history — loses no message the
/// whole says. A page may say one again (an adapter's head lines, a turn
/// a boundary cuts), never fewer. Panics with what differs.
pub fn check_window(ad: &dyn Adapter, s: &Session, most: i64) {
    let name = ad.name();
    let w = ad
        .history_window()
        .unwrap_or_else(|| panic!("{name}: no HistoryWindow"));
    let hr = ad
        .history_reader()
        .unwrap_or_else(|| panic!("{name}: no HistoryReader"));
    let whole = read_history(hr, s, 0).unwrap_or_else(|e| panic!("{name}: whole: {e}"));
    let all = from_a_prompt(&whole);
    assert!(!all.is_empty(), "{name}: the whole has no prompt");
    let size = std::fs::metadata(&s.transcript)
        .unwrap_or_else(|e| panic!("{name}: transcript: {e}"))
        .len();

    let flat = |g: &[(u64, Vec<Event>)]| -> Vec<Event> {
        g.iter().flat_map(|(_, evs)| evs.iter().cloned()).collect()
    };
    let groups = w
        .history_window(s, 0, size)
        .unwrap_or_else(|e| panic!("{name}: whole window: {e}"));
    assert!(
        groups.windows(2).all(|g| g[0].0 < g[1].0),
        "{name}: groups out of order"
    );
    assert_eq!(
        from_a_prompt(&flat(&groups)),
        all,
        "{name}: the whole window differs"
    );

    // Paged back: each page reads [from, at), from a window's width below
    // at, and the next page ends at the first group the window holds at
    // or after from (an adapter's head lines sit before it).
    let mut at = size;
    let mut paged: Vec<(String, String)> = Vec::new();
    for page in 0..10_000 {
        let mut from = at.saturating_sub(most.max(1) as u64);
        let groups = loop {
            let g = w
                .history_window(s, from, at)
                .unwrap_or_else(|e| panic!("{name}: page {page}: {e}"));
            if g.iter().any(|(o, _)| *o >= from) || from == 0 {
                break g;
            }
            from = from.saturating_sub(most.max(1) as u64);
        };
        let next = groups
            .iter()
            .find(|(o, _)| *o >= from)
            .map(|(o, _)| *o)
            .unwrap_or(0);
        assert!(next < at, "{name}: page {page} made no progress");
        paged.extend(said(&flat(&groups)));
        at = next;
        if at == 0 {
            break;
        }
    }
    assert_eq!(at, 0, "{name}: paging never reached the start");
    let wants = said(&whole);
    for m in &wants {
        let left = paged.iter().filter(|x| x == &m).count();
        let want = wants.iter().filter(|x| x == m).count();
        assert!(left >= want, "{name}: paging lost {m:?}");
    }
}
