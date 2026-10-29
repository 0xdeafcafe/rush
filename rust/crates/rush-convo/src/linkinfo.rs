//! Links rush knows the kind of are drawn with what it knows of them after
//! them: a pull request where it stands, an issue whether it's closed.
//! Each kind is a LinkKind, registered once (register_link); this module
//! asks them and keeps their answers, drawing nothing of its own.
//!
//! Nothing here is asked while a frame is drawn: the first drawing of a
//! link starts its lookup on a thread of its own and draws it bare, and
//! once the answer is in, lookups_gen moves and on_lookup is told, so what
//! was drawn is drawn again with it.
//!
//! Port of internal/convo/linkinfo.go (LinkKind :20, linkFailedFor :41,
//! RegisterLink :94, LookLinks :101, linkKindOf :108, LinkInfo :124,
//! LinkInfoOf :159, due :166, ask :178, ForgetLink :198, linkAfter :213) and
//! linkinfo_github.go (the GitHub kinds, prBadge :52, issueBadge :85).
//! Go's 30-second limit on a lookup is gh's own here.

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use rush_core::text::{Line, Span};
use rush_gh::ghpr::{self, Gh, Issue, PR};

use crate::render::style::{C_BLUE, C_GREEN, C_RED, C_YELLOW, dim, faint, paint, sub};
use crate::render::txt::Txt;

/// A kind's lookup: what there is to know of a key, or why it couldn't
/// say.
pub type Look<T> = Arc<dyn Fn(&str) -> Result<T, String> + Send + Sync>;

/// A kind of address rush knows something of. T is what its lookup says
/// of one.
pub struct LinkKind<T> {
    /// Names the kind, for link_info's callers: "github-pr".
    pub name: &'static str,
    /// The address as the kind knows it, so a link to a tab or an anchor
    /// of the same thing shares one answer; "" when u isn't one.
    pub key: fn(&str) -> String,
    /// Asks what there is to know of the address at key. It runs off the
    /// UI, a few at once.
    pub look: Look<T>,
    /// What's drawn after a link, a few coloured words; empty draws
    /// nothing. A change in it is what draws the conversation again.
    pub badge: fn(&T) -> Line,
    /// How long an answer stands before the next drawing asks again; None
    /// is for good. An answer that failed is asked again after
    /// LINK_FAILED_FOR.
    pub fresh: Option<fn(&T) -> Duration>,
}

/// How long a failed lookup stands: GitHub signed out or rate limited
/// isn't asked again on every frame.
const LINK_FAILED_FOR: Duration = Duration::from_secs(10 * 60);

type Value = Arc<dyn Any + Send + Sync>;

/// A LinkKind with its T put away, so kinds of every T share a list.
struct Erased {
    name: &'static str,
    key: fn(&str) -> String,
    look: Look<Value>,
    badge: Arc<dyn Fn(&Value) -> Line + Send + Sync>,
    fresh: Arc<dyn Fn(&Value) -> Duration + Send + Sync>,
}

/// What a kind's lookup last said of one address.
struct Answer {
    kind: Arc<Erased>,
    v: Option<Value>,
    /// Its last answer was a failure.
    failed: bool,
    at: Option<Instant>,
    asking: bool,
}

impl Answer {
    /// Whether the answer is old enough to ask again.
    fn due(&self) -> bool {
        let Some(at) = self.at else {
            return true;
        };
        if self.failed {
            return at.elapsed() >= LINK_FAILED_FOR;
        }
        let d = self
            .v
            .as_ref()
            .map_or(Duration::ZERO, |v| (self.kind.fresh)(v));
        d > Duration::ZERO && at.elapsed() >= d
    }
}

static KINDS: RwLock<Vec<Arc<Erased>>> = RwLock::new(Vec::new());
static ANSWERS: LazyLock<Mutex<HashMap<String, Answer>>> = LazyLock::new(Default::default);
/// Whether lookups run at all: off, as in tests, every link is drawn bare
/// and nothing is asked.
static LIVE: AtomicBool = AtomicBool::new(false);
/// Moves each time what a link draws after it may have changed.
static GEN: AtomicU64 = AtomicU64::new(0);
/// Told when an answer lands: the UI draws again.
static ON_LOOKUP: RwLock<Option<fn()>> = RwLock::new(None);
/// Only a few lookups at once: a page of links asks about each the first
/// time it's drawn.
static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(3), Condvar::new());

fn moved() {
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(f) = *ON_LOOKUP.read().unwrap_or_else(PoisonError::into_inner) {
        f();
    }
}

/// Adds a kind of link. A kind registered before it that also knows an
/// address wins it.
pub fn register_link<T: Any + Send + Sync>(k: LinkKind<T>) {
    github();
    push_kind(k);
}

