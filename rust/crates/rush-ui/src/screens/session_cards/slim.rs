//! #slim: what a session carries in every request and has never used (MCP
//! servers, subagents, skills), to drop for that session alone, and its
//! conversation to compact. Dropped, a thing is out of the agent's context
//! from its next start; r puts it all back. After Go's
//! internal/ui/slimsheet.go (slimSheet :17, openSlim :26, drops :48,
//! body :66, key :100) and convo/slim.go (Unused :13).

use std::collections::HashSet;

use rush_core::text::Line;

use super::{Act, act, info};
use crate::frame::sheet::{sheet_row, sheet_title};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// One thing a session carries and has never used (convo.Unused).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unused {
    /// "MCP server", "subagent", "skill".
    pub what: String,
    pub name: String,
    /// What it costs every request.
    pub tokens: i64,
    /// The rules that drop it.
    pub rules: Vec<String>,
}

/// The #slim sheet on one session (slimSheet).
pub struct SlimSheet {
    pub sess: String,
    pub items: Vec<Unused>,
    /// Unticked: it stays.
    pub keep: HashSet<usize>,
    pub cur: usize,
    /// What the session goes without already.
    pub was: Vec<String>,
}

/// Opens #slim on agent key's session (openSlim).
pub fn open_slim(m: &mut Model, key: &str) {
    let Some(i) = info(m, key) else {
        m.flash_err("#slim works on a session rush runs: open one first");
        return;
    };
    let Some(items) = i.unused else {
        m.flash("no breakdown of its context yet: it comes when a turn ends");
        return;
    };
    m.open_sheet(Box::new(SlimSheet {
        sess: key.to_string(),
        items,
        keep: HashSet::new(),
        cur: 0,
        was: i.without,
    }));
}

impl SlimSheet {
    /// The rules the ticked items go by, after what's gone already, and
    /// the tokens they save a request (drops).
    pub fn drops(&self) -> (Vec<String>, i64) {
        let mut rules = self.was.clone();
        let mut tokens = 0;
        for (i, it) in self.items.iter().enumerate() {
            if self.keep.contains(&i) {
                continue;
            }
            tokens += it.tokens;
            for r in &it.rules {
                if !rules.contains(r) {
                    rules.push(r.clone());
                }
            }
        }
        (rules, tokens)
    }
}

impl Sheet for SlimSheet {
    fn width(&self, _m: &Model) -> usize {
        84
    }

    fn body(&mut self, m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title(
                "Slim this session",
                "what it carries every request and has never used",
                w,
            ),
            Line::new(),
        ];
        if self.items.is_empty() {
            out.push(line([sp(
                "  every MCP server, subagent and skill it's told of, it has used",
                DIM,
            )]));
        }
        for (i, it) in self.items.iter().enumerate() {
            let bx = if self.keep.contains(&i) {
                sp("[ ]", FAINT)
            } else {
                sp("[✕]", GREEN)
            };
            let l = line([
                bx,
                plain(" "),
                sp(fit_str(&it.what, 11), DIM),
                sp(fit_str(&it.name, w.saturating_sub(32).max(10)), TEXT),
                sp(format!("{:>7}", tokens(it.tokens)), DIM),
            ]);
            out.push(sheet_row(l, i == self.cur, w));
        }
        let (_, saved) = self.drops();
        out.push(Line::new());
        if saved > 0 {
            out.push(line([
                plain("  "),
                sp(format!("≈ {} tokens", tokens(saved)), GREEN),
                sp(" less in every request, for this session only", DIM),
            ]));
        }
        if let Some((all, results)) = info(m, &self.sess).and_then(|i| i.conv_tokens)
            && all > 0
        {
            out.push(line([
                plain("  "),
                sp(
                    format!(
                        "the conversation is {}, {} of it what tools sent back · c compacts it",
                        tokens(all),
                        tokens(results)
                    ),
                    DIM,
                ),
            ]));
        }
        if !self.was.is_empty() {
            out.push(line([
                plain("  "),
                sp(
                    format!(
                        "it goes without {} already · r puts everything back",
                        self.was.len()
                    ),
                    DIM,
                ),
            ]));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[
                ("space", "drop or keep"),
                ("a", "all or none"),
                ("enter", "drop them"),
                ("c", "compact"),
                ("esc", "cancel"),
            ],
        ));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let hosted = info(m, &self.sess).is_some();
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "up" | "k" | "shift+tab" => self.cur = self.cur.saturating_sub(1),
            "down" | "j" | "tab" => {
                self.cur = (self.cur + 1).min(self.items.len().saturating_sub(1))
            }
            "space" | " " => {
                if !self.keep.remove(&self.cur) {
                    self.keep.insert(self.cur);
                }
            }
            "a" => {
                // Any dropped: keep them all; else drop them all.
                let dropping = (0..self.items.len()).any(|i| !self.keep.contains(&i));
                self.keep.clear();
                if dropping {
                    self.keep.extend(0..self.items.len());
                }
            }
            "c" => {
                if hosted {
                    m.flash("compacting the conversation…");
                    act(m, &self.sess, Act::Send("/compact".into()));
                }
                return Out::Close;
            }
            "r" => {
                if !hosted || self.was.is_empty() {
                    return Out::Idle;
                }
                m.flash("everything it went without is back from its next start");
                act(m, &self.sess, Act::Without(None));
                return Out::Close;
            }
            "enter" => {
                let (rules, saved) = self.drops();
                if !hosted || rules == self.was {
                    return Out::Close;
                }
                m.flash(&format!(
                    "dropped ≈ {} tokens a request · from its next start, now if it's idle · #slim r puts them back",
                    tokens(saved)
                ));
                act(m, &self.sess, Act::Without(Some(rules)));
                return Out::Close;
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}
