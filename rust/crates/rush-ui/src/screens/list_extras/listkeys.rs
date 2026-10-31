//! The list's own keys on the agent picked and on the list itself:
//! ctrl+y its pull request, ctrl+t pin it, ctrl+s group by the next,
//! ctrl+p split each section by project, and ctrl+v an image off the
//! clipboard into the Agents Prompt. After Go's internal/ui/keys.go
//! (listKey's ctrl+v :284, ctrl+y :444, ctrl+t :446, ctrl+s :450, ctrl+p
//! :453; cycleGroupBy :673), model.go:2695 (togglePin), folders.go:35
//! (toggleSplit), picker.go:286 (openPR, browse :303, the pull requests'
//! rows :451-470), sidebar.go:25 (groupModes, groupLabel) and
//! clipimage.go (pasteClipImage :42, attachImages :113) with
//! rushmode.go:5342 (the Prompt's images taken as a session starts).

use rush_core::agent::Agent;

use crate::frame::picker::{Pick, Picker};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::plugin_ui::sidebar::{group_label, group_modes};
use crate::screens::session_cards::clipimage::{ClipState, clip_image};
use crate::screens::session_cards::imagerefs::with_marks;
use crate::style::*;

/// The list's keys (listKey's switch): Pass for any other.
pub fn key(m: &mut Model, k: &Press) -> Out {
    match k.s.as_str() {
        "ctrl+v" | "super+v" if m.sheet.is_none() => paste_image(m),
        "ctrl+y" => {
            let a = m.selected_row();
            open_pr(m, a.as_ref())
        }
        "ctrl+t" => match m.selected_row() {
            Some(a) => toggle_pin(m, &a),
            None => Out::Pass,
        },
        "ctrl+s" => {
            cycle_group_by(m);
            Out::Draw
        }
        "ctrl+p" => {
            toggle_split(m);
            // Split, the list's projects are asked of git again
            // (keys.go:455 refreshFolders).
            super::landing::refresh(m);
            Out::Draw
        }
        _ => Out::Pass,
    }
}

// --- ctrl+t ---

/// Pins a in its agent's own list, or unpins it, off the UI thread; its
/// row moves once the list reads the pins again (togglePin).
pub fn toggle_pin(m: &mut Model, a: &Agent) -> Out {
    let Some(ad) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.pinner().is_some())
    else {
        m.flash_err(&format!("{}'s agent keeps no pins", a.display_name));
        return Out::Draw;
    };
    let p = crate::screens::cmdbar::attach::acct_of(m, a);
    let id = a.job.id.clone();
    m.later(
        move || {
            ad.pinner()
                .expect("filtered")
                .toggle_pin(&p, &id)
                .map_err(|e| e.to_string())
        },
        |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                m.fail(&e);
                Out::Draw
            }
        },
    );
    Out::Idle
}

// --- ctrl+s and ctrl+p ---

/// The list grouped by the next of its modes, saved (cycleGroupBy).
pub fn cycle_group_by(m: &mut Model) {
    let sidebars = m.plugins().sidebars.clone();
    let modes = group_modes(&sidebars);
    let cur = m.cfg.group_by.clone();
    let mut next = modes[0].clone();
    for (i, g) in modes.iter().enumerate() {
        if *g == cur {
            next = modes[(i + 1) % modes.len()].clone();
        }
    }
    m.cfg.group_by.clone_from(&next);
    m.flash(&format!("grouped by {}", group_label(&sidebars, &next)));
    save(m, move |c| c.group_by.clone_from(&next));
    m.rebuild();
}

/// Each section split by project, or not (toggleSplit).
pub fn toggle_split(m: &mut Model) {
    let v = if m.cfg.split_projects() {
        m.flash("sections no longer split by project");
        "none"
    } else {
        m.flash("each section split by project");
        "project"
    };
    m.cfg.split_by = v.into();
    save(m, move |c| c.split_by = v.into());
    m.rebuild();
}

/// A change to config.json, in every copy rush holds and on disk in
/// order (SaveConfig).
fn save(m: &mut Model, f: impl Fn(&mut rush_state::Config) + Send + 'static) {
    crate::config::save(m, "couldn't save the settings", f);
}

// --- ctrl+y ---

