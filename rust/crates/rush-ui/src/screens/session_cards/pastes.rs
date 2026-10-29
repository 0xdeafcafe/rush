//! The long pastes a message box shows as chips; the text goes out in
//! full when the message is sent. After Go's internal/ui/pastes.go
//! (pastes :17, isLongPaste :26, add :33, expand :46, unfold :66, lastIn
//! :71, chipAt :83, open :103, applyEdit :262) and convo/pastechip.go
//! (PasteChipRe :50, HasPasteChip :53).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::frame::editor::Seg;

/// A paste chip, its number the first group. Older chips kept in drafts
/// ([#N L lines: …] and [Pasted text #N +L lines]) still match.
pub static PASTE_CHIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:[Pp]asted text )?#(\d+) (?:· )?(?:\+\d+ lines|\d+ lines?(?:: [^\]\n]*)?)\]")
        .unwrap()
});

/// A quick look for whether s may hold a chip.
pub fn has_paste_chip(s: &str) -> bool {
    s.contains("[#") || s.contains("asted text #")
}

/// A paste worth folding into a chip: any with more than one line, or a
/// very long one.
pub fn is_long_paste(s: &str) -> bool {
    s.trim_end_matches('\n').contains('\n') || s.len() > 800
}

/// The chip that stands for paste id.
pub fn chip_for(id: usize, text: &str) -> String {
    rush_convo::render::paste_chip(id, text)
}

/// A box's pastes, by their chips' numbers.
#[derive(Clone, Debug, Default)]
pub struct Pastes {
    pub n: usize,
    pub text: HashMap<usize, String>,
}

impl Pastes {
    /// Keeps text and returns the chip that stands for it.
    pub fn add(&mut self, text: &str) -> String {
        self.n += 1;
        self.text.insert(self.n, text.to_string());
        chip_for(self.n, text)
    }

    /// s with each chip's pasted text back in its place. Tagged, each goes
    /// between <pasted_content> tags as Claude Code sends a paste, with tag
    /// as its id (4 hex digits, random in Go; the caller's here).
    pub fn expand(&self, s: &str, tagged: Option<u16>) -> String {
        if self.text.is_empty() {
            return s.to_string();
        }
        PASTE_CHIP
            .replace_all(s, |c: &regex::Captures| {
                let id: usize = c[1].parse().unwrap_or(0);
                let Some(t) = self.text.get(&id) else {
                    return c[0].to_string();
                };
                match tagged {
                    Some(n) => {
                        let tag = format!("id=\"{n:04x}\"");
                        format!(
                            "\n\n<pasted_content {tag}>\n{}\n</pasted_content {tag}>\n",
                            t.trim_end_matches('\n')
                        )
                    }
                    None => t.clone(),
                }
            })
            .into_owned()
    }

    /// A sent message back in a box: each tagged paste a chip again.
    pub fn unfold(&mut self, s: &str) -> Vec<char> {
        rush_convo::render::each_paste(s, |t| self.add(t))
            .trim()
            .chars()
            .collect()
    }

    /// The id of the last chip in the draft kept here, 0 if there's none.
    pub fn last_in(&self, buf: &[char]) -> usize {
        let s: String = buf.iter().collect();
        PASTE_CHIP
            .captures_iter(&s)
            .filter_map(|c| c[1].parse::<usize>().ok())
            .filter(|id| self.text.get(id).is_some_and(|t| !t.is_empty()))
            .last()
            .unwrap_or(0)
    }

    /// The span of the chip with the character at pos in it, and whether
    /// it's a paste kept here, to open.
    pub fn chip_at(&self, buf: &[char], pos: usize) -> (Seg, bool) {
        let s: String = buf.iter().collect();
        if !has_paste_chip(&s) {
            return (Seg::default(), false);
        }
        for c in PASTE_CHIP.captures_iter(&s) {
            let all = c.get(0).unwrap();
            let from = s[..all.start()].chars().count();
            let to = from + all.as_str().chars().count();
            if pos >= from && pos < to {
                let id: usize = c[1].parse().unwrap_or(0);
                return (Seg { from, to }, self.text.contains_key(&id));
            }
        }
        (Seg::default(), false)
    }

    /// buf with the text of the chip at sp back in its place, and where
    /// the cursor goes: after it.
    pub fn open(&self, buf: &[char], sp: Seg) -> Option<(Vec<char>, usize)> {
        let (got, ok) = self.chip_at(buf, sp.from);
        if !ok || got != sp {
            return None;
        }
        let chip: String = buf[sp.from..sp.to].iter().collect();
        let id: usize = PASTE_CHIP.captures(&chip)?[1].parse().ok()?;
        let t: Vec<char> = self.text.get(&id)?.chars().collect();
        let mut out = buf[..sp.from].to_vec();
        out.extend_from_slice(&t);
        out.extend_from_slice(&buf[sp.to..]);
        Some((out, sp.from + t.len()))
    }

    /// What came back from the editor put into a box: paste id's text and
    /// its chip, or (id 0) the whole draft (applyEdit).
    pub fn apply_edit(&mut self, buf: &[char], id: usize, text: &str) -> Vec<char> {
        if id == 0 {
            return text.chars().collect();
        }
        let old = self.text.insert(id, text.to_string()).unwrap_or_default();
        let s: String = buf.iter().collect();
        s.replacen(&chip_for(id, &old), &chip_for(id, text), 1)
            .chars()
            .collect()
    }

    /// ctrl+g: the text to edit, the last paste in the draft or the whole
    /// draft, and the paste's id (0 for the draft) (editDraft).
    pub fn to_edit(&self, buf: &[char]) -> (String, usize) {
        match self.last_in(buf) {
            0 => (buf.iter().collect(), 0),
            id => (self.text[&id].clone(), id),
        }
    }
}
