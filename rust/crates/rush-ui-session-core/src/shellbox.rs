//! Shell commands rush runs itself for you, never without your click or
//! key: a shell block of a reply (▶ run on its box, or enter on it
//! picked) and a command typed after `!` in the message box (enter runs
//! it; it doesn't wait for the agent nor interrupt it). Each runs in the
//! session's folder with your login shell (`$SHELL -lc`), stdin closed,
//! in a process group of its own, so ✕ ends it and all it started; its
//! output streams into its box (rush_convo's render/draw/shellbox.rs).
//! What it printed stays on your screen: → send to agent puts the
//! command and the end of its output in the message box, for you to
//! send as any message, through whichever harness the session has.
//!
//! A chain (a block of several commands) says which command runs now and
//! how long each took, read from its processes every WATCH_EVERY with
//! rush_convo's Chain as an agent's Bash calls are; k skips the command
//! running now and p pauses it, as on an agent's (jobkeys.rs).
//!
//! The runs are kept per session while rush runs, by their box's ref,
//! and the finished ones in rush's folder (shell-runs/<session>.json), so
//! they're there when the session is opened again. Rust-only.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_convo::model::Item;
use rush_convo::shellbox::{BoxRef, ShellRun, bang_ref, shell_blocks};
use rush_convo::{Shell, ShellProc};
use rush_core::text::Line;
use rush_core::time::Ms;

use super::{agent, conn, open_key};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::paint::fit_keys;

/// How often a running chain's processes are read, to say which of its
/// commands runs now.
pub const WATCH_EVERY: Ms = 500;

/// How long after its shell exits a run waits for the rest of its output
/// (a command it left running in the background holds the pipe open).
const DRAIN: Ms = 300;

/// The most runs kept for a session on disk, the newest.
const KEEP_RUNS: usize = 50;

/// What a running command's reader has for the UI: output not yet taken,
/// and how its shell exited (Some once it has: the code, or None for a
/// signal), and whether its output has ended.
#[derive(Debug, Default)]
pub struct Shared {
    pub out: String,
    pub exit: Option<Option<i32>>,
    pub eof: bool,
}

/// A command running: its shell's pid (also its process group's), what
/// its reader has, when its shell was seen to exit, and when its
/// processes were last read.
struct Proc {
    pid: i32,
    shared: Arc<Mutex<Shared>>,
    exited: Option<Ms>,
    reading: bool,
    looked: Ms,
}

/// One session's runs.
#[derive(Default)]
pub struct Kept {
    /// By their box's ref.
    pub runs: HashMap<String, Arc<ShellRun>>,
    procs: HashMap<String, Proc>,
    loaded: bool,
    /// Bumped as the runs change; the view takes them again when it moves.
    r#gen: u64,
    shown: u64,
}

/// Every session's runs, by its key.
#[derive(Default)]
pub struct Boxes {
    pub by: HashMap<String, Kept>,
}

fn kept<'a>(m: &'a mut Model, key: &str) -> &'a mut Kept {
    m.kept::<Boxes>().by.entry(key.to_string()).or_default()
}

/// What a test runs in place of a shell: given the script and the folder,
/// what it prints and its exit code, or None to leave it running.
pub type Fake = fn(&str, &str) -> Option<(String, i32)>;

thread_local! {
    static FAKE: Cell<Option<Fake>> = const { Cell::new(None) };
    static KEEP: Cell<bool> = const { Cell::new(true) };
    static RAN: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// For tests: runs nothing, f says what each run printed; and keeps
/// nothing on disk (set_keep turns that back on).
pub fn set_fake(f: Option<Fake>) {
    FAKE.with(|c| c.set(f));
    KEEP.with(|c| c.set(f.is_none()));
}

/// For tests: whether runs are kept on disk.
pub fn set_keep(on: bool) {
    KEEP.with(|c| c.set(on));
}

/// For tests: the scripts the fake was given, and their folders.
pub fn ran() -> Vec<(String, String)> {
    RAN.with(|r| r.borrow().clone())
}

/// The folder the session works in, where its commands run.
pub fn folder(m: &Model, key: &str) -> String {
    if let Some(a) = agent(m, key)
        && !a.job.cwd.is_empty()
    {
        return a.job.cwd.clone();
    }
    match m.session.as_ref().filter(|v| v.key == key) {
        Some(v) if !v.sess.cwd.is_empty() => v.sess.cwd.clone(),
        Some(v) => v.sess.info.cwd.clone(),
        None => String::new(),
    }
}

/// The shell that runs them: yours.
fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into())
}