/// a's pull request opened in the browser, or a picker when it has
/// several; GitHub is asked again whatever the minute (openPR).
pub fn open_pr(m: &mut Model, a: Option<&Agent>) -> Out {
    if let Some(a) = a.filter(|a| !a.root.is_empty()) {
        let root = a.root.clone();
        m.later(
            move || rush_gh::ghpr::Gh::shared().refresh(&root),
            |_, ()| Out::Idle,
        );
    }
    let Some(a) = a.filter(|a| !a.prs.is_empty()) else {
        m.flash_err("no pull request found for this agent");
        return Out::Draw;
    };
    if a.prs.len() == 1 {
        crate::screens::projects_machine::place::browse(m, &a.prs[0].url);
        return Out::Draw;
    }
    let rows = a.prs.iter().map(pr_row).collect();
    let mut p = Picker::new(
        format!("Pull requests · {}", one_line(&a.display_name)),
        rows,
        Box::new(|m, pick, _| {
            if let Some(url) = pick.map(|p| p.value.clone()) {
                crate::screens::projects_machine::place::browse(m, &url);
            }
            Out::Draw
        }),
    );
    p.keys = vec![
        ("↑↓".into(), "choose".into()),
        ("enter".into(), "open in browser".into()),
        ("esc".into(), "close".into()),
    ];
    p.closes = vec!["q".into(), "ctrl+y".into(), "ctrl+l".into()];
    m.open_sheet(Box::new(p));
    Out::Draw
}

/// How wide a pull request's title is in the picker: Go's w-40 at the
/// picker's usual 100.
const PR_TITLE_W: usize = 56;

/// A pull request's row: its number in its state's colour, its title, its
/// state and its checks (pickerBody's prs).
pub fn pr_row(pr: &rush_core::adapter::Pr) -> Pick {
    let col = if pr.state == "MERGED" {
        BLUE
    } else if pr.state == "CLOSED" || pr.state == "unknown" {
        DIM
    } else if pr.checks.failed > 0 {
        RED
    } else {
        GREEN
    };
    let c = &pr.checks;
    let checks = if c.passed + c.failed + c.pending > 0 {
        format!("  {}✓ {}✗ {}…", c.passed, c.failed, c.pending)
    } else {
        String::new()
    };
    let title = if pr.title.is_empty() {
        pr.url
            .strip_prefix("https://github.com/")
            .unwrap_or(&pr.url)
            .to_string()
    } else {
        pr.title.clone()
    };
    let mut label = line([sp(format!("#{:<6}", pr.number), col), plain(" ")]);
    join(&mut label, fit(line([sp(title.clone(), TEXT)]), PR_TITLE_W));
    label.spans.push(plain("  "));
    join(&mut label, fit(line([sp(pr.state.to_lowercase(), DIM)]), 8));
    if !checks.is_empty() {
        label.spans.push(sp(checks, FAINT));
    }
    Pick {
        label,
        find: format!("#{} {title}", pr.number),
        section: String::new(),
        value: pr.url.clone(),
    }
}

// --- ctrl+v ---

/// What the line under the box says while the clipboard is read.
const READING: &str = "reading the clipboard…";

/// Reads an image off the clipboard off the UI thread, into the Prompt
/// at the cursor once it's back (pasteClipImage, clipImageMsg).
pub fn paste_image(m: &mut Model) -> Out {
    if m.kept::<ClipState>().pasting {
        return Out::Idle; // one read at a time
    }
    m.kept::<ClipState>().pasting = true;
    m.flash(READING);
    m.later(clip_image, |m, r| {
        m.kept::<ClipState>().pasting = false;
        match r {
            Err(e) => m.fail(&format!("couldn't read the clipboard: {e}")),
            Ok(None) => m.flash("no image on the clipboard"),
            Ok(Some(p)) => {
                // Go's "reading the clipboard…" is a spinner that goes
                // once the image is in (view.go:714 statusOr).
                if m.flash.as_ref().is_some_and(|f| f.0 == READING) {
                    m.flash = None;
                }
                attach_images(m, &[p]);
            }
        }
        Out::Draw
    });
    Out::Draw
}

/// Image files into the Prompt, each as [Image #N] at the cursor, spaced
/// off the words either side (attachImages, withMarks).
pub fn attach_images(m: &mut Model, imgs: &[String]) {
    let text = m.input.text.clone();
    let pos = m.input.pos.min(text.len());
    m.input.undo.save(&text, pos, false);
    let marked = with_marks(&text, pos, &mut m.imgs, imgs);
    m.input.pos = pos + (marked.len() - text.len());
    m.input.text = marked;
    m.input.anchor = None;
}

/// A task leaving the Prompt to start a session: its markers numbered in
/// order and the images they stand for, the Prompt's images let go
/// (startHosted's imgs.resolve, m.imgs = imageRefs{}).
pub fn take_images(m: &mut Model, text: &str) -> (String, Vec<String>) {
    let refs = std::mem::take(&mut m.imgs);
    refs.resolve(text)
}
