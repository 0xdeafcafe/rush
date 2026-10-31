//! Giving memory back to the system once a large thing is let go. The
//! allocator keeps what was freed for the next allocation, so after a long
//! conversation was read and closed rush went on holding pages it had no
//! use for; Go's collector hands them back as it sweeps (and rush's host
//! asks it to, debug.FreeOSMemory). Here the freeing and the asking happen
//! on a thread of their own, so the UI never waits on either.

/// Drops v on a short-lived thread of its own, then gives the memory freed
/// back to the system. Dropping a long conversation is thousands of frees:
/// the thread that let it go goes on at once.
pub fn let_go<T: Send + 'static>(v: T) {
    // No thread to be had: v goes with the closure, freed here as it
    // would have been.
    let _ = std::thread::Builder::new()
        .name("rush-let-go".into())
        .spawn(move || {
            drop(v);
            give_back();
        });
}

/// Asks the allocator to return the pages it holds free.
pub fn give_back() {
    #[cfg(target_os = "macos")]
    // SAFETY: a null zone asks every zone; it only returns free pages.
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: trims only what's free at the top of the heap and in arenas.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: libc::size_t) -> libc::size_t;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Flag(Arc<AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn what_is_let_go_is_dropped() {
        let dropped = Arc::new(AtomicBool::new(false));
        super::let_go(Flag(dropped.clone()));
        for _ in 0..500 {
            if dropped.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never dropped");
    }
}
