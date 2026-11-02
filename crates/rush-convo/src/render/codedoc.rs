//! A file's lines drawn as the document they are: each line's number in
//! the margin, its words coloured as the file's language has them. The
//! background view draws a file a command writes this way. Port of
//! internal/convo/codedoc.go:11-36 (CodeRows, IsCode).

use rush_core::text::Line;

use super::code::{HlState, highlight, lang_for};
use super::style::{C_TEXT, faint};
use super::txt::{Txt, blanks};
use super::udiff::expand_tabs;

/// Lines of the file at path as a document: nos[i] is lines[i]'s number
/// in the file (none where nos is short), w cells at most each (CodeRows).
pub fn code_rows(path: &str, lines: &[String], nos: &[usize], w: usize) -> Vec<Line> {
    let lg = lang_for(path);
    let num_w = nos.iter().map(|n| n.to_string().len()).max().unwrap_or(0);
    let mut hs = HlState::default();
    let tail = Txt::raw("…");
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let mut t = match nos.get(i) {
                Some(&n) if n > 0 => faint(&format!("{n:>num_w$}  ")),
                _ if num_w > 0 => blanks(num_w + 2),
                _ => Txt::new(),
            };
            t.push(highlight(lg, &mut hs, &expand_tabs(l), C_TEXT, None));
            t.truncate(w, &tail).into_line()
        })
        .collect()
}

/// Whether the file at path is in a language rush colours: a document to
/// number, where a log isn't (IsCode).
pub fn is_code(path: &str) -> bool {
    lang_for(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_a_document_and_leaves_a_log() {
        assert!(is_code("/w/main.go"));
        assert!(!is_code("/w/server.log"));
        let rows = code_rows(
            "/w/main.go",
            &["package main".into(), "func main() {}".into()],
            &[9, 10],
            80,
        );
        let plain: Vec<String> = rows.iter().map(|l| l.plain()).collect();
        assert_eq!(plain, [" 9  package main", "10  func main() {}"]);
        // Too narrow, cut with an ellipsis.
        let rows = code_rows("/w/main.go", &["package main".into()], &[1], 8);
        assert_eq!(rows[0].plain(), "1  pack…");
        // No numbers: no margin.
        let rows = code_rows("/w/a.py", &["x = 1".into()], &[], 80);
        assert_eq!(rows[0].plain(), "x = 1");
    }
}
