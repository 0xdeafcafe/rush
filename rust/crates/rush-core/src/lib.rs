//! rush's own idea of a coding agent and its work, apart from any one
//! agent, plus the styled text every screen is drawn from. Adapters (Claude
//! Code, Codex, …) fill these types in; nothing here knows which one it came
//! from. No terminal or platform code lives here, so it is tested and
//! benchmarked on its own.

pub mod agent;
pub mod message;
pub mod text;
pub mod theme;
pub mod time;
