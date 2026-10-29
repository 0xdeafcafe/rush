//! What runs in the background for every screen: whether a newer rush is
//! out and #update (Go's update.go), #reload and what the next rush picks
//! up (reload.go), telling sessions an API error stopped to continue once
//! the API can be reached (online.go), and storing idle transcripts
//! compressed, said as news (squeeze.go:34 onSqueezed). A newer rush found is news, not the line
//! under the box (model.go:1257, messages.go:70). Each does its work off
//! the UI thread and only when a running rush has a loop to hand results
//! back to.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_fleet::Predicates;
use rush_update::Info;
use serde_json::{Map, Value, json};

use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

// --- updates ---

/// What's known of newer rushes.
#[derive(Default)]
pub struct Update {
    /// A newer rush that's out.
    pub newer: Option<Info>,
    pub updating: bool,
    checked: bool,
    /// A newer rush than this one is installed: #reload runs it.
    pub rebuilt: bool,
}

/// What an update does around it: rush's log and the network watch.
struct Host;

fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl rush_update::Host for Host {
    fn log(&self, kind: &str, f: Value) {
        rush_events::log(kind, fields(f));
    }
    fn net_run(&self, job: &str) -> bool {
        rush_net::netwatch::run(job)
    }
    fn net_done(&self, job: &str, err: Option<&str>) {
        let e = err.map(std::io::Error::other);
        rush_net::netwatch::done(
            job,
            e.as_ref().map(|e| e as &(dyn std::error::Error + 'static)),
        );
    }
}

impl Model {
    /// Asks, in the background, whether a newer rush is out (checkUpdate).
    pub(crate) fn check_update(&mut self) {
        let u = self.kept::<Update>();
        if u.checked {
            return;
        }
        u.checked = true;
        self.later(
            || rush_update::check(&Host),
            |m, (l, newer)| {
                if !newer {
                    return Out::Idle;
                }
                rush_events::log("update-available", fields(json!({"version": l.version})));
                m.news(
                    &format!("rush {} is out: #update installs it", l.short()),
                    "",
                );
                m.kept::<Update>().newer = Some(l);
                Out::Draw
            },
        );
    }

    /// #update: the newest rush over this one (installUpdate).
    pub(crate) fn install_update(&mut self) -> Out {
        if self.kept::<Update>().updating {
            self.flash("already updating…");
            return Out::Draw;
        }
        self.kept::<Update>().updating = true;
        self.flash("updating rush…");
        self.later(
            || rush_update::install(&Host),
            |m, res| {
                let u = m.kept::<Update>();
                u.updating = false;
                match res {
                    Err(e) => m.fail(&format!("update: {e}")),
                    Ok(to) => {
                        u.newer = None;
                        u.rebuilt = true;
                        m.flash(&format!(
                            "rush {} installed · #reload runs it, sessions carry on",
                            to.short()
                        ));
                        // Said already: the hosts' look doesn't say it
                        // again (model.go:1288).
                        m.kept::<crate::screens::settings::State>()
                            .updates
                            .rebuilt_said = true;
                    }
                }
                crate::screens::settings::updates::watch_hosts(m, false, false);
                Out::Draw
            },
        );
        Out::Draw
    }

    /// This window's rush, so two windows can be told apart, and what to
    /// do when it's behind (bars.go:157 versionTag).
    pub fn version_tag(&self) -> Line {
        let v = format!("rush {}", self.version);
        let u = self.kept.peek::<Update>();
        if u.is_some_and(|u| u.rebuilt) {
            return line([sp(format!("{v} · older than installed · #reload"), YELLOW)]);
        }
        if let Some(n) = u.filter(|u| !u.updating).and_then(|u| u.newer.as_ref()) {
            return line([sp(format!("{v} · {} is out · #update", n.short()), ORANGE)]);
        }
        line([sp(v, FAINT)])
    }
}

// --- #reload ---

/// What the next rush starts with when #reload asked for one: whether the
/// Session had the keys, then the agent selected.
pub const RESTORE_ENV: &str = "RUSH_RESTORE";

/// #reload's state.
#[derive(Default)]
pub struct Reload {
    pub reloading: bool,
    restore: String,
    restore_pane: bool,
    took: bool,
    /// Stands in for running the installed rush, in tests.
    pub installed: Option<fn() -> Result<(), String>>,
}

/// Whether the installed rush starts: --version, in 3s (installedRuns).
pub fn installed_runs() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return Ok(()),
            Ok(Some(s)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                return Err(format!("{s}: {}", err.trim()));
            }
            Ok(None) if start.elapsed() > Duration::from_secs(3) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("--version didn't answer in 3s".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

impl Model {
    /// #reload: once the installed rush is seen to start, this view quits
    /// for the binary to run it in its place, with reload_env; one that
    /// doesn't start leaves this view open, saying so (reload,
    /// reloadAfter).
    pub(crate) fn reload(&mut self, all: bool) -> Out {
        let runs = self.kept::<Reload>().installed.unwrap_or(installed_runs);
        self.later(
            move || {
                let r = runs();
                if r.is_ok() && all {
                    // The other views are told too.
                    rush_state::instances::reload(std::process::id() as i32);
                }
                r
            },
            |m, r| match r {
                Err(e) => {
                    rush_events::log("reload-failed", fields(json!({"err": e, "stayed": true})));
                    m.fail(&format!(
                        "#reload: the installed rush doesn't start, so this one stays · {e}"
                    ));
                    Out::Draw
                }
                Ok(()) => {
                    m.kept::<Reload>().reloading = true;
                    Out::Quit
                }
            },
        );
        Out::Draw
    }

    /// What the next rush starts with, when #reload asked for one: the
    /// binary sets it in the environment of the rush it runs (Reload).
    pub fn reload_env(&self) -> Option<String> {
        if !self.kept.peek::<Reload>().is_some_and(|r| r.reloading) {
            return None;
        }
        let pane = if self.session.is_some() { "1" } else { "0" };
        Some(format!("{RESTORE_ENV}={pane}{}", self.sel))
    }

    /// Reads what the last rush had picked, once (takeRestore).
    pub(crate) fn take_restore(&mut self, v: Option<String>) {
        let r = self.kept::<Reload>();
        if r.took {
            return;
        }
        r.took = true;
        if let Some(v) = v
            && v.len() > 1
        {
            r.restore_pane = v.starts_with('1');
            r.restore = v[1..].to_string();
        }
    }

    /// Selects the agent the last rush had, once it's listed
    /// (applyRestore).
    pub(crate) fn apply_restore(&mut self) {
        let want = self.kept::<Reload>().restore.clone();
        if want.is_empty() {
            return;
        }
        let Some(i) = self.agents.iter().position(|a| a.key == want) else {
            return;
        };
        let pane = self.kept::<Reload>().restore_pane;
        self.kept::<Reload>().restore.clear();
        self.sel = want;
        if pane {
            self.open(i);
        }
    }
}

// --- the API watch ---

/// How often rush looks at whether a waiting session may try again.
const ONLINE_EVERY: Duration = Duration::from_secs(5);

/// How many times one session is told to continue before rush leaves it
/// to you: an API that keeps failing gives up in the end.
pub const ONLINE_MOST: u32 = 8;

/// How long Claude Code's prompt cache lasts: it writes the one-hour
/// cache.
const CACHE_LIFE: Ms = 3_600_000;

/// How long after a halt a session is told to continue, once it has been
/// told tries times since it last answered: at once the first time, then
/// 15s, doubling each time (onlineBackoff).
pub fn online_backoff(tries: u32) -> Ms {
    if tries == 0 {
        return 0;
    }
    15_000i64.saturating_mul(1 << (tries - 1).min(30))
}

/// What the API watch keeps.
#[derive(Default)]
pub struct Online {
    checking: bool,
    checked: Option<Instant>,
    /// Agent key → the halt it was told to continue from.
    pub sent: std::collections::HashMap<String, Ms>,
    /// Agent key → continues sent since it last answered.
    pub tries: std::collections::HashMap<String, u32>,
    /// When each agent's first halt in a row was: its cache was last
    /// warmed before then, whatever its later tries did.
    pub first: std::collections::HashMap<String, Ms>,
    /// Agent key → the halt last told to netproof.
    recorded: std::collections::HashMap<String, Ms>,
}

/// What one look needs, taken from the model for the look's thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub failed: Vec<Ms>,
    pub answered: bool,
    /// Agent key → its cache is still warm.
    pub waiting: std::collections::HashMap<String, bool>,
}

fn halt_at(a: &Agent) -> Ms {
    a.spend.halt.as_ref().map_or(0, |h| h.at)
}

impl Model {
    /// The agents an API error stopped that are due to be told to
    /// continue, once they may (continueWaiting).
    pub(crate) fn continue_waiting(&mut self) -> Vec<usize> {
        let now = self.now;
        let w = self.kept::<Online>();
        let (sent, tries) = (w.sent.clone(), w.tries.clone());
        (0..self.agents.len())
            .filter(|&i| {
                let a = &self.agents[i];
                let at = halt_at(a);
                let t = tries.get(&a.key).copied().unwrap_or(0);
                a.continues(now)
                    && sent.get(&a.key) != Some(&at)
                    && t < ONLINE_MOST
                    && now >= at + online_backoff(t)
            })
            .collect()
    }

    /// What the next look needs: halts not yet shared, whether a session
    /// told to continue has answered, and who waits, warm or not (look).
    pub(crate) fn look(&mut self) -> Look {
        let now = self.now;
        let mut l = Look::default();
        let agents = self.agents.clone();
        let w = self.kept::<Online>();
        for a in &agents {
            let Some(h) = &a.spend.halt else {
                if w.tries.get(&a.key).copied().unwrap_or(0) > 0 {
                    l.answered = true; // it answered: proof for every other
                }
                w.tries.remove(&a.key);
                w.first.remove(&a.key);
                continue;
            };
            if !a.continues(now) {
                continue;
            }
            if w.recorded.get(&a.key) != Some(&h.at) {
                w.recorded.insert(a.key.clone(), h.at);
                l.failed.push(h.at);
            }
            w.first.entry(a.key.clone()).or_insert(h.at);
        }
        for i in self.continue_waiting() {
            let k = self.agents[i].key.clone();
            let first = self.kept::<Online>().first.get(&k).copied().unwrap_or(0);
            l.waiting.insert(k, now < first + CACHE_LIFE - 5 * 60_000);
        }
        l
    }

    /// Shares what's new about the API with every rush process, and looks
    /// at whether each waiting session may try again, off the UI thread
    /// (watchOnline).
    pub(crate) fn watch_online(&mut self) {
        let w = self.kept::<Online>();
        if w.checking || w.checked.is_some_and(|t| t.elapsed() < ONLINE_EVERY) {
            return;
        }
        let l = self.look();
        if l.failed.is_empty() && !l.answered && l.waiting.is_empty() {
            return;
        }
        self.kept::<Online>().checking = true;
        self.later(
            move || {
                let target = rush_net::netproof::target(&[]);
                if !l.failed.is_empty() || l.answered {
                    rush_events::log(
                        "api-watch",
                        fields(json!({"target": target, "newHalts": l.failed.len(), "answered": l.answered, "waiting": l.waiting.len()})),
                    );
                }
                let at = |ms: Ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
                for &f in &l.failed {
                    rush_net::netproof::fail(&target, at(f));
                }
                if l.answered {
                    rush_net::netproof::answer(&target, SystemTime::now());
                }
                l.waiting
                    .iter()
                    .filter(|(k, warm)| rush_net::netproof::may_go(&target, k, **warm))
                    .map(|(k, _)| k.clone())
                    .collect::<std::collections::HashSet<String>>()
            },
            |m, goes| {
                m.on_online(&goes);
                Out::Draw
            },
        );
    }

    /// Tells each session that may try again to continue: a few seconds
    /// apart, so they don't all send in the same moment (onOnline).
    fn on_online(&mut self, goes: &std::collections::HashSet<String>) {
        let w = self.kept::<Online>();
        w.checking = false;
        w.checked = Some(Instant::now());
        let mut n = 0u64;
        for i in self.continue_waiting() {
            let a = self.agents[i].clone();
            if !goes.contains(&a.key) {
                continue;
            }
            let h = a.spend.halt.clone().unwrap_or_default();
            let w = self.kept::<Online>();
            w.sent.insert(a.key.clone(), h.at);
            *w.tries.entry(a.key.clone()).or_default() += 1;
            let tries = w.tries[&a.key];
            let (key, wait) = (a.key.clone(), Duration::from_secs(3 * n));
            n += 1;
            let reg = self.reg.clone();
            let (kind, id, name) = (a.kind.to_string(), a.job.id.clone(), a.display_name.clone());
            let text = retry_prompt(&h.text);
            self.later(
                move || {
                    if !claim_continue(&rush_state::dir(), &key, h.at) {
                        return None; // another rush told it
                    }
                    std::thread::sleep(wait);
                    let err = reg
                        .get(&kind)
                        .and_then(|ad| {
                            let p = ad.profiles().into_iter().next().unwrap_or_default();
                            ad.replier().map(|r| r.reply(&p, &id, &text).map_err(|e| e.to_string()))
                        })
                        .unwrap_or_else(|| Err(format!("{kind} takes no messages from outside it")));
                    rush_events::log(
                        "api-continue",
                        fields(json!({"key": key, "name": name, "try": tries, "of": ONLINE_MOST, "haltAt": h.at, "err": err.as_ref().err()})),
                    );
                    Some(err.map(|_| name))
                },
                |m, r| {
                    match r {
                        Some(Ok(name)) => m.flash(&format!("the API is back · {name} continues")),
                        Some(Err(e)) => m.fail(&e),
                        None => return Out::Idle,
                    }
                    Out::Draw
                },
            );
        }
    }
}

/// What a session an API error stopped is told, to carry on
/// (host.RetryPrompt).
pub fn retry_prompt(reason: &str) -> String {
    format!(
        "Your previous run stopped with a provider error ({}). Continue from where you left off; check what you already did and do not redo it.",
        rush_core::failure::summary(reason)
    )
}

/// Claims telling one session to continue from one halt for this
/// process, so two rushes open at once don't both send it; claims a few
/// days old are cleared as it goes (claimContinue).
pub fn claim_continue(home: &std::path::Path, key: &str, at: Ms) -> bool {
    use std::io::ErrorKind;
    let dir = home.join("online");
    if std::fs::create_dir_all(&dir).is_err() {
        return true;
    }
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| {
                t.elapsed()
                    .is_ok_and(|d| d > Duration::from_secs(72 * 3600))
            });
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let name = claim_name(key, at);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(name))
    {
        Ok(_) => true,
        Err(e) => e.kind() != ErrorKind::AlreadyExists,
    }
}

