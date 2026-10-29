//! The Changes view: the session's own edits by file, each opening to its
//! hunks under the turn that made them, then the working tree as git sees
//! it, the files this session didn't touch marked as such. git is only
//! ever run off the UI thread: a reading older than five seconds is
//! taken again once the view is drawn, and the frame after has it. After
//! Go's internal/convo/changes.go (FileChange :20, Changes :34,
//! changesNow :44, Tree :104, Stat :113, WorkingTree :132, readTree :170,
//! turnsRight :307, ChangesView :311, treeDiff :587) and
//! internal/ui/rushmode.go (alt+r :4196, readsFromTop :2705).
//!
//! Not here yet: the file in full with its diff laid over it (alt+v,
//! FileView), syntax colour in the hunks, and paths made links.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rush_convo::{Item, Session, Status};
use rush_core::text::{Color, Line};
use rush_core::time::{Ms, now};
use rush_core::tool::{Kind, Patch};

use super::conn;
use super::paint::{dim, faint, plural, row, section, text};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// An added line's ground, and a deleted one's.
const BG_ADD: Color = Color::Rgb(0x16, 0x30, 0x1a);
const BG_DEL: Color = Color::Rgb(0x3a, 0x17, 0x14);

/// Everything this session did to one file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub add: i64,
    pub del: i64,
    pub new: bool,
    pub turns: Vec<u32>,
    pub patches: Vec<Patch>,
    /// For each patch, the step that made it ("t3:s:toolu_…").
    pub from: Vec<String>,
    /// A new file's content, and the step that created it.
    pub content: String,
    pub new_from: String,
}

/// The session's edits by file, in the order files were first touched
/// (changesNow).
pub fn changes(s: &Session) -> Vec<FileChange> {
    let mut by_path: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<FileChange> = Vec::new();
    for t in &s.turns {
        for it in &t.items {
            let Item::Step(id) = it else {
                continue;
            };
            let st = s.step(*id);
            if !matches!(st.kind, Kind::Edit | Kind::Write) || st.status != Status::Ok {
                continue;
            }
            let path = st.call().input.path.clone();
            let i = *by_path.entry(path.clone()).or_insert_with(|| {
                out.push(FileChange {
                    path,
                    ..FileChange::default()
                });
                out.len() - 1
            });
            let fc = &mut out[i];
            if fc.turns.last() != Some(&t.n) {
                fc.turns.push(t.n);
            }
            let o = st.out();
            let step_ref = format!("t{}:s:{}", t.n, st.id);
            if o.created {
                let content = st.call().input.content.clone();
                fc.new = true;
                fc.add += content.lines().count() as i64;
                fc.content = content;
                fc.new_from = step_ref;
                continue;
            }
            for p in o.patches {
                for l in &p.lines {
                    if l.starts_with('+') {
                        fc.add += 1;
                    } else if l.starts_with('-') {
                        fc.del += 1;
                    }
                }
                fc.patches.push(p);
                fc.from.push(step_ref.clone());
            }
        }
    }
    out
}

/// The open session's edits, worked out again only when a step changed
/// (Changes' memo).
pub fn of_open(m: &mut Model) -> Arc<Vec<FileChange>> {
    let Some(v) = m.session.as_ref() else {
        return Arc::default();
    };
    let at = (v.sess.serial(), v.sess.step_ver());
    if let Some((k, c)) = &m.kept::<Trees>().memo
        && *k == at
    {
        return c.clone();
    }
    let Some(v) = m.session.as_ref() else {
        return Arc::default();
    };
    let c = Arc::new(changes(&v.sess));
    m.kept::<Trees>().memo = Some((at, c.clone()));
    c
}

/// What the open session added and deleted, all files.
pub fn stat(m: &mut Model) -> (i64, i64) {
    of_open(m)
        .iter()
        .fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
}

/// One file git reports as changed in the working tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    pub add: i64,
    pub del: i64,
    pub binary: bool,
    pub untracked: bool,
}

/// What git sees in a folder: changed and untracked files against HEAD.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub head: String,
    pub root: String,
    pub files: Vec<TreeFile>,
    pub err: String,
    /// When it was read.
    pub at: Ms,
}

