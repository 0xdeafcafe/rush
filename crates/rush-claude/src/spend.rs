//! Prices sessions' transcripts, each read only as far as it grew since
//! the last run. Port of internal/adapters/claude/spend.go and
//! agent/spend.go:33 (SpendTarget).
//!
//! What was read of each transcript (its Totals) is kept between runs, as
//! Go's state.CostCache kept it: CACHE_FILE in rush's cache folder. A
//! file of another version or format is dropped and rebuilt. It's read at the first
//! run, on the spend worker, and written after a run that read something
//! new, at most every SAVE_EVERY, and on flush (spend.go:83-190); written
//! beside, then renamed over, so a reader never sees half of one. A
//! transcript that shrank since is read again from its start (scan).
//!
//! As in Go (spend.go files: claude.SubagentTranscripts and
//! claude.WorkflowTranscripts, since 2c6ffb47), what a workflow's agents
//! spent counts too: Claude Code writes them under
//! subagents/workflows/<run>/, and they're the session's spend as much as
//! a subagent's is. Only their cost, tokens and today's share are taken:
//! not their folders, pull requests or last word, which stay the
//! session's and its subagents' (spend.go Run, `i >= len(files)-nFlows`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rush_core::agent::Spend;
use rush_core::time::Ms;
use rush_state::CostCache;

use crate::jobs::mtime;
use crate::json::day;
use crate::transcript::{Totals, scan, subagent_transcripts, subagents_dir};

/// Where Claude Code writes a workflow's agents, under the subagents
/// folder: one folder per run.
const WORKFLOWS: &str = "workflows";

/// The spend cache, in rush's cache folder.
pub const CACHE_FILE: &str = "costs.json";
/// The spend cache's format: a file of another starts over. Bump it when
/// Totals changes what it keeps.
/// 1: Totals as transcript.rs has it.
/// 2: and how the prompt cache lapsed (Totals::track).
/// 6: past the version of the Go rush's own costs.json, which was in
///    another format and is rebuilt.
pub const CACHE_VERSION: i64 = 6;
/// How often a run that read something saves the cache (spend.go:176).
pub const SAVE_EVERY: Ms = 30_000;

/// One session's transcript, to price.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub key: String,
    pub path: PathBuf,
    /// An agent that may be writing: its subagents' transcripts are
    /// checked every run. Others are only looked at when their own
    /// transcript or subagents folder changes.
    pub live: bool,
    /// A conversation nothing has open: looked at only every so often.
    pub past: bool,
}

/// How often a past conversation is looked at again.
pub const PAST_EVERY: Ms = 15_000;
/// How often a subagent's transcript that hasn't grown in RUN_STALE is
/// looked at: one woken again is priced that late.
pub const QUIET_EVERY: Ms = 30_000;
/// A subagent run quiet this long is taken as finished.
pub const RUN_STALE: Ms = 15 * 60_000;

/// How a target's files stood at its last scan.
#[derive(Clone, Debug, Default)]
struct Seen {
    main: u64,
    /// The times of its subagents folder and its workflows' folders,
    /// which change as runs start (folder_times).
    sub_dirs: Vec<Ms>,
    subs: Vec<PathBuf>,
    /// Its workflows' agents' transcripts (workflow_transcripts).
    flows: Vec<PathBuf>,
    /// Some quiet subagents weren't looked at: the scan after isn't skipped
    /// for nothing having changed until they are.
    deferred: bool,
    at: Ms,
}

/// What looking at a target's files found (Scanner::look).
enum Look {
    /// A past conversation looked at lately: left as it is.
    Skip,
    /// Nothing changed since its last scan.
    Same,
    /// Its files, their sizes (None: gone), and which grew.
    Files {
        seen: Seen,
        sizes: Vec<Option<u64>>,
        grew: Vec<bool>,
    },
}

/// Prices Claude Code's transcripts, keeping what it read.
#[derive(Default)]
pub struct Scanner {
    cache: CostCache<Totals>,
    /// The cache is kept in CACHE_FILE between runs: read at the first.
    kept: bool,
    /// Where it's kept, once read.
    file: Option<PathBuf>,
    saved: Ms,
    /// Each file's size as last read, and when it was last seen to grow
    /// (0: not since it was first read).
    sizes: HashMap<PathBuf, (u64, Ms)>,
    /// When the subagents' that haven't grown lately were last looked at.
    quiet_at: Ms,
    /// The folders transcripts worked in, each held once however many
    /// transcripts (and rows) name it: most name the same few.
    dirs: HashSet<Arc<str>>,
    seen: HashMap<PathBuf, Seen>,
    buf: Vec<u8>,
    day: u32,
}

