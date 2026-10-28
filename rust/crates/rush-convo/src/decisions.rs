//! A decisions block: how an agent that needs you to decide something ends
//! its turn, read without a model.
//!
//! ```text
//! Decisions:
//! 1. **Topic:** (a) first option; (b) second option.
//! My picks: 1a 2b.
//! ```
//!
//! Only the last run of numbered items counts. Picks come from any line
//! that says "pick" or "picks", or from "(recommended)" on an option.
//! Port of internal/host/decisions.go:52-375 (Decision, DecisionOption,
//! statedPicks, DecisionsIn, preview, showWith, decision, decisionTopic,
//! decisionOption, distinctLabels).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// One numbered item of a decisions block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// Its number in the message.
    pub r#ref: String,
    pub topic: String,
    /// What's at stake, between the topic and its options.
    pub context: String,
    pub options: Vec<DecisionOption>,
}

/// One lettered option of a Decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionOption {
    /// Its letter.
    pub key: String,
    pub label: String,
    pub detail: String,
    /// What it would look like, shown beside it.
    pub preview: String,
    pub recommended: bool,
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

static ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(\d+)[.)]\s+(.*)$"));
static GO_ON: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{2,}\S"));
static OPT: LazyLock<Regex> = LazyLock::new(|| re(r"\(([a-z])\)\s*"));
static PICK: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bpicks?\b"));
static REF: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+)([a-z])\b"));
static REC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\((recommended|my pick)\)"));
static REC_IN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s*\((recommended|my pick)\)"));
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(```|~~~)"));
// "(a) tabs:" or "Preview of (b):" names the option a preview is of.
static SHOWS: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:\W*preview\b[^(]*)?\(([a-z])\)[^()]{0,60}$"));
// A short lead before ": ", "; ", " — " or ". " is the label.
static SPLIT: LazyLock<Regex> = LazyLock::new(|| re(r"^(.{2,70}?)(?::\s|;\s|\s[—–-]\s|\.\s)(.+)$"));
static HEAD: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^\W*decisions\b[^\n]*:\W*$"));

/// Card text with no markdown: inline code loses its backticks.
fn plain_text(s: &str) -> String {
    s.replace('`', "")
}