/// The id a paused command of run r is kept under, for rush to let it go
/// on as it quits (shells.rs).
fn pause_id(key: &str, r: &str) -> String {
    format!("shellbox:{key}:{r}")
}

/// Starts script in cwd with shell -lc, stdin closed, stdout and stderr
/// into one pipe, in a process group of its own; a thread reads what it
/// writes into the Shared it returns, and its exit.
pub fn spawn(script: &str, cwd: &str, shell: &str) -> std::io::Result<(i32, Arc<Mutex<Shared>>)> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let (mut r, w) = std::io::pipe()?;
    let w2 = w.try_clone()?;
    let mut cmd = Command::new(shell);
    cmd.arg("-lc")
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(w)
        .stderr(w2)
        .process_group(0);
    let mut child = cmd.spawn()?;
    // The pipe's writing ends are the child's alone now: its output ends
    // when it and all it started are done with them.
    drop(cmd);
    let pid = child.id() as i32;
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (out, done) = (shared.clone(), shared.clone());
    std::thread::Builder::new()
        .name("rush-shell-out".into())
        .spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            let mut held: Vec<u8> = Vec::new();
            loop {
                let n = match r.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                held.extend_from_slice(&buf[..n]);
                // A character cut by the read's end waits for the next.
                let upto = match std::str::from_utf8(&held) {
                    Ok(_) => held.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => held.len(),
                };
                let s = String::from_utf8_lossy(&held[..upto]).into_owned();
                held.drain(..upto);
                out.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .out
                    .push_str(&s);
            }
            let mut sh = out.lock().unwrap_or_else(|e| e.into_inner());
            sh.out.push_str(&String::from_utf8_lossy(&held));
            sh.eof = true;
        })?;
    std::thread::Builder::new()
        .name("rush-shell-wait".into())
        .spawn(move || {
            let code = child.wait().ok().map(|s| s.code());
            done.lock().unwrap_or_else(|e| e.into_inner()).exit = Some(code.flatten());
        })?;
    Ok((pid, shared))
}

/// The shell running a command and what runs under it, read from the
/// machine, for which command of its chain runs now. Blocks on the
/// system: off the UI thread.
fn read_shell(pid: i32, command: &str) -> Option<Shell> {
    let t = rush_sys::proc::snapshot();
    let p = t.procs.get(&pid)?;
    let start = super::jobprocs::ms(p);
    let kids = super::jobprocs::kids(&t, pid, 0);
    // A shell may run its last command in its own place (exec).
    let kids = if super::jobprocs::shell_comm(&p.comm) {
        kids
    } else {
        vec![ShellProc {
            pid,
            args: rush_sys::proc::args(pid),
            start,
            kids,
        }]
    };
    Some(Shell {
        cmd: String::new(),
        command: command.to_string(),
        start,
        kids,
    })
}

/// The script of the shell block box r stands for, as the open
/// conversation has it now.
pub fn block_script(m: &Model, r: &str) -> Option<String> {
    let BoxRef::Block { turn, item, k } = BoxRef::parse(r)? else {
        return None;
    };
    let v = m.session.as_ref()?;
    let t = v.sess.turns.iter().find(|t| t.n == turn)?;
    let Item::Text { text, .. } = t.items.get(item)? else {
        return None;
    };
    Some(shell_blocks(text).get(k)?.script())
}

/// The run of box r, if it was run.
pub fn run_of(m: &mut Model, key: &str, r: &str) -> Option<Arc<ShellRun>> {
    kept(m, key).runs.get(r).cloned()
}