impl Tree {
    /// Lines added and deleted in the working tree.
    pub fn stat(&self) -> (i64, i64) {
        self.files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
    }
}

/// A session (its serial) at a version of its steps.
type Ver = (u64, u64);

/// Each folder's latest reading of git, and each opened file's diff.
#[derive(Default)]
pub struct Trees {
    by_dir: HashMap<String, Tree>,
    reading: HashSet<String>,
    diffs: HashMap<String, (Ms, Vec<String>)>,
    diff_reading: HashSet<String>,
    /// The open session's edits, and the session and step version they're
    /// of.
    memo: Option<(Ver, Arc<Vec<FileChange>>)>,
}

/// git in dir, its output when it ran clean (GIT_OPTIONAL_LOCKS=0: a look
/// must not hold up the agent's own git).
fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Reads what git sees in dir (readTree). Off the UI thread only.
pub fn read_tree(dir: &str) -> Tree {
    let mut t = Tree {
        at: now(),
        ..Tree::default()
    };
    let Some(top) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        t.err = "not a git repository".into();
        return t;
    };
    let root = top.trim().to_string();
    t.root = root.clone();
    let base = match git(dir, &["rev-parse", "--short", "HEAD"]) {
        Some(h) => {
            t.head = h.trim().to_string();
            "HEAD"
        }
        None => {
            t.head = "no commits yet".into();
            "--cached" // staged files are all there is
        }
    };
    let out = git(&root, &["diff", base, "--numstat", "-z", "--no-renames"]).unwrap_or_default();
    for rec in out.split('\0') {
        let f: Vec<&str> = rec.splitn(3, '\t').collect();
        if f.len() != 3 || f[2].is_empty() {
            continue;
        }
        let path = Path::new(&root).join(f[2]).to_string_lossy().into_owned();
        let mut tf = TreeFile {
            path,
            ..TreeFile::default()
        };
        if f[0] == "-" {
            tf.binary = true;
        } else {
            tf.add = f[0].parse().unwrap_or(0);
            tf.del = f[1].parse().unwrap_or(0);
        }
        t.files.push(tf);
    }
    let untracked =
        git(&root, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    for p in untracked.split('\0').filter(|p| !p.is_empty()) {
        t.files.push(TreeFile {
            path: Path::new(&root).join(p).to_string_lossy().into_owned(),
            untracked: true,
            ..TreeFile::default()
        });
    }
    t.files.sort_by(|a, b| a.path.cmp(&b.path));
    t
}

/// The last reading of git for dir, read again off the UI thread when
/// it's older than five seconds (WorkingTree); "reading git…" until the
/// first is in.
pub fn tree_of(m: &mut Model, dir: &str) -> Tree {
    let now_ = m.now;
    let ts = m.kept::<Trees>();
    let have = ts.by_dir.get(dir).cloned();
    let stale = have.as_ref().is_none_or(|t| now_ - t.at > 5000);
    if stale && !ts.reading.contains(dir) {
        ts.reading.insert(dir.to_string());
        let d = dir.to_string();
        m.later(move || read_tree(&d), {
            let d = dir.to_string();
            move |m: &mut Model, t: Tree| {
                let ts = m.kept::<Trees>();
                ts.reading.remove(&d);
                let same = ts
                    .by_dir
                    .get(&d)
                    .is_some_and(|o| o.files == t.files && o.head == t.head && o.err == t.err);
                ts.by_dir.insert(d, t);
                if same { Out::Idle } else { Out::Draw }
            }
        });
    }
    let ts = m.kept::<Trees>();
    ts.by_dir.get(dir).cloned().unwrap_or(Tree {
        err: "reading git…".into(),
        at: now_,
        ..Tree::default()
    })
}

