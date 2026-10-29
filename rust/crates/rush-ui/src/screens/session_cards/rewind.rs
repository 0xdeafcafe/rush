//! /rewind: takes an agent back to before one of your messages, to try
//! again with less in its context. It's the same agent afterwards. The
//! path it leaves is kept as a branch you can go back down, the code can
//! stay as it is or be put back, and a note of what was learned can come
//! back with you. After Go's internal/ui/rewindsheet.go (rewindSheet :26,
//! openRewindTo :49, openRewindAt :71, preview :168, key :190, body :219,
//! start :345, clearInPlace :431, leftOf :463) and convo/turnstarts.go
//! (TurnStarts :27). The host's half (rewindHost, hangUp, onRewound) is
//! the session's: [`Act::Rewind`] hands it a [`Rewound`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rush_convo::{Decode, Session, Tail};
use rush_core::adapter::{AdapterRef, FileRewind, Profile};
use rush_core::text::Line;
use rush_core::time::Ms;

use super::util::{first_non_empty, plural};
use super::{Act, act, info};
use crate::frame::picker::round_move;
use crate::frame::sheet::{sheet_row, sheet_title, window};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// Where one of the conversation's turns starts in its transcript
/// (convo.TurnStart).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnStart {
    pub n: u32,
    pub prompt: String,
    pub offset: u64,
    /// The message's id in the transcript, which Claude Code's file
    /// checkpoints are kept under.
    pub uuid: String,
    /// Set when you didn't send it (Turn.from).
    pub from: String,
}

/// Reads a transcript and says where each turn starts, so a copy cut
/// there holds the conversation as it was before that turn (TurnStarts).
/// Off the UI thread: it reads the whole file.
pub fn turn_starts(path: &Path, decode: Decode) -> std::io::Result<Vec<TurnStart>> {
    let f = std::fs::File::open(path)?;
    let mut r = BufReader::with_capacity(256 << 10, f);
    let mut tail = Tail::new(path, decode);
    let mut sess = Session::new();
    let mut out = Vec::new();
    let mut off = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(out);
        }
        // The tail takes exactly this line, framed as the conversation
        // frames it.
        let before = sess.turns.len();
        tail.fetch_most(n)?.apply(&mut sess);
        if sess.turns.len() > before
            && let Some(t) = sess.turns.last()
        {
            let uuid = serde_json::from_slice::<serde_json::Value>(trim(&line))
                .ok()
                .and_then(|v| v.get("uuid")?.as_str().map(str::to_string))
                .unwrap_or_default();
            out.push(TurnStart {
                n: t.n,
                prompt: t.prompt.clone(),
                offset: off,
                uuid,
                from: t.from.clone(),
            });
        }
        off += n as u64;
    }
}

fn trim(b: &[u8]) -> &[u8] {
    let s = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let e = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(s, |i| i + 1);
    &b[s..e]
}

/// A path left by an earlier rewind: its session, when it was left, the
/// first turn of its own, your messages on it and the last of them
/// (host.Branch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    pub session_id: String,
    pub left: Ms,
    pub from: u32,
    pub turns: usize,
    pub last: String,
}

/// A rewind, its agent-side work done, for the session's host to carry
/// on from: session_id (resumed, or fresh), the path left kept as a
/// branch, the message to put back in the box and what to say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rewound {
    pub session_id: String,
    pub resume: bool,
    pub left: Branch,
    pub draft: String,
    pub text: String,
}

/// What putting the files back would do (filesPreview).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilesPreview {
    pub loading: bool,
    pub r: FileRewind,
    pub err: String,
}

/// The /rewind sheet (rewindSheet).
pub struct RewindSheet {
    pub agent_key: String,
    pub agent: String,
    pub kind: String,
    pub acct: Profile,
    pub sid: String,
    pub cwd: String,
    pub path: String,
    pub model: String,
    /// Your messages, newest first.
    pub turns: Vec<TurnStart>,
    /// Paths left by earlier rewinds, newest first.
    pub branches: Vec<Branch>,
    /// Into turns, then branches.
    pub sel: usize,
    pub put_back: bool,
    pub recap: bool,
    /// What putting the files back would do, by message.
    pub previews: HashMap<String, FilesPreview>,
}

