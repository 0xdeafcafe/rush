//! Getting started.
use rush_core::text::Line;
use crate::frame::{Out, Press};
use crate::model::Model;
pub fn note_progress(_m: &mut Model) {}
pub fn dock(_m: &mut Model, _w: usize) -> Vec<Line> { Vec::new() }
pub fn tips_command(_m: &mut Model, _a: &str) -> Out { Out::Idle }
pub fn saw_key(_m: &mut Model, _k: &Press) {}