/// git's diff of one working-tree file against HEAD (the file itself when
/// it's untracked), read off the UI thread with each reading of the tree
/// (treeDiff); "reading the diff…" until it's in.
fn tree_diff(m: &mut Model, t: &Tree, f: &TreeFile) -> Vec<String> {
    let k = format!("{}\0{}", t.root, f.path);
    let ts = m.kept::<Trees>();
    let have = ts.diffs.get(&k).cloned();
    if have.as_ref().is_none_or(|d| d.0 != t.at) && !ts.diff_reading.contains(&k) {
        ts.diff_reading.insert(k.clone());
        let (root, f, at) = (t.root.clone(), f.clone(), t.at);
        let key = k.clone();
        m.later(
            move || {
                let out = if f.untracked {
                    Command::new("git")
                        .args([
                            "-C",
                            &root,
                            "diff",
                            "--no-index",
                            "--",
                            "/dev/null",
                            &f.path,
                        ])
                        .env("GIT_OPTIONAL_LOCKS", "0")
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                        .unwrap_or_default()
                } else {
                    git(&root, &["diff", "HEAD", "--", &f.path]).unwrap_or_default()
                };
                diff_lines(&out)
            },
            move |m, lines| {
                let ts = m.kept::<Trees>();
                ts.diff_reading.remove(&key);
                ts.diffs.insert(key, (at, lines));
                Out::Draw
            },
        );
    }
    match have {
        Some((_, l)) => l,
        None => vec!["reading the diff…".into()],
    }
}

/// A diff's lines, less the header lines that say what the row already
/// does, 400 at most.
pub fn diff_lines(out: &str) -> Vec<String> {
    let mut lines: Vec<String> = out
        .trim_end_matches('\n')
        .split('\n')
        .filter(|l| {
            !(l.starts_with("diff --git")
                || l.starts_with("index ")
                || l.starts_with("--- ")
                || l.starts_with("+++ ")
                || l.starts_with("new file")
                || l.starts_with('\\'))
        })
        .map(str::to_string)
        .collect();
    if lines.len() > 400 {
        let more = lines.len() - 400;
        lines.truncate(400);
        lines.push(format!("… {more} more lines"));
    }
    lines
}

/// A changed file's row's right side: its counts and the turns that made
/// it, the turns left out when they'd crowd the row (turnsRight).
fn turns_right(mut counts: Line, turns: &[u32], capw: usize) -> Line {
    let room = capw.saturating_sub(4 + counts.width() + 3);
    let mut b = String::new();
    for (i, n) in turns.iter().enumerate() {
        if i > 0 {
            b.push(' ');
        }
        b.push_str(&format!("#{n}"));
        if b.len() > room {
            return Line::new();
        }
    }
    counts.spans.push(plain("   "));
    counts.spans.push(dim(b));
    counts
}

/// path as the session would say it: inside its folder, from there.
fn rel(path: &str, cwd: &str) -> String {
    if !cwd.is_empty()
        && let Some(r) = path.strip_prefix(cwd).and_then(|r| r.strip_prefix('/'))
    {
        return r.to_string();
    }
    tildify(path)
}

fn plus() -> rush_core::text::Span {
    spb("+", GREEN)
}
fn minus() -> rush_core::text::Span {
    spb("−", RED)
}

