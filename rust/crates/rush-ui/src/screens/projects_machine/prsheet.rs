//! One pull request, whole: its branches, reviews, checks, description and
//! conversation, and what you can do on GitHub from it (comment, approve,
//! ask for changes, merge). Go: internal/ui/prs.go (prSheet :298,
//! openPRSheet :316, readComments :323, act :340, width :364, pr :367,
//! lines :378, htmlComment :454, prMarkdown :462, conversation :467, body
//! :487, keys :519, key :531, askApprove :586, askMerge :598, mergeDetail
//! :650, writeKey :692, mouse :713). GitHub is asked off the UI.

use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::Line;
use rush_gh::ghpr::{Comment, Error as GhError, Gh, PR};

use super::data::*;
use super::rows::*;
use super::scan;
use super::view::pr_review_word;
use crate::frame::confirm::{Confirm, Then};
use crate::frame::editor::edit;
use crate::frame::sheet::{Sheet, sheet_title, text_field};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// A pull request whole.
pub(crate) struct PrSheet {
    pub root: String,
    pub num: u64,
    pub top: usize,
    /// As last drawn, should GitHub's next answer drop it.
    pub last: PR,
    pub comments: Vec<Comment>,
    /// Why the conversation couldn't be read, or "" once it is.
    pub said: String,
    pub read: bool,
    /// What's being written: "comment" or "changes" (a review asking for
    /// them); input and pos the text and where the cursor is.
    pub writing: String,
    pub input: Vec<char>,
    pub pos: usize,
    /// What's on its way to GitHub.
    pub sending: String,
}

/// What bots hide in what they write: markers no one reads.
static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("a pattern"));

/// Opens a pull request whole, and asks GitHub for its conversation.
pub(crate) fn open_pr_sheet(m: &mut Model, root: &str, pr: &PR) {
    m.open_sheet(Box::new(PrSheet {
        root: root.to_string(),
        num: pr.number,
        top: 0,
        last: pr.clone(),
        comments: Vec::new(),
        said: String::new(),
        read: false,
        writing: String::new(),
        input: Vec::new(),
        pos: 0,
        sending: String::new(),
    }));
    read_comments(m, root.to_string(), pr.number);
}

/// The open PR sheet, when it's still the one for (root, n).
fn sheet_of<'a>(m: &'a mut Model, root: &str, n: u64) -> Option<&'a mut PrSheet> {
    m.sheet_as::<PrSheet>()
        .filter(|s| s.root == root && s.num == n)
}

/// Asks GitHub for the pull request's conversation.
fn read_comments(m: &mut Model, root: String, n: u64) {
    if data(m).frozen {
        return;
    }
    let r = root.clone();
    m.later(
        move || Gh::shared().comments(&r, n),
        move |m, got| {
            if let Some(s) = sheet_of(m, &root, n) {
                s.read = true;
                match got {
                    Ok(cs) => (s.comments, s.said) = (cs, String::new()),
                    Err(e) => (s.comments, s.said) = (Vec::new(), e.to_string()),
                }
            }
            Out::Draw
        },
    );
}

/// What's done once GitHub has answered, before the rest.
type After = Box<dyn FnOnce(&mut Model, &Result<(), GhError>) + Send>;

/// Does something on GitHub as you, then says how it went and asks GitHub
/// again of the pull request. after runs first, with how it went.
fn act(
    m: &mut Model,
    root: String,
    n: u64,
    done: String,
    f: impl FnOnce(&Gh) -> Result<(), GhError> + Send + 'static,
    after: Option<After>,
) {
    let r = root.clone();
    m.later(
        move || f(Gh::shared()),
        move |m, res| {
            if let Some(s) = sheet_of(m, &root, n) {
                s.sending.clear();
            }
            if let Some(a) = after {
                a(m, &res);
            }
            if let Err(e) = res {
                m.fail(&format!("GitHub said no: {e}"));
                return Out::Draw;
            }
            m.flash(&done);
            if let Some(repo) = m.kept::<Data>().prs.get_mut(&r) {
                repo.next = 0;
            }
            scan::refresh_prs(m);
            read_comments(m, root, n);
            Out::Draw
        },
    );
}

