//! Durations as Go reads and writes them: the gate's settings ("30m",
//! "1h") and what it says of a run ("45m0s", "3s").

use std::time::Duration;

/// d as Go's Duration.String writes it: "30m0s", "1.5s", "0s".
pub fn string(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns == 0 {
        return "0s".into();
    }
    let trim = |v: u128, unit: u128, digits: usize| -> String {
        let whole = v / unit;
        let frac = v % unit;
        if frac == 0 {
            return whole.to_string();
        }
        let f = format!("{frac:0digits$}");
        format!("{whole}.{}", f.trim_end_matches('0'))
    };
    if ns < 1_000 {
        return format!("{ns}ns");
    }
    if ns < 1_000_000 {
        return format!("{}µs", trim(ns, 1_000, 3));
    }
    if ns < 1_000_000_000 {
        return format!("{}ms", trim(ns, 1_000_000, 6));
    }
    let secs = ns / 1_000_000_000;
    let (h, m) = (secs / 3600, secs / 60 % 60);
    let s = trim(ns % 60_000_000_000, 1_000_000_000, 9);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m{s}s"),
        (h, m) => format!("{h}h{m}m{s}s"),
    }
}

/// d rounded to the second, halves away from zero, as Go's
/// Duration.Round(time.Second).
pub fn round_secs(d: Duration) -> Duration {
    let ns = d.as_nanos();
    let r = ns % 1_000_000_000;
    let mut s = ns / 1_000_000_000;
    if r >= 500_000_000 {
        s += 1;
    }
    Duration::from_secs(s as u64)
}

/// s as Go's time.ParseDuration reads it, when it reads and is more than
/// nothing.
pub fn parse_positive(s: &str) -> Option<Duration> {
    match rush_events::parse_duration(s) {
        Ok(ns) if ns > 0 => Some(Duration::from_nanos(ns as u64)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_go_writes_them() {
        for (d, want) in [
            (Duration::ZERO, "0s"),
            (Duration::from_secs(30 * 60), "30m0s"),
            (Duration::from_secs(3600), "1h0m0s"),
            (Duration::from_secs(10), "10s"),
            (Duration::from_millis(1500), "1.5s"),
            (Duration::from_secs(45 * 60), "45m0s"),
        ] {
            assert_eq!(string(d), want);
        }
        assert_eq!(
            round_secs(Duration::from_millis(1499)),
            Duration::from_secs(1)
        );
        assert_eq!(
            round_secs(Duration::from_millis(1500)),
            Duration::from_secs(2)
        );
        assert_eq!(parse_positive("20m"), Some(Duration::from_secs(1200)));
        assert_eq!(parse_positive("0s"), None);
        assert_eq!(parse_positive("1"), None);
    }
}
