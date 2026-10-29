//! Cards: what a command did that outlives it (a commit, a push, a pull
//! request, a merge, work thrown away, tests or a build that failed), read
//! from the command and its output and drawn under the step's row. Port of
//! internal/convo/cards.go (card, tone, verb, cardsOf, commitCards,
//! commitMessages, expands, messageBody, flagValues, blankHeredocs,
//! argValue, pushCard, repoName, prCreateCard, prMergeCard, mergeCard,
//! firstArg, discardCards, testCard, buildCard, dedent, joinCards, card,
//! failRows, listRows, box, frameBox, testsFailed) and
//! history_settled.go:249 (superseded). What git itself is asked
//! (commits.go: gitCommits, gitMerge) isn't ported: cards come from the
//! text alone.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::{Color, Style};

use super::CAP_ROW;
use super::draw::Drawer;
use super::shell::{shell_tokens, unquote};
use super::style::*;
use super::txt::{Txt, blanks};
use super::udiff::expand_tabs;
use crate::model::{Status, Step, StepId};
use rush_core::tool::Kind;

/// Starts the ref of a card's first row: a pull request ("pr:" and its
/// link) or a commit ("commit:" and its hash).
pub const ENTITY_REF: &str = "ent:";

/// A thing a command did that outlives it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Card {
    /// "commit", "push", "pr", "tests", "build", "merge", "discard".
    pub kind: &'static str,
    pub sha: String,
    pub branch: String,
    pub subject: String,
    pub body: Vec<String>,
    pub files: usize,
    pub add: usize,
    pub del: usize,
    pub amend: bool,
    pub root: bool,
    /// Co-authors.
    pub with: Vec<String>,
    pub remote: String,
    pub refs: Vec<Pushed>,
    pub num: String,
    pub url: String,
    pub state: &'static str,
    pub title: String,
    pub head: String,
    pub base: String,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub fails: Vec<Failure>,
    pub how: &'static str,
    pub conflicts: Vec<String>,
    /// A pull request's commits and push, joined into its card; a
    /// commit's push.
    pub joined: Vec<Card>,
    pub pushed: String,
    pub what: String,
    pub lost: Vec<String>,
    pub undo: String,
    /// Set aside, or deleted safely: yellow rather than red.
    pub warn: bool,
}

/// A test that failed and what it said, or a package that didn't build.
#[derive(Clone, Debug, Default)]
pub(crate) struct Failure {
    pub name: String,
    pub msg: String,
    pub build: bool,
}

/// One ref a push moved: main -> main, 88c42b1..3225847.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pushed {
    pub from: String,
    pub to: String,
    pub span: String,
    pub forced: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    None,
    Made,
    Warn,
    Lost,
}

impl Card {
    /// What the card's action did, for its colour (cards.go:80).
    fn tone(&self) -> Tone {
        match self.kind {
            "commit" if self.amend => Tone::Warn,
            "commit" => Tone::Made,
            "push" => {
                let mut t = Tone::None;
                for p in &self.refs {
                    if p.span == "deleted" {
                        return Tone::Lost;
                    } else if p.forced {
                        t = Tone::Warn;
                    } else if t == Tone::None && p.span.starts_with("new ") {
                        t = Tone::Made;
                    }
                }
                t
            }
            "pr" if self.state == "draft" => Tone::None,
            "pr" => Tone::Made,
            "tests" | "build" => Tone::Lost,
            "merge" if !self.conflicts.is_empty() || !self.sha.is_empty() => Tone::Warn,
            "discard" if self.warn => Tone::Warn,
            "discard" => Tone::Lost,
            _ => Tone::None,
        }
    }

    /// The card's command, for a folded run's list of steps (cards.go:124).
    pub fn verb(&self) -> String {
        match self.kind {
            "push" => "git push".into(),
            "pr" if self.state == "merged" => "gh pr merge".into(),
            "pr" => "gh pr create".into(),
            "tests" => "tests".into(),
            "build" => self.what.clone(),
            "merge" | "discard" => {
                format!("git {}", self.what.split_whitespace().next().unwrap_or(""))
            }
            _ => "git commit".into(),
        }
    }
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

macro_rules! lazy_re {
    ($($name:ident = $pat:expr;)*) => {
        $(static $name: LazyLock<Regex> = LazyLock::new(|| re($pat));)*
    };
}

lazy_re! {
    GIT_VERB = r"\bgit\s+(?:-[Cc]\s+\S+\s+)*(commit|cherry-pick|revert|push|merge|rebase|pull|reset|branch|stash|clean|checkout|restore)\b";
    GH_VERB = r"\bgh\s+pr\s+(create|merge)\b";
    BUILD_VERB = r"\b(?:go (?:build|install|vet)|(?:vue-)?tsc|nx|(?:npm|pnpm|yarn|bun) (?:run )?(?:build|dev|start|typecheck)\S*)\b";
    TEST_VERB = r"\b(?:(?:go|cargo|npm|pnpm|yarn|bun|deno|make|mix|dotnet) (?:run )?test\S*|pytest|jest|vitest|rspec)\b";
    COMMIT_HEAD = r"^\[(.+?) ([0-9a-f]{7,40})\] (.*)$";
    COMMIT_STAT = r"^ (\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?";
    PUSH_TO = r"^To (\S+)$";
    PUSH_REF = r"^ ([ +*-]) (\[[^\]]+\]|\S+)\s+(\S+) -> (\S+)";
    PR_URL = r"https://([^/\s]+/[^/\s]+/[^/\s]+)/pull/(\d+)";
    PR_CREATING = r"(?m)^Creating (?:draft )?pull request for (\S+) into (\S+) in ";
    PR_MERGED = r"(?m)^✓ (Squashed and merged|Rebased and merged|Merged) pull request \S*?#(\d+) \((.*)\)$";
    PR_DELETED = r"(?m)^✓ Deleted (?:local |remote )?branch (\S+)";
    MSG_FLAG = r"(?:^|\s)(?:-a?m|--message)(?:=|\s*)";
    TRAILER = r"(?i)^(co-authored-by|signed-off-by):\s*(.*?)\s*(?:<[^>]*>)?\s*$";
    HEREDOC_ARG = r#"^"?\$\(cat\s*<<-?\s*['"]?(\w+)['"]?"#;
    STDIN_MSG = r"(?:^|\s)(?:-F\s*|--file[=\s])-(?:\s|$)";
    STDIN_DOC = r#"<<-?\s*['"]?(\w+)['"]?"#;
    QUIET_FLAG = r"(?:^|\s)(?:-q|--quiet)(?:\s|$)|>\s*/dev/null";
    SHELL_SUB = r"\$[A-Za-z_{(0-9@*#?]|`";
    LOG_ONELINE = r"^([0-9a-f]{7,40}) (?:\([^)]*\) )?(.*)$";
    TITLE_FLAG = r"\s(?:--title|-t)(?:=|\s+)";
    BODY_FLAG = r"\s(?:--body|-b)(?:=|\s+)";
    BASE_FLAG = r"\s(?:--base|-B)(?:=|\s+)";
    HEAD_FLAG = r"\s(?:--head|-H)(?:=|\s+)";
    DRAFT_FLAG = r"\s(?:--draft|-d)\b";
    MERGE_HOW = r"\s--(squash|rebase|merge)\b|\s-([srm])\b";
    PR_ARG = r"\bgh\s+pr\s+merge\s+(?:\S*/pull/)?#?(\d+)\b";
    CONFLICT = r"^CONFLICT \([^)]*\): (?:Merge conflict in )?(.+)$";
    STOPPED = r"^(?:error: )?[Cc]ould not apply ([0-9a-f]{7,40})\.\.\. ?(.*)$";
    REBASED = r"^Successfully rebased and updated (?:refs/heads/)?(\S+?)\.?$";
    MERGED_BY = r"^Merge made by the '(\S+)' strategy\.$";
    HEAD_NOW = r"^HEAD is now at ([0-9a-f]{7,40}) ?(.*)$";
    BRANCH_DEL = r"^Deleted (?:remote-tracking )?branch (\S+) \(was ([0-9a-f]{7,40})\)\.$";
    STASH_SAVE = r"^Saved working directory and index state (.*)$";
    STASH_DROP = r"^Dropped (\S+) \(([0-9a-f]{7,40})\)$";
    REMOVING = r"^Removing (.+)$";
    GO_FAIL_T = r"^\s*--- FAIL: (\S+)";
    GO_PASS_T = r"^\s*--- PASS: ";
    GO_SKIP_T = r"^\s*--- SKIP: ";
    GO_BUILD_F = r"^FAIL\s+(\S+) \[(build failed|setup failed)\]";
    GO_ERR_LINE = r"^\S+\.go:\d+:\d+: .+";
    GO_END = r"^\s*(?:---|===) |^(?:FAIL|ok|PASS)\b|^panic: |^exit status";
    PY_FAILED = r"^(?:FAILED|ERROR) (\S+?)(?: - (.*))?$";
    PY_SUMMARY = r"^=+ (.*\d+ (?:failed|passed|error).*) in [\d.]+s";
    JEST_SUM = r"^Tests:\s+(.*\d.*)$";
    VITEST_SUM = r"^\s*Tests\s+(.*\d+ (?:failed|passed).*)$";
    CARGO_SUM = r"^test result: \w+\. (.*)$";
    CARGO_FAIL = r"^test (\S+) \.\.\. FAILED$";
    JEST_FAIL = r"^\s+● (.+ › .+)$";
    VITEST_FAIL = r"^\s*FAIL\s+(.+ > .+?)(?:\s+\[.*\])?$";
    VITEST_X = r"^\s*[×✗]\s+(.+?)(?:\s+\d+m?s)?$";
    VITEST_FILE = r"^\s*(?:❯|>|FAIL)\s+(\S+\.\w+)\s+\((\d+) tests?\s*\|\s*(\d+) failed";
    AN_ERROR = r"^\s*(?:\w*(?:Error|Exception)\b:?|[Ee]xpected\b|assert(?:ion)?\b).*";
    TEST_TALLY = r"(\d+) (failed|passed|skipped|errors?|ignored)\b";
    SRC_LOC = r"^(?:\S*/)?([^/\s]+\.\w+:\d+)(?::\d+)?:\s*";
    TSC_PAREN = r"^(\S+?)\((\d+),\d+\): error TS\d+: (.*)$";
    TSC_PRETTY = r"^(\S+?):(\d+):\d+ - error TS\d+: (.*)$";
    TSC_FOUND = r"^Found (\d+) errors?\b";
    ESBUILD_E = r"^\s*[✘X] \[ERROR\] (.*?)(?:\s+\[plugin .*\])?$";
    ESBUILD_AT = r"^\s+(\S+:\d+):\d+:\s*$";
    WEBPACK_E = r"^ERROR in (\S+?)(?: (\d+):\d+(?:-\d+)?)?$";
    NX_FAILED = r"^\s*-\s+(\S+:\S+)\s*$";
}

fn num(m: Option<regex::Match>) -> usize {
    m.and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

fn cap<'h>(c: &regex::Captures<'h>, i: usize) -> &'h str {
    c.get(i).map_or("", |m| m.as_str())
}

