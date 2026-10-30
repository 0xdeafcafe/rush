//! The memory review: a project's notes worth a look, one at a time, each
//! with why (untidy, or about work that may have moved on), its text, and
//! what to do with it: keep it as it is, edit it, merge it into another,
//! move it into CLAUDE.md for every project, or forget it. Go:
//! internal/ui/memreview.go (memReview :22, memReviewItem :30,
//! readMemReview :38, openMemReview :68, memReviewSheet :87, width :98,
//! cur :100, targets :108, body :119, key :182, merge :271, do :282,
//! next :297, back :305, gone :315, dropMemCache :321). A note's repeat
//! is the agent's (f.Same, :56), so m merges straight into it; e opens
//! the note in $EDITOR and counts it edited once that's done (:221-235).
//!
//! The agent's side of keeping notes (agent.MemoryKeeper) is whoever
//! registers one: the binary with [`register_keeper`] (rush-claude's
//! NoteKeeper), a test with [`set_keeper`]; with none, m and g do
//! nothing, as Go's do. $EDITOR has the terminal through
//! session_core::memedit::edit_then (the frame's frame::outside).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rush_core::adapter::{MemoryFile, Profile, Registry};
use rush_core::text::Line;
use rush_memory::Keeper;

use super::data::Data;
use super::memory::mem_type_colour;
use super::rows::ln;
use crate::frame::confirm::Confirm;
use crate::frame::sheet::{Sheet, sheet_row, sheet_title};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::session::harness_name;
use crate::style::*;

/// The agents whose memory notes rush can write, by kind.
#[derive(Default)]
pub struct MemKeepers(HashMap<String, Arc<dyn Keeper>>);

/// Makes k the keeper of agent kind's memory notes.
pub fn set_keeper(m: &mut Model, kind: &str, k: Arc<dyn Keeper>) {
    m.kept::<MemKeepers>().0.insert(kind.to_string(), k);
}

/// The keepers the binary registers as it starts, for every Model
/// (Go's adapters registering themselves as agent.MemoryKeeper).
static KEEPERS: std::sync::Mutex<Vec<(String, Arc<dyn Keeper>)>> =
    std::sync::Mutex::new(Vec::new());

/// Makes k the keeper of agent kind's memory notes in every Model, as the
/// binary starts.
pub fn register_keeper(kind: &str, k: Arc<dyn Keeper>) {
    let mut ks = KEEPERS.lock().unwrap_or_else(|e| e.into_inner());
    ks.retain(|(n, _)| n != kind);
    ks.push((kind.to_string(), k));
}

/// The keeper of agent kind's memory notes: the Model's own, else the
/// one the binary registered.
pub(crate) fn keeper(m: &mut Model, kind: &str) -> Option<Arc<dyn Keeper>> {
    let kind = rush_core::adapter::migrated(kind);
    if let Some(k) = m.kept::<MemKeepers>().0.get(kind.as_str()) {
        return Some(k.clone());
    }
    let ks = KEEPERS.lock().unwrap_or_else(|e| e.into_inner());
    ks.iter()
        .find(|(n, _)| *n == kind.as_str())
        .map(|(_, k)| k.clone())
}

/// A project's notes to review, read off the UI (memReview).
#[derive(Clone, Default)]
pub(crate) struct MemReview {
    pub p: Profile,
    pub title: String,
    pub items: Vec<MemReviewItem>,
    /// Every note, to merge into.
    pub notes: Vec<MemoryFile>,
}

#[derive(Clone, Default)]
pub(crate) struct MemReviewItem {
    pub f: MemoryFile,
    pub why: String,
    pub text: String,
    /// The note it repeats, to merge into.
    pub same: String,
}

