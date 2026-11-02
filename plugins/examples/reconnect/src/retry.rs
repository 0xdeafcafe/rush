//! The state machine, with no I/O and no clock: which sessions wait to be
//! continued, and when. Callers pass the time in.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The longest backoff between two tries.
pub const MAX_DELAY: Duration = Duration::from_secs(600);

/// Whether a session stopped by an error of this kind may be continued. A
/// usage limit, a login problem and anything else are left alone: sending
/// "continue" wouldn't help and might spend what's left.
pub fn retryable(kind: &str) -> bool {
    kind == "offline" || kind == "retryable"
}

/// What the settings say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Before the first try, and the backoff's base.
    pub wait: Duration,
    /// Continues sent before giving up.
    pub tries: u32,
    /// Retrying sessions rush says it's retrying itself.
    pub alongside: bool,
}

/// Reads the "wait", "tries" and "alongside" settings, falling back to
/// their defaults.
pub fn parse_config(v: &BTreeMap<String, String>) -> Config {
    let get = |k: &str| v.get(k).map_or("", String::as_str);
    Config {
        wait: parse_duration(get("wait"))
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_secs(15)),
        tries: get("tries")
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(5),
        alongside: get("alongside") == "true",
    }
}

/// A duration as Go writes them: "15s", "1m30s", "1.5h", "250ms".
pub fn parse_duration(s: &str) -> Option<Duration> {
    if s == "0" {
        return Some(Duration::ZERO);
    }
    let mut rest = s.strip_prefix('+').unwrap_or(s);
    if rest.is_empty() {
        return None;
    }
    let mut total = 0f64;
    while !rest.is_empty() {
        let n = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let num: f64 = rest[..n].parse().ok()?;
        rest = &rest[n..];
        let u = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = match &rest[..u] {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return None,
        };
        rest = &rest[u..];
        total += num * unit;
    }
    Duration::try_from_secs_f64(total).ok()
}

impl Config {
    /// How long to wait for a turn to start after try n (from 1) before
    /// the next: wait, 2×wait, 4×wait … up to MAX_DELAY.
    pub fn backoff(&self, n: u32) -> Duration {
        let mut d = self.wait;
        let mut i = 1;
        while i < n && d < MAX_DELAY {
            d *= 2;
            i += 1;
        }
        d.min(MAX_DELAY)
    }
}

/// A session an error stopped, that it means to continue.
#[derive(Debug, Default)]
struct Waiting {
    /// To say which, when giving up.
    name: String,
    /// Continues sent.
    tries: u32,
    /// When to send the next, or give up; None: wait for the network.
    next: Option<Instant>,
}

/// Something [`Retrier::due`] says to do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub session: String,
    pub name: String,
    /// True: say so and forget it; false: send "continue".
    pub give_up: bool,
    /// Which try this is, from 1.
    pub tries: u32,
}

/// Which sessions wait to be continued, and when.
pub struct Retrier {
    pub config: Config,
    /// Whether the network answers, as far as events said.
    up: bool,
    wait: BTreeMap<String, Waiting>,
}

impl Retrier {
    /// Starts with the network taken to be up: rush says when it goes
    /// down.
    pub fn new(config: Config) -> Retrier {
        Retrier {
            config,
            up: true,
            wait: BTreeMap::new(),
        }
    }

    /// Takes a session stopped by an error of kind. One of the kinds worth
    /// retrying is remembered (keeping the tries it's had, if it stopped
    /// again after a continue); any other forgets it.
    pub fn stopped(&mut self, id: &str, name: &str, kind: &str, now: Instant) {
        if !retryable(kind) {
            self.wait.remove(id);
            return;
        }
        let (up, config) = (self.up, self.config);
        let w = self.wait.entry(id.to_string()).or_default();
        w.name = name.to_string();
        w.next = if !up {
            None // network.up will set it
        } else if w.tries == 0 {
            // Up as far as we know. An offline stop usually comes with a
            // network.down; if it didn't, try after the wait all the same.
            Some(now + config.wait)
        } else {
            Some(now + config.backoff(w.tries))
        };
    }

    /// A turn starting: the session is going again, by our continue or
    /// anyone's.
    pub fn started(&mut self, id: &str) {
        self.wait.remove(id);
    }

    /// Drops a session without trying it.
    pub fn forget(&mut self, id: &str) {
        self.wait.remove(id);
    }

    /// The network going away: nothing is tried until it's back, and the
    /// time waited so far doesn't count against a session.
    pub fn down(&mut self) {
        self.up = false;
        for w in self.wait.values_mut() {
            w.next = None;
        }
    }

    /// The network back: each session waiting is continued now, then backs
    /// off as before.
    pub fn up(&mut self, now: Instant) {
        self.up = true;
        for w in self.wait.values_mut() {
            w.next = Some(now);
        }
    }

    /// What to do now: continue each session whose time has come, or give
    /// it up once it's had every try and the last one's backoff passed with
    /// no turn started. It moves on as if each were done.
    pub fn due(&mut self, now: Instant) -> Vec<Action> {
        let config = self.config;
        let mut out = Vec::new();
        let mut gone = Vec::new();
        for (id, w) in &mut self.wait {
            if w.next.is_none_or(|n| now < n) {
                continue;
            }
            if w.tries >= config.tries {
                out.push(Action {
                    session: id.clone(),
                    name: w.name.clone(),
                    give_up: true,
                    tries: w.tries,
                });
                gone.push(id.clone());
                continue;
            }
            w.tries += 1;
            w.next = Some(now + config.backoff(w.tries));
            out.push(Action {
                session: id.clone(),
                name: w.name.clone(),
                give_up: false,
                tries: w.tries,
            });
        }
        for id in gone {
            self.wait.remove(&id);
        }
        out
    }

