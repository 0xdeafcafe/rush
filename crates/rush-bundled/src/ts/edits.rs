//! Renames: the language server says what to change, and this writes it,
//! all of it or none, and never outside the checkout.
//! Ported from internal/bundled/ts/edits.go:1-272.

use std::collections::BTreeMap;
use std::path::Path;

use rush_plugin::Ctx;
use serde::Deserialize;
use serde_json::value::RawValue;

use super::server::{Server, decode, file_uri, path_of};
use super::tools::{Ask, Position, Range, doc_params};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct TextEdit {
    pub range: Range,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// LSP's, in either of its shapes: changes by URI, or documentChanges,
/// which may also create, rename or delete files.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct WorkspaceEdit {
    changes: Option<BTreeMap<String, Vec<TextEdit>>>,
    #[serde(rename = "documentChanges")]
    document_changes: Option<Vec<Box<RawValue>>>,
}

/// One of documentChanges' file operations.
#[derive(Default, Deserialize)]
#[serde(default)]
struct FileOp {
    kind: String,
    uri: String,
    #[serde(rename = "oldUri")]
    old_uri: String,
    #[serde(rename = "newUri")]
    new_uri: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct DocId {
    uri: String,
}

/// One of documentChanges' text edits.
#[derive(Default, Deserialize)]
#[serde(default)]
struct DocEdit {
    #[serde(rename = "textDocument")]
    text_document: DocId,
    edits: Vec<TextEdit>,
}

type Edits = (BTreeMap<String, Vec<TextEdit>>, Vec<(String, String)>);

impl WorkspaceEdit {
    /// Its text edits by file, and its file renames, in order; refused when
    /// it reaches outside root or creates or deletes.
    fn edits(&self, root: &str) -> Result<Edits, String> {
        let mut by: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
        let mut renames = Vec::new();
        let inside = |uri: &str| -> Result<String, String> {
            let p = path_of(uri);
            match crate::go_rel(root, &p) {
                Some(rel) if !rel.starts_with("..") => Ok(p),
                _ => Err(format!(
                    "the server's edit reaches {p}, outside {root}: not applied"
                )),
            }
        };
        for (uri, es) in self.changes.iter().flatten() {
            by.entry(inside(uri)?)
                .or_default()
                .extend(es.iter().cloned());
        }
        for raw in self.document_changes.iter().flatten() {
            let op: FileOp = rush_state::json::from_slice(raw.get().as_bytes()).unwrap_or_default();
            match op.kind.as_str() {
                "" => {
                    let d: DocEdit = rush_state::json::from_slice(raw.get().as_bytes())
                        .map_err(|e| e.to_string())?;
                    by.entry(inside(&d.text_document.uri)?)
                        .or_default()
                        .extend(d.edits);
                }
                "rename" => renames.push((inside(&op.old_uri)?, inside(&op.new_uri)?)),
                k => {
                    return Err(format!(
                        "the server's edit would {k} {}: not applied",
                        path_of(&op.uri)
                    ));
                }
            }
        }
        Ok((by, renames))
    }
}

/// Writes edits into text: LSP positions are 0-based lines and UTF-16
/// columns, and each edit is against the text before any of them.
pub(crate) fn apply(text: &str, edits: &[TextEdit]) -> Result<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    // strings.SplitAfter leaves an empty last part after a final newline,
    // or for empty text: one more line to point at.
    let n = lines.len() + usize::from(text.is_empty() || text.ends_with('\n'));
    let line_at = |i: usize| lines.get(i).copied().unwrap_or("");
    let offset = |p: Position| -> Result<usize, String> {
        let l = p.line as usize;
        if l > n || l == n && p.character > 0 {
            return Err(format!("an edit at line {} is past the end", p.line + 1));
        }
        let at: usize = (0..l).map(|i| line_at(i).len()).sum();
        if l == n {
            return Ok(at);
        }
        let line = line_at(l);
        let mut units = 0;
        for (i, r) in line.char_indices() {
            if units >= p.character {
                return Ok(at + i);
            }
            units += r.len_utf16() as i64;
        }
        Ok(at + line.trim_end_matches('\n').len())
    };
    let mut spans = Vec::with_capacity(edits.len());
    for e in edits {
        spans.push((
            offset(e.range.start)?,
            offset(e.range.end)?,
            e.new_text.as_str(),
        ));
    }
    spans.sort_by_key(|a| std::cmp::Reverse(a.0)); // stable, as SliceStable
    for w in spans.windows(2) {
        if w[1].1 > w[0].0 {
            return Err("the server's edits overlap: not applied".into());
        }
    }
    let mut out = text.to_string();
    for (from, to, t) in spans {
        if from > to || to > out.len() {
            return Err("the server's edits overlap: not applied".into());
        }
        out.replace_range(from..to, t);
    }
    Ok(out)
}

