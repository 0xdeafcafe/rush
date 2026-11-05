//! What's new: the notes of rush's GitHub releases between two versions,
//! kept like the update check's answer, and made readable.
//!
//! New in the port: there's no Go source. Go's rush was installed with go
//! install and had no release notes to show. A release's notes are what
//! .github/workflows/release.yml writes: the first-parent subjects of
//! main since the release before (nightlies since the nightly before),
//! then GitHub's generated notes (`--generate-notes`). The rolling
//! nightly's tag is `nightly`; its version is in its title, "Nightly
//! (0.1.0-nightly.20261105)", and only the newest nightly's notes are on
//! GitHub.

use std::cmp::Ordering;
use std::path::Path;

use rush_core::time::{Ms, now};
use serde::{Deserialize, Serialize};

use crate::{API, EVERY, Error, Host, REPO, agent, create_dir, current_from, semver, state_dir};

/// What fetching release notes is called where rush shows what waits on
/// the network.
pub const NOTES_JOB: &str = "Release notes";

/// The file in rush's folder where the last notes fetched are kept.
pub const NOTES_CACHE: &str = "rust-notes.json";

/// The most of GitHub's answer about the newest releases that's read: a
/// page of them, notes and file lists, rarely more than a few hundred kB.
const MAX_NOTES_BODY: u64 = 4 << 20;

/// How many of the newest releases are asked for: more than come out
/// between two updates.
const PER_PAGE: usize = 30;

/// How soon GitHub is asked again for a version it had no notes for (a
/// release only just out, or a nightly already replaced).
const RETRY: Ms = 5 * 60 * 1000;

/// One release's notes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    /// Its version without the v: 0.1.0-beta.8, 0.1.0-nightly.20261105.
    pub version: String,
    /// Its tag: v0.1.0-beta.8, or nightly.
    #[serde(default)]
    pub tag: String,
    /// As GitHub has it: markdown.
    #[serde(default)]
    pub body: String,
    /// Its page on GitHub.
    #[serde(default)]
    pub url: String,
}

impl Notes {
    /// v and the version, as rush shows it.
    pub fn short(&self) -> String {
        format!("v{}", self.version)
    }

    /// What it says, the boilerplate left out ([`tidy`]).
    pub fn items(&self) -> Vec<Item> {
        tidy(&self.body)
    }
}

/// A line of notes worth reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A heading over the lines after it, without its #s.
    Heading(String),
    /// One change: a bullet's text.
    Bullet(String),
    /// Anything else written.
    Text(String),
}

/// A release as GitHub's list of them gives it.
#[derive(Deserialize)]
struct GhNotes {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
}

/// The version a release is of: its tag's, or the rolling nightly's from
/// its title. None when that isn't semver.
fn version_of(tag: &str, name: &str) -> Option<String> {
    let v = if tag == "nightly" {
        current_from(name.trim().strip_prefix("Nightly (")?.strip_suffix(')')?)?
    } else {
        current_from(tag)?
    };
    semver::order(&v, &v).map(|_| v)
}

/// Asks GitHub for the notes of the newest releases.
fn fetch_notes(api: &str) -> Result<Vec<Notes>, Error> {
    let url = format!("{api}/repos/{REPO}/releases?per_page={PER_PAGE}");
    let mut res = agent(crate::TIMEOUT)
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| Error(format!("Get {url:?}: {e}")))?;
    if res.status() != 200 {
        return Err(Error(format!("GitHub said {}", res.status())));
    }
    let b = res
        .body_mut()
        .with_config()
        .limit(MAX_NOTES_BODY)
        .read_to_vec()
        .map_err(|e| Error(e.to_string()))?;
    let rs: Vec<GhNotes> = serde_json::from_slice(&b).map_err(|e| Error(e.to_string()))?;
    Ok(rs
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            Some(Notes {
                version: version_of(&r.tag_name, r.name.as_deref().unwrap_or(""))?,
                tag: r.tag_name,
                body: r.body.unwrap_or_default(),
                url: r.html_url,
            })
        })
        .collect())
}

/// The notes last fetched, shared by every Rust rush (rust-notes.json).
#[derive(Default, Serialize, Deserialize)]
struct NotesCache {
    /// When GitHub was last asked, in Unix ms.
    #[serde(default)]
    checked: Ms,
    #[serde(default)]
    releases: Vec<Notes>,
}

/// The newest releases' notes, from what was kept when it's fresh and has
/// want (a version or a tag), else from GitHub. What was kept stands in
/// when GitHub can't be asked.
fn releases_in(
    dir: &Path,
    api: &str,
    want: &str,
    host: &dyn Host,
    now: Ms,
) -> Result<Vec<Notes>, Error> {
    let path = dir.join(NOTES_CACHE);
    let c: NotesCache = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let has = c
        .releases
        .iter()
        .any(|r| r.version == want || r.tag == want);
    let age = now - c.checked;
    if c.checked > 0 && ((has && age <= EVERY.as_millis() as Ms) || age <= RETRY) {
        return Ok(c.releases);
    }
    if !host.net_run(NOTES_JOB) {
        return if c.checked > 0 {
            Ok(c.releases)
        } else {
            Err(Error("the network is down".into()))
        };
    }
    let got = fetch_notes(api);
    host.net_done(NOTES_JOB, got.as_ref().err().map(|e| e.0.as_str()));
    match got {
        Ok(releases) => {
            let c = NotesCache {
                checked: now,
                releases,
            };
            if let Ok(b) = serde_json::to_vec(&c) {
                let _ = create_dir(dir);
                let _ = crate::write_file(&path, &b, 0o600);
            }
            Ok(c.releases)
        }
        Err(_) if c.checked > 0 => Ok(c.releases),
        Err(e) => Err(e),
    }
}

