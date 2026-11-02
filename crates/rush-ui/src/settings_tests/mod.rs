//! Settings' tests that need the whole UI (new_model, the other screens
//! registered): its pages, keys, sheets and their words, moved here with
//! the screen to rush-ui-settings. The pure ones stay in that crate.

// Settings' names, as its mod.rs had them in scope for `use super::*`.
#[allow(unused_imports)]
pub use rush_ui_settings::*;

#[allow(unused_imports)]
use std::cell::Cell;
#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use std::rc::Rc;

#[allow(unused_imports)]
use crate::frame::{Ext, Mouse, Out, Place, PlaceId, Press};
#[allow(unused_imports)]
use crate::model::Model;
#[allow(unused_imports)]
use crate::screens::social::stream::ModelStream;
#[allow(unused_imports)]
use crate::style::*;
#[allow(unused_imports)]
use rush_core::text::Line;

mod tests;
mod tests_config;
mod tests_own;
mod tests_providers;
mod tests_remote;