fn rel(p: &str, cwd: &str) -> String {
    match crate::go_rel(cwd, p) {
        Some(r) if !r.starts_with("..") => r,
        _ => p.to_string(),
    }
}

fn move_file(from: &str, to: &str) -> Result<(), String> {
    if std::fs::symlink_metadata(to).is_ok() || std::fs::metadata(to).is_ok() {
        return Err(format!("{to} is there already"));
    }
    if let Some(d) = Path::new(to).parent() {
        std::fs::create_dir_all(d).map_err(|e| rush_plugin::gopath::os_err("mkdir", d, &e))?;
    }
    std::fs::rename(from, to).map_err(|e| {
        format!(
            "rename {from} {to}: {}",
            rush_plugin::gopath::errno_text(&e)
        )
    })
}

impl Server {
    /// Writes a workspace edit to disk, all of it or none: every file's new
    /// text is worked out before any is written. It is the files changed,
    /// from cwd.
    pub(crate) fn apply_edit(&self, w: &WorkspaceEdit, cwd: &str) -> Result<Vec<String>, String> {
        let (by, renames) = w.edits(&self.root)?;
        let mut news: BTreeMap<String, String> = BTreeMap::new();
        for (p, es) in &by {
            let b = std::fs::read(p)
                .map_err(|e| rush_plugin::gopath::os_err("open", Path::new(p), &e))?;
            let out = apply(&String::from_utf8_lossy(&b), es).map_err(|e| format!("{p}: {e}"))?;
            news.insert(p.clone(), out);
        }
        let mut changed = Vec::new();
        for (p, text) in &news {
            std::fs::write(p, text)
                .map_err(|e| rush_plugin::gopath::os_err("open", Path::new(p), &e))?;
            changed.push(rel(p, cwd));
        }
        for (from, to) in &renames {
            move_file(from, to)?;
            changed.push(format!("{} → {}", rel(from, cwd), rel(to, cwd)));
        }
        let events: Vec<serde_json::Value> = news
            .keys()
            .map(|p| serde_json::json!({ "uri": file_uri(p), "type": 2 }))
            .collect();
        if !events.is_empty() {
            self.notify(
                "workspace/didChangeWatchedFiles",
                &serde_json::json!({ "changes": events }),
            );
        }
        changed.sort();
        Ok(changed)
    }
}

/// Renames the symbol at pos everywhere it's used, and writes it.
pub(crate) fn rename(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    pos: Position,
    a: &Ask,
    cwd: &str,
) -> Result<String, String> {
    if a.new_name.is_empty() {
        return Err("give new_name".into());
    }
    let r = s.call(
        ctx,
        "textDocument/rename",
        &doc_params(uri, pos, serde_json::json!({ "newName": a.new_name })),
    )?;
    let w: WorkspaceEdit = decode(r.as_deref())?;
    let changed = s.apply_edit(&w, cwd)?;
    if changed.is_empty() {
        return Ok("nothing to rename there".into());
    }
    Ok(format!(
        "renamed to {} in {} files:\n{}",
        a.new_name,
        changed.len(),
        changed.join("\n")
    ))
}

