//! Claude Code's daemon keeps its background sessions' screens: rush opens
//! them full screen, mirrors them in the preview, and keeps the daemon's
//! pins. Port of internal/adapters/claude/screen.go and claude/jobs.go:
//! 211-247 (LoadPins, WritePins).

use std::io::{Read, Write};
use std::process::Command;

use rush_core::adapter::{
    self, Elsewhere, Gone, Joiner, Mirror, Pinner, Profile, SessionsViewer, Terminal,
};

use crate::account::Account;
use crate::actions::claude_cmd;
use crate::adapter::{Claude, account};
use crate::daemon::{self, Client};

fn client(p: &Profile) -> Client {
    Client::new(account(p))
}

/// The daemon's refusal, in its words, that is rush's error too: its
/// source is Gone or Elsewhere.
#[derive(Debug)]
pub struct Refusal {
    pub err: adapter::Error,
    is: Is,
    /// Says what it is before the daemon's words, as Go's "%w: %w" does.
    say_is: bool,
}

#[derive(Debug)]
enum Is {
    Gone(Gone),
    Elsewhere(Elsewhere),
}

impl Refusal {
    pub(crate) fn gone(err: adapter::Error, say_is: bool) -> Refusal {
        Refusal {
            err,
            is: Is::Gone(Gone),
            say_is,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.say_is {
            match &self.is {
                Is::Gone(g) => write!(f, "{g}: ")?,
                Is::Elsewhere(e) => write!(f, "{e}: ")?,
            }
        }
        write!(f, "{}", self.err)
    }
}

impl std::error::Error for Refusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match &self.is {
            Is::Gone(g) => g,
            Is::Elsewhere(e) => e,
        })
    }
}

/// A session's screen in rush's terminal, its refusals as rush's errors.
struct Joined(daemon::Session);

impl Terminal for Joined {
    /// Says why the daemon refused, as rush's errors too.
    fn run(&mut self) -> adapter::Result<()> {
        let err = match self.0.run() {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        let is = if daemon::is_refusal(&*err, "EKICKED") {
            Is::Elsewhere(Elsewhere)
        } else if daemon::is_refusal(&*err, "ENOJOB") {
            Is::Gone(Gone)
        } else {
            return Err(err);
        };
        Err(Box::new(Refusal {
            err,
            is,
            say_is: false,
        }))
    }
    // It runs on rush's own terminal, as Go's does.
    fn set_stdin(&mut self, _: Box<dyn Read + Send>) {}
    fn set_stdout(&mut self, _: Box<dyn Write + Send>) {}
    fn set_stderr(&mut self, _: Box<dyn Write + Send>) {}
}

/// A mirror's side of its attachment: dropping it detaches, ending the
/// relay's read too (Go closes the one conn both use, live.go:185).
struct Viewer(std::os::unix::net::UnixStream);

impl Write for Viewer {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

impl Joiner for Claude {
    /// Attaches to session short's screen in rush's terminal.
    fn join(&self, p: &Profile, short: &str) -> Box<dyn Terminal> {
        Box::new(Joined(daemon::Session {
            client: client(p),
            short: short.into(),
        }))
    }

    fn service_up(&self, p: &Profile) -> bool {
        client(p).running()
    }

    fn mirror(
        &self,
        p: &Profile,
        short: &str,
        viewer: &str,
        cols: u16,
        rows: u16,
    ) -> adapter::Result<Mirror> {
        let a = client(p).attach(short, viewer, cols, rows)?;
        let mut r = a.r;
        Ok(Mirror {
            modes: a.info.dec_modes,
            conn: Box::new(Viewer(a.conn)),
            relay: Box::new(move |w| daemon::relay(&mut r, w)),
        })
    }

    fn resize(
        &self,
        p: &Profile,
        short: &str,
        viewer: &str,
        cols: u16,
        rows: u16,
    ) -> adapter::Result<()> {
        client(p).resize(short, viewer, cols, rows)
    }
}

impl Pinner for Claude {
    /// Pins session short in the daemon's list, or unpins it.
    fn toggle_pin(&self, p: &Profile, short: &str) -> adapter::Result<()> {
        let acct = account(p);
        let pins = load_pins(&acct)?;
        let mut out: Vec<String> = pins.iter().filter(|id| *id != short).cloned().collect();
        if out.len() == pins.len() {
            out.push(short.into());
        }
        write_pins(&acct, &out)
    }
}

impl SessionsViewer for Claude {
    /// claude agents, Claude Code's own list of its sessions.
    fn sessions_view(&self, p: &Profile) -> Command {
        claude_cmd(&account(p), "", &["agents"])
    }
}

fn pins_path(a: &Account) -> std::path::PathBuf {
    a.jobs_dir().join("pins.json")
}

/// The native view's pin list. One it could not read is an error, so a
/// write never replaces a list that is mid-write or in an unknown format.
pub fn load_pins(a: &Account) -> adapter::Result<Vec<String>> {
    let b = match std::fs::read(pins_path(a)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let ids: Option<Vec<String>> =
        serde_json::from_slice(&b).map_err(|e| format!("couldn't read the pin list: {e}"))?;
    Ok(ids.unwrap_or_default())
}

/// Writes the pin list, indented two spaces as Go's jsonx.MarshalIndent does, through
/// a file beside it.
pub fn write_pins(a: &Account, ids: &[String]) -> adapter::Result<()> {
    let b = serde_json::to_vec_pretty(ids)?;
    let path = pins_path(a);
    let mut tmp = path.clone().into_os_string();
    tmp.push(".rush.tmp");
    std::fs::write(&tmp, b)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::profile;

    #[test]
    fn pins_toggle() {
        let d = tempfile::tempdir().unwrap();
        let acct = Account::at("x", d.path());
        std::fs::create_dir_all(acct.jobs_dir()).unwrap();
        let p = profile(&acct);
        let c = Claude::new();
        c.toggle_pin(&p, "a1").unwrap();
        c.toggle_pin(&p, "b2").unwrap();
        assert_eq!(load_pins(&acct).unwrap(), ["a1", "b2"]);
        assert_eq!(
            std::fs::read_to_string(pins_path(&acct)).unwrap(),
            "[\n  \"a1\",\n  \"b2\"\n]"
        );
        c.toggle_pin(&p, "a1").unwrap();
        assert_eq!(load_pins(&acct).unwrap(), ["b2"]);
        std::fs::write(pins_path(&acct), "{oops").unwrap();
        assert!(
            c.toggle_pin(&p, "c3").is_err(),
            "a list it can't read stays"
        );
        assert_eq!(std::fs::read_to_string(pins_path(&acct)).unwrap(), "{oops");
    }

    #[test]
    fn refusals_are_rushs_errors() {
        let e = Refusal::gone(
            Box::new(daemon::Error {
                code: "ENOJOB".into(),
                message: "gone".into(),
            }),
            true,
        );
        assert_eq!(
            e.to_string(),
            "the agent has let the session go: ENOJOB: gone"
        );
        assert!(adapter::caused_by::<Gone>(&e));
        assert!(!adapter::caused_by::<Elsewhere>(&e));
    }
}