/// Reads the notes of p's project in cwd worth a look (readMemReview). It
/// reads the disk.
pub(crate) fn read_mem_review(
    reg: &Registry,
    mk: Option<&dyn Keeper>,
    p: &Profile,
    cwd: &str,
    title: &str,
) -> Result<MemReview, String> {
    let ad = reg.get(p.kind.as_str());
    let Some(mr) = ad.as_ref().and_then(|ad| ad.memory_reader()) else {
        return Err(format!(
            "{} keeps no memory rush can read",
            harness_name(reg, &p.kind)
        ));
    };
    let mem = mr.memory(p, cwd, "");
    let mut r = MemReview {
        p: p.clone(),
        title: title.to_string(),
        ..MemReview::default()
    };
    let now = rush_core::time::now();
    for f in mem.files {
        if f.group != "Auto memory" || f.name == "MEMORY.md" || f.missing {
            continue;
        }
        r.notes.push(f.clone());
        let why = rush_memory::why(&f, now);
        if why.is_empty() {
            continue;
        }
        let text = mk
            .and_then(|k| k.read_note(&f.path).ok())
            .map(|n| n.body)
            .unwrap_or_default();
        let same = f.same.clone();
        r.items.push(MemReviewItem { f, why, text, same });
    }
    Ok(r)
}

/// Reads p's project in cwd and opens its review (openMemReview).
pub(crate) fn open_mem_review(m: &mut Model, p: Profile, cwd: &str, title: &str) {
    m.flash(&format!("reading {title}'s memory…"));
    let reg = m.reg.clone();
    let mk = keeper(m, p.kind.as_str());
    let (cwd, title) = (cwd.to_string(), title.to_string());
    m.later(
        {
            let mk = mk.clone();
            let title = title.clone();
            move || read_mem_review(&reg, mk.as_deref(), &p, &cwd, &title)
        },
        move |m, r| {
            match r {
                Err(e) => m.fail(&e),
                Ok(r) if r.items.is_empty() => m.flash(&format!(
                    "nothing in {title}'s memory to review · {} notes",
                    r.notes.len()
                )),
                Ok(r) if m.sheet.is_some() => m.flash(&format!(
                    "{} of {title}'s notes worth a look · close this to review them",
                    r.items.len()
                )),
                Ok(r) => m.open_sheet(Box::new(MemReviewSheet::new(r, mk))),
            }
            Out::Draw
        },
    );
}

/// Goes through a review's notes, one at a time (memReviewSheet).
pub(crate) struct MemReviewSheet {
    pub r: MemReview,
    keeper: Option<Arc<dyn Keeper>>,
    pub i: usize,
    /// What was done to each, by path.
    pub done: HashMap<String, String>,
    /// Picking the note to merge the one under review into.
    pub merging: bool,
    pub pick: usize,
    pub busy: bool,
    pub err: String,
}

/// What the review does to a note, off the UI.
type Work = Box<dyn FnOnce() -> Result<(), String> + Send>;

impl MemReviewSheet {
    pub fn new(r: MemReview, keeper: Option<Arc<dyn Keeper>>) -> MemReviewSheet {
        MemReviewSheet {
            r,
            keeper,
            i: 0,
            done: HashMap::new(),
            merging: false,
            pick: 0,
            busy: false,
            err: String::new(),
        }
    }

    fn cur(&self) -> Option<&MemReviewItem> {
        self.r.items.get(self.i)
    }

    /// The notes the one under review can merge into.
    fn targets(&self) -> Vec<MemoryFile> {
        let Some(it) = self.cur() else {
            return Vec::new();
        };
        self.r
            .notes
            .iter()
            .filter(|f| f.path != it.f.path && !self.gone(&f.path))
            .cloned()
            .collect()
    }

    fn next(&mut self) {
        self.i += 1;
        while self.i < self.r.items.len() && self.gone(&self.r.items[self.i].f.path) {
            self.i += 1;
        }
    }

    /// Goes to the note before that's still there, if there is one.
    fn back(&mut self) {
        for j in (0..self.i).rev() {
            if !self.gone(&self.r.items[j].f.path) {
                self.i = j;
                return;
            }
        }
    }

