//! The push-to-talk helper: rush, run from inside `rush voice.app`. New in
//! the Rust port (no Go original).
//!
//! The main thread runs the app (in the background, no Dock icon), which
//! macOS needs to ask for the microphone and speech recognition and to
//! hand back what the speech model hears. The socket is served on threads
//! of their own, which hand each command to the main thread; everything a
//! dictation holds lives on the main thread. The microphone's sound comes
//! on an audio thread, which feeds the speech model and says how loud it
//! is, at most twenty times a second.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioEngine, AVAudioPCMBuffer,
    AVAudioTime,
};
use objc2_foundation::{NSError, NSLocale, NSOperationQueue};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};
use serde_json::{Map, Value};

use crate::client::encode;
use crate::{Command, Live, audio, split};

/// How long the helper stays with nobody connected before it quits.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// How long finishing the words may take before the helper says done with
/// what it has.
const FINISHING: Duration = Duration::from_secs(3);
/// At most this often a change that's only the loudness.
const LEVEL_EVERY: Duration = Duration::from_millis(50);

const SPEECH_DENIED: &str = "rush voice can't use speech recognition: allow it in System Settings › Privacy & Security › Speech Recognition";
const MIC_DENIED: &str = "rush voice can't use the microphone: allow it in System Settings › Privacy & Security › Microphone";

/// Where a dictation's changes go: the window's end of the socket.
type Out = Arc<Mutex<UnixStream>>;

/// Connections open now, and when one last came or went.
static CONNS: AtomicUsize = AtomicUsize::new(0);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The dictation under way, on the main thread.
    static NOW: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// One dictation and what it holds.
struct Session {
    id: u64,
    conn: u64,
    out: Out,
    device: String,
    live: Live,
    engine: Option<Retained<AVAudioEngine>>,
    request: Option<Retained<SFSpeechAudioBufferRecognitionRequest>>,
    task: Option<Retained<SFSpeechRecognitionTask>>,
    // Kept for as long as its task runs.
    _recognizer: Option<Retained<SFSpeechRecognizer>>,
}

impl Session {
    /// Sends where the dictation is to the window.
    fn say(&self) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(&encode(&self.live));
    }

    /// Stops hearing the microphone and tells the speech model that's all.
    fn quiet(&mut self) {
        if let Some(e) = self.engine.take() {
            // SAFETY: the engine this dictation started, on the main thread.
            unsafe {
                e.stop();
                e.inputNode().removeTapOnBus(0);
            }
        }
        if let Some(r) = &self.request {
            // SAFETY: a live request.
            unsafe { r.endAudio() };
        }
    }

    /// Ends the dictation with this phase and says so.
    fn end(&mut self, phase: &str) {
        self.quiet();
        if let Some(t) = self.task.take() {
            // SAFETY: a live task; one already finished ignores it.
            unsafe { t.cancel() };
        }
        self.live.phase = phase.into();
        self.live.level = 0;
        if phase == "done" {
            let all = format!("{} {}", self.live.settled, self.live.guessing);
            (self.live.settled, self.live.guessing) = (split(&all, true).0, String::new());
        }
        self.say();
    }
}

