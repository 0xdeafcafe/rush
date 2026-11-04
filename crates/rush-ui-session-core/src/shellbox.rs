//! Shell commands rush runs itself for you, never without your click or
//! key: a shell block of a reply (▶ run on its box, or enter on it
//! picked) and a command typed after `!` in the message box (enter runs
//! it; it doesn't wait for the agent nor interrupt it). Each runs in the
//! session's folder with your login shell (`$SHELL -lc`), stdin closed,
//! in a session (and process group) of its own, so ✕ ends it and all it
//! started, and rush quitting doesn't: its output goes to a file under
//! shell-runs/ that streams into its box (rush_convo's
//! render/draw/shellbox.rs), its code to a file beside it as it ends.
//! What it printed stays on your screen: → send to agent puts the
//! command and the end of its output in the message box, for you to
//! send as any message, through whichever harness the session has. A
//! block's run whose output is short (ShellRun::sends_itself) is sent so
//! by itself as it ends, unless Settings has short output wait for you.
//!
//! A chain (a block of several commands) says which command runs now and
//! how long each took, read from its processes every WATCH_EVERY with
//! rush_convo's Chain as an agent's Bash calls are; k skips the command
//! running now and p pauses it, as on an agent's (jobkeys.rs).
//!
//! The runs are kept per session while rush runs, by their box's ref,
//! and in rush's folder (shell-runs/<session>.json), so they're there
//! when the session is opened again: one running then, its process the
//! same one still (pid and start time), runs on in its box; one whose
//! code was written is done; one with neither ended while rush was
//! closed. Rust-only.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_convo::model::Item;
use rush_convo::shellbox::{BoxRef, OUT_KEEP, ShellRun, bang_ref, shell_blocks};
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
    /// The box whose commands are being edited (✎ edit), by its ref, and
    /// the text as it stands, edited as the message box is.
    editing: Option<(String, crate::frame::editor::Editor)>,
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

/// What runs a command for rush, as `/bin/sh -c RUNNER <shell> <script>
/// <exit file>`: the login shell ($0) runs the script ($1), its output
/// going to the file rush gave this one's; then its code is written to
/// the exit file ($2), by way of a .part file so it's never read half
/// written, and is this one's code too. TERM, INT and HUP wait for the
/// command (it gets them too, as its group does), so ✕ still has its code
/// written.
const RUNNER: &str = r#"trap : TERM INT HUP; "$0" -lc "$1"; c=$?; echo "$c" > "$2.part" && mv -f "$2.part" "$2"; exit "$c""#;

/// How often a run's output file is read for what's new, and whether it
/// has ended.
const FOLLOW_EVERY: Duration = Duration::from_millis(50);

/// The file a run's code is written to as it ends, beside its output's.
pub fn exit_of(log: &std::path::Path) -> PathBuf {
    log.with_extension("exit")
}

/// The code written to an exit file; None until it's there.
fn read_exit(p: &std::path::Path) -> Option<i32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// When pid started, in ms; 0 when it isn't running.
pub fn started_ms(pid: i32) -> Ms {
    rush_sys::proc::started(pid)
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as Ms)
}

/// Whether the process that ran a command is still the one running:
/// pid is there and started when it did (within a second; a pid used
/// again started later).
pub fn still_running(pid: i32, start: Ms) -> bool {
    if pid <= 0 || start <= 0 {
        return false;
    }
    let now = started_ms(pid);
    now != 0 && (now - start).abs() < 1000
}