/// Previews that landed while the sheet was out of the model.
#[derive(Default)]
struct Landed(HashMap<String, FilesPreview>);

/// alt+r on a turn in the history: the sheet, picked to go back to just
/// after it, so it's the last thing the agent remembers (openRewindTo).
pub fn open_rewind_to(m: &mut Model, key: &str, n: u32) -> Out {
    let next = super::session(m, key).and_then(|s| {
        s.turns
            .iter()
            .filter(|u| u.n > n && u.from.is_empty())
            .min_by_key(|u| u.n)
            .map(|u| (u.n, u.prompt.clone()))
    });
    match next {
        Some((n, prompt)) => open_rewind_at(m, key, n, &prompt),
        None => {
            let name = info(m, key).map(|i| i.name).unwrap_or_default();
            m.flash(&format!(
                "that's where {name} is now · pick an earlier turn, or /rewind"
            ));
            Out::Draw
        }
    }
}

/// /rewind: reads where each of your messages starts in the
/// conversation, then opens the sheet (openRewind).
pub fn open_rewind(m: &mut Model, key: &str) -> Out {
    open_rewind_at(m, key, 0, "")
}

/// Opens it with the message numbered n, whose text is prompt, picked
/// (the transcript may number turns differently; the text decides)
/// (openRewindAt).
pub fn open_rewind_at(m: &mut Model, key: &str, n: u32, prompt: &str) -> Out {
    let Some(inf) = info(m, key) else {
        m.flash_err(
            "/rewind works in rush-mode sessions · /rush moves this one over, or /fork from an earlier turn",
        );
        return Out::Draw;
    };
    let live = super::session(m, key).is_some_and(|s| s.live().is_some());
    if inf.state == "working" || inf.state == "blocked" || live {
        m.flash_err(&format!(
            "{} is working: stop it (esc) or let the turn end, then /rewind",
            inf.name
        ));
        return Out::Draw;
    }
    let decode = match m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| &v.source)
    {
        Some(crate::session::Source::Transcript { decode, .. }) => Some(decode.clone()),
        _ => crate::session::decoder(&m.reg, &inf.kind),
    };
    let path = first_non_empty(&[&inf.path, &transcript_of(m, key)]).to_string();
    let f = RewindSheet {
        agent_key: key.into(),
        agent: inf.name.clone(),
        kind: inf.kind.clone(),
        acct: inf.acct.clone(),
        sid: inf.session_id.clone(),
        cwd: inf.cwd.clone(),
        path: path.clone(),
        model: inf.model.clone(),
        turns: Vec::new(),
        branches: inf.branches.clone(),
        sel: 0,
        put_back: false,
        recap: false,
        previews: HashMap::new(),
    };
    m.kept::<Landed>().0.clear();
    let prompt = prompt.to_string();
    m.later(
        move || match decode {
            None => Err("couldn't read the conversation: no reader for it".to_string()),
            Some(d) => turn_starts(Path::new(&path), d)
                .map_err(|e| format!("couldn't read the conversation: {e}")),
        },
        move |m, r| {
            let starts = match r {
                Ok(s) => s,
                Err(e) => {
                    m.flash_err(&e);
                    return Out::Draw;
                }
            };
            let mut f = f;
            f.turns = starts
                .into_iter()
                .rev()
                .filter(|s| s.from.is_empty())
                .collect();
            if !prompt.is_empty() {
                let mut gap = None;
                for (i, t) in f.turns.iter().enumerate() {
                    let g = t.n.abs_diff(n);
                    if t.prompt == prompt && gap.is_none_or(|x| g < x) {
                        f.sel = i;
                        gap = Some(g);
                    }
                }
            }
            if f.turns.is_empty() && f.branches.is_empty() {
                m.flash_err(&format!(
                    "nothing to rewind yet: {} hasn't had a turn",
                    f.agent
                ));
                return Out::Draw;
            }
            m.open_sheet(Box::new(f));
            Out::Draw
        },
    );
    Out::Draw
}

