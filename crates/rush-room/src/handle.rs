//! How a session rush runs is @named. What a post, a DM or a message
//! keeps is the session's id, written @{id}, so a mention keeps pointing
//! at the session when its title changes. What's drawn is its title now,
//! kebab-cased: "Terminal Decline: Shell Autocomplete Ideas" is
//! @terminal-decline-shell-autocomplete-ideas. Two sessions of one title
//! tell themselves apart by a short id suffix (@fix-login-a1b2); one with
//! no title yet goes by session-<id>. Text from before keeps the animal
//! handles rush used to give (@biscuit-heron); they come from the id, so
//! they map back to their session and draw as its title too.
//!
//! Everything that names a session goes through here: the feed and its
//! CLI, agents' messages on the feed, the list, the conversation and the
//! @ picker in the message box.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use crate::community::{Author, mention_re, words};

/// The most cells a handle is drawn in, its @ and its … included. The
/// feed shows a hovered or picked post's handles whole.
pub const WIDTH: usize = 24;

/// title kebab-cased: its runs of letters and digits, lowercase, joined
/// by dashes. Punctuation and emoji go; "" when nothing is left.
pub fn kebab(title: &str) -> String {
    words(title).join("-")
}

/// id's letters and digits, lowercase: what a suffix and a plain name
/// are cut from.
fn alnum(id: &str) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// What a session with no title yet is called, from its id:
/// session-a1b2c3d4. An empty id is the person at the keyboard.
pub fn plain(id: &str) -> String {
    if id.is_empty() {
        return "you".into();
    }
    let a: String = alnum(id).chars().take(8).collect();
    if a.is_empty() {
        return "session".into();
    }
    format!("session-{a}")
}

/// A mention of session id as it's kept: @{id}.
pub fn stored(id: &str) -> String {
    format!("@{{{id}}}")
}

