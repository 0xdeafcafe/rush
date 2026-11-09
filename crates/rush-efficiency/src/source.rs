//! Where efficiency's figures come from: an agent whose transcripts it
//! reads, and whose setup savers are found in. Port of
//! internal/efficiency/source.go.

use std::collections::HashMap;

use rush_core::adapter::Profile;
use rush_state::Time;
use rush_state::settingsfile;

use crate::scan::File;

/// An agent whose transcripts efficiency reads: it lists them, and reads
/// each line of one into a File through File's saw, request, call, result
/// and the rest.
pub trait Source: Send + Sync {
    /// The agent's kind, as its profiles name it: "claude", "codex".
    fn kind(&self) -> &'static str;
    /// p's transcripts: its sessions' and their subagents', whose paths
    /// have a "subagents" folder in them.
    fn transcripts(&self, p: &Profile) -> Vec<String>;
    /// The folder all of p's transcripts are under.
    fn transcripts_dir(&self, p: &Profile) -> String;
    /// Reads one whole line of a transcript into f.
    fn read_line(&self, f: &mut File, line: &[u8]);
    /// What's set up in p's home, for savers to be found in.
    fn setup(&self, p: &Profile) -> Setup;
}

/// An agent's setup in one home, as far as savers show in it.
#[derive(Default)]
pub struct Setup {
    /// Its own settings.
    pub settings: Option<settingsfile::File>,
    /// Every hook's command.
    pub hooks: Vec<String>,
    /// Plugins turned on, by id.
    pub enabled: HashMap<String, bool>,
    /// Plugins installed, by id: when, if known.
    pub plugins: HashMap<String, Time>,
    /// MCP servers, by name.
    pub mcp: HashMap<String, bool>,
    /// The files an install may change, copied first.
    pub backup: Vec<String>,
    /// The environment its CLI runs in for that home.
    pub env: Vec<String>,
}

/// Claude Code's Source: the one savers are found in, and the one a
/// profile that names no agent reads through.
pub fn source() -> &'static dyn Source {
    &crate::claude::Claude
}

/// Every agent efficiency reads the transcripts of: Claude Code's, then
/// Codex's (Rust-only).
pub fn sources() -> &'static [&'static dyn Source] {
    &[&crate::claude::Claude, &crate::codex::Codex]
}

/// The Source of p's agent, Claude Code's for a profile that names none;
/// None for an agent whose transcripts efficiency doesn't read.
pub fn source_for(p: &Profile) -> Option<&'static dyn Source> {
    let k = p.kind.0.as_str();
    if k.is_empty() {
        return Some(source());
    }
    sources().iter().copied().find(|s| s.kind() == k)
}

/// p's transcripts, as its agent's Source finds them.
pub fn transcripts(p: &Profile) -> Vec<String> {
    source_for(p).map(|s| s.transcripts(p)).unwrap_or_default()
}

/// The folder all of p's transcripts are under.
pub fn transcripts_dir(p: &Profile) -> String {
    source_for(p)
        .map(|s| s.transcripts_dir(p))
        .unwrap_or_default()
}
