//! Each session's name as rush lists it, by the codename other sessions
//! know it by (rush-8a, as ListAgents gives it). Port of
//! internal/convo/messages.go:66-90 (peers, SetPeers).

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

static PEERS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// Who each codename is, as the list names them now. What was drawn
/// before they changed is drawn again (SetPeers).
pub fn set_peers(names: HashMap<String, String>) {
    let mut p = PEERS.write().unwrap_or_else(PoisonError::into_inner);
    if p.as_ref() == Some(&names) {
        return;
    }
    *p = Some(names);
    crate::linkinfo::bump_gen();
}

/// The name the list gives the session known by codename to.
pub(crate) fn name_of(to: &str) -> Option<String> {
    let p = PEERS.read().unwrap_or_else(PoisonError::into_inner);
    p.as_ref()?.get(to).cloned()
}
