//! The small providers rush knows, each a rush_core Adapter: DeepSeek's
//! own coding agent, dsh (`deepseek`), and one provider's models in
//! another's harness (`cross`): DeepSeek and GLM in Claude Code, and
//! Anthropic, OpenAI, DeepSeek and GLM in Pi.
//!
//! GLM through ZCode (Go's internal/adapters/glm) rides ACP, and is
//! rush-acp's to port.

pub mod cross;
pub mod deepseek;

use std::sync::Arc;

use rush_core::adapter::Registry;

pub use cross::Pair;
pub use deepseek::DeepSeek;

/// Reads an environment variable: the process's, or a test's own.
pub type Getenv = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The process's environment, an empty variable as none.
pub fn process_env() -> Getenv {
    Arc::new(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

/// Registers every adapter here with reg (Go's init funcs).
pub fn register(reg: &Registry) {
    reg.register(DeepSeek::new(reg.clone()));
    for p in cross::pairs(reg) {
        reg.register(p);
    }
}

/// The home folder, as getenv has it.
pub(crate) fn home_dir(getenv: &Getenv) -> Option<String> {
    getenv("HOME")
}
