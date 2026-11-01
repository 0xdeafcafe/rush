//! A process under a session's opened task, as the Tasks box lists it
//! (session_core/jobprocs.rs reads them).

/// A process under an opened task: its program, its words, and what it
/// uses now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcRow {
    pub pid: i32,
    /// Its program's name (filepath.Base of its comm).
    pub comm: String,
    /// Its words, short (shortCmd).
    pub cmd: String,
    pub cpu: f64,
    pub footprint: u64,
}
