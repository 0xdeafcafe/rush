//! drafts is rush's stash, as a plugin bundled with it: a message box is
//! kept until it's sent, a message can be stashed while you send another,
//! and what you sent, cleared and replaced can be put back. What it's
//! called, Stash or Drafts, is a setting: see Words.
//!
//! This is the logic, with no I/O and no clock of its own; mod.rs wires it
//! to rush.
//! Ported from internal/bundled/drafts/book.go:1-391.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use rush_plugin::{MAX_PICK_ITEMS, Pick, PickAction, PickItem, TextBox};
use rush_state::Time;
use serde::{Deserialize, Serialize};

// Kinds of what's kept.
pub const SENT: &str = "sent";
/// wiped from a box without sending
pub const CLEARED: &str = "cleared";
/// a box's text a put-back replaced; "kept" on disk
pub const REPLACED: &str = "kept";

/// The pick's tabs, in order, after the first: the stashes, by Words.
pub const TABS: [&str; 3] = ["Sent", "Cleared", "Replaced"];

pub(crate) fn kind_tab(kind: &str) -> i64 {
    match kind {
        SENT => 1,
        CLEARED => 2,
        REPLACED => 3,
        _ => 0,
    }
}

/// What it's called in everything it and rush say, by its called setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    /// the history's: Stash
    pub title: &'static str,
    /// the stashes' tab: Stashed
    pub tab: &'static str,
    /// what the stash key does: stash
    pub verb: &'static str,
    /// what it did: stashed
    pub did: &'static str,
    /// one set aside: your stash
    pub noun: &'static str,
}

/// The names it can go by; the first is the default.
pub const CALLED: [&str; 2] = ["Stash", "Drafts"];

const STASH: Words = Words {
    title: "Stash",
    tab: "Stashed",
    verb: "stash",
    did: "stashed",
    noun: "stash",
};

const DRAFTS: Words = Words {
    title: "Drafts",
    tab: "Drafts",
    verb: "keep as a draft",
    did: "kept as a draft",
    noun: "draft",
};

/// The words for a called setting, Stash's for any other.
pub fn words_for(called: &str) -> Words {
    match called {
        "Drafts" => DRAFTS,
        _ => STASH,
    }
}

impl Words {
    /// The # command rush opens the history with: #stash.
    pub fn command(&self) -> String {
        format!("#{}", self.title.to_lowercase())
    }

    /// Puts the words in s where it says {title}, {tab}, {verb}, {did},
    /// {noun} or {command}.
    pub fn fill(&self, s: &str) -> String {
        s.replace("{title}", self.title)
            .replace("{tab}", self.tab)
            .replace("{verb}", self.verb)
            .replace("{did}", self.did)
            .replace("{noun}", self.noun)
            .replace("{command}", &self.command())
    }
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Something typed into a box, kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    #[serde(rename = "box")]
    pub text_box: TextBox,
    /// the box's: "" is the Prompt
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session: String,
    /// what that session was called
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub at: Time,
}

/// A box set aside.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stash {
    #[serde(rename = "box")]
    pub text_box: TextBox,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub at: Time,
}

/// What's kept on disk.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// What each box held, until it's sent or emptied.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub boxes: BTreeMap<String, TextBox>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub stashes: BTreeMap<String, Stash>,
    /// newest first
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Entry>,
    #[serde(skip_serializing_if = "is_zero")]
    pub seq: i64,
    /// The drafts rush kept itself, before, are taken in.
    #[serde(skip_serializing_if = "is_false")]
    pub imported: bool,
}

/// Keeps the store, and says what to do to rush's boxes. What it says
/// names rush's stash key as {aside} and its history key as {history},
/// for rush to fill in with the keys you have.
#[derive(Clone, Debug)]
pub struct Book {
    pub s: Store,
    pub w: Words,
    /// entries kept of each kind
    keep: usize,
}

/// How many of each kind are kept.
pub const DEFAULT_KEEP: usize = 300;

/// The keep setting.
pub fn parse_keep(v: &str) -> usize {
    match v.parse::<i64>() {
        Ok(n) if n > 0 => n as usize,
        _ => DEFAULT_KEEP,
    }
}

/// A box with nothing in it.
pub fn empty(b: &TextBox) -> bool {
    b.text.trim().is_empty() && b.images.is_empty()
}

/// A box to set: key's, to text_box, only while it holds if_text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Set {
    pub key: String,
    pub text_box: TextBox,
    pub if_text: String,
}

/// What stashing did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StashResult {
    pub set: Option<Set>,
    /// what to tell you
    pub said: String,
}