/// Runs script for box r of session key, now, in its folder; bang is a
/// `!` run's place (its turn, and the items it had).
pub fn start(m: &mut Model, key: &str, r: &str, script: &str, bang: Option<(u32, usize)>) -> Out {
    let script = script.trim();
    if script.is_empty() {
        return Out::Idle;
    }
    load(m, key);
    if kept(m, key).procs.contains_key(r) {
        m.flash_err("that's still running · ✕ or x stops it");
        return Out::Draw;
    }
    let cwd = folder(m, key);
    let fake = FAKE.with(Cell::get);
    if fake.is_none() && (cwd.is_empty() || !std::path::Path::new(&cwd).is_dir()) {
        m.flash_err(&format!(
            "its folder isn't there to run it in: {}",
            crate::style::tildify(&cwd)
        ));
        return Out::Draw;
    }
    let now = m.now;
    let mut run = ShellRun::new(script, &cwd, now);
    if let Some((turn, after)) = bang {
        (run.bang, run.turn, run.after) = (true, turn, after);
    }
    let proc_ = match fake {
        Some(f) => {
            RAN.with(|x| x.borrow_mut().push((script.to_string(), cwd.clone())));
            match f(script, &cwd) {
                Some((out, code)) => {
                    run.push(&out);
                    run.finish(Some(code), now);
                    None
                }
                None => Some((0, Arc::new(Mutex::new(Shared::default())))),
            }
        }
        None => match spawn(script, &cwd, &login_shell()) {
            Ok(p) => Some(p),
            Err(e) => {
                m.fail(&format!("couldn't run it: {e}"));
                return Out::Draw;
            }
        },
    };
    let done = proc_.is_none();
    let k = kept(m, key);
    if let Some((pid, shared)) = proc_ {
        k.procs.insert(
            r.to_string(),
            Proc {
                pid,
                shared,
                exited: None,
                reading: false,
                looked: now,
            },
        );
    }
    k.runs.insert(r.to_string(), Arc::new(run));
    k.r#gen += 1;
    if done {
        save(m, key);
    }
    show(m, key);
    m.ask_wake(Instant::now() + Duration::from_millis(100));
    Out::Draw
}

/// Runs what's typed after `!` in session key's box, drawn after what
/// its latest turn has now.
pub fn run_typed(m: &mut Model, key: &str) -> Out {
    let script = conn(m, key).input.string();
    if script.trim().is_empty() {
        return Out::Idle;
    }
    let (turn, after) = m
        .session
        .as_ref()
        .and_then(|v| v.sess.turns.last())
        .map_or((0, 0), |t| (t.n, t.items.len()));
    if turn == 0 {
        m.flash_err("there's no conversation here yet to put it in");
        return Out::Draw;
    }
    let c = conn(m, key);
    c.input.clear();
    c.bang = false;
    let r = bang_ref(turn, m.now);
    start(m, key, &r, &script, Some((turn, after)))
}

/// Takes in what session key's running commands wrote and how they
/// ended, reads a chain's processes now and then, and hands the view the
/// runs as they stand: once a frame, from the dock.
pub fn sync(m: &mut Model, key: &str) {
    load(m, key);
    let now = m.now;
    let (mut ended, mut running) = (false, false);
    let mut looks: Vec<(String, i32, String)> = Vec::new();
    let k = kept(m, key);
    let refs: Vec<String> = k.procs.keys().cloned().collect();
    for r in refs {
        let Some(run) = k.runs.get_mut(&r) else {
            k.procs.remove(&r);
            continue;
        };
        let Some(p) = k.procs.get_mut(&r) else {
            continue;
        };
        let (out, exit, eof) = {
            let mut s = p.shared.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut s.out), s.exit, s.eof)
        };
        if !out.is_empty() {
            Arc::make_mut(run).push(&out);
            k.r#gen += 1;
        }
        if let Some(code) = exit {
            let at = *p.exited.get_or_insert(now);
            if eof || now - at >= DRAIN {
                Arc::make_mut(run).finish(code, now);
                k.procs.remove(&r);
                k.r#gen += 1;
                ended = true;
                continue;
            }
        }
        running = true;
        if p.pid > 0 && !p.reading && now - p.looked >= WATCH_EVERY {
            (p.reading, p.looked) = (true, now);
            looks.push((r.clone(), p.pid, run.command.clone()));
        }
    }
    for (r, pid, cmd) in looks {
        let key = key.to_string();
        m.later(
            move || read_shell(pid, &cmd),
            move |m, sh| {
                let now = m.now;
                let k = kept(m, &key);
                if let Some(p) = k.procs.get_mut(&r) {
                    p.reading = false;
                }
                let (Some(sh), Some(run)) = (sh, k.runs.get_mut(&r)) else {
                    return Out::Idle;
                };
                if !run.running() {
                    return Out::Idle;
                }
                Arc::make_mut(run).watch(&sh, now);
                k.r#gen += 1;
                show(m, &key);
                Out::Draw
            },
        );
    }
    if ended {
        save(m, key);
    }
    if running {
        m.ask_wake(Instant::now() + Duration::from_millis(100));
    }
    show(m, key);
}

