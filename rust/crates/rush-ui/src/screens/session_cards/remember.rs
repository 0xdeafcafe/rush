//! Remembering: #remember <text>, or alt+k on a picked message, has a
//! cheap model write a memory note from it (its name, description and
//! type), then asks before it's kept: y keeps it, e keeps it and opens it
//! in the memory view. And the memory view's keys for its project's
//! memory as a whole: + remembers something new, r reviews the notes
//! worth a look, f tidies the index.
//!
//! Go: internal/ui/remember.go (rememberDraft :21, remember :30, memDraft
//! :121, rememberPicked :127), keys.go:909 (#remember), rushmode.go:3735
//! (alt+k) and memory.go:557 (memoryWideKey). e picks the new note in
//! the memory view and opens it in the view's editor (remember.go:92).

use std::sync::Arc;

use rush_core::adapter::{Feature, Profile};
use rush_memory::MemoryNote;

use super::rewind::sel_turn;
use super::util::first_non_empty;
use crate::frame::confirm::Confirm;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::projects_machine::memreview::{self, keeper};
use crate::screens::session_core::{self as core, views};
use crate::session::harness_name;

/// Drafts a note from (what, around): the cheap model's, or a test's.
pub type Drafter = Arc<dyn Fn(&str, &str) -> Result<MemoryNote, String> + Send + Sync>;

/// What remembering keeps between keys.
#[derive(Default)]
pub struct Remembering {
    /// Drafts notes in place of the cheap model (rememberDraft), for
    /// tests.
    pub drafter: Option<Drafter>,
    /// A note drafted while you'd moved on, waiting to be asked about
    /// (memDraft).
    waiting: Option<Draft>,
}

/// A drafted note, and whose project it's for.
#[derive(Clone)]
struct Draft {
    key: String,
    p: Profile,
    cwd: String,
    n: MemoryNote,
}

/// The profile and folder of session key's project (memProfile).
fn mem_profile(m: &Model, key: &str) -> (Profile, String) {
    let cwd = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| v.sess.info.cwd.clone())
        .unwrap_or_default();
    match core::agent(m, key) {
        Some(a) => {
            let mut p = a.acct.clone();
            if p.kind.is_empty() {
                p.kind = a.kind.clone();
            }
            (p, first_non_empty(&[&cwd, &a.job.cwd]).to_string())
        }
        None => (Profile::default(), cwd),
    }
}

/// The kind of session key's agent.
fn kind_of(m: &Model, key: &str) -> String {
    core::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default()
}

/// #remember <what>: for the open session; the agent picked in the list
/// must be the one open.
pub fn remember_command(m: &mut Model, arg: &str) -> Out {
    let open = core::open_key(m);
    if !m.sel.is_empty() && open.as_deref() != Some(m.sel.as_str()) {
        let name = core::agent(m, &m.sel.clone())
            .map(|a| a.job.name.clone())
            .unwrap_or_default();
        if !name.is_empty() {
            m.flash_err(&format!(
                "open {name}'s session to remember something for its project"
            ));
            return Out::Draw;
        }
    }
    remember(m, open.as_deref(), arg, "")
}

/// Drafts a note from what, and the conversation around it, for session
/// key, then asks to keep it (remember).
pub fn remember(m: &mut Model, key: Option<&str>, what: &str, around: &str) -> Out {
    let Some(key) = key else {
        m.flash_err("open a session to remember something for its project");
        return Out::Draw;
    };
    let kind = kind_of(m, key);
    let mk = keeper(m, &kind);
    if mk.is_none() || !m.reg.supports(&kind, Feature::Memory) {
        let name = harness_name(&m.reg, &kind.as_str().into());
        m.flash_err(&format!("{name} keeps no memory rush can write"));
        return Out::Draw;
    }
    if format!("{what}{around}").trim().is_empty() {
        let r = m.kept::<Remembering>();
        if r.waiting.as_ref().is_some_and(|d| d.key == key) {
            let d = r.waiting.take().expect("checked");
            ask(m, d);
            return Out::Draw;
        }
        m.flash_err("#remember <what to remember>, or alt+k on a message");
        return Out::Draw;
    }
    let (p, cwd) = mem_profile(m, key);
    m.flash("writing the note…");
    let drafter = m.kept::<Remembering>().drafter.clone();
    let reg = m.reg.clone();
    let (what, around, key) = (what.to_string(), around.to_string(), key.to_string());
    m.later(
        move || match drafter {
            Some(d) => d(&what, &around),
            None => rush_memory::draft(
                |system, prompt| {
                    rush_core::adapter::ask_cheap(&reg, system, prompt, "haiku")
                        .map(|(_, out)| out)
                        .map_err(|e| e.to_string())
                },
                &what,
                &around,
            ),
        },
        move |m, r| {
            let mut n = match r {
                Ok(n) => n,
                Err(e) => {
                    m.fail(&format!("couldn't write the note: {e}"));
                    return Out::Draw;
                }
            };
            n.title = first_non_empty(&[&n.title, &n.name]).to_string();
            let d = Draft { key, p, cwd, n };
            // The draft takes a while: if you've moved on to something
            // else meanwhile, it waits rather than take your next key.
            let typing = !core::conn(m, &d.key).input.is_empty();
            if m.confirm.is_some() || m.sheet.is_some() || typing {
                m.kept::<Remembering>().waiting = Some(d);
                m.flash("the note is ready · #remember alone shows it");
                return Out::Draw;
            }
            ask(m, d);
            Out::Draw
        },
    );
    Out::Draw
}

