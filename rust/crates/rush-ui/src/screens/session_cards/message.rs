//! The message sheet: your messages as sent, whole, and what came with
//! them, one at a time. After Go's internal/ui/messagesheet.go
//! (messageSheet :19, body :148, key :189, mouse :253, openUserMessage
//! :280, jumpUserMessage :307, messageLink :336 (a link of a kind rush
//! opens itself first, :335-337), openImage :358,
//! messageTabs :387, draftImage :401) and convo/usermessages.go
//! (UserMessage :130, UserMessages :150, ExpandedMessage :166). An
//! image's tab draws it in quarter blocks (pictures.rs; prepare :58,
//! stripRow :100), with a strip of them all under it when there are
//! several. Gap: no Kitty pictures, so no s to flip one sharp.

use rush_convo::{Item, Session};
use rush_core::text::Line;

use super::imagerefs::ImageRefs;
use super::pictures;
use crate::frame::overlayclick::MouseEv;
use crate::frame::sheet::sheet_title;
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// One message of yours as sent: its ref in the conversation, its text
/// and the images that came with it (UserMessage).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserMessage {
    pub r#ref: String,
    pub text: String,
    /// Image files, or where rush keeps a copy of one embedded.
    pub images: Vec<String>,
}

/// Your messages in s, oldest first: each turn's prompt and each message
/// sent mid-turn (UserMessages).
pub fn user_messages(s: &Session) -> Vec<UserMessage> {
    let mut out = Vec::new();
    for t in &s.turns {
        if t.from.is_empty()
            && (!t.prompt.is_empty() || !t.images.is_empty() || !t.pictures.is_empty())
        {
            let mut images: Vec<String> = t.images.iter().map(|p| p.to_string()).collect();
            if images.is_empty() {
                images = t.pictures.iter().map(|p| p.path.to_string()).collect();
            }
            out.push(UserMessage {
                r#ref: format!("t{}", t.n),
                text: t.prompt.clone(),
                images,
            });
        }
        for (i, it) in t.items.iter().enumerate() {
            if let Item::Interject(x) = it {
                let mut images: Vec<String> = x.images.iter().map(|p| p.to_string()).collect();
                if images.is_empty() {
                    images = x.pictures.iter().map(|p| p.path.to_string()).collect();
                }
                out.push(UserMessage {
                    r#ref: format!("t{}:u:{i}", t.n),
                    text: x.text.clone(),
                    images,
                });
            }
        }
    }
    out
}

/// A message with its pastes as they were, their newlines kept
/// (ExpandedMessage).
pub fn expanded_message(text: &str) -> String {
    rush_convo::render::each_paste(text, |s| format!("\n{s}\n"))
}

/// The message sheet (messageSheet).
#[derive(Default)]
pub struct MessageSheet {
    pub messages: Vec<UserMessage>,
    pub at: usize,
    /// 0 its text, then each image.
    pub tab: usize,
    pub scroll: usize,
    pub page: usize,
    text_key: String,
    rows: Vec<String>,
    picture: Vec<Line>,
    /// The images small, when there are several.
    strip: Vec<Line>,
    /// The body line the strip starts on.
    strip_y: usize,
    /// Each thumbnail's left edge, and the tab it shows.
    strip_at: Vec<(usize, usize)>,
}

/// What a message of several images gives its strip under the picture: a
/// blank, the count, thumbnails three rows tall and a mark (stripRoom).
const STRIP_ROOM: usize = 6;

impl MessageSheet {
    pub fn new(messages: Vec<UserMessage>, at: usize) -> MessageSheet {
        MessageSheet {
            messages,
            at,
            ..MessageSheet::default()
        }
    }

    fn images(&self) -> &[String] {
        &self.messages[self.at].images
    }

    /// Fits the image shown to w by h cells, as pictures makes it off the
    /// UI thread, with a strip of all of them under it when there are
    /// several (prepare).
    fn prepare(&mut self, m: &Model, w: usize, mut h: usize) {
        self.picture.clear();
        self.strip.clear();
        self.strip_at.clear();
        if self.tab == 0 {
            return;
        }
        let pics = self.images().to_vec();
        if self.tab > pics.len() {
            self.tab = 0;
            return;
        }
        if pics.len() > 1 {
            h = h.saturating_sub(STRIP_ROOM);
            self.strip_row(m, &pics, w);
        }
        match pictures::picture(m, &pics[self.tab - 1], w.max(8), h.max(4)) {
            None => {}
            Some(rows) if rows.is_empty() => {
                self.picture = vec![line([sp(
                    "Image preview unavailable. The original file may no longer exist.",
                    DIM,
                )])];
            }
            Some(rows) => {
                let pad = " ".repeat(w.saturating_sub(rows[0].width()) / 2);
                self.picture = rows
                    .iter()
                    .map(|r| {
                        let mut l = line([plain(pad.clone())]);
                        l.spans.extend(r.spans.iter().cloned());
                        l
                    })
                    .collect();
            }
        }
    }

