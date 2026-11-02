//! The bundled plugin that shows a yap dictation in the message box it's
//! going to: the words come up as yap hears them, the ones it's still
//! guessing with a light passing over them, and a light runs round the
//! box's edge. yap (github.com/0xdeafcafe/yap) is dictation for macOS on
//! Apple's own speech model; it says what it hears on a socket of its own,
//! which the window reads (follow). The plugin is the switch.
//! Ported from internal/bundled/yap/plugin.go:1-116. New in the Rust port
//! (no Go original): the window also tells yap the words on its screen on
//! that socket ([`context_line`]), so yap spells them as they're written,
//! and answers each finished dictation: it put it in its box itself
//! ([`took_line`]), so a yap that waits for that skips its paste, or it
//! left it ([`left_line`]), so yap pastes without waiting longer. While
//! the window is in front with a box taking the words, its context says
//! it `shows` the dictation itself, so a yap that knows shows nothing of
//! its own.

use std::io::{BufRead, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rush_plugin::{Bundle, Manifest};
use serde::{Deserialize, Serialize};

/// The yap plugin's.
pub fn manifest() -> Manifest {
    crate::manifest(
        "yap",
        "voice",
        "Shows a yap dictation in the message box as you speak: the words as yap hears them, \
         a light over the ones it's still guessing, and a light round the box. Does nothing without yap.",
        16,
    )
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), false, crate::run_switch)
}

/// Where a dictation is, as yap says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// hidden, listening, finishing (let go, tidying), done (the text is
    /// in settled and its paste follows) or failed.
    pub phase: String,
    /// words it's sure of
    pub settled: String,
    /// words it may still change
    pub guessing: String,
    /// how loud, 0 to 100
    pub level: i64,
    /// hands free: it listens until fn is tapped
    pub locked: bool,
    /// the bundle id of the app it'll paste into
    pub app: String,
}

impl State {
    /// Whether there's a dictation to show.
    pub fn active(&self) -> bool {
        matches!(self.phase.as_str(), "listening" | "finishing" | "done")
    }
}

/// Where yap says what it hears.
pub fn socket() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("yap").join("live.sock")
}

/// How long follow waits between looks for yap.
const RETRY: Duration = Duration::from_secs(2);
/// How long a line to yap may take to go: a yap that doesn't read ends
/// the connection, as one that doesn't answer would.
const WRITE_LIMIT: Duration = Duration::from_secs(1);

/// What the window tells yap: the app it is and the words on its screen.
#[derive(Serialize)]
struct Hello<'a> {
    context: Context<'a>,
}

#[derive(Serialize)]
struct Context<'a> {
    app: &'a str,
    words: &'a [String],
    /// The window answers a finished dictation: it put it in its box
    /// itself ([`took_line`]) or left it ([`left_line`]); a yap that
    /// knows waits a moment for the answers before it pastes.
    takes: bool,
    /// The window is in front with a box that takes the dictation, and
    /// draws it there: a yap that knows shows nothing of its own while
    /// this app is in front. Left out when it isn't, so the line is as
    /// before.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    shows: bool,
}

/// The line that tells yap the words on the window's screen, most wanted
/// first, the app it is (its bundle id, empty when unknown), and that it
/// takes a finished dictation itself:
/// `{"context":{"app":"…","words":["Redis","rushd"],"takes":true}}` and a
/// newline. shows: the window is in front with a box taking the words,
/// and shows the dictation itself; the line then ends `"shows":true}}`.
pub fn context_line(app: &str, words: &[String], shows: bool) -> String {
    let mut s = serde_json::to_string(&Hello {
        context: Context {
            app,
            words,
            takes: true,
            shows,
        },
    })
    .unwrap_or_default();
    s.push('\n');
    s
}

/// The line that tells yap the window put the dictation it just finished
/// (text, as its done said it) in its box, so yap needn't paste it:
/// `{"took":"…"}` and a newline. A yap that doesn't know it ignores it.
pub fn took_line(text: &str) -> String {
    answer("took", text)
}

/// The line that tells yap the window left the dictation it just finished
/// to its paste (it hasn't the keys, or no box has): `{"left":"…"}` and a
/// newline.
pub fn left_line(text: &str) -> String {
    answer("left", text)
}

fn answer(what: &str, text: &str) -> String {
    let mut m = serde_json::Map::new();
    m.insert(what.into(), text.into());
    let mut s = serde_json::Value::Object(m).to_string();
    s.push('\n');
    s
}

