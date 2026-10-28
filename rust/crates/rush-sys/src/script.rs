//! Runs an agent's longer shell calls as a file rush can follow: each
//! command's line and when it began go to a trace as it runs, and a line
//! with a breakpoint stops the script there until it's let go.
//!
//! ```text
//! scripts/<id>.sh      the call's command, as written
//! scripts/<id>.trace   "pid N" once, then "LINE TIME" before each command
//! scripts/<id>.breaks  the lines to stop at, one a line
//! ```
//!
//! The call is the agent's tool call ID, or, where the harness gives none,
//! the command's hash.
//!
//! Ported from internal/script/script.go:1-221. Go keeps the folder at
//! `state.Dir()/scripts`; here it's given: [`Scripts::new`] takes rush's
//! state folder.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::shellwords;

/// How many commands a call runs as it is; past it, as a script.
pub const MOST: usize = 3;

/// The name a call's files go by: its ID, else its command's hash.
pub fn key(id: &str, command: &str) -> String {
    if !id.is_empty() && !id.contains(['/', '\\', '.']) {
        return id.to_string();
    }
    let h = Sha256::digest(command.as_bytes());
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether command is more than one line and runs more commands than
/// [`MOST`]: a one-liner has no lines to stop at. One with a here-document
/// isn't run as a script on macOS: the bash that can trace one hangs on a
/// long here-document there (see the gate's Wrap).
pub fn long(command: &str) -> bool {
    if cfg!(target_os = "macos") && shellwords::here_doc(command) {
        return false;
    }
    command.trim().contains('\n') && shellwords::commands(command).map_or(0, |c| c.len()) > MOST
}

/// What runs a script: before each command, its line and the time go to
/// the trace, and at a line with a breakpoint it stops itself once, so rush
/// can let it go on. Sourced, the script's own line numbers are the ones
/// $LINENO counts. A write per command; a loop of thousands writes
/// thousands of lines.
const RUNNER: &str = r#"__rush_s=$1; __rush_t=${1%.sh}.trace; __rush_b=${1%.sh}.breaks; __rush_at=
printf 'pid %s %s\n' "$$" "$(date +%s)" > "$__rush_t"
__rush_step() {
  [ "${BASH_SOURCE[1]}" = "$__rush_s" ] || return 0
  printf '%s %s\n' "$1" "${EPOCHREALTIME:-$SECONDS}" >> "$__rush_t"
  [ -s "$__rush_b" ] || return 0
  [ "$1" = "$__rush_at" ] && return 0
  __rush_at=$1
  local l
  while IFS= read -r l; do
    if [ "$l" = "$1" ]; then printf 'break %s\n' "$1" >> "$__rush_t"; kill -STOP $$; return 0; fi
  done < "$__rush_b"
}
set -T
trap '__rush_step "$LINENO"' DEBUG
shift
. "$__rush_s"
"#;

/// A bash new enough to trace a script by its lines (4 or later), None for
/// none: macOS's own is 3.2.
pub fn bash() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = vec![
        "/opt/homebrew/bin/bash".into(),
        "/usr/local/bin/bash".into(),
    ];
    if !cfg!(target_os = "macos") {
        cands.push("/bin/bash".into());
        cands.push("/usr/bin/bash".into());
    }
    if let Some(p) = look_path("bash")
        && p != Path::new("/bin/bash")
    {
        cands.insert(0, p);
    }
    cands
        .into_iter()
        .find(|p| p.metadata().is_ok_and(|m| !m.is_dir()))
}

/// The first executable named name on PATH, as Go's exec.LookPath.
fn look_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| {
        let d = if d.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            d
        };
        let p = d.join(name);
        let m = p.metadata().ok()?;
        (!m.is_dir() && m.permissions().mode() & 0o111 != 0).then_some(p)
    })
}

/// A command's line beginning at a time.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub line: i32,
    pub at: SystemTime,
}

/// A script as its trace has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub pid: i32,
    pub start: Option<SystemTime>,
    pub hits: Vec<Hit>,
    pub breaks: Vec<i32>,
    /// The line it stopped at for a breakpoint, 0 when it isn't.
    pub held_at: i32,
    pub source: String,
    /// When the trace last grew.
    pub changed: Option<SystemTime>,
}

