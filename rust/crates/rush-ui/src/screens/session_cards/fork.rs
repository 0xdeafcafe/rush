//! /fork: sets a fork up before it starts: its name, how much of the
//! conversation it keeps, model, effort, permissions, folder (this one or
//! a new worktree) and a first message. After Go's
//! internal/ui/forksheet.go (forkSheet :24, openFork :105, openForkAt
//! :148, cycle :160, key :180, current :213, body :223, firstNote :311,
//! worktreeName :339, start :352, cutAfter :425). Starting the host
//! (host.Spawn, with the dispatch defaults for "as now") is the session's:
//! [`Act::Fork`] hands it a [`Fork`].

use std::path::Path;

use rush_core::adapter::{AdapterRef, Profile};
use rush_core::text::{Color, Line, Span};
use rush_core::time::Ms;

use super::rewind::{new_session_id, transcript_of, turn_starts};
use super::util::{first_non_empty, join as path_join};
use super::{Act, act, info};
use crate::frame::editor::edit;
use crate::frame::sheet::{sheet_row, sheet_title, text_field};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// A fork to start, its conversation copied already when it's part of
/// one or in another folder: the host starts session_id in cwd, as the
/// agent's own fork of it (Claude Code's --fork-session) when fork.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fork {
    pub name: String,
    pub cwd: String,
    pub acct: Profile,
    pub session_id: String,
    pub fork: bool,
    /// The session it was forked from.
    pub from: String,
    /// "" for as now: the host fills in what the session runs with, or
    /// was started with, or the dispatch default.
    pub model: String,
    pub effort: String,
    pub perm: String,
    /// What to tell it first.
    pub prompt: String,
}

/// One of the conversation's turns, as the sheet offers it (forkTurn).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForkTurn {
    pub n: u32,
    pub prompt: String,
    pub end: Ms,
}

/// The sheet's rows.
pub const NAME: usize = 0;
pub const FROM: usize = 1;
pub const MODEL: usize = 2;
pub const EFFORT: usize = 3;
pub const PERM: usize = 4;
pub const FOLDER: usize = 5;
pub const FIRST: usize = 6;
pub const GO: usize = 7;
const ROWS: usize = 8;

/// The /fork sheet (forkSheet).
pub struct ForkSheet {
    pub conn: String,
    pub agent: String,
    pub prof: Profile,
    pub kind: String,
    /// Copies the conversation for a fork of part of it, or into a
    /// worktree; None forks only the whole of it, in the same folder.
    pub br: Option<AdapterRef>,
    pub sid: String,
    pub cwd: String,
    pub path: String,
    /// Newest first: "all of it", then up to each earlier turn.
    pub turns: Vec<ForkTurn>,
    /// The git checkout, when there is one.
    pub repo: String,
    pub row: usize,
    pub name: Vec<char>,
    pub first: Vec<char>,
    pub name_pos: usize,
    pub first_pos: usize,
    /// Index into turns; 0 keeps them all.
    pub up_to: usize,
    pub model: usize,
    pub effort: usize,
    pub perm: usize,
    pub worktree: bool,
    /// The agent's choices, "" (as now) first.
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub perms: Vec<String>,
    /// What it runs with now.
    pub now_model: String,
    pub now_effort: String,
    pub now_perm: String,
    pub cache_warm: Ms,
    /// The repo is looked up.
    pub looked: bool,
}

/// What the background lookup found, when it landed with the sheet out
/// of the model.
#[derive(Default)]
struct Looked(Option<(String, String)>);

fn choice_ids(cs: &[rush_core::adapter::Choice]) -> Vec<String> {
    std::iter::once(String::new())
        .chain(cs.iter().map(|c| c.id.clone()))
        .collect()
}