/// A line for yap: one that's said again each time yap is found (the
/// words on screen, latest wins), or one said once, to the yap there now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Say {
    Kept(String),
    Once(String),
}

/// Hands out each state yap reports until stop is sent to or dropped,
/// finding yap again whenever it starts. Losing yap mid-dictation ends the
/// dictation. Each line sent on say goes to yap as it is (a newline put
/// on its end if it hasn't one); the latest kept one is said again each
/// time yap is found, a once one only to the yap there (lost when there's
/// none). A line that can't be written ends that connection, as one that
/// can't be read does.
pub fn follow(stop: mpsc::Receiver<()>, say: mpsc::Receiver<Say>, out: impl FnMut(State)) {
    follow_at(&socket(), stop, say, out);
}

/// What follow's threads share: whether it's stopped, the connection to
/// yap if there is one, and the latest line to say to it.
#[derive(Default)]
struct Link {
    stopped: bool,
    conn: Option<UnixStream>,
    latest: Option<String>,
}

type Shared = std::sync::Arc<std::sync::Mutex<Link>>;

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, Link> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// Writes the latest kept line on the connection.
fn write_line(g: &mut Link) {
    if let Some(line) = g.latest.clone() {
        write_str(g, &line);
    }
}

/// Writes line on the connection; one that fails is shut, which ends its
/// read.
fn write_str(g: &mut Link, line: &str) {
    let failed = match &g.conn {
        Some(c) => (&*c).write_all(line.as_bytes()).is_err(),
        None => false,
    };
    if failed && let Some(c) = g.conn.take() {
        let _ = c.shutdown(std::net::Shutdown::Both);
    }
}

fn follow_at(
    path: &std::path::Path,
    stop: mpsc::Receiver<()>,
    say: mpsc::Receiver<Say>,
    mut out: impl FnMut(State),
) {
    // stop is watched on a thread of its own, which wakes a blocked read
    // by shutting the socket down. Both sides take the lock, so a socket
    // handed over is always seen by a stop that comes after.
    let (stopped_tx, stopped) = mpsc::channel::<()>();
    let current: Shared = Default::default();
    let cur = current.clone();
    std::thread::spawn(move || {
        let _ = stop.recv();
        let mut g = lock(&cur);
        g.stopped = true;
        if let Some(c) = g.conn.take() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        drop(g);
        let _ = stopped_tx.send(());
    });
    // What's to be said is written on a thread of its own too, under the
    // same lock: a connection handed over is told the latest first.
    let cur = current.clone();
    std::thread::spawn(move || {
        for said in say {
            let (kept, mut line) = match said {
                Say::Kept(l) => (true, l),
                Say::Once(l) => (false, l),
            };
            if !line.ends_with('\n') {
                line.push('\n');
            }
            let mut g = lock(&cur);
            if g.stopped {
                return;
            }
            if kept {
                g.latest = Some(line);
                write_line(&mut g);
            } else {
                write_str(&mut g, &line);
            }
        }
    });
    loop {
        if let Ok(c) = UnixStream::connect(path)
            && let Ok(c2) = c.try_clone()
        {
            let _ = c2.set_write_timeout(Some(WRITE_LIMIT));
            {
                let mut g = lock(&current);
                if g.stopped {
                    return;
                }
                g.conn = Some(c2);
                write_line(&mut g);
            }
            read(c, &mut out);
            lock(&current).conn.take();
        }
        match stopped.recv_timeout(RETRY) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
    }
}

