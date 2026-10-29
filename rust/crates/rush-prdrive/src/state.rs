//! Everything between a pull request and merge, as of one look, in the
//! order to do it; and that as an agent reads it.
//! Ported from internal/prdrive/state.go.

use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rush_state::Time;
use rush_watch::go::{clip, short};
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::github::{Actor, Check, Raw, Review, Thread};
use crate::trust::{LW_FINDING, REVIEWERS, Reviewer, Trust, known_bot, reviewer_of};

/// Something on the list can be done now.
pub const WORK: &str = "WORK";
/// Nothing to do; checks still running.
pub const WAITING: &str = "WAITING";
/// Green and resolved, not approved: a person's turn.
pub const READY: &str = "READY FOR APPROVAL";
/// Approved and green.
pub const MERGEABLE: &str = "MERGEABLE";
/// Its head is in a fork: not ours to push to.
pub const FORK: &str = "FORK";
pub const MERGED: &str = "MERGED";
pub const CLOSED: &str = "CLOSED";

/// A Go slice: written null when empty, as Go writes a nil one (every one
/// here is built by appending, so empty is nil), read null as empty.
mod nil_vec {
    use super::*;

    pub fn serialize<T: Serialize, S: Serializer>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
        if v.is_empty() {
            s.serialize_none()
        } else {
            v.serialize(s)
        }
    }
}

/// One thing to do. Key is the same for the same thing from one look to
/// the next, so a follower can tell what's new.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename = "Key", default, deserialize_with = "rush_watch::gh::nd")]
    pub key: String,
    #[serde(rename = "Text", default, deserialize_with = "rush_watch::gh::nd")]
    pub text: String,
    /// Quiet items are done without a word and never wake the agent.
    #[serde(rename = "Quiet", default, deserialize_with = "rush_watch::gh::nd")]
    pub quiet: bool,
    /// A failed check's Actions job, whose log says why.
    #[serde(rename = "Job", default, deserialize_with = "rush_watch::gh::nd")]
    pub job: i64,
}

/// Everything between a pull request and merge, as of one look. Kept in a
/// follower's entry with Go's field names, in Go's order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct State {
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub repo: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub title: String,
    #[serde(rename = "URL", default, deserialize_with = "rush_watch::gh::nd")]
    pub url: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub head: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub branch: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub base: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub number: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub verdict: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd", serialize_with = "nil_vec::serialize")]
    pub todo: Vec<Item>,
    /// Checks still running, the longest first.
    #[serde(default, deserialize_with = "rush_watch::gh::nd", serialize_with = "nil_vec::serialize")]
    pub running: Vec<String>,
    #[serde(default, deserialize_with = "rush_watch::gh::nd", serialize_with = "nil_vec::serialize")]
    pub notes: Vec<String>,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub passed: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub failed: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub pending: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub skipped: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub required_green: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub required_total: i64,
    /// Its latest verdict.
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub review_bot: String,
    #[serde(default, deserialize_with = "rush_watch::gh::nd", serialize_with = "nil_vec::serialize")]
    pub approved: Vec<String>,
    /// Review threads still open, trusted.
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub open: i64,
    /// Review threads still open, untrusted.
    #[serde(default, deserialize_with = "rush_watch::gh::nd")]
    pub untrusted: i64,
    #[serde(default, deserialize_with = "rush_watch::gh::nd", serialize_with = "nil_vec::serialize")]
    pub untrusters: Vec<String>,
}

impl State {
    fn add(&mut self, key: impl Into<String>, text: impl Into<String>) {
        self.todo.push(Item {
            key: key.into(),
            text: text.into(),
            ..Default::default()
        });
    }
}

/// What raw needs, in the order to do it. us are the logins whose
/// comments are the agent's own: the signed-in user and the PR's author.
pub fn analyse(raw: &Raw, trust: &Trust, us: &[String]) -> State {
    analyse_at(raw, trust, us, Time::now())
}