    /// Whether the review forgot, merged or moved the note at path.
    fn gone(&self, path: &str) -> bool {
        matches!(
            self.done.get(path).map(String::as_str),
            Some("forgotten" | "merged" | "moved to CLAUDE.md")
        )
    }

    /// What f did to the note at path, once it's done: on to the next.
    fn landed(&mut self, path: &str, what: &str, r: Result<(), String>) {
        self.busy = false;
        if let Err(e) = r {
            self.err = e;
            return;
        }
        self.done.insert(path.to_string(), what.to_string());
        self.next();
    }

    /// Runs f on the note under review off the UI, then moves on (do).
    fn run(&mut self, m: &mut Model, what: &'static str, f: Work) -> Out {
        let Some(path) = self.cur().map(|it| it.f.path.clone()) else {
            return Out::Idle;
        };
        (self.busy, self.err) = (true, String::new());
        if m.sender().is_none() {
            // No loop: done at once, while this sheet has the key.
            let r = f();
            self.landed(&path, what, r);
            return Out::Draw;
        }
        m.later(f, move |m, r| land(m, &path, what, r));
        Out::Draw
    }

    /// Merges the note under review into the one at into (merge).
    fn merge(&mut self, m: &mut Model, into: String) -> Out {
        let Some(mk) = self.keeper.clone() else {
            return Out::Idle;
        };
        let Some(from) = self.cur().map(|it| it.f.path.clone()) else {
            return Out::Idle;
        };
        self.merging = false;
        self.run(m, "merged", Box::new(move || mk.merge_notes(&into, &from)))
    }
}

/// What a run did, to the review if it's still open.
fn land(m: &mut Model, path: &str, what: &str, r: Result<(), String>) -> Out {
    match m.sheet_as::<MemReviewSheet>() {
        Some(s) => {
            s.landed(path, what, r);
            Out::Draw
        }
        None => Out::Idle,
    }
}

/// A run asked for by a question: the review is open under it.
fn after_question(m: &mut Model, what: &'static str, f: Work) -> Out {
    let Some(s) = m.sheet_as::<MemReviewSheet>() else {
        return Out::Idle;
    };
    let Some(path) = s.cur().map(|it| it.f.path.clone()) else {
        return Out::Idle;
    };
    (s.busy, s.err) = (true, String::new());
    m.later(f, move |m, r| land(m, &path, what, r));
    Out::Draw
}

/// Has the memory view and Projects read memory again (dropMemCache).
pub(crate) fn drop_mem_cache(m: &mut Model) {
    if let Some(key) = crate::screens::session_core::open_key(m) {
        crate::screens::session_core::memory::forget(m, &key);
    }
    m.kept::<Data>().mem_counts.clear();
}

/// The note at path edited in $EDITOR: counted, and on to the next; or
/// why it couldn't be, on the review if it's still open.
fn edited(m: &mut Model, path: &str, r: Result<(), String>) -> Out {
    let Some(s) = m.sheet_as::<MemReviewSheet>() else {
        return Out::Idle;
    };
    match r {
        Err(e) => s.err = e,
        Ok(()) => {
            s.done.insert(path.to_string(), "edited".into());
            s.next();
        }
    }
    Out::Draw
}

