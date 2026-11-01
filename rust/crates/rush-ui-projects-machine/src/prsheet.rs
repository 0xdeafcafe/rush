//! One pull request, whole: its branches, reviews, checks, description and
//! conversation, and what you can do on GitHub from it (comment, approve,
//! ask for changes, merge). Go: internal/ui/prs.go (prSheet :298,
//! openPRSheet :316, readComments :323, act :340, width :364, pr :367,
//! lines :378, htmlComment :454, prMarkdown :462, conversation :467, body
//! :487, keys :519, key :531, askApprove :586, askMerge :598, mergeDetail
//! :650, writeKey :692, mouse :713). One opened from a link (linkkinds,
//! linkkinds.go:53 openPRLink) has its url: GitHub is asked of it by that,
//! wherever its repository is, and it's read from the link's lookup
//! (prs.go:325 live, :328 sel, :331 mergeKey, :354 ForgetLink, :388 pr,
//! :566 url in pr-merge). GitHub is asked off the UI, and only ever acted
//! on from a key.

use std::sync::LazyLock;

use regex::Regex;
use rush_convo::linkinfo::{LINK_GITHUB_PR, forget_link, link_info_of};
use rush_core::text::Line;
use rush_gh::ghpr::{self, Comment, Error as GhError, Gh, PR};

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
pub struct PrSheet {
    pub root: String,
    pub num: u64,
    /// Its address when it was opened from a link: GitHub is asked of it
    /// by that, wherever its repository is, and root is "".
    pub url: String,
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
pub fn open_pr_sheet(m: &mut Model, root: &str, pr: &PR) {
    open_sheet(m, root, "", pr);
}

/// Opens a linked pull request in its sheet, asked of GitHub by its url
/// (openPRLink).
pub fn open_pr_link(m: &mut Model, url: &str, pr: &PR) {
    open_sheet(m, "", url, pr);
}

fn open_sheet(m: &mut Model, root: &str, url: &str, pr: &PR) {
    let s = PrSheet {
        root: root.to_string(),
        num: pr.number,
        url: url.to_string(),
        top: 0,
        last: pr.clone(),
        comments: Vec::new(),
        said: String::new(),
        read: false,
        writing: String::new(),
        input: Vec::new(),
        pos: 0,
        sending: String::new(),
    };
    let at = s.at();
    m.open_sheet(Box::new(s));
    read_comments(m, at);
}

/// Which pull request a sheet is, and how gh is asked of it: its
/// repository's folder ("" for one opened from its link), its number,
/// and its url.
#[derive(Clone)]
pub struct At {
    root: String,
    num: u64,
    url: String,
}

impl At {
    /// Names the pull request to gh (ghpr.Sel).
    fn sel(&self) -> String {
        ghpr::sel(self.num, &self.url)
    }

    /// The pull request in the merge maps (mergeKey).
    fn merge_key(&self) -> String {
        if self.root.is_empty() {
            self.url.clone()
        } else {
            format!("{}#{}", self.root, self.num)
        }
    }
}

/// The open PR sheet, when it's still the one at at.
fn sheet_of<'a>(m: &'a mut Model, at: &At) -> Option<&'a mut PrSheet> {
    m.sheet_as::<PrSheet>()
        .filter(|s| s.root == at.root && s.num == at.num && s.url == at.url)
}