/// analyse, as of now (how long checks have run is said from it).
pub fn analyse_at(raw: &Raw, trust: &Trust, us: &[String], now: Time) -> State {
    let mut s = State {
        repo: raw.repo.clone(),
        number: raw.number,
        title: raw.title.clone(),
        url: raw.url.clone(),
        head: short(&raw.head).into(),
        branch: raw.head_ref.clone(),
        base: raw.base.clone(),
        ..Default::default()
    };
    if raw.state == "MERGED" {
        s.verdict = MERGED.into();
        return s;
    }
    if raw.state == "CLOSED" {
        s.verdict = CLOSED.into();
        return s;
    }
    if raw.head_repo != raw.repo {
        s.verdict = FORK.into();
        s.notes
            .push("its head lives in a fork: not drivable, and review bots skip it".into());
        return s;
    }
    let ours = |a: &Actor| us.contains(&a.login);
    if raw.draft {
        s.notes.push(
            "draft: review bots and auto-approval skip drafts; mark it ready once the rest is done"
                .into(),
        );
    }

    // 1. It must merge cleanly before anything else counts.
    match raw.mergeable.as_str() {
        "CONFLICTING" => s.add(
            "conflict",
            format!(
                "CONFLICT with {b}: git merge origin/{b} (never rebase or force-push), resolve, verify, push. CI doesn't run on a conflicting branch.",
                b = raw.base
            ),
        ),
        "UNKNOWN" => s.notes.push(
            "GitHub is still working out whether it merges: look again in a minute".into(),
        ),
        _ => {}
    }

    // 2. Every failed check, now: never wait for the slowest first.
    let required = &raw.required;
    let mut failed: Vec<&Check> = Vec::new();
    let mut pending: Vec<&Check> = Vec::new();
    for c in &raw.checks {
        match c.state.as_str() {
            "passed" => s.passed += 1,
            "skipped" => s.skipped += 1,
            "pending" => {
                s.pending += 1;
                pending.push(c);
            }
            _ => {
                s.failed += 1;
                failed.push(c);
            }
        }
    }
    s.required_total = required.len() as i64;
    let mut missing: Vec<&str> = Vec::new();
    for name in required {
        let (mut green, mut seen) = (true, false);
        for c in raw.checks.iter().filter(|c| &c.name == name) {
            seen = true;
            green = green && (c.state == "passed" || c.state == "skipped");
        }
        if !seen {
            missing.push(name);
        }
        if green && seen {
            s.required_green += 1;
        }
    }
    failed.sort_by_key(|c| !c.required);
    for c in failed {
        // A review bot's check is red because it asks for changes: its findings are the work.
        if REVIEWERS
            .iter()
            .any(|r| !r.check.is_empty() && r.check == c.name)
            && c.state == "failed"
        {
            s.notes.push(format!(
                "'{}' is red because the bot asks for changes: its findings below are the work, not its log",
                c.name
            ));
            continue;
        }
        let tag = if c.required {
            "required"
        } else {
            "not required, still fix it"
        };
        let name = check_name(c);
        if c.state == "cancelled" {
            s.add(
                format!("cancelled:{}:{name}", short(&raw.head)),
                format!("CANCELLED {name} ({tag}): gh run rerun {} --failed", c.run),
            );
            continue;
        }
        let how = if c.job != 0 {
            format!("gh run view --job {} --log-failed | tail -80", c.job)
        } else {
            c.url.clone()
        };
        s.todo.push(Item {
            key: format!("failed:{}:{name}", short(&raw.head)),
            job: c.job,
            text: format!("FAILED {name} ({tag}): {how}"),
            quiet: false,
        });
    }

    // 3. Review threads: trusted, open, the ones that block first.
    let bot = latest_bot_review(&raw.reviews);
    if let Some(bot) = bot {
        s.review_bot = format!("{} at {}", bot.state, short(&bot.commit));
        if bot.commit != raw.head {
            s.review_bot += " (an older commit; it reviews again with its check)";
        }
        if bot.state == "CHANGES_REQUESTED" && bot.commit == raw.head {
            for f in outside_diff(&bot.body) {
                if !f.contains("[P2]") {
                    s.add(
                        format!("outside:{}", hash(&f)),
                        format!(
                            "BOT FINDING outside the diff (blocking, no thread to reply in; fix it): {f}"
                        ),
                    );
                }
            }
        }
    }
    struct View<'a> {
        t: &'a Thread,
        /// A reviewer's name, "person", or "" for untrusted.
        who: String,
        reviewer: Option<&'static Reviewer>,
        severity: String,
        blocking: bool,
        id: String,
        gist: String,
        our_replies: usize,
        our_last: Time,
        last_ours: bool,
    }
    let mut views: Vec<View> = Vec::new();
    let mut answered: HashSet<String> = HashSet::new();
    for t in &raw.threads {
        let Some(root) = t.comments.first() else {
            continue;
        };
        let mut v = View {
            t,
            who: String::new(),
            reviewer: None,
            severity: String::new(),
            blocking: false,
            id: String::new(),
            gist: gist(&root.body, 150),
            our_replies: 0,
            our_last: Time::default(),
            last_ours: false,
        };
        if let Some((r, f)) = reviewer_of(&root.author, &root.body) {
            v.who = r.name.into();
            v.reviewer = Some(r);
            v.severity = f.severity;
            v.blocking = f.blocking;
            v.id = f.id;
        } else if trust.person(&root.author) || ours(&root.author) {
            v.who = "person".into();
            v.blocking = true;
        }
        let mut last = Actor::default();
        for c in &t.comments {
            if ours(&c.author) {
                v.our_replies += 1;
                v.our_last = c.at;
            }
            // An untrusted reply is no one's word: it neither answers nor reopens.
            if ours(&c.author) || known_bot(&c.author) || trust.person(&c.author) {
                last = c.author.clone();
            }
        }
        v.last_ours = ours(&last);
        if let Some(r) = v.reviewer
            && !v.id.is_empty()
            && v.our_replies > 0
        {
            answered.insert(format!("{}:{}", r.name, v.id));
        }
        views.push(v);
    }
    views.sort_by(|a, b| {
        b.blocking
            .cmp(&a.blocking)
            .then_with(|| (b.who == "person").cmp(&(a.who == "person")))
            .then_with(|| a.t.path.cmp(&b.t.path))
    });
    let mut unread: Vec<String> = Vec::new();
    let mut untrusters: BTreeSet<String> = BTreeSet::new();
    for v in &views {
        if v.t.resolved {
            continue;
        }
        if v.who.is_empty() {
            s.untrusted += 1;
            untrusters.insert(v.t.comments[0].author.login.clone());
            continue;
        }
        s.open += 1;
        let mut label = format!("{} {}", v.who, v.severity).trim().to_string();
        if v.reviewer.is_some() {
            label += if v.blocking { " blocking" } else { " minor" };
        }
        let place = if v.t.line == 0 {
            v.t.path.clone()
        } else {
            format!("{}:{}", v.t.path, v.t.line)
        };
        let head = format!("[{label}] {place} thread {}", v.t.id);
        let key = |kind: &str| format!("thread:{}:{kind}", v.t.id);
        match v.reviewer {
            Some(r) if r.ignored.contains(&v.id.as_str()) => s.todo.push(Item {
                key: key("ignored"),
                quiet: true,
                text: format!(
                    "IGNORED {head}: '{}' is a finding the user never wants worked. Resolve it, no reply.",
                    v.id
                ),
                job: 0,
            }),
            Some(r) if v.last_ours && !r.signature.is_empty() && v.blocking => {
                if bot.is_some_and(|b| b.at.after(v.our_last)) {
                    s.add(
                        key("contested"),
                        format!(
                            "CONTESTED {head}: the bot read our reply and still holds this. Do NOT reply again. Fix it, or put it to the user as one decision. {}",
                            v.gist
                        ),
                    );
                } else {
                    unread.push(place);
                }
            }
            _ if v.last_ours => s.add(
                key("resolve"),
                format!("RESOLVE {head}: the last word is ours and nothing new came: rush pr resolve"),
            ),
            Some(r)
                if !v.id.is_empty()
                    && answered.contains(&format!("{}:{}", r.name, v.id))
                    && v.our_replies == 0 =>
            {
                s.add(
                    key("repeat"),
                    format!(
                        "REPEAT {head}: finding '{}' was already answered in another thread. Resolve this one without replying, unless the code changed and it's now right.",
                        v.id
                    ),
                )
            }
            _ if v.blocking => s.add(key("address"), format!("ADDRESS {head}: {}", v.gist)),
            _ => s.add(
                key("minor"),
                format!(
                    "MINOR {head}: fix it if it's clearly right and small, else resolve with one line. Never argue a minor point. {}",
                    v.gist
                ),
            ),
        }
    }
    if !unread.is_empty() {
        let mut how = String::from("push the other fixes");
        for r in &REVIEWERS {
            for c in &raw.checks {
                if !r.check.is_empty() && c.name == r.check && bot.is_some() && c.run != 0 {
                    how += &format!(", or if nothing else is going out, gh run rerun {}", c.run);
                }
            }
        }
        s.add(
            format!("unread:{}", raw.head),
            format!(
                "BOT HASN'T READ our replies on {}. It reads them only on its next run: {how}.",
                unread.join(", ")
            ),
        );
    }
    s.untrusters = untrusters.into_iter().collect();
    if s.untrusted > 0 {
        s.notes.push(format!(
            "{} open thread(s) from untrusted authors ({}): never acted on; they still block the merge, so tell the user",
            s.untrusted,
            s.untrusters.join(", ")
        ));
    }

    // 4. People's comments on the pull request since the last push.
    for c in &raw.comments {
        if c.at.before(raw.head_at) || ours(&c.author) || c.author.bot || !trust.person(&c.author)
        {
            continue;
        }
        s.add(
            format!("comment:{}", c.id),
            format!(
                "READ @{}'s comment on the PR: {} {}",
                c.author.login,
                gist(&c.body, 200),
                c.url
            ),
        );
    }

    // 5. The description, and what GitHub found itself.
    for g in description_gaps(&raw.body) {
        s.add(format!("desc:{g}"), format!("DESCRIPTION: {g}"));
    }
    for a in &raw.alerts {
        s.add(
            format!("alert:{a}"),
            format!("CODE SCANNING alert open on this PR: {a}"),
        );
    }

    if s.pending == 0 && raw.mergeable != "CONFLICTING" && !raw.checks.is_empty() {
        missing.sort();
        for name in missing {
            s.notes.push(format!(
                "required check '{name}' hasn't reported on this commit, and GitHub won't merge without it"
            ));
        }
    }

    // 6. What's still running: never a reason to sit still while the list has items.
    pending.sort_by(|a, b| a.started.cmp(&b.started));
    for c in pending {
        let mut line = check_name(c);
        if !c.started.is_zero() {
            line += &format!(" (running {})", since(now, c.started));
        } else {
            line += " (queued)";
        }
        s.running.push(line);
    }

    for r in &raw.reviews {
        if r.state == "APPROVED" && r.commit == raw.head && !s.approved.contains(&r.author.login)
        {
            s.approved.push(r.author.login.clone());
        }
    }
    let loud = s.todo.iter().filter(|it| !it.quiet).count();
    s.verdict = if loud > 0 {
        WORK
    // No check on this commit yet while the branch requires some: just
    // pushed, or GitHub wasn't read; never green.
    } else if s.pending > 0
        || raw.mergeable == "UNKNOWN"
        || raw.checks.is_empty() && !required.is_empty()
    {
        WAITING
    } else if s.approved.is_empty() {
        READY
    } else {
        MERGEABLE
    }
    .into();
    if s.todo.len() > loud && loud == 0 {
        s.notes
            .push("only quiet items left: resolve them, they block the merge".into());
    }
    s
}