/// Starts script in cwd with shell -lc, stdin closed, stdout and stderr
/// into the file log, in a session (and process group) of its own, so
/// it carries on when rush quits; its code is written beside it as it
/// ends (exit_of). A thread follows the file into the Shared it returns.
/// Returns its process (the runner's, the group's leader) and when that
/// started.
pub fn spawn(
    script: &str,
    cwd: &str,
    shell: &str,
    log: &std::path::Path,
) -> std::io::Result<(i32, Ms, Arc<Mutex<Shared>>)> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d)?;
    }
    let out = std::fs::File::create(log)?;
    let err = out.try_clone()?;
    let exit = exit_of(log);
    let _ = std::fs::remove_file(&exit);
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(RUNNER)
        .arg(shell)
        .arg(script)
        .arg(&exit)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    // SAFETY: setsid is async-signal-safe, all that runs between fork
    // and exec here.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    drop(cmd);
    let pid = child.id() as i32;
    // Read before it's waited for, while it can't be gone.
    let start = started_ms(pid);
    let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let g = gone.clone();
    std::thread::Builder::new()
        .name("rush-shell-wait".into())
        .spawn(move || {
            let _ = child.wait();
            g.store(true, std::sync::atomic::Ordering::SeqCst);
        })?;
    let shared = follow(
        log.to_path_buf(),
        0,
        Box::new(move || !gone.load(std::sync::atomic::Ordering::SeqCst)),
    )?;
    Ok((pid, start, shared))
}

/// Follows a run's output file from byte from into the Shared it
/// returns, until it has ended (its exit file is there, or alive says its
/// process is gone) and all it wrote is read; or until no one reads the
/// Shared.
pub fn follow(
    log: PathBuf,
    from: u64,
    alive: Box<dyn Fn() -> bool + Send>,
) -> std::io::Result<Arc<Mutex<Shared>>> {
    use std::io::{Seek, SeekFrom};
    let shared = Arc::new(Mutex::new(Shared::default()));
    let out = shared.clone();
    std::thread::Builder::new()
        .name("rush-shell-out".into())
        .spawn(move || {
            let exit = exit_of(&log);
            let open = || {
                let mut f = std::fs::File::open(&log).ok()?;
                f.seek(SeekFrom::Start(from)).ok()?;
                Some(f)
            };
            let mut f = open();
            let mut held: Vec<u8> = Vec::new();
            let put = |s: String| {
                if !s.is_empty() {
                    out.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .out
                        .push_str(&s);
                }
            };
            loop {
                if Arc::strong_count(&out) == 1 {
                    return; // let go of
                }
                let ended = exit.exists() || !alive();
                if f.is_none() {
                    f = open();
                }
                if let Some(f) = f.as_mut() {
                    loop {
                        let s = read_new(f, &mut held, ended);
                        let more = !s.is_empty() && ended;
                        put(s);
                        if !more {
                            break;
                        }
                    }
                }
                if ended {
                    let mut sh = out.lock().unwrap_or_else(|e| e.into_inner());
                    sh.exit = Some(read_exit(&exit));
                    sh.eof = true;
                    return;
                }
                std::thread::sleep(FOLLOW_EVERY);
            }
        })?;
    Ok(shared)
}

/// What's new in f as text, at most a MiB of it: a character cut by the
/// read's end waits in held for the next, unless this is the last read.
fn read_new(f: &mut std::fs::File, held: &mut Vec<u8>, last: bool) -> String {
    let mut buf = [0u8; 16 * 1024];
    let mut got = 0;
    while got < 1 << 20 {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                held.extend_from_slice(&buf[..n]);
                got += n;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let upto = match std::str::from_utf8(held) {
        Ok(_) => held.len(),
        Err(e) if e.error_len().is_none() && !last => e.valid_up_to(),
        Err(_) => held.len(),
    };
    let s = String::from_utf8_lossy(&held[..upto]).into_owned();
    held.drain(..upto);
    s
}

/// The last OUT_KEEP bytes of a run's output file from its first whole
/// line, how many lines came before them, and how long the file is.
fn read_tail(p: &std::path::Path) -> Option<(String, usize, u64)> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    let skip = len.saturating_sub(OUT_KEEP as u64);
    let mut dropped = 0;
    let mut buf = [0u8; 64 * 1024];
    let mut left = skip;
    while left > 0 {
        let want = (left as usize).min(buf.len());
        let n = f.read(&mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        dropped += buf[..n].iter().filter(|&&b| b == b'\n').count();
        left -= n as u64;
    }
    f.seek(SeekFrom::Start(skip)).ok()?;
    let mut rest = Vec::new();
    f.take(len - skip).read_to_end(&mut rest).ok()?;
    if skip > 0
        && let Some(nl) = rest.iter().position(|&b| b == b'\n')
    {
        // A line cut at the front is let go of whole.
        rest.drain(..=nl);
        dropped += 1;
    }
    Some((String::from_utf8_lossy(&rest).into_owned(), dropped, len))
}

/// What a run whose record says it was running as rush went is now:
/// still running (its process is, the same one), its output followed
/// again from the byte it returns; done, its code in its exit file; or
/// ended while rush was closed, how not known. Its output is read back
/// from its file. Reads the disk: off the UI thread.
pub fn pick_up(run: &mut ShellRun, now: Ms) -> Option<u64> {
    let log = PathBuf::from(&run.log);
    let mtime = |p: &std::path::Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as Ms)
    };
    let exit = exit_of(&log);
    let code = (!run.log.is_empty()).then(|| read_exit(&exit)).flatten();
    let mut at = 0;
    if !run.log.is_empty()
        && let Some((out, dropped, len)) = read_tail(&log)
    {
        (run.out, run.dropped, at) = (out, dropped, len);
        run.ver += 1;
    }
    if code.is_none() && !run.log.is_empty() && still_running(run.pid, run.pid_start) {
        return Some(at);
    }
    match code {
        Some(c) => run.finish(Some(c), mtime(&exit).unwrap_or(now)),
        None => {
            run.lost = true;
            run.finish(None, mtime(&log).unwrap_or(run.start));
        }
    }
    None
}

