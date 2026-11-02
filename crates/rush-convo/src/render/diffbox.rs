//! A change to a file as the diff box draws it (draw/diffbox.rs): its
//! hunks as lines with their old and new numbers, each highlighted in the
//! file's language, the words that changed between a removed line and the
//! added line it pairs with picked out; and how those lines lay out, in
//! one column or side by side, the long unchanged stretches folded.
//!
//! A change is worked out once: what an edit's call and output give is
//! kept by what they hold, so drawing it again (a fold opened, a wider
//! pane, the next frame) only lays it out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use rush_core::text::Color;
use rush_core::tool::{Call, Output};
use rush_hl::Lang;

use super::code::{HlState, lang_for, paint_words, show_whitespace};
use super::style::{BG_ADD_HI, BG_DEL_HI, C_SUB, C_TEXT};
use super::txt::Txt;
use super::udiff::expand_tabs;

/// Unchanged lines kept beside a change; past them a stretch folds.
pub(crate) const CONTEXT: usize = 3;

/// What a line of a change is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    Same,
    Del,
    Add,
}

/// One line of a change: its numbers in the old file and the new (0 when
/// it has none there, or they aren't known), highlighted.
pub(crate) struct Ln {
    pub tag: Tag,
    pub old: usize,
    pub new: usize,
    pub hl: Txt,
}

/// A run of lines the change shows together, and how many lines of the
/// file were left out before it (0: none, or not known).
pub(crate) struct Hunk {
    pub lines: Vec<Ln>,
    pub before: usize,
}

/// One edit's change to a file.
pub(crate) struct Part {
    pub hunks: Vec<Hunk>,
    pub add: usize,
    pub del: usize,
}

/// Where a line is: its part, hunk and line.
pub(crate) type At = (usize, usize, usize);

/// A row of the box, as laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A line, in one column.
    One(At),
    /// A row side by side: the old file's line on the left, the new's on
    /// the right; an unchanged line on both.
    Two(Option<At>, Option<At>),
    /// n unchanged lines folded away, the box's fold k.
    Fold { n: usize, k: usize, open: bool },
    /// Lines of the file not in the change (0: not known).
    Gap(usize),
}

impl Row {
    /// Whether it shows a line of the file.
    pub(crate) fn is_line(&self) -> bool {
        matches!(self, Row::One(_) | Row::Two(..))
    }
}

/// A line before it's highlighted.
struct Raw {
    tag: Tag,
    old: usize,
    new: usize,
    text: String,
}

thread_local! {
    /// Each change as worked out, by what it was worked out from.
    static PARTS: RefCell<HashMap<u64, Rc<Part>>> = RefCell::new(HashMap::new());
}

/// The most changes kept worked out; past it they're all forgotten.
const KEPT: usize = 512;

/// An edit's change (call and output), worked out once and kept: its
/// patches; a new file's content; with bare, what an edit with no patch
/// asked to replace, or the content it wrote. None when it has none.
pub(crate) fn part_of(call: &Call, out: &Output, bare: bool) -> Option<Rc<Part>> {
    let x = &call.input;
    let from = if out.created {
        1
    } else if !out.patches.is_empty() {
        2
    } else if bare && !x.edits.is_empty() {
        3
    } else if bare && !x.content.is_empty() {
        1
    } else {
        return None;
    };
    let marked = show_whitespace();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (from, marked, &x.path).hash(&mut h);
    match from {
        1 => x.content.hash(&mut h),
        2 => {
            for p in &out.patches {
                (p.old_start, p.new_start, &p.lines).hash(&mut h);
            }
        }
        _ => {
            for e in &x.edits {
                (&e.old, &e.new).hash(&mut h);
            }
        }
    }
    let key = h.finish();
    if let Some(p) = PARTS.with(|m| m.borrow().get(&key).cloned()) {
        return Some(p);
    }
    let raw = match from {
        1 => vec![(content_lines(&x.content), 0)],
        2 => patch_lines(out),
        _ => x
            .edits
            .iter()
            .map(|e| (edit_lines(&e.old, &e.new), 0))
            .collect(),
    };
    let p = Rc::new(finish(lang_for(&x.path), raw, marked));
    PARTS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= KEPT {
            m.clear();
        }
        m.insert(key, p.clone());
    });
    Some(p)
}