impl Sheet for MemReviewSheet {
    fn width(&self, _m: &Model) -> usize {
        100
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let Some(it) = self.cur() else {
            let mut out = vec![
                sheet_title("Memory review", &format!("{} · done", self.r.title), w),
                Line::new(),
            ];
            for d in [
                "kept",
                "edited",
                "merged",
                "moved to CLAUDE.md",
                "forgotten",
            ] {
                let n = self.done.values().filter(|v| *v == d).count();
                if n > 0 {
                    out.push(ln([plain("  "), sp(format!("{n} {d}"), TEXT)]));
                }
            }
            if self.done.is_empty() {
                out.push(ln([plain("  "), sp("nothing changed", DIM)]));
            }
            out.push(Line::new());
            out.push(keys_fit(w, &[("esc", "close")]));
            return out;
        };
        let mut out = vec![
            sheet_title(
                "Memory review",
                &format!(
                    "{} · {} of {}",
                    self.r.title,
                    self.i + 1,
                    self.r.items.len()
                ),
                w,
            ),
            Line::new(),
        ];
        let f = &it.f;
        let about = if f.about.is_empty() {
            &f.name
        } else {
            &f.about
        };
        for l in wrap(about, w.saturating_sub(4)) {
            out.push(ln([plain("  "), spb(l, TEXT)]));
        }
        let kind = if f.kind.is_empty() {
            "no type"
        } else {
            &f.kind
        };
        let mut meta = ln([
            plain("  "),
            sp(kind, mem_type_colour(&f.kind)),
            sp(format!(" · {}", f.name), DIM),
        ]);
        if f.reads > 0 {
            meta.spans.push(sp(
                format!(
                    " · opened {}× · last {}",
                    f.reads,
                    rush_memory::day_month(f.last_read)
                ),
                DIM,
            ));
        }
        if f.mod_time != 0 {
            meta.spans.push(sp(
                format!(
                    " · changed {} ago",
                    dur(rush_core::time::now() - f.mod_time)
                ),
                DIM,
            ));
        }
        out.push(meta);
        out.push(ln([plain("  "), sp(format!("! {}", it.why), YELLOW)]));
        out.push(Line::new());
        if self.merging {
            out.push(ln([
                plain("  "),
                sp("Merge it into which note?", TEXT),
                sp(" its text goes below that note's, and this one goes", DIM),
            ]));
            out.push(Line::new());
            let ts = self.targets();
            let top = (self.pick as isize - (h as isize - 14) / 2)
                .min(ts.len() as isize - (h as isize - 12))
                .max(0) as usize;
            for (j, t) in ts.iter().enumerate().skip(top) {
                if out.len() >= h.saturating_sub(3) {
                    break;
                }
                let about = if t.about.is_empty() {
                    &t.name
                } else {
                    &t.about
                };
                let l = ln([sp(about, TEXT), plain("  "), sp(&t.name, DIM)]);
                out.push(sheet_row(l, j == self.pick, w));
            }
            out.push(Line::new());
            out.push(keys_fit(
                w,
                &[("↑↓", "pick"), ("enter", "merge"), ("esc", "back")],
            ));
            return out;
        }
        let mut text = wrap(&it.text, w.saturating_sub(6));
        let room = 3.max(h as isize - out.len() as isize - 4) as usize;
        if text.len() > room {
            text.truncate(room - 1);
            text.push("…".into());
        }
        for l in text {
            out.push(ln([plain("    "), sp(l, DIM)]));
        }
        if !self.err.is_empty() {
            out.push(Line::new());
            out.push(ln([plain("  "), sp(&self.err, RED)]));
        }
        out.push(Line::new());
        if self.busy {
            out.push(ln([sp("⋯", YELLOW)]));
            return out;
        }
        let merge = if !it.same.is_empty() && !self.gone(&it.same) {
            let base = Path::new(&it.same)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("merge into {}", base.strip_suffix(".md").unwrap_or(&base))
        } else {
            "merge".into()
        };
        out.push(keys_fit(
            w,
            &[
                ("k", "keep"),
                ("e", "edit"),
                ("m", &merge),
                ("g", "to CLAUDE.md"),
                ("x", "forget"),
                ("→", "skip"),
                ("esc", "stop"),
            ],
        ));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if self.busy {
            return Out::Idle;
        }
        let s = k.s.as_str();
        let Some(it) = self.cur().cloned() else {
            return match s {
                "esc" | "enter" | "q" | "ctrl+c" => Out::Close,
                _ => Out::Idle,
            };
        };
        if self.merging {
            let ts = self.targets();
            match s {
                "up" | "k" => self.pick = self.pick.saturating_sub(1),
                "down" | "j" => self.pick = (self.pick + 1).min(ts.len().saturating_sub(1)),
                "enter" => {
                    if let Some(t) = ts.get(self.pick) {
                        return self.merge(m, t.path.clone());
                    }
                    return Out::Idle;
                }
                "esc" => self.merging = false,
                _ => return Out::Idle,
            }
            return Out::Draw;
        }
        let path = it.f.path.clone();
        match s {
            "esc" | "q" | "ctrl+c" => {
                drop_mem_cache(m);
                return Out::Close;
            }
            "right" | "n" | "tab" | "down" => self.next(),
            "left" | "shift+tab" | "up" => self.back(),
            "k" | "enter" => {
                return self.run(m, "kept", Box::new(move || rush_memory::keep(&path)));
            }
            "e" => {
                let p = path.clone();
                return crate::screens::session_core::memedit::edit_then(
                    m,
                    &path,
                    Box::new(move |m, r| edited(m, &p, r)),
                );
            }
            "m" => {
                if self.keeper.is_none() {
                    return Out::Idle;
                }
                if !it.same.is_empty() && !self.gone(&it.same) {
                    return self.merge(m, it.same.clone());
                }
                if self.targets().is_empty() {
                    m.flash("no other note to merge it into");
                    return Out::Draw;
                }
                (self.merging, self.pick) = (true, 0);
            }
            "g" => {
                let Some(mk) = self.keeper.clone() else {
                    return Out::Idle;
                };
                let p = self.r.p.clone();
                m.ask(
                    Confirm::new("Move it into your CLAUDE.md?", move |m| {
                        after_question(
                            m,
                            "moved to CLAUDE.md",
                            Box::new(move || mk.make_global(&p, &path)),
                        )
                    })
                    .detail(
                        "every session in every project loads it then, not just this project's · the note goes",
                    ),
                );
            }
            "x" | "delete" => {
                let reg = m.reg.clone();
                let kind = self.r.p.kind.clone();
                m.ask(
                    Confirm::new(format!("Forget {}?", it.f.name), move |m| {
                        after_question(
                            m,
                            "forgotten",
                            Box::new(move || {
                                let ad = reg.get(kind.as_str());
                                match ad.as_ref().and_then(|ad| ad.memory_reader()) {
                                    Some(mr) => mr.forget_file(&path).map_err(|e| e.to_string()),
                                    None => std::fs::remove_file(&path).map_err(|e| e.to_string()),
                                }
                            }),
                        )
                    })
                    .detail("its file is deleted, and its line in MEMORY.md"),
                );
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A keeper that merges by appending, as Claude Code's does, and says
    /// what it was asked.
    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<String>>,
    }

    impl Keeper for Fake {
        fn remember(
            &self,
            _: &Profile,
            _: &str,
            _: &rush_memory::MemoryNote,
        ) -> rush_memory::Result<String> {
            Err("no".into())
        }
        fn tidy_memory(&self, _: &str) -> rush_memory::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn merge_notes(&self, keep: &str, drop: &str) -> rush_memory::Result<()> {
            let add = std::fs::read_to_string(drop).map_err(|e| e.to_string())?;
            let mut to = std::fs::read_to_string(keep).map_err(|e| e.to_string())?;
            to += &add;
            std::fs::write(keep, to).map_err(|e| e.to_string())?;
            self.asked.lock().unwrap().push(format!("merge {drop}"));
            std::fs::remove_file(drop).map_err(|e| e.to_string())
        }
        fn make_global(&self, _: &Profile, path: &str) -> rush_memory::Result<()> {
            self.asked.lock().unwrap().push(format!("global {path}"));
            Ok(())
        }
        fn read_note(&self, _: &str) -> rush_memory::Result<rush_memory::MemoryNote> {
            Err("no".into())
        }
        fn count_opens(
            &self,
            _: &Profile,
            _: &rush_core::adapter::Memory,
        ) -> HashMap<String, rush_memory::Opens> {
            HashMap::new()
        }
    }

    fn note(dir: &Path, name: &str, kind: &str, body: &str) -> MemoryFile {
        let path = dir.join(format!("{name}.md"));
        std::fs::write(&path, body).unwrap();
        MemoryFile {
            group: "Auto memory".into(),
            path: path.to_string_lossy().into_owned(),
            name: name.into(),
            kind: kind.into(),
            ..MemoryFile::default()
        }
    }

    /// Two notes worth a look: one repeats another, one is old.
    fn review(dir: &Path) -> MemReview {
        let tests = note(dir, "tests", "feedback", "Targeted tests.");
        let again = note(dir, "tests-again", "feedback", "Again.");
        let launch = note(dir, "launch", "project", "Launch Friday.");
        let items = vec![
            MemReviewItem {
                f: again.clone(),
                why: "repeats tests".into(),
                text: "Again.".into(),
                same: tests.path.clone(),
            },
            MemReviewItem {
                f: launch.clone(),
                why: "unchanged since 2 Jan · is it still true?".into(),
                text: "Launch Friday.".into(),
                same: String::new(),
            },
        ];
        MemReview {
            p: Profile::default(),
            title: "app".into(),
            items,
            notes: vec![tests, again, launch],
        }
    }

    fn press(s: &mut MemReviewSheet, m: &mut Model, k: &str) -> Out {
        s.key(m, &Press::of(k).unwrap())
    }

    fn body(s: &mut MemReviewSheet, m: &Model) -> String {
        s.body(m, 100, 30)
            .iter()
            .map(|l| l.plain())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The repeat merges straight into the note it repeats; a kept note
    /// isn't asked about again until it changes (TestMemReview).
    #[test]
    fn mem_review() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Model::new("t");
        let fake = Arc::new(Fake::default());
        let mut s = MemReviewSheet::new(review(dir.path()), Some(fake.clone()));
        let b = body(&mut s, &m);
        for want in [
            "1 of 2",
            "m merge into tests",
            "g to CLAUDE.md",
            "! repeats tests",
        ] {
            assert!(b.contains(want), "sheet lacks {want:?}:\n{b}");
        }
        let first = s.cur().unwrap().f.path.clone();
        press(&mut s, &mut m, "m");
        assert!(
            !Path::new(&first).exists(),
            "the repeat should be merged away"
        );
        let kept = std::fs::read_to_string(dir.path().join("tests.md")).unwrap();
        assert!(
            kept.contains("Targeted tests.") && kept.contains("Again."),
            "{kept}"
        );
        let launch = s.cur().unwrap().f.clone();
        press(&mut s, &mut m, "k");
        assert!(s.cur().is_none(), "review should be done: {:?}", s.done);
        let b = body(&mut s, &m);
        assert!(b.contains("1 kept") && b.contains("1 merged"), "{b}");
        let mut f = launch;
        f.mod_time = std::fs::metadata(&f.path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(rush_memory::is_kept(&f.path, f.mod_time));
        assert_eq!(press(&mut s, &mut m, "esc"), Out::Close);
    }

    /// Merging with nothing else left to merge into says so, and never
    /// picks past the list; back steps over notes the review already
    /// forgot (TestMemReviewEdges).
    #[test]
    fn mem_review_edges() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Model::new("t");
        let mut s = MemReviewSheet::new(review(dir.path()), Some(Arc::new(Fake::default())));
        s.r.notes = vec![s.cur().unwrap().f.clone()];
        s.r.items[0].same.clear();
        press(&mut s, &mut m, "m");
        assert!(!s.merging, "no picker with nothing to merge into");
        s.merging = true;
        press(&mut s, &mut m, "down");
        assert_eq!(press(&mut s, &mut m, "enter"), Out::Idle);
        assert_eq!(s.pick, 0);
        s.merging = false;
        let gone = s.r.items[0].f.path.clone();
        s.done.insert(gone, "forgotten".into());
        s.i = 1;
        press(&mut s, &mut m, "left");
        assert_eq!(s.i, 1, "back landed on a forgotten note");
    }

    /// The repeat the agent found is the one m merges into (f.Same).
    #[test]
    fn same_is_the_agents() {
        let dir = tempfile::tempdir().unwrap();
        let r = review(dir.path());
        let mut f = r.items[0].f.clone();
        f.same = r.notes[0].path.clone();
        f.reads = 4;
        f.last_read = 1;
        let mut s = MemReviewSheet::new(
            MemReview {
                items: vec![MemReviewItem {
                    same: f.same.clone(),
                    f,
                    why: "repeats tests".into(),
                    text: String::new(),
                }],
                ..r
            },
            Some(Arc::new(Fake::default())),
        );
        let m = Model::new("t");
        let b = body(&mut s, &m);
        assert!(b.contains("m merge into tests"), "{b}");
        assert!(
            b.contains("feedback · tests-again · opened 4× · last "),
            "{b}"
        );
    }

    /// e opens the note in $EDITOR; once that's done it's counted edited
    /// and the review moves on (memreview.go:221).
    #[test]
    fn e_edits_it_in_the_editor() {
        use crate::screens::session_core::memedit::{Done, set_editor};
        let dir = tempfile::tempdir().unwrap();
        let mut m = Model::new("t");
        type Slot = Arc<Mutex<Option<(String, Done)>>>;
        let slot: Slot = Arc::new(Mutex::new(None));
        let s2 = slot.clone();
        set_editor(
            &mut m,
            Arc::new(move |_, path: &str, done| {
                *s2.lock().unwrap() = Some((path.to_string(), done));
                Out::Draw
            }),
        );
        let r = review(dir.path());
        let first = r.items[0].f.path.clone();
        m.open_sheet(Box::new(MemReviewSheet::new(r, None)));
        m.sheet_key(&Press::of("e").unwrap());
        let (path, done) = slot.lock().unwrap().take().expect("the editor ran");
        assert_eq!(path, first);
        assert_eq!(done(&mut m, Ok(())), Out::Draw);
        let s = m.sheet_as::<MemReviewSheet>().unwrap();
        assert_eq!(s.done.get(&first).map(String::as_str), Some("edited"));
        assert_eq!(s.i, 1, "on to the next");
    }

    /// e hands the terminal to the editor for real: a fake one, run once
    /// the key has been handled, edits the note, and the review counts it.
    #[test]
    fn e_runs_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Model::new("t");
        m.set_editor_program("f() { echo edited >> \"$1\"; }; f");
        let r = review(dir.path());
        let first = r.items[0].f.path.clone();
        m.open_sheet(Box::new(MemReviewSheet::new(r, None)));
        m.sheet_key(&Press::of("e").unwrap());
        assert_eq!(m.run_outside(), 1);
        let text = std::fs::read_to_string(&first).unwrap();
        assert!(text.ends_with("edited\n"), "{text}");
        let s = m.sheet_as::<MemReviewSheet>().unwrap();
        assert_eq!(s.done.get(&first).map(String::as_str), Some("edited"));
    }

    /// Without a keeper, merging and moving do nothing.
    #[test]
    fn mem_review_without_a_keeper() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Model::new("t");
        let mut s = MemReviewSheet::new(review(dir.path()), None);
        assert_eq!(press(&mut s, &mut m, "m"), Out::Idle);
        assert_eq!(press(&mut s, &mut m, "g"), Out::Idle);
        assert!(!s.merging && m.confirm.is_none());
    }
}