impl PrSheet {
    /// The pull request as GitHub last said it.
    fn pr(&self, m: &Model) -> PR {
        data(m)
            .prs
            .get(&self.root)
            .and_then(|r| r.prs.iter().find(|p| p.number == self.num))
            .cloned()
            .unwrap_or_else(|| self.last.clone())
    }

    fn lines(&self, m: &Model, pr: &PR, w: usize) -> Vec<Line> {
        let mut out = Vec::new();
        let who = if pr.mine {
            "you want ".to_string()
        } else {
            format!("@{} wants ", pr.author)
        };
        let mut l = ln([
            sp(who, DIM),
            sp(&pr.head, BLUE),
            sp(" into ", DIM),
            sp(&pr.base, SUB),
        ]);
        if pr.updated != 0 {
            l.spans.push(sp(
                format!(" · updated {} ago", age(m.now - pr.updated)),
                DIM,
            ));
        }
        out.extend([l, Line::new()]);

        // Reviews: who's said what, and who it still waits on.
        out.push(psection("Review", &text_of(&pr_review_word(pr)), w));
        if pr.reviews.is_empty() && pr.waiting.is_empty() {
            out.push(ln([sp("  no one has reviewed it or been asked to", DIM)]));
        }
        for r in &pr.reviews {
            let (mark, col, word) = match r.state.as_str() {
                "APPROVED" => ("✓", GREEN, "approved"),
                "CHANGES_REQUESTED" => ("✗", RED, "asked for changes"),
                "DISMISSED" => ("○", DIM, "dismissed"),
                _ => ("●", SUB, "commented"),
            };
            let mut l = ln([
                plain("  "),
                sp(format!("{mark} {}", r.who), col),
                sp(format!(" {word}"), DIM),
            ]);
            let b = one_line(&HTML_COMMENT.replace_all(&r.body, ""));
            if !b.is_empty() {
                l.spans.push(sp(" · ", DIM));
                l.spans.push(sp(b, FAINT));
            }
            out.push(fit(l, w));
        }
        for q in &pr.waiting {
            out.push(if q == "you" {
                ln([
                    plain("  "),
                    spb("◌ you", ORANGE),
                    sp(" are asked to review it", DIM),
                ])
            } else {
                ln([
                    plain("  "),
                    sp(format!("◌ {q}"), YELLOW),
                    sp(" asked, not reviewed yet", DIM),
                ])
            });
        }

        // Checks, failed first.
        let (ok, failed, pending) = pr.counts();
        let mut meta = format!("{ok} of {} passed", pr.checks.len());
        if failed > 0 {
            meta += &format!(" · {failed} failed");
        }
        if pending > 0 {
            meta += &format!(" · {pending} running");
        }
        out.extend([Line::new(), psection("Checks", &meta, w)]);
        if pr.checks.is_empty() {
            out.push(ln([sp("  none", DIM)]));
        }
        for want in ["failed", "pending", "passed", "skipped"] {
            for c in pr.checks.iter().filter(|c| c.state == want) {
                let mark = match want {
                    "failed" => sp("✗", RED),
                    "pending" => sp(SPINNER[m.tick % SPINNER.len()], ORANGE),
                    "passed" => sp("✓", GREEN),
                    _ => sp("–", FAINT),
                };
                let name = if c.workflow.is_empty() {
                    c.name.clone()
                } else {
                    format!("{} / {}", c.workflow, c.name)
                };
                out.push(fit(ln([plain("  "), mark, plain(" "), sp(name, TEXT)]), w));
            }
        }

        out.extend([Line::new(), psection("Description", "", w)]);
        if pr.body.trim().is_empty() {
            out.push(ln([sp("  no description", DIM)]));
        } else {
            out.extend(pr_markdown(&pr.body, w));
        }
        out.extend(self.conversation(m, w));
        out
    }

