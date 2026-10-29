//! The preview's live mirror: the picked agent's own screen, attached in
//! the background the way #full attaches and drawn into a terminal in
//! memory the size of the Session beside the list, so the pane shows
//! exactly what the session draws rather than its conversation read
//! back. After Go's internal/ui/live.go (live :23, frameEvery :45,
//! liveCapable :56, daemonRunning :64, previewID :103, openLive :105,
//! next, resize :174, close :185, lines :195, syncLive :228, closeLive
//! :253, onLiveOpen :261, onLive :280, liveSize :297, liveLines :304),
//! view.go:1225-1235 (the pane: its strip, then the screen or the
//! summary) and :1480 (claudeStrip), keys.go:491 (v swaps the
//! screen for the summary), and typing into it: embed.go (canEmbed :12,
//! embedKey :17, embedPaste :26, sendLive :32), keys.go:57 (every key
//! goes to it first), :265 and :315 (tab and → give it the keys),
//! model.go:1401 (pastes), :1664 and :1726 (a click on it gives it the
//! keys, one on Agents takes them back), keybind.go:111 (ctrl+] is never
//! a chord while it has them) and view.go:2760-2781 (the box waits).
//! With the summary showing, the strip stays over it so v brings the
//! screen back (view.go:1225: the strip over either).
//!
//! One mirror at a time, for the agent the pane shows, sized to it, and
//! only while the pane is drawn: it's let go as the pane closes or moves
//! on, while a session's own screen has the terminal
//! (cmdbar::attach::attached, live.go:238), and attach lets it go itself
//! through [`close`] (cmdbar::attach::Attached::close_live). Attaching
//! and asking whether the agent's service runs are done off the UI
//! thread; what the session draws is read on a thread of its own into
//! the screen, which pokes the UI on a lane of one, so a busy session
//! costs at most a frame per poke the UI takes, and an idle one nothing.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rush_core::adapter::{AdapterRef, Mirror, Profile};
use rush_core::agent::Agent;
use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use super::vt::Vt;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::cmdbar::attach::{Attached, acct_of, attached};
use crate::style::*;
use crate::worker::Note;

/// How long a mirror that failed or ended waits before it's tried again,
/// so a session opened elsewhere isn't fought over.
const RETRY_MS: Ms = 10_000;
/// How long the service's answer stands (daemonRunning).
const DAEMON_MS: Ms = 1_000;

/// What the screen and its reader share.
struct Shared {
    vt: Mutex<Vt>,
    /// Takes the answers to the session's queries; None once let go.
    conn: Mutex<Option<Box<dyn Write + Send>>>,
    /// Something was drawn.
    ready: AtomicBool,
    /// The stream ended, or it was let go.
    dead: AtomicBool,
}

/// An agent's own screen, mirrored (live).
struct Live {
    key: String,
    short: String,
    ad: AdapterRef,
    p: Profile,
    w: usize,
    h: usize,
    sh: Arc<Shared>,
    /// Which mirror this is, for what its reader says.
    n: u64,
    /// The screen as last drawn, while nothing's been written since.
    drawn: Vec<Line>,
    /// What's typed into it, written by a thread of its own in order
    /// (sendLive); None until the attach is in.
    keys: Option<std::sync::mpsc::Sender<Vec<u8>>>,
}

/// Whether an agent's service ran when last asked.
#[derive(Clone, Copy, Default)]
struct Daemon {
    at: Ms,
    up: bool,
    asking: bool,
}

/// The preview's mirror and what it's kept by (Go's m.live,
/// m.liveOpening, m.liveFailed, m.claudeView).
#[derive(Default)]
pub struct LiveState {
    live: Option<Live>,
    opening: String,
    failed: (String, Ms),
    /// The agent the pane drew this frame and the screen's size there;
    /// None when no pane was drawn.
    want: Option<(String, usize, usize)>,
    daemons: HashMap<String, Daemon>,
    /// v: the summary in the screen's place (claudeView 1).
    pub summary: bool,
    /// The keys go to the screen (Go's m.embedded).
    pub embedded: bool,
}

