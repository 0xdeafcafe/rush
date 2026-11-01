//! Push-to-talk's key and the engines that hear you, apart from the
//! window: no Go original, new in the Rust port (the apple voice plugin).
//!
//! The key is ctrl+space, both ways. In a terminal that says when a key
//! is let go, hold it and speak, and letting go after [`HOLD`] stops; let
//! go sooner and it keeps listening, hands-free, until the key is pressed
//! again. In one that can't, each press starts or stops it. esc drops it
//! either way. [`Talk`] is that, with the time given, so tests need no
//! clock.
//!
//! An [`Engine`] is what hears: rush's own helper app ([`AppleVoice`],
//! rush_voice) today. Another engine plugs in by being one; the window
//! draws what any of them says alike (dictation.rs).

use rush_core::time::Ms;

/// How long ctrl+space must be held for letting go to stop: a shorter
/// tap keeps listening hands-free.
pub const HOLD: Ms = 350;

/// Where push-to-talk is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Talk {
    /// Not listening.
    #[default]
    Idle,
    /// Listening while the key is held, since at.
    Held { at: Ms },
    /// Listening hands-free, until the key is pressed again.
    Free,
    /// Told to stop: the last words are being finished.
    Finishing,
}

/// What a key asks of the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    Start,
    Stop,
    Cancel,
}

impl Talk {
    /// The key pressed at now; repeat when it's the terminal saying a
    /// held key again, releases when the terminal says when it's let go.
    pub fn press(&mut self, now: Ms, repeat: bool, releases: bool) -> Do {
        match *self {
            // Held from before (through the end of the last one): not a
            // press.
            _ if repeat => Do::Nothing,
            Talk::Idle => {
                *self = if releases {
                    Talk::Held { at: now }
                } else {
                    Talk::Free
                };
                Do::Start
            }
            // A press while held means its letting go was missed: stop
            // as it would have.
            Talk::Held { .. } | Talk::Free => {
                *self = Talk::Finishing;
                Do::Stop
            }
            Talk::Finishing => Do::Nothing,
        }
    }

    /// The key let go at now.
    pub fn release(&mut self, now: Ms) -> Do {
        let Talk::Held { at } = *self else {
            return Do::Nothing;
        };
        if now - at >= HOLD {
            *self = Talk::Finishing;
            Do::Stop
        } else {
            *self = Talk::Free;
            Do::Nothing
        }
    }

    /// esc: drop what it heard.
    pub fn esc(&mut self) -> Do {
        if *self == Talk::Idle {
            return Do::Nothing;
        }
        *self = Talk::Idle;
        Do::Cancel
    }

    /// The engine said it's over (done, failed or dropped).
    pub fn ended(&mut self) {
        *self = Talk::Idle;
    }

    /// Whether it's listening or finishing.
    pub fn on(&self) -> bool {
        *self != Talk::Idle
    }

    /// What's going on, for the box's bottom edge; key is the one that
    /// stops it.
    pub fn hint(&self, key: &str) -> String {
        match self {
            Talk::Idle => String::new(),
            Talk::Held { .. } => "listening · let go to stop".into(),
            Talk::Free => format!("listening · {key} to stop"),
            Talk::Finishing => "finishing".into(),
        }
    }
}

/// What an engine says of a dictation, each time it changes: the shape
/// yap says on its socket and rush_voice's helper says on its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// listening, finishing, done (the words are in settled and nothing
    /// more comes), failed (why is in error) or hidden (dropped).
    pub phase: String,
    /// Words it's sure of.
    pub settled: String,
    /// Words it may still change.
    pub guessing: String,
    /// How loud, 0 to 100.
    pub level: i64,
    /// Why it failed, in words for you.
    pub error: String,
}

impl Heard {
    /// Whether this is the last change of a dictation.
    pub fn last(&self) -> bool {
        matches!(self.phase.as_str(), "done" | "failed" | "hidden")
    }
}

/// A microphone an engine can hear.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mic {
    /// What the setting keeps; empty is the Mac's input.
    pub id: String,
    pub name: String,
}

/// Where each change an engine hears goes: on a thread of the engine's.
pub type Out = Box<dyn FnMut(Heard) + Send>;

/// A dictation engine rush starts and stops itself; asked for its
/// microphones off the UI thread.
pub trait Engine: Send + Sync {
    /// Whether it can work here at all.
    fn available(&self) -> bool;
    /// The microphones it can hear, asked off the UI thread.
    fn devices(&self) -> Vec<Mic>;
    /// Begins hearing the microphone with this id (empty: the Mac's
    /// input), handing each change to out until its last.
    fn start(&self, device: &str, out: Out) -> Result<Box<dyn Listening>, String>;
}

/// A dictation an engine has under way. Dropping it drops the dictation.
pub trait Listening {
    /// Stop listening and finish: the last change is done.
    fn stop(&self);
    /// Stop listening and drop it: the last change is hidden.
    fn cancel(&self);
}