fn boxed(text: &str) -> TextBox {
    TextBox {
        text: text.into(),
        cursor: text.chars().count() as i64,
        ..Default::default()
    }
}

impl Book {
    /// A book over s, keeping keep of each kind (0 for the default).
    pub fn new(s: Store, keep: usize) -> Book {
        Book {
            s,
            w: words_for(""),
            keep: if keep == 0 { DEFAULT_KEEP } else { keep },
        }
    }

    /// Notes what a box holds now, saying whether that's its stash back: a
    /// stash goes only once the box is seen holding it, so one rush
    /// couldn't put back is never lost.
    pub fn changed(&mut self, key: &str, b: TextBox) -> bool {
        if empty(&b) {
            self.s.boxes.remove(key);
            return false;
        }
        let text = b.text.trim().to_string();
        self.s.boxes.insert(key.into(), b);
        if self
            .s
            .stashes
            .get(key)
            .is_some_and(|st| st.text_box.text.trim() == text)
        {
            self.s.stashes.remove(key);
            return true;
        }
        false
    }

    /// A box coming into view: what it held, if rush's is empty.
    pub fn opened(&self, key: &str) -> Option<Set> {
        self.s.boxes.get(key).map(|b| Set {
            key: key.into(),
            text_box: b.clone(),
            if_text: String::new(),
        })
    }

    /// A box's message gone: kept as sent, and the box's stash, if it has
    /// one, put back in the box it left empty.
    pub fn sent(&mut self, key: &str, name: &str, text: &str, now: Time) -> Option<Set> {
        // The box as last seen has its chips; the text sent, only when
        // there was none.
        let b = self.s.boxes.remove(key).unwrap_or_else(|| boxed(text));
        if !empty(&b) {
            self.add(SENT, key, name, b, now);
        }
        self.s.stashes.get(key).map(|st| Set {
            key: key.into(),
            text_box: st.text_box.clone(),
            if_text: String::new(),
        })
    }

    /// A box wiped without sending, kept as cleared.
    pub fn cleared(&mut self, key: &str, name: &str, text: &str, now: Time) {
        let b = self.s.boxes.remove(key).unwrap_or_else(|| boxed(text));
        if !empty(&b) {
            self.add(CLEARED, key, name, b, now);
        }
    }

    /// What the book says, in its words.
    pub fn said(&self, s: &str) -> String {
        self.w.fill(s)
    }

    /// The stash key on a box holding b: what's typed is set aside and the
    /// box emptied; with nothing typed, what's stashed comes back; with
    /// both, the two change places.
    pub fn stash(&mut self, key: &str, name: &str, b: TextBox, now: Time) -> StashResult {
        let st = self.s.stashes.get(key).cloned();
        match (empty(&b), st) {
            (true, None) => StashResult {
                set: None,
                said: self.said("nothing to {verb} · type a message first"),
            },
            (true, Some(st)) => StashResult {
                set: Some(Set {
                    key: key.into(),
                    text_box: st.text_box,
                    if_text: b.text,
                }),
                said: self.said("your {noun} is back in the box"),
            },
            (false, None) => {
                let if_text = b.text.clone();
                self.s.stashes.insert(
                    key.into(),
                    Stash {
                        text_box: b,
                        name: name.into(),
                        at: now,
                    },
                );
                self.s.boxes.remove(key);
                // The Prompt's stash key is the list's own once it's empty.
                let again = if key.is_empty() {
                    "{history} › {tab} has it"
                } else {
                    "{aside} brings it back now"
                };
                StashResult {
                    set: Some(Set {
                        key: key.into(),
                        text_box: TextBox::default(),
                        if_text,
                    }),
                    said: self.said(&format!("{{did}} · it comes back after you send · {again}")),
                }
            }
            (false, Some(st)) => {
                let if_text = b.text.clone();
                self.s.stashes.insert(
                    key.into(),
                    Stash {
                        text_box: b,
                        name: name.into(),
                        at: now,
                    },
                );
                StashResult {
                    set: Some(Set {
                        key: key.into(),
                        text_box: st.text_box,
                        if_text,
                    }),
                    said: self.said(
                        "{did} this one, and your other {noun} is back · {aside} swaps them again",
                    ),
                }
            }
        }
    }

    /// What a box's edge says: whether it has a stash.
    pub fn note(&self, key: &str) -> String {
        if self.s.stashes.contains_key(key) {
            return self.said("{did} · back after you send");
        }
        String::new()
    }

