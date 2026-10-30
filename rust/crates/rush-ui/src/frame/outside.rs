//! Running another program with the terminal handed to it, and $EDITOR on
//! a file. After Go's tea.ExecProcess (bubbletea v2.0.9 exec.go:50
//! ExecProcess, :102 exec: what the program says when it fails) and
//! internal/ui/dialog.go:64-82 (editFile, editorCmd). The loop does the
//! handing over (rush_draw::App::outside): asked for while a key or a
//! message is handled, the program runs once it has been, and what's done
//! after runs on the UI thread, the screen drawn whole. Its callers: $EDITOR
//! (memedit), a sign-in, and an agent's own screens (cmdbar/attach.rs:
//! attach, join, the native view; session_cards/claudescreen.rs: Claude
//! Code's /plugin and /mcp).

use std::io;
use std::process::{Command, ExitStatus};

use super::Out;
use crate::model::Model;
use crate::worker::Note;

/// How the program went: Err says why it failed, in Go's words ("exit
/// status 1").
pub type Ran = Result<(), String>;

/// What's done on the UI thread once the program has ended.
pub type Done = Box<dyn FnOnce(&mut Model, Ran) -> Out + Send>;

/// What waits to run with the terminal (one at a time, in order), and the
/// editor a test runs instead of $VISUAL / $EDITOR.
#[derive(Default)]
pub struct Outside {
    queued: Vec<Box<dyn FnOnce() -> Note + Send>>,
    editor: Option<String>,
}

impl Model {
    /// Hands the terminal to cmd once what asked has been handled, then
    /// runs done with how it went (tea.ExecProcess). Its stdin, stdout and
    /// stderr are the terminal. With no loop (tests, `rush --render`) it
    /// waits for [`Model::run_outside`].
    pub fn exec(&mut self, mut cmd: Command, done: Done) {
        self.exec_with(move || ran(cmd.status()), done);
    }

    /// Hands the terminal to run, which isn't a program of its own (an
    /// agent's session screen joined in this process: Go's tea.Exec with
    /// an ExecCommand), then runs done with what it said, on the UI thread.
    pub fn exec_with<T: Send + 'static>(
        &mut self,
        run: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Model, T) -> Out + Send + 'static,
    ) {
        self.kept::<Outside>().queued.push(Box::new(move || {
            let r = run();
            Note::Apply(Box::new(move |m: &mut Model| done(m, r)))
        }));
    }

    /// The next program asked for, for the loop to run with the terminal
    /// handed over; its message runs done.
    pub(crate) fn take_outside(&mut self) -> Option<rush_draw::Outside<Note>> {
        let q = &mut self.kept::<Outside>().queued;
        if q.is_empty() {
            return None;
        }
        let run = q.remove(0);
        Some(Box::new(run))
    }

    /// Runs what waits to run outside, here and now, and what's done after
    /// each: for tests, which have no terminal to hand over. How many ran.
    pub fn run_outside(&mut self) -> usize {
        let mut n = 0;
        while let Some(run) = self.take_outside() {
            if let Note::Apply(f) = run() {
                f(self);
            }
            n += 1;
        }
        n
    }

    /// Runs `ed` instead of $VISUAL or $EDITOR (tests).
    pub fn set_editor_program(&mut self, ed: &str) {
        self.kept::<Outside>().editor = Some(ed.to_string());
    }
}

/// How a program's wait ended, as Go's exec.Cmd.Run says it.
fn ran(r: io::Result<ExitStatus>) -> Ran {
    use std::os::unix::process::ExitStatusExt;
    let st = r.map_err(|e| format!("exec: {e}"))?;
    if st.success() {
        return Ok(());
    }
    if let Some(code) = st.code() {
        return Err(format!("exit status {code}"));
    }
    match st.signal() {
        Some(sig) => Err(format!("signal: {}", signal_name(sig))),
        None => Err(st.to_string()),
    }
}

/// A signal as Go's os.Signal names it.
fn signal_name(sig: i32) -> String {
    match sig {
        libc::SIGHUP => "hangup".into(),
        libc::SIGINT => "interrupt".into(),
        libc::SIGQUIT => "quit".into(),
        libc::SIGABRT => "abort trap".into(),
        libc::SIGKILL => "killed".into(),
        libc::SIGSEGV => "segmentation fault".into(),
        libc::SIGTERM => "terminated".into(),
        n => format!("signal {n}"),
    }
}

/// The editor to run: $VISUAL, else $EDITOR, else vi (editorCmd).
fn editor_program(m: &mut Model) -> String {
    if let Some(e) = &m.kept::<Outside>().editor {
        return e.clone();
    }
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "vi".into())
}

/// $EDITOR on path, through sh so an editor with flags ("code -w") runs
/// as typed (editorCmd).
pub fn editor_command(ed: &str, path: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(format!("{ed} \"$1\"")).arg("sh").arg(path);
    c
}

/// Opens path in $VISUAL or $EDITOR (vi without either), then done
/// (editorCmd).
pub fn edit(m: &mut Model, path: &str, done: Done) -> Out {
    let ed = editor_program(m);
    m.exec(editor_command(&ed, path), done);
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ran_says_it_as_go_does() {
        let ok = Command::new("/usr/bin/true").status();
        assert_eq!(ran(ok), Ok(()));
        let one = Command::new("/usr/bin/false").status();
        assert_eq!(ran(one), Err("exit status 1".into()));
        let gone = Command::new("/nonexistent/rush-editor").status();
        assert!(ran(gone).unwrap_err().starts_with("exec: "));
        let killed = Command::new("/bin/sh").args(["-c", "kill -9 $$"]).status();
        assert_eq!(ran(killed), Err("signal: killed".into()));
    }

    /// The editor runs once the key that asked has been handled, never
    /// within it, with the file as its one argument; done hears how it
    /// went.
    #[test]
    fn edit_runs_the_editor_on_the_file() {
        let dir = std::env::temp_dir().join(format!("rush-outside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a note.md");
        std::fs::write(&file, "old").unwrap();
        let mut m = Model::new("t");
        // A fake editor: writes what it was given into the file.
        m.set_editor_program("f() { printf 'edited %s' \"$(basename \"$1\")\" > \"$1\"; }; f");
        let said = std::sync::Arc::new(std::sync::Mutex::new(None));
        let s2 = said.clone();
        let p = file.to_string_lossy().into_owned();
        edit(
            &mut m,
            &p,
            Box::new(move |_, r| {
                *s2.lock().unwrap() = Some(r);
                Out::Draw
            }),
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old", "not yet");
        assert_eq!(m.run_outside(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited a note.md");
        assert_eq!(*said.lock().unwrap(), Some(Ok(())));
        // One that fails says how.
        m.set_editor_program("exit 3;");
        let s3 = said.clone();
        edit(
            &mut m,
            &p,
            Box::new(move |_, r| {
                *s3.lock().unwrap() = Some(r);
                Out::Draw
            }),
        );
        m.run_outside();
        assert_eq!(*said.lock().unwrap(), Some(Err("exit status 3".into())));
        assert_eq!(m.run_outside(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