/// A whole file's lines, all added, numbered from 1.
fn content_lines(s: &str) -> Vec<Raw> {
    if s.is_empty() {
        return Vec::new();
    }
    s.trim_end_matches('\n')
        .split('\n')
        .enumerate()
        .map(|(i, l)| Raw {
            tag: Tag::Add,
            old: 0,
            new: i + 1,
            text: l.trim_end_matches('\r').to_string(),
        })
        .collect()
}

/// A patch's hunks, numbered from where each starts, and the lines left
/// out between them.
fn patch_lines(out: &Output) -> Vec<(Vec<Raw>, usize)> {
    let mut hunks = Vec::new();
    let mut end = 0usize;
    for (pi, p) in out.patches.iter().enumerate() {
        let (mut o, mut n) = (p.old_start.max(0) as usize, p.new_start.max(0) as usize);
        let before = if pi > 0 { o.saturating_sub(end) } else { 0 };
        let mut ls = Vec::new();
        for l in &p.lines {
            let (tag, text) = match l.chars().next() {
                Some('-') => (Tag::Del, &l[1..]),
                Some('+') => (Tag::Add, &l[1..]),
                Some('\\') => continue, // \ No newline at end of file
                Some(' ') => (Tag::Same, &l[1..]),
                _ => (Tag::Same, l.as_str()),
            };
            let (old, new) = match tag {
                Tag::Same => (o, n),
                Tag::Del => (o, 0),
                Tag::Add => (0, n),
            };
            if tag != Tag::Add {
                o += 1;
            }
            if tag != Tag::Del {
                n += 1;
            }
            ls.push(Raw {
                tag,
                old,
                new,
                text: text.to_string(),
            });
        }
        end = o;
        hunks.push((ls, before));
    }
    hunks
}

/// What an edit replaced, as a diff of its lines: no numbers, as where in
/// the file isn't known.
fn edit_lines(old: &str, new: &str) -> Vec<Raw> {
    let split = |s: &str| -> Vec<String> {
        if s.is_empty() {
            return Vec::new();
        }
        s.trim_end_matches('\n')
            .split('\n')
            .map(str::to_string)
            .collect()
    };
    let (a, b) = (split(old), split(new));
    let raw = |tag, s: &String| Raw {
        tag,
        old: 0,
        new: 0,
        text: s.clone(),
    };
    // The lines both start and end with, then the rest by their longest
    // run in common; past a size, the rest all removed and all added.
    let mut p = 0;
    while p < a.len() && p < b.len() && a[p] == b[p] {
        p += 1;
    }
    let mut q = 0;
    while q < a.len() - p && q < b.len() - p && a[a.len() - 1 - q] == b[b.len() - 1 - q] {
        q += 1;
    }
    let (x, y) = (&a[p..a.len() - q], &b[p..b.len() - q]);
    let mut out: Vec<Raw> = a[..p].iter().map(|s| raw(Tag::Same, s)).collect();
    if x.len() * y.len() > 250_000 {
        out.extend(x.iter().map(|s| raw(Tag::Del, s)));
        out.extend(y.iter().map(|s| raw(Tag::Add, s)));
    } else {
        let (n, m) = (x.len(), y.len());
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * (m + 1) + j] = if x[i] == y[j] {
                    t[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        // Removed lines before the added ones in each run of changes.
        let (mut dels, mut adds) = (Vec::new(), Vec::new());
        let flush = |out: &mut Vec<Raw>, dels: &mut Vec<Raw>, adds: &mut Vec<Raw>| {
            out.append(dels);
            out.append(adds);
        };
        while i < n || j < m {
            if i < n && j < m && x[i] == y[j] {
                flush(&mut out, &mut dels, &mut adds);
                out.push(raw(Tag::Same, &x[i]));
                i += 1;
                j += 1;
            } else if j >= m || (i < n && t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1]) {
                dels.push(raw(Tag::Del, &x[i]));
                i += 1;
            } else {
                adds.push(raw(Tag::Add, &y[j]));
                j += 1;
            }
        }
        flush(&mut out, &mut dels, &mut adds);
    }
    out.extend(a[a.len() - q..].iter().map(|s| raw(Tag::Same, s)));
    out
}

