//! What Claude Code loads from a project's memory and instructions, when,
//! and what looks untidy about them; and every file a session reads, for
//! rush's memory screen. Port of internal/adapters/claude/claude/memory.go
//! and internal/adapters/claude/memory.go.
//!
//! At the start of every session Claude Code reads the CLAUDE.md files
//! (the user's, the project's and its parent folders', CLAUDE.local.md),
//! the rules without a paths: list, whatever they @-import, and the first
//! INDEX_LINES lines (at most INDEX_BYTES) of the auto memory's MEMORY.md.
//! A memory note is read only when it's recalled, which goes by its line
//! in MEMORY.md or its description.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::sync::LazyLock;

use regex::Regex;
use rush_core::adapter::{Memory, MemoryFile, MemoryProblem, front_matter as front_matter_of};

use crate::files::{base_of, clean, dir_of, home, join};
use crate::jobs::{ms_of, project_slug};

/// How much of MEMORY.md Claude Code loads (Claude Code 2.1: its lines
/// after 200 are cut, and at 25,000 bytes whatever comes first).
pub const INDEX_LINES: usize = 200;
pub const INDEX_BYTES: usize = 25_000;
/// What Claude Code asks each line of MEMORY.md to stay under.
pub const INDEX_LINE_LEN: usize = 150;
/// When the instructions loaded every session are worth trimming, in
/// estimated tokens; BIG_FILE is the size of one file Claude Code itself
/// warns about.
pub const BIG_INSTRUCTIONS: i64 = 10_000;
pub const BIG_FILE: i64 = 40_000;
const IMPORT_HOPS: usize = 5;

/// A rough token count for n bytes of text: a quarter.
pub fn est_tokens(n: i64) -> i64 {
    (n + 3) / 4
}

/// name, description and type from a markdown file's frontmatter, at any
/// depth (a memory note keeps its type under metadata), and "paths" when a
/// rule has a paths: list.
pub fn front_matter(path: &str) -> BTreeMap<String, String> {
    match fs::File::open(path) {
        Ok(f) => front_matter_of(io::BufReader::new(f)),
        Err(_) => BTreeMap::new(),
    }
}

fn fm(m: &BTreeMap<String, String>, k: &str) -> String {
    m.get(k).cloned().unwrap_or_default()
}

/// A MEMORY.md, and how much of it is loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemIndex {
    pub path: String,
    pub exists: bool,
    pub lines: usize,
    pub bytes: usize,
    /// What reaches Claude.
    pub loaded_lines: usize,
    pub loaded_bytes: usize,
    pub links: Vec<MemLink>,
    /// Lines over INDEX_LINE_LEN characters.
    pub long: usize,
}

impl MemIndex {
    /// Whether some of it isn't loaded.
    pub fn cut(&self) -> bool {
        self.loaded_lines < self.lines
    }
}

/// A line of MEMORY.md pointing at a note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemLink {
    /// From 1.
    pub line: usize,
    /// The note.
    pub file: String,
    /// Past what's loaded.
    pub past: bool,
}

static MD_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(([^)\s]+\.md)\)").unwrap());

/// Reads a MEMORY.md.
pub fn read_mem_index(path: &str) -> MemIndex {
    let mut ix = MemIndex {
        path: path.to_string(),
        ..MemIndex::default()
    };
    let Ok(b) = fs::read(path) else {
        return ix;
    };
    ix.exists = true;
    ix.bytes = b.len();
    let s = String::from_utf8_lossy(&b);
    let text = s.trim_end_matches('\n');
    if text.trim().is_empty() {
        return ix;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    ix.lines = lines.len();
    let mut n = 0;
    for (i, l) in lines.iter().enumerate() {
        let loaded = i < INDEX_LINES && n + l.len() <= INDEX_BYTES;
        if loaded {
            n += l.len() + 1;
            ix.loaded_lines += 1;
        }
        if l.chars().count() > INDEX_LINE_LEN {
            ix.long += 1;
        }
        for m in MD_LINK.captures_iter(l) {
            let mut p = m[1].to_string();
            if p.contains("://") {
                continue;
            }
            if !p.starts_with('/') {
                p = join(&[&dir_of(path), &p]);
            }
            ix.links.push(MemLink {
                line: i + 1,
                file: clean(&p),
                past: !loaded,
            });
        }
    }
    ix.loaded_bytes = n.min(ix.bytes);
    ix
}

/// A memory note, next to MEMORY.md.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemNote {
    pub path: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub size: i64,
    /// Whether a loaded line of MEMORY.md points at it.
    pub indexed: bool,
    /// Whether one does only past what's loaded.
    pub past_cut: bool,
    /// Files it names that no longer exist.
    pub gone: Vec<String>,
    /// The path of a note it repeats.
    pub same: String,
}

/// A file loaded as instructions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instruction {
    pub path: String,
    /// user, project, local, parent, rule, import
    pub scope: String,
    /// The file that @-imports it, and how it was written.
    pub from: String,
    pub reference: String,
    /// A rule with paths: it loads when Claude reads a file that matches
    /// them, not at the start.
    pub on_demand: bool,
    pub missing: bool,
    pub size: i64,
}

/// The folders a session in cwd reads from, nearest first, up to but not
/// including home, whose .claude is the user's own.
fn up_from(cwd: &str, home: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut d = cwd.to_string();
    while !d.is_empty() && d != "/" && d != "." && d != home {
        let up = dir_of(&d);
        dirs.push(d);
        d = up;
    }
    dirs
}