/// The open conversation's transcript, when it's read from one.
pub(super) fn transcript_of(m: &Model, key: &str) -> String {
    match m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| &v.source)
    {
        Some(crate::session::Source::Transcript { path, .. }) => {
            path.to_string_lossy().into_owned()
        }
        _ => String::new(),
    }
}

impl RewindSheet {
    fn n(&self) -> usize {
        self.turns.len() + self.branches.len()
    }

    /// The message picked, or None when a branch is.
    pub fn turn(&self) -> Option<&TurnStart> {
        self.turns.get(self.sel)
    }

    pub fn branch(&self) -> Option<&Branch> {
        self.sel
            .checked_sub(self.turns.len())
            .and_then(|i| self.branches.get(i))
    }

    // The agent's adapter, when it can rewind: /rewind is offered only to
    // agents that are both Rewinder and Brancher (FeatureRewind).
    fn rewinder(&self, m: &Model) -> Result<AdapterRef, String> {
        match m.reg.get(&self.kind) {
            Some(a) if a.rewinder().is_some() && a.brancher().is_some() => Ok(a),
            _ => Err(format!("{} can't rewind", self.agent)),
        }
    }

    // Takes in previews that landed while it was out of the model.
    fn take_landed(&mut self, m: &mut Model) {
        for (k, v) in m.kept::<Landed>().0.drain() {
            self.previews.insert(k, v);
        }
    }

    /// Asks, once per message, what putting the files back would do
    /// (preview).
    fn preview(&mut self, m: &mut Model) {
        let Some(t) = self.turn() else { return };
        if !self.put_back || t.uuid.is_empty() || self.previews.contains_key(&t.uuid) {
            return;
        }
        let Ok(ad) = self.rewinder(m) else { return };
        let uuid = t.uuid.clone();
        self.previews.insert(
            uuid.clone(),
            FilesPreview {
                loading: true,
                ..FilesPreview::default()
            },
        );
        let (acct, cwd, sid, model) = (
            self.acct.clone(),
            self.cwd.clone(),
            self.sid.clone(),
            self.model.clone(),
        );
        let key = self.agent_key.clone();
        let u = uuid.clone();
        m.later(
            move || {
                let rw = ad.rewinder().expect("a rewinder");
                rw.rewind_files(&acct, &cwd, &sid, &model, &u, true)
                    .map_err(|e| e.to_string())
            },
            move |m, r| {
                let p = match r {
                    Ok(r) => FilesPreview {
                        loading: false,
                        r,
                        err: String::new(),
                    },
                    Err(err) => FilesPreview {
                        loading: false,
                        r: FileRewind::default(),
                        err,
                    },
                };
                match m.sheet_as::<RewindSheet>().filter(|s| s.agent_key == key) {
                    Some(s) => {
                        s.previews.insert(uuid, p);
                    }
                    None => {
                        m.kept::<Landed>().0.insert(uuid, p);
                    }
                }
                Out::Draw
            },
        );
    }

