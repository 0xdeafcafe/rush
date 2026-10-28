//! Agents talking among themselves where you can see: rooms, a group
//! chat of fresh agents on one topic that ends on a verdict (`room`, its
//! driver and `rush room`), and the feed, the shared local board agents
//! chirp to (`community` and `rush feed`). Both keep Go rush's files, so
//! either rush reads and writes the other's.
//!
//! Ported from internal/room, internal/community, cmd/rush/room.go and
//! cmd/rush/community.go. What a room asks of rush's sessions (starting,
//! dialling, their events) goes through [`driver::Sessions`]; what it
//! asks of the agents rush knows, through [`Agents`].

pub mod cli;
pub mod community;
pub mod doing;
pub mod driver;
pub mod feed;
pub mod room;

pub use cli::{MemberConfig, ROOM_USAGE, RoomCtx, room_cmd, room_line};
pub use community::{Author, Feed, Message, TagCount, Thread};
pub use driver::{MemberEvent, Msg, RetryInfo, Session, SessionInfo, Sessions};
pub use feed::{FEED_USAGE, FeedCtx, feed_cmd};
pub use room::{Entry, Lister, Member, Room, Rooms, Summary};

/// What a room asks of the agents rush knows.
pub trait Agents: Send + Sync {
    /// The program agent kind runs in, by name: Claude Code for Ollama in
    /// Claude Code (agent.HarnessLabel).
    fn harness_label(&self, kind: &str) -> String;
    /// model as agent kind says it, or model as it is (agent.ModelName).
    fn model_name(&self, kind: &str, model: &str) -> String;
    /// Whether agent kind takes a message mid-turn
    /// (agent.Supports(FeatureGuide)).
    fn supports_guide(&self, kind: &str) -> bool;
}

/// The agents registered with rush_core's registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct Registered;

impl Agents for Registered {
    fn harness_label(&self, kind: &str) -> String {
        rush_core::adapter::global().harness_label(kind)
    }
    fn model_name(&self, kind: &str, model: &str) -> String {
        rush_core::adapter::global().model_name(kind, model)
    }
    fn supports_guide(&self, kind: &str) -> bool {
        rush_core::adapter::global().supports(kind, rush_core::adapter::Feature::Guide)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::Agents;

    /// No adapters registered: a harness's name is its kind's, and models
    /// go by their ids.
    pub struct NoAgents;

    impl Agents for NoAgents {
        fn harness_label(&self, kind: &str) -> String {
            kind.to_string()
        }
        fn model_name(&self, _: &str, model: &str) -> String {
            model.to_string()
        }
        fn supports_guide(&self, kind: &str) -> bool {
            kind == "claude"
        }
    }
}
