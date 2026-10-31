//! The command bar's transcript search, off the UI thread: once typing
//! pauses, every agent's conversation is read on a few threads, the most
//! recently active first, and what they find comes back in batches, so a
//! thousand files cost a handful of redraws. Port of
//! internal/ui/cmdbar.go:328-575 (barFound :342, barSearch :412, wait
//! :425, searchAgentTranscripts :455, conversationOf :562,
//! indexConversations :572) and the 200 ms pause of barChanged :278. The
//! searching itself is rush-convo's (search/find.rs).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rush_convo::Session;
use rush_convo::search::{Found, Reader, Source, library};
use rush_core::adapter::{self, Registry};
use rush_core::agent::Agent;
use rush_draw::Sender;

use crate::session;
use crate::worker::Note;

/// One agent's matches, the closest first (barFound).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarFound {
    pub key: String,
    pub name: String,
    /// The agent's place in the search order: most recent first.
    pub rank: usize,
    pub found: Found,
}

/// A search under way; dropping it stops it (barSearch.cancel).
pub struct Search {
    _cancel: Option<mpsc::Sender<()>>,
    pub started: Instant,
}

/// Where a batch of what was found goes: (model, gen, found, done).
pub type Took = fn(&mut crate::model::Model, u64, Vec<BarFound>, bool) -> crate::frame::Out;

/// A conversation to search: its agent's key and name, and where it is.
pub type Job = (String, String, Source);

/// How long typing pauses before transcripts are read.
const PAUSE: Duration = Duration::from_millis(200);
/// How long a batch gathers before it's handed to the UI.
const BATCH: Duration = Duration::from_millis(60);

impl Search {
    /// A search with no thread of its own: one done at once.
    pub fn idle() -> Search {
        Search {
            _cancel: None,
            started: Instant::now(),
        }
    }

    /// Searches the jobs for q on a few threads, after the pause when
    /// pause is set, handing back what they find in batches through tx
    /// (searchAgentTranscripts).
    pub fn start(tx: Sender<Note>, r#gen: u64, jobs: Vec<Job>, q: String, pause: bool) -> Search {
        Search::start_to(tx, r#gen, jobs, q, pause, super::bar::took)
    }

    /// start, each batch handed to took rather than the bar's (the
    /// Agents list's filter searches the same way: listfilter.go:93).
    pub fn start_to(
        tx: Sender<Note>,
        r#gen: u64,
        jobs: Vec<Job>,
        q: String,
        pause: bool,
        took: Took,
    ) -> Search {
        let (cancel, cancelled) = mpsc::channel::<()>();
        let tx = tx.lane(1);
        let _ = thread::Builder::new()
            .name("rush-barsearch".into())
            .spawn(move || {
                if pause {
                    match cancelled.recv_timeout(PAUSE) {
                        Err(RecvTimeoutError::Timeout) => {}
                        _ => return, // typed on, or closed
                    }
                }
                let stop = Arc::new(AtomicBool::new(false));
                let next = Arc::new(AtomicUsize::new(0));
                let jobs = Arc::new(jobs);
                let (res_tx, res_rx) = mpsc::channel::<BarFound>();
                for _ in 0..4.min(jobs.len()) {
                    let (stop, next, jobs, res_tx, q) = (
                        stop.clone(),
                        next.clone(),
                        jobs.clone(),
                        res_tx.clone(),
                        q.clone(),
                    );
                    let _ = thread::Builder::new()
                        .name("rush-barsearch".into())
                        .spawn(move || {
                            loop {
                                let i = next.fetch_add(1, Ordering::Relaxed);
                                if i >= jobs.len() || stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                let f = search_one(&jobs[i], i, &q);
                                if res_tx.send(f).is_err() {
                                    return;
                                }
                            }
                        });
                }
                drop(res_tx);
                let gone = || matches!(cancelled.try_recv(), Err(TryRecvError::Disconnected));
                let mut batch = Vec::new();
                let mut since = Instant::now();
                loop {
                    match res_rx.recv_timeout(BATCH) {
                        Ok(f) => batch.push(f),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => {
                            let b = std::mem::take(&mut batch);
                            tx.send(Note::Apply(Box::new(move |m| took(m, r#gen, b, true))));
                            return;
                        }
                    }
                    if gone() {
                        stop.store(true, Ordering::Relaxed);
                        return;
                    }
                    if !batch.is_empty() && since.elapsed() >= BATCH {
                        let b = std::mem::take(&mut batch);
                        if !tx.send(Note::Apply(Box::new(move |m| took(m, r#gen, b, false)))) {
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                        since = Instant::now();
                    }
                }
            });
        Search {
            _cancel: Some(cancel),
            started: Instant::now(),
        }
    }
}

fn search_one(j: &Job, rank: usize, q: &str) -> BarFound {
    let (key, name, src) = j;
    let found = library()
        .search(&src.path, q, 3, &src.read)
        .unwrap_or_default();
    BarFound {
        key: key.clone(),
        name: name.clone(),
        rank,
        found,
    }
}

/// Every job searched here and now: with no loop to hand batches to.
pub fn search_all(jobs: &[Job], q: &str) -> Vec<BarFound> {
    jobs.iter()
        .enumerate()
        .map(|(i, j)| search_one(j, i, q))
        .collect()
}

/// Where a's conversation is on disk to search: Claude Code's transcript,
/// or another agent's history read through its adapter (conversationOf);
/// None when there's none.
pub fn conversation_of(reg: &Registry, a: &Agent) -> Option<Source> {
    let path = a.job.transcript_path.clone();
    if path.is_empty() {
        return None;
    }
    match session::Source::of(reg, a)? {
        session::Source::Transcript { decode, .. } => Some(Source {
            path,
            read: Reader::Transcript(decode),
        }),
        session::Source::History {
            adapter: ad,
            session: s,
        } => {
            let read = Arc::new(Mutex::new((ad, s)));
            Some(Source {
                path,
                read: Reader::Other(Arc::new(move || {
                    let (ad, s) = &*read.lock().ok()?;
                    let r = ad.history_reader()?;
                    let evs = adapter::read_history(r, s, 0).ok()?;
                    let mut sess = Session::new();
                    for ev in &evs {
                        sess.apply(ev, 0);
                    }
                    Some(sess)
                })),
            })
        }
        session::Source::Hosted | session::Source::Room => None,
    }
}

/// Has every agent's conversation indexed in the background, the most
/// recently active first, for search to find made (indexConversations).
pub fn index_conversations(reg: &Registry, agents: &[Agent], now: rush_core::time::Ms) {
    use rush_fleet::Predicates;
    let mut v: Vec<&Agent> = agents.iter().collect();
    v.sort_by_key(|a| a.age(now));
    let srcs = v
        .into_iter()
        .filter_map(|a| conversation_of(reg, a))
        .collect();
    library().index_in_background(srcs);
}
