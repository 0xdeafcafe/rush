//! The Memory view: what the session's agent reads for its project, as
//! its adapter says (for Claude Code its auto memory, the CLAUDE.md files
//! and rules, settings, agents, skills…), each file with what it costs a
//! session, and the picked one shown in the bottom half. Read off the UI
//! thread, again at most every two seconds, with how often sessions
//! opened each note (the agent's CountOpens) and which notes are worth a
//! look (rush_memory::why) counted there too. After Go's
//! internal/ui/memory.go (memoryOf :32, its read :49-62, memoryLines :113,
//! the editor alone when short :119, what this session edited there :134,
//! the review line :144-160 and the rows :206 through projects_machine's
//! memory_head, mem_rows and mem_note_row (:288, :295), memNothing :336,
//! docPane's title rule :362, memoryKey's ↑↓ :376, fileSize :603) and
//! efficiency/findings.go:167 (Tokens).
//!
//! Editing the picked file in place (space), $EDITOR (ctrl+g) and
//! deleting it (x) are memedit.rs; the bottom half is its editor once
//! space opened one, else the file as read.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rush_core::adapter::{Memory, Profile};
use rush_core::text::Line;
use rush_core::time::Ms;

use super::paint::{dim, faint, spread, text};
use super::{agent, conn, open_key};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::projects_machine::memory::{mem_nothing, mem_rows, memory_head};
use crate::session::harness_name;
use crate::style::*;

/// A token count as the efficiency page says it.
pub fn tokens(n: i64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{:.0}k", n as f64 / 1e3),
        n if n >= 1000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// Bytes as a file's size.
pub fn file_size(n: i64) -> String {
    if n < 1 << 10 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    }
}

/// What each session's agent reads, as last read, and the files picked.
#[derive(Default)]
pub struct Memories {
    by_key: HashMap<String, (Ms, Memory)>,
    reading: HashSet<String>,
    /// A picked file's lines (None: it isn't there), by path.
    docs: HashMap<String, Option<Vec<String>>>,
    doc_reading: HashSet<String>,
}

/// What session key's agent reads, read again off the UI thread at most
/// every two seconds; None until the first read is in (memoryOf).
pub fn memory_of(m: &mut Model, key: &str) -> Option<Memory> {
    let now = m.now;
    let mem = m.kept::<Memories>();
    let have = mem.by_key.get(key).cloned();
    if have.as_ref().is_none_or(|(at, _)| now - at >= 2000) && !mem.reading.contains(key) {
        let a = agent(m, key).cloned();
        let Some(a) = a else {
            return have.map(|h| h.1);
        };
        let reg = m.reg.clone();
        let cwd = m
            .session
            .as_ref()
            .map(|v| v.sess.info.cwd.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| a.job.cwd.clone());
        m.kept::<Memories>().reading.insert(key.to_string());
        let k = key.to_string();
        m.later(
            move || {
                let ad = reg.get(rush_core::adapter::migrated(a.kind.as_str()).as_str())?;
                let mr = ad.memory_reader()?;
                let ps = reg.profiles_of(&ad);
                let p = ps
                    .iter()
                    .find(|p| p.name == a.job.account)
                    .or(ps.first())
                    .cloned()
                    .unwrap_or_else(|| Profile {
                        kind: a.kind.clone(),
                        ..Profile::default()
                    });
                let mut mem = mr.memory(&p, &cwd, &a.job.transcript_path);
                // How often sessions opened each note: it reads
                // transcripts, so only here, off the UI.
                if let Some(mk) = ad.memory_keeper() {
                    mk.count_opens(&p, &mut mem);
                }
                let now = rush_core::time::now();
                for f in &mut mem.files {
                    f.review = rush_memory::why(f, now);
                }
                Some(mem)
            },
            move |m, r| {
                let now = rush_core::time::now();
                let mem = m.kept::<Memories>();
                mem.reading.remove(&k);
                let r = r.unwrap_or_default();
                let same = mem.by_key.get(&k).is_some_and(|(_, o)| *o == r);
                mem.by_key.insert(k, (now, r));
                if same { Out::Idle } else { Out::Draw }
            },
        );
    }
    let mem = m.kept::<Memories>();
    mem.by_key.get(key).map(|h| h.1.clone())
}