/// The notes of every release after old, up to and including new, newest
/// first, by semver's order: a nightly comes before its release.
pub fn between(old: &str, new: &str, host: &dyn Host) -> Result<Vec<Notes>, Error> {
    between_in(&state_dir(), API, old, new, host, now())
}

pub(crate) fn between_in(
    dir: &Path,
    api: &str,
    old: &str,
    new: &str,
    host: &dyn Host,
    now: Ms,
) -> Result<Vec<Notes>, Error> {
    let (old, new) = (bare(old), bare(new));
    let mut out: Vec<Notes> = releases_in(dir, api, &new, host, now)?
        .into_iter()
        .filter(|r| {
            semver::order(&r.version, &old) == Some(Ordering::Greater)
                && matches!(
                    semver::order(&r.version, &new),
                    Some(Ordering::Less | Ordering::Equal)
                )
        })
        .collect();
    out.sort_by(|a, b| semver::order(&b.version, &a.version).unwrap_or(Ordering::Equal));
    out.dedup_by(|a, b| a.version == b.version);
    Ok(out)
}

/// The notes of one release: by its version (v or not), or `nightly` for
/// the newest nightly. None when GitHub has none for it.
pub fn of(version: &str, host: &dyn Host) -> Result<Option<Notes>, Error> {
    of_in(&state_dir(), API, version, host, now())
}

pub(crate) fn of_in(
    dir: &Path,
    api: &str,
    version: &str,
    host: &dyn Host,
    now: Ms,
) -> Result<Option<Notes>, Error> {
    let v = bare(version);
    Ok(releases_in(dir, api, &v, host, now)?
        .into_iter()
        .find(|r| r.version == v || r.tag == v))
}

/// A version without its v.
fn bare(v: &str) -> String {
    current_from(v).unwrap_or_default()
}

/// The lines of notes worth reading, in order: GitHub's boilerplate
/// (Full Changelog, New Contributors, What's Changed, comments), the
/// release's own (how to install it, its sha256s, the nightly's warning)
/// and code blocks left out; a bullet's "by @who in <its PR>" said as
/// (#N), and a merge's "merge: " dropped; headings with nothing under them
/// left out.
pub fn tidy(body: &str) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    let (mut fence, mut skip) = (false, false);
    for l in body.lines() {
        let t = l.trim();
        if t.starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence || matches!(t, "" | "-" | "*" | "+") || boilerplate(t) {
            continue;
        }
        if t.starts_with('#') {
            let h = inline(t.trim_start_matches('#').trim());
            skip = h.eq_ignore_ascii_case("new contributors");
            if !skip && !h.eq_ignore_ascii_case("what's changed") && !h.is_empty() {
                out.push(Item::Heading(h));
            }
            continue;
        }
        if skip {
            continue;
        }
        let item = match ["- ", "* ", "+ "].iter().find_map(|&b| t.strip_prefix(b)) {
            Some(b) => Item::Bullet(bullet(b)),
            None => Item::Text(inline(t)),
        };
        if !matches!(&item, Item::Bullet(b) | Item::Text(b) if b.is_empty()) {
            out.push(item);
        }
    }
    // A heading with nothing under it says nothing.
    let mut kept: Vec<Item> = Vec::with_capacity(out.len());
    for it in out {
        if matches!(it, Item::Heading(_)) && matches!(kept.last(), Some(Item::Heading(_))) {
            kept.pop();
        }
        kept.push(it);
    }
    if matches!(kept.last(), Some(Item::Heading(_))) {
        kept.pop();
    }
    kept
}

/// The lines a release's notes have whatever changed.
fn boilerplate(t: &str) -> bool {
    let t = t.trim_start_matches('*').trim_start();
    [
        "Full Changelog",
        "Install with Homebrew",
        "SHA-256:",
        "Automated nightly build of",
        "<!--",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

/// A bullet's text: "Fix x by @who in https://github.com/…/pull/12" as
/// "Fix x (#12)", a merge's subject without "merge: ".
fn bullet(b: &str) -> String {
    let mut b = b.trim();
    let mut pr = String::new();
    if let Some((head, url)) = b.rsplit_once(" in https://")
        && let Some((_, n)) = url.rsplit_once("/pull/")
        && !n.is_empty()
        && n.bytes().all(|c| c.is_ascii_digit())
    {
        pr = format!(" (#{n})");
        b = head.rsplit_once(" by @").map_or(head, |(t, _)| t);
    }
    let b = b.strip_prefix("merge: ").unwrap_or(b);
    format!("{}{pr}", inline(b))
}

/// Markdown's marks inside a line taken away: bold and code.
fn inline(t: &str) -> String {
    t.replace("**", "").replace('`', "").trim().to_string()
}

/// Notes as text for a terminal, newest first: each release's version,
/// what's new in it (at most most lines in all, then how many more), and
/// its page on GitHub.
pub fn text(notes: &[Notes], most: usize) -> String {
    let mut s = String::new();
    let mut left = most;
    for (i, n) in notes.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&format!("what's new in {}\n", n.short()));
        let items = n.items();
        if items.is_empty() {
            s.push_str("  nothing written down for it\n");
        }
        let shown = items.len().min(left);
        for it in &items[..shown] {
            match it {
                Item::Heading(h) => s.push_str(&format!("  {h}\n")),
                Item::Bullet(b) => s.push_str(&format!("  - {b}\n")),
                Item::Text(t) => s.push_str(&format!("  {t}\n")),
            }
        }
        left -= shown;
        if shown < items.len() {
            s.push_str(&format!("  … and {} more\n", items.len() - shown));
        }
        if !n.url.is_empty() {
            s.push_str(&format!("  {}\n", n.url));
        }
    }
    s
}