impl Scanner {
    /// A scanner whose cache lives for the process only.
    pub fn new() -> Scanner {
        Scanner {
            cache: CostCache::new(CACHE_VERSION),
            buf: Vec::with_capacity(64 << 10),
            ..Scanner::default()
        }
    }

    /// A scanner that keeps its cache in CACHE_FILE between runs. It reads
    /// nothing until its first run, so that's off the UI thread, in the
    /// cache folder of the thread that runs it.
    pub fn kept() -> Scanner {
        Scanner {
            kept: true,
            ..Scanner::new()
        }
    }

    /// The totals kept for one transcript, as last scanned.
    pub fn totals(&self, path: &Path) -> Option<&Totals> {
        self.cache.files.get(&*path.to_string_lossy())
    }

    /// Writes the cache to its file, if it's kept and changed.
    pub fn save(&mut self) {
        if let Some(f) = &self.file {
            let _ = self.cache.save_to(f);
        }
    }

    /// Looks at a target's files, reading none: whether anything about
    /// one that isn't live changed since the last scan, and if so its
    /// transcripts (its own and its subagents' first, its workflows'
    /// agents' after them) and their sizes, as run takes them. Only looks,
    /// so a run looks at every target's side by side.
    fn look(&self, t: &Target, now: Ms, quiet: bool) -> Look {
        let prev = self.seen.get(&t.path);
        if let Some(p) = prev
            && t.past
            && !t.live
            && now - p.at < PAST_EVERY
        {
            return Look::Skip;
        }
        let main = std::fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0);
        let sub_dirs = folder_times(&t.path);
        let (subs, flows) = match prev {
            Some(p) if !t.live && p.main == main && p.sub_dirs == sub_dirs && !p.deferred => {
                return Look::Same;
            }
            Some(p) if p.sub_dirs == sub_dirs => (p.subs.clone(), p.flows.clone()),
            _ => (subagent_transcripts(&t.path), workflow_transcripts(&t.path)),
        };
        let n = 1 + subs.len() + flows.len();
        let (mut sizes, mut grew, mut deferred) = (Vec::with_capacity(n), vec![false; n], false);
        let files = std::iter::once(&t.path).chain(&subs).chain(&flows);
        for (i, f) in files.enumerate() {
            if let Some(&(sz, at)) = self.sizes.get(f)
                && i > 0
                && t.live
                && !quiet
                && now - at > RUN_STALE
            {
                sizes.push(Some(sz)); // a live session's subagent, quiet a while: as it was
                deferred = true;
                continue;
            }
            let Ok(m) = std::fs::metadata(f) else {
                sizes.push(None);
                continue;
            };
            sizes.push(Some(m.len()));
            grew[i] = self.sizes.get(f).map_or(0, |s| s.0) != m.len();
        }
        Look::Files {
            seen: Seen {
                main,
                sub_dirs,
                subs,
                flows,
                deferred,
                at: now,
            },
            sizes,
            grew,
        }
    }

    /// Reads the cache from CACHE_FILE, the first time, if it's kept.
    pub fn load(&mut self) {
        if self.kept && self.file.is_none() {
            let f = rush_state::cache_path(CACHE_FILE);
            // The file this cache had while the Go rush ran beside it.
            let _ = std::fs::remove_file(rush_state::cache_path("costs-rs.json"));
            self.cache = CostCache::load_from(&f, CACHE_VERSION);
            self.file = Some(f);
            let dirs = &mut self.dirs;
            for tot in self.cache.files.values_mut() {
                intern(dirs, &mut tot.dirs);
            }
        }
    }

    /// A transcript's spend as the cache last kept it, its subagents' and
    /// workflows' agents' with it, as run sums them, reading no
    /// transcript: what a row shows before its first scan. None when the
    /// cache has nothing of it (nothing at all when it's gone).
    pub fn kept_spend(&mut self, main: &Path, now: Ms) -> Option<Spend> {
        self.load();
        let key = main.to_string_lossy();
        let Some(tot) = self.cache.files.get(&*key) else {
            // A transcript that's gone is priced at nothing, as run would.
            return (!main.exists()).then(|| Spend {
                ready: true,
                ..Spend::default()
            });
        };
        let today = day(now);
        let mut sp = Spend::default();
        add(&mut sp, tot, true, true, today);
        // Under main's subagents folder, in the cache's order (run's, but
        // for workflows' agents listed after them).
        let subs = format!("{}/", subagents_dir(main).to_string_lossy());
        let flows = format!("{subs}{WORKFLOWS}/");
        for (f, tot) in self.cache.files.range(subs.clone()..) {
            if !f.starts_with(&subs) {
                break;
            }
            if !f.ends_with(".jsonl") {
                continue;
            }
            match f.strip_prefix(&flows) {
                // workflows/<run>/<agent>.jsonl
                Some(rest) if rest.matches('/').count() == 1 => {
                    add(&mut sp, tot, false, false, today)
                }
                Some(_) => {}
                None if !f[subs.len()..].contains('/') => add(&mut sp, tot, false, true, today),
                None => {}
            }
        }
        sp.ready = true;
        Some(sp)
    }

    /// Scans every target whose files grew and returns their new spend, by
    /// key.
    pub fn run(&mut self, targets: &[Target], now: Ms) -> HashMap<String, Spend> {
        self.load();
        let mut out = HashMap::new();
        let today = day(now);
        if today != self.day {
            // A new day: today's spend starts again, so everything is summed afresh.
            self.day = today;
            self.sizes.clear();
            self.seen.clear();
        }
        let quiet = now - self.quiet_at >= QUIET_EVERY;
        if quiet {
            self.quiet_at = now;
        }
        // Every target's files looked at side by side, then taken in order.
        let looks = rush_core::par::map_with(
            targets.iter().collect::<Vec<_>>(),
            || (),
            |_, t| self.look(t, now, quiet),
        );
        for (t, look) in targets.iter().zip(looks) {
            let (seen, sizes, grew) = match look {
                Look::Skip => continue,
                Look::Same => {
                    if let Some(p) = self.seen.get_mut(&t.path) {
                        p.at = now;
                    }
                    continue;
                }
                Look::Files { seen, sizes, grew } => (seen, sizes, grew),
            };
            let mut files = Vec::with_capacity(sizes.len());
            files.push(t.path.clone());
            files.extend(seen.subs.iter().cloned());
            let own = files.len();
            files.extend(seen.flows.iter().cloned());
            self.seen.insert(t.path.clone(), seen);
            let mut changed = false;
            for (f, _) in files.iter().zip(&grew).filter(|(_, g)| **g) {
                changed = true;
                // Its size is set as it's read, below.
                self.sizes.entry(f.clone()).or_default().1 = now;
            }
            if !changed && !self.sizes.is_empty() && self.sizes.contains_key(&t.path) {
                continue;
            }
            let mut sp = Spend::default();
            for (n, (f, size)) in files.iter().zip(&sizes).enumerate() {
                let tot = self.cache.get_mut(&f.to_string_lossy());
                // Of a session's many subagent transcripts, most haven't
                // grown since they were last read: their totals stand.
                let mut read = false;
                if size.is_none_or(|s| s != tot.size) {
                    let before = tot.offset;
                    let _ = scan(f, tot, &mut self.buf);
                    read = tot.offset != before;
                    if read {
                        intern(&mut self.dirs, &mut tot.dirs);
                    }
                }
                match self.sizes.get_mut(f) {
                    Some(s) => s.0 = tot.size,
                    None => {
                        self.sizes.insert(f.clone(), (tot.size, 0));
                    }
                }
                // A workflow's agent: what it spent, nothing more.
                add(&mut sp, tot, *f == t.path, n < own, today);
                if read {
                    self.cache.mark_dirty();
                }
            }
            if self.buf.capacity() > 8 << 20 {
                self.buf = Vec::with_capacity(64 << 10);
            }
            sp.ready = true;
            out.insert(t.key.clone(), sp);
        }
        if now - self.saved > SAVE_EVERY {
            self.save();
            self.saved = now;
            // Folders no transcript names any more.
            self.dirs.retain(|d| Arc::strong_count(d) > 1);
        }
        out
    }
}

