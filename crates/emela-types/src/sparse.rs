//! A map keyed by arena indices that only holds the entries it was given.
//!
//! `la_arena::ArenaMap` is a `Vec<Option<V>>` as long as the largest index inserted. Inference
//! runs per group of definitions, and a group's expressions, patterns and locals have indices
//! spread across the whole program's arenas, so a group-local `ArenaMap` grows to the size of the
//! program and every group pays for it (allocating, zeroing, scanning, dropping). That made
//! checking quadratic in the number of definitions. [`SparseMap`] costs only the group's entries
//! and iterates in index order like `ArenaMap`, so passes that walk it keep their order.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use la_arena::{Idx, RawIdx};

pub(crate) struct SparseMap<IDX, V> {
    map: BTreeMap<u32, V>,
    _ty: PhantomData<IDX>,
}

fn raw<T>(idx: Idx<T>) -> u32 {
    u32::from(idx.into_raw())
}

fn idx<T>(raw: u32) -> Idx<T> {
    Idx::from_raw(RawIdx::from(raw))
}

impl<T, V> SparseMap<Idx<T>, V> {
    pub(crate) fn new() -> Self {
        SparseMap {
            map: BTreeMap::new(),
            _ty: PhantomData,
        }
    }

    pub(crate) fn insert(&mut self, id: Idx<T>, value: V) -> Option<V> {
        self.map.insert(raw(id), value)
    }

    pub(crate) fn get(&self, id: Idx<T>) -> Option<&V> {
        self.map.get(&raw(id))
    }

    /// Entries in index order.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = (Idx<T>, &V)> {
        self.map.iter().map(|(&k, v)| (idx(k), v))
    }
}

impl<IDX, V: Clone> Clone for SparseMap<IDX, V> {
    fn clone(&self) -> Self {
        SparseMap {
            map: self.map.clone(),
            _ty: PhantomData,
        }
    }
}

impl<T, V> Default for SparseMap<Idx<T>, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use la_arena::{Idx, RawIdx};

    use super::SparseMap;

    fn id(raw: u32) -> Idx<()> {
        Idx::from_raw(RawIdx::from(raw))
    }

    #[test]
    fn iterates_in_index_order_like_arena_map() {
        let mut map = SparseMap::new();
        for raw in [1_000_000, 3, 70_000, 0] {
            map.insert(id(raw), raw);
        }
        let order: Vec<u32> = map.iter().map(|(_, &v)| v).collect();
        assert_eq!(order, [0, 3, 70_000, 1_000_000]);
        assert_eq!(map.get(id(70_000)), Some(&70_000));
        assert_eq!(map.get(id(4)), None);
    }
}
