//! rush's own idea of a coding agent and its work, apart from any one
//! agent, plus the styled text every screen is drawn from. Adapters (Claude
//! Code, Codex, …) fill these types in; nothing here knows which one it came
//! from. No terminal or platform code lives here, so it is tested and
//! benchmarked on its own.

pub mod adapter;
pub mod agent;
pub mod event;
pub mod failure;
pub mod sandbox;
pub mod text;
pub mod theme;
pub mod time;
pub mod tool;
pub mod usage;
pub mod wire;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