    /// When due next has something to do, if it will.
    pub fn next(&self) -> Option<Instant> {
        self.wait.values().filter_map(|w| w.next).min()
    }

    /// How many sessions wait to be continued.
    pub fn waiting(&self) -> usize {
        self.wait.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            wait: secs(15),
            tries: 3,
            alongside: false,
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn settings(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_the_settings() {
        let c = parse_config(&BTreeMap::new());
        assert_eq!((c.wait, c.tries, c.alongside), (secs(15), 5, false));
        let c = parse_config(&settings(&[
            ("wait", "60s"),
            ("tries", "8"),
            ("alongside", "true"),
        ]));
        assert_eq!((c.wait, c.tries, c.alongside), (secs(60), 8, true));
        let c = parse_config(&settings(&[("wait", "soon"), ("tries", "0")]));
        assert_eq!(
            (c.wait, c.tries),
            (secs(15), 5),
            "bad values should fall back"
        );
        assert_eq!(parse_duration("1m30s"), Some(secs(90)));
        assert_eq!(parse_duration("1.5h"), Some(secs(5400)));
        assert_eq!(parse_duration("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("5"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let c = cfg();
        for (n, want) in [
            (1, secs(15)),
            (2, secs(30)),
            (3, secs(60)),
            (4, secs(120)),
            (20, MAX_DELAY),
        ] {
            assert_eq!(c.backoff(n), want, "backoff({n})");
        }
    }

    #[test]
    fn only_offline_and_retryable_are_retried() {
        let t0 = Instant::now();
        for kind in ["limit", "auth", "other", "too-long", ""] {
            let mut r = Retrier::new(cfg());
            r.stopped("s1", "", kind, t0);
            r.up(t0);
            assert!(
                r.waiting() == 0 && r.due(t0 + secs(3600)).is_empty(),
                "kind {kind:?}: would retry"
            );
        }
        // A later stop of another kind cancels one waiting.
        let mut r = Retrier::new(cfg());
        r.stopped("s1", "", "retryable", t0);
        r.stopped("s1", "", "limit", t0 + secs(1));
        assert_eq!(
            r.waiting(),
            0,
            "a limit after a retryable error should stop retrying"
        );
    }

    #[test]
    fn offline_waits_for_the_network() {
        let t0 = Instant::now();
        let mut r = Retrier::new(cfg());
        r.down();
        r.stopped("s1", "fix bug", "offline", t0);
        assert!(
            r.due(t0 + secs(3600)).is_empty() && r.next().is_none(),
            "tried while the network was down"
        );
        let up = t0 + secs(7200);
        r.up(up);
        let d = r.due(up);
        assert_eq!(
            d,
            [Action {
                session: "s1".into(),
                name: "fix bug".into(),
                give_up: false,
                tries: 1
            }],
            "on network.up, one continue for s1"
        );
    }

    #[test]
    fn retryable_waits_when_the_network_is_up() {
        let t0 = Instant::now();
        let mut r = Retrier::new(cfg());
        r.stopped("s1", "", "retryable", t0);
        assert!(r.due(t0 + secs(14)).is_empty(), "continued before the wait");
        let d = r.due(t0 + secs(15));
        assert!(d.len() == 1 && !d[0].give_up, "after the wait, due = {d:?}");
    }

    #[test]
    fn backs_off_then_gives_up() {
        let t0 = Instant::now();
        let mut r = Retrier::new(cfg()); // 3 tries, 15s base
        r.down();
        r.stopped("s1", "", "offline", t0);
        let mut now = t0 + secs(60);
        r.up(now);
        let mut at: Vec<(u64, bool)> = Vec::new();
        for _ in 0..100 {
            let Some(next) = r.next() else {
                break;
            };
            now = next;
            for d in r.due(now) {
                if d.give_up {
                    assert_eq!(d.tries, 3, "gave up after {} tries, want 3", d.tries);
                }
                at.push(((now - t0).as_secs(), d.give_up));
            }
        }
        // Continues at the network coming back, then 15s and 30s after;
        // giving up 60s after the last with no turn started.
        assert_eq!(at, [(60, false), (75, false), (105, false), (165, true)]);
        assert_eq!(r.waiting(), 0, "still waiting after giving up");
    }

    #[test]
    fn turn_started_stops_retrying() {
        let t0 = Instant::now();
        let mut r = Retrier::new(cfg());
        r.stopped("s1", "", "retryable", t0);
        r.due(t0 + secs(15)); // try 1
        r.started("s1");
        assert!(
            r.waiting() == 0 && r.due(t0 + secs(3600)).is_empty(),
            "kept retrying after the session went again"
        );
    }

    #[test]
    fn stopping_again_keeps_the_tries_spent() {
        let t0 = Instant::now();
        let mut r = Retrier::new(cfg());
        r.stopped("s1", "", "retryable", t0);
        r.due(t0 + secs(15)); // try 1
        // The continue failed the same way: it waits the backoff for try 1
        // from now, and counts on from there.
        r.stopped("s1", "", "retryable", t0 + secs(20));
        assert_eq!(r.next(), Some(t0 + secs(35)));
        let d = r.due(t0 + secs(35));
        assert!(d.len() == 1 && d[0].tries == 2, "due = {d:?}, want try 2");
    }

    #[test]
    fn network_down_pauses_and_up_resumes_all() {
        let t0 = Instant::now();
        let mut r = Retrier::new(cfg());
        r.stopped("s1", "", "retryable", t0);
        r.stopped("s2", "", "offline", t0);
        r.down();
        assert!(r.due(t0 + secs(3600)).is_empty(), "tried while down");
        let up = t0 + secs(7200);
        r.up(up);
        assert_eq!(r.due(up).len(), 2, "on up, both");
    }
}
