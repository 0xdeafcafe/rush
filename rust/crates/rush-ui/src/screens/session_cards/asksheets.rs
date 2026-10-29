//! Claude Code's commands that need the session: /btw's prompts to pass a
//! message to a subagent, /subtask, and /export's sheet. After Go's
//! internal/ui/asksheets.go (relayPrompt :61, childRelayPrompt :68,
//! subtaskPrompt :74, exportSheet :81, openExport :96, save :128, key
//! :156, body :178). askClaude :21 is the host's ([`super::ask`]).

use rush_core::text::Line;

use super::util::{approx_tokens, expand, join};
use super::{Sub, ask};
use crate::frame::sheet::{sheet_row, sheet_title};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// Asks Claude to pass msg on to its running subagent sa word for word:
/// Claude Code takes no message into a subagent but its own SendMessage
/// tool (relayPrompt).
pub fn relay_prompt(sa: &Sub, msg: &str) -> String {
    format!(
        "Pass this message on to your running subagent {} ({}) with SendMessage, word for word, then carry on with what you were doing. Don't act on it yourself.\n\nThe message: {msg}",
        sa.tool_use_id, sa.r#type
    )
}

/// Asks Codex to pass msg on to sa, one of its spawned threads
/// (childRelayPrompt).
pub fn child_relay_prompt(sa: &Sub, msg: &str) -> String {
    format!(
        "Pass this message on to your subagent {} word for word: send_message while it's running, followup_task if it has finished. Then carry on with what you were doing. Don't act on it yourself.\n\nThe message: {msg}",
        one_line(&sa.description)
    )
}

/// Asks the agent to send a subagent off with the task, as Claude Code's
/// /subtask does, and to carry on meanwhile (subtaskPrompt).
pub fn subtask_prompt(task: &str) -> String {
    format!(
        "Send a subagent off in the background (the Agent tool with run_in_background, or spawn_agent) to do the task below. Give it everything it needs from our conversation so far, since it starts without it. Don't wait for it: carry on here, and when it's done tell me what it found.\n\nThe task: {task}"
    )
}

/// /export: saves or copies the conversation as the plain text Claude
/// Code's /export writes (exportSheet).
#[derive(Default)]
pub struct ExportSheet {
    pub conn: String,
    pub text: String,
    pub filename: String,
    pub dir: String,
    /// 0 copy, 1 save.
    pub cur: usize,
    pub err: String,
    pub loading: bool,
    /// Written in the background; enter waits for it.
    pub saving: bool,
}

/// Asks Claude Code for agent key's conversation as text; given a file
/// name it's saved there straight away, else the sheet opens and waits
/// on it (openExport).
pub fn open_export(m: &mut Model, key: &str, dir: &str, arg: &str) -> Out {
    let had = m.sheet.take();
    if arg.is_empty() {
        m.open_sheet(Box::new(ExportSheet {
            conn: key.into(),
            dir: dir.into(),
            loading: true,
            ..ExportSheet::default()
        }));
    }
    let (k, a, d) = (key.to_string(), arg.to_string(), dir.to_string());
    let asked = ask(
        m,
        key,
        serde_json::json!({"subtype": "export_conversation"}),
        Box::new(move |m, r| exported(m, &k, &d, &a, r)),
    );
    if !asked && arg.is_empty() {
        // askClaude said why; the sheet that was stays.
        m.sheet = had;
    } else if !arg.is_empty() && m.sheet.is_none() {
        m.sheet = had;
    }
    Out::Draw
}

// The conversation came back (or didn't): into the sheet, or saved.
fn exported(
    m: &mut Model,
    key: &str,
    dir: &str,
    arg: &str,
    r: Result<serde_json::Value, String>,
) -> Out {
    let mut k = ExportSheet {
        conn: key.into(),
        dir: dir.into(),
        ..ExportSheet::default()
    };
    match r {
        Err(e) => k.err = e,
        Ok(v) => match v.get("text").and_then(|t| t.as_str()) {
            None => k.err = "Claude Code sent back something rush can't read".into(),
            Some(t) => {
                k.text = t.to_string();
                k.filename = v
                    .get("default_filename")
                    .and_then(|f| f.as_str())
                    .filter(|f| !f.is_empty())
                    .unwrap_or("conversation.txt")
                    .to_string();
                if k.text.trim().is_empty() {
                    k.err = "there's nothing in the conversation yet".into();
                }
            }
        },
    }
    if !arg.is_empty() {
        if k.err.is_empty() {
            save(m, key, dir, arg, k.text);
        } else {
            m.flash_err(&k.err);
        }
        return Out::Draw;
    }
    if let Some(s) = m.sheet_as::<ExportSheet>().filter(|s| s.conn == key) {
        *s = k;
    }
    Out::Draw
}