    /// Rewinds: a note of the path being left first (while it's all
    /// there), then the files, then a copy of the conversation cut before
    /// the picked message, which the agent's host switches to (start).
    fn start(&mut self, m: &mut Model) -> Out {
        if let Some(t) = self.turn()
            && self.put_back
        {
            let p = self.previews.get(&t.uuid);
            if t.uuid.is_empty()
                || p.is_some_and(|p| !p.loading && (!p.err.is_empty() || !p.r.can_rewind))
            {
                m.flash_err("can't put the code back from there · c keeps it as it is");
                return Out::Idle;
            }
        }
        // The path being left, as a branch to come back to.
        let mut left = Branch {
            turns: self.turns.len(),
            last: self
                .turns
                .first()
                .map(|t| t.prompt.clone())
                .unwrap_or_default(),
            left: super::now(m),
            ..Branch::default()
        };
        let key = self.agent_key.clone();
        if let Some(b) = self.branch() {
            left.from = b.from;
            let from = b.from.max(1);
            m.flash(&format!(
                "switching {} to the path from turn {from}…",
                self.agent
            ));
            return close_with(act(
                m,
                &key,
                Act::Rewind(Rewound {
                    session_id: b.session_id.clone(),
                    resume: true,
                    left,
                    draft: String::new(),
                    text: format!("back down the path from turn {from} · the one you left is kept"),
                }),
            ));
        }
        let Some(turn) = self.turn().cloned() else {
            return Out::Idle;
        };
        left.from = turn.n;
        let ad = match self.rewinder(m) {
            Ok(a) => a,
            Err(e) => {
                m.flash_err(&e);
                return Out::Close;
            }
        };
        let (put_back, recap) = (self.put_back, self.recap);
        let (src, sid, acct, cwd, model) = (
            self.path.clone(),
            self.sid.clone(),
            self.acct.clone(),
            self.cwd.clone(),
            self.model.clone(),
        );
        if recap {
            m.flash(&format!(
                "asking {} what it learned, then rewinding…",
                self.agent
            ));
        } else {
            m.flash(&format!("rewinding {}…", self.agent));
        }
        m.later(
            move || -> Result<Rewound, String> {
                let rw = ad.rewinder().expect("a rewinder");
                let br = ad.brancher().expect("a brancher");
                let mut draft = turn.prompt.clone();
                if recap {
                    let note = rw
                        .recap(&acct, &cwd, &sid, &model, &turn.prompt)
                        .map_err(|e| e.to_string())?;
                    draft = format!(
                        "I tried this before and rewound. What we learned:\n\n{note}\n\n{}",
                        turn.prompt
                    );
                }
                if put_back {
                    rw.rewind_files(&acct, &cwd, &sid, &model, &turn.uuid, false)
                        .map_err(|e| format!("couldn't put the code back: {e}"))?;
                }
                // Before the first message there's nothing to resume: a
                // fresh one.
                let new_id = new_session_id();
                let resume = turn.offset > 0 && turn.n > 1;
                if resume {
                    // With its file checkpoints, so the copy can put files
                    // back to its earlier turns too.
                    br.branch(&acct, &src, &cwd, &sid, &new_id, turn.offset as i64)
                        .map_err(|e| format!("couldn't copy the conversation: {e}"))?;
                }
                let mut text = format!(
                    "rewound to before turn {} · the path you left is kept (/rewind)",
                    turn.n
                );
                if put_back {
                    text.push_str(" · code put back");
                }
                Ok(Rewound {
                    session_id: new_id,
                    resume,
                    left,
                    draft,
                    text,
                })
            },
            move |m, r| match r {
                Err(e) => {
                    m.flash_err(&e);
                    Out::Draw
                }
                Ok(rw) => act(m, &key, Act::Rewind(rw)),
            },
        );
        Out::Close
    }
}

// The sheet closes whatever the act said.
fn close_with(_: Out) -> Out {
    Out::Close
}

