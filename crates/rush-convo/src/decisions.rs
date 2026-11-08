//! A decisions block: how an agent that needs you to decide something ends
//! its turn, read without a model.
//!
//! ```text
//! Decisions:
//! 1. **Topic:** (a) first option; (b) second option.
//! My picks: 1a 2b.
//! ```
//!
//! The run of numbered items under the last "Decisions" heading counts, or
//! with no heading the last run with lettered options. Picks come from the
//! heading and the block's picks line ("My picks: 1a 2b", "I'd go with
//! 1a"), or from "(recommended)" on an option.
//! Port of internal/host/decisions.go:52-375 (Decision, DecisionOption,
//! statedPicks, DecisionsIn, preview, showWith, decision, decisionTopic,
//! decisionOption, distinctLabels), with Go's misreadings fixed (see
//! DESIGN.md, "Decisions blocks"): options start only at a real "(a)",
//! option lines may sit at any indent, previews are named by the option
//! line above them, nested lists stay inside their item.

use std::borrow::Cow;
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

static ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"^(\s*)(\d+)[.)]\s+(.*)$"));
static GO_ON: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{2,}\S"));
// A line that is an option: "(a) …", "a) …", "- (B) …".
static OPT_LINE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:[-*•+]\s+)?(?:\(([a-zA-Z])\)|([a-zA-Z])\))(?:\s+|$)"));
static BULLET: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[-*•+]\s+"));
static PICK: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(?:picks?|recommend(?:s|ed|ation)?|go with|prefer|lean(?:ing)? (?:to|towards?))\b")
});
static REF: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(\d+)([a-z])\b"));
static REC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\((recommended|my pick)\)"));
static REC_IN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s*\((recommended|my pick)\)"));
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(`{3,}|~{3,})"));
// "Preview of (b):" names the option a preview is of.
static SHOWS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*(?:\W*preview\b[^(]*)?\(([a-z])\)[^()]{0,60}$"));
// A short lead before ": ", "; ", " — " or ". " is the label.
static SPLIT: LazyLock<Regex> = LazyLock::new(|| re(r"^(.{2,70}?)(?::\s|;\s|\s[—–-]\s|\.\s)(.+)$"));
// "Decisions:", "Decisions (my picks: 1a 2a):", "## Decisions", "**Decisions**".
static HEAD: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^\W*decisions\b(?:[^\n]*:)?\W*$"));

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

/// Whether l states picks: "My picks: 1a 2b", "I'd go with 1a".
fn picks_line(l: &str) -> bool {
    PICK.is_match(l) && REF.is_match(l)
}

/// The picks the given lines state, by decision number.
fn stated_picks<'a>(lines: impl IntoIterator<Item = &'a str>) -> HashMap<String, String> {
    let mut picks = HashMap::new();
    for l in lines {
        if !picks_line(l) {
            continue;
        }
        for m in REF.captures_iter(l) {
            picks.insert(m[1].to_string(), m[2].to_ascii_lowercase());
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

/// One numbered item as read: its lines joined by newlines, the lines that
/// say more of an option it already offered, its previews and where it is.
struct Pending {
    r#ref: String,
    body: String,
    /// "(a) …" lines under an item that already offered (a).
    extras: Vec<(String, String)>,
    shows: Vec<Preview>,
    start: usize,
    end: usize,
}

/// A run of numbered items, and the heading just above it.
struct Run {
    items: Vec<Pending>,
    indent: usize,
    head: Option<(usize, usize)>,
}

/// What to take back when a line naming an option turns out to be the line
/// before its preview.
enum Undo {
    Body(usize),
    Extra,
}

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start_matches([' ', '\t']).len()
}

/// Whether l closes a fence opened with n of ch.
fn closes(l: &str, ch: char, n: usize) -> bool {
    let t = l.trim();
    t.len() >= n && t.chars().all(|c| c == ch)
}

/// Where s's options start: each "(a)", "(B)" or "c)" that is a real one
/// (not inside a word or inline code), the letters running a, b, c…; one
/// out of sequence is a mention inside the text around it. Each is its
/// start, where its text starts, and its letter.
fn option_marks(s: &str) -> Vec<(usize, usize, char)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut code = false;
    let mut next = b'a';
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'`' {
            code = !code;
            i += 1;
            continue;
        }
        if code {
            i += 1;
            continue;
        }
        let mut found = None;
        if c == b'('
            && i + 2 < b.len()
            && b[i + 1].is_ascii_alphabetic()
            && b[i + 2] == b')'
            && (i == 0 || !word(b[i - 1]))
            && b.get(i + 3).is_none_or(|&n| !word(n))
        {
            found = Some((i, i + 3, b[i + 1].to_ascii_lowercase()));
        } else if c.is_ascii_alphabetic()
            && b.get(i + 1) == Some(&b')')
            && b.get(i + 2).is_none_or(|n| n.is_ascii_whitespace())
            && (i == 0 || (!word(b[i - 1]) && b[i - 1] != b'('))
            && bare_ok(s, i)
        {
            found = Some((i, i + 2, c.to_ascii_lowercase()));
        }
        if let Some((st, mut en, k)) = found {
            if k == next {
                while en < b.len() && b[en].is_ascii_whitespace() {
                    en += 1;
                }
                out.push((st, en, k as char));
                next += 1;
                i = en;
                continue;
            }
            i = st + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Whether a bare "a)" at i opens an option: at a line's start (after any
/// bullet), or after punctuation that ends what came before ("…; b) no").
fn bare_ok(s: &str, i: usize) -> bool {
    let line = &s[s[..i].rfind('\n').map_or(0, |n| n + 1)..i];
    if line.trim().is_empty() || BULLET.find(line).is_some_and(|m| m.end() == line.len()) {
        return true;
    }
    let lead = line.trim_end();
    lead.len() < line.len() && lead.ends_with([';', ':', ',', '.', '*', '—', '–'])
}

/// l with a bullet before its option taken off.
fn unbullet(l: &str) -> &str {
    let t = l.trim();
    match BULLET.find(t) {
        Some(m) if OPT_LINE.is_match(t) => &t[m.end()..],
        _ => t,
    }
}

fn push_line(body: &mut String, l: &str) {
    body.push('\n');
    body.push_str(unbullet(l));
}

/// The block a run holds and what's around it: the run's index into
/// runs, and the index after its last decision item.
fn chosen(runs: &[Run], read: &[Vec<Option<Decision>>]) -> Option<(usize, usize)> {
    let with: Vec<usize> = (0..runs.len())
        .filter(|&i| read[i].iter().any(Option::is_some))
        .collect();
    let i = with
        .iter()
        .rev()
        .find(|&&i| runs[i].head.is_some())
        .or(with.last())
        .copied()?;
    let last = read[i].iter().rposition(Option::is_some)? + 1;
    Some((i, last))
}

/// The decisions block in text with what's around it: the text before and
/// after it, its "Decisions:" line and its picks line left out. With no
/// decisions, before is all of text (decisions.go:125).
pub fn decisions_in(text: &str) -> (&str, Vec<Decision>, Cow<'_, str>) {
    let none = || (text, Vec::new(), Cow::Borrowed(""));
    if !text.contains("a)") && !text.contains("A)") {
        return none(); // every decision has an (a)
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut cur: Option<Run> = None;
    let mut head: Option<(usize, usize)> = None;
    let mut fence: Option<(Preview, char, usize)> = None;
    let mut outer: Option<(char, usize)> = None;
    let mut named = String::new();
    let mut undo: Option<Undo> = None;
    let mut at = 0;
    for raw in text.split_inclusive('\n') {
        let start = at;
        at += raw.len();
        let l = raw.trim_end_matches(['\r', '\n']);
        if let Some((ch, n)) = outer {
            if closes(l, ch, n) {
                outer = None;
            }
            continue;
        }
        if let Some((f, ch, n)) = &mut fence {
            let it = cur.as_mut().and_then(|r| r.items.last_mut()).unwrap();
            it.end = start + l.len();
            if closes(l, *ch, *n) {
                let (f, _, _) = fence.take().unwrap();
                it.shows.push(f);
                named.clear();
                continue;
            }
            f.text.push_str(l);
            f.text.push('\n');
            continue;
        }
        if l.trim().is_empty() {
            continue; // blank lines stay inside a run
        }
        let ind = indent_of(l);
        if let Some(m) = FENCE.captures(l) {
            let (ch, n) = (m[1].chars().next().unwrap(), m[1].len());
            if let Some(run) = &mut cur
                && (!named.is_empty() || GO_ON.is_match(l))
            {
                let it = run.items.last_mut().unwrap();
                match undo.take() {
                    Some(Undo::Body(n)) => it.body.truncate(n),
                    Some(Undo::Extra) => {
                        it.extras.pop();
                    }
                    None => {}
                }
                fence = Some((
                    Preview {
                        key: std::mem::take(&mut named),
                        text: String::new(),
                        indent: ind,
                    },
                    ch,
                    n,
                ));
                it.end = start + l.len();
                continue;
            }
            // A fence of its own: it ends the run, and what's in it is no
            // list.
            runs.extend(cur.take());
            head = None;
            outer = Some((ch, n));
            continue;
        }
        undo = None;
        if let Some(run) = &mut cur {
            let it = run.items.last_mut().unwrap();
            let end = start + l.len();
            if let Some(m) = OPT_LINE.captures(l) {
                let kc = m.get(1).or(m.get(2)).unwrap().as_str().as_bytes()[0];
                let kc = char::from(kc.to_ascii_lowercase());
                let key = kc.to_string();
                let marks = option_marks(&it.body);
                let offered = marks.iter().any(|&(_, _, k)| k == kc);
                let next = char::from(b'a' + marks.len().min(25) as u8);
                if offered {
                    // More about an option the item already offered, or the
                    // line naming the preview under it.
                    let rest = l[m.get(0).unwrap().end()..].trim().to_string();
                    if rest.len() <= 60 && !rest.contains(['(', ')']) {
                        undo = Some(Undo::Extra);
                    }
                    it.extras.push((key.clone(), rest));
                    named = key;
                    it.end = end;
                    continue;
                }
                if kc == next || ind >= 2 {
                    push_line(&mut it.body, l);
                    named = if kc == next { key } else { String::new() };
                    it.end = end;
                    continue;
                }
            }
            if let Some(m) = ITEM.captures(l)
                && m[1].len() <= run.indent
            {
                named.clear();
                run.items.push(Pending {
                    r#ref: m[2].to_string(),
                    body: m[3].to_string(),
                    extras: Vec::new(),
                    shows: Vec::new(),
                    start,
                    end,
                });
                continue;
            }
            if GO_ON.is_match(l) || ITEM.is_match(l) {
                // An indented line, a nested list's too, carries it on.
                if let Some(m) = SHOWS.captures(l) {
                    let k = char::from(m[1].as_bytes()[0].to_ascii_lowercase());
                    if option_marks(&it.body).iter().any(|&(_, _, c)| c == k) {
                        undo = Some(Undo::Body(it.body.len()));
                        named = k.to_string();
                    }
                }
                push_line(&mut it.body, l);
                it.end = end;
                continue;
            }
            runs.extend(cur.take());
            named.clear();
        }
        if let Some(m) = ITEM.captures(l) {
            cur = Some(Run {
                items: vec![Pending {
                    r#ref: m[2].to_string(),
                    body: m[3].to_string(),
                    extras: Vec::new(),
                    shows: Vec::new(),
                    start,
                    end: start + l.len(),
                }],
                indent: m[1].len(),
                head: head.take(),
            });
            continue;
        }
        head = HEAD.is_match(l).then_some((start, at));
    }
    if let Some((f, _, _)) = fence
        && let Some(it) = cur.as_mut().and_then(|r| r.items.last_mut())
    {
        it.shows.push(f); // a fence never closed runs to the end
    }
    runs.extend(cur.take());
    // Lazy "1. 1. 1." numbering: number them as they come.
    for run in &mut runs {
        let mut seen = HashSet::new();
        if !run.items.iter().all(|it| seen.insert(it.r#ref.clone())) {
            for (i, it) in run.items.iter_mut().enumerate() {
                it.r#ref = (i + 1).to_string();
            }
        }
    }
    let read: Vec<Vec<Option<Decision>>> = runs
        .iter()
        .map(|r| r.items.iter().map(read_item).collect())
        .collect();
    let Some((ri, last)) = chosen(&runs, &read) else {
        return none();
    };
    let run = &runs[ri];
    let from = run.head.map_or(run.items[0].start, |h| h.0);
    let to = run.items[last - 1].end;
    let before = text[..from].trim_end_matches([' ', '\t', '\r', '\n']);
    let tail = text[to..].trim_start_matches([' ', '\t', '\r', '\n']);
    // Its picks line: the first after it, wherever it is.
    let mut pick_lines: Vec<&str> = run.head.iter().map(|h| &text[h.0..h.1]).collect();
    let mut after = Cow::Borrowed(tail);
    let mut off = 0;
    for l in tail.split_inclusive('\n') {
        if picks_line(l) {
            pick_lines.push(l);
            let rest = tail[off + l.len()..].trim_start_matches([' ', '\t', '\r', '\n']);
            after = if off == 0 {
                Cow::Borrowed(rest)
            } else {
                let lead = &tail[..off];
                let mut s = lead.to_string();
                if rest.is_empty() {
                    s.truncate(lead.trim_end().len());
                }
                s.push_str(rest);
                Cow::Owned(s)
            };
            break;
        }
        off += l.len();
    }
    if stated_picks(pick_lines.iter().copied()).is_empty() {
        // A picks line just above the block.
        pick_lines.extend(before.rsplit('\n').next());
    }
    let picks = stated_picks(pick_lines);
    let mut ds = Vec::new();
    for d in read[ri][..last].iter().flatten() {
        let mut d = d.clone();
        if let Some(k) = picks.get(&d.r#ref) {
            for o in &mut d.options {
                o.recommended |= &o.key == k;
            }
        }
        ds.push(d);
    }
    (before, ds, after)
}

/// One item read as a decision, its previews given out.
fn read_item(it: &Pending) -> Option<Decision> {
    let mut d = decision(&it.r#ref, &it.body, &it.extras)?;
    show_with(&mut d, &it.shows);
    Some(d)
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

/// One numbered item's topic and lettered options, extras the lines that
/// say more of an option (decisions.go:268).
fn decision(r#ref: &str, body: &str, extras: &[(String, String)]) -> Option<Decision> {
    let at = option_marks(body);
    let first = at.first()?.0;
    let (mut topic, context) = decision_topic(&body[..first].replace('\n', " "));
    if topic.is_empty() {
        topic = format!("Decision {}", r#ref);
    }
    let mut opts: Vec<(String, String)> = at
        .iter()
        .enumerate()
        .map(|(i, &(_, s, k))| {
            let end = at.get(i + 1).map_or(body.len(), |n| n.0);
            (k.to_string(), body[s..end].to_string())
        })
        .collect();
    for (k, more) in extras {
        let Some(o) = opts.iter_mut().find(|o| &o.0 == k) else {
            continue;
        };
        // "(a) tabs: what it costs" under "(a) tabs": only what's new.
        let label = decision_option(k, &o.1, true).label;
        let mut more = clean_text(more);
        if let Some(rest) = more.get(label.len()..)
            && more[..label.len()].eq_ignore_ascii_case(&label)
        {
            more = rest
                .trim_start_matches([':', ';', ',', '.', '—', '–', '-', ' '])
                .to_string();
        }
        if more.is_empty() {
            continue;
        }
        let t = clean_text(&o.1);
        let t = t.trim_end_matches([';', ',', '.', ' ']);
        let sep = if SPLIT.is_match(t) { ". " } else { ": " };
        o.1 = format!("{t}{sep}{more}");
    }
    if opts.len() < 2 {
        return None;
    }
    // Labels stay distinct: when shortening would repeat one, they're kept
    // whole; an option whose whole text repeats an earlier one is left out.
    let build = |split: bool| -> Vec<DecisionOption> {
        opts.iter()
            .map(|(k, t)| decision_option(k, t, split))
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

fn decision_option(key: &str, raw: &str, split: bool) -> DecisionOption {
    let mut text = clean_text(raw);
    // A bullet left before the next option's line.
    for b in [" -", " *", " •", " +"] {
        if let Some(t) = text.strip_suffix(b) {
            text = t.to_string();
        }
    }
    if text.ends_with([';', ',', '.']) {
        text.pop();
    }
    let mut o = DecisionOption {
        key: key.to_string(),
        recommended: REC.is_match(&text),
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
