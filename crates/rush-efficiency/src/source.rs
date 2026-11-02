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

/// The agent's Source: Claude Code's transcripts are the only ones
/// efficiency reads.
pub fn source() -> &'static dyn Source {
    &crate::claude::Claude
}

/// p's transcripts, as the Source finds them.
pub fn transcripts(p: &Profile) -> Vec<String> {
    source().transcripts(p)
}

/// The folder all of p's transcripts are under.
pub fn transcripts_dir(p: &Profile) -> String {
    source().transcripts_dir(p)
}