/// Hands the open view session key's runs, when they've changed since.
fn show(m: &mut Model, key: &str) {
    let dir = folder(m, key);
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key)
        && v.o.shell_cwd != dir
    {
        v.o.shell_cwd = dir;
    }
    let k = kept(m, key);
    let (r#gen, n, shown) = (k.r#gen, k.runs.len(), k.shown);
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return;
    };
    if shown == r#gen && v.o.shells.len() == n {
        return;
    }
    let k = kept(m, key);
    k.shown = r#gen;
    let runs = k.runs.clone();
    if let Some(v) = m.session.as_mut() {
        v.o.shells = runs;
    }
}

/// Changes run r of session key with f, and has it drawn again.
fn change(m: &mut Model, key: &str, r: &str, f: impl FnOnce(&mut ShellRun)) {
    let k = kept(m, key);
    if let Some(run) = k.runs.get_mut(r) {
        let run = Arc::make_mut(run);
        f(run);
        run.ver += 1;
        k.r#gen += 1;
    }
    show(m, key);
}

/// Stops run r: its process group is told to end, then made to, three
/// seconds on.
pub fn stop(m: &mut Model, key: &str, r: &str) -> Out {
    let Some(pid) = kept(m, key).procs.get(r).map(|p| p.pid) else {
        m.flash("that isn't running");
        return Out::Draw;
    };
    let _ = crate::shells::take_paused(&pause_id(key, r));
    change(m, key, r, |x| (x.stopped, x.paused) = (true, false));
    if pid == 0 {
        // A test's: nothing ran.
        let now = m.now;
        change(m, key, r, |x| x.finish(None, now));
        kept(m, key).procs.remove(r);
        save(m, key);
        return Out::Draw;
    }
    m.flash("stopping it");
    let _ = std::thread::Builder::new()
        .name("rush-shell-stop".into())
        .spawn(move || {
            use rush_sys::proc::kill_group;
            let _ = kill_group(pid, libc::SIGCONT);
            let _ = kill_group(pid, libc::SIGTERM);
            std::thread::sleep(Duration::from_secs(3));
            let _ = kill_group(pid, libc::SIGKILL);
        });
    Out::Draw
}

/// Runs box r again: its block's script as the reply has it, a `!` run's
/// command where it was.
pub fn again(m: &mut Model, key: &str, r: &str) -> Out {
    let Some(run) = run_of(m, key, r) else {
        return Out::Idle;
    };
    let bang = run.bang.then_some((run.turn, run.after));
    let script = block_script(m, r).unwrap_or_else(|| run.command.clone());
    start(m, key, r, &script, bang)
}

/// Lets go of a finished run: a block's box is as before it ran, a `!`
/// run's is gone.
pub fn clear(m: &mut Model, key: &str, r: &str) -> Out {
    if kept(m, key).procs.contains_key(r) {
        return stop(m, key, r);
    }
    let k = kept(m, key);
    if k.runs.remove(r).is_none() {
        return Out::Idle;
    }
    k.r#gen += 1;
    if let Some(v) = m.session.as_mut() {
        v.o.open.remove(r);
        if v.sel == r {
            v.sel.clear();
        }
    }
    save(m, key);
    show(m, key);
    Out::Draw
}

