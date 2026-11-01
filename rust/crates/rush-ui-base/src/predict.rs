//! A conversation's end read ahead, as a view opened takes it: the
//! frame's side of screens/list_extras/predict.rs, which reads them.

use rush_convo::Session;
use rush_convo::reading::Reading;

/// One taken for a view, whatever agent's: fresh when nothing was written
/// since it was read. A partial sess is only its end, the reading still at
/// its start; else sess is all of it, and the reading where it stopped.
pub struct Hit {
    pub fresh: bool,
    pub reading: Box<dyn Reading>,
    pub sess: Box<Session>,
}
