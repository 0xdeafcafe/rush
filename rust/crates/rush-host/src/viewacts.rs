//! A view can be worked, not just read. A row a click opens shows its
//! detail and its buttons; each button does what the agent wrote on it:
//! sends the agent a message as if you'd typed it, opens a link, copies
//! words, or changes the row (ticked, its tone, its word) at once, with no
//! turn of the agent's. A change you make is kept with the view, over what
//! the agent says of that row, until the agent changes the same itself;
//! the agent hears of it at its next step.
//!
//! Ported from internal/host/viewacts.go.

use serde::{Deserialize, Serialize, Serializer};

use crate::views::{View, clip, cut, name_ok, tone};

/// A button: what it says, and what a click on it does, any of these at
/// once.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    /// A message to the agent, sent at once.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub send: String,
    /// A web link.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub open: String,
    /// Words to copy.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub copy: String,
    /// A change to its row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<Change>,
}

/// What a click changes on a row: None leaves it as it is. A tone or meta
/// of "" (taken off) is left out when written, as Go's omitempty leaves
/// out a pointer to an empty string.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Change {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    #[serde(skip_serializing_if = "empty", serialize_with = "some")]
    pub tone: Option<String>,
    #[serde(skip_serializing_if = "empty", serialize_with = "some")]
    pub meta: Option<String>,
}

fn empty(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(str::is_empty)
}

fn some<S: Serializer>(s: &Option<String>, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_str(s.as_deref().unwrap_or(""))
}

/// A change you made to the row of key, and what the agent had there then:
/// when the agent says otherwise, it has changed it itself, and its word
/// goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Yours {
    pub key: String,
    pub set: Change,
    pub was: Change,
}

const ACTIONS_MOST: usize = 6; // buttons on a row or a view
const DETAIL_MOST: usize = 1200; // characters in a row's detail
const YOURS_MOST: usize = 200;

/// as cut to size: each with a label, its link a web one, its change in
/// tones a view has.
pub(crate) fn check_actions(as_: &mut Vec<Action>) {
    as_.truncate(ACTIONS_MOST);
    as_.retain_mut(|a| {
        clip(&mut a.label);
        if a.label.chars().count() > 24 {
            a.label = cut(&a.label, 24);
        }
        tone(&mut a.tone);
        clip(&mut a.send);
        clip(&mut a.copy);
        if !a.open.starts_with("https://") && !a.open.starts_with("http://") {
            a.open.clear();
        }
        if let Some(c) = &mut a.set {
            if let Some(t) = &mut c.tone {
                tone(t);
            }
            if let Some(m) = &mut c.meta {
                clip(m);
            }
            if c.done.is_none() && c.tone.is_none() && c.meta.is_none() {
                a.set = None;
            }
        }
        !a.label.is_empty()
            && (!a.send.is_empty() || !a.open.is_empty() || !a.copy.is_empty() || a.set.is_some())
    });
}

/// A row's detail cut to size, its lines kept.
pub(crate) fn clip_detail(s: &mut String) {
    let lines: Vec<String> = s
        .split('\n')
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let t = lines.join("\n").trim().to_string();
    *s = cut(&t, DETAIL_MOST);
}

/// The key of a row under prefix: its id, else its text, else its first
/// cell.
pub fn item_key(prefix: &str, id: &str, text: &str, cells: &[String]) -> String {
    if !id.is_empty() {
        format!("{prefix}/{id}")
    } else if !text.is_empty() {
        format!("{prefix}/{text}")
    } else if let Some(c) = cells.first() {
        format!("{prefix}/{c}")
    } else {
        String::new()
    }
}

/// The prefix of block bi's rows.
pub fn block_key(bi: usize) -> String {
    format!("b{bi}")
}

/// The prefix of group gi's rows.
pub fn group_key(gi: usize) -> String {
    format!("g{gi}")
}

/// Where a row is in a view.
#[derive(Clone, Debug, PartialEq)]
enum Loc {
    Group(usize, usize),
    Item(usize, Vec<usize>),
}

/// A row's fields a change reaches, wherever it is in a view.
struct RowRef<'a> {
    text: String,
    /// None for a group's row: it can't be ticked.
    done: Option<&'a mut Option<bool>>,
    tone: &'a mut String,
    meta: &'a mut String,
}

