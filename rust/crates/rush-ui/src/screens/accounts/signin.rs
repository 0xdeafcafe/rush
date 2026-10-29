//! Signing in. An agent's own sign-in (claude auth login, codex login)
//! runs in the background, its output on a pipe and a pipe for its stdin:
//! both open the browser themselves and print the link, and Claude Code
//! reads a pasted code from stdin. After Go's internal/ui/signinsheet.go
//! (signInWait :23, signInSheet :29, signIn :59, launch :87, on :140,
//! took :163, firstURL :177, said :191, toTerminal :204, body :226, key
//! :256, paste :299). One whose output can't be followed would go to the
//! terminal (inTerminal), which waits on rush-draw handing the terminal
//! over: until then the sheet says so.

use std::io::{Read, Write};
use std::process::{ChildStdin, Command as Cmd, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_core::text::Line;

use super::bits::strip_ansi;
use crate::frame::sheet::{Sheet, sheet_title, text_field};
use crate::frame::work::Apply;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;
use crate::worker::Note;

/// How long a sign-in has to print its link before it goes to the
/// terminal (signInWait).
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(15);

/// What's done once the sign-in program has ended well, off the UI
/// thread: it reads what was signed in to, then says so on the UI thread.
pub type Finish = Box<dyn FnOnce() -> Apply + Send>;

/// Makes a sign-in's command and what's done once it succeeds; run off
/// the UI thread (signInStart).
pub type Start = Arc<dyn Fn() -> Result<(Cmd, Finish), String> + Send + Sync>;

/// The sign-in program running: its process group and its stdin
/// (signInRun).
pub struct Run {
    id: u64,
    pid: u32,
    stdin: Mutex<Option<ChildStdin>>,
}

impl Run {
    /// Stops it and what it started (kill).
    pub fn kill(&self) {
        // SAFETY: kill only signals; a group gone already is fine.
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGTERM);
        }
    }
}

static RUNS: AtomicU64 = AtomicU64::new(1);

/// The Sign in sheet (signInSheet).
pub struct SignInSheet {
    pub title: String,
    /// Kept to start it again in the terminal (inTerminal).
    pub start: Start,
    run: Option<Arc<Run>>,
    /// All it printed.
    pub out: String,
    pub url: String,
    /// It asked for a code.
    pub asks: bool,
    /// A code went, not yet answered.
    pub sent: bool,
    pub code: Vec<char>,
    /// Why it failed, once it has.
    pub err: Option<String>,
}

impl SignInSheet {
    pub fn new(title: &str, start: Start) -> SignInSheet {
        SignInSheet {
            title: title.into(),
            start,
            run: None,
            out: String::new(),
            url: String::new(),
            asks: false,
            sent: false,
            code: Vec::new(),
            err: None,
        }
    }

    /// Reads what the program printed: its link, and whether it asks for
    /// a code (took).
    pub fn took(&mut self, text: &str) {
        self.out.push_str(text);
        if self.url.is_empty() {
            self.url = first_url(&self.out);
        }
        if strip_ansi(&self.out).to_lowercase().contains("paste code") {
            self.asks = true;
        }
        if self.sent && !strip_ansi(text).trim().is_empty() {
            self.sent = false; // it answered the code
        }
    }

    /// The last thing the program printed that isn't its link (said).
    pub fn said(&self) -> String {
        strip_ansi(&self.out)
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.contains("http"))
            .unwrap_or("")
            .to_string()
    }

    /// Stops the background run and would hand the terminal to a fresh
    /// one (toTerminal).
    fn hand_terminal(&mut self, m: &mut Model) -> Out {
        if let Some(r) = &self.run {
            r.kill();
        }
        m.flash_err(
            "rush can't hand the terminal to a sign-in yet · run it in a terminal of your own",
        );
        Out::Close
    }
}

/// The first https link in out: bare, or an OSC 8 link's target, which
/// ends at the sequence's terminator (firstURL).
pub fn first_url(out: &str) -> String {
    let Some(i) = out.find("https://") else {
        return String::new();
    };
    let u = &out[i..];
    let end = u
        .find([' ', '\t', '\r', '\n', '\x07', '\x1b', '"', '\'', '<', '>'])
        .unwrap_or(u.len());
    u[..end].to_string()
}