    /// Puts an item back in the box of key, which holds cur (None when it
    /// isn't the box with the keys: then only an empty one is set). What
    /// the box held is kept as replaced, so nothing is lost, and the said
    /// says where.
    pub fn put_back(
        &mut self,
        id: &str,
        key: &str,
        name: &str,
        cur: Option<&TextBox>,
        now: Time,
    ) -> Option<(Set, String)> {
        let b = if let Some(from) = id.strip_prefix("stash:") {
            self.s.stashes.remove(from)?.text_box
        } else {
            self.s.history.iter().find(|e| e.id == id)?.text_box.clone()
        };
        let Some(cur) = cur else {
            return Some((
                Set {
                    key: key.into(),
                    text_box: b,
                    if_text: String::new(),
                },
                String::new(),
            ));
        };
        let mut said = String::new();
        if !empty(cur) {
            self.add(REPLACED, key, name, cur.clone(), now);
            said = "put back · what the box held is under {history} › Replaced".into();
        }
        Some((
            Set {
                key: key.into(),
                text_box: b,
                if_text: cur.text.clone(),
            },
            said,
        ))
    }

    /// Drops an item.
    pub fn forget(&mut self, id: &str) {
        if let Some(from) = id.strip_prefix("stash:") {
            self.s.stashes.remove(from);
            return;
        }
        self.s.history.retain(|e| e.id != id);
    }

    /// Keeps b as kind, newest first, dropping the same text of the same
    /// kind kept before, and the oldest past keep.
    fn add(&mut self, kind: &str, key: &str, name: &str, b: TextBox, now: Time) {
        let text = plain(&b).trim().to_string();
        self.s
            .history
            .retain(|e| !(e.kind == kind && plain(&e.text_box).trim() == text));
        self.s.seq += 1;
        let e = Entry {
            id: format!("h{}", self.s.seq),
            kind: kind.into(),
            text_box: b,
            session: key.into(),
            name: name.into(),
            at: now,
        };
        self.s.history.insert(0, e);
        self.trim();
    }

    /// Changes how many are kept, saying whether any went.
    pub fn set_keep(&mut self, n: usize) -> bool {
        self.keep = n;
        let was = self.s.history.len();
        self.trim();
        self.s.history.len() != was
    }

    fn trim(&mut self) {
        let mut count: BTreeMap<String, usize> = BTreeMap::new();
        let keep = self.keep;
        self.s.history.retain(|e| {
            let c = count.entry(e.kind.clone()).or_default();
            *c += 1;
            *c <= keep
        });
    }

    /// The list of what's kept, for the box of key: the stashes, then each
    /// kind, newest first.
    pub fn pick(&self, key: &str, now: Time) -> Pick {
        let mut p = Pick {
            id: "history".into(),
            title: self.w.title.into(),
            about: self
                .said("what you {did}, sent, cleared and replaced · enter puts one in the box"),
            tabs: std::iter::once(self.w.tab)
                .chain(TABS)
                .map(String::from)
                .collect(),
            session: key.into(),
            actions: vec![
                PickAction {
                    key: "enter".into(),
                    name: "put it in the box".into(),
                    stay: false,
                },
                PickAction {
                    key: "ctrl+d".into(),
                    name: "forget it".into(),
                    stay: true,
                },
            ],
            empty: vec![
                self.said("nothing here · {aside} sets what's typed aside while you send another"),
                "nothing sent yet · the messages you send land here, to use again".into(),
                "nothing cleared · what you wipe from a box lands here".into(),
                "nothing replaced · what a box held when you put another back lands here".into(),
            ],
            ..Default::default()
        };
        let mut stashed: Vec<&String> = self.s.stashes.keys().collect();
        stashed.sort_by(|x, y| self.s.stashes[*y].at.cmp(&self.s.stashes[*x].at));
        for k in &stashed {
            let st = &self.s.stashes[*k];
            p.items.push(PickItem {
                id: format!("stash:{k}"),
                tab: 0,
                text: plain(&st.text_box),
                meta: meta(&st.name, k, st.at, now),
            });
        }
        for e in &self.s.history {
            if p.items.len() >= MAX_PICK_ITEMS {
                break;
            }
            p.items.push(PickItem {
                id: e.id.clone(),
                tab: kind_tab(&e.kind),
                text: plain(&e.text_box),
                meta: meta(&e.name, &e.session, e.at, now),
            });
        }
        // It opens on what you did last, a minute ago or less; else on this
        // box's stash, or on what you sent.
        p.tab = match self.s.history.first() {
            Some(h) if now.sub_nanos(h.at) < MINUTE => kind_tab(&h.kind),
            _ if self.s.stashes.contains_key(key) || !stashed.is_empty() => 0,
            _ => kind_tab(SENT),
        };
        p
    }
}

