//! rush-ui-session-core's tests: they make a whole UI model (new_model), so
//! they are here. The crate's modules are in scope as `super::..`, as they
//! were beside the code.

pub use rush_ui_session_core::*;

mod tests;
mod tests_cards;
mod tests_changes_inline;
mod tests_clicks;
mod tests_cold;
mod tests_drops;
mod tests_exchange_inline;
mod tests_finish_inline;
mod tests_grid;
mod tests_header;
mod tests_jobs;
mod tests_move;
mod tests_next;
mod tests_pastes;
mod tests_plugins;
mod tests_scripts;
mod tests_slash;
mod tests_split;
mod tests_start;
mod tests_subs;
mod tests_subview;
mod tests_switch;
mod tests_tasks;
mod tests_turnwants_inline;
mod tests_undo;
