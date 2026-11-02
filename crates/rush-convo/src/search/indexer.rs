//! The indexer makes the word and piece indexes of the conversations it's
//! given, one at a time on a thread of its own, so the first search finds
//! them made: it never runs on the UI's, and it rests between
//! conversations so it takes little from the machine. Port of
//! internal/convo/indexer.go:1-97.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::find::{Library, Reader, Source, Stat, read_whole};
use super::pieces;

/// How long the indexer rests after each conversation it reads.
const INDEX_REST: Duration = Duration::from_millis(20);

#[derive(Default)]
pub(crate) struct Indexer {
    want: Vec<Source>,
    running: bool,
    /// The conversations whose pieces were found standing, or with nothing
    /// in them to index, as they stood: each reading of the list asks
    /// again of every agent's, and one unchanged since isn't looked at on
    /// disk (or read whole) again.
    standing: HashMap<String, Stat>,
}

impl Library {
    /// Has the conversations of srcs indexed, the first first, those
    /// already indexed passed over. It replaces what an earlier call asked
    /// for and returns at once. Nothing happens without a folder to keep
    /// them in (indexer.go:41 IndexInBackground).
    pub fn index_in_background(self: &Arc<Self>, srcs: Vec<Source>) {
        if self.dir().is_none() {
            return;
        }
        let mut ix = self.indexer.lock().unwrap();
        ix.want = srcs;
        if ix.running {
            return;
        }
        ix.running = true;
        let lib = self.clone();
        let spawned = std::thread::Builder::new()
            .name("rush-indexer".into())
            .spawn(move || lib.run_indexer());
        if spawned.is_err() {
            ix.running = false;
        }
    }

    fn run_indexer(&self) {
        let mut read = false;
        loop {
            let src = {
                let mut ix = self.indexer.lock().unwrap();
                if ix.want.is_empty() {
                    ix.running = false;
                    break;
                }
                ix.want.remove(0)
            };
            if self.index_one(&src) {
                read = true;
                std::thread::sleep(INDEX_REST);
            }
        }
        // A long conversation read whole is let go by now: its pages go
        // back to the system rather than staying rush's.
        if read {
            rush_sys::mem::give_back();
        }
    }

    /// Indexes src unless it's indexed as it stands; whether it read it
    /// (indexer.go:72 indexOne).
    pub fn index_one(&self, src: &Source) -> bool {
        let Ok((st, dir)) = Stat::of(&src.path) else {
            return false;
        };
        if dir || self.indexed_as_is(&src.path, st) {
            return false;
        }
        if let Ok(b) = read_whole(&src.path, &src.read)
            && let Some(s) = b.session()
        {
            self.index_session(&src.path, st, s);
        } else {
            let mut ix = self.indexer.lock().unwrap();
            ix.standing.insert(src.path.clone(), st);
        }
        true
    }

    /// Whether path's pieces are indexed as it stands; or it's still
    /// growing, its words read on as it does, to be indexed again once it
    /// settles.
    fn indexed_as_is(&self, path: &str, st: Stat) -> bool {
        if self.indexer.lock().unwrap().standing.get(path) == Some(&st) {
            return true;
        }
        if pieces::stand(self.dir().as_deref(), path, st) {
            let mut ix = self.indexer.lock().unwrap();
            ix.standing.insert(path.to_string(), st);
            return true;
        }
        self.index_for(path, st).is_some() && !st.settled()
    }
}

/// A Reader is cheap to clone: an Arc either way.
impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Reader::Transcript(_) => "Reader::Transcript",
            Reader::Other(_) => "Reader::Other",
        })
    }
}
