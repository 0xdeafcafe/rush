//! The board: kanban-code's cards, as `~/.kanban-code/links.json` holds
//! them. kanban-code's app owns the file; the plugin only reads it, and
//! changes cards through the kanban CLI.

use std::fmt;
use std::io::{BufReader, Read};
use std::path::PathBuf;

use rush_plugin_sdk::proto::{Session, nullable};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// The part of a kanban-code card (a "link" in links.json) this plugin
/// reads.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Card {
    #[serde(deserialize_with = "nullable")]
    pub id: String,
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    #[serde(deserialize_with = "nullable")]
    pub project_path: String,
    #[serde(deserialize_with = "nullable")]
    pub column: String,
    #[serde(deserialize_with = "nullable")]
    pub manually_archived: bool,
    #[serde(deserialize_with = "nullable")]
    pub parent_card_id: String,
    #[serde(deserialize_with = "nullable")]
    pub prompt_body: String,
    #[serde(deserialize_with = "nullable")]
    pub last_activity: String,
    pub sort_order: Option<i64>,
    #[serde(deserialize_with = "nullable")]
    pub updated_at: String,
    pub session_link: Option<SessionLink>,
    pub worktree_link: Option<WorktreeLink>,
    pub issue_link: Option<IssueLink>,
    #[serde(deserialize_with = "nullable")]
    pub pr_links: Vec<Pr>,
    #[serde(deserialize_with = "nullable")]
    pub queued_prompts: Vec<QueuedPrompt>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionLink {
    #[serde(deserialize_with = "nullable")]
    pub session_id: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct WorktreeLink {
    #[serde(deserialize_with = "nullable")]
    pub path: String,
    #[serde(deserialize_with = "nullable")]
    pub branch: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct IssueLink {
    #[serde(deserialize_with = "nullable")]
    pub number: i64,
    #[serde(deserialize_with = "nullable")]
    pub url: String,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    #[serde(deserialize_with = "nullable")]
    pub body: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QueuedPrompt {
    #[serde(deserialize_with = "nullable")]
    pub body: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pr {
    #[serde(deserialize_with = "nullable")]
    pub number: i64,
    #[serde(deserialize_with = "nullable")]
    pub url: String,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    #[serde(deserialize_with = "nullable")]
    pub status: String,
    #[serde(deserialize_with = "nullable")]
    pub unresolved_threads: i64,
    #[serde(rename = "firstUnresolvedThreadURL")]
    #[serde(deserialize_with = "nullable")]
    pub first_unresolved_thread_url: String,
    #[serde(deserialize_with = "nullable")]
    pub check_runs: Vec<CheckRun>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CheckRun {
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    #[serde(deserialize_with = "nullable")]
    pub status: String,
    #[serde(deserialize_with = "nullable")]
    pub conclusion: String,
}

impl Pr {
    /// The names of its checks that finished badly.
    pub fn failing(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .check_runs
            .iter()
            .filter(|c| {
                matches!(
                    c.conclusion.as_str(),
                    "failure" | "timed_out" | "cancelled" | "action_required"
                )
            })
            .map(|c| c.name.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The columns in board order, as kanban-code names and shows them.
pub const COLUMNS: &[(&str, &str)] = &[
    ("in_progress", "In Progress"),
    ("requires_attention", "Waiting"),
    ("in_review", "In Review"),
    ("backlog", "Backlog"),
    ("done", "Done"),
];

pub fn column_title(id: &str) -> &str {
    COLUMNS
        .iter()
        .find(|(c, _)| *c == id)
        .map_or(id, |&(_, t)| t)
}

/// Where kanban-code keeps its files: `$KANBAN_CODE_HOME`, which the
/// manifest sets, else `~/.kanban-code`.
pub fn kanban_home() -> PathBuf {
    match std::env::var_os("KANBAN_CODE_HOME").filter(|h| !h.is_empty()) {
        Some(h) => PathBuf::from(h),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".kanban-code"),
    }
}

pub fn links_path() -> PathBuf {
    kanban_home().join("links.json")
}

/// The board: every card that's on it, not archived, and not a subagent of
/// another.
pub fn read_cards() -> Result<Vec<Card>, String> {
    let path = links_path();
    let f = std::fs::File::open(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "no kanban-code board at {}: is kanban-code installed, and KANBAN_CODE_HOME right in plugin.json?",
                kanban_home().display()
            )
        } else {
            e.to_string()
        }
    })?;
    parse_cards(BufReader::with_capacity(64 << 10, f))
}

/// Reads the board from links.json's contents. A card at a time: the file
/// holds every conversation kanban-code has seen, and can be tens of
/// megabytes, most of it cards left off here.
pub fn parse_cards(r: impl Read) -> Result<Vec<Card>, String> {
    let mut de = serde_json::Deserializer::from_reader(r);
    let cards = Board::deserialize(&mut de)
        .and_then(|b| de.end().map(|()| b))
        .map_err(|e| format!("reading links.json: {e}"))?;
    Ok(cards.0)
}

/// links.json's top object, keeping only its links.
struct Board(Vec<Card>);

impl<'de> Deserialize<'de> for Board {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Board, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Board;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object with links")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Board, A::Error> {
                let mut out = Vec::new();
                while let Some(k) = m.next_key::<String>()? {
                    if k == "links" {
                        m.next_value_seed(Links(&mut out))?;
                    } else {
                        m.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(Board(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// The links, each card read and kept or dropped before the next.
struct Links<'a>(&'a mut Vec<Card>);

impl<'de> DeserializeSeed<'de> for Links<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Links<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list of cards")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<(), A::Error> {
        while let Some(c) = s.next_element::<Card>()? {
            if !c.manually_archived && c.parent_card_id.is_empty() && c.column != "all_sessions" {
                self.0.push(c);
            }
        }
        Ok(())
    }
}

impl Card {
    pub fn title(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if let Some(i) = &self.issue_link
            && !i.title.is_empty()
        {
            return i.title.clone();
        }
        if !self.prompt_body.is_empty() {
            return first_line(&self.prompt_body);
        }
        self.id.clone()
    }

    /// The Claude Code conversation kanban-code linked it to, or "".
    pub fn session_id(&self) -> &str {
        self.session_link
            .as_ref()
            .map_or("", |l| l.session_id.as_str())
    }
}

/// A card by id, id prefix, or name, as the kanban CLI finds them.
pub fn find_card<'a>(cards: &'a [Card], q: &str) -> Result<&'a Card, String> {
    let q = q.trim();
    let mut by_prefix = Vec::new();
    for c in cards {
        if c.id == q {
            return Ok(c);
        }
        if c.id.starts_with(q) {
            by_prefix.push(c);
        }
    }
    if let [c] = by_prefix[..] {
        return Ok(c);
    }
    let lower = q.to_lowercase();
    cards
        .iter()
        .find(|c| c.title().to_lowercase() == lower)
        .ok_or_else(|| format!("no card {q:?} on the board"))
}

/// The card a session works on: the one it was started for, the one
/// kanban-code linked to its conversation, or the one whose worktree it's
/// in.
pub fn card_of<'a>(cards: &'a [Card], s: &Session) -> Option<&'a Card> {
    if let Some(id) = s.meta.get("card").filter(|id| !id.is_empty())
        && let Some(c) = cards.iter().find(|c| &c.id == id)
    {
        return Some(c);
    }
    if !s.session_id.is_empty()
        && let Some(c) = cards.iter().find(|c| c.session_id() == s.session_id)
    {
        return Some(c);
    }
    cards.iter().find(|c| {
        c.worktree_link
            .as_ref()
            .is_some_and(|w| !w.path.is_empty() && within(&s.cwd, &w.path))
    })
}

fn within(dir: &str, root: &str) -> bool {
    let root = root.strip_suffix('/').unwrap_or(root);
    dir == root || dir.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// A branch and folder name made from a card's title.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut s = out.trim_matches('-').to_string();
    if s.len() > 40 {
        s.truncate(40);
        s = s.trim_end_matches('-').to_string();
    }
    if s.is_empty() {
        return "card".into();
    }
    s
}

pub fn first_line(s: &str) -> String {
    let s = s.trim();
    let s = s.split_once('\n').map_or(s, |(a, _)| a);
    if s.len() > 80 {
        let mut end = 80;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        return format!("{}…", &s[..end]);
    }
    s.to_string()
}

/// A card in full, for the agent working on it.
pub fn describe(c: &Card) -> String {
    let mut b = format!(
        "Card {}: {}\nColumn: {}\n",
        c.id,
        c.title(),
        column_title(&c.column)
    );
    if !c.project_path.is_empty() {
        b += &format!("Project: {}\n", c.project_path);
    }
    if let Some(wt) = &c.worktree_link {
        b += &format!("Worktree: {} (branch {})\n", wt.path, wt.branch);
    }
    if let Some(is) = &c.issue_link {
        b += &format!("\nIssue #{}: {}\n{}\n", is.number, is.title, is.url);
        let body = is.body.trim();
        if !body.is_empty() {
            b += &format!("\n{body}\n");
        }
    }
    let p = c.prompt_body.trim();
    if !p.is_empty() {
        b += &format!("\nTask:\n{p}\n");
    }
    for p in &c.pr_links {
        b += &format!("\nPR #{}", p.number);
        if !p.title.is_empty() {
            b += &format!(": {}", p.title);
        }
        b.push('\n');
        if !p.url.is_empty() {
            b += &format!("{}\n", p.url);
        }
        if !p.status.is_empty() {
            b += &format!("Status: {}\n", p.status);
        }
        let f = p.failing();
        if !f.is_empty() {
            b += &format!("Failing checks: {}\n", f.join(", "));
        }
        if p.unresolved_threads > 0 {
            b += &format!(
                "Unresolved review threads: {} (first: {})\n",
                p.unresolved_threads, p.first_unresolved_thread_url
            );
        }
    }
    if !c.queued_prompts.is_empty() {
        b += "\nQueued for later:\n";
        for q in &c.queued_prompts {
            b += &format!("- {}\n", first_line(&q.body));
        }
    }
    b
}

/// The board, a column at a time; only column, if it names one.
pub fn board(cards: &[Card], column: &str) -> String {
    let mut b = String::new();
    for (id, title) in COLUMNS {
        if !column.is_empty() && *id != column {
            continue;
        }
        let mut in_col: Vec<&Card> = cards.iter().filter(|c| c.column == *id).collect();
        if in_col.is_empty() {
            continue;
        }
        sort_column(&mut in_col);
        b += &format!("## {title}\n");
        for c in in_col {
            b += &format!("- {}  {}", c.id, c.title());
            if let Some(wt) = &c.worktree_link
                && !wt.branch.is_empty()
            {
                b += &format!("  [{}]", wt.branch);
            }
            for p in &c.pr_links {
                b += &format!("  PR #{} {}", p.number, p.status);
            }
            b.push('\n');
        }
        b.push('\n');
    }
    if b.is_empty() {
        return "The board is empty.".into();
    }
    b.trim().to_string()
}

/// A column's cards in the order kanban-code shows them: those given a
/// place first, by it, then the most recently active.
pub fn sort_column(cards: &mut [&Card]) {
    use std::cmp::Ordering;
    cards.sort_by(|a, b| match (a.sort_order, b.sort_order) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => {
            let ta = or(&a.last_activity, &a.updated_at);
            let tb = or(&b.last_activity, &b.updated_at);
            tb.cmp(ta).then_with(|| a.id.cmp(&b.id))
        }
    });
}

pub fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINKS: &str = r#"{"links": [
  {"id": "card_aaa111", "name": "Fix login", "column": "in_progress", "projectPath": "/src/app",
   "sessionLink": {"sessionId": "s-1"}, "worktreeLink": {"path": "/src/app/.claude/worktrees/fix-login", "branch": "fix-login"},
   "prLinks": [{"number": 7, "status": "failing", "unresolvedThreads": 1, "firstUnresolvedThreadURL": "https://gh/t/1",
     "checkRuns": [{"name": "lint", "status": "completed", "conclusion": "failure"}, {"name": "test", "status": "completed", "conclusion": "success"}]}],
   "manualOverrides": {}, "manuallyArchived": false, "source": "manual", "isRemote": false, "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-01T00:00:00Z"},
  {"id": "card_bbb222", "column": "backlog", "projectPath": "/src/app", "issueLink": {"number": 12, "title": "Dark mode", "body": "Please."},
   "manualOverrides": {}, "manuallyArchived": false, "source": "githubIssue", "isRemote": false, "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-02T00:00:00Z"},
  {"id": "card_ccc333", "name": "Old", "column": "done", "manuallyArchived": true, "manualOverrides": {}, "source": "manual", "isRemote": false, "createdAt": "x", "updatedAt": "x"},
  {"id": "card_ddd444", "name": "A subagent", "column": "in_progress", "parentCardId": "card_aaa111", "manualOverrides": {}, "manuallyArchived": false, "source": "manual", "isRemote": false, "createdAt": "x", "updatedAt": "x"},
  {"id": "card_eee555", "name": "Discovered", "column": "all_sessions", "manualOverrides": {}, "manuallyArchived": false, "source": "discovered", "isRemote": false, "createdAt": "x", "updatedAt": "x"}
], "settings": {"anything": [1, 2, {"x": null}]}}"#;

    fn test_board() -> Vec<Card> {
        parse_cards(LINKS.as_bytes()).unwrap()
    }

    #[test]
    fn reads_cards() {
        let cards = test_board();
        assert_eq!(
            cards.len(),
            2,
            "archived, subagent and unfiled cards should be left off: {cards:?}"
        );
        let b = board(&cards, "");
        assert!(
            b.contains("## In Progress\n- card_aaa111  Fix login  [fix-login]  PR #7 failing")
                && b.contains("## Backlog\n- card_bbb222  Dark mode"),
            "board =\n{b}"
        );
        assert!(!board(&cards, "backlog").contains("Fix login"));
        assert!(
            parse_cards(&b"{\"links\": [}"[..])
                .unwrap_err()
                .starts_with("reading links.json")
        );
        assert!(parse_cards(&b"{}"[..]).unwrap().is_empty());
    }

    #[test]
    fn finds_a_sessions_card() {
        let cards = test_board();
        let s = |meta: Option<&str>, session_id: &str, cwd: &str| Session {
            meta: meta
                .map(|m| [("card".to_string(), m.to_string())].into())
                .unwrap_or_default(),
            session_id: session_id.into(),
            cwd: cwd.into(),
            ..Default::default()
        };
        for (name, sess, want) in [
            (
                "meta",
                s(Some("card_bbb222"), "s-1", ""),
                Some("card_bbb222"),
            ),
            ("session", s(None, "s-1", ""), Some("card_aaa111")),
            (
                "worktree",
                s(None, "", "/src/app/.claude/worktrees/fix-login/web"),
                Some("card_aaa111"),
            ),
            (
                "none",
                s(None, "", "/src/app/.claude/worktrees/fix-login-2"),
                None,
            ),
        ] {
            assert_eq!(
                card_of(&cards, &sess).map(|c| c.id.as_str()),
                want,
                "{name}"
            );
        }
        for (q, want) in [
            ("card_bbb222", "card_bbb222"),
            ("card_a", "card_aaa111"),
            ("dark MODE", "card_bbb222"),
        ] {
            assert_eq!(find_card(&cards, q).map(|c| c.id.as_str()), Ok(want), "{q}");
        }
        assert!(
            find_card(&cards, "card_").is_err(),
            "an ambiguous prefix found a card"
        );
    }

    #[test]
    fn describes_a_card() {
        let cards = test_board();
        let d = describe(&cards[0]);
        for want in [
            "Column: In Progress",
            "PR #7",
            "Failing checks: lint",
            "Unresolved review threads: 1 (first: https://gh/t/1)",
        ] {
            assert!(d.contains(want), "describe lacks {want:?}:\n{d}");
        }
    }

    #[test]
    fn slugs_and_lines() {
        assert_eq!(slug("Fix the Login — again!"), "fix-the-login-again");
        assert_eq!(slug("!!!"), "card");
        assert_eq!(slug(&"abc ".repeat(20)).len(), 39);
        assert_eq!(first_line("  one\ntwo"), "one");
        assert_eq!(first_line(&"é".repeat(50)).chars().count(), 41);
    }
}
