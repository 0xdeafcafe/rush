//! A question asked in a box over the screen: yes or cancel, or two
//! choices and more keys beside. After Go's internal/ui/model.go
//! (confirmation :61, confirmChoice :83), keys.go:1165 (confirmKey) and
//! view.go (keys :739, boxLine :753, confirmModal :760, confirmBody :763).

use rush_core::text::Line;

use super::Out;
use crate::model::Model;
use crate::style::*;

/// What a choice does once picked; the question has closed by then.
pub type Then = Box<dyn FnOnce(&mut Model) -> Out>;

/// A question asked in a box over the screen.
#[derive(Default)]
pub struct Confirm {
    pub question: String,
    pub detail: String,
    pub on_yes: Option<Then>,
    /// ! does something stronger, said by bang_text.
    pub on_bang: Option<Then>,
    pub bang_text: String,
    /// A question between two choices rather than yes or cancel: y and n
    /// each do something, labelled yes_text and no_text; esc still
    /// cancels.
    pub yes_text: String,
    pub no_text: String,
    pub on_no: Option<Then>,
    /// A key that says yes too: the arrow pushed once more past the edge
    /// that asked (edgepush.rs).
    pub again: String,
    /// Choices beside yes, each on a key of its own.
    pub more: Vec<(String, String, Then)>,
    /// The Session whose message box asks it, on its top line, instead
    /// of a box over the screen, by agent key; inlined is whether this
    /// frame drew it there (confirmation's box, inlined).
    pub box_key: String,
    pub inlined: bool,
}

impl Confirm {
    /// A yes-or-cancel question.
    pub fn new(
        question: impl Into<String>,
        on_yes: impl FnOnce(&mut Model) -> Out + 'static,
    ) -> Confirm {
        Confirm {
            question: question.into(),
            on_yes: Some(Box::new(on_yes)),
            ..Confirm::default()
        }
    }

    pub fn detail(mut self, d: impl Into<String>) -> Confirm {
        self.detail = d.into();
        self
    }

    /// The keys it waits on, and what each does (keys).
    pub fn keys(&self) -> Line {
        let yes = if self.yes_text.is_empty() {
            "yes"
        } else {
            &self.yes_text
        };
        let mut l = line([sp("y", ORANGE), sp(format!(" {yes}"), DIM)]);
        let pair = |l: &mut Line, k: &str, t: &str| {
            l.spans.push(plain("   "));
            l.spans.push(sp(k.to_string(), ORANGE));
            l.spans.push(sp(format!(" {t}"), DIM));
        };
        if self.on_bang.is_some() && !self.bang_text.is_empty() {
            pair(&mut l, "!", &self.bang_text);
        }
        for (k, t, _) in &self.more {
            pair(&mut l, k, t);
        }
        if self.on_no.is_some() {
            pair(&mut l, "n", &self.no_text);
            pair(&mut l, "esc", "cancel");
            return l;
        }
        pair(&mut l, "n", "cancel");
        l
    }

    /// The question on an input box's top line, with its keys (boxLine).
    pub fn box_line(&self) -> Line {
        let yes = if self.yes_text.is_empty() {
            "yes"
        } else {
            &self.yes_text
        };
        let mut l = line([
            sp(self.question.clone(), ORANGE),
            sp(" · ", DIM),
            sp("enter", ORANGE),
            sp(format!(" {yes}"), DIM),
        ]);
        for (k, t, _) in &self.more {
            l.spans.push(sp(" · ", DIM));
            l.spans.push(sp(k.clone(), ORANGE));
            l.spans.push(sp(format!(" {t}"), DIM));
        }
        l.spans.push(sp(" · ", DIM));
        l.spans.push(sp("esc", ORANGE));
        l.spans.push(sp(" edit", DIM));
        l
    }

    /// The box's inside, for a box bw wide (confirmBody). A long list of
    /// keys wraps rather than lose its last, n to cancel.
    pub fn body(&self, bw: usize) -> Vec<Line> {
        let mut out = vec![line([spb(self.question.clone(), TEXT)])];
        if !self.detail.is_empty() {
            for l in wrap(&self.detail, bw.saturating_sub(4)) {
                out.push(line([sp(l, DIM)]));
            }
        }
        out.push(Line::new());
        let k = self.keys();
        if k.width() > bw.saturating_sub(4) {
            out.extend(wrap_line(&k, bw.saturating_sub(4)));
        } else {
            out.push(k);
        }
        out
    }
}

impl Model {
    /// Asks c in a box over the screen; it has every key until answered.
    pub fn ask(&mut self, c: Confirm) {
        self.confirm = Some(c);
    }

    /// A key while a question is asked (confirmKey).
    pub(crate) fn confirm_key(&mut self, s: &str) -> Out {
        let Some(mut c) = self.confirm.take() else {
            return Out::Pass;
        };
        let s = if !c.again.is_empty() && s == c.again {
            "y"
        } else {
            s
        };
        if let Some(i) = c.more.iter().position(|(k, _, _)| k == s) {
            let (_, _, f) = c.more.swap_remove(i);
            return done(f(self));
        }
        let then = match s {
            "y" | "enter" => c.on_yes.take(),
            "!" => c.on_bang.take(),
            "n" => c.on_no.take(),
            "esc" | "ctrl+c" => None,
            _ => {
                self.confirm = Some(c);
                return Out::Idle;
            }
        };
        match then {
            Some(f) => done(f(self)),
            None => Out::Draw,
        }
    }

    /// The question in a box over base (confirmModal).
    pub(crate) fn confirm_view(&mut self, base: &mut [Line]) {
        let Some(c) = self.confirm.take() else {
            return;
        };
        if c.inlined {
            // Asked on its Session's box this frame (view.go:629).
            self.confirm = Some(c);
            return;
        }
        let bw = self.w.saturating_sub(4).min(72);
        let body = c.body(bw);
        self.confirm = Some(c);
        self.modal_over(base, &body, bw, YELLOW);
    }
}

// What a choice made of it, once the box has gone: at least a frame.
fn done(o: Out) -> Out {
    match o {
        Out::Pass | Out::Idle => Out::Draw,
        o => o,
    }
}