/// Opens the sheet and starts the sign-in (signIn). With no loop (tests,
/// `--render`) nothing is started.
pub fn sign_in(m: &mut Model, title: &str, start: Start) -> Out {
    m.open_sheet(Box::new(SignInSheet::new(title, start.clone())));
    let Some(tx) = m.sender() else {
        return Out::Draw;
    };
    let tx = tx.lane(8);
    let id = RUNS.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("rush-signin".into())
        .spawn(move || launch(id, start, tx));
    if spawned.is_err() {
        m.flash_err("couldn't start a thread for that");
    }
    Out::Draw
}

/// The sheet whose run is id, or the sheet with no run yet when id is 0.
fn sheet_of(m: &mut Model, id: u64) -> Option<&mut SignInSheet> {
    let s = m.sheet_as::<SignInSheet>()?;
    match &s.run {
        Some(r) if r.id == id => Some(s),
        _ => None,
    }
}

/// Starts the sign-in with its output on a pipe and reads it till it
/// ends; finish runs once it succeeds (launch, drain, on).
fn launch(id: u64, start: Start, tx: rush_draw::Sender<Note>) {
    let send = |f: Apply| {
        tx.send(Note::Apply(f));
    };
    let (mut cmd, finish) = match start() {
        Ok(v) => v,
        Err(e) => {
            send(Box::new(move |m: &mut Model| {
                if let Some(s) = m.sheet_as::<SignInSheet>()
                    && s.run.is_none()
                {
                    s.err = Some(e);
                }
                Out::Draw
            }));
            return;
        }
    };
    use std::os::unix::process::CommandExt;
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0); // killed with what it starts
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let e = e.to_string();
            send(Box::new(move |m: &mut Model| {
                if let Some(s) = m.sheet_as::<SignInSheet>()
                    && s.run.is_none()
                {
                    s.err = Some(e);
                }
                Out::Draw
            }));
            return;
        }
    };
    let run = Arc::new(Run {
        id,
        pid: child.id(),
        stdin: Mutex::new(child.stdin.take()),
    });
    {
        let run = run.clone();
        send(Box::new(move |m: &mut Model| {
            match m.sheet_as::<SignInSheet>() {
                Some(s) if s.run.is_none() => s.run = Some(run),
                _ => run.kill(), // closed since: it only drains
            }
            Out::Draw
        }));
    }
    // No link in time: the terminal.
    {
        let tx = tx.clone();
        let _ = std::thread::Builder::new()
            .name("rush-signin-wait".into())
            .spawn(move || {
                std::thread::sleep(SIGN_IN_WAIT);
                tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                    let Some(s) = sheet_of(m, id) else {
                        return Out::Idle;
                    };
                    if s.url.is_empty() && s.err.is_none() {
                        let mut s = m.sheet.take().expect("the sign-in sheet");
                        let s: &mut dyn std::any::Any = s.as_mut();
                        if let Some(s) = s.downcast_mut::<SignInSheet>() {
                            return s.hand_terminal(m);
                        }
                    }
                    Out::Idle
                })));
            });
    }
    let mut readers = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                    if let Some(s) = sheet_of(m, id) {
                        s.took(&text);
                        return Out::Draw;
                    }
                    Out::Idle
                })));
            }
        }));
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait();
    let ok = status.as_ref().is_ok_and(|s| s.success());
    let then = ok.then(finish);
    let err = match status {
        Ok(s) if s.success() => String::new(),
        Ok(s) => s.to_string(),
        Err(e) => e.to_string(),
    };
    send(Box::new(move |m: &mut Model| {
        let Some(s) = sheet_of(m, id) else {
            return Out::Idle;
        };
        if let Some(then) = then {
            m.sheet = None;
            return then(m);
        }
        if s.url.is_empty() {
            let mut b = m.sheet.take().expect("the sign-in sheet");
            let b: &mut dyn std::any::Any = b.as_mut();
            if let Some(s) = b.downcast_mut::<SignInSheet>() {
                return s.hand_terminal(m);
            }
            return Out::Draw;
        }
        s.err = Some(err);
        s.sent = false;
        Out::Draw
    }));
}

