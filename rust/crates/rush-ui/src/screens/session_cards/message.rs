//! The message sheet: your messages as sent, whole, and what came with
//! them, one at a time. After Go's internal/ui/messagesheet.go
//! (messageSheet :19, body :148, key :189, mouse :253, openUserMessage
//! :280, jumpUserMessage :307, messageLink :336, openImage :358,
//! messageTabs :387, draftImage :401) and convo/usermessages.go
//! (UserMessage :130, UserMessages :150, ExpandedMessage :166).
//!
//! Gap: pictures aren't drawn (Go's convo.Picture and termimg, Kitty or
//! half blocks): rush-draw has no image output yet, so an image's tab
//! names it and o opens the original; the strip of thumbnails isn't
//! there.

use rush_convo::{Item, Session};
use rush_core::text::Line;

use super::imagerefs::ImageRefs;
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
}

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
                Err(e) => m.flash_err(&format!("couldn't open the image: {e}")),
            }
            Out::Draw
        },
    );
}

impl Sheet for MessageSheet {
    fn width(&self, m: &Model) -> usize {
        40.max(m.w.saturating_sub(6))
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.page = 1.max(h as isize - 5) as usize;
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
            let p = &self.images()[self.tab - 1];
            let name = std::path::Path::new(p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone());
            vec![
                line([sp("▣ ", BLUE), sp(name, TEXT)]),
                line([sp(tildify(p), DIM)]),
            ]
        };
        let page = self.page;
        self.scroll = self.scroll.min(rows.len().saturating_sub(page));
        let end = rows.len().min(self.scroll + page);
        out.extend(rows[self.scroll..end].iter().cloned());
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

/// A link in a sent message: rush:message/<ref>[/image/<n>] opens it,
/// rush:agent/… is the session's (Pass). (messageLink.)
pub fn message_link(m: &mut Model, key: &str, target: &str) -> Out {
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