    /// What's been said on the pull request, oldest first.
    fn conversation(&self, m: &Model, w: usize) -> Vec<Line> {
        if self.root.is_empty() {
            return Vec::new();
        }
        if !self.read {
            return vec![
                Line::new(),
                psection("Conversation", "", w),
                ln([sp("  asking GitHub…", FAINT)]),
            ];
        }
        if !self.said.is_empty() {
            return vec![
                Line::new(),
                psection("Conversation", "", w),
                ln([sp(format!("  {}", self.said), DIM)]),
            ];
        }
        let mut out = vec![
            Line::new(),
            psection("Conversation", &self.comments.len().to_string(), w),
        ];
        if self.comments.is_empty() {
            out.push(ln([sp("  no one has said anything yet", DIM)]));
        }
        for c in &self.comments {
            out.push(Line::new());
            out.push(ln([
                plain("  "),
                spb(&c.who, SUB),
                sp(format!(" · {} ago", age(m.now - c.at)), DIM),
            ]));
            out.extend(pr_markdown(&c.body, w.saturating_sub(2)));
        }
        out
    }

    /// What the sheet's keys do here: no reviewing your own pull request,
    /// which GitHub refuses, nor acting on one GitHub hasn't listed.
    fn keys(&self, pr: &PR, w: usize) -> Line {
        let mut k = vec![("↑↓", "scroll"), ("o", "on GitHub"), ("y", "copy link")];
        if !self.root.is_empty() && pr.state == "OPEN" {
            k.push(("c", "comment"));
            if !pr.mine {
                k.extend([("a", "approve"), ("x", "ask for changes")]);
            }
            k.push(("m", "merge…"));
        }
        k.push(("esc", "back"));
        keys_fit(w, &k)
    }

    /// Types a comment, or what to change, and posts it with enter.
    fn write_key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" => {
                self.writing.clear();
                (self.input, self.pos) = (Vec::new(), 0);
            }
            "enter" => {
                let text: String = self.input.iter().collect::<String>().trim().to_string();
                if text.is_empty() {
                    return Out::Idle;
                }
                let (root, n) = (self.root.clone(), self.num);
                let what = std::mem::take(&mut self.writing);
                (self.input, self.pos) = (Vec::new(), 0);
                if what == "changes" {
                    self.sending = "asking for changes".into();
                    act(
                        m,
                        root.clone(),
                        n,
                        format!("asked for changes on #{n}"),
                        move |gh| gh.request_changes(&root, n, &text),
                        None,
                    );
                } else {
                    self.sending = "commenting".into();
                    act(
                        m,
                        root.clone(),
                        n,
                        format!("commented on #{n}"),
                        move |gh| gh.say(&root, n, &text),
                        None,
                    );
                }
            }
            _ => match edit(&self.input, self.pos, k) {
                Some((b, p)) => (self.input, self.pos) = (b, p),
                None => return Out::Idle,
            },
        }
        Out::Draw
    }
}

/// Something said on GitHub drawn w wide.
fn pr_markdown(s: &str, w: usize) -> Vec<Line> {
    rush_convo::render::markdown_lines(HTML_COMMENT.replace_all(s, "").trim(), w)
}

/// Asks before approving the pull request as you.
fn ask_approve(m: &mut Model, root: String, pr: &PR) {
    let n = pr.number;
    m.ask(
        Confirm::new(format!("Approve #{n} on GitHub, as you?"), move |m| {
            if let Some(s) = sheet_of(m, &root, n) {
                s.sending = "approving".into();
            }
            let r = root.clone();
            act(
                m,
                root,
                n,
                format!("approved #{n}"),
                move |gh| gh.approve(&r, n, ""),
                None,
            );
            Out::Draw
        })
        .detail(one_line(&pr.title)),
    );
}

