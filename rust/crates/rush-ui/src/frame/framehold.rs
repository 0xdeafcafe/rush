//! Background work drawn at most every quarter second: readings, scans
//! and reads land several times a second, and each would draw a whole
//! frame whether or not what it read had changed. What you do (keys, the
//! mouse, a resize) draws at once. After Go's internal/ui/framehold.go
//! (backgroundGap :13, held :16, holdBackground :27, heldFrame :49).
//!
//! Rust-only: the gap runs from the last frame background work drew, not
//! from the last frame. In Go a key's frame started the gap too, so what
//! a key asked for (a conversation's end, a diff, a sheet's reading)
//! stayed a placeholder until a quarter second after the key even when
//! it was read in a few milliseconds. Background work alone still draws
//! at most four frames a second.

use std::time::{Duration, Instant};

/// The least time between frames that work from the background draws.
pub const BACKGROUND_GAP: Duration = Duration::from_millis(250);

/// When background work last drew, and the frame held back for it
/// since.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub drew_at: Instant,
    /// The frame owed: when it's due.
    pub due: Option<Instant>,
    /// What a key asked for is in: the next frame is drawn at once.
    pub answered: bool,
}

impl Default for Hold {
    fn default() -> Hold {
        Hold {
            // Long enough ago that the first background work draws.
            drew_at: Instant::now() - BACKGROUND_GAP,
            due: None,
            answered: false,
        }
    }
}

impl Hold {
    /// Background work wants a frame at now: true to draw it now; false
    /// when it's held, a frame then owed for when the gap is up
    /// (holdBackground).
    pub fn background(&mut self, now: Instant) -> bool {
        let at = self.drew_at + BACKGROUND_GAP;
        if now >= at || std::mem::take(&mut self.answered) {
            self.drew_at = now;
            return true;
        }
        self.due.get_or_insert(at);
        false
    }

    /// What a key asked for (a placeholder's answer) is in: drawn at once,
    /// whatever background work drew just before.
    pub fn answer(&mut self) {
        self.answered = true;
    }

    /// A frame was drawn: anything owed is in it, and that was
    /// background work drawing.
    pub fn drew(&mut self, now: Instant) {
        if self.due.take().is_some() {
            self.drew_at = now;
        }
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

    /// An answer to what a key asked for isn't held behind background
    /// work that drew just before it.
    #[test]
    fn answers_dont_wait() {
        let mut h = Hold::default();
        let t0 = Instant::now();
        assert!(h.background(t0));
        h.drew(t0);
        h.answer();
        assert!(h.background(t0 + Duration::from_millis(5)), "drawn at once");
        h.drew(t0 + Duration::from_millis(5));
        assert!(!h.background(t0 + Duration::from_millis(10)), "once");
    }

    /// A key's frame doesn't hold back what the key asked for.
    #[test]
    fn keys_dont_hold() {
        let mut h = Hold::default();
        let t0 = Instant::now();
        h.drew(t0); // a key's frame
        assert!(
            h.background(t0 + Duration::from_millis(10)),
            "drawn at once"
        );
        h.drew(t0 + Duration::from_millis(10));
        assert!(
            !h.background(t0 + Duration::from_millis(20)),
            "the next waits"
        );
        let due = t0 + Duration::from_millis(10) + BACKGROUND_GAP;
        assert_eq!(h.due, Some(due));
        // A key's frame drew what was owed: the gap starts again there.
        h.drew(t0 + Duration::from_millis(100));
        assert_eq!(h.due, None);
        assert!(!h.background(t0 + Duration::from_millis(200)));
        assert_eq!(
            h.due,
            Some(t0 + Duration::from_millis(100) + BACKGROUND_GAP)
        );
    }
}
