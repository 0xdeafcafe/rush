//! A session's permission modes: those its harness says it has, the
//! don't-ask one the approval card's o and #yolo switch to, and changing
//! it. After Go's internal/ui/permissions.go (permissionChoices :12,
//! permissionKnown :31, bypassMode :42, setPermission :51,
//! permissionCommand :79). How much a mode trusts (choices.go Trust,
//! TrustedAs) is rush-core's adapter::trust and trusted_as.
//! #perm with nothing named opens the setup sheet on its Permissions row,
//! to apply only (permissions.go:105-111).

use rush_core::adapter::{Choice, trust};

use super::{Act, HostInfo, act, info};
use crate::frame::Out;
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
        .find(|v| trust(&v.id) == 3)
        .map(|v| v.id)
        .unwrap_or_default()
}

/// Switches agent key's hosted session to mode (setPermission).
pub fn set_permission(m: &mut Model, key: &str, mode: &str) -> Out {
    use crate::screens::session_core::{agent, switch};
    // A host gone to sleep is woken first, then asked (setPermission's
    // wakeHostThen, permissions.go:60).
    if m.kept
        .peek::<crate::screens::session_core::Core>()
        .and_then(|c| c.conns.get(key))
        .is_some_and(|c| !c.hosted() && c.sleeping)
        && let Some(a) = agent(m, key).cloned()
    {
        let (key, mode) = (key.to_string(), mode.to_string());
        return switch::wake_then(m, &a, move |m| set_permission(m, &key, &mode));
    }
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

/// #perm [mode] and #yolo on agent key's session (permissionCommand).
pub fn permission_command(m: &mut Model, key: &str, arg: &str, yolo: bool) -> Out {
    perm_command(m, Some(key), arg, yolo)
}

/// #perm [mode] and #yolo: on the open session key, or the next session
/// when none is. A mode named (#yolo: the harness's bypass mode) is set
/// if its harness has it; nothing named opens the setup sheet on its
/// Permissions row, to apply only (permissionCommand).
pub fn perm_command(m: &mut Model, key: Option<&str>, arg: &str, yolo: bool) -> Out {
    use crate::screens::accounts::{setup, startsheet};
    let c = key.and_then(|k| info(m, k));
    let next = |m: &Model| {
        let dir = crate::screens::list_extras::prompt::start_dir(m);
        setup::next_start(m, &dir)
    };
    let mut o = match key {
        Some(k) => crate::screens::bars::agent::session_start(m, k).unwrap_or_else(|| next(m)),
        None => next(m),
    };
    let mut arg = arg.trim().to_string();
    if yolo {
        if !arg.is_empty() {
            m.flash_err("#yolo takes no arguments; use #perm to choose a permission mode");
            return Out::Draw;
        }
        arg = bypass_mode(m, &o.kind, c.as_ref());
        if arg.is_empty() {
            m.flash_err("This harness does not advertise a bypass permission mode");
            return Out::Draw;
        }
    }
    if !arg.is_empty() {
        if !permission_known(m, &o.kind, c.as_ref(), &arg) {
            m.flash_err(&format!(
                "Unknown permission mode {arg} · #perm shows supported modes"
            ));
            return Out::Draw;
        }
        if let Some(k) = key {
            return set_permission(m, k, &arg);
        }
        o.mode = arg.clone();
        o.mode_set = true;
        crate::screens::accounts::state::accounts(m).start_over = Some(o);
        m.flash(&format!("Next session permissions: {arg}"));
        return Out::Draw;
    }
    let out = match key {
        Some(k) => startsheet::open_switch_sheet(m, k, o),
        None => startsheet::open_start_sheet(m),
    };
    if let Some(s) = m.sheet_as::<startsheet::StartSheet>() {
        s.row = 6;
        s.apply_only = true;
    }
    out
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