static MIRRORS: AtomicU64 = AtomicU64::new(1);

/// The name rush's attachments for the preview go by, one per rush
/// (previewID).
fn preview_id() -> String {
    format!("rush-preview-{}", std::process::id())
}

/// Whether a's screen can be mirrored: a session its agent's service runs
/// with a process; attaching to anything else would start it again just
/// for a preview (liveCapable).
fn capable(m: &mut Model, a: &Agent) -> bool {
    !a.interactive && a.worker.is_some() && daemon_running(m, a)
}

/// Whether a's agent's service is up, as last asked: asked again off the
/// UI thread at most once a second, down until the first answer
/// (daemonRunning).
fn daemon_running(m: &mut Model, a: &Agent) -> bool {
    let Some(ad) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.joiner().is_some())
    else {
        return false;
    };
    let p = acct_of(m, a);
    let now = m.now;
    let st = m.kept::<LiveState>();
    let d = st.daemons.entry(p.dir.clone()).or_default();
    if d.asking || d.at != 0 && now - d.at < DAEMON_MS {
        return d.up;
    }
    d.asking = true;
    let (dir, key) = (p.dir.clone(), p.dir.clone());
    m.later(
        move || ad.joiner().is_some_and(|j| j.service_up(&p)),
        move |m, up| {
            let now = m.now;
            let d = m.kept::<LiveState>().daemons.entry(dir).or_default();
            let was = d.up;
            *d = Daemon {
                at: now,
                up,
                asking: false,
            };
            if was != up { Out::Draw } else { Out::Idle }
        },
    );
    m.kept::<LiveState>()
        .daemons
        .get(&key)
        .is_some_and(|d| d.up)
}

/// The pane is about to draw agent key's Session w by rows: the screen
/// is sized to it, less the strip (liveSize).
pub fn want(m: &mut Model, key: &str, w: usize, rows: usize) {
    m.kept::<LiveState>().want =
        Some((key.to_string(), w.saturating_sub(3), rows.saturating_sub(1)));
}

/// Keeps one mirror open for the agent the pane drew this frame, sized to
/// it, and lets it go when the pane closes or moves on (syncLive). Run
/// once a frame, after the pane.
pub fn sync(m: &mut Model, _: &mut Vec<Line>) {
    let want = m.kept::<LiveState>().want.take();
    let a = want
        .as_ref()
        .and_then(|(k, _, _)| m.agents.iter().find(|a| &a.key == k).cloned());
    let shown = m.session.is_none() && m.sheet.is_none() && m.confirm.is_none() && !m.on_place();
    let (Some((_, w, h)), Some(a), true, None) = (want, a, shown, attached(m)) else {
        close(m);
        return;
    };
    if a.rush || w < 20 || h < 4 || !capable(m, &a) {
        close(m);
        return;
    }
    let now = m.now;
    let st = m.kept::<LiveState>();
    if st.failed.0 == a.key && now - st.failed.1 < RETRY_MS {
        return;
    }
    if let Some(l) = st.live.as_mut().filter(|l| l.key == a.key) {
        resize(l, w, h);
        return;
    }
    if st.opening == a.key {
        return;
    }
    close(m);
    open(m, &a, w, h);
}

