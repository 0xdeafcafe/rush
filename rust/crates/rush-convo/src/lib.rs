//! A transcript as a conversation: a Session of turns, items and steps
//! built from rush_core::event::Event (a hosted session's, or a transcript
//! the Tail reads back), which a renderer draws. No terminal or platform
//! code; see DESIGN.md.
//!
//! Port of internal/convo's model and transcript reading (model.go,
//! transcript.go, neutral.go, words.go, usermessages.go) and its rendering
//! (render.go and what it draws with). Tool calls are rush_core::tool's.

pub mod decisions;
pub mod model;
pub mod render;
pub mod search;
pub mod text;
pub mod transcript;
pub mod words;

pub use model::{
    Asking, Compacted, ExchangeItem, Hook, Info, Interject, Item, Job, Picture, Request, Session,
    Status, Step, StepId, Task, ToolStat, Turn, exchange_peer, peer_label,
};
pub use render::{Depth, Options, RenderedLine, Renderer};
pub use search::{Found, Hit, Library, WHO_AGENT, library};
pub use transcript::{Decode, Fresh, Tail, decode, history, history_from, history_start, injected};
