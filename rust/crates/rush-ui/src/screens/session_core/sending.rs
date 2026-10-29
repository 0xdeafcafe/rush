//! A message you've sent shows as sending, under the conversation, until
//! the agent has it: a turn or an interjection of yours, or a message in
//! its queue. Until then a slow host or a transcript not yet written would
//! leave it looking lost. After Go's internal/ui/sending.go (yours :30,
//! markSending :47, settleSending :58, landing :70, arrived :84,
//! sendingLines :124, sendFailed :162) and editor.go:429 (shortImages).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use rush_convo::Session;
use rush_convo::model::Item;
use rush_core::text::Line;
use rush_core::time::Ms;

use crate::style::*;

/// A message sent and not yet seen to arrive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sending {
    pub text: String,
    /// How many of yours the agent had, and those sending before it.
    pub base: usize,
    pub at: Ms,
}

/// How long a message shows as sending before rush stops waiting to see
/// it arrive.
pub const SENDING_FOR: Ms = 90_000;

/// Where a message arrived: its turn's start, and the interjection's place
/// in it (-1 for the turn's prompt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Landing {
    pub start: Ms,
    pub item: isize,
}

impl Landing {
    fn after(self, o: Landing) -> bool {
        self.start > o.start || self.start == o.start && self.item > o.item
    }
}

/// What the agent has of yours: your turns and interjections, and queued,
/// what waits in its queue.
pub fn yours(s: &Session, queued: usize) -> usize {
    let mut n = queued;
    for t in &s.turns {
        if t.from.is_empty() {
            n += 1;
        }
        n += t
            .items
            .iter()
            .filter(|it| matches!(it, Item::Interject(_)))
            .count();
    }
    n
}

/// The messages on their way to one session, and where each text was last
/// seen to land.
#[derive(Clone, Debug, Default)]
pub struct Outbox {
    pub sending: Vec<Sending>,
    pub landed: HashMap<String, Landing>,
}

impl Outbox {
    /// Shows text as sending until the agent has it.
    pub fn mark(&mut self, s: &Session, queued: usize, text: &str, now: Ms) {
        let base = yours(s, queued) + self.sending.len();
        self.sending.push(Sending {
            text: text.to_string(),
            base,
            at: now,
        });
    }

    /// Drops the messages the agent now has, oldest first, and those waited
    /// on too long.
    pub fn settle(&mut self, s: &Session, queued: usize, now: Ms) {
        if self.sending.is_empty() {
            return;
        }
        let n = yours(s, queued);
        while let Some(first) = self.sending.first().cloned() {
            if self.arrived(s, &first) || n > first.base || now - first.at > SENDING_FOR {
                self.sending.remove(0);
            } else {
                break;
            }
        }
    }

    /// The last message sending didn't go: it stops showing (sendFailed).
    pub fn failed(&mut self) {
        self.sending.pop();
    }

    /// Whether m is in the conversation by its text, since it was sent: the
    /// count alone misses it when the conversation is swapped for a
    /// shorter one. Each arrival settles one message, so the same text sent
    /// twice waits for two.
    fn arrived(&mut self, s: &Session, m: &Sending) -> bool {
        let want = m.text.trim();
        let since = m.at - 2000;
        let prev = self.landed.get(want).copied();
        let mut found: Option<Landing> = None;
        for t in s.turns.iter().rev() {
            if t.start < since {
                break;
            }
            let mut here = Vec::new();
            if t.from.is_empty() && t.prompt.trim() == want {
                here.push(Landing {
                    start: t.start,
                    item: -1,
                });
            }
            for (j, it) in t.items.iter().enumerate() {
                if let Item::Interject(x) = it
                    && x.text.trim() == want
                {
                    here.push(Landing {
                        start: t.start,
                        item: j as isize,
                    });
                }
            }
            // The earliest arrival not yet claimed.
            for l in here.into_iter().rev() {
                if prev.is_none_or(|p| l.after(p)) {
                    found = Some(l);
                }
            }
        }
        let Some(l) = found else {
            return false;
        };
        self.landed.insert(want.to_string(), l);
        true
    }

    /// The rows for messages still on their way, w wide (sendingLines;
    /// settle first).
    pub fn lines(&self, tick: usize, w: usize) -> Vec<Line> {
        self.sending
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut text = one_line(&s.text);
                if text.is_empty() {
                    text = "images".into();
                }
                let text = rush_draw::truncate(&short_images(&text), w.saturating_sub(16), "…")
                    .into_owned();
                line([
                    plain("  "),
                    sp(SPINNER[(tick + i) % SPINNER.len()], ORANGE),
                    plain(" "),
                    sp("sending", SUB),
                    plain("  "),
                    sp(text, TEXT),
                ])
            })
            .collect()
    }
}

static IMAGE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[image: ([^\]]+)\]").expect("the image tag's pattern"));

/// The image paths a message carries, shown as their names.
pub fn short_images(s: &str) -> String {
    IMAGE_TAG
        .replace_all(s, |c: &regex::Captures| {
            format!("▣ {}", rush_convo::render::image_label(&c[1]))
        })
        .into_owned()
}