/// Moves a file or folder and rewrites the imports that name it, as the
/// server says to.
pub(crate) fn rename_file(
    ctx: &Ctx,
    s: &Server,
    uri: &str,
    _: Position,
    a: &Ask,
    cwd: &str,
) -> Result<String, String> {
    if a.to.is_empty() {
        return Err("give to: the new path".into());
    }
    let from = path_of(uri);
    let to = if Path::new(&a.to).is_absolute() {
        a.to.clone()
    } else {
        Path::new(cwd).join(&a.to).to_string_lossy().into_owned()
    };
    if crate::go_rel(&s.root, &to).is_none_or(|r| r.starts_with("..")) {
        return Err(format!("{to} is outside {}", s.root));
    }
    let files = serde_json::json!([{ "oldUri": file_uri(&from), "newUri": file_uri(&to) }]);
    let r = s.call(
        ctx,
        "workspace/willRenameFiles",
        &serde_json::json!({ "files": files }),
    )?;
    let w: WorkspaceEdit = decode(r.as_deref())?;
    let changed = s.apply_edit(&w, cwd)?;
    move_file(&from, &to).map_err(|e| {
        format!(
            "imports rewritten in {} files, but the move failed: {e}",
            changed.len()
        )
    })?;
    s.forget(uri);
    s.notify(
        "workspace/didRenameFiles",
        &serde_json::json!({ "files": files }),
    );
    let mut out = format!("moved {} → {}", rel(&from, cwd), rel(&to, cwd));
    if !changed.is_empty() {
        out.push_str(&format!(
            "; imports rewritten in {} files:\n{}",
            changed.len(),
            changed.join("\n")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ts_test.go TestApplyEditsInUTF16
    #[test]
    fn apply_edits_in_utf16() {
        let text = "const é = 1; const b = é;\nb + é\n";
        let edit = |l, c, n, s: &str| TextEdit {
            range: Range {
                start: Position {
                    line: l,
                    character: c,
                },
                end: Position {
                    line: l,
                    character: c + n,
                },
            },
            new_text: s.into(),
        };
        let out = apply(
            text,
            &[edit(0, 6, 1, "x"), edit(0, 23, 1, "x"), edit(1, 4, 1, "x")],
        )
        .unwrap();
        assert_eq!(out, "const x = 1; const b = x;\nb + x\n");
        assert!(
            apply(text, &[edit(0, 0, 5, "a"), edit(0, 2, 5, "b")]).is_err(),
            "overlapping edits applied"
        );
        // A line past the end, and the empty one after the last newline.
        assert!(apply("a\n", &[edit(3, 0, 0, "x")]).is_err());
        assert_eq!(apply("a\n", &[edit(1, 0, 0, "b\n")]).unwrap(), "a\nb\n");
    }

    #[test]
    fn edits_stay_inside_the_root() {
        let w: WorkspaceEdit = serde_json::from_str(
            r#"{"changes":{"file:///r/a.ts":[]},"documentChanges":[{"kind":"rename","oldUri":"file:///r/b.ts","newUri":"file:///r/c.ts"}]}"#,
        )
        .unwrap();
        let (by, renames) = w.edits("/r").unwrap();
        assert!(by.contains_key("/r/a.ts"));
        assert_eq!(renames, [("/r/b.ts".to_string(), "/r/c.ts".to_string())]);
        let out: WorkspaceEdit =
            serde_json::from_str(r#"{"changes":{"file:///etc/x":[]}}"#).unwrap();
        assert!(out.edits("/r").unwrap_err().contains("outside /r"));
        let del: WorkspaceEdit = serde_json::from_str(
            r#"{"documentChanges":[{"kind":"delete","uri":"file:///r/a.ts"}]}"#,
        )
        .unwrap();
        assert_eq!(
            del.edits("/r").unwrap_err(),
            "the server's edit would delete /r/a.ts: not applied"
        );
    }
}
