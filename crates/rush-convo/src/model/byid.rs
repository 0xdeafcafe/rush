//! A session's steps by their call's id (Rust-only): the index holds each
//! step's number only, and finds it by the id the step itself holds, so
//! an id is kept once, not again as the index's key.

use std::hash::BuildHasher;

use hashbrown::HashTable;

use super::{Step, StepId};

#[derive(Clone, Default)]
pub(crate) struct ById {
    table: HashTable<StepId>,
    hasher: std::hash::RandomState,
}

impl std::fmt::Debug for ById {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ById({})", self.table.len())
    }
}

impl ById {
    /// The step whose call has this id, among steps.
    pub(crate) fn get(&self, steps: &[Step], id: &str) -> Option<StepId> {
        let h = self.hasher.hash_one(id);
        self.table
            .find(h, |&sid| &*steps[sid as usize].id == id)
            .copied()
    }

    pub(crate) fn contains(&self, steps: &[Step], id: &str) -> bool {
        self.get(steps, id).is_some()
    }

    /// Indexes step sid by its id, replacing the step that had it.
    pub(crate) fn insert(&mut self, steps: &[Step], sid: StepId) {
        let id = &*steps[sid as usize].id;
        let h = self.hasher.hash_one(id);
        let hasher = &self.hasher;
        match self.table.entry(
            h,
            |&x| &*steps[x as usize].id == id,
            |&x| hasher.hash_one(&*steps[x as usize].id),
        ) {
            hashbrown::hash_table::Entry::Occupied(mut e) => *e.get_mut() = sid,
            hashbrown::hash_table::Entry::Vacant(e) => {
                e.insert(sid);
            }
        }
    }
}
