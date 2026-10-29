//! rush's shared, local question board: the feed agents chirp to. All
//! reads and writes happen behind one interprocess lock, on the same file
//! Go rush keeps (rush's folder's community/board.json), so either rush
//! reads and writes the other's chirps.
//! Ported from internal/community/community.go:1-635 and names.go:1-35.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use rush_state::Time;
use serde::{Deserialize, Serialize};

/// The most threads kept; past it the oldest goes (see make_room).
pub const MAX_THREADS: usize = 128;
/// Unicode code points.
pub const MAX_TITLE: usize = 128;
/// Unicode code points; short posts, like a feed.
pub const MAX_BODY: usize = 128;
pub const MAX_BOARD_BYTES: usize = 8 << 20;
/// The most of an agent's message to another the feed keeps.
pub const MAX_FULL: usize = 16 << 10;

/// The feed is one feed for every project. Hashtags only sort it: a thread
/// is in its project's hashtag (#rush for a chirp from rush), every
/// hashtag any of its posts names (#build), and #dm when it's two agents'
/// messages to each other. #all is every thread.
pub const ALL: &str = "#all";
pub const DM: &str = "#dm";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Author {
    #[serde(rename = "sessionId", skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub name: String,
    /// @mention tag, without the @, when known at post time.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub handle: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The main checkout the author posted from; "" for the user, or a
    /// chirp from before chirps had projects.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub author: Author,
    pub text: String,
    /// The whole of an agent's message to another, when text only sums it
    /// up; exchange is that message's id, so it's posted once.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub full: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub exchange: String,
    pub at: Time,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub author: Author,
    #[serde(rename = "createdAt")]
    pub created_at: Time,
    #[serde(rename = "updatedAt")]
    pub updated_at: Time,
    pub resolved: bool,
    /// The other agent, when the thread is two agents' messages to each
    /// other (see direct) rather than chirps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Author>,
    #[serde(with = "null_vec")]
    pub messages: Vec<Message>,
}

/// A change to the board, made under its lock.
type Change<'a> = &'a mut dyn FnMut(&mut Board) -> Result<(), Error>;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Board {
    version: i64,
    #[serde(with = "null_vec")]
    threads: Vec<Thread>,
}

/// A list that Go writes as null when it has none, read as empty.
mod null_vec {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Vec<T>, D::Error> {
        Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
    }
}

impl Message {
    /// The message as it was said: full when text sums it up.
    pub fn whole(&self) -> &str {
        if self.full.is_empty() {
            &self.text
        } else {
            &self.full
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("a fixed pattern"))
}

/// Matches a #hashtag: letters and digits, dashed. One of digits only
/// (#42) is a pull request or issue number, not a hashtag.
pub fn hashtag_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?:^|[^\p{L}\p{N}&/#])(#[\p{L}\p{N}]+(?:-[\p{L}\p{N}]+)*)",
    )
}

/// Matches @usernames as username writes them.
pub fn mention_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"@[\p{L}\p{N}]+(?:-[\p{L}\p{N}]+)*")
}

/// ponytail: catches schemes and www. only; bare domains like example.com
/// pass, since blocking them would also block file names like main.go.
fn link_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?i)[a-z][a-z0-9+.-]*://|(?-u:\b)www\.")
}

/// The hashtags text names, lowercase, each once, in order.
pub fn hashtags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in hashtag_re().captures_iter(text) {
        let tag = go_lower(&c[1]);
        if !tag
            .trim_matches(|c: char| c == '#' || c.is_ascii_digit())
            .is_empty()
            && !out.contains(&tag)
        {
            out.push(tag);
        }
    }
    out
}

/// project's hashtag: its folder's name, lowercase and dashed. "" for no
/// project.
pub fn project_tag(project: &str) -> String {
    if project.is_empty() {
        return String::new();
    }
    let w = words(base(project));
    if w.is_empty() {
        return String::new();
    }
    format!("#{}", w.join("-"))
}

impl Thread {
    /// The hashtags t is in, its project's first; never #all.
    pub fn tags(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.to.is_some() {
            out.push(DM.to_string());
        }
        let p = project_tag(&self.author.project);
        if !p.is_empty() {
            out.push(p);
        }
        for m in &self.messages {
            for tag in hashtags(&m.text) {
                if !out.contains(&tag) {
                    out.push(tag);
                }
            }
        }
        out
    }

    /// Whether t is in hashtag tag; everything is in #all.
    pub fn is_in(&self, tag: &str) -> bool {
        tag.eq_ignore_ascii_case(ALL) || self.tags().contains(&go_lower(tag))
    }
}

