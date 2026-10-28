//! Prices sessions' transcripts, each read only as far as it grew since
//! the last run. Port of internal/adapters/claude/spend.go and
//! agent/spend.go:33 (SpendTarget). The cost cache lives in memory only:
//! Go also saves it to disk between runs (state.CostCache); not yet here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rush_core::agent::Spend;
use rush_core::time::Ms;

use crate::jobs::mtime;
use crate::json::day;
use crate::transcript::{Totals, scan, subagent_transcripts, subagents_dir};

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
    sub_dir: Ms,
    subs: Vec<PathBuf>,
    /// Some quiet subagents weren't looked at: the scan after isn't skipped
    /// for nothing having changed until they are.
    deferred: bool,
    at: Ms,
}

/// Prices Claude Code's transcripts, keeping what it read.
#[derive(Default)]
pub struct Scanner {
    cache: HashMap<PathBuf, Totals>,
    sizes: HashMap<PathBuf, u64>,
    /// When each file was last seen to grow, and when the subagents' that
    /// haven't lately were last looked at.
    grew: HashMap<PathBuf, Ms>,
    quiet_at: Ms,
    seen: HashMap<PathBuf, Seen>,
    buf: Vec<u8>,
    day: u32,
}

impl Scanner {
    pub fn new() -> Scanner {
        Scanner {
            buf: Vec::with_capacity(64 << 10),
            ..Scanner::default()
        }
    }

    /// The totals kept for one transcript, as last scanned.
    pub fn totals(&self, path: &Path) -> Option<&Totals> {
        self.cache.get(path)
    }

    /// A target's transcripts, or None when nothing about one that isn't
    /// live has changed since the last scan.
    fn files(&mut self, t: &Target, now: Ms) -> Option<Vec<PathBuf>> {
        let prev = self.seen.get_mut(&t.path);
        if let Some(p) = &prev
            && t.past
            && !t.live
            && now - p.at < PAST_EVERY
        {
            return None;
        }
        let main = std::fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0);
        let sub_dir = mtime(&subagents_dir(&t.path)).unwrap_or(0);
        let subs = match prev {
            Some(p) if !t.live && p.main == main && p.sub_dir == sub_dir && !p.deferred => {
                p.at = now;
                return None;
            }
            Some(p) if p.sub_dir == sub_dir => std::mem::take(&mut p.subs),
            _ => subagent_transcripts(&t.path),
        };
        let mut files = Vec::with_capacity(1 + subs.len());
        files.push(t.path.clone());
        files.extend(subs.iter().cloned());
        self.seen.insert(
            t.path.clone(),
            Seen {
                main,
                sub_dir,
                subs,
                deferred: false,
                at: now,
            },
        );
        Some(files)
    }

    /// Scans every target whose files grew and returns their new spend, by
    /// key.
    pub fn run(&mut self, targets: &[Target], now: Ms) -> HashMap<String, Spend> {
        let mut out = HashMap::new();
        let today = day(now);
        if today != self.day {
            // A new day: today's spend starts again, so everything is summed afresh.
            self.day = today;
            self.sizes.clear();
            self.seen.clear();
            self.grew.clear();
        }
        let quiet = now - self.quiet_at >= QUIET_EVERY;
        if quiet {
            self.quiet_at = now;
        }
        for t in targets {
            let Some(files) = self.files(t, now) else {
                continue;
            };
            let (mut changed, mut deferred) = (false, false);
            let mut sizes: Vec<Option<u64>> = Vec::with_capacity(files.len());
            for (i, f) in files.iter().enumerate() {
                if let Some(&sz) = self.sizes.get(f)
                    && i > 0
                    && t.live
                    && !quiet
                    && now - self.grew.get(f).copied().unwrap_or(0) > RUN_STALE
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
                if self.sizes.get(f).copied().unwrap_or(0) != m.len() {
                    changed = true;
                    self.grew.insert(f.clone(), now);
                }
            }
            if let Some(st) = self.seen.get_mut(&t.path) {
                st.deferred = deferred;
            }
            if !changed && !self.sizes.is_empty() && self.sizes.contains_key(&t.path) {
                continue;
            }
            let mut sp = Spend::default();
            for (f, size) in files.iter().zip(&sizes) {
                let tot = self.cache.entry(f.clone()).or_default();
                // Of a session's many subagent transcripts, most haven't
                // grown since they were last read: their totals stand.
                if size.is_none_or(|s| s != tot.size) {
                    let _ = scan(f, tot, &mut self.buf);
                }
                self.sizes.insert(f.clone(), tot.size);
                let (c, u) = tot.spend();
                sp.cost += c;
                sp.usage.add(u);
                sp.today += tot.day_spend(today);
                if *f == t.path {
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
                if tot.last > sp.last {
                    sp.last = tot.last;
                }
                for p in &tot.prs {
                    if !sp.prs.contains(p) {
                        sp.prs.push(p.clone());
                    }
                }
                for d in &tot.dirs {
                    if !sp.dirs.contains(d) {
                        sp.dirs.push(d.clone());
                    }
                }
            }
            if self.buf.capacity() > 8 << 20 {
                self.buf = Vec::with_capacity(64 << 10);
            }
            sp.ready = true;
            out.insert(t.key.clone(), sp);
        }
        out
    }
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
        s.grew.insert(sub.clone(), t0 - RUN_STALE - 60_000);
        let was = s.sizes[&sub];
        grow(&sub);
        grow(&main);
        s.run(&targets, t0 + 1);
        assert_eq!(
            s.sizes[&sub], was,
            "a quiet subagent was looked at before QUIET_EVERY"
        );
        assert_eq!(
            s.sizes[&main],
            size(&main),
            "the session's own transcript wasn't looked at"
        );
        s.quiet_at = 0;
        s.run(&targets, t0 + 2);
        assert_eq!(
            s.sizes[&sub],
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
        s.grew.insert(sub.clone(), t0 - RUN_STALE - 60_000);
        grow(&sub);
        s.run(&live, t0 + 1); // deferred
        let idle = [Target {
            live: false,
            ..live[0].clone()
        }];
        s.run(&idle, t0 + 2);
        assert_eq!(
            s.sizes[&sub],
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
}