/// Asks whether to keep the note drafted.
fn ask(m: &mut Model, d: Draft) {
    let n = &d.n;
    let mut c = Confirm::new(format!("Remember this? {}", n.title), {
        let d = d.clone();
        move |m| keep(m, d, false)
    })
    .detail(format!("{} · {}\n\n{}", n.kind, n.description, n.body));
    c.yes_text = "remember".into();
    c.more.push((
        "e".into(),
        "remember and edit".into(),
        Box::new(move |m| keep(m, d, true)),
    ));
    m.ask(c);
}

/// Writes the note, off the UI thread, then says where and how many other
/// notes are worth a look; edit opens the memory view on it.
fn keep(m: &mut Model, d: Draft, edit: bool) -> Out {
    let kind = d.p.kind.as_str().to_string();
    let kind = if kind.is_empty() {
        kind_of(m, &d.key)
    } else {
        kind
    };
    let Some(mk) = keeper(m, &kind) else {
        return Out::Idle;
    };
    let reg = m.reg.clone();
    let Draft { key, p, cwd, n } = d;
    let title = n.title.clone();
    m.later(
        move || {
            let path = mk.remember(&p, &cwd, &n)?;
            let others = memreview::read_mem_review(&reg, Some(&*mk), &p, &cwd, "")
                .map(|r| r.items.iter().filter(|it| it.f.path != path).count())
                .unwrap_or(0);
            Ok::<_, String>((path, others))
        },
        move |m, r| {
            let (path, others) = match r {
                Ok(x) => x,
                Err(e) => {
                    m.fail(&e);
                    return Out::Draw;
                }
            };
            core::memory::forget(m, &key);
            memreview::drop_mem_cache(m);
            let mut msg = format!("remembered: {title} · in the memory view");
            if others > 0 {
                msg +=
                    &format!(" · {others} other notes worth a look: r on one there reviews them");
            }
            m.flash(&msg);
            if edit
                && views::show_view(m, &key, "memory")
                && let Some(v) = m.session.as_mut().filter(|v| v.key == key)
            {
                v.sel = format!("mem:{path}");
                return core::memedit::open_doc(m, &key, &path);
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// alt+k on a picked message, the box empty: remember it, its exchange or
/// its turn's answer, for a cheap model to pick the one thing worth
/// keeping from (rememberPicked; a session_keys hook).
pub fn remember_key_hook(m: &mut Model, k: &Press) -> Out {
    if k.s != "alt+k" {
        return Out::Pass;
    }
    let Some(key) = core::open_key(m) else {
        return Out::Pass;
    };
    if !core::conn(m, &key).input.is_empty() {
        return Out::Pass;
    }
    let Some(v) = m.session.as_ref() else {
        return Out::Pass;
    };
    if v.sel.is_empty() {
        return Out::Pass;
    }
    let text = v.sess.exchange_text(&v.sel).or_else(|| {
        let n = sel_turn(v)?;
        let t = v.sess.turns.iter().find(|t| t.n == n)?;
        t.answer().filter(|a| !a.is_empty()).map(str::to_string)
    });
    match text {
        Some(text) => remember(m, Some(&key), "", &text),
        None => Out::Pass,
    }
}

/// The memory view's keys for its project's memory as a whole, while a
/// file is picked: + remembers something new, r reviews the notes worth a
/// look, f tidies the index (memoryWideKey; a session_keys hook).
pub fn memory_wide_key_hook(m: &mut Model, k: &Press) -> Out {
    if !matches!(k.s.as_str(), "+" | "r" | "f") {
        return Out::Pass;
    }
    let Some(key) = core::open_key(m) else {
        return Out::Pass;
    };
    let picked = m
        .session
        .as_ref()
        .is_some_and(|v| v.sel.starts_with("mem:"));
    if !picked || views::view_name(m, &key) != "memory" {
        return Out::Pass;
    }
    match k.s.as_str() {
        "+" => {
            core::conn(m, &key).input.set("#remember ");
            if let Some(v) = m.session.as_mut() {
                v.sel.clear();
            }
            Out::Draw
        }
        "r" => {
            let (p, cwd) = mem_profile(m, &key);
            let title = std::path::Path::new(first_non_empty(&[&cwd, "this project"]))
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "this project".into());
            memreview::open_mem_review(m, p, &cwd, &title);
            Out::Draw
        }
        _ => {
            let kind = kind_of(m, &key);
            let dir = core::memory::memory_known(m, &key)
                .map(|mem| mem.dir)
                .unwrap_or_default();
            let Some(mk) = keeper(m, &kind).filter(|_| !dir.is_empty()) else {
                return Out::Pass;
            };
            m.later(
                move || mk.tidy_memory(&dir),
                move |m, r| {
                    core::memory::forget(m, &key);
                    match r {
                        Err(e) => m.fail(&e),
                        Ok(did) if did.is_empty() => m.flash(
                            "nothing in the index to tidy · the rest needs you: r reviews it",
                        ),
                        Ok(did) => m.flash(&did.join(" · ")),
                    }
                    Out::Draw
                },
            );
            Out::Draw
        }
    }
}