/// Asks how to merge the pull request, saying what's not ready.
fn ask_merge(m: &mut Model, root: String, pr: &PR) {
    let n = pr.number;
    let key = format!("{root}#{n}");
    let base = if pr.base.is_empty() {
        "its base".to_string()
    } else {
        pr.base.clone()
    };
    let merge = |how: &'static str, admin: bool| -> Then {
        let (root, key, base) = (root.clone(), key.clone(), base.clone());
        Box::new(move |m: &mut Model| {
            let d = m.kept::<Data>();
            d.merging.insert(key.clone(), how.into());
            d.merge_failed.remove(&key);
            if let Some(s) = sheet_of(m, &root, n) {
                s.sending = "merging".into();
            }
            let r = root.clone();
            act(
                m,
                root.clone(),
                n,
                format!("merged #{n} into {base}"),
                move |gh| gh.merge(&r, n, how, admin),
                Some(Box::new(move |m: &mut Model, res: &Result<(), GhError>| {
                    let d = m.kept::<Data>();
                    d.merging.remove(&key);
                    let mut ev =
                        serde_json::json!({"root": root, "number": n, "how": how, "admin": admin});
                    match res {
                        Err(e) => {
                            ev["error"] = e.to_string().into();
                            let admin = matches!(e, GhError::Merge(me) if me.admin);
                            // The flash goes in seconds; the card keeps it.
                            d.merge_failed.insert(key.clone(), (e.to_string(), admin));
                        }
                        Ok(()) => {
                            d.merged.insert(key.clone());
                            // Its branch's status, which the list falls
                            // back on once it's no longer open.
                            let r = root.clone();
                            m.later(move || Gh::shared().refresh(&r), |_, ()| Out::Idle);
                        }
                    }
                    rush_sys::events::log("pr-merge", ev);
                })),
            );
            Out::Draw
        })
    };
    let mut more: Vec<(String, String, Then)> = vec![
        ("r".into(), "rebase".into(), merge("rebase", false)),
        ("M".into(), "merge commit".into(), merge("merge", false)),
    ];
    let mut detail = merge_detail(pr);
    if let Some((err, admin)) = data(m).merge_failed.get(&key) {
        detail += &format!("\n\nlast try failed: {err}");
        if *admin {
            more.push(("a".into(), "squash as admin".into(), merge("squash", true)));
        }
    }
    m.ask(Confirm {
        question: format!("Merge #{n} on GitHub?"),
        detail,
        yes_text: "squash".into(),
        on_yes: Some(merge("squash", false)),
        more,
        ..Confirm::default()
    });
}

/// What a merge would do, and what stands against it: the branches, its
/// size, its checks and review, and what each way leaves.
pub(crate) fn merge_detail(pr: &PR) -> String {
    let or = |s: &str, d: &str| {
        if s.is_empty() {
            d.to_string()
        } else {
            s.to_string()
        }
    };
    let base = or(&pr.base, "its base");
    let mut lines = vec![format!("{} → {base}", or(&pr.head, "its branch"))];
    let mut size = Vec::new();
    if pr.added + pr.removed > 0 {
        size.push(format!("+{} −{}", pr.added, pr.removed));
    }
    if pr.files > 0 {
        size.push(format!("{} file{}", pr.files, plural(pr.files as usize)));
    }
    if pr.comments > 0 {
        size.push(format!("{} comment{}", pr.comments, plural(pr.comments)));
    }
    if !size.is_empty() {
        lines.push(size.join(" · "));
    }
    let (ok, failed, pending) = pr.counts();
    let checks = if failed + pending > 0 {
        format!("checks: {failed} failed, {pending} still running, {ok} passed")
    } else if ok > 0 {
        format!("checks: all {ok} passed")
    } else {
        "checks: none run on it".into()
    };
    let review = match pr.decision.as_str() {
        "APPROVED" => "review: approved",
        "CHANGES_REQUESTED" => "review: changes asked for",
        "REVIEW_REQUIRED" => "review: needed, not given",
        _ => "review: none needed",
    };
    lines.push(format!("{checks} · {review}"));
    lines.push(String::new());
    lines.push(format!(
        "squash: one commit on {base}, titled \"{}\"",
        pr.title
    ));
    lines.push(format!("rebase: each of its commits put on {base} in turn"));
    lines.push("merge commit: its commits as they are, joined by a merge".into());
    lines.join("\n")
}

