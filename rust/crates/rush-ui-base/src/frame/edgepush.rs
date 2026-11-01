//! An arrow pushed into the edge of a box with text in it: the third
//! within a second asks to go past it, and a fourth (Confirm::again) says
//! yes. After Go's internal/ui/edgepush.go:8 (edgePush).

use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct EdgePush {
    key: String,
    at: Option<Instant>,
    n: u32,
}

impl EdgePush {
    /// Counts a push of key at now; true on the third in a row within a
    /// second of each other.
    pub fn push(&mut self, key: &str, now: Instant) -> bool {
        if key != self.key
            || self
                .at
                .is_none_or(|a| now.duration_since(a) > Duration::from_secs(1))
        {
            self.n = 0;
        }
        self.key = key.to_string();
        self.at = Some(now);
        self.n += 1;
        if self.n < 3 {
            return false;
        }
        self.n = 0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_push_asks() {
        let mut e = EdgePush::default();
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        assert!(!e.push("right", ms(0)));
        assert!(!e.push("right", ms(100)));
        assert!(e.push("right", ms(200)));
        // Counted afresh after.
        assert!(!e.push("right", ms(300)));
        // Another key, or a pause, starts again.
        assert!(!e.push("left", ms(400)));
        assert!(!e.push("left", ms(500)));
        assert!(!e.push("left", ms(1600)));
    }
}