    /// The message's images small in one row w wide, as many as fit
    /// around the one shown, each where a click shows it; one not made yet
    /// stands as its number (stripRow).
    fn strip_row(&mut self, m: &Model, pics: &[String], w: usize) {
        const TW: usize = 12;
        let n = 1.max(pics.len().min((w + 2) / (TW + 2)));
        let from = (self.tab as isize - 1 - n as isize / 2)
            .min(pics.len() as isize - n as isize)
            .max(0) as usize;
        let thumbs: Vec<Vec<Line>> = (0..n)
            .map(|i| match pictures::picture(m, &pics[from + i], TW, 3) {
                Some(rows) if !rows.is_empty() => rows.to_vec(),
                _ => vec![line([
                    sp("▣ ", BLUE),
                    sp(fit_str(&(from + i + 1).to_string(), TW - 2), TEXT),
                ])],
            })
            .collect();
        let (mut rows, at) = pictures::tile(&thumbs, w, 2);
        let lead = w.saturating_sub(at[n - 1][0] + at[n - 1][2]) / 2;
        for r in &mut rows {
            r.spans.insert(0, plain(" ".repeat(lead)));
        }
        while rows.len() < 3 {
            rows.push(Line::new());
        }
        let cur = at[self.tab - 1 - from];
        rows.push(line([
            plain(" ".repeat(lead + cur[0])),
            sp("▔".repeat(cur[2]), ORANGE),
        ]));
        self.strip = rows;
        // Its left edge, and its tab.
        self.strip_at = at
            .iter()
            .enumerate()
            .map(|(i, a)| (lead + a[0], from + i + 1))
            .collect();
    }
}

/// s cut or padded to n cells (fit).
fn fit_str(s: &str, n: usize) -> String {
    let t = rush_draw::truncate(s, n, "…");
    format!(
        "{t}{}",
        " ".repeat(n.saturating_sub(rush_core::text::width(&t)))
    )
}

/// text cut into rows of w cells, words or not, its newlines kept
/// (ansi.Hardwrap).
pub fn hard_wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = Vec::new();
    for l in text.split('\n') {
        let mut row = String::new();
        let mut n = 0;
        for c in l.chars() {
            let cw = rush_core::text::width(c.encode_utf8(&mut [0; 4]));
            if n + cw > w {
                out.push(std::mem::take(&mut row));
                n = 0;
            }
            row.push(c);
            n += cw;
        }
        out.push(row);
    }
    out
}

/// The tabs: text and each image, the one showing bright (messageTabs).
pub fn message_tabs(names: &[String], selected: usize, w: usize) -> Line {
    let mut l = Line::new();
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            l.spans.push(plain("  ·  "));
        }
        if i == selected {
            l.spans.push(spb(n, ORANGE));
        } else {
            l.spans.push(sp(n, DIM));
        }
    }
    l.spans.push(sp("   tab", DIM));
    fit(l, w)
}

fn tab_names(n: usize) -> Vec<String> {
    std::iter::once("Text".to_string())
        .chain((1..=n).map(|i| format!("Image {i}")))
        .collect()
}

/// Opens image path in the system's viewer, at full size (openImage).
pub fn open_image(m: &mut Model, path: &str) {
    if path.is_empty() {
        m.flash_err("couldn't keep the image to open: rush keeps no copy of it");
        return;
    }
    let p = path.to_string();
    m.later(
        move || {
            std::process::Command::new("open")
                .arg(&p)
                .status()
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    if s.success() {
                        Ok(())
                    } else {
                        Err(format!("open said {s}"))
                    }
                })
        },
        |m, r| {
            match r {
                Ok(()) => m.flash("opened the image"),
                Err(e) => m.fail(&format!("couldn't open the image: {e}")),
            }
            Out::Draw
        },
    );
}

