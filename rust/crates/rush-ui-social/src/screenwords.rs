//! The words on the window's screen a dictation should expect, so names
//! come out spelled as they're written: the sessions' names, their repos'
//! and branches' names, and the distinctive words of the conversation on
//! screen (file names, CamelCase, snake_case, kebab-case, words with
//! digits or capitals in them, names in mid-sentence). The open (or
//! picked) session's come first; at most [`MAX`]. yap and rush's own
//! push-to-talk are told them (dictation.rs). New in the Rust port (no Go
//! original).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use rush_core::agent::Agent;

use crate::model::Model;
use crate::session::SessionView;

/// At most this many words.
pub const MAX: usize = 200;
/// Read again at most this often.
pub const EVERY: Duration = Duration::from_secs(1);
/// A session's name is given whole when it's no longer than this.
const NAME_MAX: usize = 40;

/// Words gathered in order, each once whatever its case, at most [`MAX`].
#[derive(Default)]
pub struct Words {
    seen: HashSet<String>,
    pub list: Vec<String>,
}

impl Words {
    /// Adds w unless it's empty, already there or there's no room.
    pub fn add(&mut self, w: &str) {
        let w = w.trim();
        if w.is_empty() || self.full() || !self.seen.insert(w.to_lowercase()) {
            return;
        }
        self.list.push(w.to_string());
    }

    pub fn full(&self) -> bool {
        self.list.len() >= MAX
    }

    /// Adds text's distinctive words.
    pub fn add_text(&mut self, text: &str) {
        for t in distinctive(text) {
            if self.full() {
                return;
            }
            self.add(&t);
        }
    }

    /// Adds a name as it is (a session's, a repo's, a branch's) and its
    /// distinctive words: a commit's or worktree's hex isn't a name.
    pub fn add_name(&mut self, name: &str) {
        let name = name.trim();
        if name.chars().count() <= NAME_MAX
            && name.chars().any(char::is_alphabetic)
            && !hexish(name)
        {
            self.add(name);
        }
        self.add_text(name);
    }
}

/// A token's characters: the rest split words.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')
}

/// The distinctive words of text, in order, as they're written: a path's
/// last part, file names, CamelCase, snake_case, kebab-case, words with
/// digits or capitals in them, and a capitalised word in mid-sentence (a
/// name: Redis, LangWatch). Ordinary words, numbers and hex hashes aren't.
pub fn distinctive(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // At a sentence's start a capital says nothing.
    let mut start = true;
    let mut tok = String::new();
    let mut flush = |tok: &mut String, start: &mut bool| {
        if tok.is_empty() {
            return;
        }
        let ends = tok.ends_with('.');
        let t = tok.trim_matches(|c| matches!(c, '.' | '-' | '_'));
        if !t.is_empty() {
            if let Some(w) = classify(t, *start) {
                out.push(w);
            }
            *start = false;
        }
        if ends {
            *start = true;
        }
        tok.clear();
    };
    for c in text.chars() {
        if word_char(c) {
            tok.push(c);
            continue;
        }
        flush(&mut tok, &mut start);
        if matches!(c, '!' | '?' | ':' | '\n' | '•' | '›') {
            start = true;
        }
    }
    flush(&mut tok, &mut start);
    out
}

/// What a token gives, if it's distinctive.
fn classify(t: &str, sentence_start: bool) -> Option<String> {
    if t.contains('/') {
        // A path or a branch: its last part, a file or an identifier.
        let last = t.rsplit('/').find(|p| !p.is_empty())?;
        let last = last.trim_matches(|c| matches!(c, '.' | '-' | '_'));
        return (file_name(last) || identifier(last, true)).then(|| last.to_string());
    }
    if t.contains('.') {
        return file_name(t).then(|| t.to_string());
    }
    identifier(t, sentence_start).then(|| t.to_string())
}

/// stem.ext: a file's (or a host's) name, its stem two letters or more,
/// its extension a short word.
fn file_name(t: &str) -> bool {
    let Some((stem, ext)) = t.rsplit_once('.') else {
        return false;
    };
    stem.chars().count() >= 2
        && stem.chars().any(char::is_alphabetic)
        && (1..=6).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
        && t.len() <= 60
}

/// Whether t (letters, digits, - and _ only) reads as a name rather than
/// an ordinary word.
fn identifier(t: &str, sentence_start: bool) -> bool {
    let n = t.chars().count();
    if !(3..=40).contains(&n)
        || !t
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        || !t.chars().next().is_some_and(char::is_alphabetic)
        || hexish(t)
    {
        return false;
    }
    let letters = t.chars().filter(|c| c.is_alphabetic()).count();
    let digits = t.chars().any(|c| c.is_ascii_digit());
    let snake_or_kebab = t.contains('_') || t.contains('-');
    let caps_inside = t.chars().skip(1).any(char::is_uppercase);
    let capitalised = t.chars().next().is_some_and(char::is_uppercase);
    snake_or_kebab || (digits && letters >= 2) || caps_inside || (capitalised && !sentence_start)
}

/// A hash or an id: seven or more hex digits with a digit among them.
fn hexish(t: &str) -> bool {
    t.len() >= 7
        && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && t.chars().any(|c| c.is_ascii_digit())
}

/// A session's names: its own, its repo's, its branch's and its
/// worktree's.
fn names(w: &mut Words, a: &Agent) {
    w.add_name(&a.display_name);
    for p in [&a.repo, &a.root] {
        w.add_name(last(p));
    }
    w.add_name(&a.branch);
    if let Some(ws) = &a.workspace {
        w.add_name(last(&ws.repo));
        w.add_name(&ws.branch);
        w.add_name(&ws.target);
    }
}