const MINUTE: i128 = 60 * 1_000_000_000;

/// convo.PasteChipRe: a paste's chip, its number the first group.
static PASTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:[Pp]asted text )?#(\d+) (?:· )?(?:\+\d+ lines|\d+ lines?(?:: [^\]\n]*)?)\]")
        .expect("a good pattern")
});

/// A box's text with its pastes put back, as it would be sent.
pub fn plain(b: &TextBox) -> String {
    if b.pastes.is_empty() {
        return b.text.clone();
    }
    PASTE_RE
        .replace_all(&b.text, |c: &regex::Captures| {
            let n: i64 = c[1].parse().unwrap_or(0);
            match b.pastes.get(&n) {
                Some(t) => t.clone(),
                None => c[0].to_string(),
            }
        })
        .into_owned()
}

fn meta(name: &str, key: &str, at: Time, now: Time) -> String {
    let mut name = name.to_string();
    if name.is_empty() && key.is_empty() {
        name = "the Prompt".into();
    }
    let s = ago(now.sub_nanos(at));
    if name.is_empty() {
        return s;
    }
    if name.chars().count() > 22 {
        name = name.chars().take(21).collect::<String>() + "…";
    }
    format!("{name} · {s}")
}

fn ago(d: i128) -> String {
    const HOUR: i128 = 60 * MINUTE;
    if d < MINUTE {
        "just now".into()
    } else if d < HOUR {
        format!("{}m ago", d / MINUTE)
    } else if d < 24 * HOUR {
        format!("{}h ago", d / HOUR)
    } else {
        format!("{}d ago", d / HOUR / 24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Time {
        Time::parse("2026-09-29T12:00:00Z").unwrap()
    }

    fn at(secs: i64) -> Time {
        t0().add_nanos(secs * 1_000_000_000)
    }

    fn b(s: &str) -> TextBox {
        boxed(s)
    }

    // book_test.go TestBoxKeptUntilSent: a box is kept until it's sent,
    // and comes back into an empty box.
    #[test]
    fn box_kept_until_sent() {
        let mut bk = Book::new(Store::default(), 0);
        let chip = TextBox {
            text: "fix [Pasted text #1 +4 lines]".into(),
            cursor: 3,
            pastes: BTreeMap::from([(1, "a\nb\nc\nd".into())]),
            ..Default::default()
        };
        bk.changed("s1", chip);
        let set = bk.opened("s1").expect("opened");
        assert!(
            set.if_text.is_empty()
                && set.text_box.cursor == 3
                && !set.text_box.pastes[&1].is_empty()
        );
        bk.sent("s1", "one", "fix a b c d", t0());
        assert!(bk.opened("s1").is_none(), "a sent box shouldn't come back");
        assert!(
            bk.s.history.len() == 1
                && bk.s.history[0].kind == SENT
                && !bk.s.history[0].text_box.pastes[&1].is_empty(),
            "sent should be kept with its chips: {:?}",
            bk.s.history
        );
        bk.changed("s1", b("  "));
        assert!(
            bk.opened("s1").is_none(),
            "an emptied box shouldn't come back"
        );
    }

    // book_test.go TestStash: the stash key sets a message aside, brings it
    // back, or swaps; sending brings it back by itself.
    #[test]
    fn stash() {
        let mut bk = Book::new(Store::default(), 0);
        assert!(
            bk.stash("s1", "one", b(""), t0()).set.is_none(),
            "nothing to stash"
        );
        let r = bk.stash("s1", "one", b("first"), t0());
        let s = r.set.unwrap();
        assert!(s.text_box.text.is_empty() && s.if_text == "first" && !bk.note("s1").is_empty());
        let r = bk.stash("s1", "one", b("second"), t0());
        let s = r.set.unwrap();
        assert!(
            s.text_box.text == "first"
                && s.if_text == "second"
                && bk.s.stashes["s1"].text_box.text == "second"
        );
        bk.changed("s1", b("third"));
        let set = bk.sent("s1", "one", "third", t0()).expect("after sending");
        assert!(set.text_box.text == "second" && set.if_text.is_empty());
        // It's kept until the box is seen holding it: rush may not have.
        assert!(!bk.note("s1").is_empty());
        assert!(bk.changed("s1", b("second")));
        assert!(
            bk.note("s1").is_empty(),
            "the stash should go once it's back"
        );
        bk.stash("s1", "one", b("fourth"), t0());
        let r = bk.stash("s1", "one", b(""), t0());
        let s = r.set.unwrap();
        assert!(s.text_box.text == "fourth");
        assert!(
            bk.changed("s1", s.text_box) && bk.s.stashes.is_empty(),
            "back on the key"
        );
    }

    // book_test.go TestPutBackAndForget: putting one back keeps what the
    // box held; forgetting drops it.
    #[test]
    fn put_back_and_forget() {
        let mut bk = Book::new(Store::default(), 0);
        bk.changed("s1", b("wiped"));
        bk.cleared("s1", "one", "wiped", t0());
        bk.stash("s2", "two", b("aside"), t0());
        let p = bk.pick("s1", at(10));
        assert!(
            p.tab == 2 && p.items.len() == 2 && p.items[0].id == "stash:s2" && p.items[1].tab == 2,
            "pick: {p:?}"
        );
        rush_plugin::clean_pick(p.clone()).expect("the pick should be one rush takes");
        let cur = b("half typed");
        let (set, said) = bk
            .put_back(&p.items[1].id, "s1", "one", Some(&cur), t0())
            .unwrap();
        assert!(
            set.text_box.text == "wiped"
                && set.if_text == "half typed"
                && bk.s.history[0].kind == REPLACED
                && said.contains("Replaced"),
            "put back: {set:?} {said}"
        );
        let (set, _) = bk.put_back("stash:s2", "s1", "one", None, t0()).unwrap();
        assert!(set.text_box.text == "aside" && set.if_text.is_empty() && bk.s.stashes.is_empty());
        let id = bk.s.history[0].id.clone();
        bk.forget(&id);
        assert_eq!(bk.s.history.len(), 1);
    }

    // book_test.go TestKeepsNewest: the same text sent twice is kept once;
    // each kind keeps its newest.
    #[test]
    fn keeps_newest() {
        let mut bk = Book::new(Store::default(), 2);
        for (i, s) in ["a", "b", "a", "c"].into_iter().enumerate() {
            bk.changed("s1", b(s));
            bk.sent("s1", "", s, at(60 * i as i64));
        }
        let texts: Vec<&str> =
            bk.s.history
                .iter()
                .map(|e| e.text_box.text.as_str())
                .collect();
        assert_eq!(texts, ["c", "a"]);
    }

    // book_test.go TestEveryWayOutLandsInTheHistory
    #[test]
    fn every_way_out_lands_in_the_history() {
        let mut bk = Book::new(Store::default(), 0);
        bk.changed("s1", b("wiped"));
        bk.cleared("s1", "one", "wiped", t0());
        bk.stash("s1", "one", b("aside"), t0());
        bk.changed("s1", b("gone out"));
        bk.sent("s1", "one", "gone out", t0());
        bk.changed("s1", b("aside")); // the stash is back in the box
        let cur = b("aside");
        let sent = bk.s.history[0].id.clone();
        bk.put_back(&sent, "s1", "one", Some(&cur), t0());
        bk.stash("s2", "two", b("still aside"), t0());
        let tabs: BTreeMap<String, i64> = bk
            .pick("s1", t0())
            .items
            .into_iter()
            .map(|i| (i.text, i.tab))
            .collect();
        for (text, tab) in [
            ("still aside", 0),
            ("gone out", kind_tab(SENT)),
            ("wiped", kind_tab(CLEARED)),
            ("aside", kind_tab(REPLACED)),
        ] {
            assert_eq!(tabs.get(text), Some(&tab), "{text}");
        }
        let p = bk.pick("s1", t0());
        assert!(
            p.tabs[0] == "Stashed" && p.tabs[3] == "Replaced",
            "tabs {:?}",
            p.tabs
        );
    }

    // book_test.go TestCalledDrafts: what it's called is said everywhere it
    // speaks.
    #[test]
    fn called_drafts() {
        let mut bk = Book::new(Store::default(), 0);
        bk.w = words_for("Drafts");
        let r = bk.stash("s1", "one", b("x"), t0());
        let p = bk.pick("s1", t0());
        assert!(
            r.said.starts_with("kept as a draft")
                && p.title == "Drafts"
                && p.tabs[0] == "Drafts"
                && bk.note("s1").starts_with("kept as a draft"),
            "said {:?}, pick {:?} {:?}, note {:?}",
            r.said,
            p.title,
            p.tabs,
            bk.note("s1")
        );
        let mut all = vec![r.said.clone(), p.about.clone(), bk.note("s1")];
        all.extend(p.empty.clone());
        for s in all {
            assert!(!s.to_lowercase().contains("stash"), "says stash: {s:?}");
        }
    }
}