/// Puts run r's command and the end of its output in the message box,
/// after what's typed there: yours to send, or not.
pub fn send(m: &mut Model, key: &str, r: &str) -> Out {
    let Some(run) = run_of(m, key, r) else {
        return Out::Idle;
    };
    if run.running() {
        m.flash_err("it's still running · send it once it's done");
        return Out::Draw;
    }
    let text = run.send_text();
    let c = conn(m, key);
    c.bang = false;
    if c.input.is_empty() {
        c.input.set(&text);
    } else {
        let mut was = c.input.string();
        was.push_str("\n\n");
        was.push_str(&text);
        c.input.set(&was);
    }
    change(m, key, r, |x| x.sent = true);
    if let Some(v) = m.session.as_mut() {
        v.sel.clear(); // to the box, to send it
    }
    save(m, key);
    m.flash("in your message · enter sends it to the agent");
    Out::Draw
}

/// k: ends the command of run r's chain running now, the rest of it going
/// on as after a failure.
fn skip(m: &mut Model, key: &str, r: &str) -> Out {
    let Some(rp) = run_of(m, key, r).and_then(|x| x.running_part()) else {
        m.flash_err("rush hasn't seen which command of it runs yet · x stops it all");
        return Out::Draw;
    };
    m.flash(&format!("skipping {}", rp.command));
    let procs = rp.procs;
    super::jobkeys::off_ui(m, move || {
        let errs: Vec<String> = super::jobkeys::live_parts(&procs)
            .into_iter()
            .filter_map(|p| {
                rush_sys::actions::end_tree(p.pid, p.start, Duration::from_secs(3))
                    .err()
                    .map(|e| e.to_string())
            })
            .collect();
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("\n"))
        }
    });
    Out::Draw
}

/// p: pauses the command of run r's chain running now (the whole run when
/// it's one command), or lets it go on.
fn pause(m: &mut Model, key: &str, r: &str) -> Out {
    use crate::shells as sh;
    let id = pause_id(key, r);
    if let Some(pp) = sh::take_paused(&id) {
        change(m, key, r, |x| x.paused = false);
        m.flash(&format!("{} goes on", pp.what));
        super::jobkeys::off_ui(m, move || sh::signal_parts(&pp.procs, libc::SIGCONT));
        return Out::Draw;
    }
    let Some(run) = run_of(m, key, r).filter(|x| x.running()) else {
        return Out::Idle;
    };
    let Some(pid) = kept(m, key).procs.get(r).map(|p| p.pid) else {
        return Out::Idle;
    };
    let (what, procs) = match run.running_part() {
        Some(rp) => (
            rush_ui_list_extras::jobs::first_word(&rp.command).to_string(),
            super::jobkeys::live_parts(&rp.procs),
        ),
        None => {
            let start = rush_sys::proc::started(pid);
            let procs = start
                .map(|start| vec![sh::PartProc { pid, start }])
                .unwrap_or_default();
            ("it".to_string(), procs)
        }
    };
    if pid > 0 && procs.is_empty() {
        m.flash_err("its command is gone already");
        return Out::Draw;
    }
    sh::note_paused(
        &id,
        sh::PausedPart {
            at: m.now,
            what: what.clone(),
            procs: procs.clone(),
        },
    );
    change(m, key, r, |x| x.paused = true);
    m.flash(&format!("paused {what} · p lets it go on"));
    if pid > 0 {
        super::jobkeys::off_ui(m, move || sh::signal_parts(&procs, libc::SIGSTOP));
    }
    Out::Draw
}

/// The box picked in the open conversation, if one is.
fn picked(m: &Model) -> Option<String> {
    let v = m.session.as_ref()?;
    BoxRef::parse(&v.sel).map(|_| v.sel.clone())
}

/// enter on box r: runs it when it hasn't, else opens all it wrote, or
/// folds it again.
fn enter(m: &mut Model, key: &str, r: &str) -> Out {
    if run_of(m, key, r).is_none() {
        let Some(script) = block_script(m, r) else {
            return Out::Idle;
        };
        return start(m, key, r, &script, None);
    }
    if let Some(v) = m.session.as_mut() {
        let open = v.o.open.get(r).copied().unwrap_or(false);
        v.o.open.insert(r.to_string(), !open);
    }
    Out::Draw
}

