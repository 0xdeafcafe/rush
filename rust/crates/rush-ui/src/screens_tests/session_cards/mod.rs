//! rush-ui-session-cards's tests: they make a whole UI model (new_model),
//! so they are here. The crate's modules are in scope as `super::..`, as
//! they were beside the code.

pub use rush_ui_session_cards::*;

// What the crate's root had in scope, which its tests used through super::*.
#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use std::sync::OnceLock;

#[allow(unused_imports)]
use rush_convo::{Session, StepId};
#[allow(unused_imports)]
use rush_core::event::Question;
#[allow(unused_imports)]
use rush_core::text::Line;
#[allow(unused_imports)]
use rush_draw::MouseKind;

#[allow(unused_imports)]
use crate::frame::{Ext, Mouse, Out};
#[allow(unused_imports)]
use crate::model::Model;
#[allow(unused_imports)]
use crate::style::*;

mod tests;
mod tests_compact;
mod tests_go;
mod tests_limit;
mod tests_more;
mod tests_narrow;
mod tests_remember;
mod tests_sheets;
mod tests_wrap;