/// The Changes view's rows, w wide (ChangesView).
pub fn lines(m: &mut Model, key: &str, w: usize) -> Vec<(Line, String)> {
    let Some(v) = m.session.as_ref() else {
        return Vec::new();
    };
    if v.sess.partial {
        return vec![
            (Line::new(), String::new()),
            (line([plain("  "), dim("counting…")]), String::new()),
        ];
    }
    let sel = v.sel.clone();
    let verbose = v.verbose;
    let dir = if v.sess.info.cwd.is_empty() {
        v.sess.cwd.clone()
    } else {
        v.sess.info.cwd.clone()
    };
    let chg = of_open(m);
    let (open, marks) = {
        let c = conn(m, key);
        (c.vs.open.clone(), c.vs.marks.clone())
    };
    let mut out: Vec<(Line, String)> = Vec::new();
    let add = |out: &mut Vec<(Line, String)>, r: &str, left: Line, right: Line| {
        let left = if !r.is_empty() && r == sel {
            // The keyboard's mark in the gutter.
            let mut l = line([sp("▏", SUB)]);
            let mut first = true;
            for s in left.spans {
                if first {
                    first = false;
                    if let Some(t) = s.text.strip_prefix(' ') {
                        l.spans
                            .push(rush_core::text::Span::new(t.to_string(), s.style));
                        continue;
                    }
                }
                l.spans.push(s);
            }
            l
        } else {
            left
        };
        out.push((row(None, left, right, w, w), r.to_string()));
    };
    let rule = |out: &mut Vec<(Line, String)>, title: &str, meta: &str| {
        if !out.is_empty() {
            out.push((row(None, Line::new(), Line::new(), w, w), String::new()));
        }
        out.push((
            row(None, section(title, meta, w, 3), Line::new(), w, w),
            String::new(),
        ));
    };
    let (adds, dels) = chg.iter().fold((0, 0), |(a, d), f| (a + f.add, d + f.del));
    let mut meta = "nothing edited yet".to_string();
    if !chg.is_empty() {
        let seen = chg
            .iter()
            .filter(|f| marks.get(&f.path).copied().unwrap_or(false))
            .count();
        meta = format!(
            "{} · +{adds} −{dels} · {seen} of {} reviewed · alt+r marks one",
            plural(chg.len(), "file"),
            chg.len()
        );
    }
    rule(&mut out, "This session", &meta);
    let bw = w.saturating_sub(16);
    for fc in chg.iter() {
        let r = format!("chg:{}", fc.path);
        let is_open = open.get(&r).copied().unwrap_or(false);
        let arrow = faint(if is_open { "▾" } else { "▸" });
        let counts = if fc.new {
            line([sp(format!("new · {} lines", fc.add), GREEN)])
        } else {
            line([
                sp(format!("+{}", fc.add), GREEN),
                plain(" "),
                sp(format!("−{}", fc.del), RED),
            ])
        };
        let marked = marks.get(&fc.path).copied().unwrap_or(false);
        let (mark, name) = if marked {
            (sp("✓ ", GREEN), dim(rel(&fc.path, &dir)))
        } else {
            (plain("  "), text(rel(&fc.path, &dir)))
        };
        add(
            &mut out,
            &r,
            line([plain("  "), arrow, plain(" "), mark, name]),
            turns_right(counts, &fc.turns, w),
        );
        if !is_open {
            continue;
        }
        let pad = "       ";
        // Each hunk under a row naming the turn that made it; enter on it goes
        // to that step in the conversation.
        let hunk = |out: &mut Vec<(Line, String)>, from: &str, what: &str| {
            let turn = from.split(':').next().unwrap_or("");
            add(
                out,
                &format!("jump:{from}"),
                line([
                    plain(pad),
                    sp("@ ", BLUE),
                    dim(what.to_string()),
                    plain("  "),
                    sp(format!("#{}", turn.trim_start_matches('t')), SUB),
                ]),
                line([dim("enter goes to the step")]),
            );
        };
        if fc.new {
            hunk(&mut out, &fc.new_from, "created");
            for (i, l) in fc.content.trim_end_matches('\n').split('\n').enumerate() {
                if i >= 40 && !verbose {
                    add(
                        &mut out,
                        "",
                        line([plain(pad), dim("… ctrl+o shows the rest")]),
                        Line::new(),
                    );
                    break;
                }
                add(
                    &mut out,
                    "",
                    line([
                        plain(pad),
                        sp("▏", GREEN),
                        dim(format!("{:5} ", i + 1)),
                        sp(fit_str(l, bw).trim_end().to_string(), SUB),
                    ]),
                    Line::new(),
                );
            }
        }
        for (pi, p) in fc.patches.iter().enumerate() {
            let from = fc.from.get(pi).map_or("", String::as_str);
            hunk(&mut out, from, &format!("line {}", p.new_start));
            let (mut old_n, mut new_n) = (p.old_start, p.new_start);
            for l in &p.lines {
                let l = if l.is_empty() { " " } else { l.as_str() };
                if l.starts_with('\\') {
                    continue; // "\ No newline at end of file"
                }
                let body = fit_str(&l[1..], bw).trim_end().to_string();
                let (bg, n, sign, ink) = match l.as_bytes()[0] {
                    b'+' => {
                        new_n += 1;
                        (Some(BG_ADD), new_n - 1, plus(), TEXT)
                    }
                    b'-' => {
                        old_n += 1;
                        (Some(BG_DEL), old_n - 1, minus(), TEXT)
                    }
                    _ => {
                        old_n += 1;
                        new_n += 1;
                        (None, new_n - 1, plain(" "), SUB)
                    }
                };
                let left = line([
                    plain(pad),
                    dim(format!("{n:5} ")),
                    sign,
                    plain(" "),
                    sp(body, ink),
                ]);
                out.push((row(bg, left, Line::new(), w, w), String::new()));
            }
        }
    }
    if dir.is_empty() {
        return out;
    }
    let tree = tree_of(m, &dir);
    if !tree.err.is_empty() {
        rule(&mut out, "Working tree", &tree.err);
        return out;
    }
    let ours: HashSet<&str> = chg.iter().map(|f| f.path.as_str()).collect();
    let others = tree
        .files
        .iter()
        .filter(|f| !ours.contains(f.path.as_str()))
        .count();
    let mut meta = format!(
        "{} changed against {}",
        plural(tree.files.len(), "file"),
        tree.head
    );
    if others > 0 {
        meta.push_str(&format!(" · {others} not from this session"));
    }
    rule(&mut out, "Working tree", &meta);
    if tree.files.is_empty() {
        add(
            &mut out,
            "",
            line([plain("    "), dim("clean")]),
            Line::new(),
        );
    }
    for (i, f) in tree.files.iter().enumerate() {
        if i >= 60 && !verbose {
            add(
                &mut out,
                "",
                line([
                    plain("    "),
                    dim(format!(
                        "… {} more · ctrl+o shows all",
                        tree.files.len() - i
                    )),
                ]),
                Line::new(),
            );
            break;
        }
        let mut counts = if f.untracked {
            line([sp("new", GREEN)])
        } else if f.binary {
            line([dim("binary")])
        } else {
            line([
                sp(format!("+{}", f.add), GREEN),
                plain(" "),
                sp(format!("−{}", f.del), RED),
            ])
        };
        let is_ours = ours.contains(f.path.as_str());
        let (who, mark) = if is_ours {
            (sp("this session", ORANGE), sp("●", ORANGE))
        } else {
            (dim("not from this session"), dim("·"))
        };
        counts.spans.push(plain("   "));
        counts.spans.push(who);
        let r = format!("tree:{}", f.path);
        let is_open = open.get(&r).copied().unwrap_or(false);
        let arrow = faint(if is_open { "▾" } else { "▸" });
        add(
            &mut out,
            &r,
            line([
                plain("  "),
                arrow,
                plain(" "),
                mark,
                plain(" "),
                text(rel(&f.path, &dir)),
            ]),
            counts,
        );
        if is_open {
            for l in tree_diff(m, &tree, f) {
                if l.starts_with("@@") {
                    let t = rush_draw::truncate(&l, w.saturating_sub(10), "…").into_owned();
                    out.push((
                        row(
                            None,
                            line([plain("       "), sp(t, BLUE)]),
                            Line::new(),
                            w,
                            w,
                        ),
                        String::new(),
                    ));
                    continue;
                }
                let (bg, sign) = if l.starts_with('+') {
                    (Some(BG_ADD), plus())
                } else if l.starts_with('-') {
                    (Some(BG_DEL), minus())
                } else {
                    (None, plain(" "))
                };
                let body = l
                    .strip_prefix(['+', '-', ' '])
                    .unwrap_or(&l)
                    .replace('\t', "    ");
                let left = line([
                    plain("       "),
                    sign,
                    plain(" "),
                    text(fit_str(&body, w.saturating_sub(12)).trim_end().to_string()),
                ]);
                out.push((row(bg, left, Line::new(), w, w), String::new()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lines_drop_headers() {
        let d = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n";
        assert_eq!(diff_lines(d), vec!["@@ -1 +1 @@", "-a", "+b"]);
    }

    #[test]
    fn turns_leave_the_row_when_they_crowd_it() {
        let c = line([plain("+1 −0")]);
        assert_eq!(turns_right(c.clone(), &[3, 4], 80).plain(), "+1 −0   #3 #4");
        let many: Vec<u32> = (1..60).collect();
        assert_eq!(turns_right(c, &many, 40).plain(), "");
    }

    #[test]
    fn rel_paths() {
        assert_eq!(rel("/w/src/a.rs", "/w"), "src/a.rs");
        assert_eq!(rel("/x/a.rs", "/w"), "/x/a.rs");
    }
}