impl View {
    /// Where the row of key is: the first of that key, groups first.
    fn find(&self, key: &str) -> Option<Loc> {
        for (gi, g) in self.groups.iter().enumerate() {
            for (i, r) in g.rows.iter().enumerate() {
                let k = item_key(&group_key(gi), &r.id, &r.text, &[]);
                if !k.is_empty() && k == key {
                    return Some(Loc::Group(gi, i));
                }
            }
        }
        fn walk(prefix: &str, items: &[crate::views::Item], key: &str, path: &mut Vec<usize>) -> bool {
            for (i, it) in items.iter().enumerate() {
                let k = item_key(prefix, &it.id, &it.text, &it.cells);
                if k.is_empty() {
                    continue;
                }
                path.push(i);
                if k == key || walk(&k, &it.items, key, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        for (bi, b) in self.blocks.iter().enumerate() {
            let mut path = Vec::new();
            if walk(&block_key(bi), &b.items, key, &mut path) {
                return Some(Loc::Item(bi, path));
            }
        }
        None
    }

    fn row(&mut self, key: &str) -> Option<RowRef<'_>> {
        match self.find(key)? {
            Loc::Group(gi, i) => {
                let r = &mut self.groups[gi].rows[i];
                Some(RowRef {
                    text: r.text.clone(),
                    done: None,
                    tone: &mut r.tone,
                    meta: &mut r.meta,
                })
            }
            Loc::Item(bi, path) => {
                let mut it = &mut self.blocks[bi].items[path[0]];
                for &i in &path[1..] {
                    it = &mut it.items[i];
                }
                let text = if it.text.is_empty() {
                    it.cells.first().cloned().unwrap_or_default()
                } else {
                    it.text.clone()
                };
                Some(RowRef {
                    text,
                    done: Some(&mut it.done),
                    tone: &mut it.tone,
                    meta: &mut it.meta,
                })
            }
        }
    }

    /// Puts your changes on v, to draw it as you left it.
    pub fn apply_yours(&mut self) {
        let ys = self.yours.clone();
        for y in ys {
            if let Some(r) = self.row(&y.key) {
                r.apply(&y.set);
            }
        }
    }
}

impl RowRef<'_> {
    /// What r has now of the fields c changes.
    fn now(&self, c: &Change) -> Change {
        let mut was = Change::default();
        if c.done.is_some()
            && let Some(d) = &self.done
        {
            was.done = Some(d.unwrap_or(false));
        }
        if c.tone.is_some() {
            was.tone = Some(self.tone.clone());
        }
        if c.meta.is_some() {
            was.meta = Some(self.meta.clone());
        }
        was
    }

    /// Makes c on r.
    fn apply(self, c: &Change) {
        if let (Some(d), Some(done)) = (c.done, self.done) {
            *done = Some(d);
        }
        if let Some(t) = &c.tone {
            *self.tone = t.clone();
        }
        if let Some(m) = &c.meta {
            *self.meta = m.clone();
        }
    }
}

/// The changes in ys that still stand on v as the agent has it now: each
/// field the agent hasn't changed from what it had when you made yours, on
/// a row it still has.
pub(crate) fn keep_yours(v: &mut View, ys: Vec<Yours>) -> Vec<Yours> {
    let mut out = Vec::new();
    for mut y in ys {
        let Some(r) = v.row(&y.key) else { continue };
        // What it had reads back as nothing when it was nought or "".
        let now = r.now(&y.set);
        if y.set.done.is_some() && now.done.is_none_or(|d| d != y.was.done.unwrap_or(false)) {
            y.set.done = None;
            y.was.done = None;
        }
        if y.set.tone.is_some() && now.tone.as_deref().unwrap_or("") != y.was.tone.as_deref().unwrap_or("") {
            y.set.tone = None;
            y.was.tone = None;
        }
        if y.set.meta.is_some() && now.meta.as_deref().unwrap_or("") != y.was.meta.as_deref().unwrap_or("") {
            y.set.meta = None;
            y.was.meta = None;
        }
        if y.set.done.is_some() || y.set.tone.is_some() || y.set.meta.is_some() {
            out.push(y);
        }
    }
    if out.len() > YOURS_MOST {
        out.drain(..out.len() - YOURS_MOST);
    }
    out
}

/// Makes c on the row of key in the session's view name, as you clicked
/// it: kept over the agent's until it changes the same, and told the agent
/// at its next step. It's the row's text, for what rush says.
pub fn change_row(id: &str, name: &str, key: &str, c: Change) -> Result<String, String> {
    if !name_ok(name) {
        return Err(format!("no view {name}"));
    }
    let p = crate::paths::dir(id).join("views").join(format!("{name}.json"));
    let b = std::fs::read(&p).map_err(|e| e.to_string())?;
    let mut v: View = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    let (text, tickable, was) = {
        let Some(r) = v.row(key) else {
            return Err("that row has gone from the view".into());
        };
        (r.text.clone(), r.done.is_some(), r.now(&c))
    };
    // What the agent has there: the same row's earlier change knows.
    let at = match v.yours.iter().rposition(|y| y.key == key) {
        Some(i) => i,
        None => {
            v.yours.push(Yours {
                key: key.to_string(),
                ..Yours::default()
            });
            v.yours.len() - 1
        }
    };
    let y = &mut v.yours[at];
    if c.done.is_some() {
        if y.set.done.is_none() {
            y.was.done = was.done;
        }
        y.set.done = c.done;
    }
    if c.tone.is_some() {
        if y.set.tone.is_none() {
            y.was.tone = was.tone.clone();
        }
        y.set.tone = c.tone.clone();
    }
    if c.meta.is_some() {
        if y.set.meta.is_none() {
            y.was.meta = was.meta.clone();
        }
        y.set.meta = c.meta.clone();
    }
    if c.done.is_some() && !tickable {
        return Err("that row can't be ticked".into());
    }
    crate::json::write_atomic(&p, &crate::json::line(&v)).map_err(|e| e.to_string())?;
    let _ = crate::inbox::notice(id, &change_words(&v.title, &text, &c));
    Ok(text)
}

/// Tells the agent what you changed on its view.
pub fn change_words(title: &str, text: &str, c: &Change) -> String {
    let mut did = Vec::new();
    if let Some(d) = c.done {
        did.push(if d { "ticked it off".to_string() } else { "unticked it".to_string() });
    }
    if let Some(t) = &c.tone {
        did.push(if t.is_empty() {
            "took its tone off".to_string()
        } else {
            format!("set its tone to {t}")
        });
    }
    if let Some(m) = &c.meta {
        did.push(format!("set its word to “{m}”"));
    }
    format!(
        "In your view “{title}”, the user clicked “{text}” and {}. The view keeps it; set the row the same when you next update the view, or say otherwise.",
        did.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;
    use crate::views::{Block, Item, save_view, views};

    // viewacts_test.go:10: a change you make to a row shows over the
    // agent's, stays when it saves the view again, and goes once the agent
    // changes the same itself.
    #[test]
    fn change_row_keeps_yours() {
        let _h = Home::new();
        std::fs::create_dir_all(crate::paths::dir("s1")).unwrap();
        let good = || Some("good".to_string());
        let view = |done: bool, tone: &str| View {
            name: "todo".into(),
            title: "To do".into(),
            blocks: vec![Block {
                kind: "checklist".into(),
                items: vec![
                    Item {
                        text: "write it".into(),
                        done: Some(done),
                        tone: tone.into(),
                        actions: vec![
                            Action { label: "approve".into(), set: Some(Change { tone: good(), ..Change::default() }), ..Action::default() },
                            Action { label: "nothing".into(), ..Action::default() },
                            Action { label: "bad".into(), open: "file:///x".into(), ..Action::default() },
                        ],
                        ..Item::default()
                    },
                    Item { text: "test it".into(), ..Item::default() },
                ],
                ..Block::default()
            }],
            ..View::default()
        };
        save_view("s1", view(false, "")).unwrap();
        let (vs, _) = views("s1");
        let as_ = &vs[0].blocks[0].items[0].actions;
        assert!(as_.len() == 1 && as_[0].label == "approve", "buttons that do nothing were kept: {as_:?}");
        let key = item_key(&block_key(0), "", "write it", &[]);
        change_row("s1", "todo", &key, Change { done: Some(true), tone: good(), meta: None }).unwrap();
        let it = || views("s1").0[0].blocks[0].items[0].clone();
        let got = it();
        assert!(got.done == Some(true) && got.tone == "good", "the change doesn't show: {got:?}");
        // The agent saves it again as it had it: yours stays.
        save_view("s1", view(false, "")).unwrap();
        let got = it();
        assert!(got.done == Some(true) && got.tone == "good", "the agent's save undid yours: {got:?}");
        // It changes the tone itself: its word goes, your tick stays.
        save_view("s1", view(false, "warn")).unwrap();
        let got = it();
        assert!(got.done == Some(true) && got.tone == "warn", "the agent's own change didn't win: {got:?}");
        change_row("s1", "todo", &key, Change { done: Some(false), ..Change::default() }).unwrap();
        assert_eq!(it().done, Some(false), "unticking didn't show");
        assert!(change_row("s1", "todo", "b0/gone", Change { done: Some(true), ..Change::default() }).is_err());
    }

    #[test]
    fn words_as_go_says_them() {
        let want = include_str!("../tests/fixtures/change_words.txt");
        let c = Change { done: Some(true), tone: Some(String::new()), meta: Some("ok".into()) };
        assert_eq!(change_words("To do", "write it", &c) + "\n", want);
    }
}