/// The scripts folder: where scripts and their traces are kept.
#[derive(Clone, Debug)]
pub struct Scripts {
    dir: PathBuf,
}

impl Scripts {
    /// The scripts kept under rush's state folder.
    pub fn new(state_dir: impl AsRef<Path>) -> Scripts {
        Scripts {
            dir: state_dir.as_ref().join("scripts"),
        }
    }

    /// Where scripts and their traces are kept.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{key}.{ext}"))
    }

    /// Writes command to its script and is the command that runs it, from
    /// bash, the runner given inline; None when there's no bash to.
    pub fn wrap(&self, key: &str, command: &str) -> std::io::Result<Option<String>> {
        let Some(sh) = bash() else {
            return Ok(None);
        };
        std::fs::create_dir_all(&self.dir)?;
        let p = self.file(key, "sh");
        std::fs::write(&p, format!("{command}\n"))?;
        let _ = std::fs::remove_file(self.file(key, "trace"));
        let _ = std::fs::remove_file(self.file(key, "breaks")); // a hash key's last run left its own
        Ok(Some(format!(
            "{} --noprofile --norc -c {} rush-script {}",
            quote(&sh.to_string_lossy()),
            quote(RUNNER),
            quote(&p.to_string_lossy())
        )))
    }

    /// The run of the script key, None when there's none. It reads files:
    /// keep it off the UI thread.
    pub fn read(&self, key: &str) -> Option<Run> {
        let src = std::fs::read(self.file(key, "sh")).ok()?;
        let src = String::from_utf8_lossy(&src);
        let mut r = Run {
            pid: 0,
            start: None,
            hits: Vec::new(),
            breaks: self.breaks(key),
            held_at: 0,
            source: src.strip_suffix('\n').unwrap_or(&src).to_string(),
            changed: None,
        };
        let Ok(trace) = std::fs::File::open(self.file(key, "trace")) else {
            return Some(r); // written, not begun
        };
        r.changed = trace.metadata().and_then(|m| m.modified()).ok();
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut &trace, &mut text);
        let mut base = 0f64; // what $SECONDS counts from, where the shell has no EPOCHREALTIME
        for line in text.lines() {
            let w: Vec<&str> = line.split_ascii_whitespace().collect();
            if w.len() < 2 {
                continue;
            }
            match w[0] {
                "pid" => {
                    r.pid = w[1].parse().unwrap_or(0);
                    if w.len() > 2 {
                        base = w[2].parse().unwrap_or(0.0);
                        r.start = Some(when(base));
                    }
                }
                "break" => r.held_at = w[1].parse().unwrap_or(0),
                _ => {
                    let (Ok(n), Ok(mut t)) = (w[0].parse::<i32>(), w[1].parse::<f64>()) else {
                        continue;
                    };
                    if !w[1].contains('.') {
                        t += base;
                    }
                    if r.held_at != 0 && n != r.held_at {
                        r.held_at = 0; // let go: it's moved on
                    }
                    r.hits.push(Hit {
                        line: n,
                        at: when(t),
                    });
                }
            }
        }
        Some(r)
    }

    /// The lines script key stops at.
    pub fn breaks(&self, key: &str) -> Vec<i32> {
        let b = std::fs::read_to_string(self.file(key, "breaks")).unwrap_or_default();
        b.split_ascii_whitespace()
            .filter_map(|l| l.parse().ok())
            .collect()
    }

    /// Sets a breakpoint at line of script key, or takes it away, and says
    /// whether it's set now.
    pub fn toggle(&self, key: &str, line: i32) -> std::io::Result<bool> {
        let mut bs = self.breaks(key);
        let on = !bs.contains(&line);
        if on {
            bs.push(line);
        } else {
            bs.retain(|&n| n != line);
        }
        bs.sort_unstable();
        let s: String = bs.iter().map(|n| format!("{n}\n")).collect();
        std::fs::write(self.file(key, "breaks"), s)?;
        Ok(on)
    }
}

fn when(sec: f64) -> SystemTime {
    let ns = (sec * 1e9) as i64;
    if ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(ns.unsigned_abs())
    }
}