fn last(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// The conversation's rows on screen.
fn conversation(w: &mut Words, v: &SessionView, h: usize) {
    w.add_name(last(&v.sess.cwd));
    let rows = v.r.rows();
    let from = v.start.saturating_sub(v.base);
    let mut text = String::new();
    for r in rows.iter().skip(from).take(h.max(1)) {
        text.push_str(&r.line.plain());
        text.push('\n');
    }
    w.add_text(&text);
}

/// The conversation on screen: the open session's, else the one beside
/// the list.
fn shown(m: &Model) -> Option<&SessionView> {
    m.session.as_ref().or(m.pane.as_ref())
}

/// The words on this window's screen now, the open (or picked) session's
/// first.
pub fn collect(m: &Model) -> Vec<String> {
    let mut w = Words::default();
    let view = shown(m);
    let focus = view.map_or(m.sel.as_str(), |v| v.key.as_str());
    if let Some(a) = m.agents.iter().find(|a| a.key == focus) {
        names(&mut w, a);
    }
    if let Some(v) = view {
        conversation(&mut w, v, m.h);
    }
    for a in &m.agents {
        if w.full() {
            break;
        }
        names(&mut w, a);
    }
    w.list
}

/// What the words are read from, cheaply: when it's the same the words
/// are, and aren't read again.
pub fn fingerprint(m: &Model) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.sel.hash(&mut h);
    m.h.hash(&mut h);
    if let Some(v) = shown(m) {
        v.key.hash(&mut h);
        v.r#gen.hash(&mut h);
        (v.start, v.base, v.r.kept_rows(), v.r.window_turns()).hash(&mut h);
        v.sess.turns.len().hash(&mut h);
        if let Some(t) = v.sess.turns.last() {
            (t.items.len(), t.live, t.end).hash(&mut h);
        }
    }
    m.agents.len().hash(&mut h);
    for a in &m.agents {
        (&a.key, &a.display_name, &a.repo, &a.branch).hash(&mut h);
        if let Some(ws) = &a.workspace {
            (&ws.branch, &ws.target).hash(&mut h);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinctive_words() {
        // (text, the words it gives)
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            ("the quick brown fox jumps", &[]),
            ("We use Redis for the queue.", &["Redis"]),
            // A capital at a sentence's start is just a sentence.
            ("Redis is fast. The queue is slow", &[]),
            ("ask LangWatch about rushd", &["LangWatch"]),
            ("open `screenwords.rs` now", &["screenwords.rs"]),
            (
                "edit crates/rush-ui/src/screens/social/dictation.rs",
                &["dictation.rs"],
            ),
            (
                "cd ~/Source/github.com/0xdeafcafe/rush-voice/",
                &["rush-voice"],
            ),
            (
                "call follow_at and context_line",
                &["follow_at", "context_line"],
            ),
            (
                "the rush-ui crate's push-to-talk",
                &["rush-ui", "push-to-talk"],
            ),
            ("objc2 and x86_64, not 100ms or 2nd", &["objc2", "x86_64"]),
            (
                "on macOS and iPhone with JSON",
                &["macOS", "iPhone", "JSON"],
            ),
            ("commit a9b7ecfe and c6d9b964-818d", &[]),
            ("e.g. this, i.e. that, version 1.2.3", &[]),
            ("see github.com for it", &["github.com"]),
            (
                "SFSpeechRecognizer: setContextualStrings",
                &["SFSpeechRecognizer", "setContextualStrings"],
            ),
            ("--verbose and -D warnings", &[]),
            ("line one\nRedis on a new row", &[]),
            ("ok, Alex said so", &["Alex"]),
        ];
        for (text, want) in cases {
            assert_eq!(distinctive(text), *want, "{text:?}");
        }
    }

    #[test]
    fn names_and_order() {
        // (names added, what's kept)
        let cases: &[(&[&str], &[&str])] = &[
            (&["rush", "rush"], &["rush"]),
            (&["Redis", "redis"], &["Redis"]),
            (&["rush/c6d9b964"], &["rush/c6d9b964"]),
            (&["c6d9b964"], &[]),
            (&["feat/rusty-rush"], &["feat/rusty-rush", "rusty-rush"]),
            (
                &["Fix the LangWatch exporter"],
                &["Fix the LangWatch exporter", "LangWatch"],
            ),
            (&[""], &[]),
        ];
        for (names, want) in cases {
            let mut w = Words::default();
            for n in *names {
                w.add_name(n);
            }
            assert_eq!(w.list, *want, "{names:?}");
        }
    }

    #[test]
    fn capped() {
        let mut w = Words::default();
        for i in 0..MAX + 50 {
            w.add(&format!("word{i}"));
        }
        assert_eq!(w.list.len(), MAX);
        assert_eq!(w.list[0], "word0");
    }

    #[test]
    fn focused_session_first() {
        let mut m = Model::bare("t", |_| {});
        let agent = |key: &str, name: &str, branch: &str| Agent {
            key: key.into(),
            display_name: name.into(),
            repo: "/src/langwatch".into(),
            branch: branch.into(),
            ..Agent::default()
        };
        m.agents = vec![
            agent("a", "Tidy the RedisQueue", "main"),
            agent("b", "rushd restarts", "fix/rushd-restart"),
        ];
        m.sel = "b".into();
        assert_eq!(
            collect(&m),
            [
                "rushd restarts",
                "langwatch",
                "fix/rushd-restart",
                "rushd-restart",
                "Tidy the RedisQueue",
                "RedisQueue",
                "main",
            ]
        );
        let before = fingerprint(&m);
        assert_eq!(fingerprint(&m), before);
        m.agents[0].branch = "dev".into();
        assert_ne!(fingerprint(&m), before);
    }
}