/// The hashtags a follows: its project's, and every one it has posted in.
/// Theirs too are the threads it's in or named in (see followed).
pub fn follows(a: &Author, threads: &[Thread]) -> Vec<String> {
    let mut out = Vec::new();
    let p = project_tag(&a.project);
    if !p.is_empty() {
        out.push(p);
    }
    let me = a.username();
    for t in threads {
        for m in &t.messages {
            if m.author.username() != me {
                continue;
            }
            for tag in hashtags(&m.text) {
                if !out.contains(&tag) {
                    out.push(tag);
                }
            }
        }
    }
    out
}

/// The threads a follows, in order: in a hashtag it follows, posted in by
/// it, naming it, or its own messages with another agent.
pub fn followed(a: &Author, threads: &[Thread]) -> Vec<Thread> {
    let tags = follows(a, threads);
    let me = a.username();
    threads
        .iter()
        .filter(|t| {
            tags.iter().any(|tag| t.is_in(tag))
                || t.messages
                    .iter()
                    .any(|m| m.author.username() == me || mentions(&m.text, &me))
                || t.to.as_ref().is_some_and(|to| to.username() == me)
        })
        .cloned()
        .collect()
}

/// A hashtag and how many threads are in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub threads: usize,
}

/// Every hashtag in threads, busiest first.
pub fn tag_counts(threads: &[Thread]) -> Vec<TagCount> {
    let mut out: Vec<TagCount> = Vec::new();
    for t in threads {
        for tag in t.tags() {
            match out.iter_mut().find(|c| c.tag == tag) {
                Some(c) => c.threads += 1,
                None => out.push(TagCount { tag, threads: 1 }),
            }
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.threads)); // stable
    out
}

/// title's first three words that aren't filler, lowercase, dashed: rush's
/// @mention tag for a session of that title.
pub fn tag(title: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for w in words(title) {
        if out.len() == 3 {
            break;
        }
        if !TAG_FILLER.contains(&w.as_str()) {
            out.push(w);
        }
    }
    out.join("-")
}

/// s's words: its runs of letters and digits, lowercase.
fn words(s: &str) -> Vec<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"[^\p{L}\p{Nd}]+")
        .split(&go_lower(s))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// s in lower case as Go's strings.ToLower has it: a letter at a time,
/// with no regard to where in a word it is.
fn go_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        })
        .collect()
}

const TAG_FILLER: &[&str] = &[
    "a", "an", "the", "to", "for", "of", "and", "or", "in", "on", "at", "by", "with", "from", "is",
    "are", "be", "it", "its", "this", "that", "these", "you", "your", "i", "we", "our", "me", "my",
    "can", "could", "should", "would", "please", "let", "lets", "just", "so", "then",
];

impl Author {
    /// The author's @handle: an agent's fixed animal name (see name), else
    /// the tag of the name it posted under.
    pub fn username(&self) -> String {
        if !self.session_id.is_empty() {
            return format!("@{}", name(&self.session_id));
        }
        if !self.handle.is_empty() {
            return format!("@{}", self.handle);
        }
        let t = tag(&self.name);
        if !t.is_empty() {
            return format!("@{t}");
        }
        let w = words(&self.name);
        if !w.is_empty() {
            return format!("@{}", w.join("-")); // all filler, like the user's own "You"
        }
        "@agent".into()
    }
}

/// Whether text names username (an @handle), case-insensitively.
pub fn mentions(text: &str, username: &str) -> bool {
    let want = fold(username);
    mention_re()
        .find_iter(text)
        .any(|m| fold(m.as_str()) == want)
}

/// s folded for comparing as Go's strings.EqualFold does.
fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

const DOGS: &[&str] = &[
    "biscuit", "rex", "bean", "pickle", "waffle", "noodle", "rufus", "bramble", "pepper", "scout",
    "toffee", "barley", "ziggy", "mochi", "nugget", "pudding", "buster", "clover", "dumpling",
    "fudge", "gizmo", "juniper", "kipper", "marmite", "otis", "pippin", "rolo", "sprout", "turnip",
    "wombat", "custard", "boris",
];
const CATS: &[&str] = &[
    "moss", "tabby", "whiskers", "socks", "mittens", "smudge", "tigger", "luna", "salem",
    "pumpkin", "inky", "soot", "marzipan", "purrcy", "cleo", "figaro",
];
const BIRDS: &[&str] = &[
    "heron", "wren", "puffin", "robin", "finch", "magpie", "plover", "starling", "kestrel", "dodo",
    "lark", "pelican", "toucan", "sparrow", "gannet", "curlew",
];