/// s without bold markers, its whitespace collapsed.
fn clean_text(s: &str) -> String {
    s.replace("**", "")
        .replace("__", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The picks text states, by decision number.
fn stated_picks(text: &str) -> HashMap<String, String> {
    let mut picks = HashMap::new();
    if !text.to_lowercase().contains("pick") {
        return picks;
    }
    for l in text.split_inclusive('\n') {
        if !PICK.is_match(l) {
            continue;
        }
        for m in REF.captures_iter(l) {
            picks.insert(m[1].to_string(), m[2].to_string());
        }
    }
    picks
}

/// A fenced block under a decision, and the option it's of, when the line
/// before it named one.
#[derive(Clone, Default)]
struct Preview {
    key: String,
    text: String,
    indent: usize,
}

impl Preview {
    /// Its text with the list's indent taken off.
    fn body(&self) -> String {
        self.text
            .trim_end_matches('\n')
            .split('\n')
            .map(|l| {
                let l = l.trim_end_matches(['\r', '\n']);
                let b = l.as_bytes();
                let mut cut = 0;
                while cut < self.indent && cut < b.len() && (b[cut] == b' ' || b[cut] == b'\t') {
                    cut += 1;
                }
                &l[cut..]
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct Pending {
    r#ref: String,
    body: String,
    shows: Vec<Preview>,
}

/// The last decisions block in text with what's around it: the text before
/// and after it, its "Decisions:" line and the picks line just after it
/// left out. With no decisions, before is all of text (decisions.go:125).
pub fn decisions_in(text: &str) -> (&str, Vec<Decision>, &str) {
    if !text.contains("(a)") {
        return (text, Vec::new(), ""); // every decision has an (a)
    }
    let mut last: Vec<Pending> = Vec::new();
    let mut block: Vec<Pending> = Vec::new();
    let (mut from, mut to, mut last_from, mut last_to, mut at) = (0, 0, 0, 0, 0);
    let mut fence: Option<Preview> = None;
    let (mut fence_mark, mut named, mut held) = (String::new(), String::new(), String::new());
    for raw in text.split_inclusive('\n') {
        let start = at;
        at += raw.len();
        let l = raw.trim_end_matches(['\r', '\n']);
        if let Some(f) = &mut fence {
            to = start + l.len();
            if l.trim() == fence_mark {
                let f = fence.take().unwrap();
                block.last_mut().unwrap().shows.push(f);
                continue;
            }
            f.text.push_str(l);
            f.text.push('\n');
            continue;
        }
        if let Some(m) = FENCE.captures(l)
            && !block.is_empty()
            && (!named.is_empty() || GO_ON.is_match(l))
        {
            fence = Some(Preview {
                key: std::mem::take(&mut named),
                text: String::new(),
                indent: l.len() - l.trim_start_matches([' ', '\t']).len(),
            });
            fence_mark = m[1].to_string();
            to = start + l.len();
            held.clear();
            continue;
        }
        if !held.is_empty() && l.trim().is_empty() {
            continue; // a blank line before its fence
        }
        if !held.is_empty() {
            // No fence after it: it carried the item on after all.
            let b = &mut block.last_mut().unwrap().body;
            b.push(' ');
            b.push_str(held.trim());
        }
        named.clear();
        held.clear();
        // A line naming an option the item already offered may be the
        // line before its preview.
        if let Some(m) = SHOWS.captures(l)
            && let Some(b) = block.last()
            && GO_ON.is_match(l)
            && b.body.contains(&format!("({})", &m[1]))
        {
            named = m[1].to_string();
            held = l.to_string();
            to = start + l.len();
            continue;
        }
        if let Some(m) = ITEM.captures(l) {
            if block.is_empty() {
                from = start;
            }
            block.push(Pending {
                r#ref: m[1].to_string(),
                body: m[2].to_string(),
                shows: Vec::new(),
            });
            to = start + l.len();
        } else if !block.is_empty() && GO_ON.is_match(l) {
            let b = &mut block.last_mut().unwrap().body;
            b.push(' ');
            b.push_str(l.trim());
            to = start + l.len();
        } else if !l.trim().is_empty() && !block.is_empty() {
            last = std::mem::take(&mut block);
            last_from = from;
            last_to = to;
        }
    }
    if !held.is_empty() {
        let b = &mut block.last_mut().unwrap().body;
        b.push(' ');
        b.push_str(held.trim());
    }
    if let Some(f) = fence
        && let Some(b) = block.last_mut()
    {
        b.shows.push(f); // a fence never closed runs to the end
    }
    if !block.is_empty() {
        last = block;
        last_from = from;
        last_to = to;
    }
    let mut ds = Vec::new();
    let mut picks: Option<HashMap<String, String>> = None;
    for it in &last {
        if !it.body.contains("(a)") {
            continue;
        }
        let picks = picks.get_or_insert_with(|| stated_picks(text));
        let picked = picks.get(&it.r#ref).map_or("", String::as_str);
        if let Some(mut d) = decision(&it.r#ref, &it.body, picked) {
            show_with(&mut d, &it.shows);
            ds.push(d);
        }
    }
    if ds.is_empty() {
        return (text, Vec::new(), "");
    }
    let mut before = text[..last_from].trim_end_matches([' ', '\t', '\r', '\n']);
    let mut after = text[last_to..].trim_start_matches([' ', '\t', '\r', '\n']);
    let i = before.rfind('\n').map_or(0, |i| i + 1);
    if HEAD.is_match(&before[i..]) {
        before = before[..i].trim_end_matches([' ', '\t', '\r', '\n']);
    }
    let (l, rest) = after.split_once('\n').unwrap_or((after, ""));
    if PICK.is_match(l) && REF.is_match(l) {
        after = rest.trim_start_matches([' ', '\t', '\r', '\n']);
    }
    (before, ds, after)
}

/// Gives d's options their previews: each to the option it names, the
/// rest in order to options without one.
fn show_with(d: &mut Decision, shows: &[Preview]) {
    let by_key: HashMap<String, usize> = d
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| (o.key.clone(), i))
        .collect();
    let mut loose = Vec::new();
    for p in shows {
        match by_key.get(&p.key) {
            Some(&i) if d.options[i].preview.is_empty() => d.options[i].preview = p.body(),
            _ => loose.push(p),
        }
    }
    let mut loose = loose.into_iter();
    for o in &mut d.options {
        if o.preview.is_empty() {
            match loose.next() {
                Some(p) => o.preview = p.body(),
                None => break,
            }
        }
    }
}

/// One numbered item's topic and lettered options; picked is the letter
/// picked for it, if any (decisions.go:268).
fn decision(r#ref: &str, body: &str, picked: &str) -> Option<Decision> {
    let at: Vec<_> = OPT.captures_iter(body).collect();
    let first = at.first()?.get(0).unwrap().start();
    let (topic, context) = decision_topic(&body[..first]);
    // Letters run a, b, c…: one out of sequence is a mention inside the
    // option before it.
    let mut opts: Vec<(String, String)> = Vec::new();
    for (i, m) in at.iter().enumerate() {
        let key = &m[1];
        let end = at
            .get(i + 1)
            .map_or(body.len(), |n| n.get(0).unwrap().start());
        let text = &body[m.get(0).unwrap().end()..end];
        let next = char::from(b'a' + opts.len() as u8);
        if key.len() == 1 && key.starts_with(next) {
            opts.push((key.to_string(), text.to_string()));
        } else {
            let o = opts.last_mut()?;
            o.1.push_str(&format!("({key}) {text}"));
        }
    }
    if topic.is_empty() || opts.len() < 2 {
        return None;
    }
    // Labels stay distinct: when shortening would repeat one, they're kept
    // whole; an option whose whole text repeats an earlier one is left out.
    let build = |split: bool| -> Vec<DecisionOption> {
        opts.iter()
            .map(|(k, t)| decision_option(k, t, picked == k, split))
            .collect()
    };
    let mut list = build(true);
    if !distinct_labels(&list) {
        list = build(false);
    }
    let mut seen = HashSet::new();
    list.retain(|o| seen.insert(o.label.clone()));
    if list.len() < 2 {
        return None;
    }
    Some(Decision {
        r#ref: r#ref.to_string(),
        topic,
        context,
        options: list,
    })
}

/// What comes before a decision's options as its topic, the bold lead or
/// what's before its first colon, and the context after it.
fn decision_topic(raw: &str) -> (String, String) {
    let raw = raw.trim();
    let (mut topic, mut context) = ("", "");
    if let Some(r) = raw.strip_prefix("**") {
        if let Some(i) = r.find("**")
            && i > 0
        {
            topic = &r[..i];
            context = &r[i + 2..];
        }
    } else if let Some(i) = raw.find(": ")
        && i > 0
        && i <= 70
    {
        topic = &raw[..i];
        context = &raw[i + 2..];
    }
    if topic.is_empty() {
        topic = raw;
    }
    let topic = plain_text(clean_text(topic).trim_end_matches([':', ' ', '\t']));
    let context = clean_text(context)
        .trim_start_matches([':', ' ', '\t'])
        .trim()
        .replace("**", "");
    (topic, context)
}

fn decision_option(key: &str, raw: &str, picked: bool, split: bool) -> DecisionOption {
    let mut text = clean_text(raw);
    if text.ends_with([';', ',', '.']) {
        text.pop();
    }
    let mut o = DecisionOption {
        key: key.to_string(),
        recommended: picked || REC.is_match(&text),
        ..DecisionOption::default()
    };
    let text = REC_IN.replace_all(&text, "").trim().to_string();
    o.label = plain_text(&text);
    // Never split inside inline code.
    if split
        && let Some(m) = SPLIT.captures(&text)
        && m[1].matches('`').count() % 2 == 0
    {
        o.label = plain_text(&m[1]);
        o.detail = plain_text(&m[2]);
    }
    o
}

fn distinct_labels(list: &[DecisionOption]) -> bool {
    let mut seen = HashSet::new();
    list.iter().all(|o| seen.insert(o.label.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_its_picks_and_what_is_around_it() {
        let text = "Done the first part.\n\nDecisions:\n1. **Remote:** which fixes it. (a) you run the command above; (b) allow me to change git remotes and I'll run it.\n2. **Renamed repos in rush:** (a) yes, make rush look up the current name; (b) no, the remote fix is enough.\nMy picks: 1a 2a.\n\nThanks.";
        let (before, ds, after) = decisions_in(text);
        assert_eq!(before, "Done the first part.");
        assert_eq!(after, "Thanks.");
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].topic, "Remote");
        assert_eq!(ds[0].context, "which fixes it.");
        assert_eq!(ds[0].options[0].label, "you run the command above");
        assert!(ds[0].options[0].recommended);
        assert_eq!(
            ds[0].options[1].label,
            "allow me to change git remotes and I'll run it"
        );
        // A comma doesn't split a label from its detail.
        assert_eq!(
            ds[1].options[0].label,
            "yes, make rush look up the current name"
        );
        assert!(!ds[1].options[1].recommended);
    }

    #[test]
    fn previews_and_plain_lists() {
        let text = "Decisions:\n1. **Look:** (a) tabs; (b) a list.\n   (b) a list:\n   ```\n   - one\n   ```\n";
        let (_, ds, _) = decisions_in(text);
        assert_eq!(ds[0].options[1].preview, "- one");
        assert!(ds[0].options[0].preview.is_empty());
        // Numbered lists with no options are left alone.
        let (b, ds, _) = decisions_in("Land it?\n1. Yes (a)\n2. No");
        assert!(ds.is_empty());
        assert_eq!(b, "Land it?\n1. Yes (a)\n2. No");
    }
}
