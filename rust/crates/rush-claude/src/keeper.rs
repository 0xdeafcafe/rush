//! Claude Code's side of rush's memory notes, as rush-memory's Keeper:
//! writing a note where a session in a folder reads it, tidying MEMORY.md,
//! merging, moving one into the user's CLAUDE.md, reading one, and how
//! often sessions opened each. Port of internal/adapters/claude/memory.go
//! (CountOpens :58, ReadNote :74, Remember :100, TidyMemory :110,
//! MergeNotes :113, MakeGlobal :117).

use std::collections::HashMap;

use rush_core::adapter::{Memory, Profile};
use rush_memory::{Keeper, MemoryNote, Opens};

use crate::files::join;
use crate::memory::project_folder;
use crate::notes::{self, Note};

/// Claude Code's memory notes, for rush to write and tidy.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoteKeeper;

impl Keeper for NoteKeeper {
    fn remember(&self, p: &Profile, cwd: &str, n: &MemoryNote) -> rush_memory::Result<String> {
        let proj = project_folder(&p.dir, cwd);
        if proj.is_empty() {
            return Err("no project to remember it in".into());
        }
        let note = Note {
            name: n.name.clone(),
            title: n.title.clone(),
            description: n.description.clone(),
            kind: n.kind.clone(),
            body: n.body.clone(),
        };
        notes::write_note(&join(&[&proj, "memory"]), note).map_err(|e| e.to_string())
    }

    fn tidy_memory(&self, dir: &str) -> rush_memory::Result<Vec<String>> {
        notes::fix_memory(dir).map_err(|e| e.to_string())
    }

    fn merge_notes(&self, keep: &str, drop: &str) -> rush_memory::Result<()> {
        notes::merge_notes(keep, drop).map_err(|e| e.to_string())
    }

    fn make_global(&self, p: &Profile, path: &str) -> rush_memory::Result<()> {
        notes::make_global(&p.dir, path).map_err(|e| e.to_string())
    }

    fn read_note(&self, path: &str) -> rush_memory::Result<MemoryNote> {
        let n = notes::read_note(path).map_err(|e| e.to_string())?;
        Ok(MemoryNote {
            name: n.name,
            title: n.title,
            description: n.description,
            kind: n.kind,
            body: n.body,
        })
    }

    fn count_opens(&self, _p: &Profile, mem: &Memory) -> HashMap<String, Opens> {
        let mut mem = mem.clone();
        crate::memory::count_opens(&mut mem);
        mem.files
            .into_iter()
            .filter(|f| f.counted)
            .map(|f| {
                let o = Opens {
                    reads: f.reads as i32,
                    last_read: f.last_read,
                    counted: true,
                };
                (f.path, o)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_reads_and_tidies() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        let cwd = tmp.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let p = Profile {
            dir: cfg.to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let k = NoteKeeper;
        let n = MemoryNote {
            name: "tests-first".into(),
            title: "Tests first".into(),
            description: "write the test before the fix".into(),
            kind: "feedback".into(),
            body: "Write the failing test first.".into(),
        };
        let path = k.remember(&p, &cwd.to_string_lossy(), &n).expect("written");
        assert!(path.ends_with("/memory/tests-first.md"), "{path}");
        let back = k.read_note(&path).unwrap();
        assert_eq!(back.description, n.description);
        assert_eq!(back.kind, "feedback");
        assert!(back.body.contains("Write the failing test first."));
        assert!(
            k.tidy_memory(&crate::files::dir_of(&path))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            k.remember(&Profile::default(), "/x", &n).unwrap_err(),
            "no project to remember it in"
        );
    }
}