/// The claim's file: the first 8 bytes of SHA-256 of key and the halt's
/// time, as Go names it.
fn claim_name(key: &str, at: Ms) -> String {
    use sha2_lite::sha256;
    let t = rush_core::time::format_rfc3339(at);
    let sum = sha256(format!("{key}\0{t}").as_bytes());
    sum[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256, for claim names that match Go's.
mod sha2_lite {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    pub fn sha256(b: &[u8]) -> [u8; 32] {
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut msg = b.to_vec();
        let bits = (b.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bits.to_be_bytes());
        for chunk in msg.as_chunks::<64>().0 {
            let mut w = [0u32; 64];
            for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
                w[i] = u32::from_be_bytes(*word);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *x = x.wrapping_add(y);
            }
        }
        let mut out = [0u8; 32];
        for (i, x) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
        }
        out
    }
}

// --- squeezing transcripts ---

/// When transcripts were last squeezed: two minutes after rush opens,
/// then every six hours.
pub struct Squeeze {
    next: Instant,
    running: bool,
}

impl Default for Squeeze {
    fn default() -> Self {
        Squeeze {
            next: Instant::now() + Duration::from_secs(120),
            running: false,
        }
    }
}

impl Model {
    /// Stores transcripts untouched for two days compressed, in the
    /// background: they read exactly as before, to every agent and
    /// everything else (squeezeTranscripts, onSqueezed).
    pub(crate) fn squeeze_transcripts(&mut self) {
        let s = self.kept::<Squeeze>();
        if s.running || Instant::now() < s.next {
            return;
        }
        s.running = true;
        s.next = Instant::now() + Duration::from_secs(6 * 3600);
        let dirs: Vec<PathBuf> = self
            .reg
            .native_agent()
            .and_then(|a| {
                let t = a.transcripts()?;
                Some(
                    a.profiles()
                        .iter()
                        .map(|p| PathBuf::from(t.transcripts_dir(p)))
                        .collect(),
                )
            })
            .unwrap_or_default();
        self.later(
            move || {
                if rush_state::Store::load().config.keep_transcripts_plain {
                    return rush_sys::squeeze::Pass::default();
                }
                rush_sys::squeeze::transcripts(&dirs, Duration::from_secs(48 * 3600))
            },
            |m, r| {
                m.kept::<Squeeze>().running = false;
                if r.files > 0 || r.failed > 0 {
                    rush_events::log(
                        "transcripts-compressed",
                        fields(json!({"files": r.files, "failed": r.failed, "bytesBefore": r.before, "bytesAfter": r.after})),
                    );
                }
                if r.files == 0 {
                    return Out::Idle;
                }
                // News, not the line under the box: you may be typing
                // there (squeeze.go:42).
                m.news(
                    &format!(
                        "compressed {} idle transcripts: {} → {} on disk · they read the same as before",
                        r.files,
                        super::parts::disk(r.before),
                        super::parts::disk(r.after)
                    ),
                    "",
                );
                Out::Draw
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::agent::{Halt, State};

    #[test]
    fn backs_off() {
        assert_eq!(online_backoff(0), 0);
        assert_eq!(online_backoff(1), 15_000);
        assert_eq!(online_backoff(3), 60_000);
    }

    #[test]
    fn sha256_known_answers() {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(&sha2_lite::sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha2_lite::sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn cut(key: &str, now: Ms, ago: Ms, text: &str) -> Agent {
        Agent {
            key: key.into(),
            display_name: key.into(),
            pid: 7,
            job: rush_core::agent::Job {
                state: State::Done,
                updated_at: now - ago,
                ..Default::default()
            },
            spend: rush_core::agent::Spend {
                halt: Some(Halt {
                    kind: "server_error".into(),
                    text: text.into(),
                    at: now - ago,
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // Once the API can be reached, each session an API error stopped is
    // told to continue once per halt, backing off, up to ONLINE_MOST.
    #[test]
    fn network_back_continues_what_it_stopped() {
        let mut m = super::super::feed::tests::model(100, 30);
        let now = m.clock.unwrap();
        m.now = now;
        let offline = "API Error: Unable to connect to API (ENOTFOUND)";
        m.agents = vec![cut("a", now, 60_000, offline)];
        let keys = |m: &mut Model| -> Vec<String> {
            m.continue_waiting()
                .into_iter()
                .map(|i| m.agents[i].key.clone())
                .collect()
        };
        assert_eq!(keys(&mut m), ["a"]);
        let at = halt_at(&m.agents[0]);
        m.kept::<Online>().sent.insert("a".into(), at);
        m.kept::<Online>().tries.insert("a".into(), 1);
        assert!(keys(&mut m).is_empty(), "told again for the same halt");
        m.agents[0] = cut(
            "a",
            now,
            10_000,
            "API Error: Connection dropped (ECONNRESET)",
        );
        assert!(keys(&mut m).is_empty(), "told again before its backoff");
        m.agents[0] = cut(
            "a",
            now,
            20_000,
            "API Error: Connection dropped (ECONNRESET)",
        );
        assert_eq!(keys(&mut m).len(), 1, "a new halt past its backoff");
        m.kept::<Online>().tries.insert("a".into(), ONLINE_MOST);
        assert!(keys(&mut m).is_empty(), "told past ONLINE_MOST");
        m.agents[0].spend.halt = None;
        let l = m.look();
        assert!(l.answered && !m.kept::<Online>().tries.contains_key("a"));
    }

    // A look shares each new halt once, and says whether each waiting
    // session is still warm: its cache dates from its first halt in a row.
    #[test]
    fn look_shares_halts_and_warmth() {
        let mut m = super::super::feed::tests::model(100, 30);
        let now = m.clock.unwrap();
        m.now = now;
        let dropped = "API Error: Connection dropped (ECONNRESET)";
        let mut cold = cut("cold", now, 2 * 60_000, dropped);
        cold.job.updated_at = now;
        m.agents = vec![cut("warm", now, 60_000, dropped), cold];
        m.kept::<Online>()
            .first
            .insert("cold".into(), now - 2 * 3_600_000);
        let l = m.look();
        assert!(
            l.failed.len() == 2 && l.waiting["warm"] && !l.waiting["cold"],
            "{l:?}"
        );
        assert!(m.look().failed.is_empty(), "halts shared twice");
    }

    #[test]
    fn continue_is_claimed_once() {
        let home = tempfile::tempdir().unwrap();
        let at = 1_790_000_000_000;
        assert!(claim_continue(home.path(), "k", at));
        assert!(
            !claim_continue(home.path(), "k", at),
            "the second rush finds it claimed"
        );
        assert!(
            claim_continue(home.path(), "k", at + 1000),
            "a new halt is a new claim"
        );
    }

    // #reload hands the next rush the agent selected and where the keys
    // were, and that rush picks them up once the agent is listed.
    #[test]
    fn reload_keeps_the_place() {
        let mut m = super::super::feed::tests::model(100, 30);
        m.sel = "claude:x:a".into();
        assert!(m.reload_env().is_none(), "no reload asked for");
        m.kept::<Reload>().installed = Some(|| Ok(()));
        assert_eq!(m.reload(false), Out::Draw);
        let env = m.reload_env().expect("reloading");
        let (k, v) = env.split_once('=').unwrap();
        assert_eq!((k, v), (RESTORE_ENV, "0claude:x:a"));
        let mut n = super::super::feed::tests::model(100, 30);
        n.take_restore(Some("1claude:x:a".into()));
        n.set_fleet(crate::worker::Fleet {
            agents: vec![Agent {
                key: "claude:x:a".into(),
                ..Default::default()
            }],
            roots: vec![String::new()],
            ..Default::default()
        });
        n.apply_restore();
        assert_eq!(n.sel, "claude:x:a");
    }

    // An installed rush that doesn't start leaves the view open, saying so.
    #[test]
    fn reload_stays_when_the_installed_rush_fails() {
        let mut m = super::super::feed::tests::model(100, 30);
        m.kept::<Reload>().installed = Some(|| Err("exit status 2: dyld: missing symbol".into()));
        m.reload(false);
        assert!(m.reload_env().is_none());
        let (said, _, err) = m.flash.clone().unwrap();
        assert!(
            err && said.contains("doesn't start") && said.contains("missing symbol"),
            "{said}"
        );
    }

    #[test]
    fn version_tags() {
        let mut m = super::super::feed::tests::model(100, 30);
        assert_eq!(m.version_tag().plain(), "rush test");
        m.kept::<Update>().newer = Some(Info {
            version: "v0.9.0".into(),
            time: 0,
        });
        assert_eq!(
            m.version_tag().plain(),
            "rush test · v0.9.0 is out · #update"
        );
        m.kept::<Update>().rebuilt = true;
        assert!(
            m.version_tag()
                .plain()
                .ends_with("older than installed · #reload")
        );
    }
}