/// Attaches to a's screen off the UI thread, w by h (openLive).
fn open(m: &mut Model, a: &Agent, w: usize, h: usize) {
    let Some(ad) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.joiner().is_some())
    else {
        return;
    };
    // attach lets the mirror go before it hands the terminal over.
    m.kept::<Attached>().close_live = Some(close);
    m.kept::<LiveState>().opening = a.key.clone();
    let p = acct_of(m, a);
    let (key, short) = (a.key.clone(), a.job.id.clone());
    let (ad2, p2, short2) = (ad.clone(), p.clone(), short.clone());
    let (cols, rows) = (
        w.min(u16::MAX as usize) as u16,
        h.min(u16::MAX as usize) as u16,
    );
    m.later(
        move || {
            let j = ad.joiner().expect("filtered");
            j.mirror(&p, &short, &preview_id(), cols, rows)
                .map_err(|e| e.to_string())
        },
        move |m, r| {
            let l = Live {
                key,
                short: short2,
                ad: ad2,
                p: p2,
                w,
                h,
                sh: Arc::new(Shared {
                    vt: Mutex::new(Vt::new(w, h)),
                    conn: Mutex::new(None),
                    ready: AtomicBool::new(false),
                    dead: AtomicBool::new(false),
                }),
                n: MIRRORS.fetch_add(1, Ordering::Relaxed),
                drawn: Vec::new(),
                keys: None,
            };
            on_open(m, l, r)
        },
    );
}

/// The attach is in (onLiveOpen): kept and read, unless the pane moved
/// on meanwhile; a failure says why and waits before trying again.
fn on_open(m: &mut Model, l: Live, r: Result<Mirror, String>) -> Out {
    let now = m.now;
    let st = m.kept::<LiveState>();
    if l.key != st.opening {
        if let Ok(mr) = r {
            let_go(Some(mr.conn));
        }
        return Out::Idle;
    }
    st.opening.clear();
    let mr = match r {
        Ok(mr) => mr,
        Err(e) => {
            // The conversation stays up; say why the live one is missing.
            st.failed = (l.key.clone(), now);
            m.fail(&format!("live view unavailable: {e}"));
            return Out::Draw;
        }
    };
    let modes: String = mr.modes.iter().map(|n| format!("\x1b[?{n}h")).collect();
    {
        let mut vt = l.sh.vt.lock().unwrap_or_else(|e| e.into_inner());
        vt.write(modes.as_bytes());
        vt.take_answers();
    }
    *l.sh.conn.lock().unwrap_or_else(|e| e.into_inner()) = Some(mr.conn);
    let (sh, n) = (l.sh.clone(), l.n);
    let tx = m.sender().map(|t| t.lane(1));
    let relay = mr.relay;
    let spawned = std::thread::Builder::new()
        .name("rush-live".into())
        .spawn(move || {
            let mut feed = Feed {
                sh: sh.clone(),
                tx: tx.clone(),
                n,
            };
            let _ = relay(&mut feed);
            sh.dead.store(true, Ordering::SeqCst);
            if let Some(tx) = tx {
                tx.send(Note::Apply(Box::new(move |m| on_live(m, n))));
            }
        });
    if spawned.is_err() {
        m.fail("couldn't start a thread for the live view");
        return Out::Draw;
    }
    let mut l = l;
    l.keys = typist(l.sh.clone());
    m.kept::<LiveState>().live = Some(l);
    Out::Draw
}

/// A thread that writes what's typed into the session, in order, until
/// the mirror is let go (sendLive's writes, one at a time).
fn typist(sh: Arc<Shared>) -> Option<std::sync::mpsc::Sender<Vec<u8>>> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("rush-live-keys".into())
        .spawn(move || {
            for b in rx {
                if sh.dead.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(c) = sh.conn.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                    let _ = c.write_all(&b);
                    let _ = c.flush();
                }
            }
        })
        .ok()?;
    Some(tx)
}

/// What the session draws, written into the screen; its queries answered
/// up the stream; the UI poked to draw it.
struct Feed {
    sh: Arc<Shared>,
    tx: Option<rush_draw::Sender<Note>>,
    n: u64,
}

