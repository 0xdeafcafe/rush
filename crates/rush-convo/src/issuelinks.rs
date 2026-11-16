//! #1234 in a conversation drawn as a link to that issue or pull request
//! of the session's GitHub repository (github.com/<owner>/<repo>/issues/N,
//! which GitHub sends on to the pull request when N is one). Rust-only:
//! Go had none.
//!
//! The repository is its checkout's origin, asked of git once per folder
//! on a thread of its own, never while a frame is drawn: the first drawing
//! is bare, and once the answer is in, what was drawn is drawn again
//! (linkinfo::landed), as a link's lookup is; only with lookups on (the
//! UI's), so tests and --render ask nothing. While a session is drawn, its
//! repository is the one in scope (Scope).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use regex::Regex;
use rush_core::text::{Span, Style};

use crate::render::txt::Txt;

/// Each folder's GitHub repository ("owner/repo"), None for one that has
/// none (or while it's asked).
static REPOS: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

thread_local! {
    /// The repository of the session being drawn on this thread.
    static DRAWING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The GitHub repository an origin URL names, "owner/repo"; None for any
/// other host. https, ssh:// and scp-like (git@github.com:o/r.git) alike.
pub fn github_repo(url: &str) -> Option<String> {
    let u = url.trim();
    let u = u.strip_suffix(".git").unwrap_or(u).trim_end_matches('/');
    let rest = if let Some((_, r)) = u.split_once("://") {
        let r = r.rsplit_once('@').map_or(r, |(_, h)| h);
        r.strip_prefix("github.com/")
            .or_else(|| r.strip_prefix("www.github.com/"))
            .or_else(|| r.strip_prefix("github.com:22/"))?
    } else {
        let r = u.rsplit_once('@').map_or(u, |(_, h)| h);
        r.strip_prefix("github.com:")?
    };
    let mut parts = rest.split('/');
    let (o, r) = (parts.next()?, parts.next()?);
    let ok = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (parts.next().is_none() && ok(o) && ok(r)).then(|| format!("{o}/{r}"))
}

/// The GitHub repository of the checkout at dir, as last asked: None
/// before git has said, and then for one with none. The first call for a
/// folder asks git on a thread of its own, and redraws once it has.
pub fn repo_of(dir: &str) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    let mut m = REPOS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(r) = m.get(dir) {
        return r.clone();
    }
    // Asked only with lookups on, as the UI has them: tests and --render
    // ask nothing.
    if !crate::linkinfo::live() {
        return None;
    }
    m.insert(dir.to_string(), None);
    drop(m);
    let dir = dir.to_string();
    let _ = std::thread::Builder::new()
        .name("issue-links".into())
        .spawn(move || {
            let out = std::process::Command::new("git")
                .args(["-C", &dir, "remote", "get-url", "origin"])
                .stderr(std::process::Stdio::null())
                .output();
            let repo = out
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| github_repo(&String::from_utf8_lossy(&o.stdout)));
            if repo.is_some() {
                REPOS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(dir, repo);
                crate::linkinfo::landed();
            }
        });
    None
}

/// Says a folder's repository without asking git: for tests, and for a
/// caller that knows it.
pub fn set_repo_of(dir: &str, repo: Option<&str>) {
    REPOS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(dir.to_string(), repo.map(String::from));
}

/// While it lives, the session in cwd is the one drawn on this thread: its
/// #numbers link to its repository. The one before comes back after.
pub struct Scope(Option<String>);

impl Scope {
    pub fn of(cwd: &str) -> Scope {
        let repo = repo_of(cwd);
        Scope(DRAWING.with(|d| d.replace(repo)))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let was = self.0.take();
        DRAWING.with(|d| *d.borrow_mut() = was);
    }
}

/// #N as GitHub writes a reference: a word of its own, 1 to 7 digits.
static ISSUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#[0-9]{1,7}\b").expect("issue"));

/// Where #n of repo links.
pub fn issue_url(repo: &str, n: &str) -> String {
    format!("https://github.com/{repo}/issues/{n}")
}

/// Each #N in t linked to that issue of the repository being drawn: one
/// that starts a word (not a URL's #anchor, an HTML entity &#38;, a colour
/// #123456 in code), outside a link already and outside `code`.
pub(crate) fn issue_links(t: Txt, code: Style) -> Txt {
    if !t.0.iter().any(|s| s.text.contains('#')) {
        return t;
    }
    let Some(repo) = DRAWING.with(|d| d.borrow().clone()) else {
        return t;
    };
    let mut out = Txt::new();
    for s in t.0 {
        if !s.style.link.is_none() || s.style == code || !s.text.contains('#') {
            out.0.push(s);
            continue;
        }
        let b = s.text.as_bytes();
        let mut last = 0;
        for m in ISSUE.find_iter(&s.text) {
            let i = m.start();
            if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b"&/#_-".contains(&b[i - 1])) {
                continue;
            }
            if i > last {
                out.0.push(Span::new(&s.text[last..i], s.style));
            }
            let url = issue_url(&repo, &s.text[i + 1..m.end()]);
            out.0
                .push(Span::new(m.as_str(), s.style.underline().linked(&url)));
            last = m.end();
        }
        if last < s.text.len() {
            out.0.push(Span::new(&s.text[last..], s.style));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::style::C_TEXT;

    #[test]
    fn reads_github_remotes() {
        for (u, want) in [
            (
                "https://github.com/0xdeafcafe/rush.git",
                Some("0xdeafcafe/rush"),
            ),
            (
                "https://github.com/langwatch/langwatch",
                Some("langwatch/langwatch"),
            ),
            ("git@github.com:o/r.git\n", Some("o/r")),
            ("ssh://git@github.com/o/r.git", Some("o/r")),
            ("https://token@github.com/o/my.repo/", Some("o/my.repo")),
            ("https://gitlab.com/o/r.git", None),
            ("git@gitlab.com:o/r.git", None),
            ("https://github.com/o", None),
            ("https://github.com/o/r/extra", None),
            ("", None),
        ] {
            assert_eq!(github_repo(u).as_deref(), want, "{u}");
        }
    }

    fn links(t: &Txt) -> Vec<(String, String)> {
        t.0.iter()
            .filter_map(|s| {
                s.style
                    .link
                    .url()
                    .map(|u| (s.text.to_string(), u.to_string()))
            })
            .collect()
    }

    // A #number that starts a word links to its issue; an anchor, an
    // entity, a colour in code, one in a link and one outside a repo don't.
    #[test]
    fn links_issue_numbers() {
        set_repo_of("/w/rush", Some("o/r"));
        let code = C_TEXT.bold();
        let _s = Scope::of("/w/rush");
        let t = Txt(vec![
            Span::new(
                "fixes #3455 and (#12), not a#1, x/#2, &#38; or #abc",
                C_TEXT,
            ),
            Span::new("#ff0000", code),
            Span::new(" see ", C_TEXT),
            Span::new("#9", C_TEXT.linked("https://x.dev")),
        ]);
        let got = issue_links(t.clone(), code);
        assert_eq!(
            links(&got),
            [
                ("#3455".into(), "https://github.com/o/r/issues/3455".into()),
                ("#12".into(), "https://github.com/o/r/issues/12".into()),
                ("#9".into(), "https://x.dev".into()),
            ]
        );
        assert_eq!(got.plain(), t.plain(), "the words are as written");
        drop(_s);
        set_repo_of("/w/plain", None);
        let _s = Scope::of("/w/plain");
        let got = issue_links(t, code);
        assert_eq!(links(&got).len(), 1, "no repo: only the link it had");
    }
}