struct Instr<'a> {
    home: &'a str,
    out: Vec<Instruction>,
    seen: HashSet<String>,
}

impl Instr<'_> {
    fn add(&mut self, mut ins: Instruction, hops: usize) {
        if self.seen.contains(&ins.path) {
            return;
        }
        let st = fs::metadata(&ins.path);
        let bad = st.as_ref().map_or(true, |m| m.is_dir());
        if ins.scope != "import" && bad {
            return;
        }
        self.seen.insert(ins.path.clone());
        ins.missing = bad;
        if !bad && let Ok(m) = &st {
            ins.size = m.len() as i64;
        }
        let (path, on_demand, missing) = (ins.path.clone(), ins.on_demand, ins.missing);
        self.out.push(ins);
        if missing || hops >= IMPORT_HOPS {
            return;
        }
        for im in imports(&path, self.home) {
            self.add(
                Instruction {
                    path: im.1,
                    scope: "import".into(),
                    from: path.clone(),
                    reference: im.0,
                    on_demand,
                    ..Instruction::default()
                },
                hops + 1,
            );
        }
    }

    fn rules(&mut self, root: &str, scope: &str) {
        let mut files = Vec::new();
        let _ = crate::files::walk_files(std::path::Path::new(&join(&[root, "rules"])), &mut |p| {
            if p.extension().is_some_and(|x| x == "md") {
                files.push(p.to_string_lossy().into_owned());
            }
            Ok(())
        });
        for p in files {
            let on_demand = !fm(&front_matter(&p), "paths").is_empty();
            self.add(
                Instruction {
                    path: p,
                    scope: scope.into(),
                    on_demand,
                    ..Instruction::default()
                },
                0,
            );
        }
    }
}

/// The instruction files a session in cwd loads, the user's first, then
/// the project's and each parent folder's nearest first (up to but not
/// including home), each followed by what it @-imports.
pub fn instructions(cfg: &str, cwd: &str) -> Vec<Instruction> {
    instructions_in(cfg, cwd, &home())
}

fn instructions_in(cfg: &str, cwd: &str, home: &str) -> Vec<Instruction> {
    let mut x = Instr {
        home,
        out: Vec::new(),
        seen: HashSet::new(),
    };
    let one = |path: String, scope: &str| Instruction {
        path,
        scope: scope.into(),
        ..Instruction::default()
    };
    x.add(one(join(&[cfg, "CLAUDE.md"]), "user"), 0);
    let dirs = up_from(cwd, home);
    for d in &dirs {
        let scope = if d == cwd { "project" } else { "parent" };
        x.add(one(join(&[d, "CLAUDE.md"]), scope), 0);
        x.add(one(join(&[d, ".claude", "CLAUDE.md"]), scope), 0);
        x.add(one(join(&[d, "CLAUDE.local.md"]), "local"), 0);
    }
    x.rules(cfg, "rule");
    for d in &dirs {
        x.rules(&join(&[d, ".claude"]), "rule");
    }
    x.out
}

static AT_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|[\s(])@([^\s@()\[\]<>`"']+)"#).unwrap());
static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new("`[^`]*`").unwrap());
static FILE_EXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.[A-Za-z][A-Za-z0-9]{0,5}$").unwrap());

/// The @paths a file imports, outside code, as Claude Code reads them:
/// from home with ~/, else from the file's folder. One that doesn't exist
/// counts only when it's plainly a file (a path, or a name with an
/// extension), so a package name like @scope/pkg isn't one. Each is (how
/// it was written, its path).
fn imports(path: &str, home: &str) -> Vec<(String, String)> {
    let Ok(b) = fs::read(path) else {
        return Vec::new();
    };
    let s = String::from_utf8_lossy(&b);
    let mut out = Vec::new();
    let mut fenced = false;
    for l in s.split('\n') {
        let t = l.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let bare = CODE_SPAN.replace_all(l, "");
        for m in AT_REF.captures_iter(&bare) {
            let r = m[1]
                .trim_end_matches(['.', ',', ';', ':', '!', '?'])
                .to_string();
            let p = if let Some(rest) = r.strip_prefix("~/") {
                join(&[home, rest])
            } else if !r.starts_with('/') {
                join(&[&dir_of(path), &r])
            } else {
                r.clone()
            };
            if fs::metadata(&p).is_err() {
                let plain = ["./", "../", "~/", "/"].iter().any(|x| r.starts_with(x));
                if !plain && !FILE_EXT.is_match(&r) {
                    continue;
                }
            }
            out.push((r, clean(&p)));
        }
    }
    out
}

/// What a session in cwd loads from its instructions and its auto memory
/// (in proj, the project's folder under projects/), and what's worth
/// tidying.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemReport {
    pub index: MemIndex,
    pub notes: Vec<MemNote>,
    pub instr: Vec<Instruction>,
    /// The bytes loaded every session: the instructions and their
    /// imports, and what's loaded of MEMORY.md.
    pub upfront: i64,
    pub problems: Vec<MemoryProblem>,
}

impl MemReport {
    /// The note at path.
    pub fn note(&self, path: &str) -> Option<&MemNote> {
        self.notes.iter().find(|n| n.path == path)
    }
}

/// What a session in cwd loads, and what's untidy in it.
pub fn check_memory(cfg: &str, cwd: &str, proj: &str) -> MemReport {
    check_memory_in(cfg, cwd, proj, &home())
}

fn check_memory_in(cfg: &str, cwd: &str, proj: &str, home: &str) -> MemReport {
    let mut r = MemReport {
        instr: instructions_in(cfg, cwd, home),
        ..MemReport::default()
    };
    let instr: i64 = r
        .instr
        .iter()
        .filter(|i| !i.missing && !i.on_demand)
        .map(|i| i.size)
        .sum();
    if !proj.is_empty() {
        let dir = join(&[proj, "memory"]);
        r.index = read_mem_index(&join(&[&dir, "MEMORY.md"]));
        let mut paths: Vec<String> = fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.ends_with(".md"))
                    .map(|n| join(&[&dir, &n]))
                    .collect()
            })
            .unwrap_or_default();
        paths.sort();
        // 1 loaded, 2 only past the cut.
        let mut linked: HashMap<&str, u8> = HashMap::new();
        for l in &r.index.links {
            if !l.past {
                linked.insert(&l.file, 1);
            } else {
                linked.entry(&l.file).or_insert(2);
            }
        }
        for p in paths {
            if base_of(&p) == "MEMORY.md" {
                continue;
            }
            let f = front_matter(&p);
            let at = linked.get(p.as_str()).copied().unwrap_or(0);
            r.notes.push(MemNote {
                name: fm(&f, "name"),
                description: fm(&f, "description"),
                kind: fm(&f, "type"),
                indexed: at == 1,
                past_cut: at == 2,
                size: fs::metadata(&p).map_or(0, |m| m.len() as i64),
                gone: gone_files(&p, cwd, home),
                path: p,
                ..MemNote::default()
            });
        }
        same_notes(&mut r.notes);
    }
    r.upfront = instr + r.index.loaded_bytes as i64;
    r.problems = mem_problems(&r, instr);
    r
}

