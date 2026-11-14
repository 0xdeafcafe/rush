//! rush's memory notes for an agent that isn't Claude Code: the project's
//! notes are kept where Claude Code's sessions read them, so another agent
//! on the same project is told what they hold and how to add one, and the
//! two learn from each other. Rust-only: Go's other agents went without.

use std::path::Path;

use rush_core::adapter::{LEGACY_KIND, Registry};

/// How much of MEMORY.md is told, as Claude Code reads it: its first
/// lines, and no more than this many bytes.
const MAX_LINES: usize = 200;
const MAX_BYTES: usize = 25_000;

/// What a session in cwd on an agent that isn't Claude Code is told of
/// the project's memory notes: where they are, their index, and how to
/// add one. "" without Claude Code, whose folder they're kept in.
pub fn memory_prompt(reg: &Registry, cwd: &str) -> String {
    let Some(a) = reg.get(LEGACY_KIND) else {
        return String::new();
    };
    let Some(p) = reg.profiles_of(&a).into_iter().next() else {
        return String::new();
    };
    let dir = a
        .memory_reader()
        .map(|r| r.memory_dir(&p, cwd))
        .unwrap_or_default();
    if dir.is_empty() {
        return String::new();
    }
    let index = std::fs::read_to_string(Path::new(&dir).join("MEMORY.md")).unwrap_or_default();
    prompt(&dir, &index)
}

/// The prompt for the memory folder dir whose MEMORY.md reads index.
pub(crate) fn prompt(dir: &str, index: &str) -> String {
    let mut p = format!(
        "# Memory\n\nrush keeps memory notes for this project in {dir}, shared with every agent that works on it, Claude Code's sessions too. Each note is one file holding one fact; MEMORY.md is their index. Read a note's file when its line bears on your work; notes reflect what was true when written, so check one against the code before you rely on it.\n\nWhen you learn something a later session should know (who the user is and how they like to work, a correction or approach they confirmed and why, a project goal or constraint the code and git history don't record, where to find something outside the repository), save it with `rush memory add \"<the fact, and why>\"` (optionally with --type user, feedback, project or reference), rather than writing files there yourself. Don't save what the repository already records or what only matters to this conversation."
    );
    let index = clip(index.trim());
    if index.is_empty() {
        p.push_str("\n\nThere are no notes yet.");
    } else {
        p.push_str("\n\nMEMORY.md:\n\n");
        p.push_str(index);
    }
    p
}

/// index's first MAX_LINES lines, within MAX_BYTES, cut at a line.
fn clip(index: &str) -> &str {
    let mut end = 0;
    for (n, line) in index.split_inclusive('\n').enumerate() {
        if n == MAX_LINES || end + line.len() > MAX_BYTES {
            break;
        }
        end += line.len();
    }
    index[..end].trim_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_has_the_index_or_says_none() {
        for (index, has, hasnt) in [
            (
                "- [Merge](merge.md) — squash\n",
                "MEMORY.md:\n\n- [Merge](merge.md) — squash",
                "no notes yet",
            ),
            ("", "There are no notes yet.", "MEMORY.md:\n"),
            ("  \n", "There are no notes yet.", "MEMORY.md:\n"),
        ] {
            let p = prompt("/m", index);
            assert!(p.starts_with("# Memory\n\nrush keeps memory notes for this project in /m,"));
            assert!(p.contains("`rush memory add"), "{p}");
            assert!(p.contains(has) && !p.contains(hasnt), "{index:?}: {p}");
        }
    }

    #[test]
    fn index_clipped_at_a_line() {
        let long: String = (0..300).map(|i| format!("- note {i}\n")).collect();
        let c = clip(&long);
        assert_eq!(c.lines().count(), MAX_LINES);
        assert!(c.ends_with("- note 199"));
        let wide = format!("{}\n{}\n", "a".repeat(MAX_BYTES - 10), "b".repeat(20));
        assert_eq!(clip(&wide), "a".repeat(MAX_BYTES - 10));
    }
}