/// The hunks highlighted, the old file's lines carrying on from each
/// other and the new's from each other, the changed words of each removed
/// line and the added line it pairs with (the nth of each in a run of
/// changes) on their stronger ground.
fn finish(lg: Option<&'static Lang>, raw: Vec<(Vec<Raw>, usize)>, marked: bool) -> Part {
    let (mut add, mut del) = (0, 0);
    let mut hunks = Vec::new();
    for (ls, before) in raw {
        let text: Vec<String> = ls
            .iter()
            .map(|l| {
                if marked {
                    l.text.clone()
                } else {
                    expand_tabs(&l.text)
                }
            })
            .collect();
        let (mut old_st, mut new_st) = (HlState::default(), HlState::default());
        let mut lines: Vec<Ln> = Vec::with_capacity(ls.len());
        let mut i = 0;
        while i < ls.len() {
            if ls[i].tag == Tag::Same {
                let hl = paint_words(
                    lg,
                    &mut new_st,
                    &text[i],
                    C_SUB,
                    &[],
                    Color::Default,
                    marked,
                );
                old_st = new_st;
                lines.push(Ln {
                    tag: Tag::Same,
                    old: ls[i].old,
                    new: ls[i].new,
                    hl,
                });
                i += 1;
                continue;
            }
            let mut j = i;
            while j < ls.len() && ls[j].tag != Tag::Same {
                j += 1;
            }
            let dels: Vec<usize> = (i..j).filter(|&k| ls[k].tag == Tag::Del).collect();
            let adds: Vec<usize> = (i..j).filter(|&k| ls[k].tag == Tag::Add).collect();
            let mut words: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
            for (d, a) in dels.iter().zip(&adds) {
                if let Some((x, y)) = rush_hl::code::changed_words(&text[*d], &text[*a]) {
                    words.insert(*d, x);
                    words.insert(*a, y);
                }
            }
            for k in i..j {
                let w = words.get(&k).map_or(&[][..], Vec::as_slice);
                let (st, on) = if ls[k].tag == Tag::Del {
                    del += 1;
                    (&mut old_st, BG_DEL_HI)
                } else {
                    add += 1;
                    (&mut new_st, BG_ADD_HI)
                };
                let hl = paint_words(lg, st, &text[k], C_TEXT, w, on, marked);
                lines.push(Ln {
                    tag: ls[k].tag,
                    old: ls[k].old,
                    new: ls[k].new,
                    hl,
                });
            }
            i = j;
        }
        hunks.push(Hunk { lines, before });
    }
    Part { hunks, add, del }
}

