//! Helpers the tests of several screens share, here because only rush-ui
//! can make a whole model. Not for the binary.

use crate::model::Model;
use rush_ui_social::stream::Stream;

/// A whole UI model, w by h, on a fixed clock with the feed on: rush's
/// folder is somewhere with no board, never the real one.
pub fn model(w: usize, h: usize) -> Model {
    rush_state::set_thread_dir(Some(std::path::PathBuf::from(
        "/nonexistent/rush-test-home",
    )));
    let mut m = crate::new_model("test");
    m.w = w;
    m.h = h;
    m.clock = Some(1_790_000_000_000);
    m.kept::<Stream>().on = Some(true);
    m
}
