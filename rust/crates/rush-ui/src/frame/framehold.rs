//! Background work drawn at most every quarter second: readings, scans
//! and reads land several times a second, and each would draw a whole
//! frame whether or not what it read had changed. What you do (keys, the
//! mouse, a resize) draws at once. After Go's internal/ui/framehold.go
//! (backgroundGap :13, held :16, holdBackground :27, heldFrame :49).

use std::time::{Duration, Instant};

/// The least time between frames that work from the background draws.
pub const BACKGROUND_GAP: Duration = Duration::from_millis(250);

/// When the last frame was drawn, and the frame held back for
/// background work since.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub drew_at: Instant,
    /// The frame owed: when it's due.
    pub due: Option<Instant>,
}

impl Default for Hold {
    fn default() -> Hold {
        Hold {
            // Long enough ago that the first background work draws.
            drew_at: Instant::now() - BACKGROUND_GAP,
            due: None,
        }
    }
}

impl Hold {
    /// Background work wants a frame at now: true to draw it now; false
    /// when it's held, a frame then owed for when the gap is up
    /// (holdBackground).
    pub fn background(&mut self, now: Instant) -> bool {
        let at = self.drew_at + BACKGROUND_GAP;
        if now >= at {
            return true;
        }
        self.due.get_or_insert(at);
        false
    }

    /// A frame was drawn: anything owed is in it.
    pub fn drew(&mut self, now: Instant) {
        self.drew_at = now;
        self.due = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_within_the_gap() {
        let mut h = Hold::default();
        let t0 = Instant::now();
        assert!(h.background(t0), "the first draws");
        h.drew(t0);
        assert!(!h.background(t0 + Duration::from_millis(10)));
        assert_eq!(h.due, Some(t0 + BACKGROUND_GAP));
        assert!(!h.background(t0 + Duration::from_millis(100)));
        assert_eq!(h.due, Some(t0 + BACKGROUND_GAP), "one frame owed, not two");
        assert!(h.background(t0 + BACKGROUND_GAP));
        h.drew(t0 + BACKGROUND_GAP);
        assert_eq!(h.due, None);
    }
}
