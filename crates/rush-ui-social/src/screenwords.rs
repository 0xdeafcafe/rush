//! The words on the window's screen a dictation should expect, so names
//! come out spelled as they're written. Most wanted first, at most
//! [`MAX`]: the open (or picked) session's names (its own, its repo's and
//! branch's); the agents, models, providers and harnesses rush knows, from
//! its registry ([`agent_names`]), as they're said to it ("a Fable
//! subagent", "a Sol 6.1 one"), whether or not they're on screen; every
//! session's title and @handle; the @handles and #hashtags on screen;
//! then the distinctive words of the conversation on screen (file names,
//! CamelCase, snake_case, kebab-case, words with digits or capitals in
//! them, names in mid-sentence); then the other sessions' repos and
//! branches. yap and rush's own push-to-talk are told them
//! (dictation.rs). New in the Rust port (no Go original).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use rush_core::adapter::{AdapterRef, Registry};
use rush_core::agent::Agent;

use crate::model::Model;
use crate::session::SessionView;

/// At most this many words.
pub const MAX: usize = 200;
/// Read again at most this often.
pub const EVERY: Duration = Duration::from_secs(1);
/// A session's name is given whole when it's no longer than this.
const NAME_MAX: usize = 40;
/// At most this many of the registry's names, so the screen's own words
/// keep room.
pub const AGENT_MAX: usize = MAX / 2;

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

/// The names a user is likely to say of the agents rush knows, from its
/// registry: each provider and harness by name (Anthropic, Claude Code
/// and Claude, OpenAI, Codex, Kimi, GLM, DeepSeek, Ollama, OpenCode…),
/// then their models as people say them (Opus 5.5 and Opus, Fable, Sol
/// and Sol 6.1 for gpt-6.1-sol, as rush names that agent sol), then the
/// models' ids as written (gpt-6.1-sol). At most [`AGENT_MAX`]. A
/// harness's models may be read from its home (Codex's models cache), so
/// never on the UI thread.
pub fn agent_names(reg: &Registry) -> Vec<String> {
    let agents: Vec<AdapterRef> = reg
        .all()
        .iter()
        .filter(|a| {
            let k = a.kind();
            !reg.is_helper(k.as_str()) && reg.current_kind(k.as_str()) == k
        })
        .cloned()
        .collect();
    let mut w = Words::default();
    for a in &agents {
        let k = a.kind();
        let k = k.as_str();
        add_label(&mut w, &reg.provider_label(&reg.provider_of(k)));
        add_label(&mut w, &reg.harness_label(k));
        add_label(&mut w, &a.name());
    }
    // Kinds as rush's commands type them (opencode), when no name has it.
    for a in &agents {
        let k = a.kind();
        if k.as_str().chars().all(|c| c.is_ascii_lowercase()) {
            w.add(k.as_str());
        }
    }
    let mut ids = Vec::new();
    for a in &agents {
        let k = a.kind();
        for id in models_of(reg, a) {
            said_model(&mut w, reg, k.as_str(), &id);
            ids.push(id);
        }
    }
    for id in ids {
        if id.chars().any(char::is_alphabetic) && !id.contains('[') {
            w.add(&id);
        }
    }
    w.list.truncate(AGENT_MAX);
    w.list
}

/// A name as it's written, and its first word when it has more (Claude
/// of Claude Code, Opus of Opus 5.5, Ollama of Ollama in Codex).
fn add_label(w: &mut Words, s: &str) {
    let s = s.trim();
    if s.chars().count() <= NAME_MAX && s.chars().any(char::is_alphabetic) {
        w.add(s);
    }
    if let Some((first, _)) = s.split_once(' ')
        && first.chars().filter(|c| c.is_alphabetic()).count() >= 2
    {
        w.add(first);
    }
}

/// Every model agent a offers, by id, each once: its own snapshot's, its
/// chooser's and those its lister reads from its first profile.
fn models_of(reg: &Registry, a: &AdapterRef) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: String| {
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    };
    if let Some(s) = a.runner_snapshot() {
        s.runner_snapshot().0.into_iter().for_each(|c| push(c.id));
    }
    if let Some(c) = reg.choices_of(a.kind().as_str()) {
        c.models.into_iter().for_each(|c| push(c.id));
    }
    if let Some(ml) = a.model_lister()
        && let Some(p) = reg.profiles_of(a).first()
    {
        ml.list_models(p).into_iter().for_each(|c| push(c.id));
    }
    out
}