/// Adds one transcript's totals to a target's spend: main is the
/// target's own transcript, ours one of its own (not a workflow's agent,
/// which adds what it spent and nothing more).
fn add(sp: &mut Spend, tot: &Totals, main: bool, ours: bool, today: u32) {
    let (c, u) = tot.spend();
    sp.cost += c;
    sp.usage.add(u);
    sp.today += tot.day_spend(today);
    if !main {
        sp.sub_cache.add(&tot.sub_cache());
    }
    if main {
        sp.model.clone_from(&tot.last_model);
        sp.first = tot.first;
        sp.last = tot.last;
        sp.halt.clone_from(&tot.halt);
        sp.progress.clone_from(&tot.progress);
        sp.progress_at = tot.progress_at;
        sp.context = tot.context();
        sp.compacts = tot.compacts;
        sp.dir.clone_from(&tot.dir);
        sp.dir_at = tot.dir_at;
    }
    if ours && tot.last > sp.last {
        sp.last = tot.last;
    }
    for p in tot.prs.iter().filter(|_| ours) {
        if !sp.prs.contains(p) {
            sp.prs.push(p.clone());
        }
    }
    for d in tot.dirs.iter().filter(|_| ours) {
        if !sp.dirs.contains(d) {
            sp.dirs.push(d.clone());
        }
    }
}