impl Sheet for MessageSheet {
    fn width(&self, m: &Model) -> usize {
        40.max(m.w.saturating_sub(6))
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.page = 1.max(h as isize - 5) as usize;
        self.prepare(m, w, self.page);
        let n = self.images().len();
        if self.tab > n {
            self.tab = 0;
        }
        let title = format!("Message {} of {}", self.at + 1, self.messages.len());
        let mut out = vec![
            sheet_title(&title, "complete sent text and attachments", w),
            message_tabs(&tab_names(n), self.tab, w),
            Line::new(),
        ];
        let mut hint =
            "↑↓ scroll · pgup/pgdown · [ ] messages · tab attachments · c copy · esc close";
        let rows: Vec<Line> = if self.tab == 0 {
            let key = format!("{}/{w}", self.at);
            if key != self.text_key {
                self.text_key = key;
                self.rows = hard_wrap(
                    &super::util::strip_ansi(&expanded_message(&self.messages[self.at].text)),
                    w,
                );
            }
            self.rows.iter().map(|r| line([plain(r.clone())])).collect()
        } else {
            hint =
                "↑↓ scroll · [ ] messages · tab attachments · o open original · c copy · esc close";
            if self.picture.is_empty() {
                vec![line([sp("Loading image…", DIM)])]
            } else {
                self.picture.clone()
            }
        };
        let mut page = self.page;
        if !self.strip.is_empty() {
            page = page.saturating_sub(STRIP_ROOM);
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(page));
        let end = rows.len().min(self.scroll + page);
        out.extend(rows[self.scroll..end].iter().cloned());
        if !self.strip.is_empty() {
            while out.len() < 3 + page {
                out.push(Line::new());
            }
            let count = format!("{} / {}", self.tab, n);
            out.push(Line::new());
            out.push(line([
                plain(" ".repeat(w.saturating_sub(count.len()) / 2)),
                spb(count, TEXT),
            ]));
            self.strip_y = out.len();
            out.extend(self.strip.iter().cloned());
            hint = "←→ images · click one to show it · [ ] messages · tab text · o open original · esc close";
        }
        out.push(Line::new());
        out.push(line([sp(hint, DIM)]));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let n = self.images().len();
        match k.s.as_str() {
            "esc" => return Out::Close,
            "c" => m.copy_text(expanded_message(&self.messages[self.at].text)),
            "o" => {
                if self.tab > 0 && self.tab <= n {
                    let p = self.images()[self.tab - 1].clone();
                    open_image(m, &p);
                }
            }
            "up" | "k" => self.scroll = self.scroll.saturating_sub(1),
            "down" | "j" => self.scroll += 1,
            "pgup" => self.scroll = self.scroll.saturating_sub(self.page.max(1)),
            "pgdown" | "space" | " " => self.scroll += self.page.max(1),
            "home" => self.scroll = 0,
            "end" => self.scroll = usize::MAX / 2,
            "[" | "ctrl+home" => {
                self.at = if k.s == "ctrl+home" {
                    0
                } else {
                    self.at.saturating_sub(1)
                };
                self.tab = 0;
                self.scroll = 0;
            }
            "]" | "ctrl+end" => {
                let last = self.messages.len().saturating_sub(1);
                self.at = if k.s == "ctrl+end" {
                    last
                } else {
                    (self.at + 1).min(last)
                };
                self.tab = 0;
                self.scroll = 0;
            }
            "tab" | "shift+tab" => {
                let t = n + 1;
                self.tab = if k.s == "tab" {
                    (self.tab + 1) % t
                } else {
                    (self.tab + t - 1) % t
                };
                self.scroll = 0;
            }
            "left" | "right" => {
                // Round the images: from the text, → is the first.
                if n > 0 {
                    self.tab = if self.tab == 0 {
                        1
                    } else if k.s == "left" {
                        (self.tab + n - 2) % n + 1
                    } else {
                        self.tab % n + 1
                    };
                    self.scroll = 0;
                }
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn mouse(&mut self, _m: &mut Model, ev: MouseEv, x: i32, y: i32) -> Out {
        if ev == MouseEv::Press && y == 1 {
            let mut col = 0i32;
            for (i, label) in tab_names(self.images().len()).iter().enumerate() {
                let n = rush_core::text::width(label) as i32;
                if x >= col && x < col + n {
                    self.tab = i;
                    self.scroll = 0;
                    return Out::Draw;
                }
                col += n + 5;
            }
        }
        let y = y.max(0) as usize;
        if ev == MouseEv::Press
            && !self.strip.is_empty()
            && y >= self.strip_y
            && y < self.strip_y + self.strip.len()
            && let Some(&(_, tab)) = self.strip_at.iter().rev().find(|a| x >= a.0 as i32)
        {
            self.tab = tab;
            self.scroll = 0;
            return Out::Draw;
        }
        match ev {
            MouseEv::WheelUp => self.scroll = self.scroll.saturating_sub(3),
            MouseEv::WheelDown => self.scroll += 3,
            _ => return Out::Pass,
        }
        Out::Draw
    }
}

/// Opens the message sheet on agent key's conversation, at the message
/// ref names (its turn's, failing that), else the latest
/// (openUserMessage).
pub fn open_user_message(m: &mut Model, key: &str, r#ref: &str) -> Out {
    let Some(v) = super::session_view(m, key) else {
        return Out::Pass;
    };
    let msgs = user_messages(&v.sess);
    if msgs.is_empty() {
        return Out::Pass;
    }
    let r = if r#ref.is_empty() {
        v.sel.clone()
    } else {
        r#ref.to_string()
    };
    let turn = r.split(':').next().unwrap_or("");
    let mut at = msgs.len() - 1;
    for (i, msg) in msgs.iter().enumerate() {
        if msg.r#ref == turn {
            at = i;
        }
        if msg.r#ref == r {
            at = i;
            break;
        }
    }
    m.open_sheet(Box::new(MessageSheet::new(msgs, at)));
    Out::Draw
}