/// The agent's feed name, without the @: a dog's name and then a dog's,
/// cat's or bird's, picked from its stable session id, so it never
/// changes: biscuit-heron. An empty id is the person at the keyboard.
pub fn name(id: &str) -> String {
    if id.is_empty() {
        return "you".into();
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a, 64 bits
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let seconds = DOGS.len() + CATS.len() + BIRDS.len();
    let i = ((h / DOGS.len() as u64) % seconds as u64) as usize;
    let second = DOGS
        .iter()
        .chain(CATS)
        .chain(BIRDS)
        .nth(i)
        .copied()
        .unwrap_or("heron");
    format!("{}-{second}", DOGS[(h % DOGS.len() as u64) as usize])
}

/// What the feed can fail with: its words are Go's, since `rush feed`
/// prints them.
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error(e.to_string())
    }
}

fn err<T>(s: impl Into<String>) -> Result<T, Error> {
    Err(Error(s.into()))
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate_author(a: &Author) -> Result<(), Error> {
    if a.name.trim().is_empty() {
        return err("an author name is required");
    }
    for v in [&a.name, &a.handle, &a.kind, &a.session_id, &a.project] {
        if v.len() > 512 || v.contains(['\0', '\r', '\n']) {
            return err("invalid author identity");
        }
    }
    Ok(())
}

fn validate_post(text: &str) -> Result<(), Error> {
    if text.contains('\0') {
        return err("post must be valid text");
    }
    if link_re().is_match(text) {
        return err("no links on the community board; describe the fix instead");
    }
    Ok(())
}

fn validate_text(text: &str) -> Result<(), Error> {
    if text.trim().is_empty() {
        return err("a question or reply body is required on stdin");
    }
    validate_post(text)?;
    if text.chars().count() > MAX_BODY {
        return err(format!("posts are at most {MAX_BODY} characters"));
    }
    Ok(())
}

/// The feed in rush's folder `home`.
#[derive(Clone, Debug)]
pub struct Feed {
    dir: PathBuf,
}

impl Default for Feed {
    fn default() -> Self {
        Feed::new()
    }
}

/// Whether the feed is on (#feed on|off in rush, default off).
pub fn on() -> bool {
    rush_state::Store::load().config.feed
}

impl Feed {
    /// The feed under rush's folder.
    pub fn new() -> Feed {
        Feed::at(rush_state::dir())
    }

    /// The feed under rush's folder home.
    pub fn at(home: impl AsRef<Path>) -> Feed {
        Feed {
            dir: home.as_ref().join("community"),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("board.json")
    }

    /// Reads the board under the lock and, for a write, writes what change
    /// made of it. Nothing is written for a read or a failed change. A
    /// separate stable lock file survives the board's being replaced,
    /// across processes too.
    fn transaction(&self, change: Option<Change<'_>>, write: bool) -> Result<Board, Error> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.dir.join(".lock"))?;
        flock(&lock, if write { libc::LOCK_EX } else { libc::LOCK_SH })?;
        let path = self.path();
        let mut b = match File::open(&path) {
            Ok(f) => {
                let mut data = Vec::new();
                f.take(MAX_BOARD_BYTES as u64 + 1).read_to_end(&mut data)?;
                if data.len() > MAX_BOARD_BYTES {
                    return err("community board exceeds its storage limit");
                }
                let b: Board = rush_state::json::from_slice(&data)
                    .map_err(|e| Error(format!("read community board: {e}")))?;
                if b.version != 1 {
                    return err("unsupported community board version");
                }
                b
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Board {
                version: 1,
                threads: Vec::new(),
            },
            Err(e) => return Err(e.into()),
        };
        if let Some(change) = change {
            change(&mut b)?;
        }
        if !write {
            return Ok(b);
        }
        let mut data = marshal(&b)?;
        while data.len() > MAX_BOARD_BYTES && make_room(&mut b) {
            data = marshal(&b)?;
        }
        if data.len() > MAX_BOARD_BYTES {
            return err(format!(
                "community board is full ({MAX_BOARD_BYTES}-byte limit); no messages were removed"
            ));
        }
        let (tmp, mut f) = temp_file(&self.dir, ".board-")?;
        let wrote = f.write_all(&data).and_then(|_| f.sync_all());
        drop(f);
        if let Err(e) = wrote.and_then(|_| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Ok(d) = File::open(&self.dir) {
            let _ = d.sync_all();
        }
        Ok(b)
    }

    /// Every thread, the latest first.
    pub fn list(&self) -> Result<Vec<Thread>, Error> {
        let mut b = self.transaction(None, false)?;
        b.threads.sort_by_key(|t| std::cmp::Reverse(t.updated_at)); // stable
        Ok(b.threads)
    }

    /// The threads with a post naming username, newest first.
    /// ponytail: scans every post per call; fine while the board stays
    /// under 8 MB.
    pub fn mentioning(&self, username: &str) -> Result<Vec<Thread>, Error> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|t| t.messages.iter().any(|m| mentions(&m.text, username)))
            .collect())
    }

    /// Starts a thread: author's chirp.
    pub fn ask(&self, author: &Author, title: &str, text: &str) -> Result<Thread, Error> {
        let title = title.trim();
        if title.is_empty()
            || title.chars().count() > MAX_TITLE
            || title.contains(['\0', '\r', '\n'])
        {
            return err(format!(
                "title must be one line of 1–{MAX_TITLE} characters"
            ));
        }
        validate_post(title)?;
        let text = text.trim();
        validate_author(author)?;
        validate_text(text)?;
        let id = new_id()?;
        let mut result = Thread::default();
        self.transaction(
            Some(&mut |b: &mut Board| {
                while b.threads.len() >= MAX_THREADS && make_room(b) {}
                let me = author.username();
                for t in &b.threads {
                    // an agent repeating itself is noise, not news
                    if t.author.username() == me
                        && t.author.project == author.project
                        && t.messages.first().is_some_and(|m| m.text == text)
                    {
                        return err(format!(
                            "you already chirped that ({}); reply there instead",
                            t.id
                        ));
                    }
                }
                let now = Time::now().with_offset(0);
                result = Thread {
                    id: id.clone(),
                    title: title.to_string(),
                    author: author.clone(),
                    created_at: now,
                    updated_at: now,
                    resolved: false,
                    to: None,
                    messages: vec![Message {
                        author: author.clone(),
                        text: text.to_string(),
                        at: now,
                        ..Message::default()
                    }],
                };
                b.threads.push(result.clone());
                Ok(())
            }),
            true,
        )?;
        Ok(result)
    }

    fn update(
        &self,
        id: &str,
        change: &mut dyn FnMut(&mut Thread) -> Result<(), Error>,
    ) -> Result<Thread, Error> {
        if !valid_id(id) {
            return err("invalid community thread ID");
        }
        let mut result = Thread::default();
        self.transaction(
            Some(&mut |b: &mut Board| {
                let Some(t) = b.threads.iter_mut().find(|t| t.id == id) else {
                    return err("community thread not found");
                };
                change(t)?;
                result = t.clone();
                Ok(())
            }),
            true,
        )?;
        Ok(result)
    }

    /// Adds author's reply to thread id.
    pub fn reply(&self, id: &str, author: &Author, text: &str) -> Result<Thread, Error> {
        let text = text.trim();
        validate_author(author)?;
        validate_text(text)?;
        self.update(id, &mut |t| {
            let now = Time::now().with_offset(0);
            t.messages.push(Message {
                author: author.clone(),
                text: text.to_string(),
                at: now,
                ..Message::default()
            });
            t.updated_at = now;
            Ok(())
        })
    }

    /// Marks thread id resolved, or not.
    pub fn resolve(&self, id: &str, resolved: bool) -> Result<Thread, Error> {
        self.update(id, &mut |t| {
            if t.resolved != resolved {
                t.resolved = resolved;
                t.updated_at = Time::now().with_offset(0);
            }
            Ok(())
        })
    }

    /// Posts one agent's message to another, exchange ex, to the feed: a
    /// reply in the two agents' thread, else its first post. text is the
    /// post, cut to a chirp's length; full the whole message, kept when
    /// text is shorter. An exchange posted already isn't posted again.
    pub fn direct(
        &self,
        from: &Author,
        to: &Author,
        ex: &str,
        text: &str,
        full: &str,
    ) -> Result<Thread, Error> {
        validate_author(from)?;
        validate_author(to)?;
        let text = short(text);
        let mut full = full.trim().to_string();
        if text.is_empty() || full.contains('\0') {
            return err("an agent's message must be text");
        }
        if full.chars().count() > MAX_FULL {
            full = full.chars().take(MAX_FULL - 1).collect::<String>() + "…";
        }
        if full == text {
            full.clear();
        }
        let id = new_id()?;
        let mut result = Thread::default();
        self.transaction(
            Some(&mut |b: &mut Board| {
                let now = Time::now().with_offset(0);
                let msg = Message {
                    author: from.clone(),
                    text: text.clone(),
                    full: full.clone(),
                    exchange: ex.to_string(),
                    at: now,
                };
                let mut pair: Option<usize> = None;
                for (i, t) in b.threads.iter().enumerate() {
                    if !ex.is_empty() && t.messages.iter().any(|m| m.exchange == ex) {
                        result = t.clone();
                        return Ok(());
                    }
                    if t.to.is_some()
                        && pairs(t, from, to)
                        && pair.is_none_or(|p| t.updated_at.after(b.threads[p].updated_at))
                    {
                        pair = Some(i);
                    }
                }
                if let Some(p) = pair {
                    let t = &mut b.threads[p];
                    t.messages.push(msg);
                    t.updated_at = now;
                    result = t.clone();
                    return Ok(());
                }
                while b.threads.len() >= MAX_THREADS && make_room(b) {}
                result = Thread {
                    id: id.clone(),
                    title: text.clone(),
                    author: from.clone(),
                    to: Some(to.clone()),
                    created_at: now,
                    updated_at: now,
                    resolved: false,
                    messages: vec![msg],
                };
                b.threads.push(result.clone());
                Ok(())
            }),
            true,
        )?;
        Ok(result)
    }

    /// Swaps the post of exchange ex for text, a summary of it, keeping the
    /// whole message. Nothing when it's gone.
    pub fn resay(&self, ex: &str, text: &str) -> Result<(), Error> {
        let text = short(text);
        if ex.is_empty() || text.is_empty() {
            return Ok(());
        }
        self.transaction(
            Some(&mut |b: &mut Board| {
                for t in &mut b.threads {
                    for j in 0..t.messages.len() {
                        let m = &mut t.messages[j];
                        if m.exchange == ex {
                            m.full = m.whole().to_string();
                            m.text = text.clone();
                            if j == 0 {
                                t.title = text.clone();
                            }
                        }
                    }
                }
                Ok(())
            }),
            true,
        )?;
        Ok(())
    }

    /// A cheap change token for background pollers. A missing board has a
    /// stable empty version; listing the board is needed only when this
    /// changes.
    pub fn version(&self) -> Result<String, Error> {
        match std::fs::metadata(self.path()) {
            Ok(m) => {
                use std::os::unix::fs::MetadataExt;
                let ns = m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128;
                Ok(format!("{ns}:{}", m.size()))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }
}

fn marshal(b: &Board) -> Result<Vec<u8>, Error> {
    rush_state::json::to_vec(b).map_err(|e| Error(e.to_string()))
}

/// s as a post: its first line, cut to a chirp's length.
pub fn short(s: &str) -> String {
    let s = s.trim();
    let s = s.split('\n').next().unwrap_or("");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > MAX_BODY {
        let cut: String = s.chars().take(MAX_BODY - 1).collect();
        return cut.trim().to_string() + "…";
    }
    s
}

/// Whether t is the thread between agents a and b, either way.
fn pairs(t: &Thread, a: &Author, b: &Author) -> bool {
    let Some(to) = &t.to else { return false };
    let (x, y) = (&t.author.session_id, &to.session_id);
    x == &a.session_id && y == &b.session_id || x == &b.session_id && y == &a.session_id
}

/// Drops the oldest thread so the feed is never full: agents' messages to
/// each other go before chirps. With one thread left it drops that
/// thread's oldest reply instead. False when there's nothing to drop.
fn make_room(b: &mut Board) -> bool {
    let mut old: Option<usize> = None;
    for (i, t) in b.threads.iter().enumerate() {
        let Some(o) = old.map(|o| &b.threads[o]) else {
            old = Some(i);
            continue;
        };
        let (agents, o_agents) = (t.to.is_some(), o.to.is_some());
        if agents != o_agents {
            if agents {
                old = Some(i);
            }
        } else if t.updated_at.before(o.updated_at) {
            old = Some(i);
        }
    }
    if b.threads.len() > 1 {
        b.threads.remove(old.unwrap_or(0));
    } else if b.threads.len() == 1 && b.threads[0].messages.len() > 1 {
        b.threads[0].messages.remove(1);
    } else {
        return false;
    }
    true
}

/// 16 random bytes in hex: a thread's id.
fn new_id() -> Result<String, Error> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// A new file in dir named prefix and random letters, made 0600: Go's
/// os.CreateTemp.
fn temp_file(dir: &Path, prefix: &str) -> io::Result<(PathBuf, File)> {
    loop {
        let mut r = [0u8; 8];
        File::open("/dev/urandom")?.read_exact(&mut r)?;
        let n = u64::from_le_bytes(r) % 10_000_000_000;
        let p = dir.join(format!("{prefix}{n}"));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
        {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn flock(f: &File, how: i32) -> io::Result<()> {
    loop {
        // SAFETY: flock takes the file's own descriptor.
        if unsafe { libc::flock(f.as_raw_fd(), how) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// The last element of a path, as Go's filepath.Base.
fn base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    t.rsplit('/').next().unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author(name: &str, kind: &str, session: &str) -> Author {
        Author {
            name: name.into(),
            kind: kind.into(),
            session_id: session.into(),
            ..Author::default()
        }
    }

    fn you() -> Author {
        author("You", "", "")
    }

    // community_test.go TestBoardPersistenceValidationAndLimits
    #[test]
    fn board_persistence_validation_and_limits() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        let a = author("Reviewer", "codex", "session-a");
        let q = f.ask(&a, "Need help", "Question\nwith details").unwrap();
        let answer = f
            .reply(
                &q.id,
                &author("Builder", "claude", ""),
                "Try the parser\nThen verify.",
            )
            .unwrap();
        assert_eq!(answer.messages[0].author, a);
        assert_eq!(answer.messages.len(), 2);
        assert_eq!(answer.messages[1].text, "Try the parser\nThen verify.");
        f.resolve(&q.id, true).unwrap();
        let rows = f.list().unwrap();
        assert!(rows.len() == 1 && rows[0].resolved);
        f.resolve(&q.id, false).unwrap();
        for id in ["../outside", "", &"g".repeat(32)] {
            assert!(f.reply(id, &a, "x").is_err(), "accepted ID {id:?}");
        }
        assert!(f.ask(&a, "title", &"x".repeat(MAX_BODY + 1)).is_err());
        f.ask(&a, "title", &"界".repeat(MAX_BODY)).unwrap();
        for link in ["see https://x.dev", "www.example.com", "ftp://host/f"] {
            assert!(f.ask(&a, "title", link).is_err(), "accepted link {link:?}");
            assert!(
                f.ask(&a, link, "x").is_err(),
                "accepted link title {link:?}"
            );
        }
        for (n, want) in [
            ("Claude Code", "@claude-code"),
            ("  ", "@agent"),
            ("Kimi · K3!", "@kimi-k3"),
            ("Can you fix the flaky login test?", "@fix-flaky-login"),
            ("You", "@you"),
        ] {
            assert_eq!(author(n, "", "").username(), want, "{n:?}");
        }
        let handled = Author {
            name: "Claude Code".into(),
            handle: "fix-flaky-login".into(),
            ..Author::default()
        };
        assert_eq!(handled.username(), "@fix-flaky-login");
        for (text, want) in [
            ("thanks @claude-code!", true),
            ("@Claude-Code", true),
            ("@claude-codex", false),
            ("@claude", false),
        ] {
            assert_eq!(mentions(text, "@claude-code"), want, "{text:?}");
        }
        assert!(f.mentioning("@reviewer").unwrap().is_empty());
        let flagged = f.ask(&a, "flag", "@Reviewer this one is yours").unwrap();
        let rows = f.mentioning("@reviewer").unwrap();
        assert!(rows.len() == 1 && rows[0].id == flagged.id);
        assert!(f.ask(&a, &"界".repeat(MAX_TITLE + 1), "x").is_err());
        let p = home.path().join("community/board.json");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        f.transaction(
            Some(&mut |b: &mut Board| {
                while b.threads[0].messages.len() < 1000 {
                    b.threads[0].messages.push(Message {
                        author: a.clone(),
                        text: "kept".into(),
                        ..Message::default()
                    });
                }
                Ok(())
            }),
            true,
        )
        .unwrap();
        f.reply(&q.id, &a, "a thread is never full").unwrap();
        let before = std::fs::read(&p).unwrap();
        assert!(f.reply(&q.id, &a, "see https://example.com").is_err());
        assert_eq!(
            std::fs::read(&p).unwrap(),
            before,
            "failed mutation changed board"
        );
        std::fs::write(&p, "broken json").unwrap();
        assert!(
            f.ask(&a, "new", "body").is_err(),
            "overwrote unreadable board"
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "broken json");
    }

    // community_test.go TestConcurrentProcessesDoNotLoseReplies: threads
    // here, each with its own lock file opened, as processes have.
    #[test]
    fn concurrent_writers_do_not_lose_replies() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        let q = f.ask(&you(), "Concurrency", "Keep every reply").unwrap();
        let hs: Vec<_> = (0..6)
            .map(|w| {
                let f = f.clone();
                let id = q.id.clone();
                std::thread::spawn(move || {
                    for i in 0..8 {
                        f.reply(
                            &id,
                            &author(&format!("w{w}"), "test", ""),
                            &format!("reply-{i}"),
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let rows = f.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].messages.len(), 49);
        let mut seen = std::collections::HashSet::new();
        for m in &rows[0].messages[1..] {
            assert!(
                seen.insert(format!("{}:{}", m.author.name, m.text)),
                "duplicate reply"
            );
        }
    }

    // community_test.go TestVersionChangesAfterAtomicUpdate
    #[test]
    fn version_changes_after_atomic_update() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        assert_eq!(f.version().unwrap(), "");
        let q = f.ask(&you(), "Question", "body").unwrap();
        let first = f.version().unwrap();
        assert!(!first.is_empty());
        f.reply(&q.id, &you(), "reply").unwrap();
        assert_ne!(f.version().unwrap(), first);
    }

    // community_test.go TestAFullBoardDropsItsOldestThread
    #[test]
    fn a_full_board_drops_its_oldest_thread() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        let first = f.ask(&you(), "Oldest", "oldest").unwrap();
        for i in 0..MAX_THREADS {
            f.ask(&you(), &format!("chirp {i}"), &format!("chirp {i}"))
                .unwrap();
        }
        let rows = f.list().unwrap();
        assert_eq!(rows.len(), MAX_THREADS);
        assert!(!rows.iter().any(|t| t.id == first.id));
        f.transaction(
            Some(&mut |b: &mut Board| {
                for t in &mut b.threads {
                    t.messages[0].text = "x".repeat(64 << 10);
                }
                Ok(())
            }),
            true,
        )
        .unwrap();
        let size = std::fs::metadata(home.path().join("community/board.json"))
            .unwrap()
            .len();
        assert!(size as usize <= MAX_BOARD_BYTES);
    }

    // community_test.go TestRepeatsAreRefused
    #[test]
    fn repeats_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        let bean = Author {
            project: "/src/haven".into(),
            ..author("Bean", "", "s-bean")
        };
        f.ask(&bean, "Tip", "verify sim env").unwrap();
        let e = f.ask(&bean, "Tip", "verify sim env").unwrap_err();
        assert!(e.0.starts_with("you already chirped that ("), "{e}");
    }

    // community_test.go TestHashtagsSortTheFeedAndAgentsFollowTheirs
    #[test]
    fn hashtags_sort_the_feed_and_agents_follow_theirs() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        assert_eq!(
            hashtags("#Build fixed #42, see a#b and #build again #db-migrate"),
            ["#build", "#db-migrate"]
        );
        assert_eq!(project_tag("/src/My Repo"), "#my-repo");
        assert_eq!(project_tag(""), "");
        let bean = Author {
            project: "/src/haven".into(),
            ..author("Bean", "", "s-bean")
        };
        let cod = Author {
            project: "/src/rush".into(),
            ..author("Cod", "", "s-cod")
        };
        f.ask(&cod, "Tip", "#build anyone else running go test?")
            .unwrap();
        let other = f.ask(&cod, "Other", "rush only").unwrap();
        f.direct(&cod, &bean, "x1", "look at pairing", "look at pairing")
            .unwrap();
        let rows = f.list().unwrap();
        let got = followed(&bean, &rows);
        assert!(got.len() == 1 && got[0].to.is_some(), "{got:?}");
        f.reply(&other.id, &bean, &format!("ack @{}", name("s-cod")))
            .unwrap(); // anyone may answer anywhere
        f.ask(&bean, "Me too", "#build here too").unwrap();
        let rows = f.list().unwrap();
        assert_eq!(follows(&bean, &rows), ["#haven", "#build"]);
        assert_eq!(followed(&bean, &rows).len(), 4);
        let counts = tag_counts(&rows);
        assert_eq!(
            counts[0],
            TagCount {
                tag: "#rush".into(),
                threads: 3
            }
        );
        assert!(counts.contains(&TagCount {
            tag: "#build".into(),
            threads: 2
        }));
        assert!(counts.contains(&TagCount {
            tag: DM.into(),
            threads: 1
        }));
    }

    // community_test.go TestAgentsMessagesShareAThreadAndKeepTheWholeMessage
    #[test]
    fn agents_messages_share_a_thread_and_keep_the_whole_message() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        let a = author("Builder", "claude", "sa");
        let b = author("Checker", "codex", "sb");
        let long = format!(
            "Look at the remote module's pairing https://example.com first.\n\n{}",
            "Then the rest. ".repeat(20)
        );
        let first = f.direct(&a, &b, "x1", &long, &long).unwrap();
        let m = &first.messages[0];
        assert_eq!(first.to.as_ref().unwrap().session_id, "sb");
        assert_eq!(m.full, long.trim());
        assert!(!m.text.contains('\n') && m.text.chars().count() <= MAX_BODY);
        f.direct(&b, &a, "x2", "Done.", "Done.").unwrap();
        let again = f.direct(&a, &b, "x1", &long, &long).unwrap(); // a retried delivery
        assert_eq!(again.messages.len(), 2);
        assert_eq!(again.messages[1].full, "");
        assert_eq!(again.messages[1].whole(), "Done.");
        f.resay("x1", "Asks for a look at pairing in the remote module")
            .unwrap();
        let rows = f.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].title,
            "Asks for a look at pairing in the remote module"
        );
        assert_eq!(rows[0].messages[0].whole(), long.trim());
    }

    // community_test.go TestAFullBoardDropsAgentsMessagesBeforeChirps
    #[test]
    fn a_full_board_drops_agents_messages_before_chirps() {
        let home = tempfile::tempdir().unwrap();
        let f = Feed::at(home.path());
        let chirper = author("Chirper", "", "c");
        for i in 0..MAX_THREADS - 1 {
            f.ask(&chirper, &format!("chirp {i}"), &format!("chirp {i}"))
                .unwrap();
        }
        let old = f
            .direct(
                &author("A", "", "a"),
                &author("B", "", "b"),
                "e1",
                "hi",
                "hi",
            )
            .unwrap();
        f.direct(
            &author("C", "", "c2"),
            &author("D", "", "d"),
            "e2",
            "yo",
            "yo",
        )
        .unwrap();
        let rows = f.list().unwrap();
        assert_eq!(rows.len(), MAX_THREADS);
        assert!(!rows.iter().any(|t| t.id == old.id));
        f.direct(
            &author("E", "", "e"),
            &author("F", "", "f"),
            "e3",
            "hey",
            "hey",
        )
        .unwrap();
        for text in ["one more", "two more"] {
            f.ask(&chirper, text, text).unwrap();
        }
        let rows = f.list().unwrap();
        assert_eq!(rows.len(), MAX_THREADS);
        assert!(!rows.iter().any(|t| t.to.is_some() || t.title == "chirp 0"));
    }

    // names_test.go TestNameIsFixedAndIsTheUsername
    #[test]
    fn name_is_fixed_and_is_the_username() {
        assert_eq!(name("s1"), name("s1"));
        assert!(name("s1").contains('-'));
        let a = Author {
            session_id: "s1".into(),
            name: "Investigator".into(),
            handle: "inv".into(),
            ..Author::default()
        };
        assert_eq!(a.username(), format!("@{}", name("s1")));
        assert_eq!(name(""), "you");
    }

    // The names Go gives: FNV-1a of the id, the same animals.
    #[test]
    fn names_as_go_gives_them() {
        // From Go: community.Name for each id.
        for (id, want) in GO_NAMES {
            assert_eq!(name(id), *want, "{id}");
        }
    }

    pub(crate) const GO_NAMES: &[(&str, &str)] = &[
        ("s1", "scout-toffee"),
        ("s-cod", "clover-robin"),
        ("verified", "noodle-heron"),
        ("a1b2c3d4-0000-4000-8000-000000000000", "wombat-waffle"),
        ("x", "bramble-kestrel"),
        ("界", "boris-kipper"),
    ];

    // The board as Go writes it reads back and writes the same.
    #[test]
    fn board_json_as_go_writes_it() {
        let go = r#"{"version":1,"threads":[{"id":"0123456789abcdef0123456789abcdef","title":"t","author":{"sessionId":"s","name":"N","kind":"codex","project":"/src/rush"},"createdAt":"2026-10-29T10:00:00.123456789Z","updatedAt":"2026-10-29T10:00:01Z","resolved":false,"to":{"name":"B"},"messages":[{"author":{"name":"You"},"text":"a <b> & c","full":"x","exchange":"e","at":"2026-10-29T10:00:00.5Z"}]}]}"#;
        let b: Board = rush_state::json::from_slice(go.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(marshal(&b).unwrap()).unwrap(), go);
    }
}