/// Swaps each of dirs for the one kept in `kept`, keeping it if new.
fn intern(kept: &mut HashSet<Arc<str>>, dirs: &mut [Arc<str>]) {
    for d in dirs {
        match kept.get(d) {
            Some(k) => *d = k.clone(),
            None => {
                kept.insert(d.clone());
            }
        }
    }
}

/// The times of a session's subagents folder, its workflows folder and
/// each workflow run's folder: one changes as a subagent, a run or a run's
/// agent starts. Empty past the first when there's no subagents folder.
fn folder_times(main: &Path) -> Vec<Ms> {
    let subs = subagents_dir(main);
    let Some(at) = mtime(&subs) else {
        return vec![0];
    };
    let mut out = vec![at];
    let flows = subs.join(WORKFLOWS);
    let Some(at) = mtime(&flows) else {
        return out;
    };
    out.push(at);
    let mut runs: Vec<(PathBuf, Ms)> = std::fs::read_dir(&flows)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_dir()
                .then(|| (e.path(), m.modified().map_or(0, crate::jobs::ms_of)))
        })
        .collect();
    runs.sort();
    out.extend(runs.into_iter().map(|(_, t)| t));
    out
}

/// The transcripts of the agents a session's workflows ran, sorted:
/// subagents/workflows/<run>/*.jsonl.
pub fn workflow_transcripts(main: &Path) -> Vec<PathBuf> {
    let Ok(runs) = std::fs::read_dir(subagents_dir(main).join(WORKFLOWS)) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = runs
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| std::fs::read_dir(e.path()).ok())
        .flat_map(|ents| ents.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tests::TempDir;
    use std::io::Write;

    fn grow(p: &Path) {
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(b"{}\n").unwrap();
    }

    fn setup(d: &TempDir) -> (PathBuf, PathBuf) {
        let main = d.0.join("s.jsonl");
        let subs = d.0.join("s").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let sub = subs.join("agent-a1.jsonl");
        std::fs::write(&main, "{}\n").unwrap();
        std::fs::write(&sub, "{}\n").unwrap();
        (main, sub)
    }

    fn size(p: &Path) -> u64 {
        std::fs::metadata(p).unwrap().len()
    }

    // A live session's subagent transcript quiet past RUN_STALE is looked
    // at every QUIET_EVERY, not every scan; the session's own, every scan.
    #[test]
    fn skips_quiet_subagents() {
        let d = TempDir::new("quiet");
        let (main, sub) = setup(&d);
        let mut s = Scanner::new();
        let targets = [Target {
            key: "k".into(),
            path: main.clone(),
            live: true,
            past: false,
        }];
        let t0 = rush_core::time::now();
        s.run(&targets, t0);
        assert!(
            s.sizes.contains_key(&sub),
            "the subagent's transcript wasn't read"
        );
        s.sizes.get_mut(&sub).unwrap().1 = t0 - RUN_STALE - 60_000;
        let was = s.sizes[&sub].0;
        grow(&sub);
        grow(&main);
        s.run(&targets, t0 + 1);
        assert_eq!(
            s.sizes[&sub].0, was,
            "a quiet subagent was looked at before QUIET_EVERY"
        );
        assert_eq!(
            s.sizes[&main].0,
            size(&main),
            "the session's own transcript wasn't looked at"
        );
        s.quiet_at = 0;
        s.run(&targets, t0 + 2);
        assert_eq!(
            s.sizes[&sub].0,
            size(&sub),
            "a quiet subagent wasn't looked at after QUIET_EVERY"
        );
    }

    // A subagent's write left unread while its session was live is read
    // once the session isn't, though its own transcript didn't change.
    #[test]
    fn catches_up_deferred() {
        let d = TempDir::new("deferred");
        let (main, sub) = setup(&d);
        let mut s = Scanner::new();
        let live = [Target {
            key: "k".into(),
            path: main.clone(),
            live: true,
            past: false,
        }];
        let t0 = rush_core::time::now();
        s.run(&live, t0);
        s.sizes.get_mut(&sub).unwrap().1 = t0 - RUN_STALE - 60_000;
        grow(&sub);
        s.run(&live, t0 + 1); // deferred
        let idle = [Target {
            live: false,
            ..live[0].clone()
        }];
        s.run(&idle, t0 + 2);
        assert_eq!(
            s.sizes[&sub].0,
            size(&sub),
            "the deferred write was never read"
        );
    }

    #[test]
    fn prices_a_session_and_its_subagents() {
        let d = TempDir::new("price");
        let (main, sub) = setup(&d);
        let line = |id: &str, out: i64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-09-23T10:00:00Z","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"https://github.com/a/b/pull/{out}"}}],"usage":{{"output_tokens":{out}}}}}}}"#
            )
        };
        std::fs::write(&main, line("m1", 1) + "\n").unwrap();
        std::fs::write(&sub, line("s1", 2) + "\n").unwrap();
        let mut s = Scanner::new();
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let now = rush_core::time::now();
        let out = s.run(&t, now);
        let sp = &out["k"];
        assert!(sp.ready);
        assert_eq!(sp.usage.output, 3);
        assert_eq!(sp.model, "claude-opus-5-5");
        assert_eq!(sp.prs.len(), 2);
        assert!((sp.cost - 3.0 * 20.0 / 1e6).abs() < 1e-12);
        // Nothing changed: nothing to report.
        assert!(s.run(&t, now + 1).is_empty());
    }

    /// A subagent that waited 7 minutes between requests: its cache (5
    /// minutes) had lapsed, so it wrote its context again where a read
    /// would have done. Opus 5.5: $4 a million in, a write 1.25×, a read
    /// $0.20 a million, and a 1-hour write 2×.
    #[test]
    fn counts_a_subagents_cache_lapse() {
        let d = TempDir::new("lapse");
        let (main, sub) = setup(&d);
        let req = |id: &str, at: &str, read: i64, wrote: i64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{at}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":{read},"cache_creation_input_tokens":{wrote}}}}}}}"#
            ) + "\n"
        };
        std::fs::write(&main, req("m1", "2026-09-23T10:00:00Z", 0, 50_000)).unwrap();
        std::fs::write(
            &sub,
            req("s1", "2026-09-23T10:00:00Z", 0, 100_000)
                + &req("s2", "2026-09-23T10:01:00Z", 100_000, 1_000)
                + &req("s3", "2026-09-23T10:08:00Z", 0, 101_000)
                + &req("s4", "2026-09-23T10:09:00Z", 101_000, 500),
        )
        .unwrap();
        let t = [Target {
            key: "k".into(),
            path: main,
            live: false,
            past: false,
        }];
        let sp = &Scanner::new().run(&t, rush_core::time::now())["k"];
        let c = sp.sub_cache;
        assert_eq!(
            c.lapses, 1,
            "the main session's own requests aren't counted"
        );
        assert!(
            (c.cost - 101_000.0 * (5.0 - 0.2) / 1e6).abs() < 1e-9,
            "{c:?}"
        );
        // The 1-hour write costs $3 a million more, on all but the rewrite.
        assert!((c.hour - 101_500.0 * 3.0 / 1e6).abs() < 1e-9, "{c:?}");
        assert!(c.hour_saves() > 0.0);
    }

    fn line(id: &str, out: i64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-23T10:00:00Z","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"x"}}],"usage":{{"output_tokens":{out}}}}}}}"#
        ) + "\n"
    }

    /// A session with a subagent, its cache kept in a temp cache folder.
    fn kept(d: &TempDir) -> (PathBuf, PathBuf, [Target; 1]) {
        let (main, sub) = setup(d);
        std::fs::write(&main, line("m1", 1)).unwrap();
        std::fs::write(&sub, line("s1", 2)).unwrap();
        let cache = d.0.join("cache");
        rush_state::set_thread_cache_dir(Some(cache.clone()));
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        (main, cache, t)
    }

    fn output(s: &mut Scanner, t: &[Target]) -> i64 {
        s.run(t, rush_core::time::now())["k"].usage.output
    }

    // A session's spend as the cache kept it, its subagent's and its
    // workflow's agent's with it, is what a run priced, with no transcript
    // read; nothing for a transcript the cache hasn't.
    #[test]
    fn kept_spend_is_the_runs() {
        let d = TempDir::new("keptspend");
        let (main, _, t) = kept(&d);
        let run = main
            .with_extension("")
            .join("subagents")
            .join(WORKFLOWS)
            .join("r1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("agent-w.jsonl"), line("w1", 5)).unwrap();
        let now = rush_core::time::now();
        let mut a = Scanner::kept();
        let ran = a.run(&t, now).remove("k").unwrap();
        a.save();
        let mut b = Scanner::kept();
        let got = b.kept_spend(&main, now).unwrap();
        assert_eq!(got.usage.output, 1 + 2 + 5);
        assert_eq!(format!("{got:?}"), format!("{ran:?}"));
        let other = d.0.join("other.jsonl");
        let gone = b.kept_spend(&other, now).unwrap();
        assert!(gone.ready && gone.cost == 0.0, "{gone:?}");
        std::fs::write(&other, line("o1", 1)).unwrap();
        assert!(b.kept_spend(&other, now).is_none(), "not priced yet");
    }

    // What one run read is saved, and the next run starts from it.
    #[test]
    fn keeps_the_cache_between_runs() {
        let d = TempDir::new("kept");
        let (main, cache, t) = kept(&d);
        let mut a = Scanner::kept();
        assert_eq!(output(&mut a, &t), 3);
        let file = cache.join(CACHE_FILE);
        let back: CostCache<Totals> = CostCache::load_from(&file, CACHE_VERSION);
        assert_eq!(back.version, CACHE_VERSION);
        assert_eq!(back.files.len(), 2);
        assert_eq!(
            back.files.get(&*main.to_string_lossy()),
            a.totals(&main),
            "the saved totals aren't what was read"
        );
        // Rewritten in place, its size the same: a scanner that read the
        // cache doesn't read it again.
        std::fs::write(&main, line("m1", 9)).unwrap();
        let mut b = Scanner::kept();
        assert_eq!(output(&mut b, &t), 3, "the transcript was read again");
    }

    // A transcript that grew since is read only from where the last run
    // stopped; one that shrank is read again from its start.
    #[test]
    fn reads_only_what_grew() {
        let d = TempDir::new("grew");
        let (main, _, t) = kept(&d);
        output(&mut Scanner::kept(), &t);
        // Its first line changed, at the same length, and a line added: only
        // the added line is read.
        std::fs::write(&main, line("m1", 9) + &line("m2", 4)).unwrap();
        assert_eq!(output(&mut Scanner::kept(), &t), 1 + 4 + 2);
        // Shorter than what was read: read again.
        std::fs::write(&main, line("m3", 7)).unwrap();
        assert_eq!(output(&mut Scanner::kept(), &t), 7 + 2);
    }

    // A cache of another version, or one that doesn't read, starts over.
    #[test]
    fn starts_over_on_another_version() {
        let d = TempDir::new("stale");
        let (main, cache, t) = kept(&d);
        std::fs::create_dir_all(&cache).unwrap();
        let file = cache.join(CACHE_FILE);
        let size = std::fs::metadata(&main).unwrap().len();
        let stale = format!(
            r#"{{"version":{},"files":{{"{}":{{"offset":{size},"size":{size},"cost":1000.0}}}}}}"#,
            CACHE_VERSION - 1,
            main.display()
        );
        for old in [
            stale.as_str(),
            "{not json",
            r#"{"version":1,"files":{"x":7}}"#,
        ] {
            std::fs::write(&file, old).unwrap();
            let mut s = Scanner::kept();
            let sp = s.run(&t, rush_core::time::now()).remove("k").unwrap();
            assert_eq!(sp.usage.output, 3, "{old}");
            assert!(sp.cost < 1.0, "{old}: the stale totals were used");
            let back: CostCache<Totals> = CostCache::load_from(&file, CACHE_VERSION);
            assert_eq!(back.files.len(), 2, "{old}: not written afresh");
        }
    }

    // A costs.json the Go rush left, of the version it had, is discarded
    // and rebuilt: it never stands for a transcript.
    #[test]
    fn rebuilds_gos_old_costs_json() {
        let d = TempDir::new("gos");
        let (main, cache, t) = kept(&d);
        std::fs::create_dir_all(&cache).unwrap();
        let gos = cache.join(CACHE_FILE);
        let size = std::fs::metadata(&main).unwrap().len();
        // Were it read, this would stand for the transcript.
        let theirs = format!(
            r#"{{"version":5,"files":{{"{}":{{"offset":{size},"size":{size},"cost":1000.0}}}}}}"#,
            main.display()
        );
        std::fs::write(&gos, &theirs).unwrap();
        let mut s = Scanner::kept();
        assert_eq!(output(&mut s, &t), 3);
        std::fs::write(&main, line("m1", 1) + &line("m2", 4)).unwrap();
        assert_eq!(output(&mut s, &t), 7);
        s.save();
        let back: CostCache<Totals> = CostCache::load_from(&gos, CACHE_VERSION);
        assert_eq!(back.version, CACHE_VERSION);
        assert_ne!(std::fs::read_to_string(&gos).unwrap(), theirs);
    }

    /// An assistant line that spent out output tokens, said a pull
    /// request, worked in dir, at the time given.
    fn said(id: &str, out: i64, pr: u32, dir: &str, at: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","cwd":"{dir}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"https://github.com/a/b/pull/{pr}"}}],"usage":{{"output_tokens":{out}}}}}}}"#
        ) + "\n"
    }

    fn workflow_agent(main: &Path, run: &str, name: &str, body: &str) -> PathBuf {
        let dir = subagents_dir(main).join(WORKFLOWS).join(run);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    // A workflow's agents' transcripts (subagents/workflows/<run>/) are
    // priced with the session's: their cost, tokens and today's share, as
    // Go's since 2c6ffb47 (spend.go files, claude.WorkflowTranscripts).
    #[test]
    fn prices_workflow_agents() {
        let d = TempDir::new("flows");
        let (main, sub) = setup(&d);
        std::fs::write(&main, said("m1", 1, 1, "/w/main", "2026-09-23T10:00:00Z")).unwrap();
        std::fs::write(&sub, said("s1", 2, 2, "/w/sub", "2026-09-23T10:01:00Z")).unwrap();
        workflow_agent(
            &main,
            "wf_1",
            "agent-a.jsonl",
            &said("w1", 4, 3, "/w/flow", "2026-09-23T11:00:00Z"),
        );
        workflow_agent(
            &main,
            "wf_2",
            "agent-b.jsonl",
            &said("w2", 8, 4, "/w/flow2", "2026-09-23T12:00:00Z"),
        );
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let sp = s_run(&mut Scanner::new(), &t);
        assert_eq!(sp.usage.output, 1 + 2 + 4 + 8);
        assert!((sp.cost - 15.0 * 20.0 / 1e6).abs() < 1e-12, "{}", sp.cost);
        // Its pull requests, folders and last word stay the session's and
        // its subagents'.
        assert_eq!(sp.prs.len(), 2, "{:?}", sp.prs);
        assert!(
            !sp.dirs.iter().any(|d| d.starts_with("/w/flow")),
            "{:?}",
            sp.dirs
        );
        assert_eq!(sp.last, last_of(&sub));
    }

    // Today's share is what the session, its subagents and its workflows'
    // agents spent on the run's local day, priced call by call: an earlier
    // day's calls count in the total, not today (spend.go Run,
    // sp.Today += tot.DaySpend(today); transcript.go:111 DaySpend).
    #[test]
    fn todays_share_is_the_days_calls() {
        let d = TempDir::new("today");
        let (main, sub) = setup(&d);
        let now = rush_core::time::parse_rfc3339("2026-09-23T12:00:00Z");
        let (before, earlier) = ("2026-09-23T11:00:00Z", "2026-09-21T12:00:00Z");
        std::fs::write(
            &main,
            said("m0", 100, 1, "/w", earlier) + &said("m1", 1, 1, "/w", before),
        )
        .unwrap();
        std::fs::write(&sub, said("s1", 2, 1, "/w", before)).unwrap();
        workflow_agent(
            &main,
            "wf_1",
            "agent-a.jsonl",
            &(said("w0", 200, 1, "/w", earlier) + &said("w1", 4, 1, "/w", before)),
        );
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let sp = Scanner::new().run(&t, now).remove("k").expect("priced");
        let per = 20.0 / 1e6; // an output token of Opus
        assert!((sp.cost - 307.0 * per).abs() < 1e-12, "{}", sp.cost);
        assert!((sp.today - 7.0 * per).abs() < 1e-12, "{}", sp.today);
    }

    fn s_run(s: &mut Scanner, t: &[Target]) -> Spend {
        s.run(t, rush_core::time::now())
            .remove("k")
            .expect("priced")
    }

    /// When a lone transcript's last line was written, as scanned.
    fn last_of(p: &Path) -> Ms {
        let mut tot = Totals::default();
        let _ = scan(p, &mut tot, &mut Vec::new());
        tot.last
    }

    // A session that isn't live is priced again when a workflow starts,
    // though its own transcript and subagents folder didn't change.
    #[test]
    fn a_new_workflow_is_priced() {
        let d = TempDir::new("newflow");
        let (main, sub) = setup(&d);
        std::fs::write(&main, line("m1", 1)).unwrap();
        std::fs::write(&sub, line("s1", 2)).unwrap();
        workflow_agent(&main, "wf_1", "agent-a.jsonl", &line("w1", 4));
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: false,
            past: false,
        }];
        let mut s = Scanner::new();
        let now = rush_core::time::now();
        assert_eq!(s.run(&t, now)["k"].usage.output, 7);
        assert!(s.run(&t, now + 1).is_empty(), "nothing changed");
        workflow_agent(&main, "wf_2", "agent-b.jsonl", &line("w2", 8));
        assert_eq!(s.run(&t, now + 2)["k"].usage.output, 15);
    }

    // A live session's workflow agent is read again as it grows.
    #[test]
    fn a_growing_workflow_agent_is_priced() {
        let d = TempDir::new("growflow");
        let (main, _) = setup(&d);
        let agent = workflow_agent(&main, "wf_1", "agent-a.jsonl", &line("w1", 4));
        let t = [Target {
            key: "k".into(),
            path: main.clone(),
            live: true,
            past: false,
        }];
        let mut s = Scanner::new();
        let now = rush_core::time::now();
        assert_eq!(s.run(&t, now)["k"].usage.output, 4);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&agent)
            .unwrap();
        f.write_all(line("w2", 5).as_bytes()).unwrap();
        assert_eq!(s.run(&t, now + 1)["k"].usage.output, 9);
    }

    // Only a run's folder's *.jsonl are a workflow's agents: not the
    // run's own record beside them, nor anything else.
    #[test]
    fn lists_workflow_agents() {
        let d = TempDir::new("listflows");
        let main = d.0.join("s.jsonl");
        assert!(workflow_transcripts(&main).is_empty(), "no folder: none");
        let b = workflow_agent(&main, "wf_2", "agent-b.jsonl", "");
        let a = workflow_agent(&main, "wf_1", "agent-a.jsonl", "");
        workflow_agent(&main, "wf_1", "agent-a.meta.json", "");
        let flows = subagents_dir(&main).join(WORKFLOWS);
        std::fs::write(flows.join("stray.jsonl"), "").unwrap();
        assert_eq!(workflow_transcripts(&main), [a, b]);
        // The subagents' own list is as Go's: the folder's *.jsonl only.
        assert!(subagent_transcripts(&main).is_empty());
    }
}
