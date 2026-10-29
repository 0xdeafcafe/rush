//! The Memory view: what the session's agent reads for its project, as
//! its adapter says (for Claude Code its auto memory, the CLAUDE.md files
//! and rules, settings, agents, skills…), each file with what it costs a
//! session, and the picked one shown in the bottom half. Read off the UI
//! thread, again at most every two seconds. After Go's
//! internal/ui/memory.go (memoryOf :32, memoryLines :113, what this
//! session edited there :134, memRows :160,
//! memNothing :236, docPane's title rule :262, memoryKey's ↑↓ :376,
//! fileSize :603) and efficiency/findings.go:167 (Tokens).
//!
//! Not here yet: editing the picked file in place (space), $EDITOR
//! (ctrl+g) and forgetting a file (x); the bottom half shows the file.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rush_core::adapter::{Memory, MemoryFile, Profile};
use rush_core::text::Line;
use rush_core::time::Ms;

use super::dock::picked1;
use super::paint::{dim, faint, spread, text};
use super::{agent, conn, open_key};
use crate::frame::Out;
use crate::model::Model;
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
                Some(mr.memory(&p, &cwd, &a.job.transcript_path))
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

/// A row for each file, under its group's name and when its files reach
/// the model; on the right what a file costs (memRows).
pub fn mem_rows(
    files: &[MemoryFile],
    when: &std::collections::BTreeMap<String, String>,
    picked: &str,
    w: usize,
    now: Ms,
    nothing: &str,
) -> Vec<(Line, String)> {
    let mut out = Vec::new();
    let mut group = "";
    for f in files {
        if f.group != group {
            group = &f.group;
            let mut head = line([plain("  "), spb(group.to_string(), TEXT)]);
            let w_ = line([
                faint(when.get(group).cloned().unwrap_or_default()),
                plain("  "),
            ]);
            // The memory's folder gives way to when.
            if group == "Auto memory" {
                let dir = tildify(
                    &Path::new(&f.path)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                let room = w.saturating_sub(1 + head.width() + w_.width() + 6);
                if room > 10 {
                    let dw = rush_core::text::width(&dir);
                    let shown = if dw > room {
                        let cs: Vec<char> = dir.chars().collect();
                        let keep = cs.len().saturating_sub(dw - room + 1);
                        format!("…{}", cs[cs.len() - keep..].iter().collect::<String>())
                    } else {
                        dir
                    };
                    head.spans.push(plain("  "));
                    head.spans.push(faint(shown));
                }
            }
            out.push((fit(spread(head, w_, w.saturating_sub(1)), w), String::new()));
        }
        let r = format!("mem:{}", f.path);
        let mut meta: Vec<rush_core::text::Span> = Vec::new();
        if f.up > 0 {
            meta.push(sp(format!("≈{}", tokens(f.up)), SUB));
        }
        if f.later > 0 {
            meta.push(faint(format!("≈{} {}", tokens(f.later), f.when)));
        }
        let ago = || dur(now - f.mod_time);
        if f.missing && !f.warn.is_empty() {
            meta.push(dim("missing"));
        } else if f.missing {
            meta.push(dim("not written yet"));
        } else if f.up == 0 && f.later == 0 {
            meta.push(dim(format!("{} · {} ago", file_size(f.size), ago())));
        } else {
            meta.push(dim(format!("{} ago", ago())));
        }
        let mut right = Line::new();
        for (i, s) in meta.into_iter().enumerate() {
            if i > 0 {
                right.spans.push(dim(" · "));
            }
            right.spans.push(s);
        }
        let mark = if f.missing && !f.warn.is_empty() {
            sp("◇ ", YELLOW)
        } else if f.missing {
            faint("◇ ")
        } else {
            sp("◆ ", BLUE)
        };
        let mut left = if f.sub {
            line([plain("      "), faint("↳ ")])
        } else {
            line([plain("    "), mark])
        };
        left.spans.push(sp(f.name.clone(), TEXT));
        if !f.kind.is_empty() {
            left.spans.push(plain("  "));
            left.spans.push(sp(f.kind.clone(), SUB));
        }
        let (note, ink) = if f.warn.is_empty() {
            (&f.about, DIM)
        } else {
            (&f.warn, YELLOW)
        };
        if !note.is_empty() {
            let room = w.saturating_sub(left.width() + right.width() + 8);
            if room > 12 {
                left.spans.push(plain("  "));
                left.spans.push(sp(
                    rush_draw::truncate(&one_line(note), room, "…").into_owned(),
                    ink,
                ));
            }
        }
        right.spans.push(plain("  "));
        let mut row = spread(left, right, w.saturating_sub(1));
        if r == picked {
            row = picked1(row, w, true);
        }
        out.push((fit(row, w), r));
    }
    if files.is_empty() {
        out.push((
            fit(line([plain("    "), dim(nothing.to_string())]), w),
            String::new(),
        ));
    }
    out
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
    let ed_h = if h < 14 { 0 } else { h / 2 };
    let list_h = h - ed_h;
    let mut out: Vec<(Line, String)> = Vec::new();
    let empty = Memory::default();
    let r = mem.as_ref().unwrap_or(&empty);
    let up: i64 = r.files.iter().map(|f| f.up).sum();
    out.push((
        fit(
            spread(
                line([
                    plain("  "),
                    spb("Memory", SUB),
                    plain("   "),
                    dim(format!("what {harness} reads for this project, and when")),
                ]),
                line([
                    sp(format!("≈{}", tokens(up)), SUB),
                    dim(format!(" tokens every session · {} files", r.files.len())),
                    plain("  "),
                ]),
                w,
            ),
            w,
        ),
        String::new(),
    ));
    out.push((
        line([plain("  "), faint("─".repeat(w.saturating_sub(4)))]),
        String::new(),
    ));
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
    let head = out.len();
    if let Some(p) = r.problems.first() {
        let n = if r.problems.len() == 1 {
            "1 thing to tidy".to_string()
        } else {
            format!("{} things to tidy", r.problems.len())
        };
        let mut left = line([plain("  "), sp(format!("! {n}"), YELLOW)]);
        let room = w.saturating_sub(left.width() + 5);
        if room > 12 {
            left.spans.push(dim(format!(
                " · {}",
                rush_draw::truncate(&p.title, room, "…")
            )));
        }
        out.push((fit(left, w), String::new()));
    }
    let nothing = if mem.is_none() {
        "Reading…"
    } else {
        "Nothing found for this project."
    };
    let rows = mem_rows(&r.files, &r.when, &picked, w, now, nothing);
    // The list keeps the picked file in sight.
    let room = list_h.saturating_sub(out.len()).max(1);
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
    let _ = head;
    while out.len() < list_h {
        out.push((Line::new(), String::new()));
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

    #[test]
    fn rows_group_files() {
        let files = vec![
            MemoryFile {
                group: "Project".into(),
                path: "/w/CLAUDE.md".into(),
                name: "CLAUDE.md".into(),
                up: 1200,
                mod_time: 0,
                ..MemoryFile::default()
            },
            MemoryFile {
                group: "Project".into(),
                path: "/w/.claude/rules.md".into(),
                name: "rules.md".into(),
                missing: true,
                ..MemoryFile::default()
            },
        ];
        let rows = mem_rows(&files, &Default::default(), "", 80, 60_000, "");
        let text: Vec<String> = rows.iter().map(|(l, _)| l.plain()).collect();
        assert!(text[0].starts_with("  Project"));
        assert!(text[1].starts_with("    ◆ CLAUDE.md"), "{}", text[1]);
        assert!(text[1].contains("≈1.2k · 1m ago"), "{}", text[1]);
        assert!(text[2].contains("not written yet"));
        assert_eq!(rows[1].1, "mem:/w/CLAUDE.md");
    }
}