static FILE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"`([~./A-Za-z0-9_-][A-Za-z0-9_./@~-]*/[A-Za-z0-9_.@-]+\.[A-Za-z][A-Za-z0-9]{0,5})`")
        .unwrap()
});

/// The files a note names, in backticks, that no longer exist: paths from
/// the project's folder whose first folder is still there (so it's the
/// same tree), or from home. A cheap check: it doesn't follow renames.
fn gone_files(note: &str, cwd: &str, home: &str) -> Vec<String> {
    if cwd.is_empty() || !fs::metadata(cwd).is_ok_and(|m| m.is_dir()) {
        return Vec::new();
    }
    let Ok(b) = fs::read(note) else {
        return Vec::new();
    };
    let s = String::from_utf8_lossy(&b);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let first = |s: &str| s.split('/').next().unwrap_or("").to_string();
    for m in FILE_REF.captures_iter(&s) {
        let r = m[1].to_string();
        if !seen.insert(r.clone()) || r.contains("...") {
            continue;
        }
        let (p, top) = if let Some(rest) = r.strip_prefix("~/") {
            (join(&[home, rest]), join(&[home, &first(rest)]))
        } else if r.starts_with('/') {
            let Some(rest) = r.strip_prefix(&format!("{cwd}/")) else {
                continue;
            };
            (r.clone(), join(&[cwd, &first(rest)]))
        } else {
            let rel = r.strip_prefix("./").unwrap_or(&r);
            if rel.starts_with("../") {
                continue;
            }
            (join(&[cwd, rel]), join(&[cwd, &first(rel)]))
        };
        if top == p || !fs::metadata(&top).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        if fs::metadata(&p).is_err() {
            out.push(r);
        }
    }
    out
}

/// Marks the notes that repeat another: the same name, or descriptions
/// that share most of their words.
fn same_notes(notes: &mut [MemNote]) {
    let words: Vec<HashSet<String>> = notes.iter().map(|n| word_set(&n.description)).collect();
    for i in 0..notes.len() {
        for j in 0..i {
            let (a, b) = (&notes[i], &notes[j]);
            let mut same = !a.name.is_empty() && squash(&a.name) == squash(&b.name);
            if !same && words[i].len() >= 4 && words[j].len() >= 4 {
                same = jaccard(&words[i], &words[j]) >= 0.7;
            }
            if same {
                notes[i].same = notes[j].path.clone();
                break;
            }
        }
    }
}