/// Runs f on the main thread, where the dictation lives.
fn on_main(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

/// Runs f on the dictation with this id, if it's still the one under way;
/// f says whether it goes on (false: it ended, and is let go).
fn step(id: u64, f: impl FnOnce(&mut Session) -> bool) {
    let Some(mut s) = NOW.with(|n| n.borrow_mut().take_if(|s| s.id == id)) else {
        return;
    };
    if f(&mut s) {
        NOW.with(|n| *n.borrow_mut() = Some(s));
    }
}

fn log(kind: &str, f: Map<String, Value>) {
    rush_events::log(kind, f);
}

fn touch() {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

pub(crate) fn run() -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("rush voice must run on the main thread")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // …/voice/rush voice.app/Contents/MacOS/rush-voice: the socket is
    // beside the bundle, wherever the rush that built it keeps its folder
    // (open passes on no RUSH_HOME).
    let voice = exe
        .ancestors()
        .nth(4)
        .map(Path::to_path_buf)
        .ok_or("rush voice isn't in its bundle")?;
    if let Some(home) = voice.parent() {
        // SAFETY: first thing in main, before any thread starts.
        unsafe { std::env::set_var("RUSH_HOME", home) };
    }
    let sock = voice.join("live.sock");
    let Some(listener) = listen(&sock)? else {
        // Another helper already answers there.
        return Ok(());
    };
    touch();
    std::thread::Builder::new()
        .name("rush-voice-serve".into())
        .spawn(move || serve(listener))
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("rush-voice-idle".into())
        .spawn(move || quit_when_idle(sock))
        .map_err(|e| e.to_string())?;
    log("voice-helper", Map::new());

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    ask();
    app.run();
    Ok(())
}

/// Takes the socket: only this user may use it. None when another helper
/// already answers on it.
fn listen(sock: &PathBuf) -> Result<Option<UnixListener>, String> {
    if UnixStream::connect(sock).is_ok() {
        return Ok(None);
    }
    if let Some(d) = sock.parent() {
        crate::bundle::make_dir(d).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(sock);
    let l = UnixListener::bind(sock).map_err(|e| format!("rush voice can't listen: {e}"))?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(Some(l))
}

/// Asks for speech recognition and then the microphone, when the helper
/// starts, so macOS asks once and up front.
fn ask() {
    // SAFETY: class methods with blocks macOS calls once.
    unsafe {
        SFSpeechRecognizer::requestAuthorization(&RcBlock::new(
            |_: SFSpeechRecognizerAuthorizationStatus| {
                on_main(|| {
                    AVAudioApplication::requestRecordPermissionWithCompletionHandler(
                        &RcBlock::new(|_: Bool| {}),
                    );
                });
            },
        ));
    }
}

fn serve(l: UnixListener) {
    for (n, stream) in l.incoming().enumerate() {
        let Ok(stream) = stream else { continue };
        CONNS.fetch_add(1, Ordering::SeqCst);
        touch();
        let conn = n as u64 + 1;
        let _ = std::thread::Builder::new()
            .name("rush-voice-conn".into())
            .spawn(move || {
                read(conn, stream);
                CONNS.fetch_sub(1, Ordering::SeqCst);
                touch();
            });
    }
}

/// Reads one window's commands until it goes.
fn read(conn: u64, stream: UnixStream) {
    let Ok(out) = stream.try_clone() else { return };
    let out: Out = Arc::new(Mutex::new(out));
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(cmd) = serde_json::from_str::<Command>(&line) else {
            continue;
        };
        let out = out.clone();
        on_main(move || command(conn, out, cmd));
    }
    on_main(move || gone(conn));
}

fn quit_when_idle(sock: PathBuf) {
    loop {
        std::thread::sleep(Duration::from_secs(30));
        let last = *LAST.lock().unwrap_or_else(|e| e.into_inner());
        if CONNS.load(Ordering::SeqCst) == 0 && last.is_none_or(|t| t.elapsed() >= IDLE) {
            let _ = std::fs::remove_file(&sock);
            std::process::exit(0);
        }
    }
}

/// The dictation this window started, if it's the one under way.
fn of_conn(conn: u64) -> Option<u64> {
    NOW.with(|n| n.borrow().as_ref().filter(|s| s.conn == conn).map(|s| s.id))
}

fn command(conn: u64, out: Out, cmd: Command) {
    match cmd {
        Command::Start { device } => {
            // One at a time: a new one drops the old.
            if let Some(mut old) = NOW.with(|n| n.borrow_mut().take()) {
                old.end("hidden");
            }
            let id = NEXT.fetch_add(1, Ordering::SeqCst);
            NOW.with(|n| {
                *n.borrow_mut() = Some(Session {
                    id,
                    conn,
                    out,
                    device,
                    live: Live::default(),
                    engine: None,
                    request: None,
                    task: None,
                    _recognizer: None,
                })
            });
            begin(id);
        }
        Command::Stop => {
            let Some(id) = of_conn(conn) else { return };
            step(id, |s| match s.live.phase.as_str() {
                "listening" => {
                    s.quiet();
                    s.live.phase = "finishing".into();
                    s.live.level = 0;
                    s.say();
                    let later = DispatchTime::try_from(FINISHING).unwrap_or(DispatchTime::NOW);
                    let _ = DispatchQueue::main().after(later, move || finished_waiting(id));
                    true
                }
                "finishing" => true,
                // Let go before it was listening: nothing was heard.
                _ => {
                    s.end("done");
                    false
                }
            });
        }
        Command::Cancel => {
            if let Some(id) = of_conn(conn) {
                step(id, |s| {
                    s.end("hidden");
                    false
                });
            }
        }
    }
}

/// The window went: its dictation goes with it, unsaid.
fn gone(conn: u64) {
    if let Some(id) = of_conn(conn) {
        step(id, |s| {
            s.quiet();
            if let Some(t) = s.task.take() {
                // SAFETY: a live task.
                unsafe { t.cancel() };
            }
            false
        });
    }
}

fn finished_waiting(id: u64) {
    step(id, |s| {
        if s.live.phase != "finishing" {
            return true;
        }
        s.end("done");
        false
    });
}

fn fail(id: u64, why: String) {
    log("voice-failed", {
        let mut m = Map::new();
        m.insert("err".into(), Value::String(why.clone()));
        m
    });
    step(id, |s| {
        s.live.error = why;
        s.end("failed");
        false
    });
}

/// Starts listening once speech recognition and the microphone are
/// allowed; asks first if macOS hasn't yet.
fn begin(id: u64) {
    if !NOW.with(|n| n.borrow().as_ref().is_some_and(|s| s.id == id)) {
        return;
    }
    let again = move || on_main(move || begin(id));
    // SAFETY: class methods; the blocks are called once, on any thread.
    unsafe {
        match SFSpeechRecognizer::authorizationStatus() {
            SFSpeechRecognizerAuthorizationStatus::Authorized => {}
            SFSpeechRecognizerAuthorizationStatus::NotDetermined => {
                SFSpeechRecognizer::requestAuthorization(&RcBlock::new(
                    move |_: SFSpeechRecognizerAuthorizationStatus| again(),
                ));
                return;
            }
            _ => return fail(id, SPEECH_DENIED.into()),
        }
        match AVAudioApplication::sharedInstance().recordPermission() {
            AVAudioApplicationRecordPermission::Granted => {}
            AVAudioApplicationRecordPermission::Undetermined => {
                AVAudioApplication::requestRecordPermissionWithCompletionHandler(&RcBlock::new(
                    move |_: Bool| again(),
                ));
                return;
            }
            _ => return fail(id, MIC_DENIED.into()),
        }
    }
    if let Err(e) = hear(id) {
        fail(id, e);
    }
}

/// Starts the microphone and the speech model for the dictation with this
/// id.
fn hear(id: u64) -> Result<(), String> {
    let Some(device) = NOW.with(|n| {
        n.borrow()
            .as_ref()
            .filter(|s| s.id == id)
            .map(|s| s.device.clone())
    }) else {
        return Ok(());
    };
    let locale = NSLocale::currentLocale();
    let lang = locale.localeIdentifier().to_string();
    // SAFETY: AppKit-free Speech and AVFAudio calls on the main thread,
    // with objects this function made.
    unsafe {
        let rec = SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale)
            .ok_or_else(|| {
                format!("Apple's speech model doesn't know this Mac's language ({lang})")
            })?;
        if !rec.supportsOnDeviceRecognition() {
            return Err(format!(
                "this Mac can't turn {lang} speech into text on the Mac itself, and rush won't send your voice anywhere else: turn on Dictation in System Settings › Keyboard so macOS fetches its speech model"
            ));
        }
        rec.setQueue(&NSOperationQueue::mainQueue());
        let req = SFSpeechAudioBufferRecognitionRequest::new();
        req.setRequiresOnDeviceRecognition(true);
        req.setShouldReportPartialResults(true);
        req.setAddsPunctuation(true);

        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        // The chosen microphone, before anything asks the input its format;
        // one that's gone is the Mac's input.
        if !device.is_empty()
            && let Some(d) = audio::find(&device)
            && let Err(st) = audio::hear(input.audioUnit().cast::<c_void>(), d)
        {
            let mut m = Map::new();
            m.insert("device".into(), Value::String(device.clone()));
            m.insert("status".into(), Value::from(st));
            log("voice-device", m);
        }
        let format = input.outputFormatForBus(0);
        if format.sampleRate() <= 0.0 || format.channelCount() == 0 {
            return Err("rush voice can't hear a microphone: is one plugged in?".into());
        }
        let tap = tap(id, req.clone());
        objc2::exception::catch(AssertUnwindSafe(|| {
            input.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                Some(&format),
                RcBlock::as_ptr(&tap),
            )
        }))
        .map_err(|e| {
            format!(
                "rush voice can't hear the microphone: {}",
                e.map(|e| format!("{e:?}")).unwrap_or_default()
            )
        })?;

        let task = rec.recognitionTaskWithRequest_resultHandler(
            &req,
            &RcBlock::new(
                move |res: *mut SFSpeechRecognitionResult, err: *mut NSError| {
                    let heard = res.as_ref().map(|r| {
                        (
                            r.bestTranscription().formattedString().to_string(),
                            r.isFinal(),
                        )
                    });
                    let err = err.as_ref().map(|e| e.localizedDescription().to_string());
                    result(id, heard, err);
                },
            ),
        );
        engine.prepare();
        if let Err(e) = engine.startAndReturnError() {
            input.removeTapOnBus(0);
            task.cancel();
            return Err(format!(
                "rush voice can't start the microphone: {}",
                e.localizedDescription()
            ));
        }
        step(id, |s| {
            s.engine = Some(engine);
            s.request = Some(req);
            s.task = Some(task);
            s._recognizer = Some(rec);
            s.live.phase = "listening".into();
            s.say();
            true
        });
    }
    Ok(())
}