/// s in single quotes for sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fswait::{self, File, Stop};
    use std::io::Read;
    use std::process::Command;

    #[test]
    fn long_calls() {
        assert!(!long("go build ./... && go test ./..."));
        assert!(
            long("cd x && go build\ngo vet && go test"),
            "more than three commands over more than one line is long"
        );
        assert!(
            !long("cd x && go build && go vet && go test"),
            "a one-liner has no lines to stop at"
        );
        assert_eq!(
            long("cat <<EOF\nx\nEOF\na; b; c; d"),
            !cfg!(target_os = "macos")
        );
    }

    #[test]
    fn keys() {
        assert_eq!(key("toolu_01", "ls"), "toolu_01");
        for id in ["", "a/b", "a.b", "a\\b"] {
            assert_eq!(key(id, "ls"), key("", "ls"), "{id:?} can't name a file");
        }
        // The first 8 bytes of sha256("ls"), in hex, as Go's.
        assert_eq!(key("", "ls"), "c7b68ac37f364473");
    }

    // A script stops at its breakpoint, says where, and finishes once let
    // go; its trace has each command's line by the script's own count.
    #[test]
    fn breakpoint() {
        if bash().is_none() {
            eprintln!("no bash 4 or later");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let s = Scripts::new(home.path());
        let cmd = s
            .wrap("k1", "echo one\nf() { echo in f; }\nf\necho two && echo three\nfor i in 1 2; do echo loop $i; done")
            .unwrap()
            .unwrap();
        assert!(s.toggle("k1", 4).unwrap());
        // The trace is there before it runs, so its growth can be waited on.
        let trace = s.dir().join("k1.trace");
        std::fs::write(&trace, "").unwrap();
        let (mut r_out, w_out) = std::io::pipe().unwrap();
        let mut c = Command::new("sh")
            .args(["-c", &cmd])
            .stdout(w_out.try_clone().unwrap())
            .stderr(w_out)
            .spawn()
            .unwrap();
        let stop = Stop::new().unwrap();
        let held = loop {
            let size = std::fs::metadata(&trace).unwrap().len();
            let r = s.read("k1").unwrap();
            if r.held_at != 0 {
                break r;
            }
            fswait::grown(
                &stop,
                &[File {
                    path: trace.clone(),
                    size,
                }],
            );
        };
        assert_eq!(held.held_at, 4);
        assert_ne!(held.pid, 0);
        // SAFETY: the script's own bash, stopped.
        unsafe { libc::kill(held.pid, libc::SIGCONT) };
        assert!(c.wait().unwrap().success());
        drop(c);
        let mut out = String::new();
        r_out.read_to_string(&mut out).unwrap();
        let r = s.read("k1").unwrap();
        let lines: Vec<i32> = r.hits.iter().map(|h| h.line).collect();
        assert_eq!(r.held_at, 0);
        assert_eq!(out, "one\nin f\ntwo\nthree\nloop 1\nloop 2\n");
        assert_eq!((lines[0], *lines.last().unwrap()), (1, 5), "{lines:?}");
        assert!(r.start.is_some() && r.changed.is_some());
        assert_eq!(r.source.lines().count(), 5);
        assert!(
            !s.toggle("k1", 4).unwrap(),
            "toggled again, the breakpoint goes"
        );
        assert!(s.breaks("k1").is_empty());
    }

    #[test]
    fn read_without_bash_time() {
        // A shell without EPOCHREALTIME writes $SECONDS: counted from the
        // pid line's start.
        let home = tempfile::tempdir().unwrap();
        let s = Scripts::new(home.path());
        std::fs::create_dir_all(s.dir()).unwrap();
        std::fs::write(s.dir().join("k.sh"), "a\nb\n").unwrap();
        assert_eq!(s.read("k").unwrap().changed, None, "written, not begun");
        std::fs::write(
            s.dir().join("k.trace"),
            "pid 42 1000\n1 0\nbreak 2\n2 3\nx\n3 1000.5\n",
        )
        .unwrap();
        let r = s.read("k").unwrap();
        assert_eq!(r.pid, 42);
        assert_eq!(r.start, Some(when(1000.0)));
        assert_eq!(
            r.hits,
            [
                Hit {
                    line: 1,
                    at: when(1000.0)
                },
                Hit {
                    line: 2,
                    at: when(1003.0)
                },
                Hit {
                    line: 3,
                    at: when(1000.5)
                },
            ]
        );
        assert_eq!(r.held_at, 0, "let go: it's moved on");
        assert_eq!(r.source, "a\nb");
        assert_eq!(s.read("none"), None);
    }
}