/// The shell running a command and what runs under it, read from the
/// machine, for which command of its chain runs now. Blocks on the
/// system: off the UI thread.
fn read_shell(pid: i32, command: &str) -> Option<Shell> {
    let t = rush_sys::proc::snapshot();
    // The runner's one child is the login shell that runs the command.
    let pid = match super::jobprocs::kids(&t, pid, 0).as_slice() {
        [k] if t
            .procs
            .get(&k.pid)
            .is_some_and(|p| super::jobprocs::shell_comm(&p.comm)) =>
        {
            k.pid
        }
        _ => pid,
    };
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
    // A block run as you edited it, not as the agent wrote it.
    run.edited = bang.is_none() && block_script(m, r).is_some_and(|b| b.trim() != script);
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
        None => {
            let log = log_of(key, r, now);
            match spawn(script, &cwd, &login_shell(), &log) {
                Ok((pid, pid_start, shared)) => {
                    (run.pid, run.pid_start) = (pid, pid_start);
                    run.log = log.to_string_lossy().into_owned();
                    Some((pid, shared))
                }
                Err(e) => {
                    m.fail(&format!("couldn't run it: {e}"));
                    return Out::Draw;
                }
            }
        }
    };
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
    let ended = !run.running();
    k.runs.insert(r.to_string(), Arc::new(run));
    k.r#gen += 1;
    // Kept at once, running or not: rush opened again picks it up.
    save(m, key);
    show(m, key);
    m.ask_wake(Instant::now() + Duration::from_millis(100));
    if ended {
        return send_itself(m, key, r).unwrap_or(Out::Draw);
    }
    Out::Draw
}

/// Sends run r of session key to the agent as it ends, when its output is
/// short enough to (ShellRun::sends_itself); None when it isn't sent.
fn send_itself(m: &mut Model, key: &str, r: &str) -> Option<Out> {
    let run = run_of(m, key, r)?;
    if !run.sends_itself() || crate::accounts::store(m).config.hold_short_output {
        return None;
    }
    // As s would: a send that asks first (a cold cache, another agent's
    // words) asks, the text in the box.
    let (o, sent) = send_as_typed(m, key, &run.send_text());
    change(m, key, r, |x| x.told = true);
    save(m, key);
    if sent {
        m.flash("its output was short · sent to the agent");
    }
    Some(o)
}

