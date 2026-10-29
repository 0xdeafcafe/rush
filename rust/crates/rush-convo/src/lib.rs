//! A transcript as a conversation: a Session of turns, items and steps
//! built from rush_core::event::Event (a hosted session's, or a transcript
//! the Tail reads back), which a renderer draws. No terminal or platform
//! code; see DESIGN.md.
//!
//! Port of internal/convo's model and transcript reading (model.go,
//! transcript.go, shellwrites.go, neutral.go, words.go, usermessages.go) and its rendering
//! (render.go and what it draws with). Tool calls are rush_core::tool's.

pub mod decisions;
pub mod handles;
pub mod linkinfo;
pub mod model;
pub mod render;
pub mod runname;
pub mod search;
pub mod shellwrites;
pub mod subagents;
pub mod text;
pub mod transcript;
pub mod words;

pub use model::{
    Asking, Compacted, ExchangeItem, Helper, Hook, Info, Interject, Item, Job, PartProc, Picture,
    Request, RunningPart, Session, Shell, ShellProc, Status, Step, StepId, Task, ToolStat, Turn,
    exchange_peer, peer_label,
};
pub use render::{Depth, Options, RenderedLine, Renderer};
pub use runname::{other_harness, run_tag, set_peer_mark};
pub use search::{Found, Hit, Library, WHO_AGENT, library};
pub use shellwrites::{ShellWrite, shell_run, shell_writes};
pub use subagents::{Subagent, Subagents, list_subagents};
pub use transcript::{Decode, Fresh, Tail, decode, history, history_from, history_start, injected};

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
