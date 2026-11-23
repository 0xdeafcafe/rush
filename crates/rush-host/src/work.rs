//! What a session has done, for the list's WORK column and the weight by
//! its name. Rust-only, nothing of Go's: its task list's tasks done and in
//! all (the list taskwatch keeps) and the files its edits changed, its
//! subagents' too, counted as the host hears its agent: never by reading a
//! transcript again. They're kept in work.json beside info.json, so a host
//! started again (a session woken) counts on from where the last left off,
//! and said in info.json's `work` for the list. A session from before this
//! has none until it next works on its list or a file: its cell is blank.

use std::collections::{BTreeSet, HashMap};

use rush_core::agent::Work;
use rush_core::event::{Message, PartKind};
use rush_core::tool::Kind;
use serde::{Deserialize, Serialize};

use crate::paths::dir;
use crate::taskwatch::Watch;

/// The file the counts are kept in, in the session's folder.
pub const FILE: &str = "work.json";

/// The files a session's edits changed: an Edit or a Write its agent was
/// answered, not refused. What its shell commands wrote isn't counted
/// (the Changes view says those are a guess).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Files {
    paths: BTreeSet<String>,
    /// The edits not answered yet: their files, by call.
    #[serde(skip)]
    pending: HashMap<String, Vec<String>>,
}

impl Files {
    /// Takes in a message: whether it changed a file none had before.
    pub fn saw(&mut self, m: &Message) -> bool {
        let mut grew = false;
        for p in &m.parts {
            match p.kind {
                PartKind::ToolCall if m.role == "assistant" => {
                    let Some(c) = &p.call else { continue };
                    if !matches!(c.kind, Kind::Edit | Kind::Write) {
                        continue;
                    }
                    let mut paths: Vec<String> = Vec::new();
                    let each = std::iter::once(c.input.path.as_str())
                        .chain(c.input.edits.iter().map(|e| e.path.as_str()));
                    for path in each {
                        if !path.is_empty() && !paths.iter().any(|q| q == path) {
                            paths.push(path.to_string());
                        }
                    }
                    if !paths.is_empty() {
                        self.pending.insert(c.id.clone(), paths);
                    }
                }
                PartKind::ToolResult => {
                    let Some(o) = &p.output else { continue };
                    let Some(paths) = self.pending.remove(&o.call_id) else {
                        continue;
                    };
                    if o.is_error {
                        continue;
                    }
                    for path in paths {
                        grew |= self.paths.insert(path);
                    }
                }
                _ => {}
            }
        }
        grew
    }

    /// How many files.
    pub fn count(&self) -> usize {
        self.paths.len()
    }
}

/// The counts as the list shows them.
pub fn of(tasks: &Watch, files: &Files) -> Work {
    let (done, total) = tasks.counts();
    let n = |v: usize| u16::try_from(v).unwrap_or(u16::MAX);
    Work {
        done: n(done),
        total: n(total),
        files: n(files.count()),
    }
}

#[derive(Serialize)]
struct Saving<'a> {
    tasks: &'a Watch,
    files: &'a Files,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Saved {
    tasks: Watch,
    files: Files,
}

/// Session id's list and files as its last host kept them; none for a
/// session that kept none.
pub fn load(id: &str) -> (Watch, Files) {
    let Ok(b) = std::fs::read(dir(id).join(FILE)) else {
        return (Watch::default(), Files::default());
    };
    let s: Saved = serde_json::from_slice(&b).unwrap_or_default();
    (s.tasks, s.files)
}

/// Keeps session id's list and files for the next host.
pub fn save(id: &str, tasks: &Watch, files: &Files) {
    let b = crate::json::line(&Saving { tasks, files });
    let _ = crate::json::write_atomic(&dir(id).join(FILE), &b);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::event::Part;
    use rush_core::tool::{Call, Input, Output, Replace};

    fn call(id: &str, kind: Kind, path: &str, edits: &[&str]) -> Message {
        Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(Call {
                    id: id.into(),
                    kind,
                    input: Input {
                        path: path.into(),
                        edits: edits
                            .iter()
                            .map(|p| Replace {
                                path: (*p).into(),
                                ..Replace::default()
                            })
                            .collect(),
                        ..Input::default()
                    },
                    ..Call::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        }
    }

    fn result(id: &str, is_error: bool) -> Message {
        Message {
            role: "user".into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: id.into(),
                    is_error,
                    ..Output::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        }
    }

    #[test]
    fn the_files_its_edits_changed() {
        struct Case {
            name: &'static str,
            /// Each call: its kind, its path, its edits' paths, and whether
            /// its answer was an error.
            calls: &'static [(Kind, &'static str, &'static [&'static str], bool)],
            want: usize,
        }
        let cases = [
            Case {
                name: "none",
                calls: &[],
                want: 0,
            },
            Case {
                name: "an edit and a write",
                calls: &[
                    (Kind::Edit, "a.rs", &[], false),
                    (Kind::Write, "b.rs", &[], false),
                ],
                want: 2,
            },
            Case {
                name: "the same file twice",
                calls: &[
                    (Kind::Edit, "a.rs", &[], false),
                    (Kind::Edit, "a.rs", &[], false),
                ],
                want: 1,
            },
            Case {
                name: "a refused edit",
                calls: &[(Kind::Edit, "a.rs", &[], true)],
                want: 0,
            },
            Case {
                name: "a patch of three files",
                calls: &[(Kind::Edit, "a.rs", &["a.rs", "b.rs", "c.rs"], false)],
                want: 3,
            },
            Case {
                name: "a read is no change",
                calls: &[(Kind::Read, "a.rs", &[], false)],
                want: 0,
            },
        ];
        for c in cases {
            let mut f = Files::default();
            for (i, (kind, path, edits, err)) in c.calls.iter().enumerate() {
                let id = format!("c{i}");
                f.saw(&call(&id, *kind, path, edits));
                f.saw(&result(&id, *err));
            }
            assert_eq!(f.count(), c.want, "{}", c.name);
        }
    }

    #[test]
    fn a_file_new_to_it_is_said() {
        let mut f = Files::default();
        f.saw(&call("c1", Kind::Edit, "a.rs", &[]));
        assert!(f.saw(&result("c1", false)), "new");
        f.saw(&call("c2", Kind::Edit, "a.rs", &[]));
        assert!(!f.saw(&result("c2", false)), "again");
    }
}
