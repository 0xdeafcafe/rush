//! Who may give the agent instructions: the review bots rush knows by
//! their accounts and marks, and people who are members of the
//! repository's owner or can write to it, asked of GitHub live.
//! Ported from internal/prdrive/trust.go.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use rush_state::Time;

use crate::github::{Actor, gh};

/// A review bot rush knows by its account and its marks, and how far its
/// findings are taken: what blocks, what the agent should never spend a
/// turn on.
pub struct Reviewer {
    pub name: &'static str,
    /// The bot account's login, without [bot].
    pub login: &'static str,
    /// Reads a root comment of the bot's: whether it's one of this
    /// reviewer's findings (None when not), how severe, whether it blocks,
    /// and its id.
    pub finding: fn(&str) -> Option<Finding>,
    /// Starts its reviews; "" when any review by its login is its.
    pub signature: &'static str,
    /// The check whose run is its review: rerun to have it read replies on
    /// the same commit.
    pub check: &'static str,
    /// Finding ids the user never wants worked: their threads are resolved
    /// without a reply and never wake the agent.
    pub ignored: &'static [&'static str],
}

/// One of a reviewer's findings, as its comment marks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: String,
    pub blocking: bool,
    pub id: String,
}

pub(crate) static LW_FINDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\*\*\[(P[012]) · (blocking|non-blocking)\]\*\*").unwrap()
});
static LW_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--[\t\n\x0c\r ]*id:([A-Za-z0-9_.:-]+)[\t\n\x0c\r ]*-->").unwrap());

const CR_LEVELS: [(&str, &str); 5] = [
    ("🔴 Critical", "critical"),
    ("🟠 Major", "major"),
    ("🟡 Minor", "minor"),
    ("🔵 Trivial", "trivial"),
    ("🧹 Nitpick", "nitpick"),
];

fn langwatch(body: &str) -> Option<Finding> {
    let m = LW_FINDING.captures(body)?;
    Some(Finding {
        severity: m[1].to_string(),
        blocking: &m[2] == "blocking",
        id: LW_ID
            .captures(body)
            .map(|i| i[1].to_string())
            .unwrap_or_default(),
    })
}

fn coderabbit(body: &str) -> Option<Finding> {
    for (mark, sev) in CR_LEVELS {
        if body.contains(mark) {
            return Some(Finding {
                severity: sev.into(),
                blocking: sev == "critical" || sev == "major",
                id: String::new(),
            });
        }
    }
    Some(Finding {
        severity: "comment".into(),
        blocking: false,
        id: String::new(),
    })
}

fn copilot(_: &str) -> Option<Finding> {
    Some(Finding {
        severity: "comment".into(),
        blocking: false,
        id: String::new(),
    })
}

/// The review bots rush knows. A bot not here is untrusted.
pub static REVIEWERS: [Reviewer; 3] = [
    Reviewer {
        name: "LangWatchReviewBot",
        login: "github-actions",
        signature: "**@LangWatchReviewBot**",
        check: "review",
        finding: langwatch,
        // It never approves, and reads replies only on its next run: PLAYBOOK.
        ignored: &["no-linked-issue"],
    },
    Reviewer {
        name: "CodeRabbit",
        login: "coderabbitai",
        signature: "",
        check: "CodeRabbit",
        finding: coderabbit,
        ignored: &[],
    },
    Reviewer {
        name: "Copilot",
        login: "copilot-pull-request-reviewer",
        signature: "",
        check: "",
        finding: copilot,
        ignored: &[],
    },
];

/// The known reviewer whose finding body is, by a bot account.
pub(crate) fn reviewer_of(a: &Actor, body: &str) -> Option<(&'static Reviewer, Finding)> {
    if !a.bot {
        return None;
    }
    for r in &REVIEWERS {
        if r.login != a.login {
            continue;
        }
        if let Some(f) = (r.finding)(body) {
            return Some((r, f));
        }
    }
    None
}

/// Whether a is one of the known reviewers' accounts.
pub(crate) fn known_bot(a: &Actor) -> bool {
    a.bot && REVIEWERS.iter().any(|r| r.login == a.login)
}

/// Asks whether login is trusted on repo.
pub type AskFn = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// Decides who may give the agent instructions: a person who is a member
/// of the repository's owner, or can write to it, asked of GitHub live and
/// kept a short while; a known reviewer's bot, by its findings.
pub struct Trust {
    pub repo: String,
    /// Whether login is trusted, asked of GitHub. Tests set it.
    pub ask: Option<AskFn>,
    cache: Mutex<HashMap<String, (bool, Time)>>,
}

/// How long an answer is kept: a follower runs for days, and someone who
/// leaves the org stops being trusted within it.
const TRUST_TTL: Duration = Duration::from_secs(30 * 60);

impl Trust {
    pub fn new(repo: &str) -> Trust {
        Trust {
            repo: repo.into(),
            ask: None,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// A Trust that asks ask rather than GitHub.
    pub fn with_ask(repo: &str, ask: impl Fn(&str, &str) -> bool + Send + Sync + 'static) -> Trust {
        Trust {
            ask: Some(Box::new(ask)),
            ..Trust::new(repo)
        }
    }

    /// Whether a, a person, is trusted.
    pub fn person(&self, a: &Actor) -> bool {
        if a.bot || a.login.is_empty() || a.login == "ghost" {
            return false;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((ok, at)) = cache.get(&a.login)
            && Time::now().sub_nanos(*at) < TRUST_TTL.as_nanos() as i128
        {
            return *ok;
        }
        let ok = match &self.ask {
            Some(ask) => ask(&self.repo, &a.login),
            None => ask_github(&self.repo, &a.login),
        };
        cache.insert(a.login.clone(), (ok, Time::now()));
        ok
    }
}

/// A member of the owning org (204), or a collaborator who can write.
/// author_association alone isn't proof.
fn ask_github(repo: &str, login: &str) -> bool {
    let owner = repo.split_once('/').map(|(o, _)| o).unwrap_or(repo);
    if let Ok(b) = gh(&["api", "-i", &format!("orgs/{owner}/members/{login}")]) {
        let text = String::from_utf8_lossy(&b);
        let f: Vec<&str> = text.split_whitespace().take(2).collect();
        if f.len() > 1 && f[1] == "204" {
            return true;
        }
    }
    let Ok(b) = gh(&[
        "api",
        &format!("repos/{repo}/collaborators/{login}/permission"),
        "--jq",
        ".permission",
    ]) else {
        return false;
    };
    matches!(
        String::from_utf8_lossy(&b).trim(),
        "admin" | "maintain" | "write"
    )
}
