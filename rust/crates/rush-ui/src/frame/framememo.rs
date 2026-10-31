//! Rust-only: what several parts of one frame each work out the same way,
//! worked out once while that frame is drawn. The header, the bars and the
//! box's shelf each ask which accounts there are and which have room
//! (accounts/rows.rs account_rows, room), and each answer reads every
//! agent's sign-in folders from disk; Go asks again each time too. Nothing
//! a frame draws changes the accounts, so its first answer holds for the
//! rest of it. Outside a frame (keys, notes) every ask is worked out
//! afresh, and anything that changes the accounts' state forgets what was
//! kept (accounts/state.rs accounts). The list's widest name, which the
//! column header and the rows each measure over every agent (list.rs
//! widest_name), is kept the same way, forgotten when the list's lines are
//! made again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rush_state::Room;

use crate::screens::accounts::rows::AcctRow;

/// A frame being drawn ([`FrameMemo::drawing`]).
pub struct Drawing(Rc<FrameMemo>);

impl Drop for Drawing {
    fn drop(&mut self) {
        self.0.end();
    }
}

#[derive(Default)]
pub struct FrameMemo {
    /// Frames being drawn (a frame drawn inside one counts once).
    depth: Cell<u32>,
    account_rows: RefCell<Option<Vec<AcctRow>>>,
    room: RefCell<Option<Room>>,
    widest_name: RefCell<Option<usize>>,
}

impl FrameMemo {
    /// A frame starts being drawn.
    pub fn begin(&self) {
        if self.depth.get() == 0 {
            self.forget();
        }
        self.depth.set(self.depth.get() + 1);
    }

    /// The frame begun last is drawn.
    pub fn end(&self) {
        let d = self.depth.get().saturating_sub(1);
        self.depth.set(d);
        if d == 0 {
            self.forget();
        }
    }

    /// Begins a frame, ended when what's returned is dropped (a panic
    /// drawing it too).
    pub fn drawing(m: &Rc<FrameMemo>) -> Drawing {
        m.begin();
        Drawing(m.clone())
    }

    /// Forgets what's kept: the accounts' state changed.
    pub fn forget(&self) {
        self.account_rows.borrow_mut().take();
        self.room.borrow_mut().take();
        self.forget_widest_name();
    }

    /// Forgets the list's widest name: its lines were made again.
    pub fn forget_widest_name(&self) {
        self.widest_name.borrow_mut().take();
    }

    pub fn account_rows(&self, f: impl FnOnce() -> Vec<AcctRow>) -> Vec<AcctRow> {
        self.get(&self.account_rows, f)
    }

    pub fn room(&self, f: impl FnOnce() -> Room) -> Room {
        self.get(&self.room, f)
    }

    pub fn widest_name(&self, f: impl FnOnce() -> usize) -> usize {
        self.get(&self.widest_name, f)
    }

    fn get<T: Clone>(&self, cell: &RefCell<Option<T>>, f: impl FnOnce() -> T) -> T {
        if self.depth.get() == 0 {
            return f();
        }
        if let Some(v) = cell.borrow().as_ref() {
            return v.clone();
        }
        let v = f();
        *cell.borrow_mut() = Some(v.clone());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_only_while_a_frame_is_drawn() {
        let m = FrameMemo::default();
        let n = Cell::new(0);
        let ask = || {
            m.room(|| {
                n.set(n.get() + 1);
                Room::new()
            })
        };
        ask();
        ask();
        assert_eq!(n.get(), 2, "outside a frame: asked each time");
        m.begin();
        ask();
        ask();
        m.begin();
        ask();
        m.end();
        ask();
        assert_eq!(n.get(), 3, "in a frame: once");
        m.forget();
        ask();
        assert_eq!(n.get(), 4, "forgotten when the accounts change");
        m.end();
        ask();
        ask();
        assert_eq!(n.get(), 6, "the frame drawn: asked each time again");
    }
}
