//! The images a Session's box holds, each standing in the text as
//! [Image #N] where it was pasted or dropped, as Claude Code shows them.
//! The marker is the image: deleting it takes the image off the message.
//! After Go's internal/ui/imagerefs.go (imageRefs :17, add :25, inline
//! :36, resolve :58, paths :83, shown :148, chipName :179) and
//! clipimage.go (withMarks :127).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::frame::editor::{Look, Seg, chip_spans, image_path, insert, path_spans};

static MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[Image #(\d+)\]").unwrap());

/// Whether s is an image's marker and nothing else.
pub fn is_marker(s: &str) -> bool {
    MARKER
        .find(s)
        .is_some_and(|m| m.start() == 0 && m.end() == s.len())
}

/// The marker for image n.
pub fn image_marker(n: usize) -> String {
    format!("[Image #{n}]")
}

/// A box's images, by their markers' numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageRefs {
    /// The last number given.
    pub n: usize,
    /// Each number's file.
    pub path: HashMap<usize, String>,
}

impl ImageRefs {
    /// Keeps an image file and returns the marker that stands for it.
    pub fn add(&mut self, path: &str) -> String {
        self.n += 1;
        self.path.insert(self.n, path.to_string());
        image_marker(self.n)
    }

    /// text with a marker in place of each image file named in it: dropped
    /// or typed paths become the images they name, where they were.
    pub fn inline(&mut self, text: &str, look: Look) -> Option<String> {
        let mut b = String::new();
        let mut last = 0;
        let mut found = false;
        for sp in path_spans(text) {
            let Some(p) = image_path(&sp.tok, look) else {
                continue;
            };
            b.push_str(&text[last..sp.from]);
            b.push_str(&self.add(&p));
            last = sp.to;
            found = true;
        }
        if !found {
            return None;
        }
        b.push_str(&text[last..]);
        Some(b)
    }

    /// A message about to be sent: its markers numbered 1, 2, 3 in the
    /// order they appear, and the images they stand for in that order. A
    /// marker whose image isn't this box's stays as it is.
    pub fn resolve(&self, text: &str) -> (String, Vec<String>) {
        if self.path.is_empty() {
            return (text.to_string(), Vec::new());
        }
        let mut images: Vec<String> = Vec::new();
        let mut seen: HashMap<usize, usize> = HashMap::new();
        let out = MARKER.replace_all(text, |c: &regex::Captures| {
            let id: usize = c[1].parse().unwrap_or(0);
            let Some(p) = self.path.get(&id) else {
                return c[0].to_string();
            };
            let n = *seen.entry(id).or_insert_with(|| {
                images.push(p.clone());
                images.len()
            });
            image_marker(n)
        });
        (out.into_owned(), images)
    }

    /// text with each marker whose image is this box's as the file's path,
    /// [image: /a.png], which Claude Code opens itself.
    pub fn paths(&self, text: &str) -> String {
        if self.path.is_empty() {
            return text.to_string();
        }
        MARKER
            .replace_all(text, |c: &regex::Captures| {
                let id: usize = c[1].parse().unwrap_or(0);
                match self.path.get(&id) {
                    Some(p) => format!("[image: {p}]"),
                    None => c[0].to_string(),
                }
            })
            .into_owned()
    }

    /// A box's text as drawn: each image's [Image #N] as its name,
    /// [▣ shot.png]. at maps each drawn position back to the text's; a
    /// place inside a name is inside its marker, so a click there picks it.
    pub fn shown(&self, buf: &[char]) -> (Vec<char>, Vec<usize>) {
        let s: String = buf.iter().collect();
        if self.path.is_empty() || !s.contains("[Image #") {
            return (buf.to_vec(), Vec::new());
        }
        let (mut out, mut at) = (Vec::new(), Vec::new());
        let mut p = 0;
        for c in MARKER.captures_iter(&s) {
            let id: usize = c[1].parse().unwrap_or(0);
            let Some(path) = self.path.get(&id) else {
                continue;
            };
            let all = c.get(0).unwrap();
            let from = s[..all.start()].chars().count();
            while p < from {
                out.push(buf[p]);
                at.push(p);
                p += 1;
            }
            for (i, ch) in format!("[▣ {}]", chip_name(path)).chars().enumerate() {
                out.push(ch);
                at.push(from + i.min(1));
            }
            p = from + all.as_str().chars().count();
        }
        while p < buf.len() {
            out.push(buf[p]);
            at.push(p);
            p += 1;
        }
        at.push(buf.len());
        (out, at)
    }
}

/// An image's name in a box: its file's, cut short when long, with nothing
/// in it that would end the chip early.
pub fn chip_name(path: &str) -> String {
    let n: Vec<char> = rush_convo::render::image_label(path)
        .replace(']', ")")
        .replace('\n', " ")
        .chars()
        .collect();
    if n.len() > 32 {
        let mut out: String = n[..20].iter().collect();
        out.push('…');
        out.extend(&n[n.len() - 11..]);
        return out;
    }
    n.into_iter().collect()
}

/// The chip with the character at p in it (chipOn).
pub fn chip_on(buf: &[char], p: usize) -> Option<Seg> {
    chip_spans(buf)
        .into_iter()
        .find(|c| p >= c.from && p < c.to)
}

/// buf with a marker for each image put in at pos, spaced off the words
/// either side (withMarks).
pub fn with_marks(buf: &[char], pos: usize, r: &mut ImageRefs, imgs: &[String]) -> Vec<char> {
    let marks: Vec<String> = imgs.iter().map(|p| r.add(p)).collect();
    let mut ins = marks.join(" ");
    if pos > 0 && buf[pos - 1] != ' ' && buf[pos - 1] != '\n' {
        ins = format!(" {ins}");
    }
    if pos < buf.len() && buf[pos] != ' ' {
        ins.push(' ');
    }
    insert(buf, pos, &ins.chars().collect::<Vec<_>>())
}
