//! What the header says new sessions start on (providers.go:179
//! startTag). How each provider is called and looks is the frame's
//! (crate::look).

use rush_core::text::Line;

pub use crate::look::*;
use crate::model::Model;
use crate::style::*;

/// What the header says new sessions start on: the provider's glyph, the
/// account, and the profile when there's more than one or #preset picked
/// one (providers.go:179 startTag).
pub fn start_tag(m: &Model) -> Line {
    use super::rows::{start_account, start_dir, start_kind, start_profile};
    use super::state::{cfg, peek};
    let k = start_kind(m);
    let mut l = line([glyph(&m.reg, &k), plain(" "), sp(start_account(m), DIM)]);
    let p = start_profile(m, &start_dir(m)).name;
    if show_profile(cfg(m), &p) || !peek(m).profile.is_empty() {
        l.spans.push(sp(" · ", FAINT));
        l.spans.push(sp(p, FAINT));
    }
    l
}
