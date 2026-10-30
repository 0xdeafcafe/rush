//! The memory view's file, edited where it's shown: space (→ or e) on a
//! picked file opens it in the bottom half to edit, every key then the
//! editor's (ctrl+s saves, esc gives the keys back, asking about unsaved
//! changes); ctrl+g opens it in $EDITOR; x (delete, ctrl+x) deletes it,
//! asked first, a memory note leaving MEMORY.md too. After Go's
//! internal/ui/memory.go (docPane :362, memoryKey :450's space, ctrl+g and
//! x, confirmForget :580, docKey :617, saveDoc :667, leaveDoc :710). The
//! file is read off the UI thread, saved off it too. $EDITOR goes
//! through [`edit_then`] (Go's editorCmd, dialog.go:69-82), which the
//! memory review's e (memreview.go:221-235) and settings' files
//! (dialog.go:64 editFile) use too: the frame hands it the terminal
//! (frame::outside), or a test's own ([`set_editor`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rush_core::text::Line;

use super::paint::{dim, faint};
use super::{agent, open_key};
use crate::frame::confirm::Confirm;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::plugin_ui::doc::{self, DocEditor, Kind};
use crate::session::harness_name;
use crate::style::*;

/// What's done once $EDITOR has closed: Err says why it couldn't run.
pub type Done = crate::frame::outside::Done;

/// Hands the terminal to $EDITOR on a file, then runs done (Go's
/// editorCmd). done runs once the key that asked has been handled, never
/// within it: a sheet asking is out of the model while its key runs.
pub type EditorRun = std::sync::Arc<dyn Fn(&mut Model, &str, Done) -> Out>;

/// The way $EDITOR is run, when a test has given its own.
#[derive(Default)]
pub struct Editors {
    run: Option<EditorRun>,
}

/// Makes run the way $EDITOR gets the terminal, in place of the frame's
/// (a test's).
pub fn set_editor(m: &mut Model, run: EditorRun) {
    m.kept::<Editors>().run = Some(run);
}

/// Opens path in $EDITOR, its folder made first, then runs done
/// (editorCmd).
pub fn edit_then(m: &mut Model, path: &str, done: Done) -> Out {
    if let Some(dir) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match m.kept::<Editors>().run.clone() {
        Some(run) => run(m, path, done),
        None => crate::frame::outside::edit(m, path, done),
    }
}

/// ctrl+g in the Agents list: a Done agent picked with nothing typed
/// comes back; else the box's draft opens in $EDITOR (keys.go:519).
pub fn list_draft_key(m: &mut Model, k: &Press) -> Out {
    if k.s != "ctrl+g" {
        return Out::Pass;
    }
    if m.input.is_empty()
        && let Some(i) = m.selected()
        && m.agents[i].done
    {
        let key = m.agents[i].key.clone();
        crate::screens::cmdbar::stops::toggle_done(m, &key); // nothing to write
        return Out::Draw;
    }
    edit_draft(m, None)
}

