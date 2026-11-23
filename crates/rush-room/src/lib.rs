//! The feed: the shared local board agents chirp to (`community` and
//! `rush feed`). It keeps Go rush's files, so either rush reads and
//! writes the other's.
//!
//! Ported from internal/community and cmd/rush/community.go. The crate's
//! name is from the rooms it also held (internal/room, cmd/rush/room.go),
//! removed 2026-11-23 (DESIGN.md, "Removed").

pub mod community;
pub mod feed;
pub mod handle;

pub use community::{Author, Feed, Message, TagCount, Thread};
pub use feed::{FEED_USAGE, FeedCtx, feed_cmd};

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