/// A check as the list names it: "workflow / name", or its name alone.
fn check_name(c: &Check) -> String {
    let full = format!("{} / {}", c.workflow, c.name);
    full.strip_prefix(" / ").map(str::to_string).unwrap_or(full)
}

fn latest_bot_review(reviews: &[Review]) -> Option<&Review> {
    let mut out = None;
    for r in reviews {
        for b in &REVIEWERS {
            if !b.signature.is_empty()
                && r.author.bot
                && r.author.login == b.login
                && r.body.starts_with(b.signature)
            {
                out = Some(r);
            }
        }
    }
    out
}

fn outside_diff(body: &str) -> Vec<String> {
    let Some((_, rest)) = body.split_once("Outside the diff:") else {
        return Vec::new();
    };
    rest.split('\n')
        .map(str::trim)
        .filter(|l| l.starts_with("- **[P"))
        .map(|l| gist(l, 220))
        .collect()
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static DETAILS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<details>.*?</details>").unwrap());
static MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new("[*_`]").unwrap());

/// A comment's start, as one plain line of at most n runes: no hidden
/// markers, folded details, badges or markdown.
pub fn gist(body: &str, n: usize) -> String {
    let body = HTML_COMMENT.replace_all(body, "");
    let body = DETAILS.replace_all(&body, "");
    let body = LW_FINDING.replace_all(&body, "");
    let mut lines: Vec<&str> = body.split('\n').filter(|l| !l.trim().is_empty()).collect();
    // CodeRabbit opens with a "category | severity | effort" badge line.
    if lines.first().is_some_and(|l| l.matches('|').count() >= 2) {
        lines.remove(0);
    }
    let plain = MARKS.replace_all(&lines.join(" "), "").into_owned();
    clip(&plain.split_whitespace().collect::<Vec<_>>().join(" "), n)
}