/// ctrl+g in a Session: the same for its message box (rushmode.go:4465).
pub fn draft_key(m: &mut Model, k: &Press) -> Out {
    if k.s != "ctrl+g" {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if super::conn(m, &key).input.is_empty() && m.agents.iter().any(|a| a.key == key && a.done) {
        crate::screens::cmdbar::stops::toggle_done(m, &key);
        return Out::Draw;
    }
    edit_draft(m, Some(key))
}

/// The last paste in the draft of a box (a Session's by its key, else the
/// Agents box), or the whole draft when it has none, opened in $EDITOR,
/// what comes back put in its place: its temp file written before, and
/// read after, off the UI thread (pastes.go:221 editInEditor, :251
/// editDraft).
pub fn edit_draft(m: &mut Model, pane: Option<String>) -> Out {
    let input = match &pane {
        Some(k) => &super::conn(m, k).input,
        None => &m.input,
    };
    let (text, id) = input.pastes.to_edit(&input.text);
    m.later(
        move || write_temp(&text),
        move |m, r| match r {
            Err(e) => {
                m.fail(&format!("editor: {e}"));
                Out::Draw
            }
            Ok(tmp) => {
                let path = tmp.to_string_lossy().into_owned();
                edit_then(
                    m,
                    &path,
                    Box::new(move |m, ran| {
                        m.later(
                            move || {
                                let read = std::fs::read_to_string(&tmp);
                                let _ = std::fs::remove_file(&tmp);
                                (ran, read)
                            },
                            move |m, (ran, read)| edited(m, pane.as_deref(), id, ran, read),
                        );
                        Out::Draw
                    }),
                )
            }
        },
    );
    Out::Draw
}

/// Text into a new temp file, rush-*.md, for $EDITOR.
fn write_temp(text: &str) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..16 {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("rush-{}{nanos}{n}.md", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(mut f) => {
                let _ = f.write_all(text.as_bytes());
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("couldn't make a temp file".into())
}

/// What came back from $EDITOR put in its box: paste id's text and its
/// chip, or (id 0) the whole draft, the cursor at its end (model.go:1232
/// editedMsg, pastes.go:262 applyEdit).
fn edited(
    m: &mut Model,
    pane: Option<&str>,
    id: usize,
    ran: Result<(), String>,
    read: std::io::Result<String>,
) -> Out {
    let text = match (ran, read) {
        (Err(e), _) => {
            m.fail(&format!("editor: {e}"));
            return Out::Draw;
        }
        (Ok(()), Err(e)) => {
            m.fail(&format!("editor: {e}"));
            return Out::Draw;
        }
        (Ok(()), Ok(t)) => t.trim_end_matches('\n').to_string(),
    };
    let input = match pane {
        // The Session's box only while it's open.
        Some(k) if open_key(m).as_deref() == Some(k) => &mut super::conn(m, k).input,
        Some(_) => return Out::Draw,
        None => &mut m.input,
    };
    input.text = input.pastes.apply_edit(&input.text, id, &text);
    input.pos = input.text.len();
    input.anchor = None;
    Out::Draw
}

/// The memory view, read again once $EDITOR is done with one of its
/// files (dialogReload).
fn reload(key: &str) -> Done {
    let k = key.to_string();
    Box::new(move |m, r| {
        super::memory::forget(m, &k);
        if let Err(e) = r {
            m.fail(&e);
        }
        Out::Draw
    })
}

/// A file of the memory view open in its editor, and whether it has the
/// keys (c.memEd, c.memEdit).
pub struct Ed {
    pub ed: DocEditor,
    pub editing: bool,
}

/// The memory view's editors, by session key.
#[derive(Default)]
pub struct MemEdits {
    by_key: HashMap<String, Ed>,
    /// Files being read for an editor, by session key.
    opening: HashSet<String>,
}

/// The picked file, when the memory view shows and a file is picked.
fn picked_path(m: &mut Model, key: &str) -> Option<String> {
    if super::views::view_name(m, key) != "memory" {
        return None;
    }
    let sel = m.session.as_ref()?.sel.clone();
    sel.strip_prefix("mem:").map(str::to_string)
}

/// Whether session key's memory file has the keys (editingDoc).
pub fn editing(m: &mut Model, key: &str) -> bool {
    m.kept::<MemEdits>()
        .by_key
        .get(key)
        .is_some_and(|e| e.editing)
}

/// The memory view's keys for its picked file; while it's being edited,
/// every key is the editor's (memoryKey).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(path) = picked_path(m, &key) else {
        if let Some(e) = m.kept::<MemEdits>().by_key.get_mut(&key) {
            e.editing = false;
        }
        return Out::Pass;
    };
    let s = k.s.as_str();
    let open = m
        .kept::<MemEdits>()
        .by_key
        .get(&key)
        .is_some_and(|e| e.editing && e.ed.path == path);
    if open {
        return doc_key(m, &key, k);
    }
    let space = matches!(s, "space" | " ");
    if !super::conn(m, &key).input.is_empty() && !space {
        return Out::Pass;
    }
    match s {
        "space" | " " | "right" | "e" => open_doc(m, &key, &path),
        "ctrl+g" => {
            super::memory::forget(m, &key); // read again once it's back
            edit_then(m, &path, reload(&key))
        }
        "x" | "delete" | "ctrl+x" => {
            let missing = super::memory::memory_known(m, &key)
                .and_then(|mem| mem.files.into_iter().find(|f| f.path == path))
                .is_none_or(|f| f.missing);
            if !missing {
                confirm_forget(m, &key, &path);
            }
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// Space on a picked file: its editor, read off the UI thread, gets the
/// keys once it's in, unless it can't be edited here.
pub(crate) fn open_doc(m: &mut Model, key: &str, path: &str) -> Out {
    let st = m.kept::<MemEdits>();
    if let Some(e) = st.by_key.get_mut(key).filter(|e| e.ed.path == path) {
        if !e.ed.read_only.is_empty() {
            let why = e.ed.read_only.clone();
            m.flash_err(&why);
            return Out::Draw;
        }
        e.editing = true;
        return Out::Draw;
    }
    if !st.opening.insert(key.to_string()) {
        let base = base_name(path);
        m.flash(&format!("still reading {base}…"));
        return Out::Draw;
    }
    let (k, p) = (key.to_string(), path.to_string());
    m.later(
        move || DocEditor::open_now(&p),
        move |m, ed| {
            let st = m.kept::<MemEdits>();
            st.opening.remove(&k);
            let why = ed.read_only.clone();
            st.by_key.insert(
                k,
                Ed {
                    ed,
                    editing: why.is_empty(),
                },
            );
            if !why.is_empty() {
                m.flash_err(&why);
            }
            Out::Draw
        },
    );
    Out::Draw
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A key for the file being edited (docKey).
fn doc_key(m: &mut Model, key: &str, k: &Press) -> Out {
    let Some(e) = m.kept::<MemEdits>().by_key.get_mut(key) else {
        return Out::Pass;
    };
    match k.s.as_str() {
        "esc" => {
            if e.ed.selection().is_some() {
                e.ed.anchor = None;
                return Out::Draw;
            }
            return leave_doc(m, key);
        }
        "ctrl+g" => {
            let path = e.ed.path.clone();
            if !e.ed.dirty() {
                e.editing = false;
                super::memory::forget(m, key);
                return edit_then(m, &path, reload(key));
            }
            return save_doc(m, key, After::Editor);
        }
        _ => {}
    }
    let (act, copied, _) = e.ed.key(k);
    if act == "format" {
        match doc::format_json(&e.ed.text(), &e.ed.indent) {
            Err(_) => m.flash_err("it can't be formatted until it's valid JSON"),
            Ok(text) if text != e.ed.text() => {
                let (ln, _) = e.ed.cursor_at();
                e.ed.change(text.chars().collect(), 0, false);
                // Back to the same line, near enough.
                let mut n = 1;
                for i in 0..e.ed.buf.len() {
                    if n >= ln {
                        break;
                    }
                    if e.ed.buf[i] == '\n' {
                        n += 1;
                        e.ed.pos = i + 1;
                    }
                }
            }
            Ok(_) => {}
        }
    }
    if !copied.is_empty() {
        m.copy_text(copied);
    }
    if act == "save" {
        return save_doc(m, key, After::Stay);
    }
    Out::Draw
}

/// What follows a save that went through.
#[derive(Clone, Copy, PartialEq)]
enum After {
    Stay,
    /// The keys go back to the list.
    Leave,
    /// It opens in $EDITOR.
    Editor,
}

/// Saves the file being edited, asking first when it changed on disk
/// meanwhile or isn't valid JSON (saveDoc).
fn save_doc(m: &mut Model, key: &str, then: After) -> Out {
    let harness = agent(m, key)
        .map(|a| harness_name(&m.reg, &a.kind))
        .unwrap_or_default();
    let Some(e) = m.kept::<MemEdits>().by_key.get_mut(key) else {
        return Out::Idle;
    };
    let bad = if e.ed.kind == Kind::Json {
        e.ed.problems().iter().rfind(|d| !d.warn).cloned()
    } else {
        None
    };
    let k = key.to_string();
    let base = base_name(&e.ed.path);
    if e.ed.stale {
        m.ask(
            Confirm::new(
                format!("{base} changed on disk since you opened it."),
                move |m| write(m, &k, then),
            )
            .detail("y saves yours over it"),
        );
        return Out::Draw;
    }
    if let Some(d) = bad {
        m.ask(
            Confirm::new(
                format!(
                    "It isn't valid JSON (line {}: {}). Save anyway?",
                    d.line + 1,
                    d.msg
                ),
                move |m| write(m, &k, then),
            )
            .detail(format!("{harness} can't read it until it's fixed")),
        );
        return Out::Draw;
    }
    write(m, key, then)
}

/// Writes the file off the UI thread (saveCmd).
fn write(m: &mut Model, key: &str, then: After) -> Out {
    let Some(e) = m.kept::<MemEdits>().by_key.get_mut(key) else {
        return Out::Idle;
    };
    let w = match e.ed.to_write() {
        Ok(w) => w,
        Err(err) => {
            m.fail(&format!("couldn't save: {err}"));
            return Out::Draw;
        }
    };
    e.ed.saving = true;
    let k = key.to_string();
    m.later(
        move || {
            let r = doc::write_doc(&w);
            (w, r)
        },
        move |m, (w, r)| {
            let Some(e) = m.kept::<MemEdits>().by_key.get_mut(&k) else {
                return Out::Idle;
            };
            e.ed.saving = false;
            let path = w.path.clone();
            match r {
                Err(err) => {
                    m.fail(&format!("couldn't save: {err}"));
                    return Out::Draw;
                }
                Ok(modified) => e.ed.wrote(w, modified),
            }
            match then {
                After::Stay | After::Leave => {
                    if then == After::Leave {
                        e.editing = false;
                    }
                    super::memory::forget(m, &k);
                    m.flash(&format!("saved {}", tildify(&path)));
                    Out::Draw
                }
                After::Editor => {
                    e.editing = false;
                    super::memory::forget(m, &k);
                    edit_then(m, &path, reload(&k))
                }
            }
        },
    );
    Out::Draw
}

/// The keys back to the list, asking about unsaved changes (leaveDoc).
fn leave_doc(m: &mut Model, key: &str) -> Out {
    let Some(e) = m.kept::<MemEdits>().by_key.get_mut(key) else {
        return Out::Idle;
    };
    if !e.ed.dirty() {
        e.editing = false;
        return Out::Draw;
    }
    let base = base_name(&e.ed.path);
    let (k1, k2) = (key.to_string(), key.to_string());
    let mut c = Confirm::new(format!("Save your changes to {base}?"), move |m| {
        save_doc(m, &k1, After::Leave)
    })
    .detail("n keeps editing");
    c.bang_text = "throw them away".into();
    c.on_bang = Some(Box::new(move |m: &mut Model| {
        if let Some(e) = m.kept::<MemEdits>().by_key.get_mut(&k2) {
            let saved: Vec<char> = e.ed.saved.chars().collect();
            let pos = e.ed.pos.min(saved.len());
            e.ed.change(saved, pos, false);
            e.ed.anchor = None;
            e.editing = false;
        }
        Out::Draw
    }));
    m.ask(c);
    Out::Draw
}

/// Asks before deleting the file at path, then deletes it off the UI
/// thread: a memory note also leaves MEMORY.md (confirmForget).
fn confirm_forget(m: &mut Model, key: &str, path: &str) {
    let kind = agent(m, key).map(|a| a.kind.clone()).unwrap_or_default();
    let harness = harness_name(&m.reg, &kind);
    let (k, p) = (key.to_string(), path.to_string());
    m.ask(
        Confirm::new(format!("Delete {}?", tildify(path)), move |m| {
            let reg = m.reg.clone();
            let p1 = p.clone();
            m.later(
                move || {
                    let ad = reg.get(rush_core::adapter::migrated(kind.as_str()).as_str());
                    match ad.as_ref().and_then(|ad| ad.memory_reader()) {
                        Some(mr) => mr.forget_file(&p1).map_err(|e| e.to_string()),
                        None => std::fs::remove_file(&p1).map_err(|e| e.to_string()),
                    }
                },
                move |m, r| {
                    if let Err(e) = r {
                        m.fail(&e);
                    }
                    super::memory::forget(m, &k);
                    if let Some(v) = m.session.as_mut()
                        && v.sel == format!("mem:{p}")
                    {
                        v.sel.clear();
                    }
                    let st = m.kept::<MemEdits>();
                    if st.by_key.get(&k).is_some_and(|e| e.ed.path == p) {
                        st.by_key.remove(&k);
                    }
                    Out::Draw
                },
            );
            Out::Draw
        })
        .detail(format!(
            "{harness} stops reading it · a memory note also leaves MEMORY.md"
        )),
    );
}

/// The picked file in its editor, in h rows: a rule with its name, its
/// state and problems, the file, and a foot with the problem on the
/// cursor's line and where the cursor is (docPane); None when path has no
/// editor open.
pub fn pane(m: &mut Model, key: &str, path: &str, w: usize, h: usize) -> Option<Vec<Line>> {
    let e = m
        .kept::<MemEdits>()
        .by_key
        .get_mut(key)
        .filter(|e| e.ed.path == path)?;
    let focused = e.editing;
    let ed = &mut e.ed;
    let rule = |left: Line, right: Line| {
        let fill = w.saturating_sub(left.width() + right.width() + 2).max(1);
        let mut l = left;
        l.spans.push(faint(format!(" {} ", "─".repeat(fill))));
        join(&mut l, right);
        l
    };
    let base = base_name(&ed.path);
    let mut left = line([faint("── ")]);
    if focused {
        left.spans.push(sp("✎ ", ORANGE));
        left.spans.push(spb(base, TEXT));
    } else {
        left.spans.push(sp(base, SUB));
    }
    let mut chips = Vec::new();
    if ed.loading {
        chips.push(dim("reading…"));
    } else if !ed.read_only.is_empty() {
        chips.push(sp("read only", YELLOW));
    } else if ed.dirty() {
        chips.push(sp("● unsaved", ORANGE));
    } else if ed.missing {
        chips.push(dim("new file"));
    }
    if ed.stale {
        chips.push(sp("changed on disk", YELLOW));
    }
    let kind = ed.kind;
    let empty = ed.text().trim().is_empty();
    let diags = ed.problems().to_vec();
    let errs = diags.iter().filter(|d| !d.warn).count();
    let warns = diags.len() - errs;
    if errs > 0 {
        chips.push(sp(format!("✗ {errs}"), RED));
    } else if warns > 0 {
        chips.push(sp(format!("! {warns}"), YELLOW));
    } else if kind == Kind::Json && !empty {
        chips.push(sp("✓ valid", GREEN));
    }
    chips.push(dim(match kind {
        Kind::Markdown => "markdown",
        Kind::Json => "json",
        Kind::Text => "text",
    }));
    let mut right = Line::new();
    for (i, c) in chips.into_iter().enumerate() {
        if i > 0 {
            right.spans.push(dim(" · "));
        }
        right.spans.push(c);
    }
    // The folder gives way to the chips.
    let folder = tildify(
        &Path::new(&ed.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let room = w.saturating_sub(left.width() + right.width() + 8);
    if room > 8 {
        let cs: Vec<char> = folder.chars().collect();
        let shown = if cs.len() > room {
            format!(
                "…{}",
                cs[cs.len() - (room - 1)..].iter().collect::<String>()
            )
        } else {
            folder
        };
        left.spans.push(plain("  "));
        left.spans.push(faint(shown));
    }
    let mut out = vec![rule(left, right)];
    for l in ed.view(w.saturating_sub(1), h.saturating_sub(2).max(1), focused) {
        let mut row = line([plain(" ")]);
        join(&mut row, l);
        out.push(row);
    }
    let (ln, col) = ed.cursor_at();
    let mut msg = line([plain("  ")]);
    if !ed.read_only.is_empty() && !ed.loading {
        msg.spans.push(sp(ed.read_only.clone(), YELLOW));
    }
    let show = diags
        .iter()
        .filter(|d| d.line + 1 == ln)
        .min_by_key(|d| d.warn)
        .or(diags.first());
    if let Some(d) = show {
        msg = line([plain("  ")]);
        let c = if d.warn { YELLOW } else { RED };
        msg.spans.push(sp(if d.warn { "! " } else { "✗ " }, c));
        msg.spans.push(sp(format!("line {}: ", d.line + 1), c));
        msg.spans.push(sp(d.msg.clone(), SUB));
        if diags.len() > 1 {
            msg.spans.push(dim(format!("  ({} problems)", diags.len())));
        }
    }
    let wh = line([dim(format!("ln {ln}, col {col}")), plain("  ")]);
    out.push(super::paint::spread(msg, wh, w));
    out.truncate(h);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_names() {
        assert_eq!(base_name("/a/b/MEMORY.md"), "MEMORY.md");
    }
}
