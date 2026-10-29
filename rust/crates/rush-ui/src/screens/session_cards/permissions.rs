//! A session's permission modes: those its harness says it has, the
//! don't-ask one the approval card's o and #yolo switch to, and changing
//! it. After Go's internal/ui/permissions.go (permissionChoices :12,
//! permissionKnown :32, bypassMode :43, setPermission :54,
//! permissionCommand :83).
//!
//! #perm with nothing named opens the modes to pick from as a choice
//! sheet; Go opens the start sheet on its permissions row, which is the
//! start sheet's group's to port.

use rush_core::adapter::Choice;

use super::{Act, HostInfo, act, info};
use crate::frame::Out;
use crate::frame::choice::{self, ChoiceSheet};
use crate::model::Model;

/// The modes on offer: what the running harness says it has when it's
/// that harness, else what its adapter says a new session can start with
/// (permissionChoices).
pub fn permission_choices(m: &Model, kind: &str, c: Option<&HostInfo>) -> Vec<Choice> {
    if let Some(c) = c
        && c.kind == kind
        && !c.permission_modes.is_empty()
    {
        return c
            .permission_modes
            .iter()
            .map(|v| {
                let mut note = v.name.trim().to_string();
                if !v.description.is_empty() {
                    if !note.is_empty() {
                        note.push_str(" · ");
                    }
                    note.push_str(&v.description);
                }
                Choice {
                    id: v.id.clone(),
                    note,
                    context: 0,
                }
            })
            .collect();
    }
    m.reg.choices_of(kind).map(|c| c.modes).unwrap_or_default()
}

/// Whether mode is one on offer (permissionKnown).
pub fn permission_known(m: &Model, kind: &str, c: Option<&HostInfo>, mode: &str) -> bool {
    permission_choices(m, kind, c).iter().any(|v| v.id == mode)
}

/// The harness's don't-ask-again mode, when it has one: #yolo's, and the
/// approval card's o (bypassMode).
pub fn bypass_mode(m: &Model, kind: &str, c: Option<&HostInfo>) -> String {
    permission_choices(m, kind, c)
        .into_iter()
        .find(|v| {
            matches!(
                v.id.as_str(),
                "bypassPermissions" | "full-access" | "yolo" | "auto-approve"
            )
        })
        .map(|v| v.id)
        .unwrap_or_default()
}

/// Switches agent key's hosted session to mode (setPermission).
pub fn set_permission(m: &mut Model, key: &str, mode: &str) -> Out {
    let Some(c) = info(m, key) else {
        m.flash_err("Permissions can be changed in a hosted session; use #rush first");
        return Out::Draw;
    };
    if !permission_known(m, &c.kind, Some(&c), mode) {
        m.flash_err(&format!(
            "This harness does not offer permission mode {mode}"
        ));
        return Out::Draw;
    }
    let out = act(m, key, Act::Permission(mode.to_string()));
    m.flash(&format!(
        "Permission change requested: {mode} · applies to subsequent tools or turns"
    ));
    out.or_idle().max_draw()
}

/// #perm [mode] and #yolo on agent key's session: a mode named is
/// switched to if the harness has it; nothing named lists them
/// (permissionCommand).
pub fn permission_command(m: &mut Model, key: &str, arg: &str, yolo: bool) -> Out {
    let c = info(m, key);
    let Some(c) = c else {
        m.flash_err("Permissions can be changed in a hosted session; use #rush first");
        return Out::Draw;
    };
    let mut arg = arg.trim().to_string();
    if yolo {
        if !arg.is_empty() {
            m.flash_err("#yolo takes no arguments; use #perm to choose a permission mode");
            return Out::Draw;
        }
        arg = bypass_mode(m, &c.kind, Some(&c));
        if arg.is_empty() {
            m.flash_err("This harness does not advertise a bypass permission mode");
            return Out::Draw;
        }
    }
    if !arg.is_empty() {
        if !permission_known(m, &c.kind, Some(&c), &arg) {
            m.flash_err(&format!(
                "Unknown permission mode {arg} · #perm shows supported modes"
            ));
            return Out::Draw;
        }
        return set_permission(m, key, &arg);
    }
    let opts: Vec<choice::Choice> = permission_choices(m, &c.kind, Some(&c))
        .into_iter()
        .map(|v| choice::Choice {
            id: v.id,
            note: v.note,
        })
        .collect();
    if opts.is_empty() {
        m.flash_err("This harness does not advertise permission modes");
        return Out::Draw;
    }
    let key = key.to_string();
    let s = ChoiceSheet::new(
        "permissions",
        "what the agent may do without asking",
        opts,
        Box::new(move |m, ch| set_permission(m, &key, &ch.id)),
    )
    .running(&c.permission_mode);
    m.open_sheet(Box::new(s));
    Out::Draw
}

trait MaxDraw {
    fn max_draw(self) -> Out;
}

impl MaxDraw for Out {
    /// Idle as Draw: a flash was set.
    fn max_draw(self) -> Out {
        if self == Out::Idle { Out::Draw } else { self }
    }
}