fn read(c: UnixStream, out: &mut impl FnMut(State)) {
    let mut r = BufReader::with_capacity(4096, c);
    let mut shown = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // A line past the scanner's 1 MiB ends the read, as in Go.
        if line.len() > 1 << 20 {
            break;
        }
        let Ok(s) = serde_json::from_slice::<State>(line.trim_ascii_end()) else {
            continue;
        };
        shown = s.active();
        out(s);
    }
    if shown {
        out(State {
            phase: "hidden".into(),
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    // plugin_test.go TestFollowReadsYapAndEndsWithIt
    #[test]
    fn follow_reads_yap_and_ends_with_it() {
        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("live.sock");
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            let Ok((mut c, _)) = l.accept() else { return };
            let _ = c.write_all(
                b"{\"phase\":\"listening\",\"settled\":\"hello\",\"guessing\":\"wor\",\"level\":40}\nnot json\n",
            );
            // yap quits mid-dictation.
        });
        let (got_tx, got) = mpsc::channel();
        let (stop_tx, stop) = mpsc::channel();
        let (_say_tx, say) = mpsc::channel();
        let t = std::thread::spawn(move || {
            follow_at(&sock, stop, say, move |s| {
                let _ = got_tx.send(s);
            })
        });
        let want = [
            State {
                phase: "listening".into(),
                settled: "hello".into(),
                guessing: "wor".into(),
                level: 40,
                ..Default::default()
            },
            State {
                phase: "hidden".into(),
                ..Default::default()
            },
        ];
        for w in want {
            let s = got
                .recv_timeout(Duration::from_secs(5))
                .expect("nothing came");
            assert_eq!(s, w);
        }
        drop(stop_tx);
        t.join().unwrap();
    }

    #[test]
    fn context_lines() {
        let cases: &[(&str, &[&str], bool, &str)] = &[
            (
                "com.mitchellh.ghostty",
                &["Redis", "rushd"],
                false,
                "{\"context\":{\"app\":\"com.mitchellh.ghostty\",\"words\":[\"Redis\",\"rushd\"],\"takes\":true}}\n",
            ),
            (
                "com.mitchellh.ghostty",
                &["Redis"],
                true,
                "{\"context\":{\"app\":\"com.mitchellh.ghostty\",\"words\":[\"Redis\"],\"takes\":true,\"shows\":true}}\n",
            ),
            (
                "",
                &[],
                false,
                "{\"context\":{\"app\":\"\",\"words\":[],\"takes\":true}}\n",
            ),
            (
                "",
                &["say \"hi\""],
                false,
                "{\"context\":{\"app\":\"\",\"words\":[\"say \\\"hi\\\"\"],\"takes\":true}}\n",
            ),
        ];
        for (app, words, shows, want) in cases {
            let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
            assert_eq!(
                context_line(app, &words, *shows),
                *want,
                "{app} {words:?} {shows}"
            );
            // Read back as yap reads it: shows there only when said.
            let v: serde_json::Value = serde_json::from_str(want).unwrap();
            assert_eq!(v["context"]["shows"].as_bool().unwrap_or(false), *shows);
        }
        assert_eq!(took_line("say \"hi\""), "{\"took\":\"say \\\"hi\\\"\"}\n");
        assert_eq!(left_line("ship it"), "{\"left\":\"ship it\"}\n");
    }

    // The latest kept line is said as yap is found, each new one as it
    // comes, and a yap found again is told the latest kept one again; a
    // once line goes only to the yap there.
    #[test]
    fn follow_tells_yap_the_words() {
        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("live.sock");
        let l = UnixListener::bind(&sock).unwrap();
        let (heard_tx, heard) = mpsc::channel::<(usize, String)>();
        std::thread::spawn(move || {
            for n in 0..2 {
                let Ok((c, _)) = l.accept() else { return };
                let mut r = BufReader::new(c);
                let lines = if n == 0 { 3 } else { 1 };
                for _ in 0..lines {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let _ = heard_tx.send((n, line));
                }
                // yap quits; the second time it stays till the test ends.
                if n == 1 {
                    let _ = r.read_line(&mut String::new());
                }
            }
        });
        let (stop_tx, stop) = mpsc::channel();
        let (say_tx, say) = mpsc::channel();
        say_tx
            .send(Say::Kept(context_line("", &["Redis".into()], false)))
            .unwrap();
        let t = std::thread::spawn(move || follow_at(&sock, stop, say, |_| {}));
        let next = || {
            heard
                .recv_timeout(Duration::from_secs(10))
                .expect("nothing came")
        };
        assert_eq!(next(), (0, context_line("", &["Redis".into()], false)));
        // Said without its newline: one is put on.
        say_tx
            .send(Say::Kept(
                context_line("", &["rushd".into()], false)
                    .trim_end()
                    .to_string(),
            ))
            .unwrap();
        assert_eq!(next(), (0, context_line("", &["rushd".into()], false)));
        say_tx.send(Say::Once(took_line("ship it"))).unwrap();
        assert_eq!(next(), (0, took_line("ship it")));
        assert_eq!(next(), (1, context_line("", &["rushd".into()], false)));
        drop(stop_tx);
        t.join().unwrap();
    }
}