fn squash(s: &str) -> String {
    s.chars()
        .filter_map(|r| match r {
            'a'..='z' | '0'..='9' => Some(r),
            'A'..='Z' => Some(r.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

fn word_set(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|r: char| !(r.is_ascii_lowercase() || r.is_ascii_digit()))
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let both = a.iter().filter(|w| b.contains(*w)).count();
    both as f64 / (a.len() + b.len() - both) as f64
}

fn stem(p: &str) -> &str {
    let b = base_of(p);
    b.strip_suffix(".md").unwrap_or(b)
}

fn names(paths: &[String]) -> String {
    let mut s = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        if i == 3 {
            s.push(format!("and {} more", paths.len() - 3));
            break;
        }
        s.push(stem(p).to_string());
    }
    s.join(", ")
}

/// What's untidy, most pressing first.
fn mem_problems(r: &MemReport, instr: i64) -> Vec<MemoryProblem> {
    let mut out = Vec::new();
    let ix = &r.index;
    let problem =
        |kind: &str, path: &str, files: Vec<String>, title: String, fix: String| MemoryProblem {
            kind: kind.into(),
            title,
            fix,
            path: path.into(),
            files,
        };
    if ix.cut() {
        let past: Vec<String> = r
            .notes
            .iter()
            .filter(|n| n.past_cut)
            .map(|n| n.path.clone())
            .collect();
        let mut fix = format!(
            "Claude never sees the {} lines past it",
            ix.lines - ix.loaded_lines
        );
        if !past.is_empty() {
            fix += &format!(
                ", so {} notes are hardly ever recalled ({})",
                past.len(),
                names(&past)
            );
        }
        out.push(problem(
            "index-cut",
            &ix.path,
            past,
            format!(
                "MEMORY.md is past what Claude loads: {} lines, {}; it reads the first {} lines or {}",
                ix.lines,
                kb(ix.bytes),
                INDEX_LINES,
                kb(INDEX_BYTES)
            ),
            fix + ". Merge or drop old lines, and keep each to a title and a short hook.",
        ));
    }
    let mut broken = Vec::new();
    let mut seen = HashSet::new();
    for l in &ix.links {
        if fs::metadata(&l.file).is_err() && seen.insert(&l.file) {
            broken.push(l.file.clone());
        }
    }
    if !broken.is_empty() {
        let fix = format!(
            "Take the lines out, or write the notes: {}.",
            names(&broken)
        );
        out.push(problem(
            "broken-link",
            &ix.path,
            broken.clone(),
            plural(
                broken.len(),
                "line of MEMORY.md points",
                "lines of MEMORY.md point",
            ) + " at a note that doesn't exist",
            fix,
        ));
    }
    let bad: Vec<&Instruction> = r
        .instr
        .iter()
        .filter(|i| i.scope == "import" && i.missing)
        .collect();
    if !bad.is_empty() {
        let what: Vec<String> = bad
            .iter()
            .map(|i| format!("@{} in {}", i.reference, base_of(&i.from)))
            .collect();
        out.push(problem(
            "broken-import",
            &bad[0].from,
            Vec::new(),
            plural(bad.len(), "@import doesn't", "@imports don't") + " lead to a file",
            format!(
                "Claude Code skips them without a word: {}. Fix the path or take it out.",
                what.join(", ")
            ),
        ));
    }
    let b = biggest(&r.instr);
    let est = est_tokens(instr);
    if est > BIG_INSTRUCTIONS || b.size > BIG_FILE {
        out.push(problem(
            "big-instructions",
            &b.path,
            Vec::new(),
            format!("≈{} tokens of CLAUDE.md and rules load every session", short(est)),
            format!(
                "Every request carries them; the biggest is {} (≈{}). Move what only some work needs into a skill or a rule with paths:, and cut the rest.",
                base_of(&b.path),
                short(est_tokens(b.size))
            ),
        ));
    }
    let (mut orphans, mut same, mut gone, mut bare, mut gone_what) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for n in &r.notes {
        if !n.indexed && !n.past_cut && ix.exists {
            orphans.push(n.path.clone());
        }
        if !n.same.is_empty() {
            same.push(format!("{} ≈ {}", stem(&n.path), stem(&n.same)));
        }
        if let Some(g) = n.gone.first() {
            gone.push(n.path.clone());
            gone_what.push(format!("{} ({g})", stem(&n.path)));
        }
        if n.description.is_empty() {
            bare.push(n.path.clone());
        }
    }
    if !orphans.is_empty() {
        let fix = format!(
            "Claude sees only the index up front, so these are found only if recall happens to pick them: {}. Add a line for each, or delete the ones you don't need.",
            names(&orphans)
        );
        out.push(problem(
            "orphan",
            &ix.path,
            orphans.clone(),
            plural(orphans.len(), "memory note isn't", "memory notes aren't") + " in MEMORY.md",
            fix,
        ));
    }
    if !same.is_empty() {
        let mut p = problem(
            "duplicate",
            &r.notes[0].path,
            Vec::new(),
            plural(same.len(), "memory note repeats", "memory notes repeat") + " another",
            format!(
                "Merge each pair into one note, and its MEMORY.md line: {}.",
                first(&same, 3).join("; ")
            ),
        );
        if let Some(n) = r.notes.iter().find(|n| !n.same.is_empty()) {
            p.path = n.path.clone();
            p.files = vec![n.path.clone(), n.same.clone()];
        }
        out.push(p);
    }
    if !gone.is_empty() {
        out.push(problem(
            "stale",
            &gone[0].clone(),
            gone.clone(),
            plural(gone.len(), "memory note names", "memory notes name") + " files that are gone",
            format!(
                "They may be out of date: {}. Check each is still true, then update or delete it.",
                first(&gone_what, 3).join(", ")
            ),
        ));
    }
    if ix.long > 0 {
        out.push(problem(
            "long-lines",
            &ix.path,
            Vec::new(),
            plural(ix.long, "line of MEMORY.md is", "lines of MEMORY.md are")
                + &format!(" over {INDEX_LINE_LEN} characters"),
            "MEMORY.md is loaded every session: keep each line a title and a short hook, and the detail in its note.".into(),
        ));
    }
    if !bare.is_empty() {
        let fix = format!(
            "Recall picks notes by their description: give each a one-line description: in its frontmatter ({}).",
            names(&bare)
        );
        out.push(problem(
            "no-description",
            &bare[0].clone(),
            bare.clone(),
            plural(bare.len(), "memory note has", "memory notes have") + " no description",
            fix,
        ));
    }
    out
}

fn biggest(ins: &[Instruction]) -> Instruction {
    let mut b = Instruction::default();
    for i in ins {
        if !i.missing && !i.on_demand && i.size > b.size {
            b = i.clone();
        }
    }
    b
}

fn first(s: &[String], n: usize) -> Vec<String> {
    if s.len() > n {
        let mut v = s[..n].to_vec();
        v.push(format!("and {} more", s.len() - n));
        return v;
    }
    s.to_vec()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn kb(n: usize) -> String {
    if n < 1000 {
        format!("{n} bytes")
    } else {
        format!("{:.1} KB", n as f64 / 1000.0)
    }
}

/// A token count in words, short.
fn short(n: i64) -> String {
    if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

// ---- What a session reads, for rush's memory screen (adapters/claude/memory.go) ----

// What Claude Code reads for a session: its auto memory, the CLAUDE.md
// files and rules, settings, and the agents, skills, commands and output
// styles on disk (plugins' are left out; an update would overwrite them).

const GROUPS: &[&str] = &[
    "Auto memory",
    "Instructions",
    "Settings",
    "Agents",
    "Skills",
    "Commands",
    "Output styles",
];

/// When each group's files reach Claude.
const WHEN: &[(&str, &str)] = &[
    (
        "Auto memory",
        "MEMORY.md every session · a note when it's recalled",
    ),
    ("Instructions", "every session"),
    ("Settings", "never sent to Claude"),
    ("Agents", "descriptions every session · the rest when used"),
    ("Skills", "descriptions every session · the rest when used"),
    ("Commands", "only when you run one"),
    ("Output styles", "only the one in use"),
];

/// What a session of the profile at cfg in cwd reads. Its auto memory is
/// beside transcript, or, for a session without one yet, in the project's
/// folder under the profile's projects.
pub fn memory(cfg: &str, cwd: &str, transcript: &str) -> Memory {
    memory_in(cfg, cwd, transcript, &home())
}

fn memory_in(cfg: &str, cwd: &str, transcript: &str, home: &str) -> Memory {
    let proj = if !transcript.is_empty() {
        dir_of(transcript)
    } else if !cwd.is_empty() {
        join(&[cfg, "projects", &project_slug(cwd)])
    } else {
        String::new()
    };
    let (files, r) = memory_files(cfg, cwd, &proj, home);
    Memory {
        files,
        groups: GROUPS.iter().map(|s| s.to_string()).collect(),
        when: WHEN
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        problems: r.problems,
    }
}

struct Files<'a> {
    out: Vec<MemoryFile>,
    seen: HashSet<String>,
    home: &'a str,
}

impl Files<'_> {
    /// Adds f when its file is there (or always), once; its index.
    fn add(&mut self, mut f: MemoryFile, always: bool) -> Option<usize> {
        if f.path.is_empty() || self.seen.contains(&f.path) {
            return None;
        }
        let st = fs::metadata(&f.path);
        if st.as_ref().map_or(true, |m| m.is_dir()) && !always {
            return None;
        }
        self.seen.insert(f.path.clone());
        f.missing = st.is_err();
        if let Ok(m) = &st {
            f.size = m.len() as i64;
            f.mod_time = m.modified().map(ms_of).unwrap_or(0);
        }
        self.out.push(f);
        Some(self.out.len() - 1)
    }

    /// p with the home folder as ~.
    fn tilde(&self, p: &str) -> String {
        tilde_in(p, self.home)
    }
}

fn tilde_in(p: &str, home: &str) -> String {
    if !home.is_empty()
        && let Some(rest) = p.strip_prefix(home)
    {
        return format!("~{rest}");
    }
    p.to_string()
}

fn exists(p: &str) -> bool {
    fs::metadata(p).is_ok()
}

fn first_non_empty(a: String, b: impl FnOnce() -> String) -> String {
    if a.is_empty() { b() } else { a }
}

/// Every .md file under dir, in order.
fn md_tree(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let _ = crate::files::walk_files(std::path::Path::new(dir), &mut |p| {
        if p.extension().is_some_and(|x| x == "md") {
            out.push(p.to_string_lossy().into_owned());
        }
        Ok(())
    });
    out
}

/// The files, grouped in GROUPS' order, with what each costs and when it's
/// loaded, and the checks they came from. The user's CLAUDE.md and
/// settings, and the project's CLAUDE.md, are listed even when they don't
/// exist yet, so they can be written.
fn memory_files(cfg: &str, cwd: &str, proj: &str, home: &str) -> (Vec<MemoryFile>, MemReport) {
    let r = check_memory_in(cfg, cwd, proj, home);
    let mut fs_ = Files {
        out: Vec::new(),
        seen: HashSet::new(),
        home,
    };
    let dirs = up_from(cwd, home);
    let name = |p: &str| -> String {
        if !cwd.is_empty() {
            if p == cwd {
                return ".".into();
            }
            if let Some(rel) = p.strip_prefix(&format!("{}/", cwd.trim_end_matches('/'))) {
                return rel.to_string();
            }
        }
        tilde_in(p, home)
    };
    let tok = est_tokens;
    let mf = |group: &str, path: &str, name: String| MemoryFile {
        group: group.into(),
        path: path.into(),
        name,
        ..MemoryFile::default()
    };

    if !proj.is_empty() {
        let ix = &r.index;
        let mut f = mf("Auto memory", &ix.path, "MEMORY.md".into());
        f.about = "the index".into();
        f.up = tok(ix.loaded_bytes as i64);
        if let Some(i) = fs_.add(f, false) {
            if ix.cut() {
                fs_.out[i].warn = format!(
                    "only its first {} of {} lines load",
                    ix.loaded_lines, ix.lines
                );
            } else if ix.long > 0 {
                fs_.out[i].warn = format!(
                    "{} of its lines are over {INDEX_LINE_LEN} characters",
                    ix.long
                );
            }
        }
        for n in &r.notes {
            let mut f = mf(
                "Auto memory",
                &n.path,
                first_non_empty(n.name.clone(), || stem(&n.path).into()),
            );
            f.about = n.description.clone();
            f.kind = n.kind.clone();
            f.later = tok(n.size);
            f.when = "on recall".into();
            let Some(i) = fs_.add(f, false) else {
                continue;
            };
            let warn = if n.past_cut {
                "its line in MEMORY.md is past what loads".to_string()
            } else if !n.indexed && ix.exists {
                "not in MEMORY.md · found only if recall picks it".into()
            } else if !n.same.is_empty() {
                format!("repeats {}", stem(&n.same))
            } else if let Some(g) = n.gone.first() {
                format!("names {g}, which is gone")
            } else if n.description.is_empty() {
                "no description to recall it by".into()
            } else {
                String::new()
            };
            fs_.out[i].warn = warn;
        }
        for l in &ix.links {
            if exists(&l.file) {
                continue;
            }
            if let Some(i) = fs_.add(mf("Auto memory", &l.file, stem(&l.file).into()), true) {
                fs_.out[i].warn =
                    format!("line {} of MEMORY.md points here · no such note", l.line);
            }
        }
    }

    // Instructions, each followed by what it imports; the project's own
    // CLAUDE.md goes after the user's, written or not.
    let about = |scope: &str| -> String {
        match scope {
            "user" => "yours · every project",
            "project" => "this project's · shared through git",
            "parent" => "a parent folder's",
            "local" => "just yours · kept out of git",
            _ => "",
        }
        .into()
    };
    let user = join(&[cfg, "CLAUDE.md"]);
    if !exists(&user) {
        let mut f = mf("Instructions", &user, fs_.tilde(&user));
        f.about = about("user");
        fs_.add(f, true);
    }
    let project_md = |fs_: &mut Files| {
        if !cwd.is_empty()
            && !exists(&join(&[cwd, "CLAUDE.md"]))
            && !exists(&join(&[cwd, ".claude", "CLAUDE.md"]))
        {
            let mut f = mf(
                "Instructions",
                &join(&[cwd, "CLAUDE.md"]),
                "CLAUDE.md".into(),
            );
            f.about = about("project");
            fs_.add(f, true);
        }
    };
    let mut placed = false;
    for ins in &r.instr {
        if !placed && ins.scope != "user" && ins.scope != "import" {
            project_md(&mut fs_);
            placed = true;
        }
        let mut f = mf("Instructions", &ins.path, name(&ins.path));
        f.about = about(&ins.scope);
        f.up = tok(ins.size);
        match ins.scope.as_str() {
            "import" => {
                f.sub = true;
                f.name = format!("@{}", ins.reference);
                f.about = format!("imported by {}", base_of(&ins.from));
                if ins.missing {
                    f.up = 0;
                    f.warn = format!("@{} doesn't lead to a file", ins.reference);
                }
            }
            "rule" => {
                f.about = first_non_empty(fm(&front_matter(&ins.path), "description"), || {
                    "rule".into()
                });
            }
            _ => {}
        }
        if ins.on_demand {
            f.up = 0;
            f.later = tok(ins.size);
            f.when = "with matching files".into();
        }
        if ins.scope == "user" {
            f.name = fs_.tilde(&ins.path);
        }
        fs_.add(f, ins.missing);
    }
    if !placed {
        project_md(&mut fs_);
    }

    let setting = |fs_: &mut Files, path: String, name: String, about: &str, always: bool| {
        let mut f = mf("Settings", &path, name);
        f.about = about.into();
        fs_.add(f, always);
    };
    let s = join(&[cfg, "settings.json"]);
    setting(
        &mut fs_,
        s.clone(),
        tilde_in(&s, home),
        "yours · every project",
        true,
    );
    let s = join(&[cfg, "settings.local.json"]);
    setting(
        &mut fs_,
        s.clone(),
        tilde_in(&s, home),
        "yours · this machine",
        false,
    );
    if !cwd.is_empty() {
        setting(
            &mut fs_,
            join(&[cwd, ".claude", "settings.json"]),
            ".claude/settings.json".into(),
            "project · shared through git",
            false,
        );
        setting(
            &mut fs_,
            join(&[cwd, ".claude", "settings.local.json"]),
            ".claude/settings.local.json".into(),
            "project · just yours",
            false,
        );
        setting(
            &mut fs_,
            join(&[cwd, ".mcp.json"]),
            ".mcp.json".into(),
            "project MCP servers",
            false,
        );
    }
    let cj = claude_json_in(cfg, home);
    setting(
        &mut fs_,
        cj.clone(),
        tilde_in(&cj, home),
        "Claude Code's own state · MCP servers, trust, per-project history",
        false,
    );

    // An agent's or skill's name and description are listed to Claude
    // every session; the rest when it's used.
    let described = |fs_: &mut Files, group: &str, p: &str, name: String, desc: String| {
        let up = tok((name.len() + desc.len()) as i64);
        let mut f = mf(group, p, name);
        f.about = desc;
        f.when = "when used".into();
        if let Some(i) = fs_.add(f, false) {
            fs_.out[i].up = up;
            fs_.out[i].later = 0.max(tok(fs_.out[i].size) - up);
        }
    };
    let mut roots: Vec<String> = dirs.iter().map(|d| join(&[d, ".claude"])).collect();
    roots.push(cfg.to_string());
    for root in &roots {
        for p in md_tree(&join(&[root, "agents"])) {
            let m = front_matter(&p);
            described(
                &mut fs_,
                "Agents",
                &p,
                first_non_empty(fm(&m, "name"), || stem(&p).into()),
                fm(&m, "description"),
            );
        }
        let skills_dir = join(&[root, "skills"]);
        let mut skills: Vec<String> = fs::read_dir(&skills_dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .map(|n| join(&[&skills_dir, &n, "SKILL.md"]))
                    .filter(|p| exists(p))
                    .collect()
            })
            .unwrap_or_default();
        skills.sort();
        for p in skills {
            let m = front_matter(&p);
            described(
                &mut fs_,
                "Skills",
                &p,
                first_non_empty(fm(&m, "name"), || base_of(&dir_of(&p)).into()),
                fm(&m, "description"),
            );
        }
        let cmds = join(&[root, "commands"]);
        for p in md_tree(&cmds) {
            let rel = p.strip_prefix(&format!("{cmds}/")).unwrap_or(&p);
            let rel = rel.strip_suffix(".md").unwrap_or(rel).replace('/', ":");
            let mut f = mf("Commands", &p, format!("/{rel}"));
            f.about = fm(&front_matter(&p), "description");
            f.when = "when run".into();
            if let Some(i) = fs_.add(f, false) {
                fs_.out[i].later = tok(fs_.out[i].size);
            }
        }
        for p in md_tree(&join(&[root, "output-styles"])) {
            let m = front_matter(&p);
            let mut f = mf(
                "Output styles",
                &p,
                first_non_empty(fm(&m, "name"), || stem(&p).into()),
            );
            f.about = fm(&m, "description");
            f.when = "if chosen".into();
            if let Some(i) = fs_.add(f, false) {
                fs_.out[i].later = tok(fs_.out[i].size);
            }
        }
    }
    // Grouped, keeping each group's own order.
    let rank = |g: &str| GROUPS.iter().position(|x| *x == g).unwrap_or(0);
    fs_.out.sort_by_key(|f| rank(&f.group));
    (fs_.out, r)
}

fn claude_json_in(cfg: &str, home: &str) -> String {
    if !home.is_empty() && cfg == join(&[home, ".claude"]) {
        return join(&[home, ".claude.json"]);
    }
    join(&[cfg, ".claude.json"])
}

/// Deletes a file, and a memory note's line in its MEMORY.md.
pub fn forget_file(path: &str) -> io::Result<()> {
    fs::remove_file(path)?;
    let dir = dir_of(path);
    let index = join(&[&dir, "MEMORY.md"]);
    if base_of(&dir) != "memory" || path == index {
        return Ok(());
    }
    let Ok(b) = fs::read_to_string(&index) else {
        return Ok(());
    };
    let link = format!("({})", base_of(path));
    let keep: Vec<&str> = b.split('\n').filter(|l| !l.contains(&link)).collect();
    fs::write(&index, keep.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &str, s: &str) {
        fs::create_dir_all(dir_of(p)).unwrap();
        fs::write(p, s).unwrap();
    }

    // memory_test.go:11 (TestCheckMemory)
    #[test]
    fn check_memory_finds_what_is_untidy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().into_owned();
        let home = root.clone();
        let cfg = join(&[&root, "cfg"]);
        let cwd = join(&[&root, "src", "app"]);
        let proj = join(&[&cfg, "projects", &project_slug(&cwd)]);
        let mem = join(&[&proj, "memory"]);
        let note = |name: &str, desc: &str, body: &str| {
            let mut f = format!("---\nname: {name}\n");
            if !desc.is_empty() {
                f += &format!("description: {desc}\n");
            }
            write(
                &join(&[&mem, &format!("{name}.md")]),
                &format!("{f}---\n\n{body}\n"),
            );
        };

        // An index past 200 lines, one line too long, one link to nothing.
        let mut index = vec![
            "- [Work on main](work-on-main.md) — no branches".to_string(),
            format!("- [Style](style.md) — {}", "long ".repeat(40)),
            "- [Gone](gone.md) — was deleted".into(),
        ];
        for i in index.len()..205 {
            index.push(format!("- filler {i}"));
        }
        index.push("- [Late](late.md) — past the cut".into());
        write(&join(&[&mem, "MEMORY.md"]), &(index.join("\n") + "\n"));
        note(
            "work-on-main",
            "no worktrees or branches, work on main directly",
            "See `internal/ui/memory.go` and `internal/ui/gone.go`, and `other/x.go`.",
        );
        note(
            "style",
            "commit style for this repository with conventional commits",
            "",
        );
        note("late", "a late one", "");
        note("stray", "", "never indexed");
        note(
            "style-2",
            "conventional commits style for this repository commit",
            "",
        );
        write(
            &join(&[&cwd, "internal", "ui", "memory.go"]),
            "package ui\n",
        );

        write(
            &join(&[&cfg, "CLAUDE.md"]),
            "@RTK.md\nUse @scope/pkg, not `@code.md`.\n```\n@fenced.md\n```\nSee @./nope.md.\n",
        );
        write(&join(&[&cfg, "RTK.md"]), "@deeper.md\n");
        write(&join(&[&cfg, "deeper.md"]), &"x".repeat(45_000));
        write(&join(&[&cwd, "CLAUDE.md"]), "# app\n");
        write(
            &join(&[&cwd, ".claude", "rules", "ts.md"]),
            "---\ndescription: |\n  TypeScript,\n  strictly\npaths:\n  - \"**/*.ts\"\n---\nstrict\n",
        );

        let r = check_memory_in(&cfg, &cwd, &proj, &home);

        let ix = &r.index;
        assert_eq!((ix.lines, ix.loaded_lines, ix.long), (206, INDEX_LINES, 1));
        assert!(ix.cut());
        let got: Vec<String> = r
            .instr
            .iter()
            .map(|i| {
                let rel = i.path.strip_prefix(&format!("{root}/")).unwrap_or(&i.path);
                format!(
                    "{} {rel} missing={} later={}",
                    i.scope, i.missing, i.on_demand
                )
            })
            .collect();
        let want = [
            "user cfg/CLAUDE.md missing=false later=false",
            "import cfg/RTK.md missing=false later=false",
            "import cfg/deeper.md missing=false later=false",
            "import cfg/nope.md missing=true later=false",
            "project src/app/CLAUDE.md missing=false later=false",
            "rule src/app/.claude/rules/ts.md missing=false later=true",
        ];
        assert_eq!(got, want);
        assert_eq!(
            fm(&front_matter(&r.instr[5].path), "description"),
            "TypeScript, strictly"
        );

        let notes: HashMap<&str, &MemNote> = r.notes.iter().map(|n| (n.name.as_str(), n)).collect();
        let w = notes["work-on-main"];
        assert!(w.indexed && w.gone == vec!["internal/ui/gone.go"], "{w:?}");
        assert!(!notes["late"].indexed && notes["late"].past_cut);
        let both = format!("{}{}", notes["style-2"].same, notes["style"].same);
        assert!(
            both.ends_with("style.md") || both.ends_with("style-2.md"),
            "{both}"
        );
        assert!(!notes["stray"].indexed && !notes["stray"].past_cut);

        let kinds: Vec<&str> = r.problems.iter().map(|p| p.kind.as_str()).collect();
        for p in &r.problems {
            assert!(
                !p.title.is_empty() && !p.fix.is_empty() && !p.path.is_empty(),
                "{p:?}"
            );
        }
        assert_eq!(
            kinds.join(" "),
            "index-cut broken-link broken-import big-instructions orphan duplicate stale long-lines no-description"
        );
        let instr: i64 = r
            .instr
            .iter()
            .filter(|i| !i.on_demand)
            .map(|i| i.size)
            .sum();
        assert_eq!(r.upfront, instr + ix.loaded_bytes as i64);
        assert!(ix.loaded_bytes < ix.bytes);

        // What the memory screen lists of it.
        let m = memory_in(&cfg, &cwd, "", &home);
        assert_eq!(m.groups.len(), 7);
        let by: HashMap<&str, &MemoryFile> = m.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let late = by[join(&[&mem, "late.md"]).as_str()];
        assert_eq!(late.warn, "its line in MEMORY.md is past what loads");
        let gone = by[join(&[&mem, "gone.md"]).as_str()];
        assert_eq!(gone.warn, "line 3 of MEMORY.md points here · no such note");
        assert!(gone.missing);
        let rule = by[join(&[&cwd, ".claude", "rules", "ts.md"]).as_str()];
        assert_eq!(
            (rule.up, rule.when.as_str(), rule.about.as_str()),
            (0, "with matching files", "TypeScript, strictly")
        );
        assert_eq!(rule.name, ".claude/rules/ts.md");
        let user = by[join(&[&cfg, "CLAUDE.md"]).as_str()];
        assert_eq!(user.name, "~/cfg/CLAUDE.md");
        let settings = by[join(&[&cfg, "settings.json"]).as_str()];
        assert!(settings.missing, "the user's settings are listed unwritten");
        let groups: Vec<&str> = m.files.iter().map(|f| f.group.as_str()).collect();
        let mut sorted = groups.clone();
        sorted.sort_by_key(|g| GROUPS.iter().position(|x| x == g));
        assert_eq!(groups, sorted, "grouped in order");
    }

    // memory_test.go:126 (TestMemIndexBytes)
    #[test]
    fn mem_index_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = join(&[&dir.path().to_string_lossy(), "MEMORY.md"]);
        let line = "y".repeat(999);
        fs::write(&p, vec![line; 30].join("\n")).unwrap();
        let ix = read_mem_index(&p);
        assert_eq!(ix.loaded_lines, 25);
        assert!(ix.loaded_bytes <= INDEX_BYTES && ix.cut());
    }

    #[test]
    fn forget_file_takes_its_line_out() {
        let dir = tempfile::tempdir().unwrap();
        let mem = join(&[&dir.path().to_string_lossy(), "memory"]);
        write(&join(&[&mem, "a.md"]), "x");
        write(
            &join(&[&mem, "MEMORY.md"]),
            "- [A](a.md) — a\n- [B](b.md) — b\n",
        );
        forget_file(&join(&[&mem, "a.md"])).unwrap();
        assert!(!exists(&join(&[&mem, "a.md"])));
        assert_eq!(
            fs::read_to_string(join(&[&mem, "MEMORY.md"])).unwrap(),
            "- [B](b.md) — b\n"
        );
    }
}