static SECTIONS_READ: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Sections read:(.*)").unwrap());
static TICKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"- \[[xX]\] I read the sections").unwrap());

/// A PR template's sections left empty.
fn description_gaps(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    if body.contains("## Architecture") {
        if !TICKED.is_match(body) {
            out.push(
                "Architecture box unticked: read the sections the change touches, then tick it"
                    .into(),
            );
        }
        let empty = match SECTIONS_READ.captures(body) {
            None => true,
            Some(m) => HTML_COMMENT.replace_all(&m[1], "").trim().is_empty(),
        };
        if empty {
            out.push("'Sections read:' is empty".into());
        }
    }
    for h in ["## Test plan", "## What changed"] {
        if let Some((_, rest)) = body.split_once(h) {
            let part = rest.split_once("\n## ").map(|(p, _)| p).unwrap_or(rest);
            if HTML_COMMENT.replace_all(part, "").trim().is_empty() {
                out.push(format!("'{}' is empty", &h[3..]));
            }
        }
    }
    out
}

/// A state as an agent reads it.
pub fn render(s: &State) -> String {
    let mut b = format!(
        "{}#{} {}\n  {}  head {}  {} -> {}\n  VERDICT: {}\n",
        s.repo,
        s.number,
        clip(&s.title, 90),
        s.url,
        s.head,
        s.branch,
        s.base,
        s.verdict
    );
    if matches!(s.verdict.as_str(), MERGED | CLOSED | FORK) {
        for n in &s.notes {
            b += &format!("  note: {n}\n");
        }
        return b.trim_end_matches('\n').to_string();
    }
    b += &format!(
        "  checks: {} passed · {} failed · {} running · required {}/{} green\n",
        s.passed, s.failed, s.pending, s.required_green, s.required_total
    );
    if !s.review_bot.is_empty() {
        b += &format!("  review bot: {}\n", s.review_bot);
    }
    b += &format!(
        "  threads: {} trusted open · {} untrusted ignored\n",
        s.open, s.untrusted
    );
    if !s.approved.is_empty() {
        b += &format!(
            "  approval: approved on this commit by {}\n",
            s.approved.join(", ")
        );
    } else {
        b += "  approval: none on this commit (a push clears approvals)\n";
    }
    if !s.todo.is_empty() {
        b += "  TO DO, in this order, all of it before the next push:\n";
        for (i, it) in s.todo.iter().enumerate() {
            b += &format!("    {}. {}\n", i + 1, it.text);
        }
    }
    if !s.running.is_empty() {
        if !s.todo.is_empty() {
            b += "  still running (work the list meanwhile; never sit on these):\n";
        } else {
            b += "  still running (nothing to do: end your turn; rush tells you when something lands):\n";
        }
        for (i, r) in s.running.iter().enumerate() {
            if i == 10 {
                b += &format!("    - … and {} more\n", s.running.len() - 10);
                break;
            }
            b += &format!("    - {r}\n");
        }
    }
    for n in &s.notes {
        b += &format!("  note: {n}\n");
    }
    match s.verdict.as_str() {
        READY => b += "  READY: green and resolved. Approval is a person's (or an auto-approve workflow's). Tell the user; don't push again.\n",
        MERGEABLE => b += "  MERGEABLE: approved and green. Tell the user; merge only if asked.\n",
        _ => {}
    }
    b.trim_end_matches('\n').to_string()
}

fn hash(s: &str) -> String {
    let h = Sha256::digest(s.as_bytes());
    h[..6].iter().map(|x| format!("{x:02x}")).collect()
}

/// How long since t: "14m", "3h05m".
fn since(now: Time, t: Time) -> String {
    let d = Duration::from_nanos(now.sub_nanos(t).clamp(0, u64::MAX as i128) as u64);
    let mins = d.as_secs() / 60;
    if d < Duration::from_secs(2 * 3600) {
        return format!("{mins}m");
    }
    format!("{}h{:02}m", d.as_secs() / 3600, mins % 60)
}

/// A state's to-do keys, sorted, for tests and logs.
pub fn keys(s: &State) -> Vec<String> {
    let mut out: Vec<String> = s.todo.iter().map(|it| it.key.clone()).collect();
    out.sort();
    out
}