/// The microphone's sound, on the audio thread: fed to the speech model,
/// and how loud it is sent on when it changes, at most every
/// [`LEVEL_EVERY`].
fn tap(
    id: u64,
    req: Retained<SFSpeechAudioBufferRecognitionRequest>,
) -> RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)> {
    let sent = Cell::new(None::<Instant>);
    let shown = Cell::new(-1i64);
    RcBlock::new(
        move |buf: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
            // SAFETY: AVAudioEngine hands a live buffer for this call;
            // appending from the audio thread is what the request is for.
            let level = unsafe {
                let buf = buf.as_ref();
                req.appendAudioPCMBuffer(buf);
                let n = buf.frameLength() as usize;
                let ch = buf.floatChannelData();
                if ch.is_null() || n == 0 {
                    0
                } else {
                    crate::level(std::slice::from_raw_parts((*ch).as_ptr(), n))
                }
            };
            if level == shown.get() || sent.get().is_some_and(|t| t.elapsed() < LEVEL_EVERY) {
                return;
            }
            sent.set(Some(Instant::now()));
            shown.set(level);
            on_main(move || {
                step(id, |s| {
                    if s.live.phase == "listening" && s.live.level != level {
                        s.live.level = level;
                        s.say();
                    }
                    true
                })
            });
        },
    )
}

/// What the speech model heard, or why it stopped, on the main thread.
fn result(id: u64, heard: Option<(String, bool)>, err: Option<String>) {
    step(id, |s| {
        if let Some((text, last)) = heard {
            (s.live.settled, s.live.guessing) = split(&text, last);
            if last {
                s.end("done");
                return false;
            }
            s.say();
            return true;
        }
        let Some(err) = err else { return true };
        if s.live.phase == "finishing" {
            // Often "no speech detected": done with whatever there was.
            s.end("done");
        } else {
            s.live.error = format!("rush voice stopped listening: {err}");
            s.end("failed");
        }
        false
    });
}
