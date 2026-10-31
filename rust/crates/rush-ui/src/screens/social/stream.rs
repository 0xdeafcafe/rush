//! The feed's posts as the screens read them: every chirp and reply on the
//! board, oldest first, read off the UI thread and read again when the
//! board changes. After Go's stream.go:27-90 (streamPost, loadStream,
//! streamOf) and :262-322 (nextTint, streamName, streamWho, streamHandle,
//! said). Go polls the board every 2s; here a worker watches its folder
//! (rush_sys::fswait) and reads it only when it changes, so a quiet board
//! costs nothing.

use std::path::PathBuf;
use std::sync::Arc;

use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_room::community::{Author, Feed, Thread};
use rush_sys::fswait::{Stop, Watcher};

use super::ink::{cells, clean_text, handle_tint, truncate};
use crate::model::Model;
use crate::style::*;
use crate::worker::Note;

/// A new post lights the feed this long.
pub const FRESH: Ms = 30_000;

/// One chirp or reply on the board.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Post {
    /// The post's own id: thread id, "/", its place in the thread.
    pub key: String,
    pub id: String,
    pub title: String,
    /// The thread's: the main checkout it was chirped from.
    pub project: String,
    /// The thread's hashtags.
    pub tags: Vec<String>,
    pub author: Author,
    pub text: String,
    pub at: Ms,
    /// When the post a reply answers was made; the sheet sets it.
    pub root_at: Ms,
    pub reply: bool,
    /// The agent an agent's message is to, its @handle; "" for a chirp.
    pub to: String,
    /// The whole of that message, when text sums it up.
    pub full: String,
    /// The sheet shows full: the post is picked.
    pub whole: bool,
}

impl Post {
    /// What the post says: a reply's text, or a post's title; an agent's
    /// message to another says who to.
    pub fn said(&self) -> String {
        if !self.to.is_empty() {
            return format!("→ {} {}", self.to, self.text);
        }
        if self.reply {
            return self.text.clone();
        }
        self.title.clone()
    }

    /// The author as @name, whatever username already carries
    /// (streamHandle).
    pub fn handle(&self) -> String {
        let u = clean_text(&self.author.username());
        format!("@{}", u.strip_prefix('@').unwrap_or(&u))
    }

    pub fn username(&self) -> String {
        self.author.username()
    }
}

/// The board as last read, and whether the feed is on.
#[derive(Default)]
pub struct Stream {
    pub posts: Vec<Post>,
    /// The board's version when last read: a read that finds it the same
    /// changes nothing.
    pub stamp: String,
    /// #feed on|off, once read from the config.
    pub on: Option<bool>,
    /// A worker watches the board.
    watching: Option<Arc<Stop>>,
}

impl Stream {
    /// The feed turned off: the board forgotten, its worker stopped.
    pub fn off(&mut self) {
        if let Some(s) = self.watching.take() {
            s.close();
        }
        self.posts.clear();
        self.stamp.clear();
        self.on = Some(false);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Some(s) = &self.watching {
            s.close();
        }
    }
}

