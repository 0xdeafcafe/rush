//! What's known of newer rushes, kept for the screens that show or act on
//! it: social's services find it out (checkUpdate, installUpdate), and
//! Settings' Updates page shows it.

use rush_update::Release;

/// What's known of newer rushes.
#[derive(Default)]
pub struct Update {
    /// A newer rush that's out.
    pub newer: Option<Release>,
    pub updating: bool,
    pub checked: bool,
    /// A newer rush than this one is installed: #reload runs it.
    pub rebuilt: bool,
    /// When the installed rush was written, as the last look at the hosts
    /// found it: a session whose host runs an older one shows "old build".
    /// 0 unread. Rust-only.
    pub installed: rush_core::time::Ms,
}
