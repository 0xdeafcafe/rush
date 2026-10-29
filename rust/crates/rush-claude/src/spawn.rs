//! Finding a Claude Code session another agent's shell began. Port of
//! internal/adapters/claude/spawn.go.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use rush_core::adapter::{Fits, Kind, Profile, Session, SpawnFinder};
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::account::path_str;
use crate::adapter::{Claude, KIND, account};
use crate::jobs::project_slug;
use crate::json::{Str, ms, nd};

fn mtime(p: &Path) -> Option<Ms> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(
        m.duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as Ms),
    )
}

impl SpawnFinder for Claude {
    /// A Claude Code transcript begun after start, in one of profiles: in
    /// the folder of the project dir first, then in any project folder
    /// written to since, taking the first fits takes.
    fn find_spawn(
        &self,
        profiles: &[Profile],
        dir: &str,
        start: Ms,
        fits: Fits,
    ) -> Option<Session> {
        let mut seen = HashSet::<PathBuf>::new();
        for p in profiles {
            let root = account(p).projects_dir();
            if !seen.insert(root.clone()) {
                continue;
            }
            // Claude Code names the folder for where it really is: /tmp is
            // /private/tmp.
            let mut dirs = vec![root.join(project_slug(dir))];
            if let Ok(real) = std::fs::canonicalize(dir)
                && real != Path::new(dir)
            {
                dirs.push(root.join(project_slug(&path_str(&real))));
            }
            if let Ok(rd) = std::fs::read_dir(&root) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_dir() && mtime(&path).is_some_and(|m| m >= start - 3000) {
                        dirs.push(path);
                    }
                }
            }
            for d in dirs {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                files.sort();
                for path in files {
                    if path.is_dir() || path.extension().is_none_or(|x| x != "jsonl") {
                        continue;
                    }
                    if mtime(&path).is_none_or(|m| m < start) || !seen.insert(path.clone()) {
                        continue;
                    }
                    let (text, at) = first_prompt(&path);
                    let s = Session {
                        kind: Kind::from(KIND),
                        profile: p.clone(),
                        id: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        name: text,
                        transcript: path_str(&path),
                        created_at: at,
                        ..Session::default()
                    };
                    if at != 0 && fits(&s) {
                        return Some(s);
                    }
                }
            }
        }
        None
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(rename = "isSidechain", deserialize_with = "nd")]
    is_sidechain: bool,
    #[serde(borrow)]
    timestamp: Str<'a>,
    #[serde(borrow)]
    message: Option<Msg<'a>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Msg<'a> {
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Block {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    text: String,
}

/// The first thing a Claude Code transcript was asked, and when, read from
/// its head alone.
fn first_prompt(path: &Path) -> (String, Ms) {
    let Ok(f) = std::fs::File::open(path) else {
        return (String::new(), 0);
    };
    let mut b = Vec::with_capacity(64 << 10);
    let _ = f.take(64 << 10).read_to_end(&mut b);
    for l in b.split(|&c| c == b'\n') {
        if memchr::memmem::find(l, br#""type":"user""#).is_none() {
            continue;
        }
        let Ok(line) = serde_json::from_slice::<Line>(l) else {
            continue;
        };
        if &*line.typ != "user" || line.is_sidechain {
            continue;
        }
        let content = line.message.and_then(|m| m.content);
        let text = match content.map(RawValue::get) {
            Some(c) if c.starts_with('"') => serde_json::from_str(c).unwrap_or_default(),
            Some(c) => serde_json::from_str::<Vec<Block>>(c)
                .unwrap_or_default()
                .into_iter()
                .filter(|b| b.typ == "text")
                .map(|b| b.text)
                .collect(),
            None => String::new(),
        };
        return (text, ms(&line.timestamp));
    }
    (String::new(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Account;
    use crate::adapter::profile;

    #[test]
    fn finds_the_session_a_shell_began() {
        let d = tempfile::tempdir().unwrap();
        let acct = Account::at("x", d.path().join("cfg"));
        let work = d.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let work = path_str(&std::fs::canonicalize(&work).unwrap());
        let folder = acct.projects_dir().join(project_slug(&work));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("aaa.jsonl"),
            r#"{"type":"user","isSidechain":true,"timestamp":"2026-10-28T10:00:00Z","message":{"content":"side"}}
{"type":"user","timestamp":"2026-10-28T10:00:01Z","message":{"content":[{"type":"text","text":"fix "},{"type":"image"},{"type":"text","text":"the tests"}]}}
"#,
        )
        .unwrap();
        std::fs::write(folder.join("bbb.jsonl"), "{\"type\":\"summary\"}\n").unwrap();
        let p = profile(&acct);
        let c = Claude::new();
        let got = c
            .find_spawn(&[p.clone(), p.clone()], &work, 0, &|s: &Session| {
                s.name.contains("tests")
            })
            .expect("found");
        assert_eq!(got.id, "aaa");
        assert_eq!(got.name, "fix the tests");
        assert_eq!(
            got.created_at,
            rush_core::time::parse_rfc3339("2026-10-28T10:00:01Z")
        );
        assert!(
            c.find_spawn(std::slice::from_ref(&p), &work, 0, &|_: &Session| false)
                .is_none()
        );
        // Only what was written since.
        let later = rush_core::time::now() + 60_000;
        assert!(
            c.find_spawn(&[p], &work, later, &|_: &Session| true)
                .is_none()
        );
    }
}