/// The rows a change lays out in, side by side or not: a gap between
/// hunks and between parts; in each hunk, unchanged lines kept CONTEXT
/// deep beside a change, longer stretches folded (open(k) says whether
/// fold k is opened) when that leaves out two lines or more.
pub(crate) fn rows(parts: &[Rc<Part>], side: bool, open: impl Fn(usize) -> bool) -> Vec<Row> {
    let mut out = Vec::new();
    let mut k = 0;
    let line = |out: &mut Vec<Row>, at: At| {
        out.push(if side {
            Row::Two(Some(at), Some(at))
        } else {
            Row::One(at)
        });
    };
    for (pi, p) in parts.iter().enumerate() {
        for (hi, h) in p.hunks.iter().enumerate() {
            if hi > 0 || (pi > 0 && !out.is_empty()) {
                out.push(Row::Gap(if hi > 0 { h.before } else { 0 }));
            }
            let ls = &h.lines;
            let mut i = 0;
            while i < ls.len() {
                let mut j = i;
                if ls[i].tag == Tag::Same {
                    while j < ls.len() && ls[j].tag == Tag::Same {
                        j += 1;
                    }
                    let n = j - i;
                    let (after, before) = (i > 0, j < ls.len());
                    let head = if after { CONTEXT } else { 0 };
                    let tail = if before { CONTEXT } else { 0 };
                    if (after || before) && n >= head + tail + 2 {
                        for x in i..i + head {
                            line(&mut out, (pi, hi, x));
                        }
                        let o = open(k);
                        out.push(Row::Fold {
                            n: n - head - tail,
                            k,
                            open: o,
                        });
                        k += 1;
                        if o {
                            for x in i + head..j - tail {
                                line(&mut out, (pi, hi, x));
                            }
                        }
                        for x in j - tail..j {
                            line(&mut out, (pi, hi, x));
                        }
                    } else {
                        for x in i..j {
                            line(&mut out, (pi, hi, x));
                        }
                    }
                    i = j;
                    continue;
                }
                while j < ls.len() && ls[j].tag != Tag::Same {
                    j += 1;
                }
                if side {
                    let dels: Vec<usize> = (i..j).filter(|&x| ls[x].tag == Tag::Del).collect();
                    let adds: Vec<usize> = (i..j).filter(|&x| ls[x].tag == Tag::Add).collect();
                    for n in 0..dels.len().max(adds.len()) {
                        out.push(Row::Two(
                            dels.get(n).map(|&x| (pi, hi, x)),
                            adds.get(n).map(|&x| (pi, hi, x)),
                        ));
                    }
                } else {
                    for x in i..j {
                        out.push(Row::One((pi, hi, x)));
                    }
                }
                i = j;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::tool::{Input, Patch, Replace};

    fn call(path: &str, edits: Vec<Replace>) -> Call {
        Call {
            input: Input {
                path: path.into(),
                edits,
                ..Input::default()
            },
            ..Call::default()
        }
    }

    fn edit(lines: &[&str], old: i64) -> (Call, Output) {
        let c = call("/w/a.rs", Vec::new());
        let o = Output {
            patches: vec![Patch {
                old_start: old,
                new_start: old,
                lines: lines.iter().map(|s| s.to_string()).collect(),
                ..Patch::default()
            }],
            ..Output::default()
        };
        (c, o)
    }

    fn shape(rows: &[Row]) -> String {
        rows.iter()
            .map(|r| match r {
                Row::One(_) => "l".to_string(),
                Row::Two(a, b) => format!("{}{}", u8::from(a.is_some()), u8::from(b.is_some())),
                Row::Fold { n, open, .. } => format!("[{n}{}]", if *open { "+" } else { "" }),
                Row::Gap(n) => format!("~{n}"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn numbers_and_counts() {
        let (c, o) = edit(&[" a", "-b", "+B", "+C", " d"], 10);
        let p = part_of(&c, &o, false).unwrap();
        let ls = &p.hunks[0].lines;
        let nums: Vec<_> = ls.iter().map(|l| (l.tag, l.old, l.new)).collect();
        assert_eq!(
            nums,
            [
                (Tag::Same, 10, 10),
                (Tag::Del, 11, 0),
                (Tag::Add, 0, 11),
                (Tag::Add, 0, 12),
                (Tag::Same, 12, 13)
            ]
        );
        assert_eq!((p.add, p.del), (2, 1));
        // Kept: the same change is worked out once.
        assert!(Rc::ptr_eq(&p, &part_of(&c, &o, false).unwrap()));
    }

    #[test]
    fn long_stretches_fold_and_pairs_line_up() {
        let mut ls = vec!["-x", "+X"];
        let same: Vec<String> = (0..10).map(|i| format!(" s{i}")).collect();
        ls.extend(same.iter().map(String::as_str));
        ls.extend(["-y", "-z", "+Y"]);
        let (c, o) = edit(&ls, 1);
        let p = part_of(&c, &o, false).unwrap();
        let one = rows(std::slice::from_ref(&p), false, |_| false);
        assert_eq!(shape(&one), "l l l l l [4] l l l l l l");
        let two = rows(std::slice::from_ref(&p), true, |_| false);
        assert_eq!(shape(&two), "11 11 11 11 [4] 11 11 11 11 10");
        let opened = rows(std::slice::from_ref(&p), true, |k| k == 0);
        assert_eq!(opened.iter().filter(|r| r.is_line()).count(), 13);
    }

    #[test]
    fn an_edit_with_no_patch_is_diffed_by_its_lines() {
        let c = call(
            "a.go",
            vec![Replace {
                old: "a\nb\nc\n".into(),
                new: "a\nB\nc\nd\n".into(),
                ..Replace::default()
            }],
        );
        let p = part_of(&c, &Output::default(), true).unwrap();
        let tags: Vec<_> = p.hunks[0].lines.iter().map(|l| l.tag).collect();
        use Tag::*;
        assert_eq!(tags, [Same, Del, Add, Same, Add]);
        assert!(part_of(&c, &Output::default(), false).is_none());
    }
}