/// Model id as people say it: as its agent names it (Opus 5.5 for
/// claude-opus-5-5, Fable for fable), and by the short name rush gives an
/// agent on it (sol for gpt-6.1-sol), capitalised as a name is, alone
/// and with its version (Sol, Sol 6.1).
fn said_model(w: &mut Words, reg: &Registry, k: &str, id: &str) {
    let named = reg.model_name(k, id);
    if named != id {
        add_label(w, &named);
    } else if id.chars().all(|c| c.is_ascii_lowercase()) {
        add_label(w, &upper_first(id)); // fable → Fable
    }
    let parts: Vec<&str> = id.split('-').collect();
    let Some(short) = parts
        .last()
        .filter(|s| parts.len() > 1 && s.len() >= 3 && s.chars().all(|c| c.is_ascii_lowercase()))
    else {
        return;
    };
    let name = upper_first(short);
    w.add(&name);
    let versions: Vec<&&str> = parts
        .iter()
        .filter(|p| {
            p.starts_with(|c: char| c.is_ascii_digit())
                && p.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .collect();
    if let [v] = versions[..] {
        w.add(&format!("{name} {v}"));
    }
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A session's @handle, whole, as it's tagged: @fix-the-login-bug.
fn handle(a: &Agent) -> String {
    format!(
        "@{}",
        crate::mention::mention_name(a, crate::mention::host_id(a))
    )
}

/// The @handles and #hashtags in text, as they're written: a handle
/// starts a word (not an email's host), a hashtag has a letter (not an
/// issue's number), and a kept @{id} isn't said.
fn tags(w: &mut Words, text: &str) {
    for m in rush_room::community::mention_re().find_iter(text) {
        let after_word = text[..m.start()]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        if !after_word && !m.as_str().starts_with("@{") {
            w.add(m.as_str());
        }
    }
    for c in rush_room::community::hashtag_re().captures_iter(text) {
        if c[1].chars().any(char::is_alphabetic) {
            w.add(&c[1]);
        }
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

/// The conversation's rows on screen, as text.
fn conversation(v: &SessionView, h: usize) -> String {
    let rows = v.r.rows();
    let from = v.start.saturating_sub(v.base);
    let mut text = String::new();
    for r in rows.iter().skip(from).take(h.max(1)) {
        text.push_str(&r.line.plain());
        text.push('\n');
    }
    text
}

/// The conversation on screen: the open session's, else the one beside
/// the list.
fn shown(m: &Model) -> Option<&SessionView> {
    m.session.as_ref().or(m.pane.as_ref())
}

/// The words on this window's screen now, most wanted first: the open
/// (or picked) session's names, then agents (agent_names, as last read),
/// every session's title and @handle, the @handles and #hashtags on
/// screen, the conversation's words, and the other sessions' names.
pub fn collect(m: &Model, agents: &[String]) -> Vec<String> {
    let mut w = Words::default();
    let view = shown(m);
    let focus = view.map_or(m.sel.as_str(), |v| v.key.as_str());
    if let Some(a) = m.agents.iter().find(|a| a.key == focus) {
        names(&mut w, a);
        w.add(&handle(a));
    }
    for n in agents {
        w.add(n);
    }
    for a in &m.agents {
        if w.full() {
            break;
        }
        w.add_name(&a.display_name);
        w.add(&handle(a));
    }
    if let Some(v) = view {
        let text = conversation(v, m.h);
        tags(&mut w, &text);
        w.add_name(last(&v.sess.cwd));
        w.add_text(&text);
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
        let agents: Vec<String> = ["Claude", "Fable", "Sol"].map(String::from).into();
        assert_eq!(
            collect(&m, &agents),
            [
                // The picked session's names.
                "rushd restarts",
                "langwatch",
                "fix/rushd-restart",
                "rushd-restart",
                "@rushd-restarts",
                // The agents and models rush knows.
                "Claude",
                "Fable",
                "Sol",
                // Every session's title and handle.
                "Tidy the RedisQueue",
                "RedisQueue",
                "@tidy-the-redisqueue",
                // The other sessions' repos and branches.
                "main",
            ]
        );
        let before = fingerprint(&m);
        assert_eq!(fingerprint(&m), before);
        m.agents[0].branch = "dev".into();
        assert_ne!(fingerprint(&m), before);
    }

    // The names rush knows go ahead of the screen's words: with the
    // screen's words past the cap, every one of them is still said.
    #[test]
    fn agent_names_ahead_of_the_screen() {
        let mut m = Model::bare("t", |_| {});
        m.agents = (0..MAX)
            .map(|i| Agent {
                key: format!("k{i}"),
                display_name: format!("Session{i}"),
                branch: format!("feat/branch-{i}"),
                ..Agent::default()
            })
            .collect();
        let agents: Vec<String> = ["Opus", "Sonnet", "Fable", "Sol 6.1", "Sol"]
            .map(String::from)
            .into();
        let got = collect(&m, &agents);
        assert_eq!(got.len(), MAX);
        for n in &agents {
            assert!(got.contains(n), "{n} cut: {got:?}");
        }
        // No session picked: the agents' names come first.
        assert_eq!(got[..agents.len()], agents[..]);
    }

    #[test]
    fn handles_and_hashtags() {
        // (text, what's kept)
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            ("ask @terminal-decline about it", &["@terminal-decline"]),
            ("#cleanup and #pin it", &["#cleanup", "#pin"]),
            ("a kept @{c6d9b964} isn't said", &[]),
            ("mail me@example.com, issue #12", &[]),
            (
                "tell @Luna to file it in #db-migrate",
                &["@Luna", "#db-migrate"],
            ),
        ];
        for (text, want) in cases {
            let mut w = Words::default();
            tags(&mut w, text);
            assert_eq!(w.list, *want, "{text:?}");
        }
    }

    // A provider's own agent with its models, as Claude Code's chooser
    // lists them and names them, and one whose models are read from its
    // home, as Codex's are.
    struct Maker {
        kind: &'static str,
        name: &'static str,
        maker: &'static str,
        models: &'static [&'static str],
        listed: &'static [&'static str],
    }

    impl rush_core::adapter::Adapter for Maker {
        fn kind(&self) -> rush_core::adapter::Kind {
            self.kind.into()
        }
        fn name(&self) -> String {
            self.name.into()
        }
        fn features(&self) -> rush_core::adapter::Features {
            Default::default()
        }
        fn level(&self) -> rush_core::adapter::Level {
            Default::default()
        }
        fn profiles(&self) -> Vec<rush_core::adapter::Profile> {
            vec![rush_core::adapter::Profile::default()]
        }
        rush_core::has!(chooser: Chooser, made: Made, model_lister: ModelLister, model_namer: ModelNamer);
    }

    fn choices(ids: &[&str]) -> Vec<rush_core::adapter::Choice> {
        ids.iter()
            .map(|id| rush_core::adapter::Choice {
                id: id.to_string(),
                ..Default::default()
            })
            .collect()
    }

    impl rush_core::adapter::Chooser for Maker {
        fn choices(&self) -> rush_core::adapter::Choices {
            rush_core::adapter::Choices {
                models: choices(self.models),
                ..Default::default()
            }
        }
    }

    impl rush_core::adapter::Made for Maker {
        fn maker(&self) -> String {
            self.maker.into()
        }
    }

    impl rush_core::adapter::ModelLister for Maker {
        fn list_models(&self, _: &rush_core::adapter::Profile) -> Vec<rush_core::adapter::Choice> {
            choices(self.listed)
        }
    }

    impl rush_core::adapter::ModelNamer for Maker {
        fn model_name(&self, id: &str) -> String {
            if self.kind == "claude" {
                rush_claude::modelname::model_name(id)
            } else {
                id.to_string()
            }
        }
    }

    #[test]
    fn agent_names_from_the_registry() {
        let reg = Registry::new();
        reg.register(Maker {
            kind: "claude",
            name: "Claude Code",
            maker: "Anthropic",
            models: &["opus", "opus[1m]", "sonnet", "haiku", "fable"],
            listed: &[],
        });
        reg.register(Maker {
            kind: "codex",
            name: "Codex",
            maker: "OpenAI",
            models: &[],
            listed: &["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna"],
        });
        reg.register(Maker {
            kind: "opencode",
            name: "OpenCode",
            maker: "",
            models: &[],
            listed: &[],
        });
        assert_eq!(
            agent_names(&reg),
            [
                "Anthropic",
                "Claude Code",
                "Claude",
                "OpenAI",
                "Codex",
                "OpenCode",
                "Opus",
                "Sonnet",
                "Haiku",
                "Fable",
                "Sol",
                "Sol 6.1",
                "Astra",
                "Astra 6",
                "Luna",
                "Luna 6",
                "gpt-6.1-sol",
                "gpt-6-astra",
                "gpt-6-luna",
            ]
        );
        assert!(agent_names(&Registry::new()).is_empty());
    }
}