/// What session key's agent reads, as last read, without reading it
/// again (Go's c.mem).
pub fn memory_known(m: &mut Model, key: &str) -> Option<Memory> {
    m.kept::<Memories>().by_key.get(key).map(|h| h.1.clone())
}

/// Forgets what session key's agent reads, so it's read again at once
/// (Go's c.mem = nil).
pub fn forget(m: &mut Model, key: &str) {
    let mem = m.kept::<Memories>();
    mem.by_key.remove(key);
    mem.docs.clear();
}

/// A picked file's lines, read off the UI thread; None while it's read,
/// Some(None) when it isn't there.
fn doc(m: &mut Model, path: &str) -> Option<Option<Vec<String>>> {
    let mem = m.kept::<Memories>();
    if let Some(d) = mem.docs.get(path) {
        return Some(d.clone());
    }
    if mem.doc_reading.insert(path.to_string()) {
        let p = path.to_string();
        m.later(
            {
                let p = p.clone();
                move || {
                    std::fs::read(&p).ok().map(|b| {
                        String::from_utf8_lossy(&b[..b.len().min(256 << 10)])
                            .lines()
                            .map(|l| l.replace('\t', "    "))
                            .collect::<Vec<_>>()
                    })
                }
            },
            move |m, d| {
                let mem = m.kept::<Memories>();
                mem.doc_reading.remove(&p);
                mem.docs.insert(p, d);
                Out::Draw
            },
        );
    }
    m.kept::<Memories>().docs.get(path).cloned()
}