/// Runs what's typed after `!` in session key's box, drawn after what
/// its latest turn has now; in a session with no turn yet, as turn 0's,
/// above the turns to come.
pub fn run_typed(m: &mut Model, key: &str) -> Out {
    let script = conn(m, key).input.string();
    if script.trim().is_empty() {
        return Out::Idle;
    }
    let (turn, after) = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .and_then(|v| v.sess.turns.last())
        .map_or((0, 0), |t| (t.n, t.items.len()));
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
    let (mut ended, mut running) = (Vec::new(), false);
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
                ended.push(r);
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
    if !ended.is_empty() {
        save(m, key);
    }
    for r in &ended {
        let _ = send_itself(m, key, r);
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
    let edit = kept(m, key)
        .editing
        .as_ref()
        .map(|(r, e)| rush_convo::shellbox::ShellEdit {
            r: r.clone(),
            text: e.string(),
            cursor: e.pos.min(e.text.len()),
        });
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key)
        && v.o.shell_edit != edit
    {
        v.o.shell_edit = edit;
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
    let script = match block_script(m, r) {
        Some(s) if !run.edited => s,
        _ => run.command.clone(),
    };
    start(m, key, r, &script, bang)
}

/// What box r's editor starts with: what it last ran, or its block's
/// commands as the agent wrote them.
fn command_now(m: &mut Model, key: &str, r: &str) -> Option<String> {
    match run_of(m, key, r) {
        Some(x) => Some(x.command.clone()),
        None => block_script(m, r),
    }
}

/// ✎ edit (e): box r's commands become an editor in place, to change
/// before they run (again).
pub fn edit(m: &mut Model, key: &str, r: &str) -> Out {
    if kept(m, key).procs.contains_key(r) {
        m.flash_err("it's still running · ✕ or x stops it first");
        return Out::Draw;
    }
    let Some(cmd) = command_now(m, key, r) else {
        return Out::Idle;
    };
    let mut e = crate::frame::editor::Editor::default();
    e.set(&cmd);
    kept(m, key).editing = Some((r.to_string(), e));
    if let Some(v) = m.session.as_mut() {
        v.sel = r.to_string();
    }
    show(m, key);
    Out::Draw
}

/// Whether a box of session key is being edited.
pub fn editing(m: &mut Model, key: &str) -> bool {
    kept(m, key).editing.is_some()
}

/// A key while box r's commands are edited: enter runs them as they
/// stand, esc (or ctrl+c) leaves them as they were; ↑↓ go between their
/// lines, and the rest edit as in the message box (a new line, undo).
fn edit_key(m: &mut Model, key: &str, k: &Press) -> Out {
    let Some((r, mut e)) = kept(m, key).editing.take() else {
        return Out::Pass;
    };
    match k.s.as_str() {
        "enter" => {
            let script = e.string();
            if script.trim().is_empty() {
                kept(m, key).editing = Some((r, e));
                m.flash_err("there's nothing to run");
                return Out::Draw;
            }
            show(m, key);
            let bang = run_of(m, key, &r)
                .filter(|x| x.bang)
                .map(|x| (x.turn, x.after));
            return start(m, key, &r, &script, bang);
        }
        "esc" | "ctrl+c" => {
            show(m, key);
            return Out::Draw;
        }
        "up" | "down" => {
            let up = k.s == "up";
            e.pos = line_move(&e.text, e.pos, up);
            e.anchor = None;
        }
        _ => {
            if let Some(copied) = e.key(k)
                && !copied.is_empty()
            {
                m.copy_text(copied);
            }
        }
    }
    kept(m, key).editing = Some((r, e));
    show(m, key);
    Out::Draw
}

/// The cursor at pos moved a line up or down, to the same column or the
/// line's end; at the first (last) line, to its start (end).
fn line_move(text: &[char], pos: usize, up: bool) -> usize {
    use crate::frame::editor::{line_end, line_start};
    let pos = pos.min(text.len());
    let start = line_start(text, pos);
    let col = pos - start;
    if up {
        if start == 0 {
            return 0;
        }
        let prev = line_start(text, start - 1);
        (prev + col).min(start - 1)
    } else {
        let end = line_end(text, pos);
        if end >= text.len() {
            return text.len();
        }
        let next = end + 1;
        (next + col).min(line_end(text, next))
    }
}

/// How long the quick model has to sum a run up.
const SUM_TIME: Duration = Duration::from_secs(45);
const SUM_BUDGET: f64 = 0.05;

/// Sends text to session key's agent as if typed and sent with enter: at
/// once, or queued while it works, as the box would; what's typed in the
/// box is left as it was. Says too whether it was sent (or queued).
fn send_as_typed(m: &mut Model, key: &str, text: &str) -> (Out, bool) {
    let c = conn(m, key);
    let was = c.input.string();
    let imgs = std::mem::take(&mut c.imgs);
    let bang = std::mem::take(&mut c.bang);
    c.input.set(text);
    let o = super::keys::send(m, key, false);
    let c = conn(m, key);
    let sent = c.input.is_empty();
    if sent {
        // Sent (or queued): what you were typing comes back.
        c.input.set(&was);
        c.imgs = imgs;
        c.bang = bang;
    }
    (o, sent)
}

/// → send (s): run r's command, code and the end of its output go to the
/// agent at once, as a message you sent.
pub fn send(m: &mut Model, key: &str, r: &str) -> Out {
    let Some(run) = run_of(m, key, r) else {
        return Out::Idle;
    };
    if run.running() {
        m.flash_err("it's still running · send it once it's done");
        return Out::Draw;
    }
    let (o, _) = send_as_typed(m, key, &run.send_text());
    change(m, key, r, |x| x.told = true);
    save(m, key);
    o
}

/// ≈ summary (m): a quick model sums run r up, off the UI thread, and the
/// summary goes to the agent at once; when it can't, the box says why
/// and nothing is sent.
pub fn summary(m: &mut Model, key: &str, r: &str) -> Out {
    let Some(run) = run_of(m, key, r) else {
        return Out::Idle;
    };
    if run.running() {
        m.flash_err("it's still running · send it once it's done");
        return Out::Draw;
    }
    if run.summing {
        return Out::Idle;
    }
    let Some(a) = agent(m, key).cloned() else {
        change(m, key, r, |x| {
            x.note = "couldn't sum it up: no agent here".into()
        });
        return Out::Draw;
    };
    change(m, key, r, |x| (x.summing, x.note) = (true, String::new()));
    let prompt = run.summary_prompt();
    let reg = m.reg.clone();
    let asker = super::viewsuggest::asker(m);
    let (key, r) = (key.to_string(), r.to_string());
    m.later(
        move || -> Result<String, String> {
            use super::viewsuggest::{Ask, Call};
            let ask: Arc<dyn Ask + Send + Sync> =
                asker.unwrap_or_else(|| Arc::new(rush_efficiency::advisor::Cli::new(reg.clone())));
            let prof = super::viewsuggest::profile(&reg, &a);
            let (quick, _) = ask.models(&prof);
            if quick.is_empty() {
                return Err("this harness has no quick model".into());
            }
            let call = Call {
                model: quick,
                budget: SUM_BUDGET,
                timeout: SUM_TIME,
                dirs: Vec::new(),
                system: rush_convo::shellbox::SUMMARY_SYSTEM,
                prompt,
                schema: rush_convo::shellbox::SUMMARY_SCHEMA.into(),
            };
            let (_, got) = ask.ask(&prof, &call);
            let b = got?;
            rush_convo::shellbox::summary_answer(&b).ok_or_else(|| "its answer was empty".into())
        },
        move |m, got| {
            let Some(run) = run_of(m, &key, &r) else {
                return Out::Idle;
            };
            match got {
                Ok(s) => {
                    change(m, &key, &r, |x| x.summing = false);
                    let (o, _) = send_as_typed(m, &key, &run.summary_text(&s));
                    change(m, &key, &r, |x| (x.told, x.told_summary) = (true, true));
                    save(m, &key);
                    o
                }
                Err(e) => {
                    let e = e.split_whitespace().collect::<Vec<_>>().join(" ");
                    change(m, &key, &r, |x| {
                        (x.summing, x.note) = (false, format!("couldn't sum it up: {e}"));
                    });
                    Out::Draw
                }
            }
        },
    );
    Out::Draw
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
/// after what's typed there: yours to change and send, or not (✎ draft).
pub fn draft(m: &mut Model, key: &str, r: &str) -> Out {
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
    if let Some(x) = run_of(m, key, r).filter(|x| x.edited) {
        return Some(x.command.clone());
    }
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
/// runs it again, e edits it first, s sends it to the agent, d puts it
/// in your message, m sends a summary of it, x stops it (or clears it
/// once it's done), k and p skip or pause the command of its chain
/// running now. While a box is edited every key is its editor's.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if editing(m, &key) {
        return edit_key(m, &key, k);
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
        "e" if !running => edit(m, &key, &r),
        "r" if done => again(m, &key, &r),
        "s" if done => send(m, &key, &r),
        "d" if done => draft(m, &key, &r),
        "m" if done => summary(m, &key, &r),
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
        "draft" => draft(m, key, r),
        "summary" => summary(m, key, r),
        "edit" => edit(m, key, r),
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
    if editing(m, key) {
        return Some(fit_keys(
            w,
            &[
                ("enter", "run it"),
                ("⇧enter", "new line"),
                ("ctrl+z", "undo"),
                ("esc", "as it was"),
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
            ("d", "draft it first"),
            ("m", "summarise and send"),
            ("r", "run again"),
            ("e", "edit and run"),
            ("enter", "all it wrote"),
            ("c", "copy"),
            ("x", "clear"),
            ("esc", "back to message"),
        ]
    } else {
        &[
            ("enter", "run it here"),
            ("e", "edit it first"),
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

/// key as a file's name: what isn't a letter, digit, - or . as _.
fn safe_name(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where session key's runs are kept.
pub fn file_of(key: &str) -> PathBuf {
    rush_state::dir()
        .join("shell-runs")
        .join(format!("{}.json", safe_name(key)))
}

/// Where run r of session key, started at start, writes its output while
/// it runs: shell-runs/<session>/<start>-<ref>.log, its code beside it.
pub fn log_of(key: &str, r: &str, start: Ms) -> PathBuf {
    rush_state::dir()
        .join("shell-runs")
        .join(safe_name(key))
        .join(format!("{start}-{}.log", safe_name(r)))
}

/// Reads session key's kept runs, once, off the UI thread. One that was
/// running as rush went is picked up (pick_up): still running, it's
/// followed again.
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
    let now = m.now;
    // The runs, those still running and what follows each, and whether
    // any that was running has ended.
    type Got = (File, Vec<(String, Arc<Mutex<Shared>>)>, bool);
    m.later(
        move || -> Option<Got> {
            let b = std::fs::read(&path).ok()?;
            let mut f = serde_json::from_slice::<File>(&b).ok()?;
            let mut live = Vec::new();
            let mut ended = false;
            for (r, run) in f.runs.iter_mut() {
                if run.end != 0 {
                    continue;
                }
                let Some(at) = pick_up(run, now) else {
                    ended = true;
                    continue;
                };
                let (pid, start) = (run.pid, run.pid_start);
                let alive = Box::new(move || still_running(pid, start));
                match follow(PathBuf::from(&run.log), at, alive) {
                    Ok(shared) => live.push((r.clone(), shared)),
                    Err(_) => {
                        run.lost = true;
                        run.finish(None, now);
                        ended = true;
                    }
                }
            }
            Some((f, live, ended))
        },
        move |m, got| {
            let Some((f, live, ended)) = got else {
                return Out::Idle;
            };
            let now = m.now;
            let k = kept(m, &key);
            for (r, mut run) in f.runs {
                if k.runs.contains_key(&r) {
                    continue;
                }
                run.ver = 1;
                k.runs.insert(r, Arc::new(run));
            }
            for (r, shared) in live {
                let Some(pid) = k.runs.get(&r).map(|x| x.pid) else {
                    continue;
                };
                if k.procs.contains_key(&r) {
                    continue;
                }
                k.procs.insert(
                    r,
                    Proc {
                        pid,
                        shared,
                        exited: None,
                        reading: false,
                        looked: now,
                    },
                );
            }
            k.r#gen += 1;
            if ended {
                save(m, &key);
            }
            show(m, &key);
            m.ask_wake(Instant::now() + Duration::from_millis(100));
            Out::Draw
        },
    );
}

/// Writes session key's runs, the newest KEEP_RUNS: those running with
/// their process and output file, not their output (read back from the
/// file); off the UI thread. A finished run's output file goes once its
/// output is kept here.
fn save(m: &mut Model, key: &str) {
    if !KEEP.with(Cell::get) {
        return;
    }
    let k = kept(m, key);
    let mut all: Vec<(&String, &Arc<ShellRun>)> = k.runs.iter().collect();
    all.sort_by_key(|(_, r)| std::cmp::Reverse(r.start));
    all.truncate(KEEP_RUNS);
    let mut gone: Vec<PathBuf> = Vec::new();
    let f = File {
        runs: all
            .into_iter()
            .map(|(r, x)| {
                let mut x = (**x).clone();
                if x.running() {
                    (x.out, x.dropped) = (String::new(), 0);
                } else if !x.log.is_empty() {
                    gone.push(PathBuf::from(std::mem::take(&mut x.log)));
                }
                (r.clone(), x)
            })
            .collect(),
    };
    let path = file_of(key);
    m.later(
        move || {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            rush_state::store::write_json(&path, &f)?;
            for log in gone {
                let _ = std::fs::remove_file(exit_of(&log));
                let _ = std::fs::remove_file(log);
            }
            Ok(())
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
    /// together, in its file and followed from there, and its exit code,
    /// written beside it for a rush opened later.
    #[test]
    fn spawn_runs_in_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("runs").join("1-t1_sh_0_0.log");
        let (pid, start, shared) = spawn(
            "echo out; echo err >&2; pwd; exit 3",
            dir.path().to_str().unwrap(),
            "/bin/sh",
            &log,
        )
        .expect("started");
        assert!(pid > 1);
        assert!(start > 0, "when it started is known");
        // Wait for the threads without sleeping: they finish as the
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
        let file = std::fs::read_to_string(&log).unwrap();
        assert!(file.contains("out\n") && file.contains("err\n"), "{file}");
        assert_eq!(read_exit(&exit_of(&log)), Some(3));
    }

    /// A run that was running as rush went, read back: its process still
    /// the same one, it runs on, its output so far read from its file;
    /// its code written, it's done with that code; neither, it ended
    /// while rush was closed. What it wrote is kept each time.
    #[test]
    fn a_run_rush_went_away_from_is_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("9-t0_bang_9.log");
        std::fs::write(&log, "compiling\nhalf done\n").unwrap();
        let me = std::process::id() as i32;
        let my_start = started_ms(me);
        assert!(my_start > 0);
        let was = |pid: i32, pid_start: Ms| {
            let mut r = ShellRun::new("make", dir.path().to_str().unwrap(), 9);
            (r.pid, r.pid_start) = (pid, pid_start);
            r.log = log.to_string_lossy().into_owned();
            r
        };
        // Still running: this test's own process stands in for it.
        let mut r = was(me, my_start);
        let at = pick_up(&mut r, 50_000);
        assert_eq!(at, Some(20), "followed on from the file's end");
        assert!(r.running());
        assert_eq!(r.lines(), ["compiling", "half done"]);
        // A pid used again by another process isn't it.
        let mut r = was(me, my_start - 60_000);
        assert_eq!(pick_up(&mut r, 50_000), None);
        assert!(r.lost && !r.running(), "{r:?}");
        assert_eq!(r.outcome(), "ended while rush was closed");
        assert_eq!(r.lines(), ["compiling", "half done"]);
        // Its code written as it ended.
        std::fs::write(exit_of(&log), "2\n").unwrap();
        let mut r = was(me, my_start - 60_000);
        assert_eq!(pick_up(&mut r, 50_000), None);
        assert!(!r.running() && !r.lost);
        assert_eq!(r.exit, Some(2));
        assert_eq!(r.outcome(), "exit 2");
        assert_eq!(r.lines(), ["compiling", "half done"]);
        // Even while its process is still there (writing its code was
        // its last act).
        let mut r = was(me, my_start);
        assert_eq!(pick_up(&mut r, 50_000), None);
        assert_eq!(r.exit, Some(2));
    }

    /// A long output file is read back from its last OUT_KEEP bytes, the
    /// lines before counted.
    #[test]
    fn a_long_output_file_is_read_from_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("x.log");
        let line = "y".repeat(1023) + "\n";
        std::fs::write(&log, line.repeat(300)).unwrap();
        let (out, dropped, len) = read_tail(&log).unwrap();
        assert_eq!(len, 300 * 1024);
        assert!(out.len() <= OUT_KEEP);
        assert_eq!(dropped + out.lines().count(), 300);
        assert!(out.starts_with('y'));
    }
}