/// The command of box r, to copy.
fn command_of(m: &mut Model, key: &str, r: &str) -> Option<String> {
    block_script(m, r).or_else(|| run_of(m, key, r).map(|x| x.command.clone()))
}

fn copy(m: &mut Model, key: &str, r: &str) -> Out {
    match command_of(m, key, r) {
        Some(c) => {
            m.copy_text(c);
            Out::Draw
        }
        None => Out::Idle,
    }
}

/// The keys of the open Session that are a box's: `!` into an empty box
/// turns it to running commands, and there enter runs one, esc and a
/// backspace at its start turn it back; on a box picked in the
/// conversation, enter runs it (or opens all it wrote), c copies it, r
/// runs it again, s puts it in your message, x stops it (or clears it
/// once it's done), k and p skip or pause the command of its chain
/// running now.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let s = k.s.as_str();
    if conn(m, &key).bang {
        return bang_key(m, &key, s);
    }
    let empty = conn(m, &key).input.is_empty();
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    if s == "!" && empty && sel.is_empty() && box_has_keys(m, &key) {
        conn(m, &key).bang = true;
        return Out::Draw;
    }
    if !empty || super::views::view_name(m, &key) != "conversation" {
        return Out::Pass;
    }
    let Some(r) = picked(m) else {
        return Out::Pass;
    };
    let running = kept(m, &key).procs.contains_key(&r);
    let done = !running && run_of(m, &key, &r).is_some();
    match s {
        "enter" => enter(m, &key, &r),
        "c" => copy(m, &key, &r),
        "r" if done => again(m, &key, &r),
        "s" if done => send(m, &key, &r),
        "x" if running => stop(m, &key, &r),
        "x" if done => clear(m, &key, &r),
        "k" if running => skip(m, &key, &r),
        "p" if running => pause(m, &key, &r),
        _ => Out::Pass,
    }
}

/// Whether what's typed goes to the box now: nothing over it has the
/// keys.
fn box_has_keys(m: &mut Model, key: &str) -> bool {
    !m.pane_idle
        && !(m.ext.calls.hosted_away())(m)
        && !rush_ui_session_cards::conn(m, key).card_focus
}

