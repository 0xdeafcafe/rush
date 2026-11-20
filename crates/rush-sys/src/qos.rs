//! A program back in macOS's default class. rush's screen threads run as
//! user-interactive (rush-draw's qos), and a program takes on the class
//! of the thread that starts it: what rush starts for good (the host, the
//! plugin broker) calls [`plain`] as it starts, so the agents and the
//! builds they run aren't put ahead of everything else too.

/// Puts the calling thread in the default class; elsewhere does nothing.
/// Fit for a pre_exec: one system call, no locks, nothing allocated.
pub fn plain() {
    #[cfg(target_os = "macos")]
    // SAFETY: takes plain values and changes only the calling thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_DEFAULT, 0);
    }
}

/// Puts the calling thread in the background class, for housekeeping
/// nobody waits on: efficiency cores and throttled disk reads. Elsewhere
/// does nothing.
pub fn background() {
    #[cfg(target_os = "macos")]
    // SAFETY: takes plain values and changes only the calling thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
}
