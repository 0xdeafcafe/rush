//! Which agent's own screen has the terminal (#full), and the profile an
//! agent runs in: the command bar's attach (cmdbar/attach.rs) sets them,
//! the preview's live mirror (list_extras/live.rs) reads them. After Go's
//! internal/ui/model.go:2663-2687 (attach), live.go:238 and outside.go.

use rush_core::adapter::{Profile, Registry};
use rush_core::agent::Agent;

use crate::model::Model;

/// Which session has the terminal (m.attached), and how the preview's
/// live mirror is let go.
#[derive(Default)]
pub struct Attached {
    /// The agent whose screen has the terminal; "" when none has.
    pub key: String,
    /// Lets go of the preview's live mirror (closeLive): set by the
    /// preview that keeps one.
    pub close_live: Option<fn(&mut Model)>,
}

/// The agent whose own screen has the terminal now, if one has: the
/// preview opens no live mirror meanwhile (live.go:238).
pub fn attached(m: &Model) -> Option<&str> {
    m.kept
        .peek::<Attached>()
        .map(|a| a.key.as_str())
        .filter(|k| !k.is_empty())
}

/// The profile a's session runs under, as its adapter knows it.
pub fn profile_of(reg: &Registry, a: &Agent) -> Profile {
    reg.get(a.kind.as_str())
        .and_then(|ad| {
            reg.profiles_of(&ad)
                .into_iter()
                .find(|p| p.name == a.job.account)
        })
        .unwrap_or_else(|| Profile {
            kind: a.kind.clone(),
            name: a.job.account.clone(),
            dir: String::new(),
        })
}

/// The profile a runs in (a.Acct), else the one its agent knows by its
/// account's name.
pub fn acct_of(m: &Model, a: &Agent) -> rush_core::adapter::Profile {
    if a.acct.name.is_empty() && a.acct.dir.is_empty() {
        profile_of(&m.reg, a)
    } else {
        a.acct.clone()
    }
}