impl Sheet for PrSheet {
    fn width(&self, m: &Model) -> usize {
        140.min(m.w.saturating_sub(6))
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let pr = self.pr(m);
        self.last = pr.clone();
        let state = if pr.draft {
            "draft".to_string()
        } else {
            pr.state.to_lowercase()
        };
        let head = vec![
            sheet_title(&format!("#{} {}", pr.number, pr.title), &state, w),
            Line::new(),
        ];
        let mut foot = vec![Line::new(), self.keys(&pr, w)];
        if !self.sending.is_empty() {
            foot[1] = ln([sp(
                format!("{} {}…", SPINNER[m.tick % SPINNER.len()], self.sending),
                ORANGE,
            )]);
        } else if !self.writing.is_empty() {
            let label = if self.writing == "changes" {
                "what to change ❯ "
            } else {
                "comment ❯ "
            };
            let mut l = ln([sp(label, ORANGE)]);
            join(
                &mut l,
                text_field(
                    &self.input,
                    self.pos,
                    true,
                    "posted on GitHub as you",
                    w.saturating_sub(rush_core::text::width(label) + 2),
                ),
            );
            foot = vec![
                Line::new(),
                l,
                keys_fit(w, &[("enter", "post it"), ("esc", "don't")]),
            ];
        }
        let all = self.lines(m, &pr, w);
        let room = h.saturating_sub(head.len() + foot.len()).max(3);
        self.top = self.top.min(all.len().saturating_sub(room));
        let end = all.len().min(self.top + room);
        if end < all.len() {
            foot[0] = ln([sp(format!("  ↓ {} more", all.len() - end), FAINT)]);
        }
        let mut out = head;
        out.extend(all[self.top..end].iter().cloned());
        out.extend(foot);
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if !self.sending.is_empty() && k.s != "esc" {
            return Out::Idle;
        }
        if !self.writing.is_empty() {
            return self.write_key(m, k);
        }
        let pr = self.pr(m);
        let open = !self.root.is_empty() && pr.state == "OPEN";
        match k.s.as_str() {
            "c" => {
                if open {
                    self.writing = "comment".into();
                    (self.input, self.pos) = (Vec::new(), 0);
                }
            }
            "x" => {
                if open && !pr.mine {
                    self.writing = "changes".into();
                    (self.input, self.pos) = (Vec::new(), 0);
                }
            }
            "a" => {
                if open && !pr.mine {
                    ask_approve(m, self.root.clone(), &pr);
                }
            }
            "m" => {
                if open {
                    ask_merge(m, self.root.clone(), &pr);
                }
            }
            "esc" | "q" | "left" => return Out::Close,
            "up" | "k" => self.top = self.top.saturating_sub(1),
            "down" | "j" => self.top += 1,
            "pgup" | "shift+space" => self.top = self.top.saturating_sub(10),
            "pgdown" | "space" => self.top += 10,
            "home" | "g" => self.top = 0,
            "end" | "G" => self.top = 1 << 30,
            "o" | "enter" => super::place::browse(m, &pr.url),
            "y" => m.copy_text(pr.url.clone()),
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_gh::ghpr::{Check, Review};

    fn pr() -> PR {
        PR {
            number: 42,
            title: "Fix the parser".into(),
            url: "https://github.com/o/r/pull/42".into(),
            state: "OPEN".into(),
            author: "sam".into(),
            head: "fix/parser".into(),
            base: "main".into(),
            decision: "REVIEW_REQUIRED".into(),
            waiting: vec!["you".into(), "kim".into()],
            reviews: vec![Review {
                who: "lee".into(),
                state: "CHANGES_REQUESTED".into(),
                body: "<!-- bot -->tabs please".into(),
            }],
            checks: vec![
                Check {
                    name: "test".into(),
                    workflow: "CI".into(),
                    state: "passed".into(),
                    ..Check::default()
                },
                Check {
                    name: "lint".into(),
                    state: "failed".into(),
                    ..Check::default()
                },
            ],
            body: "It fixes **the** parser.".into(),
            added: 10,
            removed: 2,
            files: 3,
            comments: 1,
            ..PR::default()
        }
    }

    fn sheet(read: bool) -> PrSheet {
        PrSheet {
            root: "/src/r".into(),
            num: 42,
            top: 0,
            last: pr(),
            comments: vec![Comment {
                who: "kim".into(),
                body: "looks good".into(),
                at: 0,
            }],
            said: String::new(),
            read,
            writing: String::new(),
            input: Vec::new(),
            pos: 0,
            sending: String::new(),
        }
    }

    fn text(l: &[Line]) -> String {
        l.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn says_it_whole() {
        let mut m = Model::new("t");
        m.w = 160;
        m.now = 2 * 3_600_000;
        let mut s = sheet(true);
        let t = text(&s.body(&m, 120, 60));
        for want in [
            "#42 Fix the parser  open",
            "@sam wants fix/parser into main",
            "REVIEW",
            "✗ lee asked for changes · tabs please",
            "◌ you are asked to review it",
            "◌ kim asked, not reviewed yet",
            "CHECKS  1 of 2 passed · 1 failed",
            "✗ lint",
            "✓ CI / test",
            "DESCRIPTION",
            "the",
            "CONVERSATION  1",
            "kim · 2h ago",
            "looks good",
            "c comment",
            "a approve",
            "m merge…",
        ] {
            assert!(t.contains(want), "lacks {want:?}:\n{t}");
        }
        assert!(
            t.find("✗ lint").unwrap() < t.find("✓ CI / test").unwrap(),
            "failed first"
        );
        assert!(!t.contains("bot"), "what bots hide stays hidden");
        let t = text(&sheet(false).body(&m, 120, 60));
        assert!(t.contains("asking GitHub…"));
        // a asks first.
        s.key(&mut m, &Press::of("a").unwrap());
        assert_eq!(
            m.confirm.as_ref().map(|c| c.question.as_str()),
            Some("Approve #42 on GitHub, as you?")
        );
    }

    #[test]
    fn your_own_isnt_reviewed_and_a_closed_one_isnt_acted_on() {
        let mut m = Model::new("t");
        m.w = 160;
        let mut s = sheet(true);
        s.last.mine = true;
        let t = text(&s.body(&m, 120, 60));
        assert!(t.contains("you want fix/parser into main"));
        assert!(!t.contains("a approve") && t.contains("m merge…"));
        assert_eq!(s.key(&mut m, &Press::of("x").unwrap()), Out::Draw);
        assert!(s.writing.is_empty(), "no asking for changes on your own");
        s.last.state = "MERGED".into();
        assert!(!text(&s.body(&m, 120, 60)).contains("c comment"));
        s.key(&mut m, &Press::of("c").unwrap());
        assert!(s.writing.is_empty());
    }

    #[test]
    fn writing_a_comment() {
        let mut m = Model::new("t");
        m.w = 160;
        let mut s = sheet(true);
        s.key(&mut m, &Press::of("c").unwrap());
        assert_eq!(s.writing, "comment");
        for c in ["h", "i"] {
            s.key(&mut m, &Press::of(c).unwrap());
        }
        assert_eq!(s.input, ['h', 'i']);
        let t = text(&s.body(&m, 120, 60));
        assert!(
            t.contains("comment ❯ hi") && t.contains("enter post it"),
            "{t}"
        );
        s.key(&mut m, &Press::of("esc").unwrap());
        assert!(s.writing.is_empty() && s.input.is_empty());
        assert_eq!(s.key(&mut m, &Press::of("esc").unwrap()), Out::Close);
    }

    #[test]
    fn merging_says_what_it_does() {
        assert_eq!(
            merge_detail(&pr()),
            "fix/parser → main\n+10 −2 · 3 files · 1 comment\n\
             checks: 1 failed, 0 still running, 1 passed · review: needed, not given\n\n\
             squash: one commit on main, titled \"Fix the parser\"\n\
             rebase: each of its commits put on main in turn\n\
             merge commit: its commits as they are, joined by a merge"
        );
        let mut m = Model::new("t");
        ask_merge(&mut m, "/src/r".into(), &pr());
        let c = m.confirm.as_ref().expect("asks");
        assert_eq!(c.question, "Merge #42 on GitHub?");
        assert_eq!(c.yes_text, "squash");
        assert_eq!(
            c.more
                .iter()
                .map(|(k, t, _)| format!("{k} {t}"))
                .collect::<Vec<_>>(),
            ["r rebase", "M merge commit"]
        );
        m.kept::<Data>()
            .merge_failed
            .insert("/src/r#42".into(), ("rules".into(), true));
        ask_merge(&mut m, "/src/r".into(), &pr());
        let c = m.confirm.as_ref().unwrap();
        assert!(c.detail.ends_with("\n\nlast try failed: rules"));
        assert_eq!(c.more.len(), 3, "squash as admin");
    }
}