/// The whole board's posts, oldest first (streamOf).
pub fn posts_of(threads: &[Thread]) -> Vec<Post> {
    let mut out = Vec::new();
    for t in threads {
        let tags = t.tags();
        for (i, msg) in t.messages.iter().enumerate() {
            let mut p = Post {
                key: format!("{}/{i}", t.id),
                id: t.id.clone(),
                title: t.title.clone(),
                project: t.author.project.clone(),
                tags: tags.clone(),
                author: msg.author.clone(),
                text: msg.text.clone(),
                full: msg.full.clone(),
                at: msg.at.to_ms(),
                reply: i > 0,
                ..Post::default()
            };
            if let Some(to) = &t.to {
                // Two agents' messages: each is to the other.
                p.to = if msg.author.session_id == to.session_id {
                    t.author.username()
                } else {
                    to.username()
                };
            }
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.at);
    out
}

/// The board in rush's folder, read: its version and its posts; None when
/// it's as stamp said.
fn read(stamp: &str) -> Result<Option<(String, Vec<Post>)>, String> {
    let f = Feed::new();
    let token = f.version().map_err(|e| e.to_string())?;
    if token == stamp {
        return Ok(None);
    }
    let threads = f.list().map_err(|e| e.to_string())?;
    Ok(Some((token, posts_of(&threads))))
}

/// Takes a reading of the board in.
fn take(m: &mut Model, got: Result<Option<(String, Vec<Post>)>, String>) -> crate::frame::Out {
    match got {
        Ok(Some((stamp, posts))) => {
            let s = m.kept::<Stream>();
            s.stamp = stamp;
            s.posts = posts;
            crate::frame::Out::Draw
        }
        _ => crate::frame::Out::Idle,
    }
}

impl Model {
    /// Whether the feed is on, reading the config the first time.
    pub(crate) fn feed_on(&mut self) -> bool {
        if let Some(on) = self.kept::<Stream>().on {
            return on;
        }
        let on = rush_state::Store::load().config.feed;
        self.kept::<Stream>().on = Some(on);
        on
    }

    /// The feed turned on or off in the config (Settings › The feed, or
    /// config.json changed outside rush): docked or taken away at once,
    /// as Go reads the config's Feed every frame (stream.go:55, :115).
    /// Nothing until the feed has been asked about once.
    pub(crate) fn feed_follows(&mut self, on: bool) {
        match self.kept::<Stream>().on {
            Some(was) if was != on => {}
            _ => return,
        }
        self.kept::<Stream>().on = Some(on);
        if on {
            self.load_stream();
        } else {
            self.kept::<Stream>().off();
        }
    }

    /// Reads the board off the UI thread, and has it read again whenever
    /// it changes while the feed is on (loadStream).
    pub(crate) fn load_stream(&mut self) {
        if !self.feed_on() {
            return;
        }
        let stamp = self.kept::<Stream>().stamp.clone();
        self.later(move || read(&stamp), take);
        if self.kept::<Stream>().watching.is_none()
            && let Some(tx) = self.sender()
            && let Ok(stop) = Stop::new()
        {
            let stop = Arc::new(stop);
            self.kept::<Stream>().watching = Some(stop.clone());
            let dir = rush_state::dir().join("community");
            let _ = std::thread::Builder::new()
                .name("rush-feed".into())
                .spawn(move || watch(dir, stop, tx));
        }
    }

    /// The board read again at once: a post just went in.
    pub(crate) fn reload_stream(&mut self) {
        self.kept::<Stream>().stamp.clear();
        self.load_stream();
    }

    /// Who posted p by name: its agent's title now, else the name it
    /// posted under; none for you, whose handle says it (streamName).
    pub(crate) fn stream_name(&self, p: &Post) -> String {
        if p.author.session_id.is_empty() {
            return String::new();
        }
        if let Some(a) = self.agents.iter().find(|a| a.job.id == p.author.session_id) {
            return one_line(&a.display_name);
        }
        one_line(&clean_text(&p.author.name))
    }

    /// A post's author in w cells: its @handle in col, kept whole, then
    /// its name, dim, cut to fit (streamWho).
    pub(crate) fn stream_who(&self, p: &Post, col: Color, w: usize) -> Line {
        let h = p.handle();
        let mut who = line([spb(h.clone(), col)]);
        let n = self.stream_name(p);
        let room = w as isize - cells(&h) as isize - 3;
        if !n.is_empty() && room >= 4 {
            who.spans.push(plain(" "));
            who.spans
                .push(sp(format!("({})", truncate(&n, room as usize, "…")), DIM));
        }
        who
    }
}

/// Watches the board's folder, reading it again on each change and
/// handing what's new to the UI, until stop closes.
fn watch(dir: PathBuf, stop: Arc<Stop>, tx: rush_draw::Sender<Note>) {
    let tx = tx.lane(1);
    let w = Watcher::new();
    let board = dir.join("board.json");
    let mut stamp = String::new();
    loop {
        w.watch(&[&dir, &board]);
        if let Ok(Some((s, posts))) = read(&stamp) {
            stamp = s.clone();
            let ok = tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                take(m, Ok(Some((s, posts))))
            })));
            if !ok {
                return;
            }
        }
        if !w.wait(Some(&stop), None) || stop.is_closed() {
            return;
        }
    }
}

/// The own colour of the first chirp in posts not by username, the run
/// after one of username's; replies sit inside a run (nextTint).
pub fn next_tint(posts: &[Post], username: &str) -> Option<Color> {
    posts
        .iter()
        .find(|p| !p.reply && p.username() != username)
        .map(|p| handle_tint(&p.username()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_room::community::Message;
    use rush_state::Time;

    #[test]
    fn posts_in_time_order() {
        let at = |ms| Time::from_ms(ms);
        let hub = Author {
            session_id: "s-hub".into(),
            name: "Hub".into(),
            ..Author::default()
        };
        let pep = Author {
            session_id: "s-pep".into(),
            name: "Pep".into(),
            ..Author::default()
        };
        let threads = vec![
            Thread {
                id: "a".into(),
                title: "first".into(),
                author: Author {
                    name: "Worker".into(),
                    project: "/src/rush".into(),
                    ..Author::default()
                },
                messages: vec![
                    Message {
                        author: Author {
                            name: "Worker".into(),
                            ..Author::default()
                        },
                        text: "first".into(),
                        at: at(1_000),
                        ..Message::default()
                    },
                    Message {
                        author: Author {
                            name: "You".into(),
                            ..Author::default()
                        },
                        text: "reply".into(),
                        at: at(3_000),
                        ..Message::default()
                    },
                ],
                ..Thread::default()
            },
            Thread {
                id: "d".into(),
                title: "to pep".into(),
                author: hub.clone(),
                to: Some(pep.clone()),
                messages: vec![
                    Message {
                        author: hub.clone(),
                        text: "run the tests".into(),
                        at: at(2_000),
                        ..Message::default()
                    },
                    Message {
                        author: pep.clone(),
                        text: "done".into(),
                        at: at(4_000),
                        ..Message::default()
                    },
                ],
                ..Thread::default()
            },
        ];
        let ps = posts_of(&threads);
        let keys: Vec<&str> = ps.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, ["a/0", "d/0", "a/1", "d/1"]);
        assert!(ps[2].reply && !ps[0].reply);
        assert_eq!(ps[0].project, "/src/rush");
        assert_eq!(
            ps[1].to,
            pep.username(),
            "an agent's message is to the other"
        );
        assert_eq!(ps[3].to, hub.username());
        assert!(ps[1].said().starts_with("→ @"));
        assert_eq!(ps[2].said(), "reply");
        assert_eq!(ps[0].said(), "first");
        assert_eq!(ps[2].handle(), "@you");
    }
}