/// The open Session's keys to your messages: ctrl+home and ctrl+end
/// pick your first and latest, alt+enter opens the latest whole, enter on
/// one picked opens it (rushmode.go:3833, :4104; a session_keys hook).
pub fn message_key_hook(m: &mut Model, k: &Press) -> Out {
    let Some(key) = super::open_key(m) else {
        return Out::Pass;
    };
    match k.s.as_str() {
        "ctrl+home" | "ctrl+end" => jump_user_message(m, &key, k.s == "ctrl+end"),
        "alt+enter" => open_user_message(m, &key, ""),
        "enter" => {
            let sel = m
                .session
                .as_ref()
                .map(|v| v.sel.clone())
                .unwrap_or_default();
            if sel.contains(":u:") {
                open_user_message(m, &key, &sel)
            } else {
                Out::Pass
            }
        }
        _ => Out::Pass,
    }
}

/// Picks your first message, or latest, its turn open and at the top
/// (jumpUserMessage).
pub fn jump_user_message(m: &mut Model, key: &str, last: bool) -> Out {
    let Some(v) = m.session.as_mut().filter(|v| v.key == key) else {
        return Out::Pass;
    };
    let msgs = user_messages(&v.sess);
    let Some(msg) = (if last { msgs.last() } else { msgs.first() }) else {
        return Out::Idle;
    };
    let turn = msg.r#ref.split(':').next().unwrap_or("").to_string();
    if let Some(n) = turn.strip_prefix('t').and_then(|n| n.parse().ok()) {
        v.pos = crate::session::Pos::At { n, off: 0 };
    }
    v.o.open.insert(turn, true);
    v.sel.clone_from(&msg.r#ref);
    Out::Draw
}

/// A link in a message: one of a kind rush opens itself (a pull request,
/// to merge or review) opens there; rush:message/<ref>[/image/<n>] opens
/// that message; rush:agent/<tag> goes to that agent. Anything else is
/// Pass. (messageLink, pane_pointer.go:154 linkedAgent.)
pub fn message_link(m: &mut Model, key: &str, target: &str) -> Out {
    if let Some(out) = crate::screens::cmdbar::linkkinds::open_known_link(m, target) {
        return out;
    }
    if let Some(tag) = target.strip_prefix("rush:agent/") {
        use crate::screens::social::broadcast::{mentionable, tags};
        let to = m
            .agents
            .iter()
            .find(|a| mentionable(a) && tags(a, tag))
            .map(|a| a.key.clone());
        match to {
            Some(k) => crate::screens::cmdbar::bar::go_agent(m, &k),
            None => m.flash_err("that agent isn't in the list any more"),
        }
        return Out::Draw;
    }
    let Some(rest) = target.strip_prefix("rush:message/") else {
        return Out::Pass;
    };
    let (r, image) = match rest.split_once("/image/") {
        Some((r, n)) => (r, n.parse::<usize>().ok()),
        None => (rest, None),
    };
    open_user_message(m, key, r);
    if let (Some(n), Some(s)) = (image, m.sheet_as::<MessageSheet>())
        && n > 0
        && n <= s.images().len()
    {
        s.tab = n;
    }
    Out::Draw
}

/// A click on an image in a draft opens it, without changing the draft
/// (draftImage): marker is the chip clicked, refs the box's images.
pub fn draft_image(m: &mut Model, marker: &str, refs: &ImageRefs) -> Out {
    if !super::imagerefs::is_marker(marker) {
        return Out::Pass;
    }
    let (_, paths) = refs.resolve(marker);
    if paths.is_empty() {
        return Out::Pass;
    }
    let mut s = MessageSheet::new(
        vec![UserMessage {
            images: paths,
            ..UserMessage::default()
        }],
        0,
    );
    s.tab = 1;
    m.open_sheet(Box::new(s));
    Out::Draw
}
