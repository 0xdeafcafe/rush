//! Settings › General › Commands you run: whether a shell box or a `!`
//! command runs in a terminal of its own, whether a password one asks for
//! is typed in its box, and, on a Mac, Touch ID for sudo (written to
//! /etc/pam.d/sudo_local through macOS's administrator dialog,
//! rush_sys::touchid). All off until you turn them on. Rust-only.

use std::rc::Rc;

use super::form::{Section, choice, set};
use super::{State, config, config_mut};
use crate::frame::Out;
use crate::model::Model;

/// Touch ID for sudo as last read: None off a Mac (or before it's read),
/// and whether it's being changed.
#[derive(Clone, Copy, Debug, Default)]
pub struct TouchId {
    pub on: Option<bool>,
    pub busy: bool,
}

/// Reads whether Touch ID for sudo is on, off the UI, for the row.
pub fn read(m: &mut Model) {
    // A test's machine's /etc isn't the page's to show.
    if !cfg!(target_os = "macos") || rush_core::sandbox::active() {
        return;
    }
    m.later(rush_sys::touchid::read, |m, on| {
        m.kept::<State>().touch_id.on = on;
        Out::Draw
    });
}

/// Turns Touch ID for sudo on or off, through macOS's administrator
/// dialog, off the UI; the row says what came of it.
fn set_touch_id(m: &mut Model, v: &str) -> Out {
    let on = v == "on";
    let st = &mut m.kept::<State>().touch_id;
    if st.busy || st.on == Some(on) {
        return Out::Idle;
    }
    st.busy = true;
    m.flash("macOS asks for your administrator password to change it");
    m.later(
        move || rush_sys::touchid::set(on).map(|()| rush_sys::touchid::read()),
        move |m, r| {
            let st = &mut m.kept::<State>().touch_id;
            st.busy = false;
            match r {
                Ok(now) => {
                    st.on = now;
                    m.flash(if on {
                        "Touch ID for sudo is on"
                    } else {
                        "Touch ID for sudo is off"
                    });
                }
                Err(e) => m.flash_err(&format!("Touch ID for sudo isn't changed: {e}")),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// The section.
pub fn section(m: &Model) -> Section {
    let c = config(m);
    let terminal = choice(
        "Commands you run",
        if c.shell_terminal {
            "in a terminal"
        } else {
            "without a terminal"
        },
        "How a shell block you run (▶ run) or a command after ! runs. In a terminal of its own (a pseudo-terminal), sudo, ssh and the rest that need one work, and output is coloured as in a terminal window; its input stays closed and pagers are off (PAGER=cat).",
        &[
            (
                "without a terminal",
                "its output goes straight to its box; sudo can't ask for a password.",
            ),
            (
                "in a terminal",
                "it runs in a terminal of its own, so sudo can ask for a password (or for Touch ID).",
            ),
        ],
        set(|m, v| config_mut(m).shell_terminal = v == "in a terminal"),
    );
    let passwords = choice(
        "Passwords commands ask for",
        if c.shell_passwords {
            "typed in its box"
        } else {
            "not asked for"
        },
        "What a command run in a terminal does when it asks for a password (sudo's Password:, its terminal not echoing). Typed in its box, what you type goes to the command when you press enter: rush never draws, logs or keeps it.",
        &[
            (
                "not asked for",
                "it waits for one that can't be typed; ✕ stops it.",
            ),
            (
                "typed in its box",
                "the box shows a masked field; enter gives it to the command.",
            ),
        ],
        set(|m, v| config_mut(m).shell_passwords = v == "typed in its box"),
    );
    let mut rows = vec![terminal, passwords];
    let tid = m
        .kept
        .peek::<State>()
        .map(|s| s.touch_id)
        .unwrap_or_default();
    if let Some(on) = tid.on {
        let mut row = choice(
            "Touch ID for sudo",
            if on { "on" } else { "off" },
            "Whether sudo shows the Touch ID screen instead of asking for your password, by the pam_tid.so line of /etc/pam.d/sudo_local (from the template macOS ships; /etc/pam.d/sudo itself is never changed). Changing it asks for your administrator password in macOS's own dialog. In tmux, Touch ID also needs pam_reattach.",
            &[
                ("off", "sudo asks for your password."),
                (
                    "on",
                    "sudo shows the Touch ID screen, in a terminal and in a box run in one.",
                ),
            ],
            None,
        );
        row.run = Some(Rc::new(set_touch_id));
        if tid.busy {
            row.source = "changing…".into();
        }
        rows.push(row);
    }
    Section {
        title: "Commands you run".into(),
        note: "shell boxes and ! commands".into(),
        rows,
        advanced: false,
    }
}