/// The id a kept mention names, with or without its @: "@{a1b2}" is
/// a1b2.
pub fn stored_id(tag: &str) -> Option<&str> {
    let t = tag.strip_prefix('@').unwrap_or(tag);
    let id = t.strip_prefix('{')?.strip_suffix('}')?;
    (!id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    .then_some(id)
}

/// h cut to WIDTH cells with …, when it's longer.
pub fn trim(h: &str) -> String {
    cut(h, WIDTH)
}

/// h cut to w cells with …, when it's longer.
pub fn cut(h: &str, w: usize) -> String {
    if rush_core::text::width(h) <= w {
        return h.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in h.chars() {
        let cw = rush_core::text::width(c.encode_utf8(&mut [0; 4]));
        if used + cw + 1 > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// One session as Names knows it.
#[derive(Clone, Debug, Default)]
struct Entry {
    id: String,
    /// Its title kebab-cased, else its plain name.
    base: String,
    /// What it's drawn as: base, with a suffix when another has it too.
    handle: String,
}

/// The sessions there are, by id and title: what turns a kept @{id} into
/// the handle drawn, and a typed handle into the id kept.
#[derive(Clone, Debug, Default)]
pub struct Names {
    list: Vec<Entry>,
    /// Each base, and the sessions that have it.
    by_base: HashMap<String, Vec<usize>>,
    /// Each session's place in list, by id.
    by_id: HashMap<String, usize>,
    /// Each animal handle rush used to give, and its session.
    legacy: HashMap<String, usize>,
}

/// A session's base: its title kebab-cased, else its plain name.
fn base(id: &str, title: &str) -> String {
    let k = kebab(title);
    if k.is_empty() { plain(id) } else { k }
}

impl Names {
    /// Names for sessions, each its id and its title now.
    pub fn new<I, A, B>(sessions: I) -> Names
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<String>,
        B: AsRef<str>,
    {
        let mut n = Names::default();
        for (id, title) in sessions {
            let id: String = id.into();
            if id.is_empty() || n.by_id.contains_key(&id) {
                continue;
            }
            let b = base(&id, title.as_ref());
            let i = n.list.len();
            n.by_base.entry(b.clone()).or_default().push(i);
            n.by_id.insert(id.clone(), i);
            n.legacy.insert(legacy_name(&id), i);
            n.list.push(Entry {
                id,
                base: b,
                handle: String::new(),
            });
        }
        for i in 0..n.list.len() {
            let (id, b) = (n.list[i].id.clone(), n.list[i].base.clone());
            let h = n.suffixed(&id, b);
            n.list[i].handle = h;
        }
        n
    }

    /// The sessions in rush's folder home, each by the title its
    /// config.json keeps (the host writes it there as it changes).
    pub fn from_home(home: &Path) -> Names {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Config {
            id: String,
            name: String,
        }
        let mut found: Vec<(String, String)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(home.join("sessions")) {
            for e in rd.flatten() {
                let Ok(b) = std::fs::read(e.path().join("config.json")) else {
                    continue;
                };
                let Ok(c) = rush_state::json::from_slice::<Config>(&b) else {
                    continue;
                };
                if !c.id.is_empty() && c.id == e.file_name().to_string_lossy() {
                    found.push((c.id, c.name));
                }
            }
        }
        found.sort();
        Names::new(found)
    }

    /// b for session id, with a short id suffix when another session has
    /// the same base: as long as it takes to tell them apart.
    fn suffixed(&self, id: &str, b: String) -> String {
        let others: Vec<String> = self
            .by_base
            .get(&b)
            .into_iter()
            .flatten()
            .map(|&j| &self.list[j].id)
            .filter(|o| *o != id)
            .map(|o| alnum(o.as_str()))
            .collect();
        if others.is_empty() {
            return b;
        }
        let mine = alnum(id);
        let mut n = 4.min(mine.len());
        while n < mine.len() && others.iter().any(|o| o.starts_with(&mine[..n])) {
            n += 1;
        }
        if n == 0 {
            return b;
        }
        format!("{b}-{}", &mine[..n])
    }

    /// Session id's handle, without its @, its title being title: what
    /// the list draws, with a fresher title than Names was made with.
    pub fn name(&self, id: &str, title: &str) -> String {
        if id.is_empty() {
            return plain(id);
        }
        self.suffixed(id, base(id, title))
    }

    /// Session id's handle, without its @; plain from its id when it's
    /// not a session known here.
    pub fn handle(&self, id: &str) -> String {
        match self.by_id.get(id) {
            Some(&i) => self.list[i].handle.clone(),
            None => plain(id),
        }
    }

    /// Whether id is a session known here.
    pub fn knows(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    /// The session typed names, with or without its @, any case: a kept
    /// @{id}; a handle; a title kebab-cased that only one session has; an
    /// animal handle from before; a session-<id> or its bare id; else the
    /// start, three letters or more, of only one session's handle.
    pub fn id_of(&self, typed: &str) -> Option<String> {
        let t = typed.strip_prefix('@').unwrap_or(typed);
        if let Some(id) = stored_id(t) {
            return Some(id.to_string());
        }
        let t = t.to_lowercase();
        if t.is_empty() {
            return None;
        }
        let only = |hits: Vec<usize>| (hits.len() == 1).then(|| self.list[hits[0]].id.clone());
        let all = || 0..self.list.len();
        let hit = only(all().filter(|&i| self.list[i].handle == t).collect())
            .or_else(|| only(self.by_base.get(&t).cloned().unwrap_or_default()))
            .or_else(|| self.legacy.get(&t).map(|&i| self.list[i].id.clone()))
            .or_else(|| {
                only(
                    all()
                        .filter(|&i| {
                            let id = &self.list[i].id;
                            plain(id) == t || id.to_lowercase() == t
                        })
                        .collect(),
                )
            });
        if hit.is_some() {
            return hit;
        }
        if t.chars().count() < 3 {
            return None;
        }
        only(
            all()
                .filter(|&i| self.list[i].handle.starts_with(&t))
                .collect(),
        )
    }

    /// The session an animal handle from before names, when it's one
    /// known here.
    pub fn legacy_id(&self, tag: &str) -> Option<&str> {
        let t = tag.strip_prefix('@').unwrap_or(tag).to_lowercase();
        self.legacy.get(&t).map(|&i| self.list[i].id.as_str())
    }

    /// The author drawn: an agent by its session's handle, anyone else as
    /// the feed always named them.
    pub fn who(&self, a: &Author) -> String {
        if a.session_id.is_empty() {
            return a.username();
        }
        format!("@{}", self.handle(&a.session_id))
    }

    /// text as it's drawn: each kept @{id}, and each animal handle from
    /// before that names a session known here, as that session's handle;
    /// whole, or each cut to WIDTH.
    pub fn show(&self, text: &str) -> String {
        self.shown(text, false)
    }

    /// text as show draws it, its handles cut to WIDTH.
    pub fn show_short(&self, text: &str) -> String {
        self.shown(text, true)
    }

    fn shown(&self, text: &str, short: bool) -> String {
        if !text.contains('@') {
            return text.to_string();
        }
        mention_re()
            .replace_all(text, |c: &regex::Captures| {
                let m = &c[0];
                let id = match stored_id(m) {
                    Some(id) => Some(id.to_string()),
                    None => self.legacy_id(m).map(str::to_string),
                };
                let Some(id) = id else {
                    return m.to_string();
                };
                let h = format!("@{}", self.handle(&id));
                if short { trim(&h) } else { h }
            })
            .into_owned()
    }

    /// text as it's kept: each @handle that names a session (see id_of)
    /// made @{id}. A tag must start a word, and a path (@src/x.rs) or a
    /// file (@notes.md) is no tag.
    pub fn store(&self, text: &str) -> String {
        if !text.contains('@') || self.list.is_empty() {
            return text.to_string();
        }
        let b = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in mention_re().find_iter(text) {
            let (i, j) = (m.start(), m.end());
            if i > 0 && !b" \t\n([{\"'`*_>".contains(&b[i - 1]) {
                continue;
            }
            if j < b.len() && (b[j] == b'/' || b[j] == b'.' && j + 1 < b.len() && b[j + 1] != b' ')
            {
                continue;
            }
            if stored_id(m.as_str()).is_some() {
                continue;
            }
            let Some(id) = self.id_of(m.as_str()) else {
                continue;
            };
            if !self.knows(&id) {
                continue;
            }
            out.push_str(&text[last..i]);
            out.push_str(&stored(&id));
            last = j;
        }
        out.push_str(&text[last..]);
        out
    }
}

static KNOWN: RwLock<Option<Arc<Names>>> = RwLock::new(None);

thread_local! {
    /// In a test, each test thread's own: tests run side by side in one
    /// process, each with its own list, and one mustn't name sessions
    /// after another's. A thread a test didn't tell knows none.
    static MINE: RefCell<Option<Arc<Names>>> = const { RefCell::new(None) };
}

/// The sessions rush's list has now: the UI tells it as the list
/// changes, so whatever draws a handle draws the same one.
pub fn set_known(n: Names) {
    let n = Arc::new(n);
    if rush_core::sandbox::active() {
        MINE.with(|m| *m.borrow_mut() = Some(n));
        return;
    }
    *KNOWN.write().unwrap_or_else(PoisonError::into_inner) = Some(n);
}

/// The sessions the UI last told set_known of; none until it has.
pub fn known() -> Arc<Names> {
    if rush_core::sandbox::active() {
        return MINE.with(|m| m.borrow().clone()).unwrap_or_default();
    }
    KNOWN
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
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

/// The animal handle rush used to give session id, without the @: a
/// dog's name and then a dog's, cat's or bird's, from its id
/// (biscuit-heron). Only to map text from before back to its session;
/// nothing is named this way now. Ported from Go's community.Name.
pub fn legacy_name(id: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_kebab_cased() {
        for (title, want) in [
            (
                "Terminal Decline: Shell Autocomplete Ideas",
                "terminal-decline-shell-autocomplete-ideas",
            ),
            ("Fix the login bug!", "fix-the-login-bug"),
            ("🚀 Ship it 🚀 — v2.1", "ship-it-v2-1"),
            ("  Résumé   parser…", "résumé-parser"),
            ("set up caching for CI", "set-up-caching-for-ci"),
            ("🎉🎉", ""),
            ("", ""),
        ] {
            assert_eq!(kebab(title), want, "{title:?}");
        }
        let long = "word ".repeat(40);
        let k = kebab(&long);
        assert_eq!(k.matches("word").count(), 40, "kept whole: {k}");
        assert_eq!(trim(&format!("@{k}")).chars().count(), WIDTH);
        assert!(trim(&format!("@{k}")).ends_with('…'));
        assert_eq!(trim("@short"), "@short");
    }

    #[test]
    fn no_title_is_plain_from_the_id() {
        assert_eq!(plain("a1b2c3d4"), "session-a1b2c3d4");
        assert_eq!(plain("A1B2-C3D4-E5F6"), "session-a1b2c3d4");
        assert_eq!(plain(""), "you");
        let n = Names::new([("a1b2c3d4", "")]);
        assert_eq!(n.handle("a1b2c3d4"), "session-a1b2c3d4");
        assert_eq!(n.handle("ffff0000"), "session-ffff0000", "unknown");
        assert!(!n.handle("a1b2c3d4").contains(&legacy_name("a1b2c3d4")));
    }

    #[test]
    fn the_same_title_gets_an_id_suffix() {
        let n = Names::new([
            ("a1b2c3d4", "Fix login"),
            ("a1b2ffff", "Fix login"),
            ("0000aaaa", "Fix: login!"),
            ("c0ffee00", "Docs"),
        ]);
        assert_eq!(n.handle("c0ffee00"), "docs");
        assert_eq!(n.handle("0000aaaa"), "fix-login-0000");
        // Their first four agree, so it takes more to tell them apart.
        assert_eq!(n.handle("a1b2c3d4"), "fix-login-a1b2c");
        assert_eq!(n.handle("a1b2ffff"), "fix-login-a1b2f");
        // The title alone names none of them; the suffixed handle does.
        assert_eq!(n.id_of("@fix-login"), None);
        assert_eq!(n.id_of("fix-login-0000").as_deref(), Some("0000aaaa"));
        // A fresher title than Names was made with clashes too.
        assert_eq!(n.name("c0ffee00", "Fix login"), "fix-login-c0ff");
        assert_eq!(n.name("c0ffee00", "Docs site"), "docs-site");
    }

    #[test]
    fn typed_handles_resolve() {
        let n = Names::new([
            ("a1b2c3d4", "Terminal Decline: Shell Autocomplete Ideas"),
            ("b0b0b0b0", "Docs site"),
            ("c1c1c1c1", "Docs search"),
        ]);
        let id = |t: &str| n.id_of(t);
        assert_eq!(
            id("@terminal-decline-shell-autocomplete-ideas").as_deref(),
            Some("a1b2c3d4")
        );
        assert_eq!(
            id("@Terminal-Decline").as_deref(),
            Some("a1b2c3d4"),
            "a unique start"
        );
        assert_eq!(id("@docs"), None, "the start of two");
        assert_eq!(id("@docs-si").as_deref(), Some("b0b0b0b0"));
        assert_eq!(id("@te"), None, "too short to guess from");
        assert_eq!(id("@{zzzz}").as_deref(), Some("zzzz"), "kept as it was");
        assert_eq!(id("@session-c1c1c1c1").as_deref(), Some("c1c1c1c1"));
        assert_eq!(
            id(&format!("@{}", legacy_name("b0b0b0b0"))).as_deref(),
            Some("b0b0b0b0"),
            "an animal handle from before"
        );
        assert_eq!(id("@nobody"), None);
    }

    #[test]
    fn id_kept_title_shown() {
        let before = Names::new([("a1b2c3d4", "Fix login bug"), ("b0b0b0b0", "Docs")]);
        let kept = before.store("@fix-login-bug can you ask @docs? see @src/x.rs and me@mail.dev");
        assert_eq!(
            kept,
            "@{a1b2c3d4} can you ask @{b0b0b0b0}? see @src/x.rs and me@mail.dev"
        );
        assert_eq!(before.store(&kept), kept, "kept once");
        assert_eq!(
            before.show(&kept),
            "@fix-login-bug can you ask @docs? see @src/x.rs and me@mail.dev"
        );
        // The session is renamed: what's kept names it still, and shows
        // its new title.
        let after = Names::new([
            ("a1b2c3d4", "Terminal Decline: Shell Autocomplete Ideas"),
            ("b0b0b0b0", "Docs"),
        ]);
        assert_eq!(
            after.show(&kept),
            "@terminal-decline-shell-autocomplete-ideas can you ask @docs? see @src/x.rs and me@mail.dev"
        );
        assert_eq!(
            after.show_short("hi @{a1b2c3d4}"),
            "hi @terminal-decline-shell…"
        );
        // One that's gone: plain from its id.
        assert_eq!(Names::default().show("@{a1b2c3d4}"), "@session-a1b2c3d4");
        // Nothing known, nothing changed.
        assert_eq!(Names::default().store("@docs hi"), "@docs hi");
    }

    #[test]
    fn old_animal_handles_map_to_their_session() {
        let n = Names::new([("a1b2c3d4", "Docs site")]);
        let old = format!("thanks @{}!", legacy_name("a1b2c3d4"));
        assert_eq!(n.show(&old), "thanks @docs-site!");
        assert_eq!(n.store(&old), "thanks @{a1b2c3d4}!");
        // One of a session not known here stays as it was.
        let other = format!("@{}", legacy_name("zz"));
        assert_eq!(n.show(&other), other);
    }

    #[test]
    fn authors_by_their_session() {
        let n = Names::new([("s1", "Header sims")]);
        let a = Author {
            session_id: "s1".into(),
            name: "Claude".into(),
            ..Author::default()
        };
        assert_eq!(n.who(&a), "@header-sims");
        assert_eq!(a.username(), "@{s1}", "kept by its id");
        let you = Author {
            name: "You".into(),
            ..Author::default()
        };
        assert_eq!(n.who(&you), "@you");
    }

    #[test]
    fn sessions_read_from_home() {
        let home = tempfile::tempdir().unwrap();
        for (id, name) in [("a1b2c3d4", "Fix login"), ("b0b0b0b0", "")] {
            let d = home.path().join("sessions").join(id);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("config.json"),
                format!(r#"{{"id":"{id}","name":"{name}","kind":"claude"}}"#),
            )
            .unwrap();
        }
        // A folder whose config names another session is no session.
        let d = home.path().join("sessions/imposter");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), r#"{"id":"a1b2c3d4","name":"X"}"#).unwrap();
        let n = Names::from_home(home.path());
        assert_eq!(n.handle("a1b2c3d4"), "fix-login");
        assert_eq!(n.handle("b0b0b0b0"), "session-b0b0b0b0");
        assert!(!n.knows("imposter"));
    }

    // The names Go gave: FNV-1a of the id, the same animals.
    #[test]
    fn legacy_names_as_go_gave_them() {
        for (id, want) in GO_NAMES {
            assert_eq!(legacy_name(id), *want, "{id}");
        }
        assert_eq!(legacy_name(""), "you");
    }

    const GO_NAMES: &[(&str, &str)] = &[
        ("s1", "scout-toffee"),
        ("s-cod", "clover-robin"),
        ("verified", "noodle-heron"),
        ("a1b2c3d4-0000-4000-8000-000000000000", "wombat-waffle"),
        ("x", "bramble-kestrel"),
        ("界", "boris-kipper"),
    ];
}
