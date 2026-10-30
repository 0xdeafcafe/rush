//! A shell call rush ran as a script, drawn as the file it ran from: each
//! line numbered, a breakpoint's dot by its number, the line it runs now
//! marked with how long it's been on it, and the time each line before
//! took. Port of internal/convo/scriptdoc.go (ScriptView :14,
//! ScriptLineRef :25, ScriptLine :28, scriptsSig :39, scriptMost :59,
//! scriptDoc :67) and render.go:3214 (shellBody handing such a call here).

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use rush_core::time::Ms;

use super::code::{HlState, highlight, lang_for};
use super::draw::Drawer;
use super::style::*;
use super::txt::{Txt, blanks};
use super::udiff::expand_tabs;
use crate::model::{Status, StepId};

/// A shell call rush ran as a script, as its trace has it: the line it's
/// on and since when, how long each line done took, where it stops, and
/// whether it's stopped at one now (ScriptView).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptView {
    /// The line running now; 0 when done or not begun.
    pub at: i32,
    pub since: Ms,
    /// How long each line took, in ms, by line.
    pub took: HashMap<i32, Ms>,
    pub breaks: HashSet<i32>,
    /// Stopped at a breakpoint, at `at`.
    pub held: bool,
    /// The line a failed script ended on, 0 when it didn't fail.
    pub failed_at: i32,
}

/// The ref of line n of step ref's script: clicked, it sets a breakpoint
/// there or takes it away (ScriptLineRef).
pub fn script_line_ref(step_ref: &str, n: i32) -> String {
    format!("{step_ref}:line:{n}")
}

/// The step ref and line a script_line_ref names (ScriptLine).
pub fn script_line(r: &str) -> Option<(&str, i32)> {
    let i = r.rfind(":line:")?;
    let n: i32 = r[i + ":line:".len()..].parse().ok()?;
    (n > 0).then_some((&r[..i], n))
}

/// What of the scripts of steps ids changes how a turn draws, as a key
/// (scriptsSig, kept per turn).
pub(crate) fn scripts_sig<'a>(
    vs: &HashMap<String, ScriptView>,
    ids: impl Iterator<Item = &'a str>,
) -> u64 {
    if vs.is_empty() {
        return 0;
    }
    let mut h = DefaultHasher::new();
    for id in ids {
        let Some(v) = vs.get(id) else { continue };
        let mut breaks: Vec<_> = v.breaks.iter().collect();
        breaks.sort();
        (id, v.at, v.held, v.failed_at, v.took.len(), breaks).hash(&mut h);
    }
    h.finish()
}

/// The most lines a script shows; a longer one keeps its start, the lines
/// round where it is, its breakpoints and its end.
const SCRIPT_MOST: usize = 30;

impl Drawer<'_> {
    /// Draws call id, run as script v from cmd (scriptDoc).
    pub(crate) fn script_doc(&mut self, id: StepId, v: &ScriptView, cmd: &str, indent: usize) {
        let pad = self.sp(indent - 1);
        let text = expand_tabs(cmd);
        let lines: Vec<&str> = text.split('\n').collect();
        let num_w = lines.len().to_string().len();
        let st = self.s.step(id);
        let r = format!("{}:s:{}", self.r, st.id);
        let live = st.status == Status::Running;
        let verbose = self.o.verbose;
        let count = lines.len() as i32;
        let near = |n: i32, at: i32| at > 0 && n >= at - 3 && n <= at + 3;
        let shown = |n: i32| {
            lines.len() <= SCRIPT_MOST
                || verbose
                || n <= 6
                || n > count - 2
                || v.breaks.contains(&n)
                || near(n, v.at)
                || near(n, v.failed_at)
        };
        let mut head = faint("ran as a script");
        if v.failed_at > 0 {
            head = faint("ran as a script · ")
                .and(paint(C_RED, &format!("failed at line {}", v.failed_at)));
        } else if live && !v.held {
            head = faint("running as a script · click a line's number ahead to stop there");
        }
        if v.held {
            head = paint(C_YELLOW.bold(), &format!("⏸ stopped at line {}", v.at))
                .and(faint(" · p goes on"));
        }
        self.add(&r, BG_WELL, pad.clone().and(head), Txt::new());
        let sh = lang_for("sh");
        let mut hs = HlState::default();
        for (i, l) in lines.iter().enumerate() {
            let n = i as i32 + 1;
            if !shown(n) {
                if shown(n - 1) {
                    let hid = (n..=count).take_while(|&j| !shown(j)).count();
                    let row = pad
                        .clone()
                        .and(blanks(num_w + 4))
                        .and(faint(&format!("… {}", plural(hid, "line"))));
                    self.add("", BG_WELL, row, Txt::new());
                }
                highlight(sh, &mut hs, l, C_SUB, None); // keeps a quote opened above
                continue;
            }
            let dot = if v.breaks.contains(&n) {
                paint(C_RED, "●")
            } else {
                Txt::raw(" ")
            };
            let numbered = format!("{n:>num_w$}");
            let mut num = faint(&numbered);
            let (mut mark, mut right) = (Txt::raw("  "), Txt::new());
            let took = v.took.get(&n).copied().unwrap_or(0);
            if n == v.at && v.held {
                num = paint(C_YELLOW.bold(), &numbered);
                mark = paint(C_YELLOW, "▸ ");
                right = paint(C_YELLOW, &format!("⏸ {}", self.since(v.since)));
            } else if n == v.at && live {
                num = paint(C_ORANGE.bold(), &numbered);
                mark = paint(C_ORANGE, "▸ ");
                let ago = self.since(v.since);
                right = paint(C_ORANGE, &format!("{} {ago}", spin(self.o.tick)));
            } else if n == v.failed_at {
                num = paint(C_RED.bold(), &numbered);
                mark = paint(C_RED, "✗ ");
                if took >= 100 {
                    right = faint(&dur(took));
                }
            } else if took >= 100 {
                right = faint(&dur(took));
            }
            let mut code = highlight(sh, &mut hs, l, C_SUB, None);
            if n == v.at && (live || v.held) || n == v.failed_at {
                code = paint(C_TEXT.bold(), l);
            }
            let left = pad
                .clone()
                .and(dot)
                .and_raw(" ")
                .and(num)
                .and_raw(" ")
                .and(mark)
                .and(code);
            self.add(&script_line_ref(&r, n), BG_WELL, left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_refs() {
        let r = script_line_ref("t3:s:toolu_1", 12);
        assert_eq!(r, "t3:s:toolu_1:line:12");
        assert_eq!(script_line(&r), Some(("t3:s:toolu_1", 12)));
        for bad in ["t3:s:x", "t3:s:x:line:0", "t3:s:x:line:-2", "t3:s:x:line:a"] {
            assert_eq!(script_line(bad), None, "{bad}");
        }
    }
}
