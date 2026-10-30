//! What's untidy in the memory a session loads, as findings. Port of
//! internal/efficiency/memory.go.

use std::path::Path;

use rush_core::adapter::{Profile, Registry, global};

use crate::findings::Finding;

/// What's untidy in the memory and instructions a session of p's in cwd
/// loads, as its agent's MemoryReader says: each says what to do, and
/// enter opens the file to do it in.
pub fn memory_findings(p: &Profile, cwd: &str) -> Vec<Finding> {
    memory_findings_in(global(), p, cwd)
}

/// memory_findings, through reg's adapters.
pub fn memory_findings_in(reg: &Registry, p: &Profile, cwd: &str) -> Vec<Finding> {
    if p.dir.is_empty() || cwd.is_empty() {
        return Vec::new();
    }
    let Some(a) = reg.get(p.kind.as_str()) else {
        return Vec::new();
    };
    let Some(mr) = a.memory_reader() else {
        return Vec::new();
    };
    let base = Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    mr.memory(p, cwd, "")
        .problems
        .into_iter()
        .enumerate()
        .map(|(i, pr)| Finding {
            title: pr.title,
            detail: format!("In {base}: {}", pr.fix),
            open: pr.path,
            rank: 2 + i as i32,
            ..Finding::default()
        })
        .collect()
}
