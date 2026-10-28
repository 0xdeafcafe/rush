//! A transcript as a conversation: the Tail reads a Claude Code transcript
//! (or the host's events) into a Session of turns, items and steps, which
//! a renderer draws. No terminal or platform code; see DESIGN.md.
//!
//! Port of internal/convo's model and transcript reading (model.go,
//! transcript.go, neutral.go, words.go), its rendering (render.go and
//! what it draws with) and internal/agent/tool.

pub mod decisions;
pub mod model;
pub mod render;
pub mod text;
pub mod tool;
pub mod transcript;
pub mod words;

pub use model::{
    Asking, Compacted, Event, Hook, Interject, Item, Part, Request, Session, Status, Step, StepId,
    Task, ToolStat, Turn, TurnEnd,
};
pub use render::{Depth, Options, RenderedLine, Renderer};
pub use transcript::{Decode, Fresh, Tail, history, history_from, history_start, injected};