/// One git command of a chain: its verb, and its arguments up to the end
/// of that command, space either side.
struct GitCall {
    verb: String,
    args: String,
}

fn git_calls(cmd: &str, blank: &str) -> Vec<GitCall> {
    GIT_VERB
        .captures_iter(blank)
        .map(|c| {
            let (v, end) = (c.get(1).unwrap(), c.get(0).unwrap().end());
            let mut args = cmd.get(end..).unwrap_or("");
            if let Some(i) = args.find([';', '&', '|', '\n']) {
                args = &args[..i];
            }
            GitCall {
                verb: cmd.get(v.range()).unwrap_or("").to_string(),
                args: format!(" {} ", args.trim()),
            }
        })
        .collect()
}

/// s with each heredoc's lines blanked out, byte for byte, so what's found
/// in it is where it is in s (cards.go:478).
pub(crate) fn blank_heredocs(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("<<") {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut b = s.as_bytes().to_vec();
    for c in STDIN_DOC.captures_iter(s) {
        let end = c.get(0).unwrap().end();
        let Some(nl) = s[end..].find('\n') else {
            continue;
        };
        let tag = cap(&c, 1);
        let mut i = end + nl + 1;
        while i < s.len() {
            let j = s[i..].find('\n').unwrap_or(s.len() - i);
            if s[i..i + j].trim() == tag {
                break;
            }
            b[i..i + j].fill(b' ');
            i += j + 1;
        }
    }
    // Whole lines become spaces, so it's still UTF-8.
    std::borrow::Cow::Owned(
        String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    )
}

/// Strips colour codes, as Go's stripANSI does: only "ESC [ digits m".
fn strip_sgr(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\x1b') {
        return std::borrow::Cow::Borrowed(s);
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b && b.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
                j += 1;
            }
            if b.get(j) == Some(&b'm') {
                i = j + 1;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    std::borrow::Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// The cards of a finished shell command, read from what it ran and
/// printed (cards.go:205).
pub(crate) fn cards_of(st: &Step, cmd: &str, out: &str) -> Vec<Card> {
    if st.kind != Kind::Shell || matches!(st.status, Status::Running | Status::Waiting) {
        return Vec::new();
    }
    let blank = blank_heredocs(cmd);
    let calls = git_calls(cmd, &blank);
    let mut verbs: Vec<String> = calls.iter().map(|c| c.verb.clone()).collect();
    for c in GH_VERB.captures_iter(&blank) {
        verbs.push(format!("pr {}", cap(&c, 1)));
    }
    let tests = TEST_VERB.is_match(&blank);
    let build = BUILD_VERB.find(&blank).map_or("", |m| m.as_str());
    if verbs.is_empty() && !tests && build.is_empty() {
        return Vec::new();
    }
    let out = strip_sgr(out);
    // Progress redraws itself with \r; each redraw reads as a line.
    let lines: Vec<&str> = out.split(['\n', '\r']).filter(|l| !l.is_empty()).collect();
    let has = |vs: &[&str]| verbs.iter().any(|x| vs.contains(&x.as_str()));
    let ok = st.status == Status::Ok;
    let mut cs = Vec::new();
    if has(&["merge", "rebase", "pull"])
        && let Some(c) = merge_card(&lines, &calls)
    {
        cs.push(c);
    }
    if has(&["commit", "cherry-pick", "revert"]) {
        cs.extend(commit_cards(&lines, cmd, ok));
    }
    if has(&["push"])
        && let Some(c) = push_card(&lines)
    {
        cs.push(c);
    }
    if has(&["pr create"])
        && let Some(c) = pr_create_card(&out, cmd)
    {
        cs.push(c);
    }
    if has(&["pr merge"])
        && let Some(c) = pr_merge_card(&out, cmd, ok)
    {
        cs.push(c);
    }
    cs.extend(discard_cards(&lines, &calls, ok));
    if tests && let Some(c) = test_card(&lines) {
        cs.push(c);
    }
    // A test run that didn't build already says why.
    if !build.is_empty()
        && !cs
            .iter()
            .any(|c| c.kind == "tests" && c.fails.iter().any(|f| f.build))
        && let Some(c) = build_card(build, &lines)
    {
        cs.push(c);
    }
    cs
}

/// Each commit git reported, or one told to be quiet read from its
/// message (cards.go:288).
fn commit_cards(lines: &[&str], cmd: &str, ok: bool) -> Vec<Card> {
    let mut cs = Vec::new();
    let msgs = commit_messages(cmd);
    for (i, l) in lines.iter().enumerate() {
        let Some(m) = COMMIT_HEAD.captures(l) else {
            continue;
        };
        let mut c = Card {
            kind: "commit",
            branch: cap(&m, 1).to_string(),
            sha: cap(&m, 2).to_string(),
            subject: cap(&m, 3).to_string(),
            amend: cmd.contains("--amend"),
            ..Card::default()
        };
        if let Some(b) = c.branch.strip_suffix(" (root-commit)") {
            c.branch = b.to_string();
            c.root = true;
        }
        // An amend's Date:, an unset identity's Committer: come first.
        for n in &lines[i + 1..lines.len().min(i + 6)] {
            if let Some(s) = COMMIT_STAT.captures(n) {
                c.files = num(s.get(1));
                c.add = num(s.get(2));
                c.del = num(s.get(3));
                break;
            }
            if COMMIT_HEAD.is_match(n) {
                break;
            }
        }
        for msg in &msgs {
            let subj = msg.text.split('\n').next().unwrap_or("");
            if subj.trim() == c.subject.trim() {
                (c.body, c.with) = message_body(&msg.text);
                break;
            }
        }
        cs.push(c);
    }
    if !ok {
        return cs;
    }
    for msg in msgs.iter().filter(|m| m.quiet) {
        let subj = msg.text.split('\n').next().unwrap_or("");
        let mut c = Card {
            kind: "commit",
            subject: subj.trim().to_string(),
            amend: msg.amend,
            ..Card::default()
        };
        (c.body, c.with) = message_body(&msg.text);
        for l in lines {
            if let Some(m) = LOG_ONELINE.captures(l.trim())
                && cap(&m, 2).trim() == c.subject
            {
                c.sha = cap(&m, 1).to_string();
                break;
            }
        }
        cs.push(c);
    }
    cs
}

struct CommitMsg {
    text: String,
    quiet: bool,
    amend: bool,
}

/// The message each git commit in cmd was given, with -m or from a
/// heredoc (cards.go:363).
fn commit_messages(cmd: &str) -> Vec<CommitMsg> {
    let mut out = Vec::new();
    let blank = blank_heredocs(cmd);
    let locs: Vec<_> = GIT_VERB.captures_iter(&blank).collect();
    for (i, loc) in locs.iter().enumerate() {
        let end = locs
            .get(i + 1)
            .map_or(cmd.len(), |n| n.get(0).unwrap().start());
        if cmd.get(loc.get(1).unwrap().range()) != Some("commit") {
            continue;
        }
        let Some(args) = cmd.get(loc.get(0).unwrap().end()..end) else {
            continue;
        };
        let first = args.split('\n').next().unwrap_or("");
        // What's past the command's own line is a heredoc's, not its flags.
        let flags = match first.find([';', '&', '|']) {
            Some(k) => &first[..k],
            None => first,
        };
        let mut parts = Vec::new();
        let mut opaque = false;
        for m in MSG_FLAG.find_iter(&blank_heredocs(args)) {
            let rest = &args[m.end()..];
            if let Some(v) = arg_value(rest) {
                opaque = opaque || expands(rest, &v);
                parts.push(v.trim().to_string());
            }
        }
        if parts.is_empty()
            && STDIN_MSG.is_match(flags)
            && let Some(m) = STDIN_DOC.captures(first)
            && let Some(v) = arg_value(&format!("$(cat <<'{}'{}", cap(&m, 1), &args[first.len()..]))
        {
            parts.push(v.trim().to_string());
        }
        // "$msg" is a message only the shell knew.
        if opaque {
            continue;
        }
        if !parts.is_empty() {
            out.push(CommitMsg {
                text: parts.join("\n\n"),
                quiet: QUIET_FLAG.is_match(first),
                amend: flags.contains("--amend"),
            });
        }
    }
    out
}

/// Whether the shell put something into v, an argument given as raw.
fn expands(raw: &str, v: &str) -> bool {
    if raw.starts_with('\'') {
        return false;
    }
    if HEREDOC_ARG.is_match(raw) {
        let tag = raw.split('\n').next().unwrap_or("");
        let after = tag.find("<<").map_or("", |i| &tag[i..]);
        return !after.contains(['\'', '"']) && SHELL_SUB.is_match(v);
    }
    SHELL_SUB.is_match(v)
}

/// A message's body, without its subject or trailers, and who it was
/// written with (cards.go:436).
fn message_body(msg: &str) -> (Vec<String>, Vec<String>) {
    let mut keep: Vec<String> = Vec::new();
    let mut with = Vec::new();
    for l in msg.split('\n').skip(1) {
        let l = l.trim_end_matches([' ', '\t']);
        if let Some(m) = TRAILER.captures(l.trim()) {
            if cap(&m, 1).eq_ignore_ascii_case("co-authored-by") && !cap(&m, 2).is_empty() {
                with.push(cap(&m, 2).to_string());
            }
            continue;
        }
        if l.contains("Generated with [Claude Code]") {
            continue;
        }
        keep.push(l.to_string());
    }
    let lo = keep
        .iter()
        .position(|l| !l.trim().is_empty())
        .unwrap_or(keep.len());
    let hi = keep
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(lo, |i| i + 1);
    (keep[lo..hi.max(lo)].to_vec(), with)
}

/// The value given after each match of flag in s.
fn flag_values(s: &str, flag: &Regex) -> Vec<String> {
    flag.find_iter(&blank_heredocs(s))
        .filter_map(|m| arg_value(s.get(m.end()..)?))
        .collect()
}

/// An argument's value: a heredoc fed through $(cat <<'EOF' … EOF), a
/// quoted string or a bare word (cards.go:507).
fn arg_value(s: &str) -> Option<String> {
    if let Some(m) = HEREDOC_ARG.captures(s) {
        let (_, rest) = s.split_once('\n')?;
        let tag = cap(&m, 1);
        let mut b: Vec<&str> = Vec::new();
        for l in rest.split('\n') {
            if l.trim() == tag {
                return Some(b.join("\n"));
            }
            b.push(l);
        }
        return None;
    }
    let bs = s.as_bytes();
    let q = *bs.first()?;
    if q == b'"' || q == b'\'' {
        let mut out = Vec::new();
        let mut i = 1;
        while i < bs.len() {
            let c = bs[i];
            if c == q {
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
            if q == b'"' && c == b'\\' && i + 1 < bs.len() && b"\"\\$`".contains(&bs[i + 1]) {
                i += 1;
                out.push(bs[i]);
            } else {
                out.push(c);
            }
            i += 1;
        }
        return None;
    }
    match s.split_whitespace().next() {
        Some(f) if !f.starts_with('-') => Some(f.to_string()),
        _ => None,
    }
}

/// git's report of a push: "To host:o/r.git" and the refs it moved.
fn push_card(lines: &[&str]) -> Option<Card> {
    let mut c = Card {
        kind: "push",
        ..Card::default()
    };
    for l in lines {
        if let Some(m) = PUSH_TO.captures(l) {
            c.remote = repo_name(cap(&m, 1));
            continue;
        }
        let Some(m) = PUSH_REF.captures(l) else {
            continue;
        };
        if c.remote.is_empty() {
            continue;
        }
        let mut p = Pushed {
            from: cap(&m, 3).into(),
            to: cap(&m, 4).into(),
            forced: cap(&m, 1) == "+",
            span: cap(&m, 2).trim_matches(['[', ']']).into(),
        };
        if cap(&m, 1) == "-" {
            p.span = "deleted".into();
        }
        c.refs.push(p);
    }
    (!c.refs.is_empty()).then_some(c)
}

/// A remote as you'd say it: github.com/o/r.
fn repo_name(u: &str) -> String {
    let mut u = u.trim_end_matches('/');
    u = u.strip_suffix(".git").unwrap_or(u);
    let mut scheme = false;
    if let Some(i) = u.find("://") {
        u = &u[i + 3..];
        scheme = true;
    }
    let (host, path) = u.split_once('/').unwrap_or((u, ""));
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let mut out = host.to_string();
    if !path.is_empty() {
        out.push('/');
        out.push_str(path);
    }
    if !scheme {
        out = out.replacen(':', "/", 1);
    }
    out
}

/// gh pr create: the address gh printed, and what the command gave it.
fn pr_create_card(out: &str, cmd: &str) -> Option<Card> {
    let m = PR_URL.captures_iter(out).last()?;
    if out.contains("already exists") {
        return None;
    }
    let args = cmd.split_once("gh pr create").map_or("", |(_, a)| a);
    let mut c = Card {
        kind: "pr",
        state: "open",
        url: format!("{}/pull/{}", cap(&m, 1), cap(&m, 2)),
        num: cap(&m, 2).into(),
        ..Card::default()
    };
    let spaced = format!(" {args}");
    if DRAFT_FLAG.is_match(&spaced) {
        c.state = "draft";
    }
    let first = |f: &Regex| {
        flag_values(&spaced, f)
            .first()
            .map_or(String::new(), |v| v.trim().to_string())
    };
    c.title = first(&TITLE_FLAG);
    c.base = first(&BASE_FLAG);
    c.head = first(&HEAD_FLAG);
    let b = first(&BODY_FLAG);
    if !b.is_empty() {
        c.body = message_body(&format!("\n{b}")).0;
    }
    if let Some(w) = PR_CREATING.captures(out) {
        if c.head.is_empty() {
            c.head = cap(&w, 1).into();
        }
        if c.base.is_empty() {
            c.base = cap(&w, 2).into();
        }
    }
    Some(c)
}

/// gh pr merge: what gh said it merged, or the command's own number.
fn pr_merge_card(out: &str, cmd: &str, ok: bool) -> Option<Card> {
    let mut c = Card {
        kind: "pr",
        state: "merged",
        ..Card::default()
    };
    if let Some(m) = MERGE_HOW.captures(cmd) {
        c.base = match (cap(&m, 1), cap(&m, 2)) {
            ("squash", _) | (_, "s") => "squashed",
            ("rebase", _) | (_, "r") => "rebased",
            _ => "",
        }
        .into();
    }
    if let Some(m) = PR_MERGED.captures(out) {
        c.num = cap(&m, 2).into();
        c.title = cap(&m, 3).into();
        if cap(&m, 1).starts_with("Squashed") {
            c.base = "squashed".into();
        } else if cap(&m, 1).starts_with("Rebased") {
            c.base = "rebased".into();
        }
    } else if let Some(m) = PR_ARG.captures(cmd)
        && ok
        && !cmd.contains("--auto")
    {
        c.num = cap(&m, 1).into();
    } else {
        return None;
    }
    if let Some(m) = PR_DELETED.captures(out) {
        c.head = cap(&m, 1).into();
    }
    Some(c)
}

/// A merge, rebase or pull that did something (cards.go:660).
fn merge_card(lines: &[&str], calls: &[GitCall]) -> Option<Card> {
    let mut c = Card {
        kind: "merge",
        ..Card::default()
    };
    for g in calls {
        if matches!(g.verb.as_str(), "merge" | "rebase" | "pull") {
            c.what = g.verb.clone();
            c.branch = first_arg(
                &g.args,
                &["-m", "-X", "-s", "-F", "--strategy", "--strategy-option"],
            );
        }
    }
    let mut stat = None;
    for (i, l) in lines.iter().enumerate() {
        if let Some(m) = CONFLICT.captures(l) {
            let f = cap(&m, 1);
            if !c.conflicts.iter().any(|x| x == f) {
                c.conflicts.push(f.into());
            }
        } else if let Some(m) = STOPPED.captures(l) {
            c.sha = cap(&m, 1).into();
            c.subject = cap(&m, 2).into();
        } else if *l == "Fast-forward" {
            c.how = "fast-forward";
            stat = Some(i);
        } else if MERGED_BY.is_match(l) {
            c.how = "merge commit";
            stat = Some(i);
        } else if let Some(m) = REBASED.captures(l) {
            c.how = "rebased";
            c.head = cap(&m, 1).into();
        }
    }
    if let Some(s) = stat {
        for l in &lines[s + 1..] {
            if let Some(m) = COMMIT_STAT.captures(l) {
                c.files = num(m.get(1));
                c.add = num(m.get(2));
                c.del = num(m.get(3));
                break;
            }
        }
    }
    (!c.what.is_empty() && (!c.how.is_empty() || !c.conflicts.is_empty() || !c.sha.is_empty()))
        .then_some(c)
}

/// The first word of args that isn't a flag or a flag's value.
fn first_arg(args: &str, valued: &[&str]) -> String {
    let f: Vec<&str> = shell_tokens(args)
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .collect();
    let mut i = 0;
    while i < f.len() {
        if valued.contains(&f[i]) {
            i += 1;
        } else if !f[i].starts_with('-') {
            return unquote(f[i]).to_string();
        }
        i += 1;
    }
    String::new()
}

fn short_sha(s: &str) -> &str {
    s.get(..7).unwrap_or(s)
}

/// git throwing work away or setting it aside (cards.go:713).
fn discard_cards(lines: &[&str], calls: &[GitCall], ok: bool) -> Vec<Card> {
    let mut cs = Vec::new();
    let find = |r: &Regex| -> Vec<regex::Captures<'_>> {
        lines.iter().filter_map(|l| r.captures(l)).collect()
    };
    let mut done: HashSet<&str> = HashSet::new();
    for g in calls {
        if done.contains(g.verb.as_str()) {
            continue;
        }
        let flag = |fs: &[&str]| fs.iter().any(|f| g.args.contains(&format!(" {f} ")));
        let discard = |what: &str| Card {
            kind: "discard",
            what: what.into(),
            ..Card::default()
        };
        match g.verb.as_str() {
            "reset" => {
                let m = find(&HEAD_NOW);
                if flag(&["--hard"])
                    && let Some(last) = m.last()
                {
                    cs.push(Card {
                        sha: cap(last, 1).into(),
                        subject: cap(last, 2).into(),
                        undo: "git reset --hard HEAD@{1}".into(),
                        ..discard("reset --hard")
                    });
                }
            }
            "branch" => {
                if !flag(&["-d", "-D", "--delete"]) {
                    continue;
                }
                let safe = !flag(&["-D", "--force", "-f"]);
                for m in find(&BRANCH_DEL) {
                    cs.push(Card {
                        branch: cap(&m, 1).into(),
                        sha: cap(&m, 2).into(),
                        warn: safe,
                        undo: format!("git branch {} {}", cap(&m, 1), short_sha(cap(&m, 2))),
                        ..discard(if safe { "branch -d" } else { "branch -D" })
                    });
                }
            }
            "stash" => match first_arg(&g.args, &[]).as_str() {
                "" | "push" | "save" => {
                    if let Some(m) = find(&STASH_SAVE).first() {
                        cs.push(Card {
                            subject: cap(m, 1).into(),
                            warn: true,
                            undo: "git stash pop".into(),
                            ..discard("stash")
                        });
                    }
                }
                "drop" => {
                    for m in find(&STASH_DROP) {
                        cs.push(Card {
                            branch: cap(&m, 1).into(),
                            sha: cap(&m, 2).into(),
                            undo: format!("git stash apply {}", short_sha(cap(&m, 2))),
                            ..discard("stash drop")
                        });
                    }
                }
                "clear" if ok => cs.push(discard("stash clear")),
                _ => {}
            },
            "clean" => {
                let gone: Vec<String> = find(&REMOVING).iter().map(|m| cap(m, 1).into()).collect();
                if !gone.is_empty() {
                    cs.push(Card {
                        lost: gone,
                        ..discard("clean")
                    });
                }
            }
            "checkout" | "restore" => {
                // Over your changes: checkout -- paths, checkout ., or a
                // restore of the working tree.
                let over = if g.verb == "restore" {
                    !flag(&["--staged", "-S"]) || flag(&["--worktree", "-W"])
                } else {
                    flag(&["--"]) || first_arg(&g.args, &[]) == "."
                };
                if !ok || !over {
                    continue;
                }
                let after = match g.args.split_once(" -- ") {
                    Some((_, a)) => format!(" {a}"),
                    None => g.args.clone(),
                };
                let mut paths: Vec<String> = after
                    .split_whitespace()
                    .filter(|f| !f.starts_with('-'))
                    .map(|f| unquote(f).to_string())
                    .collect();
                if g.verb == "checkout" && !g.args.contains(" -- ") && paths.len() > 1 {
                    paths.remove(0); // checkout <ref> . reads from ref
                }
                if !paths.is_empty() {
                    cs.push(Card {
                        lost: paths,
                        ..discard(&g.verb)
                    });
                }
            }
            _ => continue,
        }
        done.insert(g.verb.as_str());
    }
    cs
}

/// A test run that failed: the tally, and which failed with what each
/// said. A run that passed is no card (cards.go:844).
fn test_card(lines: &[&str]) -> Option<Card> {
    let mut c = Card {
        kind: "tests",
        what: "test".into(),
        ..Card::default()
    };
    let mut tallied = false;
    let mut add = |c: &mut Card, s: &str| {
        tallied = true;
        for m in TEST_TALLY.captures_iter(s) {
            let n = num(m.get(1));
            match cap(&m, 2) {
                "failed" | "error" | "errors" => c.failed += n,
                "passed" => c.passed += n,
                _ => c.skipped += n,
            }
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut fail = |c: &mut Card, mut f: Failure| {
        if seen.insert(f.name.clone()) {
            f.msg = f.msg.trim().to_string();
            c.fails.push(f);
        }
    };
    // The first line after i that says something.
    let next = |i: usize| -> String {
        lines[i + 1..lines.len().min(i + 4)]
            .iter()
            .map(|n| n.trim())
            .find(|n| !n.is_empty())
            .unwrap_or("")
            .to_string()
    };
    let mut files: Vec<Failure> = Vec::new();
    let (mut go_pass, mut go_skip) = (0, 0);
    for (i, &l) in lines.iter().enumerate() {
        if let Some(m) = GO_FAIL_T.captures(l) {
            // What the test logged, up to the next test or the package's end.
            let mut msg: Vec<&str> = Vec::new();
            for &n in &lines[i + 1..] {
                if GO_END.is_match(n) || msg.len() == 6 {
                    break;
                }
                msg.push(n);
            }
            // Run with -v, it logged before it said it failed.
            if dedent(&msg).trim().is_empty() {
                msg.clear();
                let mut j = i as isize - 1;
                while j >= 0
                    && !GO_END.is_match(lines[j as usize])
                    && lines[j as usize].starts_with(' ')
                    && msg.len() < 6
                {
                    msg.insert(0, lines[j as usize]);
                    j -= 1;
                }
            }
            let f = Failure {
                name: cap(&m, 1).into(),
                msg: dedent(&msg),
                build: false,
            };
            fail(&mut c, f);
        } else if GO_PASS_T.is_match(l) {
            go_pass += 1;
        } else if GO_SKIP_T.is_match(l) {
            go_skip += 1;
        } else if let Some(m) = GO_BUILD_F.captures(l) {
            // The compiler's errors under "# pkg", each where it is.
            let mut msg: Vec<String> = Vec::new();
            for j in (0..i).rev() {
                if lines[j].starts_with("# ") {
                    break;
                }
                if GO_ERR_LINE.is_match(lines[j]) {
                    msg.insert(0, SRC_LOC.replace(lines[j], "${1}: ").into_owned());
                }
            }
            if msg.is_empty() {
                msg.push(cap(&m, 2).into());
            }
            let pkg = cap(&m, 1);
            let name = pkg.rsplit_once('/').map_or(pkg, |(_, n)| n);
            msg.truncate(4);
            fail(
                &mut c,
                Failure {
                    name: name.into(),
                    msg: msg.join("\n"),
                    build: true,
                },
            );
        } else if let Some(m) = PY_FAILED.captures(l) {
            fail(
                &mut c,
                Failure {
                    name: cap(&m, 1).into(),
                    msg: cap(&m, 2).into(),
                    build: false,
                },
            );
        } else if let Some(m) = CARGO_FAIL.captures(l) {
            fail(
                &mut c,
                Failure {
                    name: cap(&m, 1).into(),
                    ..Failure::default()
                },
            );
        } else if let Some(m) = JEST_FAIL.captures(l) {
            fail(
                &mut c,
                Failure {
                    name: cap(&m, 1).replace(" › ", " > "),
                    msg: next(i),
                    build: false,
                },
            );
        } else if let Some(m) = VITEST_FILE.captures(l) {
            files.push(Failure {
                name: cap(&m, 1).into(),
                msg: format!("{} of {} failed", cap(&m, 3), cap(&m, 2)),
                build: false,
            });
        } else if let Some(m) = VITEST_FAIL.captures(l) {
            fail(
                &mut c,
                Failure {
                    name: cap(&m, 1).into(),
                    msg: next(i),
                    build: false,
                },
            );
        } else if let Some(m) = VITEST_X.captures(l) {
            let n = next(i);
            let msg = match n.strip_prefix('→') {
                Some(r) => r.trim().to_string(),
                None => String::new(),
            };
            fail(
                &mut c,
                Failure {
                    name: cap(&m, 1).into(),
                    msg,
                    build: false,
                },
            );
        } else if let Some(m) = PY_SUMMARY.captures(l) {
            add(&mut c, cap(&m, 1));
        } else if let Some(m) = JEST_SUM.captures(l) {
            add(&mut c, cap(&m, 1));
        } else if let Some(m) = VITEST_SUM.captures(l) {
            add(&mut c, cap(&m, 1));
        } else if let Some(m) = CARGO_SUM.captures(l) {
            add(&mut c, cap(&m, 1));
        }
    }
    // A failed Go subtest fails its parent too, and vitest names a failure
    // once as it runs and again in full: the one that says more stays.
    let mut keep = Vec::new();
    for k in 0..c.fails.len() {
        let mut dup = false;
        for g in 0..c.fails.len() {
            let (fname, gname) = (&c.fails[k].name, &c.fails[g].name);
            if gname.starts_with(&format!("{fname}/")) {
                dup = true;
            }
            if gname.ends_with(&format!(" > {fname}")) {
                dup = true;
                if c.fails[g].msg.is_empty() {
                    c.fails[g].msg = c.fails[k].msg.clone();
                }
            }
        }
        if !dup {
            keep.push(c.fails[k].clone());
        }
    }
    c.fails = keep;
    if !tallied {
        c.failed = c.fails.len();
        c.passed = go_pass;
        c.skipped = go_skip;
    }
    // Named no test: which files failed, else the first thing that reads
    // like an error.
    if c.failed > 0 && c.fails.is_empty() {
        c.fails = files;
        if c.fails.is_empty()
            && let Some(l) = lines.iter().find(|l| AN_ERROR.is_match(l))
        {
            c.fails = vec![Failure {
                msg: l.trim().into(),
                ..Failure::default()
            }];
        }
    }
    (c.failed > 0 || !c.fails.is_empty()).then_some(c)
}

/// A build or type check that failed: each error, where it is first
/// (cards.go:1013).
fn build_card(verb: &str, lines: &[&str]) -> Option<Card> {
    let mut c = Card {
        kind: "build",
        what: verb.into(),
        ..Card::default()
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |c: &mut Card, file: &str, line: &str, msg: &str| {
        let at = if file.is_empty() {
            String::new()
        } else {
            let base = file.rsplit_once('/').map_or(file, |(_, b)| b);
            format!("{base}:{line}: ")
        };
        let msg = msg.trim();
        let k = format!("{at}{msg}");
        if !msg.is_empty() && seen.insert(k.clone()) {
            c.fails.push(Failure {
                msg: k,
                ..Failure::default()
            });
        }
    };
    let mut nx = false;
    for (i, &l) in lines.iter().enumerate() {
        if GO_ERR_LINE.is_match(l) && l.contains(".go:") {
            add(&mut c, "", "", &SRC_LOC.replace(l, "${1}: "));
        } else if let Some(m) = TSC_PAREN.captures(l) {
            add(&mut c, cap(&m, 1), cap(&m, 2), cap(&m, 3));
        } else if let Some(m) = TSC_PRETTY.captures(l) {
            add(&mut c, cap(&m, 1), cap(&m, 2), cap(&m, 3));
        } else if let Some(m) = TSC_FOUND.captures(l) {
            c.failed = num(m.get(1));
        } else if let Some(m) = ESBUILD_E.captures(l) {
            let (mut file, mut line) = ("", "");
            for n in &lines[i + 1..lines.len().min(i + 4)] {
                if let Some(a) = ESBUILD_AT.captures(n) {
                    let at = cap(&a, 1);
                    (file, line) = at.split_once(':').unwrap_or((at, ""));
                    break;
                }
            }
            add(&mut c, file, line, cap(&m, 1));
        } else if i + 1 < lines.len()
            && let Some(m) = WEBPACK_E.captures(l)
        {
            if cap(&m, 2).is_empty() {
                let f = cap(&m, 1);
                let f = f.strip_prefix("./").unwrap_or(f);
                add(&mut c, "", "", &format!("{f}: {}", lines[i + 1]));
            } else {
                add(&mut c, cap(&m, 1), cap(&m, 2), lines[i + 1]);
            }
        } else if l.contains("Failed tasks:") {
            nx = true;
        } else if nx && let Some(m) = NX_FAILED.captures(l) {
            c.fails.push(Failure {
                name: cap(&m, 1).into(),
                build: true,
                ..Failure::default()
            });
        }
    }
    c.failed = c.failed.max(c.fails.len());
    (!c.fails.is_empty()).then_some(c)
}

/// Lines less the indentation they share, blank ends dropped.
fn dedent(lines: &[&str]) -> String {
    let mut shared: Option<usize> = None;
    for l in lines {
        if l.trim().is_empty() {
            continue;
        }
        let n = l.len() - l.trim_start_matches([' ', '\t']).len();
        if shared.is_none_or(|s| n < s) {
            shared = Some(n);
        }
    }
    let shared = shared.unwrap_or(0);
    let out: Vec<&str> = lines
        .iter()
        .map(|l| {
            let l = if shared > 0 && l.len() >= shared {
                l.get(shared..).unwrap_or(l)
            } else {
                l
            };
            l.trim_end_matches([' ', '\t'])
        })
        .collect();
    out.join("\n").trim_matches('\n').to_string()
}

/// Folds the cards about one change into one: a pull request takes its
/// commits and their push, a commit its push (cards.go:1153).
pub(crate) fn join_cards(mut cs: Vec<Card>) -> Vec<Card> {
    if cs.len() < 2 {
        return cs;
    }
    let mut gone = vec![false; cs.len()];
    fn of_branch(c: &Card, branch: &str, title: &str) -> bool {
        match c.kind {
            "commit" => {
                !branch.is_empty() && c.branch == branch || !title.is_empty() && c.subject == title
            }
            "push" => {
                !branch.is_empty()
                    && c.refs.iter().any(|p| p.from == branch || p.to == branch)
                    && !c.refs.iter().any(|p| p.forced || p.span == "deleted")
            }
            _ => false,
        }
    }
    for i in 0..cs.len() {
        if cs[i].kind != "pr" || cs[i].state == "merged" {
            continue;
        }
        let (head, title) = (cs[i].head.clone(), cs[i].title.clone());
        for j in 0..i {
            if !gone[j] && cs[j].kind == "commit" && of_branch(&cs[j], &head, &title) {
                let c = cs[j].clone();
                cs[i].joined.push(c);
                gone[j] = true;
            }
        }
        // "gh pr create" alone names no branch: its commits' is it.
        let mut branch = head;
        if branch.is_empty()
            && let Some(j) = cs[i].joined.first()
        {
            branch = j.branch.clone();
        }
        for j in 0..i {
            if !gone[j] && cs[j].kind == "push" && of_branch(&cs[j], &branch, "") {
                cs[i].pushed = cs[j].remote.clone();
                gone[j] = true;
            }
        }
    }
    for i in 0..cs.len() {
        if gone[i] || cs[i].kind != "commit" || cs[i].branch.is_empty() {
            continue;
        }
        for j in i + 1..cs.len() {
            if !gone[j] && cs[j].kind == "push" && of_branch(&cs[j], &cs[i].branch, "") {
                cs[i].pushed = cs[j].remote.clone();
                gone[j] = true;
                break;
            }
        }
    }
    let mut k = 0;
    cs.retain(|_| {
        k += 1;
        !gone[k - 1]
    });
    cs
}

/// Whether a later shell step of the turn runs tests or a build again: a
/// card for one gone past is then superseded (history_settled.go:249).
pub(crate) fn reruns(kind: &str, cmd: &str) -> bool {
    let b = blank_heredocs(cmd);
    kind == "tests" && TEST_VERB.is_match(&b) || kind == "build" && BUILD_VERB.is_match(&b)
}

thread_local! {
    /// Each step's cards while it doesn't change: a running turn redraws
    /// every second. Starts again past a few thousand.
    static MEMO: RefCell<HashMap<u64, Rc<[Card]>>> = RefCell::new(HashMap::new());
}

/// A step's cards, kept while its status and output stay the same.
pub(crate) fn step_cards(
    st: &Step,
    cmd: impl FnOnce() -> String,
    out: impl FnOnce() -> String,
) -> Rc<[Card]> {
    if &*st.tool != "Bash" || matches!(st.status, Status::Running | Status::Waiting) {
        return Rc::from(Vec::new());
    }
    let mut h = DefaultHasher::new();
    (
        &*st.id,
        st.status as u8,
        st.output.len(),
        st.result_json().len(),
    )
        .hash(&mut h);
    let k = h.finish();
    if let Some(v) = MEMO.with(|m| m.borrow().get(&k).cloned()) {
        return v;
    }
    let v: Rc<[Card]> = Rc::from(cards_of(st, &cmd(), &out()));
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= 4096 {
            m.clear();
        }
        m.insert(k, v.clone());
    });
    v
}

fn failed_in(cs: &[Card]) -> bool {
    cs.iter().any(|c| c.kind == "tests" || c.kind == "build")
}

// ---- drawing ----

/// Each failure's name, and under it what it said (cards.go:1500).
fn fail_rows(fs: &[Failure], w: usize) -> Vec<Txt> {
    let mut rows = Vec::new();
    for (i, f) in fs.iter().enumerate() {
        if rows.len() >= 11 && i < fs.len() - 1 {
            rows.push(dim(&format!("+{} more", fs.len() - i)));
            break;
        }
        let mut lead = "";
        if !f.name.is_empty() {
            let (mut name, mut place) = (f.name.as_str(), "");
            // vitest's and jest's file > describe > test: the test first.
            if let Some((file, rest)) = name.split_once(" > ") {
                name = rest;
                place = file.rsplit_once('/').map_or(file, |(_, b)| b);
            }
            let mut r = paint(C_WHITE, name);
            if f.build {
                r.push(faint(" · "));
                r.push(paint(C_LOST, "doesn't build"));
            }
            if !place.is_empty() {
                r.push_str("  ", PLAIN);
                r.push(faint(place));
            }
            rows.push(r);
            lead = "  ";
        }
        if f.msg.is_empty() {
            continue;
        }
        let msg: Vec<&str> = f.msg.split('\n').collect();
        let keep = if i >= 3 { 1 } else { 3 };
        for (j, &l) in msg[..msg.len().min(keep)].iter().enumerate() {
            let mut r = Txt::raw(lead);
            let mut l = l;
            if let Some(loc) = SRC_LOC.captures(l) {
                r.push(paint(C_BLUE, cap(&loc, 1)));
                r.push_str(" ", PLAIN);
                l = &l[loc.get(0).unwrap().end()..];
            }
            let l = expand_tabs(l);
            let ind = l.len() - l.trim_start_matches(' ').len();
            r.push(blanks(ind));
            r.push(sub(&one_line(&l)));
            if j == keep - 1 && msg.len() > keep {
                r = r.take(w.saturating_sub(1)).and(faint("…"));
            }
            rows.push(r);
        }
    }
    rows
}

/// A list for a card, five at most, each after lead.
fn list_rows(xs: &[String], lead: &Txt) -> Vec<Txt> {
    let mut rows = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        if i == 4 && xs.len() > 5 {
            rows.push(dim(&format!("+{} more", xs.len() - 4)));
            break;
        }
        rows.push(lead.clone().and(text(x)));
    }
    rows
}

/// A change's size, as git's report says it.
fn stat(add: usize, del: usize, files: usize) -> Txt {
    paint(C_GREEN, &format!("+{add}"))
        .and_raw(" ")
        .and(paint(C_RED, &format!("−{del}")))
        .and(faint(" · "))
        .and(dim(&plural(files, "file")))
}

pub(crate) fn key_hint(key: &str, what: &str) -> Txt {
    text(key).and_raw(" ").and(dim(what))
}

/// Joins parts with a faint " · ", the way a foot strings them.
fn joined(a: Txt, b: Txt) -> Txt {
    if a.is_empty() {
        b
    } else {
        a.and(faint(" · ")).and(b)
    }
}

impl Drawer<'_> {
    /// A step's cards (cards.go:1094).
    pub(crate) fn step_cards(&self, id: StepId) -> Rc<[Card]> {
        let st = self.s.step(id);
        step_cards(
            st,
            || self.info(id).call.input.command.clone(),
            || {
                let i = self.info(id);
                super::draw::bash_out(st, &i.out)
            },
        )
    }

    /// Whether a step ran tests or a build that failed, whatever it
    /// exited with: piped through tail, a failing run exits 0
    /// (cards.go:1693).
    pub(crate) fn tests_failed(&self, id: StepId) -> bool {
        failed_in(&self.step_cards(id))
    }

    /// tests_failed, for a step known by itself.
    pub(crate) fn tests_failed_step(&self, st: &Step) -> bool {
        failed_in(&step_cards(
            st,
            || st.call().input.command.clone(),
            || super::draw::bash_out(st, &st.out()),
        ))
    }

    /// The cards a step left, indent cells in (cards.go:1122).
    pub(crate) fn cards(&mut self, id: StepId, indent: usize) {
        let cs = self.step_cards(id);
        if cs.is_empty() {
            return;
        }
        for c in join_cards(cs.to_vec()) {
            self.card_of(&c, indent);
        }
    }

    /// The cards of a folded run of steps, those about one change joined
    /// (cards.go:1132).
    pub(crate) fn run_cards(&mut self, ids: &[StepId], indent: usize) {
        let mut cs = Vec::new();
        for &id in ids {
            cs.extend(self.step_cards(id).iter().cloned());
        }
        if cs.is_empty() {
            return;
        }
        self.tool += 1;
        for c in join_cards(cs) {
            self.card_of(&c, indent);
        }
        self.tool -= 1;
    }

    /// One card (cards.go:1213).
    pub(crate) fn card_of(&mut self, c: &Card, indent: usize) {
        // "│ " … " │" around the inside.
        let room = self.cw.min(CAP_ROW) as isize - indent as isize - 4;
        if room < 24 {
            return;
        }
        // Every card the same width, so a stack of them reads as one.
        let room = (room as usize).min(72);
        let (mut frame, mark) = match c.tone() {
            Tone::Made => (C_OK_Q, C_GREEN),
            Tone::Warn => (C_WARN_Q, C_YELLOW),
            Tone::Lost => (C_LOST_Q, C_LOST),
            Tone::None => (C_FAINT, C_ORANGE),
        };
        let glyph = |g: &str| paint(mark, g).and_raw(" ");
        let mut head_l: Txt;
        let (mut head_r, mut foot) = (Txt::new(), Txt::new());
        let mut r = String::new();
        let mut rows: Vec<Txt> = Vec::new();
        let sel = self.o.selected.as_str();
        match c.kind {
            "commit" => {
                if !c.sha.is_empty() {
                    r = format!("{ENTITY_REF}commit:{}", c.sha);
                    head_l = glyph("●").and(paint(C_YELLOW, short_sha(&c.sha)));
                } else {
                    head_l = glyph("●").and(dim("committed"));
                }
                if !c.branch.is_empty() {
                    head_l.push(faint(" · "));
                    head_l.push(paint(C_BLUE, &c.branch));
                }
                if c.amend {
                    head_l.push(faint(" · "));
                    head_l.push(dim("amended"));
                }
                if c.root {
                    head_l.push(faint(" · "));
                    head_l.push(dim("first commit"));
                }
                if c.files > 0 {
                    head_r = stat(c.add, c.del, c.files);
                }
                let body: Vec<&str> = c.body.iter().map(String::as_str).collect();
                rows = super::draw::card_text(&c.subject, &body, room);
                if !c.with.is_empty() {
                    foot = dim(&format!("with {}", c.with.join(", ")));
                }
                if !c.pushed.is_empty() {
                    foot = joined(
                        foot,
                        paint(C_GREEN, "↑ ")
                            .and(dim("pushed to "))
                            .and(text(&c.pushed)),
                    );
                }
                if !r.is_empty() && sel == r {
                    foot = key_hint("↵", &format!("copy {}", short_sha(&c.sha)));
                }
            }
            "push" => {
                head_l = glyph("↑").and(dim("pushed to ")).and(text(&c.remote));
                for p in &c.refs {
                    let mut t = paint(C_BLUE, &p.from);
                    if p.to != p.from {
                        t.push(faint(" → "));
                        t.push(paint(C_BLUE, &p.to));
                    }
                    let span = if p.forced {
                        paint(C_YELLOW, "forced")
                            .and(faint(" · "))
                            .and(dim(&p.span))
                    } else if p.span == "deleted" {
                        paint(C_LOST, "deleted")
                    } else {
                        dim(&p.span)
                    };
                    rows.push(t.and_raw("  ").and(span));
                }
            }
            "pr" => {
                let state = match c.state {
                    "draft" => dim("draft"),
                    "merged" => {
                        let mut s = paint(C_BLUE, "merged");
                        if !c.base.is_empty() {
                            s.push(dim(&format!(" · {}", c.base)));
                        }
                        s
                    }
                    s => paint(C_GREEN, s),
                };
                head_l = glyph("⎇");
                if !c.num.is_empty() {
                    head_l.push(text(&format!("#{}", c.num)));
                    head_l.push(faint(" · "));
                }
                head_l.push(state);
                if c.state != "merged" && !c.head.is_empty() && !c.base.is_empty() {
                    head_r = paint(C_BLUE, &c.head)
                        .and(faint(" → "))
                        .and(paint(C_BLUE, &c.base));
                }
                if !c.title.is_empty() {
                    let body: Vec<&str> = c.body.iter().map(String::as_str).collect();
                    rows = super::draw::card_text(&c.title, &body, room);
                }
                // The commits that made it, and the push that took them
                // up, on a line of their own: one change, one card.
                if !c.joined.is_empty() {
                    let (mut add, mut del, mut files) = (0, 0, 0);
                    let mut line = paint(C_GREEN, "● ");
                    for (i, j) in c.joined.iter().enumerate() {
                        if i > 0 {
                            line.push_str(" ", PLAIN);
                        }
                        line.push(paint(C_YELLOW, short_sha(&j.sha)));
                        add += j.add;
                        del += j.del;
                        files += j.files;
                    }
                    line.push(dim(" committed"));
                    if !c.pushed.is_empty() {
                        line.push(faint(" · "));
                        line.push(paint(C_GREEN, "↑ "));
                        line.push(dim("pushed"));
                    }
                    if files > 0 {
                        line.push(faint(" · "));
                        line.push(stat(add, del, files));
                    }
                    rows.push(line);
                    if c.body.is_empty() && c.joined.len() == 1 {
                        let body: Vec<&str> = c.joined[0].body.iter().map(String::as_str).collect();
                        rows.extend(super::draw::card_text("", &body, room));
                    }
                }
                if !c.url.is_empty() {
                    let web = if c.url.contains("://") {
                        c.url.clone()
                    } else {
                        format!("https://{}", c.url)
                    };
                    r = format!("{ENTITY_REF}pr:{web}");
                    foot = dim(c.url.strip_prefix("https://").unwrap_or(&c.url));
                    if sel == r {
                        foot = key_hint("↵", "open on GitHub");
                    }
                } else if c.state == "merged" && !c.head.is_empty() {
                    foot = dim(&format!("{} deleted", c.head));
                }
            }
            "tests" => {
                head_l = glyph("✗").and(paint(
                    C_LOST,
                    &format!("{} failed", c.failed.max(c.fails.len())),
                ));
                let mut tally = Vec::new();
                if c.passed > 0 {
                    tally.push(format!("{} passed", c.passed));
                }
                if c.skipped > 0 {
                    tally.push(format!("{} skipped", c.skipped));
                }
                head_r = dim(&tally.join(" · "));
                rows = fail_rows(&c.fails, room);
            }
            "build" => {
                head_l = glyph("✗").and(paint(C_LOST, &plural(c.failed, "error")));
                head_r = dim(&c.what);
                rows = fail_rows(&c.fails, room);
            }
            "merge" => {
                let (mut g, mut verb) = ("⇣", format!("{}d", c.what));
                match c.what.as_str() {
                    "rebase" => (g, verb) = ("↻", "rebased".into()),
                    "pull" => verb = "pulled".into(),
                    _ => {}
                }
                // Stopped part way, it's still going: rebasing feat onto main.
                if !c.conflicts.is_empty() || !c.sha.is_empty() {
                    g = "⚠";
                    verb = format!("{}ing", c.what.strip_suffix('e').unwrap_or(&c.what));
                }
                head_l = glyph(g).and(dim(&verb));
                if c.what == "rebase" && !c.head.is_empty() {
                    head_l.push_str(" ", PLAIN);
                    head_l.push(paint(C_BLUE, &c.head));
                    if !c.branch.is_empty() {
                        head_l.push(dim(" onto "));
                        head_l.push(paint(C_BLUE, &c.branch));
                    }
                } else if c.what == "rebase" && !c.branch.is_empty() {
                    head_l.push(dim(" onto "));
                    head_l.push(paint(C_BLUE, &c.branch));
                } else if !c.branch.is_empty() {
                    head_l.push_str(" ", PLAIN);
                    head_l.push(paint(C_BLUE, &c.branch));
                }
                if !c.how.is_empty() && c.how != "rebased" {
                    head_l.push(faint(" · "));
                    head_l.push(dim(c.how));
                }
                if c.files > 0 {
                    head_r = stat(c.add, c.del, c.files);
                }
                if !c.sha.is_empty() {
                    rows.push(
                        dim("stopped at ")
                            .and(paint(C_YELLOW, short_sha(&c.sha)))
                            .and_raw(" ")
                            .and(text(&c.subject)),
                    );
                }
                rows.extend(list_rows(
                    &c.conflicts,
                    &paint(C_YELLOW, "conflict").and_raw("  "),
                ));
                if !c.conflicts.is_empty() {
                    foot = dim(&format!("{} to resolve", plural(c.conflicts.len(), "file")));
                }
            }
            "discard" => {
                let what = match c.what.as_str() {
                    "stash" => "stashed".to_string(),
                    "clean" => format!("removed {}", plural(c.lost.len(), "untracked file")),
                    "checkout" | "restore" => "discarded changes".into(),
                    "stash clear" => "cleared every stash".into(),
                    w => w.to_string(),
                };
                head_l = glyph("⚠").and(text(&what));
                if !c.branch.is_empty() && !c.sha.is_empty() {
                    head_l.push_str(" ", PLAIN);
                    head_l.push(paint(C_BLUE, &c.branch));
                    head_l.push(faint(" · was "));
                    head_l.push(paint(C_YELLOW, short_sha(&c.sha)));
                } else if !c.sha.is_empty() {
                    head_l.push(faint(" · now at "));
                    head_l.push(paint(C_YELLOW, short_sha(&c.sha)));
                }
                if !c.subject.is_empty() {
                    rows.push(text(&c.subject));
                }
                rows.extend(list_rows(&c.lost, &Txt::new()));
                if !c.undo.is_empty() {
                    foot = dim(&format!("back: {}", c.undo));
                } else if !c.warn {
                    foot = paint(C_LOST_Q, "git can't bring this back");
                }
            }
            _ => return,
        }
        if c.kind == "tests" || c.kind == "build" {
            let l = self.sp(indent - 1).and(head_l);
            self.add("", Color::Default, l, head_r);
            for row in rows {
                let l = self.sp(indent - 1).and(row);
                self.add("", Color::Default, l, Txt::new());
            }
            if !foot.is_empty() {
                let l = self.sp(indent - 1).and(foot.over(C_DIM));
                self.add("", Color::Default, l, Txt::new());
            }
            return;
        }
        if !r.is_empty() && sel == r {
            frame = C_ORANGE; // picked: its keys show at its foot
        }
        if c.kind == "commit" || c.kind == "pr" {
            self.frame_box(&r, indent, head_l, head_r, rows, foot, room, frame, false);
            return;
        }
        self.card(&r, indent, head_l, head_r, rows, foot, room, frame);
    }

    /// A card framed all round, its first row on the top edge and its foot
    /// on the bottom: square for a thing made, round for a message
    /// (cards.go:1644).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_box(
        &mut self,
        r: &str,
        indent: usize,
        head_l: Txt,
        head_r: Txt,
        rows: Vec<Txt>,
        foot: Txt,
        inner: usize,
        frame: Style,
        round: bool,
    ) {
        let (tl, tr, bl, br) = if round {
            ("╭", "╮", "╰", "╯")
        } else {
            ("┌", "┐", "└", "┘")
        };
        let from = self.lines.len();
        let edge = |l: &str, label: Txt, right: Txt, rc: &str| -> Txt {
            if label.is_empty() && right.is_empty() {
                return paint(frame, &format!("{l}{}{rc}", "─".repeat(inner + 2)));
            }
            let mut room = inner as isize - 1;
            if !right.is_empty() {
                room -= right.width() as isize + 1;
            }
            let mut label = label;
            if label.width() as isize > room - 1 {
                label = label.take((room - 2).max(0) as usize).and(faint("…"));
            }
            let rule = (room - label.width() as isize).max(1) as usize;
            let mut out = paint(frame, l)
                .and_raw(" ")
                .and(label)
                .and_raw(" ")
                .and(paint(frame, &"─".repeat(rule)));
            if !right.is_empty() {
                out.push_str(" ", PLAIN);
                out.push(right);
            }
            out.and_raw(" ").and(paint(frame, rc))
        };
        let l = self.sp(indent - 1).and(edge(tl, head_l, head_r, tr));
        self.add(r, Color::Default, l, Txt::new());
        for row in rows {
            let row = if row.width() > inner {
                row.take(inner - 1).and(faint("…"))
            } else {
                row
            };
            let w = row.width();
            let l = self
                .sp(indent - 1)
                .and(paint(frame, "│"))
                .and_raw(" ")
                .and(row)
                .and(blanks(inner - w))
                .and_raw(" ")
                .and(paint(frame, "│"));
            self.add("", Color::Default, l, Txt::new());
        }
        let foot = if foot.is_empty() {
            foot
        } else {
            foot.over(C_DIM)
        };
        let l = self.sp(indent - 1).and(edge(bl, foot, Txt::new(), br));
        self.add("", Color::Default, l, Txt::new());
        if !r.is_empty() {
            self.own(r, from);
        }
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
mod tests;
