//! Rust-only: the agent list as last read, kept in rush's cache and drawn
//! as soon as rush opens, so it opens on your agents rather than "Reading
//! your agents…" while the first reading of every session runs. The live
//! reading replaces it whole when it lands (Model::set_fleet).
//!
//! Until then the kept list is only shown: its rows are drawn quietly
//! (dimmed, so a row that was working when rush last looked doesn't claim
//! to be now), the header still says "costing transcripts…", and nothing
//! acts on it: no host is dialled and no reading's hooks run. Kept a week
//! at most; one from another version of rush is left unread.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rush_core::agent::Agent;
use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

use crate::frame::Out;
use crate::model::Model;
use crate::worker::Note;

const FILE: &str = "list-kept.json";
/// What's kept older than this isn't shown.
const MOST_AGE: Ms = 7 * 24 * 3600 * 1000;
/// Saved at most this often while rush runs, and once more on quitting.
const EVERY: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Kept {
    version: String,
    saved_at: Ms,
    agents: Vec<Agent>,
    roots: Vec<String>,
}

/// Whether the list on screen is the kept one, not yet read again.
#[derive(Default)]
pub struct Showing(pub bool);

/// When the list was last saved.
#[derive(Default)]
struct Saved(Option<Instant>);

fn path() -> PathBuf {
    rush_state::cache_dir().join(FILE)
}

/// Reads the kept list off the UI thread and shows it, unless a live
/// reading has come first.
pub fn load(m: &Model) {
    let Some(tx) = m.sender() else { return };
    let tx = tx.lane(1);
    let version = m.version.clone();
    let _ = std::thread::Builder::new()
        .name("rush-list-kept".into())
        .spawn(move || {
            let Ok(b) = std::fs::read(path()) else { return };
            let Ok(k) = serde_json::from_slice::<Kept>(&b) else {
                return;
            };
            let now = rush_core::time::now();
            if k.version != version || now - k.saved_at > MOST_AGE || k.agents.is_empty() {
                return;
            }
            rush_draw::perf_note(|| format!("list kept read agents={}", k.agents.len()));
            tx.send(Note::Apply(Box::new(move |m: &mut Model| take(m, k))));
        });
}

fn take(m: &mut Model, k: Kept) -> Out {
    if m.loaded || !m.agents.is_empty() {
        return Out::Idle; // the live reading came first
    }
    m.agents = k.agents;
    m.roots = k.roots;
    m.kept::<Showing>().0 = true;
    m.rebuild();
    Out::Draw
}

/// After a live reading: the kept list is no longer what's shown, and
/// the reading is saved, at most once a minute.
pub fn took_reading(m: &mut Model, priced: bool) {
    m.kept::<Showing>().0 = false;
    if !priced {
        return;
    }
    let s = m.kept::<Saved>();
    if s.0.is_some_and(|t| t.elapsed() < EVERY) {
        return;
    }
    s.0 = Some(Instant::now());
    let k = kept(m);
    let _ = std::thread::Builder::new()
        .name("rush-list-save".into())
        .spawn(move || write(&k));
}

/// Saves the list now, as rush quits.
pub fn save_now(m: &mut Model) {
    if !m.loaded || m.kept::<Showing>().0 {
        return;
    }
    write(&kept(m));
}

fn kept(m: &Model) -> Kept {
    Kept {
        version: m.version.clone(),
        saved_at: rush_core::time::now(),
        agents: m.agents.clone(),
        roots: m.roots.clone(),
    }
}

fn write(k: &Kept) {
    let Ok(b) = serde_json::to_vec(k) else { return };
    let p = path();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, b).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kept list reads back as it was saved.
    #[test]
    fn kept_reads_back_whole() {
        let mut a = Agent {
            key: "claude:alex:abc".into(),
            display_name: "A name".into(),
            pid: 42,
            rush: true,
            ..Agent::default()
        };
        a.job.id = "abc".into();
        a.spend.cost = 1.5;
        let k = Kept {
            version: "v".into(),
            saved_at: 7,
            agents: vec![a.clone()],
            roots: vec!["/r".into()],
        };
        let b = serde_json::to_vec(&k).unwrap();
        let back: Kept = serde_json::from_slice(&b).unwrap();
        assert_eq!(back.agents, vec![a]);
        assert_eq!(back.roots, vec!["/r".to_string()]);
        assert_eq!((back.version.as_str(), back.saved_at), ("v", 7));
    }
}