/// A new session id: a random UUID, version 4 (host.NewSessionID).
pub fn new_session_id() -> String {
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .is_ok();
    if !ok {
        // No randomness to read: the clock and the process, mixed.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let x = t ^ ((std::process::id() as u128) << 64);
        b = x.to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// /clear: the agent carries on in a fresh conversation, named by its
/// next message, and the one it leaves is kept as a path to go back down
/// with /rewind (clearInPlace).
pub fn clear_in_place(m: &mut Model, key: &str) -> Out {
    let name = info(m, key).map(|i| i.name).unwrap_or_default();
    let mut left = left_of(m, key);
    left.left = super::now(m);
    m.flash(&format!("clearing {name}…"));
    act(
        m,
        key,
        Act::Rewind(Rewound {
            session_id: new_session_id(),
            resume: false,
            left,
            draft: String::new(),
            text: "cleared · the conversation it had is kept (/rewind)".into(),
        }),
    )
}

/// The conversation agent key leaves, as a path for /rewind (leftOf).
pub fn left_of(m: &Model, key: &str) -> Branch {
    let mut left = Branch {
        from: 1,
        ..Branch::default()
    };
    if let Some(s) = super::session(m, key) {
        for t in s.turns.iter().filter(|t| !t.prompt.is_empty()) {
            left.turns += 1;
            left.last = t.prompt.clone();
        }
    }
    left
}

/// "its host was from an older rush" when the host had to restart to
/// rewind (restartNote): for the session to add to Rewound's text.
pub fn restart_note(restarted: bool) -> &'static str {
    if restarted {
        " · its host was from an older rush, so it restarted on this one"
    } else {
        ""
    }
}

impl Sheet for RewindSheet {
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        self.take_landed(m);
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" => return Out::Close,
            "up" | "k" | "shift+tab" => self.sel = round_move(self.sel, -1, self.n()),
            "down" | "j" | "tab" => self.sel = round_move(self.sel, 1, self.n()),
            "home" | "g" => self.sel = 0,
            "end" | "G" => self.sel = self.n().saturating_sub(1),
            "c" | "left" | "right" => {
                if self.turn().is_some() {
                    self.put_back = !self.put_back;
                }
            }
            "n" | "space" | " " => {
                if self.turn().is_some() {
                    self.recap = !self.recap;
                }
            }
            "enter" | "ctrl+s" => return self.start(m),
            _ => {}
        }
        self.preview(m);
        self.take_landed(m);
        Out::Draw
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        if let Some(l) = m.kept.peek::<Landed>() {
            for (k, v) in &l.0 {
                self.previews.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let mut out = vec![
            sheet_title(
                &format!("Rewind {}", one_line(&self.agent)),
                "go back to before one of your messages and try again with less in its context",
                w,
            ),
            Line::new(),
        ];
        // What's under the list takes about 9 lines; the list gets the rest.
        let room = 3.max(h as isize - out.len() as isize - 10) as usize;
        let now = super::now(m);
        let nt = self.turns.len();
        let mut items = nt;
        if !self.branches.is_empty() {
            items += 1 + self.branches.len();
        }
        let row = |out: &mut Vec<Line>, i: usize, l: Line| {
            out.push(sheet_row(
                super::fork::cut(l, w.saturating_sub(2)),
                i == self.sel,
                w,
            ));
        };
        // Keep the picked row in view: the list is short enough to window
        // by item, headings included.
        let pos = if self.sel >= nt {
            self.sel + 1
        } else {
            self.sel
        };
        let (from, to) = window(items, pos, room);
        for k in from..to {
            if k < nt {
                let t = &self.turns[k];
                let mut label = format!("turn {}", t.n);
                if k == 0 {
                    label.push_str(" · the last");
                }
                row(
                    &mut out,
                    k,
                    line([sp(fit_str(&label, 20), SUB), sp(one_line(&t.prompt), TEXT)]),
                );
            } else if k == nt {
                out.push(Line::new());
                out.push(line([sp("  or go back down a path you left", DIM)]));
            } else {
                let j = k - nt - 1;
                let b = &self.branches[j];
                let label = format!("⑂ from turn {}", b.from.max(1));
                let mut what = format!(
                    "{} of your messages · left {} ago",
                    b.turns,
                    age(now.saturating_sub(b.left))
                );
                if !b.last.is_empty() {
                    what.push_str(&format!(" · {}", one_line(&b.last)));
                }
                row(
                    &mut out,
                    nt + j,
                    line([sp(fit_str(&label, 20), SUB), sp(what, TEXT)]),
                );
            }
        }
        if to < items {
            out.push(line([sp(format!("  … {} more", items - to), FAINT)]));
        }
        out.push(Line::new());

        if let Some(b) = self.branch() {
            out.push(line([sp(
                format!(
                    "  Carries on down that path from where it was left: {} of your messages, apart from here since turn {}.",
                    b.turns,
                    b.from.max(1)
                ),
                DIM,
            )]));
            out.push(line([sp(
                "  This path is kept as a branch too. Files stay as they are now.",
                DIM,
            )]));
            out.push(Line::new());
            out.push(keys(&[
                ("↑↓", "pick"),
                ("enter", "switch"),
                ("esc", "cancel"),
            ]));
            return out;
        }
        let Some(t) = self.turn() else { return out };
        let dropped = self.sel + 1;
        if dropped == 1 {
            out.push(line([
                sp(format!("  It forgets turn {}. ", t.n), DIM),
                sp(
                    "Your message goes back in the box to change and send again.",
                    DIM,
                ),
            ]));
        } else {
            out.push(line([
                sp(
                    format!(
                        "  It forgets turns {}–{} ({dropped} of your messages). ",
                        t.n, self.turns[0].n
                    ),
                    DIM,
                ),
                sp("Your message goes back in the box.", DIM),
            ]));
        }
        out.push(line([sp(
            "  The path you leave is kept: /rewind again to go back down it.",
            DIM,
        )]));
        out.push(Line::new());
        let opt = |out: &mut Vec<Line>, key: &str, label: &str, val: Line| {
            let mut l = line([
                plain("  "),
                sp(key, ORANGE),
                plain(" "),
                sp(fit_str(label, 11), SUB),
            ]);
            join(&mut l, val);
            out.push(l);
        };
        let p = self.previews.get(&t.uuid);
        if self.put_back {
            let mut val = line([sp(
                format!("put back as they were before turn {}", t.n),
                TEXT,
            )]);
            if t.uuid.is_empty() {
                val = line([sp("can't: this message has no checkpoint", RED)]);
            } else if let Some(p) = p {
                if p.loading {
                    val.spans.push(sp(" · checking…", FAINT));
                } else if !p.err.is_empty() || !p.r.can_rewind {
                    let why = first_non_empty(&[&p.r.error, &p.err, "no checkpoint"]);
                    val = line([sp(format!("can't: {}", one_line(why)), RED)]);
                } else if p.r.files.is_empty() {
                    val.spans.push(sp(" · nothing to change", FAINT));
                } else {
                    let n = p.r.files.len();
                    val.spans
                        .push(sp(format!(" · {n} file{} ", plural(n)), FAINT));
                    val.spans.push(sp(format!("+{}", p.r.insertions), GREEN));
                    val.spans.push(plain(" "));
                    val.spans.push(sp(format!("−{}", p.r.deletions), RED));
                }
            }
            opt(&mut out, "c", "Code", val);
            if let Some(p) = p
                && !p.loading
                && p.err.is_empty()
                && !p.r.files.is_empty()
            {
                let names: Vec<String> = p.r.files.iter().map(|f| tildify(f)).collect();
                out.push(line([sp(
                    format!(
                        "               {}",
                        rush_draw::truncate(&names.join(", "), w.saturating_sub(17), "…")
                    ),
                    FAINT,
                )]));
            }
        } else {
            opt(
                &mut out,
                "c",
                "Code",
                line([
                    sp("keep it as it is now", TEXT),
                    sp(" · only the conversation goes back", FAINT),
                ]),
            );
        }
        if self.recap {
            opt(
                &mut out,
                "n",
                "Bring back",
                line([
                    sp("a note of what was learned", TEXT),
                    sp(" · Claude writes it first, one extra turn", FAINT),
                ]),
            );
        } else {
            opt(
                &mut out,
                "n",
                "Bring back",
                line([
                    sp("nothing", TEXT),
                    sp(" · or a note of what was tried and found", FAINT),
                ]),
            );
        }
        out.push(Line::new());
        out.push(keys(&[
            ("↑↓", "pick"),
            ("c", "code"),
            ("n", "note"),
            ("enter", "rewind"),
            ("esc", "cancel"),
        ]));
        out
    }
}