/// The Memory view in h rows: the files on top, the picked one below,
/// which has the bottom half (memoryLines).
pub fn lines(m: &mut Model, key: &str, w: usize, h: usize) -> Vec<(Line, String)> {
    let mem = memory_of(m, key);
    let picked = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    let now = m.now;
    let kind = agent(m, key).map(|a| a.kind.clone()).unwrap_or_default();
    let harness = harness_name(&m.reg, &kind);
    let editing = super::memedit::editing(m, key);
    let ed_h = match h {
        h if h < 14 && editing => h, // too short to share: the editor, alone
        h if h < 14 => 0,
        h => h / 2,
    };
    let list_h = h - ed_h;
    let mut out: Vec<(Line, String)> = Vec::new();
    let empty = Memory::default();
    let r = mem.as_ref().unwrap_or(&empty);
    let review = r.files.iter().filter(|f| !f.review.is_empty()).count();
    if list_h > 0 {
        // The heading and its rule, then what this session edited, then
        // what's untidy and how many notes are worth a look.
        let mut head = memory_head(
            &harness,
            &r.files,
            &r.problems,
            review,
            !r.dir.is_empty(),
            w,
        )
        .into_iter()
        .map(|l| (l, String::new()));
        out.extend(head.by_ref().take(2));
        let mem_ed = super::changes::own_changes(m, key).0;
        if !mem_ed.is_empty() {
            // What this session wrote to it, kept out of Changes.
            let (a, d) = super::changes::change_stat(&mem_ed);
            let names: Vec<String> = mem_ed
                .iter()
                .map(|fc| {
                    Path::new(&fc.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect();
            let mut right = super::changes::add_del(a, d);
            right.spans.push(plain("  "));
            out.push((
                fit(
                    spread(
                        line([
                            plain("  "),
                            dim("this session edited "),
                            text(names.join(", ")),
                        ]),
                        right,
                        w,
                    ),
                    w,
                ),
                String::new(),
            ));
        }
        // Go counts the list's room before the review line (memory.go:143).
        let above = out.len();
        out.extend(head);
        let nothing = mem_nothing(mem.is_some());
        let rows: Vec<(Line, String)> = mem_rows(&r.files, &r.when, &picked, w, now, true, nothing)
            .into_iter()
            .map(|r| (r.line, r.r#ref))
            .collect();
        // The list keeps the picked file in sight.
        let room = list_h.saturating_sub(above).max(1);
        let sel = rows
            .iter()
            .position(|(_, r)| !picked.is_empty() && *r == picked)
            .unwrap_or(0);
        let c = conn(m, key);
        let top = &mut c.vs.mem_top;
        if sel < *top {
            *top = sel.saturating_sub(1); // its group's heading too
        } else if sel >= *top + room {
            *top = sel + 1 - room;
        }
        *top = (*top).min(rows.len().saturating_sub(room));
        let top = *top;
        out.extend(rows.into_iter().skip(top).take(room));
        while out.len() < list_h {
            out.push((Line::new(), String::new()));
        }
    }
    if ed_h > 0 {
        out.extend(
            doc_pane(m, &picked, w, ed_h)
                .into_iter()
                .map(|l| (l, String::new())),
        );
    }
    out
}

/// The picked file in h rows: a rule with its name and folder, then the
/// file (docPane, read only).
fn doc_pane(m: &mut Model, picked: &str, w: usize, h: usize) -> Vec<Line> {
    let rule = |left: Line, right: Line| {
        let fill = w.saturating_sub(left.width() + right.width() + 2).max(1);
        let mut l = left;
        l.spans.push(faint(format!(" {} ", "─".repeat(fill))));
        join(&mut l, right);
        l
    };
    let Some(path) = picked.strip_prefix("mem:") else {
        let mut out = vec![rule(line([faint("──")]), Line::new())];
        while out.len() < h {
            out.push(Line::new());
        }
        if h > 3 {
            out[h / 2] = line([
                plain("    "),
                dim("Pick a file above to see it here · space edits it"),
            ]);
        }
        return out;
    };
    if let Some(key) = open_key(m)
        && let Some(out) = super::memedit::pane(m, &key, path, w, h)
    {
        return out;
    }
    let base = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = tildify(
        &Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let d = doc(m, path);
    let chip = match &d {
        None => dim("reading…"),
        Some(None) => dim("new file"),
        Some(Some(_)) => dim(if path.ends_with(".json") {
            "json"
        } else if path.ends_with(".md") {
            "markdown"
        } else {
            "text"
        }),
    };
    let right = line([chip]);
    let mut left = line([faint("── "), sp(base, SUB)]);
    let room = w.saturating_sub(left.width() + right.width() + 8);
    if room > 8 {
        left.spans.push(plain("  "));
        left.spans
            .push(faint(rush_draw::truncate(&folder, room, "…").into_owned()));
    }
    let mut out = vec![rule(left, right)];
    if let Some(Some(lines)) = d {
        for l in lines.iter().take(h.saturating_sub(1)) {
            out.push(line([
                plain(" "),
                sp(fit_str(l, w.saturating_sub(1)).trim_end().to_string(), TEXT),
            ]));
        }
    }
    while out.len() < h {
        out.push(Line::new());
    }
    out.truncate(h);
    out
}

/// ↑↓ in the memory view go round its files (memoryKey).
pub fn step(m: &mut Model, d: isize) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(mem) = m.kept::<Memories>().by_key.get(&key).map(|h| h.1.clone()) else {
        return Out::Idle;
    };
    let n = mem.files.len();
    if n == 0 {
        return Out::Idle;
    }
    let Some(v) = m.session.as_mut() else {
        return Out::Pass;
    };
    let cur = mem
        .files
        .iter()
        .position(|f| format!("mem:{}", f.path) == v.sel);
    let i = match cur {
        Some(c) => (c as isize + d).rem_euclid(n as isize) as usize,
        None => 0,
    };
    v.sel = format!("mem:{}", mem.files[i].path);
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_tokens() {
        assert_eq!(file_size(512), "512 B");
        assert_eq!(file_size(2048), "2.0 KB");
        assert_eq!(tokens(1500), "1.5k");
        assert_eq!(tokens(15_000), "15k");
    }
}