impl Write for Feed {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.sh.dead.load(Ordering::SeqCst) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        let answers = {
            let mut vt = self.sh.vt.lock().unwrap_or_else(|e| e.into_inner());
            vt.write(b);
            vt.take_answers()
        };
        if !answers.is_empty()
            && let Some(c) = self
                .sh
                .conn
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
        {
            let _ = c.write_all(&answers);
        }
        self.sh.ready.store(true, Ordering::SeqCst);
        if let Some(tx) = &self.tx {
            // One poke waiting is enough: it draws whatever's in by then.
            let n = self.n;
            let _ = tx.try_send(Note::Apply(Box::new(move |m| on_live(m, n))));
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The screen moved, or its stream ended (onLive): an ended one is let go
/// and not tried again for a while.
fn on_live(m: &mut Model, n: u64) -> Out {
    let now = m.now;
    let st = m.kept::<LiveState>();
    let Some(l) = st.live.as_ref().filter(|l| l.n == n) else {
        return Out::Idle;
    };
    if l.sh.dead.load(Ordering::SeqCst) {
        st.failed = (l.key.clone(), now);
        close(m);
    }
    Out::Draw
}

/// Sizes the screen to w by h, and the session's attachment off the UI
/// thread (resize).
fn resize(l: &mut Live, w: usize, h: usize) {
    if (w, h) == (l.w, l.h) {
        return;
    }
    (l.w, l.h) = (w, h);
    l.sh.vt
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .resize(w, h);
    let (ad, p, short) = (l.ad.clone(), l.p.clone(), l.short.clone());
    let _ = std::thread::Builder::new()
        .name("rush-live-resize".into())
        .spawn(move || {
            if let Some(j) = ad.joiner() {
                let _ = j.resize(&p, &short, &preview_id(), w as u16, h as u16);
            }
        });
}

/// Lets go of the mirror, and of one being opened (closeLive).
pub fn close(m: &mut Model) {
    let Some(st) = m.kept.peek::<LiveState>() else {
        return;
    };
    if st.live.is_none() && st.opening.is_empty() {
        return;
    }
    let st = m.kept::<LiveState>();
    st.opening.clear();
    st.embedded = false;
    if let Some(l) = st.live.take() {
        l.sh.dead.store(true, Ordering::SeqCst);
        let conn = l.sh.conn.lock().unwrap_or_else(|e| e.into_inner()).take();
        let_go(conn);
    }
}

/// Closes a connection off the UI thread: a socket's close is a system
/// call.
fn let_go(conn: Option<Box<dyn Write + Send>>) {
    if let Some(c) = conn {
        let _ = std::thread::Builder::new()
            .name("rush-live-close".into())
            .spawn(move || drop(c));
    }
}

/// Whether agent key's screen is mirrored and has drawn.
fn ready(m: &Model, key: &str) -> bool {
    m.kept
        .peek::<LiveState>()
        .and_then(|st| st.live.as_ref())
        .is_some_and(|l| l.key == key && l.sh.ready.load(Ordering::SeqCst))
}

/// The pane for a's Session w wide, rows tall, while its screen is
/// mirrored and showing: the strip, then its title and the screen
/// (view.go:1225, liveLines); None for the conversation instead.
pub fn pane(m: &mut Model, a: &Agent, w: usize, rows: usize) -> Option<Vec<Line>> {
    if a.rush || !ready(m, &a.key) || m.kept::<LiveState>().summary {
        return None;
    }
    let sw = w.saturating_sub(3);
    let mut out = vec![strip(m, a, sw)];
    let mut title = line([
        sp("▍", FAINT),
        Span::new(one_line(&a.display_name), bold(TEXT)),
        plain("  "),
        sp("●", ORANGE),
        sp(" live", DIM),
    ]);
    if embedded(m) {
        let h = crate::session::harness_name(&m.reg, &a.kind);
        title = line([
            sp("▍", ORANGE),
            Span::new("SESSION  ", bold(ORANGE)),
            Span::new(one_line(&a.display_name), bold(BRIGHT)),
            sp(
                format!("  ·  keys go to {h} · ctrl+] or click Agents to come back"),
                DIM,
            ),
        ]);
    }
    title = fit(title, sw);
    out.push(title);
    let st = m.kept::<LiveState>();
    let l = st.live.as_mut()?;
    let dirty =
        l.sh.vt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_dirty();
    if dirty || l.drawn.is_empty() {
        l.drawn = l.sh.vt.lock().unwrap_or_else(|e| e.into_inner()).lines();
    }
    out.extend(l.drawn.iter().cloned());
    // Each row in the pane's margins (paneFrameRow).
    Some(
        out.into_iter()
            .take(rows)
            .map(|l| {
                let mut r = line([plain("  ")]);
                join(&mut r, fit(l, sw));
                r.spans.push(plain(" "));
                r
            })
            .collect(),
    )
}

/// The strip over the summary of the agent picked, w wide, while its
/// screen is mirrored and the summary was picked with v; None otherwise
/// (view.go:1225: the strip heads either view).
pub fn summary_strip(m: &mut Model, w: usize) -> Option<Line> {
    let i = m.focused()?;
    let a = m.agents[i].clone();
    let summary = m.kept.peek::<LiveState>().is_some_and(|st| st.summary);
    if a.rush || !summary || !ready(m, &a.key) {
        return None;
    }
    let sw = w.saturating_sub(3);
    let mut r = line([plain("  ")]);
    join(&mut r, fit(strip(m, &a, sw), sw));
    r.spans.push(plain(" "));
    Some(r)
}

/// The Session's head for an agent outside rush mode: its two views, the
/// screen and the summary, and what typing does (claudeStrip).
fn strip(m: &Model, a: &Agent, w: usize) -> Line {
    let live = ready(m, &a.key);
    let summary = m.kept.peek::<LiveState>().is_some_and(|st| st.summary);
    let screen_on = !summary && live;
    let tab = |name: &str, on: bool, avail: bool| -> Line {
        let label = format!(" {name} ");
        if on {
            line([Span::new(label, bold(TEXT).on(TAB_ON_BG))])
        } else if !avail {
            line([sp(label, FAINT)])
        } else {
            line([sp(label, SUB)])
        }
    };
    let mut left = line([plain("  ")]);
    join(&mut left, tab("screen", screen_on, live));
    left.spans.push(plain(" "));
    join(&mut left, tab("summary", !screen_on, true));
    let right = if embedded(m) {
        line([
            Span::new("typing into it", bold(ORANGE)),
            sp(" · ctrl+] comes back", DIM),
        ])
    } else if screen_on {
        line([sp(
            "typing goes into it · ctrl+] leaves · ctrl+f full screen",
            DIM,
        )])
    } else if !live && a.interactive {
        line([sp(a.where_from(), DIM)])
    } else if !live {
        line([sp("not running · enter resumes it", DIM)])
    } else {
        Line::new()
    };
    // withTabHint: v views, when it fits.
    let hint = line([sp("   v", DIM), sp(" views", FAINT)]);
    if left.width() + hint.width() <= w.saturating_sub(right.width() + 1 + 2) {
        join(&mut left, hint);
    }
    let mut right = right;
    right.spans.push(plain(" "));
    let gap = w as isize - left.width() as isize - right.width() as isize;
    let mut out = if gap < 2 {
        fit(left, w)
    } else {
        let mut l = left;
        l.spans.push(plain(blanks(gap as usize)));
        join(&mut l, right);
        l
    };
    out = on_ground(out, w, CHROME_BG);
    out
}

/// v in the list, with nothing typed: an agent outside rush mode's
/// Session swaps its live screen for its summary, and shows (keys.go:491).
pub fn key(m: &mut Model, k: &Press) -> Out {
    if k.s != "v" || !m.input.is_empty() {
        return Out::Pass;
    }
    let Some(i) = m.focused() else {
        return Out::Pass;
    };
    if m.agents[i].rush {
        return Out::Pass;
    }
    let st = m.kept::<LiveState>();
    st.summary = !st.summary;
    super::split::show(m);
    Out::Draw
}

/// Whether the keys go to the screen beside the list now.
pub fn embedded(m: &Model) -> bool {
    m.kept.peek::<LiveState>().is_some_and(|st| st.embedded)
}

/// Whether the selected agent's screen is mirrored, drawn and still
/// streaming, so typing can go into it (canEmbed).
pub fn can_embed(m: &Model) -> bool {
    let Some(i) = m.focused() else {
        return false;
    };
    let key = &m.agents[i].key;
    m.kept
        .peek::<LiveState>()
        .and_then(|st| st.live.as_ref())
        .is_some_and(|l| {
            &l.key == key && l.sh.ready.load(Ordering::SeqCst) && !l.sh.dead.load(Ordering::SeqCst)
        })
        && !m.kept.peek::<LiveState>().is_some_and(|st| st.summary)
}

/// bytes typed into the screen, off the UI thread (sendLive).
fn send_live(m: &Model, b: Vec<u8>) {
    if b.is_empty() {
        return;
    }
    if let Some(tx) = m
        .kept
        .peek::<LiveState>()
        .and_then(|st| st.live.as_ref())
        .and_then(|l| l.keys.as_ref())
    {
        let _ = tx.send(b);
    }
}

/// Every key, first, while the screen has them: each goes to it as the
/// bytes a terminal sends, but ctrl+], which hands the keyboard back, as
/// does the screen going away (embedKey, keys.go:57).
pub fn embed_key(m: &mut Model, k: &Press) -> Out {
    if !embedded(m) {
        return Out::Pass;
    }
    if k.s == "super+k" {
        // The command bar still opens, and takes the keys (cmdbar.go:152).
        m.kept::<LiveState>().embedded = false;
        return Out::Pass;
    }
    if k.s == "ctrl+]" || !can_embed(m) || m.zen().is_some() {
        m.kept::<LiveState>().embedded = false;
        return Out::Draw;
    }
    send_live(m, crate::screens::plugin_ui::embed::key_bytes(k));
    Out::Idle
}

/// tab or → in the list, nothing typed, on an agent whose screen shows:
/// the keys go into it (switchFocus, focusPane on the screen view).
pub fn enter_key(m: &mut Model, k: &Press) -> Out {
    if !matches!(k.s.as_str(), "tab" | "right") || !m.input.is_empty() || m.sel.starts_with('§') {
        return Out::Pass;
    }
    if !can_embed(m) || m.zen().is_some() {
        return Out::Pass;
    }
    m.kept::<LiveState>().embedded = true;
    Out::Draw
}

/// A paste while the screen has the keys goes into it whole, bracketed
/// (embedPaste).
pub fn paste(m: &mut Model, text: &str) -> Out {
    if !embedded(m) {
        return Out::Pass;
    }
    if can_embed(m) {
        send_live(m, crate::screens::plugin_ui::embed::paste_bytes(text));
    }
    Out::Idle
}

/// A click on the screen gives it the keys; one on Agents takes them
/// back (model.go:1664, :1726).
pub fn mouse(m: &mut Model, e: &crate::frame::Mouse) -> Out {
    if e.kind != rush_draw::MouseKind::Down(rush_draw::Button::Left)
        || m.session.is_some()
        || m.sheet.is_some()
        || m.confirm.is_some()
    {
        return Out::Pass;
    }
    let (lw, pw) = super::split::widths(m);
    if pw == 0 {
        return Out::Pass;
    }
    let x = e.x as usize;
    if x > lw + 1 {
        if can_embed(m) && !embedded(m) {
            m.kept::<LiveState>().embedded = true;
            return Out::Draw;
        }
        return Out::Pass;
    }
    if x < lw && embedded(m) {
        m.kept::<LiveState>().embedded = false;
    }
    Out::Pass
}