/// Opens /fork on agent key's session, name prefilled from name
/// (openFork).
pub fn open_fork(m: &mut Model, key: &str, name: &str) -> Out {
    let Some(inf) = info(m, key) else {
        m.flash_err("/fork works on a session rush runs: open one first");
        return Out::Draw;
    };
    let turns: Vec<ForkTurn> = super::session(m, key)
        .map(|s| {
            s.turns
                .iter()
                .rev()
                .map(|t| ForkTurn {
                    n: t.n,
                    prompt: t.prompt.clone(),
                    end: t.end,
                })
                .collect()
        })
        .unwrap_or_default();
    if inf.session_id.is_empty() || turns.is_empty() {
        m.flash_err(&format!(
            "nothing to fork yet: {} hasn't had a turn",
            inf.name
        ));
        return Out::Draw;
    }
    let br = m.reg.get(&inf.kind).filter(|a| a.brancher().is_some());
    let ch = m.reg.choices_of(&inf.kind).unwrap_or_default();
    let mut path = first_non_empty(&[&inf.path, &transcript_of(m, key)]).to_string();
    if path.is_empty()
        && let Some(b) = br.as_ref().and_then(|a| a.brancher())
    {
        path = b.transcript_path(&inf.acct, &inf.cwd, &inf.session_id);
    }
    let name: Vec<char> = first_non_empty(&[name, &format!("{} (fork)", inf.name)])
        .chars()
        .collect();
    let mut f = ForkSheet {
        conn: key.into(),
        agent: inf.name.clone(),
        prof: inf.acct.clone(),
        kind: inf.kind.clone(),
        sid: inf.session_id.clone(),
        cwd: inf.cwd.clone(),
        path,
        turns,
        repo: String::new(),
        row: NAME,
        name_pos: name.len(),
        name,
        first: Vec::new(),
        first_pos: 0,
        up_to: 0,
        model: 0,
        effort: 0,
        perm: 0,
        worktree: false,
        models: choice_ids(&ch.models),
        efforts: choice_ids(&ch.efforts),
        perms: choice_ids(&ch.modes),
        now_model: inf.model.clone(),
        now_effort: inf.effort.clone(),
        now_perm: inf.permission_mode.clone(),
        cache_warm: inf.cache_warm,
        looked: false,
        br,
    };
    if f.br.is_none() {
        f.turns.truncate(1); // only the whole of it
    }
    m.open_sheet(Box::new(f));
    // Whether it's in a git checkout is looked up in the background; the
    // sheet fills it in when it's in.
    let (cwd, k) = (inf.cwd.clone(), key.to_string());
    m.later(
        move || rush_sys::actions::repo_root(&cwd),
        move |m, repo| {
            match m.sheet_as::<ForkSheet>().filter(|f| f.conn == k) {
                Some(f) => {
                    f.repo = repo;
                    f.looked = true;
                }
                None => m.kept::<Looked>().0 = Some((k, repo)),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// alt+f on a turn in the history: /fork, remembering up to and
/// including turn n (openForkAt).
pub fn open_fork_at(m: &mut Model, key: &str, n: u32) -> Out {
    let out = open_fork(m, key, "");
    if let Some(f) = m.sheet_as::<ForkSheet>()
        && let Some(i) = f.turns.iter().position(|t| t.n == n)
    {
        f.up_to = i;
        f.row = FROM;
    }
    out
}

impl ForkSheet {
    fn poll(&mut self, m: &mut Model) {
        if let Some((k, repo)) = m.kept::<Looked>().0.take() {
            if k == self.conn {
                self.repo = repo;
                self.looked = true;
            } else {
                m.kept::<Looked>().0 = Some((k, repo));
            }
        }
    }

    fn cycle(&mut self, d: isize) {
        let step =
            |i: usize, n: usize| ((i as isize + d + n as isize) % n.max(1) as isize) as usize;
        match self.row {
            FROM => self.up_to = step(self.up_to, self.turns.len()),
            MODEL => self.model = step(self.model, self.models.len()),
            EFFORT => self.effort = step(self.effort, self.efforts.len()),
            PERM => self.perm = step(self.perm, self.perms.len()),
            FOLDER => {
                if !self.repo.is_empty() && self.br.is_some() {
                    self.worktree = !self.worktree;
                }
            }
            _ => {}
        }
    }

    /// What the session runs with now, for "as now" (current).
    fn current(&self, what: &str) -> &str {
        match what {
            "model" => &self.now_model,
            "effort" => &self.now_effort,
            _ => &self.now_perm,
        }
    }

    /// Whether the fork's first message is added after the prompt cache
    /// it shares with this conversation, so what it remembers is read at
    /// about a tenth of the price, or breaks it (firstNote).
    pub fn first_note(&self, now: Ms) -> Line {
        let md = &self.models[self.model];
        if !md.is_empty()
            && !self
                .current("model")
                .to_lowercase()
                .contains(md.trim_end_matches("[1m]"))
        {
            return line([sp(
                "↳ breaks the cache: another model reads all it remembers at full price",
                YELLOW,
            )]);
        }
        let e = &self.efforts[self.effort];
        if !e.is_empty() && !e.eq_ignore_ascii_case(self.current("effort")) {
            return line([sp(
                "↳ breaks the cache: another effort reads all it remembers at full price",
                YELLOW,
            )]);
        }
        let mut warm = self.cache_warm;
        if self.up_to > 0 {
            // A point further back was cached when it happened.
            let end = self.turns[self.up_to].end;
            if end != 0 {
                warm = end + 3_600_000;
            }
        }
        if now < warm {
            return line([
                sp("↳ added after the cache", GREEN),
                sp(
                    format!(
                        " (warm till {}): what it remembers costs about a tenth",
                        super::clock::hhmm(warm)
                    ),
                    DIM,
                ),
            ]);
        }
        if self.cache_warm == 0 && self.up_to == 0 {
            return line([sp(
                "↳ added after what it remembers: read from the cache if used in the last hour",
                DIM,
            )]);
        }
        line([sp(
            "↳ added after what it remembers, but that point's cache is cold: full price once",
            DIM,
        )])
    }

    /// Makes the fork. The whole conversation in the same folder is the
    /// agent's own fork; up to an earlier turn, or into a new worktree,
    /// the agent copies the transcript (cut there) and the copy is
    /// resumed (start).
    fn start(&mut self, m: &mut Model) -> Out {
        let mut name: String = self.name.iter().collect::<String>().trim().to_string();
        if name.is_empty() {
            name = format!("{} (fork)", self.agent);
        }
        let first: String = self.first.iter().collect::<String>().trim().to_string();
        let mut f = Fork {
            name: name.clone(),
            cwd: self.cwd.clone(),
            acct: self.prof.clone(),
            model: self.models[self.model].clone(),
            effort: self.efforts[self.effort].clone(),
            perm: self.perms[self.perm].clone(),
            prompt: first,
            ..Fork::default()
        };
        let cut = (self.up_to > 0).then(|| {
            (
                self.turns[self.up_to].n,
                self.turns[self.up_to - 1].prompt.clone(),
            )
        });
        let (worktree, src, sid, br) = (
            self.worktree,
            self.path.clone(),
            self.sid.clone(),
            self.br.clone(),
        );
        let wt_name = worktree_name(&name);
        let decode = match super::session_view(m, &self.conn).map(|v| &v.source) {
            Some(crate::session::Source::Transcript { decode, .. }) => Some(decode.clone()),
            _ => crate::session::decoder(&m.reg, &self.kind),
        };
        m.flash(&format!("forking {}…", self.agent));
        let key = self.conn.clone();
        if cut.is_none() && !worktree {
            f.session_id = sid.clone();
            f.fork = true;
            f.from = sid;
            act(m, &key, Act::Fork(f));
            return Out::Close;
        }
        m.later(
            move || -> Result<Fork, String> {
                if worktree {
                    f.cwd = rush_sys::actions::new_worktree(&f.cwd, &wt_name)?;
                }
                let mut up_to = -1i64;
                if let Some((n, next)) = cut {
                    let d = decode.ok_or("couldn't read the conversation: no reader for it")?;
                    up_to = cut_after(Path::new(&src), d, n, &next)?;
                }
                let new_id = new_session_id();
                let b = br
                    .as_ref()
                    .and_then(|a| a.brancher())
                    .ok_or("it can't copy its conversation")?;
                b.branch(&f.acct, &src, &f.cwd, &sid, &new_id, up_to)
                    .map_err(|e| format!("couldn't copy the conversation: {e}"))?;
                f.session_id = new_id;
                f.from = sid;
                Ok(f)
            },
            move |m, r| match r {
                Err(e) => {
                    m.flash_err(&e);
                    Out::Draw
                }
                Ok(f) => act(m, &key, Act::Fork(f)),
            },
        );
        Out::Close
    }
}

/// A folder and branch name made from the fork's name (worktreeName).
pub fn worktree_name(name: &str) -> String {
    let mut s = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            s.push(c);
            dash = false;
        } else if !dash {
            s.push('-');
            dash = true;
        }
    }
    let mut s = s.trim_matches('-').to_string();
    if s.len() > 40 {
        s.truncate(40);
        s = s.trim_end_matches('-').to_string();
    }
    if s.is_empty() { "fork".into() } else { s }
}

/// The byte offset in the transcript where turn n ends: where the turn
/// after it, whose prompt is next, starts. The prompt decides when the
/// numbering differs (cutAfter).
pub fn cut_after(path: &Path, d: rush_convo::Decode, n: u32, next: &str) -> Result<i64, String> {
    let starts =
        turn_starts(path, d).map_err(|e| format!("couldn't read the conversation: {e}"))?;
    let mut best = None;
    let mut gap = None;
    for s in &starts {
        if next.is_empty() || s.prompt != next {
            continue;
        }
        let g = s.n.abs_diff(n + 1);
        if gap.is_none_or(|x| g < x) {
            best = Some(s.offset as i64);
            gap = Some(g);
        }
    }
    if let Some(b) = best {
        return Ok(b);
    }
    starts
        .iter()
        .find(|s| s.n == n + 1)
        .map(|s| s.offset as i64)
        .ok_or_else(|| format!("couldn't find where turn {n} ends in the conversation"))
}

impl Sheet for ForkSheet {
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        self.poll(m);
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "up" | "shift+tab" => {
                self.row = (self.row + ROWS - 1) % ROWS;
                return Out::Draw;
            }
            "down" | "tab" => {
                self.row = (self.row + 1) % ROWS;
                return Out::Draw;
            }
            "enter" | "ctrl+s" => return self.start(m),
            _ => {}
        }
        match self.row {
            NAME => {
                if let Some((b, p)) = edit(&self.name, self.name_pos, k) {
                    self.name = b;
                    self.name_pos = p;
                }
                return Out::Draw;
            }
            FIRST => {
                if let Some((b, p)) = edit(&self.first, self.first_pos, k) {
                    self.first = b;
                    self.first_pos = p;
                }
                return Out::Draw;
            }
            _ => {}
        }
        match k.s.as_str() {
            "left" | "h" => self.cycle(-1),
            "right" | "l" | "space" | " " => self.cycle(1),
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn body(&mut self, m: &Model, w: usize, _h: usize) -> Vec<Line> {
        if let Some(Some((k, repo))) = m.kept.peek::<Looked>().map(|l| l.0.clone())
            && k == self.conn
        {
            self.repo = repo;
            self.looked = true;
        }
        let mut out = vec![
            sheet_title(
                &format!("Fork {}", one_line(&self.agent)),
                "a copy of the conversation carries on as a new agent; this one stays as it is",
                w,
            ),
            Line::new(),
        ];
        let label_w = 16;
        let val_w = w.saturating_sub(label_w + 6);
        let choice = |v: &str, what: &str| -> String {
            if !v.is_empty() {
                return v.to_string();
            }
            let now = self.current(what);
            if now.is_empty() {
                return "as now".into();
            }
            let now = if what == "model" {
                m.reg.model_name(&self.kind, now)
            } else {
                now.to_string()
            };
            format!("as now · {now}")
        };
        for r in 0..ROWS {
            let on = r == self.row;
            let (label, val): (&str, Line) = match r {
                NAME => (
                    "Name",
                    text_field(&self.name, self.name_pos, on, "what to call it", val_w),
                ),
                FROM => {
                    let t = &self.turns[self.up_to];
                    let v = if self.up_to == 0 {
                        line([plain(format!("the whole conversation · {} turns", t.n))])
                    } else {
                        line([
                            plain(format!("up to turn {} · ", t.n)),
                            sp(one_line(&t.prompt), FAINT),
                        ])
                    };
                    ("Remembers", v)
                }
                MODEL => (
                    "Model",
                    line([plain(choice(&self.models[self.model], "model"))]),
                ),
                EFFORT => (
                    "Effort",
                    line([plain(choice(&self.efforts[self.effort], "effort"))]),
                ),
                PERM => (
                    "Permissions",
                    line([plain(choice(&self.perms[self.perm], "perm"))]),
                ),
                FOLDER => {
                    let v = if self.worktree {
                        format!(
                            "a new worktree · {}",
                            tildify(&path_join(
                                &path_join(&path_join(&self.repo, ".claude"), "worktrees"),
                                &worktree_name(&self.name.iter().collect::<String>())
                            ))
                        )
                    } else {
                        format!("same folder · {}", tildify(&self.cwd))
                    };
                    let mut l = line([plain(v)]);
                    if !self.worktree && self.repo.is_empty() && self.looked {
                        l.spans.push(sp(" (not a git repo, so no worktree)", FAINT));
                    }
                    ("Folder", l)
                }
                FIRST => (
                    "First message",
                    text_field(
                        &self.first,
                        self.first_pos,
                        on,
                        "optional: what to tell it first",
                        val_w,
                    ),
                ),
                _ => {
                    out.push(Line::new());
                    if on {
                        out.push(line([
                            plain("  "),
                            Span::new(" Fork ", bold(ORANGE).on(SEL_BG)),
                            sp("  enter", DIM),
                        ]));
                    } else {
                        out.push(line([plain("  "), sp("[Fork]", SUB), sp("  enter", DIM)]));
                    }
                    continue;
                }
            };
            let mut l = line([sp(fit_str(label, label_w), SUB)]);
            if r == NAME || r == FIRST {
                l.spans.push(plain("  "));
                join(&mut l, val);
            } else {
                l.spans.push(sp("‹ ", FAINT));
                let mut v = cut(val, val_w);
                for s in &mut v.spans {
                    if s.style.fg == Color::Default {
                        s.style.fg = TEXT;
                    }
                }
                join(&mut l, v);
                l.spans.push(sp(" ›", FAINT));
            }
            out.push(sheet_row(l, on, w));
            if r == FIRST {
                // Where that message lands: after the cache, or breaking it.
                let mut n = line([plain(blanks(label_w + 4))]);
                join(
                    &mut n,
                    fit(
                        self.first_note(super::now(m)),
                        10.max(w.saturating_sub(label_w + 5)),
                    ),
                );
                out.push(n);
            }
        }
        // What the fork will and won't remember.
        out.push(Line::new());
        if self.up_to > 0 {
            let t = &self.turns[self.up_to];
            out.push(line([sp(
                format!(
                    "  It remembers turns 1–{} and forgets {}–{}. This one keeps them all.",
                    t.n,
                    t.n + 1,
                    self.turns[0].n
                ),
                DIM,
            )]));
        } else {
            out.push(line([sp(
                "  It remembers everything so far, then goes its own way.",
                DIM,
            )]));
        }
        if self.worktree {
            out.push(line([sp(
                format!(
                    "  A new branch, worktree-{}, so its edits don't touch this checkout.",
                    worktree_name(&self.name.iter().collect::<String>())
                ),
                DIM,
            )]));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[
                ("↑↓", "move"),
                ("←→", "change"),
                ("enter", "fork"),
                ("esc", "cancel"),
            ],
        ));
        out
    }
}

/// l cut to w cells with … when it's wider, else as it is
/// (ansi.Truncate).
pub fn cut(l: Line, w: usize) -> Line {
    if l.width() > w { rush_draw::truncate_line(&l, w, "…") } else { l }
}
