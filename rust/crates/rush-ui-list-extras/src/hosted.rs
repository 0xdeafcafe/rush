//! The hosted view's one key the list keeps: ctrl+6 shows and hides
//! Agents beside a Session. After Go's internal/ui/hosted.go:82
//! (listToggleKey). The hosted view itself (NewHosted, one session
//! alone, embedded in another app) waits on rush hosting sessions.

/// Whether s is the key that shows and hides Agents beside a Session:
/// ctrl+6, which terminals send as ctrl+^ unless they speak the kitty
/// keyboard protocol.
pub fn list_toggle_key(s: &str) -> bool {
    matches!(s, "ctrl+6" | "ctrl+^" | "ctrl+shift+6")
}
