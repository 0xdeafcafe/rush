//! What a session's views are, as data: a panel an agent (or a plugin, or
//! your script) keeps beside its conversation with `rush view set`. The
//! types and files are rush-host's, as Go's internal/host/views.go:29-175
//! (View, Block, Item, Stat, Progress, Group, Row) and viewacts.go:22-46,
//! :103-117 (Action, Change, Yours, ItemKey, BlockKey, GroupKey) are the
//! host's; the UI reads them off its thread (read.rs).

pub use rush_host::viewacts::{Action, Change, Yours, block_key, group_key, item_key};
pub use rush_host::views::{ACCENTS, BLOCK_KINDS, Block, Group, Item, Progress, Row, Stat, View};