/// rush's own push-to-talk: its helper app hearing the microphone with
/// Apple's on-device speech model (rush_voice).
pub struct AppleVoice;

impl Engine for AppleVoice {
    fn available(&self) -> bool {
        rush_voice::available()
    }

    fn devices(&self) -> Vec<Mic> {
        rush_voice::devices()
            .into_iter()
            .map(|d| Mic {
                id: d.id,
                name: d.name,
            })
            .collect()
    }

    fn start(&self, device: &str, mut out: Out) -> Result<Box<dyn Listening>, String> {
        let d = rush_voice::start(device, move |l: rush_voice::Live| {
            out(Heard {
                phase: l.phase,
                settled: l.settled,
                guessing: l.guessing,
                level: l.level,
                error: l.error,
            })
        })?;
        Ok(Box::new(d))
    }
}

impl Listening for rush_voice::Dictation {
    fn stop(&self) {
        rush_voice::Dictation::stop(self)
    }

    fn cancel(&self) {
        rush_voice::Dictation::cancel(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One step of a run: the key pressed (repeat or not), let go, or esc,
    /// at a time.
    #[derive(Clone, Copy, Debug)]
    enum Step {
        Press(Ms),
        Repeat(Ms),
        Release(Ms),
        Esc,
    }
    use Step::*;

    /// A run: its name, whether the terminal says releases, its steps,
    /// what each asks, and where it ends.
    type Case = (&'static str, bool, &'static [Step], &'static [Do], Talk);

    #[test]
    fn hold_tap_and_toggle() {
        // (name, terminal says releases, steps, what each asks, where it ends)
        let cases: &[Case] = &[
            (
                "held past the hold stops on letting go",
                true,
                &[Press(0), Repeat(30), Repeat(60), Release(900)],
                &[Do::Start, Do::Nothing, Do::Nothing, Do::Stop],
                Talk::Finishing,
            ),
            (
                "held exactly the hold stops",
                true,
                &[Press(1_000), Release(1_350)],
                &[Do::Start, Do::Stop],
                Talk::Finishing,
            ),
            (
                "a tap keeps listening hands-free",
                true,
                &[Press(0), Release(120)],
                &[Do::Start, Do::Nothing],
                Talk::Free,
            ),
            (
                "hands-free stops on the next press, not its letting go",
                true,
                &[Press(0), Release(120), Press(5_000), Release(5_100)],
                &[Do::Start, Do::Nothing, Do::Stop, Do::Nothing],
                Talk::Finishing,
            ),
            (
                "a press while held stops: its letting go was missed",
                true,
                &[Press(0), Press(2_000)],
                &[Do::Start, Do::Stop],
                Talk::Finishing,
            ),
            (
                "without releases each press toggles",
                false,
                &[Press(0), Press(3_000)],
                &[Do::Start, Do::Stop],
                Talk::Finishing,
            ),
            (
                "without releases a quick second press still stops",
                false,
                &[Press(0), Press(100)],
                &[Do::Start, Do::Stop],
                Talk::Finishing,
            ),
            (
                "pressed again while finishing does nothing",
                true,
                &[Press(0), Release(500), Press(600)],
                &[Do::Start, Do::Stop, Do::Nothing],
                Talk::Finishing,
            ),
            (
                "esc while held drops it",
                true,
                &[Press(0), Esc, Release(800)],
                &[Do::Start, Do::Cancel, Do::Nothing],
                Talk::Idle,
            ),
            (
                "esc hands-free drops it",
                false,
                &[Press(0), Esc],
                &[Do::Start, Do::Cancel],
                Talk::Idle,
            ),
            (
                "esc with nothing going does nothing",
                true,
                &[Esc],
                &[Do::Nothing],
                Talk::Idle,
            ),
            (
                "a repeat with nothing going doesn't start one",
                true,
                &[Repeat(0)],
                &[Do::Nothing],
                Talk::Idle,
            ),
            (
                "letting go with nothing going does nothing",
                true,
                &[Release(0)],
                &[Do::Nothing],
                Talk::Idle,
            ),
        ];
        for (name, releases, steps, want, end) in cases {
            let mut t = Talk::default();
            let got: Vec<Do> = steps
                .iter()
                .map(|s| match *s {
                    Press(at) => t.press(at, false, *releases),
                    Repeat(at) => t.press(at, true, *releases),
                    Release(at) => t.release(at),
                    Esc => t.esc(),
                })
                .collect();
            assert_eq!(&got, want, "{name}");
            assert_eq!(t, *end, "{name}");
        }
    }

    #[test]
    fn hints_say_how_to_stop() {
        let cases = [
            (Talk::Idle, ""),
            (Talk::Held { at: 0 }, "listening · let go to stop"),
            (Talk::Free, "listening · ctrl+space to stop"),
            (Talk::Finishing, "finishing"),
        ];
        for (t, want) in cases {
            assert_eq!(t.hint("ctrl+space"), want, "{t:?}");
        }
        let mut t = Talk::Free;
        t.ended();
        assert!(!t.on());
    }
}
