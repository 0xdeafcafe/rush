//! A conversation's end read ahead, as a view opened takes it: the
//! frame's side of screens/list_extras/predict.rs, which reads them.

use rush_convo::{Session, Tail};
use rush_core::adapter::Follow;

/// One taken for a view: fresh when nothing was written since it was read.
pub struct Hit {
    pub fresh: bool,
    pub tail: Box<Tail>,
    pub sess: Box<Session>,
}

/// A history read ahead, taken for a view.
pub struct HistHit {
    pub fresh: bool,
    pub follow: Box<dyn Follow>,
    pub sess: Box<Session>,
}