fn push_kind<T: Any + Send + Sync>(k: LinkKind<T>) {
    let LinkKind {
        name,
        key,
        look,
        badge,
        fresh,
    } = k;
    let e = Erased {
        name,
        key,
        look: Arc::new(move |key| look(key).map(|v| Arc::new(v) as Value)),
        badge: Arc::new(move |v| v.downcast_ref::<T>().map_or_else(Line::new, badge)),
        fresh: Arc::new(move |v| match (fresh, v.downcast_ref::<T>()) {
            (Some(f), Some(t)) => f(t),
            _ => Duration::ZERO,
        }),
    };
    KINDS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Arc::new(e));
}

/// Turns link lookups on or off: the UI turns them on.
pub fn look_links(on: bool) {
    if LIVE.swap(on, Ordering::SeqCst) != on {
        GEN.fetch_add(1, Ordering::SeqCst);
    }
}

/// How the UI is told an answer landed, to draw again.
pub fn on_lookup(f: fn()) {
    *ON_LOOKUP.write().unwrap_or_else(PoisonError::into_inner) = Some(f);
}

/// Moves each time a link may draw differently: a renderer that drew with
/// another draws again.
pub fn lookups_gen() -> u64 {
    GEN.load(Ordering::SeqCst)
}

/// The kind that knows u, and u's key for it.
fn kind_of(u: &str) -> Option<(Arc<Erased>, String)> {
    github();
    let kinds = KINDS.read().unwrap_or_else(PoisonError::into_inner);
    kinds.iter().find_map(|k| {
        let key = (k.key)(u);
        (!key.is_empty()).then(|| (k.clone(), key))
    })
}

/// What the kind that knows u last said of it: the kind's name, u's key
/// for it, and the answer, asking again in the background when that
/// answer is old. None for an address no kind knows, and until its lookup
/// has answered. Callers type the answer by the kind's name.
pub fn link_info(u: &str) -> Option<(&'static str, String, Value)> {
    if !LIVE.load(Ordering::SeqCst) {
        return None;
    }
    let (k, key) = kind_of(u)?;
    let id = format!("{} {key}", k.name);
    let mut answers = ANSWERS.lock().unwrap_or_else(PoisonError::into_inner);
    let a = answers.entry(id.clone()).or_insert_with(|| Answer {
        kind: k.clone(),
        v: None,
        failed: false,
        at: None,
        asking: false,
    });
    if !a.asking && a.due() {
        a.asking = true;
        let (k, key) = (k.clone(), key.clone());
        std::thread::spawn(move || ask(&id, &k, &key));
    }
    a.v.clone().map(|v| (k.name, key, v))
}

/// link_info's answer as a T, for a caller that knows which kind knows u.
pub fn link_info_of<T: Any + Send + Sync + Clone>(kind: &str, u: &str) -> Option<T> {
    let (name, _, v) = link_info(u)?;
    (name == kind).then(|| v.downcast_ref::<T>().cloned())?
}

fn ask(id: &str, k: &Erased, key: &str) {
    {
        let mut free = SLOTS.0.lock().unwrap_or_else(PoisonError::into_inner);
        while *free == 0 {
            free = SLOTS.1.wait(free).unwrap_or_else(PoisonError::into_inner);
        }
        *free -= 1;
    }
    let got = (k.look)(key);
    *SLOTS.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
    SLOTS.1.notify_one();
    let changed = {
        let mut answers = ANSWERS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(a) = answers.get_mut(id) else {
            return;
        };
        let changed = match (&got, &a.v) {
            (Ok(v), Some(old)) => (k.badge)(old) != (k.badge)(v),
            (Ok(_), None) => true,
            (Err(_), _) => false,
        };
        a.asking = false;
        a.at = Some(Instant::now());
        a.failed = got.is_err();
        if let Ok(v) = got {
            a.v = Some(v);
        }
        changed
    };
    if changed {
        moved(); // drawn again with it
    }
}

/// Has the next drawing of a link to u ask again, as after something was
/// done to what it points at from rush.
pub fn forget_link(u: &str) {
    let Some((k, key)) = kind_of(u) else {
        return;
    };
    if let Some(a) = ANSWERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_mut(&format!("{} {key}", k.name))
    {
        a.at = None;
    }
    moved();
}