/// Opens url in the browser, off the UI thread (browse).
pub fn browse(url: &str) {
    let url = url.to_string();
    let _ = std::thread::Builder::new()
        .name("rush-browse".into())
        .spawn(move || {
            let _ = Cmd::new("open")
                .arg(url)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
}

impl Sheet for SignInSheet {
    fn width(&self, _m: &Model) -> usize {
        96
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![sheet_title("Sign in", &self.title, w), Line::new()];
        let status = if let Some(e) = &self.err {
            line([sp("✗ ", RED), sp(format!("didn't sign in: {e}"), DIM)])
        } else if self.sent {
            line([sp("● ", YELLOW), sp("checking the code…", DIM)])
        } else if self.url.is_empty() && self.run.is_some() {
            line([sp("● ", YELLOW), sp("waiting for its sign-in link…", DIM)])
        } else if self.asks {
            line([
                sp("● ", GREEN),
                sp(
                    "finish in the browser it opened, or paste the code the page shows",
                    DIM,
                ),
            ])
        } else if !self.url.is_empty() {
            line([sp("● ", GREEN), sp("finish in the browser it opened", DIM)])
        } else {
            line([sp("● ", YELLOW), sp("starting…", DIM)])
        };
        let mut l = line([plain("  ")]);
        join(&mut l, status);
        out.push(l);
        if !self.url.is_empty() {
            out.push(Line::new());
            let n = 10.max(w.saturating_sub(4));
            let mut u = self.url.as_str();
            while !u.is_empty() {
                let mut cut = n.min(u.len());
                while !u.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push(line([plain("  "), sp(&u[..cut], BLUE)]));
                u = &u[cut..];
            }
        }
        if self.asks && self.err.is_none() {
            out.push(Line::new());
            let dots: Vec<char> = "•".repeat(self.code.len()).chars().collect();
            let mut l = line([plain("  "), sp("code  ", SUB)]);
            join(
                &mut l,
                text_field(
                    &dots,
                    dots.len(),
                    true,
                    "paste the code here",
                    w.saturating_sub(10),
                ),
            );
            out.push(l);
        }
        let said = self.said();
        if !said.is_empty() {
            out.push(Line::new());
            out.push(line([
                plain("  "),
                sp(
                    rush_draw::truncate(&format!("it says: {said}"), w.saturating_sub(2), "…")
                        .into_owned(),
                    FAINT,
                ),
            ]));
        }
        out.push(Line::new());
        if self.err.is_some() {
            out.push(keys_fit(
                w,
                &[("ctrl+t", "try in the terminal"), ("esc", "close")],
            ));
            return out;
        }
        out.push(keys_fit(
            w,
            &[
                ("ctrl+o", "open the link"),
                ("ctrl+y", "copy it"),
                ("enter", "send the code"),
                ("ctrl+t", "use the terminal"),
                ("esc", "cancel"),
            ],
        ));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" => {
                if let Some(r) = &self.run {
                    r.kill();
                }
                if self.err.is_none() {
                    m.flash("sign-in cancelled");
                }
                return Out::Close;
            }
            "ctrl+o" => {
                if !self.url.is_empty() {
                    browse(&self.url);
                }
            }
            "ctrl+y" => {
                if !self.url.is_empty() {
                    m.copy_text(self.url.clone());
                }
            }
            "ctrl+t" => return self.hand_terminal(m),
            "backspace" => {
                self.code.pop();
            }
            "enter" => {
                let Some(run) = self.run.clone() else {
                    return Out::Idle;
                };
                if self.code.is_empty() || self.err.is_some() {
                    return Out::Idle;
                }
                let l = format!("{}\n", self.code.iter().collect::<String>());
                self.code.clear();
                self.sent = true;
                // Off the UI thread: a full pipe would hold it.
                let _ = std::thread::Builder::new()
                    .name("rush-signin-code".into())
                    .spawn(move || {
                        if let Some(w) = run.stdin.lock().expect("stdin").as_mut() {
                            let _ = w.write_all(l.as_bytes());
                        }
                    });
            }
            _ => match k.text() {
                Some(c) if c != '\r' && c != '\n' => self.code.push(c),
                _ => return Out::Idle,
            },
        }
        Out::Draw
    }

    /// A pasted code (paste).
    fn paste(&mut self, _m: &mut Model, text: &str) -> Out {
        self.code
            .extend(text.split_whitespace().collect::<String>().chars());
        Out::Draw
    }
}
