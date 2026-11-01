//! Rust-only: one check every adapter's TailReader is held to, run by each
//! adapter's own tests on its own sessions: a session's end, read alone,
//! says what the end of the whole says.

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