/// Forgets every answer: for tests.
pub fn forget_links() {
    ANSWERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// What's drawn after a link to u: a space and what its kind knows of it,
/// or nothing when no kind knows u or its lookup hasn't answered yet.
pub fn link_after(u: &str) -> Line {
    let Some((_, _, v)) = link_info(u) else {
        return Line::new();
    };
    let Some((k, _)) = kind_of(u) else {
        return Line::new();
    };
    let b = (k.badge)(&v);
    if b.spans.iter().all(|s| s.text.is_empty()) {
        return Line::new();
    }
    let mut spans = vec![Span::new(" ", Default::default())];
    spans.extend(b.spans);
    Line::from_spans(spans)
}

/// The kinds of GitHub link rush knows: pull requests and issues, asked
/// through gh.
pub const LINK_GITHUB_PR: &str = "github-pr";
pub const LINK_GITHUB_ISSUE: &str = "github-issue";

/// Registers the GitHub kinds, once, ahead of any registered after.
fn github() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        push_kind(LinkKind::<PR> {
            name: LINK_GITHUB_PR,
            key: |u| ghpr::parse_url(u).map_or_else(String::new, |(r, n)| ghpr::pr_url(&r, n)),
            look: Arc::new(|key| Gh::shared().view(key).map_err(|e| e.to_string())),
            badge: pr_badge,
            // An open pull request's checks and review move; a merged or
            // closed one stays.
            fresh: Some(|p| {
                if p.state == "OPEN" {
                    Duration::from_secs(120)
                } else {
                    Duration::ZERO
                }
            }),
        });
        push_kind(LinkKind::<Issue> {
            name: LINK_GITHUB_ISSUE,
            key: |u| {
                ghpr::parse_issue_url(u).map_or_else(String::new, |(r, n)| ghpr::issue_url(&r, n))
            },
            look: Arc::new(|key| Gh::shared().view_issue(key).map_err(|e| e.to_string())),
            badge: issue_badge,
            fresh: Some(|_| Duration::from_secs(10 * 60)),
        });
    });
}

/// Where a pull request stands, in a few coloured words: merged, closed, a
/// draft, or open with its checks and review.
pub fn pr_badge(p: &PR) -> Line {
    match p.state.as_str() {
        "MERGED" => return paint(C_BLUE, "⇡ merged").into_line(),
        "CLOSED" => return dim("⇡ closed").into_line(),
        _ => {}
    }
    let mut s = if p.draft {
        dim("⇡ draft")
    } else {
        paint(C_GREEN, "⇡ open")
    };
    let (ok, failed, pending) = p.counts();
    let dot = || faint(" · ");
    if failed > 0 {
        s = s
            .and(dot())
            .and(paint(C_RED, &format!("✗ {failed} failing")));
    } else if pending > 0 {
        s = s.and(dot()).and(sub(&format!("{pending} running")));
    } else if ok > 0 {
        s = s.and(dot()).and(paint(C_GREEN, "✓ checks"));
    }
    s = match p.decision.as_str() {
        "APPROVED" => s.and(dot()).and(paint(C_GREEN, "approved")),
        "CHANGES_REQUESTED" => s.and(dot()).and(paint(C_RED, "changes asked")),
        "REVIEW_REQUIRED" => s.and(dot()).and(paint(C_YELLOW, "needs review")),
        _ => s,
    };
    s.into_line()
}

/// Whether an issue is open or closed, and how it closed.
pub fn issue_badge(i: &Issue) -> Line {
    let t: Txt = if i.state != "CLOSED" {
        paint(C_GREEN, "◎ open")
    } else {
        match i.reason.as_str() {
            "NOT_PLANNED" => dim("◎ not planned"),
            "DUPLICATE" => dim("◎ duplicate"),
            _ => paint(C_BLUE, "◎ closed"),
        }
    };
    t.into_line()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_gh::ghpr::Check;

    fn check(state: &str) -> Check {
        Check {
            state: state.into(),
            ..Check::default()
        }
    }

    // linkinfo_test.go: TestPRBadge.
    #[test]
    fn pr_and_issue_badges() {
        let pr = |state: &str, draft: bool, decision: &str, checks: &[&str]| PR {
            state: state.into(),
            draft,
            decision: decision.into(),
            checks: checks.iter().map(|c| check(c)).collect(),
            ..PR::default()
        };
        for (p, want) in [
            (pr("MERGED", false, "", &[]), "⇡ merged"),
            (pr("CLOSED", false, "", &[]), "⇡ closed"),
            (pr("OPEN", true, "", &[]), "⇡ draft"),
            (
                pr("OPEN", false, "APPROVED", &["passed"]),
                "⇡ open · ✓ checks · approved",
            ),
            (
                pr(
                    "OPEN",
                    false,
                    "REVIEW_REQUIRED",
                    &["passed", "failed", "failed"],
                ),
                "⇡ open · ✗ 2 failing · needs review",
            ),
            (pr("OPEN", false, "", &["pending"]), "⇡ open · 1 running"),
        ] {
            assert_eq!(pr_badge(&p).plain(), want);
        }
        let i = Issue {
            state: "CLOSED".into(),
            reason: "NOT_PLANNED".into(),
            ..Issue::default()
        };
        assert_eq!(issue_badge(&i).plain(), "◎ not planned");
    }

    #[test]
    fn github_keys() {
        github();
        let (k, key) = kind_of("https://github.com/acme/terrafied/pull/33/files").unwrap();
        assert_eq!(
            (k.name, key.as_str()),
            (LINK_GITHUB_PR, "https://github.com/acme/terrafied/pull/33")
        );
        let (k, _) = kind_of("https://github.com/acme/terrafied/issues/4").unwrap();
        assert_eq!(k.name, LINK_GITHUB_ISSUE);
        assert!(kind_of("https://example.com/x").is_none());
    }
}