/// Asks GitHub for the pull request's conversation.
fn read_comments(m: &mut Model, at: At) {
    if data(m).frozen {
        return;
    }
    let (r, sel) = (at.root.clone(), at.sel());
    m.later(
        move || Gh::shared().comments(&r, &sel),
        move |m, got| {
            if let Some(s) = sheet_of(m, &at) {
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
    at: At,
    done: String,
    f: impl FnOnce(&Gh) -> Result<(), GhError> + Send + 'static,
    after: Option<After>,
) {
    m.later(
        move || f(Gh::shared()),
        move |m, res| {
            if let Some(s) = sheet_of(m, &at) {
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
            if !at.url.is_empty() {
                forget_link(&at.url); // its link's badge, and this sheet, asked again
            }
            if let Some(repo) = m.kept::<Data>().prs.get_mut(&at.root) {
                repo.next = 0;
            }
            scan::refresh_prs(m);
            read_comments(m, at);
            Out::Draw
        },
    );
}

impl PrSheet {
    pub fn at(&self) -> At {
        At {
            root: self.root.clone(),
            num: self.num,
            url: self.url.clone(),
        }
    }

    /// Whether GitHub has listed the pull request, so it can be read and
    /// acted on: one in a repository's list, or opened from its link.
    fn live(&self) -> bool {
        !self.root.is_empty() || !self.url.is_empty()
    }

    /// The pull request as GitHub last said it: a linked one as its
    /// link's lookup last did.
    pub fn pr(&self, m: &Model) -> PR {
        if !self.url.is_empty() {
            return link_info_of::<PR>(LINK_GITHUB_PR, &self.url)
                .unwrap_or_else(|| self.last.clone());
        }
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
        if !self.live() {
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
    pub fn keys(&self, pr: &PR, w: usize) -> Line {
        let mut k = vec![("↑↓", "scroll"), ("o", "on GitHub"), ("y", "copy link")];
        if self.live() && pr.state == "OPEN" {
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
                let (at, n) = (self.at(), self.num);
                let (root, sel) = (at.root.clone(), at.sel());
                let what = std::mem::take(&mut self.writing);
                (self.input, self.pos) = (Vec::new(), 0);
                if what == "changes" {
                    self.sending = "asking for changes".into();
                    act(
                        m,
                        at,
                        format!("asked for changes on #{n}"),
                        move |gh| gh.request_changes(&root, &sel, &text),
                        None,
                    );
                } else {
                    self.sending = "commenting".into();
                    act(
                        m,
                        at,
                        format!("commented on #{n}"),
                        move |gh| gh.say(&root, &sel, &text),
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
fn ask_approve(m: &mut Model, at: At, pr: &PR) {
    let n = at.num;
    m.ask(
        Confirm::new(format!("Approve #{n} on GitHub, as you?"), move |m| {
            if let Some(s) = sheet_of(m, &at) {
                s.sending = "approving".into();
            }
            let (r, sel) = (at.root.clone(), at.sel());
            act(
                m,
                at,
                format!("approved #{n}"),
                move |gh| gh.approve(&r, &sel, ""),
                None,
            );
            Out::Draw
        })
        .detail(one_line(&pr.title)),
    );
}

/// Asks how to merge the pull request, saying what's not ready. Nothing
/// is merged until a key says how.
pub fn ask_merge(m: &mut Model, at: At, pr: &PR) {
    let n = at.num;
    let key = at.merge_key();
    let base = if pr.base.is_empty() {
        "its base".to_string()
    } else {
        pr.base.clone()
    };
    let merge = |how: &'static str, admin: bool| -> Then {
        let (at, key, base) = (at.clone(), key.clone(), base.clone());
        Box::new(move |m: &mut Model| {
            let d = m.kept::<Data>();
            d.merging.insert(key.clone(), how.into());
            d.merge_failed.remove(&key);
            if let Some(s) = sheet_of(m, &at) {
                s.sending = "merging".into();
            }
            let (root, url, sel) = (at.root.clone(), at.url.clone(), at.sel());
            let r = root.clone();
            act(
                m,
                at,
                format!("merged #{n} into {base}"),
                move |gh| gh.merge(&r, &sel, how, admin),
                Some(Box::new(move |m: &mut Model, res: &Result<(), GhError>| {
                    let d = m.kept::<Data>();
                    d.merging.remove(&key);
                    let mut ev = serde_json::json!({"root": root, "url": url, "number": n, "how": how, "admin": admin});
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

/// Asks how to merge root's pull request pr, no sheet open: the
/// finishing card's m (finishKey's prSheet{…}.askMerge).
pub fn ask_merge_pr(m: &mut Model, root: &str, pr: &PR) {
    let at = At {
        root: root.to_string(),
        num: pr.number,
        url: String::new(),
    };
    ask_merge(m, at, pr);
}

/// Asks how to merge the open sheet's pull request, pr as its link said
/// it (prLinkActs' Merge…).
pub fn ask_merge_open(m: &mut Model, pr: &PR) {
    if let Some(at) = m.sheet_as::<PrSheet>().map(|s| s.at()) {
        ask_merge(m, at, pr);
    }
}

/// What a merge would do, and what stands against it: the branches, its
/// size, its checks and review, and what each way leaves.
pub fn merge_detail(pr: &PR) -> String {
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
        let open = self.live() && pr.state == "OPEN";
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
                    ask_approve(m, self.at(), &pr);
                }
            }
            "m" => {
                if open {
                    ask_merge(m, self.at(), &pr);
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
