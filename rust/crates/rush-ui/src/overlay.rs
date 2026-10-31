//! The one way the UI changes rush's state.json (the overlay: names, put
//! away, seen, hidden, moved). Go saves it on the UI thread, in order
//! (model.go SaveOverlay); here each change is queued in the order made
//! and written off the UI thread on top of state.json as it is, so two
//! quick changes never save the earlier value over the later one. As
//! config::save does for config.json.

use crate::frame::Out;
use crate::model::Model;

/// A change to state.json, waiting to be written; true when it changed
/// anything.
type Change = Box<dyn FnOnce(&mut rush_state::Overlay) -> bool + Send>;

/// The changes not yet written, in the order they were made.
static WAITING: std::sync::Mutex<std::collections::VecDeque<Change>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// One writer of state.json at a time, from whichever thread.
static KEEPING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn queue(f: Change) {
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(f);
}

/// Writes every change waiting, in the order they were made, on top of
/// state.json as it is: whichever thread gets here first writes them all,
/// so a later change never lands before an earlier one.
fn write_waiting() -> Result<(), String> {
    let _g = KEEPING.lock().unwrap_or_else(|e| e.into_inner());
    let changes: Vec<Change> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    if changes.is_empty() {
        return Ok(()); // an earlier thread wrote this one with its own
    }
    // A save still on its way to disk is there before it's read again.
    rush_state::flush().map_err(|e| e.to_string())?;
    let mut s = rush_state::Store::load();
    let mut changed = false;
    for f in changes {
        changed |= f(&mut s.overlay);
    }
    if !changed {
        return Ok(());
    }
    s.save_overlay().map_err(|e| e.to_string())
}

/// Changes state.json as f says (f says whether it changed anything),
/// queued in the order made and written off the UI thread; done hears
/// how the write went, on the UI thread.
pub fn save(
    m: &mut Model,
    f: impl FnOnce(&mut rush_state::Overlay) -> bool + Send + 'static,
    done: impl FnOnce(&mut Model, Result<(), String>) -> Out + Send + 'static,
) {
    queue(Box::new(f));
    m.later(write_waiting, done);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two changes made in quick succession land in the order made, even
    /// when the later one's writer gets there first: Go saves them in
    /// order on the UI thread (SaveOverlay).
    #[test]
    fn changes_land_in_the_order_made() {
        let name = |n: &'static str| {
            move |o: &mut rush_state::Overlay| {
                o.names.insert("overlay-order-test".into(), n.into());
                true
            }
        };
        queue(Box::new(name("first")));
        queue(Box::new(name("second")));
        // The second change's writer first: it writes both, in order.
        write_waiting().unwrap();
        write_waiting().unwrap();
        rush_state::flush().unwrap();
        let names = rush_state::Store::load().overlay.names;
        assert_eq!(
            names.get("overlay-order-test").map(String::as_str),
            Some("second")
        );
    }

    /// Changes from two threads at once both land: neither loads before
    /// the other saved and writes the old state.json back.
    #[test]
    fn changes_from_two_threads_both_land() {
        let hide = |k: &'static str| {
            std::thread::spawn(move || {
                queue(Box::new(move |o: &mut rush_state::Overlay| {
                    o.hidden.insert(k.into(), rush_state::Time::now());
                    true
                }));
                write_waiting()
            })
        };
        let (a, b) = (hide("overlay-two-a"), hide("overlay-two-b"));
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
        rush_state::flush().unwrap();
        let hidden = rush_state::Store::load().overlay.hidden;
        assert!(hidden.contains_key("overlay-two-a"), "{hidden:?}");
        assert!(hidden.contains_key("overlay-two-b"), "{hidden:?}");
    }

    /// Through the UI's way in: a rename then putting it away, made one
    /// after the other, both kept.
    #[test]
    fn save_keeps_both() {
        let mut m = Model::new("t");
        let key = "overlay-save-test";
        save(
            &mut m,
            move |o| {
                o.names.insert(key.into(), "renamed".into());
                true
            },
            |_, r| {
                r.unwrap();
                Out::Idle
            },
        );
        save(
            &mut m,
            move |o| {
                o.done.insert(key.into(), rush_state::Time::now());
                true
            },
            |_, r| {
                r.unwrap();
                Out::Idle
            },
        );
        rush_state::flush().unwrap();
        let o = rush_state::Store::load().overlay;
        assert_eq!(o.names.get(key).map(String::as_str), Some("renamed"));
        assert!(o.done.contains_key(key));
    }
}