/// The export sheet whose write landed last, and whether it was written:
/// for a sheet whose write landed while its key was still running.
#[derive(Default)]
struct Saved(Option<(String, bool)>);

/// Writes text to name, relative to the agent's folder, not rush's, off
/// the UI thread; the sheet closes once it's written (save).
fn save(m: &mut Model, key: &str, dir: &str, name: &str, text: String) {
    let path = if !std::path::Path::new(name).is_absolute() && !name.starts_with('~') {
        join(dir, name)
    } else {
        name.to_string()
    };
    let path = expand(&path);
    let key = key.to_string();
    let p = path.clone();
    m.later(
        move || std::fs::write(&p, text).map_err(|e| e.to_string()),
        move |m, r| {
            if let Some(s) = m.sheet_as::<ExportSheet>().filter(|s| s.conn == key) {
                s.saving = false;
            }
            m.kept::<Saved>().0 = Some((key.clone(), r.is_ok()));
            match r {
                Err(e) => m.flash_err(&format!("couldn't save it: {e}")),
                Ok(()) => {
                    if m.sheet_as::<ExportSheet>().is_some_and(|s| s.conn == key) {
                        m.sheet = None;
                    }
                    m.flash(&format!("saved the conversation to {}", tildify(&path)));
                }
            }
            Out::Draw
        },
    );
}

impl Sheet for ExportSheet {
    fn width(&self, _m: &Model) -> usize {
        76
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" => Out::Close,
            "up" | "down" | "tab" | "shift+tab" => {
                self.cur = 1 - self.cur;
                Out::Draw
            }
            "enter" => {
                if self.loading || !self.err.is_empty() {
                    return Out::Idle;
                }
                if self.cur == 1 {
                    if self.saving {
                        return Out::Idle;
                    }
                    self.saving = true;
                    m.kept::<Saved>().0 = None;
                    save(m, &self.conn, &self.dir, &self.filename, self.text.clone());
                    // Landed already (no loop): written, the sheet closes.
                    if let Some((k, ok)) = m.kept::<Saved>().0.take()
                        && k == self.conn
                    {
                        self.saving = false;
                        if ok {
                            return Out::Close;
                        }
                    }
                    return Out::Draw;
                }
                m.copy_text(self.text.clone());
                m.flash(&format!(
                    "copied the conversation · {} lines",
                    self.text.matches('\n').count() + 1
                ));
                Out::Close
            }
            _ => Out::Idle,
        }
    }

    fn body(&mut self, _m: &Model, w: usize, _h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title(
                "Export",
                "the conversation as plain text, as Claude Code writes it",
                w,
            ),
            Line::new(),
        ];
        if self.loading {
            out.extend([
                line([sp("  asking Claude Code for it…", DIM)]),
                Line::new(),
                keys_fit(w, &[("esc", "close")]),
            ]);
            return out;
        }
        if !self.err.is_empty() {
            out.extend([
                line([sp(format!("  {}", self.err), RED)]),
                Line::new(),
                keys_fit(w, &[("esc", "close")]),
            ]);
            return out;
        }
        out.push(line([sp(
            format!(
                "  {} lines · {}",
                self.text.matches('\n').count() + 1,
                approx_tokens(self.text.len() as i64)
            ),
            DIM,
        )]));
        out.push(Line::new());
        let save_about = format!(
            "{} · /export <name> saves elsewhere",
            tildify(&join(&self.dir, &self.filename))
        );
        let choices = [
            ("Copy to the clipboard", String::new()),
            ("Save to a file", save_about),
        ];
        for (i, (name, about)) in choices.iter().enumerate() {
            out.push(sheet_row(line([spb(*name, TEXT)]), i == self.cur, w));
            if !about.is_empty() {
                out.push(line([
                    plain("    "),
                    sp(
                        rush_draw::truncate(about, w.saturating_sub(6), "…").into_owned(),
                        FAINT,
                    ),
                ]));
            }
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[("↑↓", "choose"), ("enter", "go"), ("esc", "cancel")],
        ));
        out
    }
}
