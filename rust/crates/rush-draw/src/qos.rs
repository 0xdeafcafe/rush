//! The screen's threads ahead of the machine's other work. A CLI's
//! threads run at macOS's default class, the same as every compiler an
//! agent starts, so with a few builds going rush waited its turn behind
//! them for each frame and keystroke. User-interactive is the class an
//! app's main thread has. Programs started from a thread take on its
//! class: the host and the plugin broker go back to the default as they
//! start (rush_sys::qos::plain), so the agents and what they run don't.

/// Puts the calling thread in macOS's user-interactive class; elsewhere
/// does nothing.
pub(crate) fn interactive() {
    #[cfg(target_os = "macos")]
    // SAFETY: takes plain values and changes only the calling thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}