/// A key while the box runs commands.
fn bang_key(m: &mut Model, key: &str, s: &str) -> Out {
    match s {
        "enter" => run_typed(m, key),
        "esc" => {
            conn(m, key).bang = false;
            Out::Draw
        }
        "backspace" | "ctrl+h" => {
            let c = conn(m, key);
            if c.input.pos == 0 && c.input.selection().is_none() {
                c.bang = false;
                return Out::Draw;
            }
            Out::Pass
        }
        "ctrl+c" if conn(m, key).input.is_empty() => {
            conn(m, key).bang = false;
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// A click on a link drawn in the conversation: a box's button does what
/// it says; None for any other link.
pub fn click(m: &mut Model, key: &str, url: &str) -> Option<Out> {
    let rest = url.strip_prefix("rush:shell/")?;
    let (act, r) = rest.split_once('/')?;
    BoxRef::parse(r)?;
    if let Some(v) = m.session.as_mut() {
        v.sel = r.to_string(); // the box clicked is the one picked
    }
    Some(match act {
        "run" => match block_script(m, r) {
            Some(script) => start(m, key, r, &script, None),
            None => Out::Idle,
        },
        "copy" => copy(m, key, r),
        "stop" => stop(m, key, r),
        "again" => again(m, key, r),
        "send" => send(m, key, r),
        "clear" => clear(m, key, r),
        _ => Out::Pass,
    })
}

/// The bottom row's keys while a box is picked.
pub fn hint(m: &mut Model, key: &str, w: usize, picked: &str) -> Option<Line> {
    let w = w.saturating_sub(4);
    if conn(m, key).bang && picked.is_empty() {
        return Some(fit_keys(
            w,
            &[
                ("enter", "run it here"),
                ("esc", "back to a message"),
                ("⌫", "at the start, too"),
            ],
        ));
    }
    BoxRef::parse(picked)?;
    let running = kept(m, key).procs.contains_key(picked);
    let done = !running && run_of(m, key, picked).is_some();
    let keys: &[(&str, &str)] = if running {
        &[
            ("x", "stop it"),
            ("k", "skip the command running"),
            ("p", "pause"),
            ("enter", "all it wrote"),
            ("c", "copy"),
            ("esc", "back to message"),
        ]
    } else if done {
        &[
            ("s", "send to agent"),
            ("r", "run again"),
            ("enter", "all it wrote"),
            ("c", "copy"),
            ("x", "clear"),
            ("esc", "back to message"),
        ]
    } else {
        &[
            ("enter", "run it here"),
            ("c", "copy"),
            ("↑↓", "pick"),
            ("esc", "back to message"),
        ]
    };
    Some(fit_keys(w, keys))
}

// ---- kept between runs of rush ----

/// What's kept of a session's runs.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct File {
    runs: BTreeMap<String, ShellRun>,
}

/// Where session key's finished runs are kept.
pub fn file_of(key: &str) -> PathBuf {
    let name: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    rush_state::dir()
        .join("shell-runs")
        .join(format!("{name}.json"))
}

/// Reads session key's kept runs, once, off the UI thread.
fn load(m: &mut Model, key: &str) {
    let k = kept(m, key);
    if k.loaded {
        return;
    }
    k.loaded = true;
    if !KEEP.with(Cell::get) {
        return;
    }
    let path = file_of(key);
    let key = key.to_string();
    m.later(
        move || {
            let b = std::fs::read(&path).ok()?;
            serde_json::from_slice::<File>(&b).ok()
        },
        move |m, f| {
            let Some(f) = f else { return Out::Idle };
            let k = kept(m, &key);
            for (r, mut run) in f.runs {
                if k.runs.contains_key(&r) {
                    continue;
                }
                if run.end == 0 {
                    // rush went while it ran.
                    (run.end, run.stopped) = (run.start, true);
                }
                run.ver = 1;
                k.runs.insert(r, Arc::new(run));
            }
            k.r#gen += 1;
            show(m, &key);
            Out::Draw
        },
    );
}

/// Writes session key's finished runs, the newest KEEP_RUNS, off the UI
/// thread.
fn save(m: &mut Model, key: &str) {
    if !KEEP.with(Cell::get) {
        return;
    }
    let k = kept(m, key);
    let mut done: Vec<(&String, &Arc<ShellRun>)> =
        k.runs.iter().filter(|(_, r)| !r.running()).collect();
    done.sort_by_key(|(_, r)| std::cmp::Reverse(r.start));
    done.truncate(KEEP_RUNS);
    let f = File {
        runs: done
            .into_iter()
            .map(|(r, x)| (r.clone(), (**x).clone()))
            .collect(),
    };
    let path = file_of(key);
    m.later(
        move || {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            rush_state::store::write_json(&path, &f)
        },
        |m, r: std::io::Result<()>| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                m.fail(&format!("couldn't keep the command's output: {e}"));
                Out::Draw
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real shell in a temp folder: its output, stdout and stderr
    /// together, and its exit code; then the pipe's end.
    #[test]
    fn spawn_runs_in_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (pid, shared) = spawn(
            "echo out; echo err >&2; pwd; exit 3",
            dir.path().to_str().unwrap(),
            "/bin/sh",
        )
        .expect("started");
        assert!(pid > 1);
        // Wait for both threads without sleeping: they finish as the
        // shell does.
        loop {
            let s = shared.lock().unwrap();
            if s.eof && s.exit.is_some() {
                break;
            }
            drop(s);
            std::thread::yield_now();
        }
        let s = shared.lock().unwrap();
        assert_eq!(s.exit, Some(Some(3)));
        assert!(
            s.out.contains("out\n") && s.out.contains("err\n"),
            "{}",
            s.out
        );
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert!(
            s.out.contains(real.to_str().unwrap()) || s.out.contains(dir.path().to_str().unwrap()),
            "{}",
            s.out
        );
    }
}
