//! What a running step has printed so far, for the conversation to show
//! while it runs, whichever agent runs it. A harness that streams a
//! call's output (Codex's command output, an ACP agent's tool content)
//! sends it as CallOutput events, kept here; for one whose shell writes
//! its output to a file (Claude Code's), rush reads the file itself and
//! hands it in through the render options (Options::live). Either way it
//! is gone once the step ends: its real result is drawn then. New in the
//! Rust port.

use super::{Session, Status, StepId};

/// The most of a running step's output kept: its last 256 KiB.
pub const LIVE_KEPT: usize = 256 << 10;

/// A running step's output so far: its last LIVE_KEPT bytes, colours and
/// all, and how many lines were let go from the front.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveOut {
    pub out: String,
    pub dropped: usize,
}

impl LiveOut {
    /// Adds what it printed next, letting go of its oldest lines past
    /// LIVE_KEPT.
    pub fn push(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.out.push_str(s);
        if self.out.len() > LIVE_KEPT {
            let mut cut = self.out.len() - LIVE_KEPT;
            while !self.out.is_char_boundary(cut) {
                cut += 1;
            }
            if let Some(nl) = self.out[cut..].find('\n') {
                cut += nl + 1;
            }
            self.dropped += self.out[..cut].matches('\n').count();
            self.out.drain(..cut);
        }
    }

    /// Its output a line each, as a terminal leaves it: a line redrawn
    /// with carriage returns is its last state; colours kept.
    pub fn lines(&self) -> Vec<&str> {
        let out = self.out.strip_suffix('\n').unwrap_or(&self.out);
        if out.is_empty() {
            return Vec::new();
        }
        out.split('\n')
            .map(|l| {
                let l = l.strip_suffix('\r').unwrap_or(l);
                match l.rfind('\r') {
                    Some(i) => &l[i + 1..],
                    None => l,
                }
            })
            .collect()
    }

    /// How many lines it printed, those let go included.
    pub fn line_count(&self) -> usize {
        self.dropped + self.lines().len()
    }
}

impl Session {
    /// Takes what running call `call` printed, as its harness streams it:
    /// all of it so far (whole), or what follows what came before.
    /// Nothing for a call that isn't running.
    pub fn call_output(&mut self, call: &str, text: &str, whole: bool) {
        let Some(sid) = self.by_id.get(&self.steps, call) else {
            return;
        };
        if self.step(sid).status != Status::Running {
            return;
        }
        let l = self.live_out.entry(sid).or_default();
        if whole {
            *l = LiveOut::default();
        }
        l.push(text);
        self.touch_step(sid);
    }

    /// What step id has printed so far, as its harness streamed it, while
    /// it runs.
    pub fn live_output(&self, id: StepId) -> Option<&LiveOut> {
        self.live_out
            .get(&id)
            .filter(|l| !l.out.is_empty() && self.step(id).status == Status::Running)
    }

    /// Lets go of the output kept for steps no longer running.
    pub(super) fn drop_live_out(&mut self) {
        if self.live_out.is_empty() {
            return;
        }
        let steps = &self.steps;
        self.live_out.retain(|id, _| {
            steps
                .get(*id as usize)
                .is_some_and(|s| s.status == Status::Running)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Past LIVE_KEPT, the oldest whole lines go, and are counted.
    #[test]
    fn keeps_the_last_of_it() {
        let mut l = LiveOut::default();
        let line = format!("{}\n", "x".repeat(99));
        for _ in 0..(LIVE_KEPT / 100 + 50) {
            l.push(&line);
        }
        assert!(l.out.len() <= LIVE_KEPT, "{}", l.out.len());
        assert!(l.dropped >= 50);
        assert_eq!(l.line_count(), LIVE_KEPT / 100 + 50);
        assert!(l.out.starts_with('x'));
    }

    /// A progress line redrawn with \r is its last state.
    #[test]
    fn a_redrawn_line_is_its_last_state() {
        let mut l = LiveOut::default();
        l.push("a\n10%\r50%\r90%\r\n");
        l.push("b");
        assert_eq!(l.lines(), ["a", "90%", "b"]);
    }
}
