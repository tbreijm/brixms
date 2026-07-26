//! Persistent key→value store with structural sharing (ADR-0002 §9.2
//! "State"):
//!
//! > Persistent HAMT-style maps with structural sharing.
//!
//! **Dependency-policy note (read before touching this module).** The
//! eventual target representation is a HAMT (hash array mapped trie) with
//! true node-level structural sharing on `insert`. This crate does **not**
//! pull in `im` or any other persistent-collections crate — the workspace
//! Ring-0 whitelist (root `Cargo.toml` `[workspace.dependencies]`) has no
//! entry for one, and ADR-0002 §3 restricts this crate's substrate to
//! `brix-canon`/`brix-semantic` only. Instead, [`PersistentMap`] is a small
//! trait the representation lives behind, and [`ArcMap`] is the **v1**
//! implementation: an `Arc`-wrapped immutable `BTreeMap` snapshot.
//!
//! `ArcMap::insert` clones the whole map's entries into a fresh `Arc`
//! (O(n) pointer-sized clones of `BTreeMap`'s nodes, not a HAMT's O(log n)
//! per-node sharing) — but it is **correct**: the receiver (`&self`, not
//! `&mut self`) is left completely unchanged, and every other snapshot
//! holding the same `Arc` is unaffected and unaware an update happened
//! elsewhere. That is the structural-sharing *property* the Step 2 (E1) gate
//! in `Build_Plan_v3_SOC.md` checks ("store persistence/sharing property:
//! old snapshot unchanged after an update on a derived one") — this module's
//! tests exercise exactly that. `BTreeMap` (not `HashMap`) keeps it
//! deterministic (Ring0 §0, `clippy.toml` `disallowed-types`). Swapping in a
//! real HAMT later is a matter of adding another `PersistentMap` impl; call
//! sites are written against the trait, not `ArcMap` directly.

use std::collections::BTreeMap;
use std::sync::Arc;

/// A persistent (immutable, versioned) key→value map. `insert` takes `&self`
/// and returns a *new* map value — the receiver is untouched, so every
/// snapshot a caller still holds remains valid after another caller derives
/// a new version from it. This is the seam the ADR-0002 §9.2 HAMT
/// requirement swaps in behind (see module docs for the v1/HAMT trade-off).
pub trait PersistentMap<K, V>: Clone {
    /// The empty map.
    fn new() -> Self;
    /// Look up `key`.
    fn get(&self, key: &K) -> Option<&V>;
    /// Return a new map with `key` bound to `value`; `self` is unchanged.
    fn insert(&self, key: K, value: V) -> Self;
    /// Number of bindings.
    fn len(&self) -> usize;
    /// Whether the map has no bindings.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The **v1** [`PersistentMap`]: an `Arc`-shared immutable `BTreeMap`
/// snapshot. See the module docs for why this representation was chosen
/// over pulling in an external persistent-collections crate, and what the
/// eventual HAMT swap-in looks like.
#[derive(Debug)]
pub struct ArcMap<K, V>(Arc<BTreeMap<K, V>>);

impl<K, V> Clone for ArcMap<K, V> {
    /// O(1): bumps the `Arc` refcount, does not touch the map's contents.
    fn clone(&self) -> Self {
        ArcMap(Arc::clone(&self.0))
    }
}

impl<K, V> ArcMap<K, V> {
    /// Whether two snapshots share the same underlying allocation (`Arc`
    /// pointer equality). A diagnostic/test helper, not part of the
    /// [`PersistentMap`] contract — a HAMT swap-in would define its own,
    /// finer-grained notion of "shares structure with".
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<K: Ord + Clone, V: Clone> PersistentMap<K, V> for ArcMap<K, V> {
    fn new() -> Self {
        ArcMap(Arc::new(BTreeMap::new()))
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.0.get(key)
    }

    fn insert(&self, key: K, value: V) -> Self {
        // Clone-on-write: the old `Arc<BTreeMap<..>>` — and everyone still
        // holding it — is left exactly as it was.
        let mut next = (*self.0).clone();
        next.insert(key, value);
        ArcMap(Arc::new(next))
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_returns_new_snapshot_old_unchanged() {
        let m0: ArcMap<u32, &str> = ArcMap::new();
        let m0 = m0.insert(1, "a");
        let m1 = m0.insert(2, "b");

        assert_eq!(m0.get(&1), Some(&"a"));
        assert_eq!(
            m0.get(&2),
            None,
            "old snapshot must not observe an update made on a derived map"
        );
        assert_eq!(m1.get(&1), Some(&"a"));
        assert_eq!(m1.get(&2), Some(&"b"));
        assert_eq!(m0.len(), 1);
        assert_eq!(m1.len(), 2);
    }

    #[test]
    fn clone_is_o1_arc_share_insert_diverges() {
        let m0: ArcMap<u32, u32> = ArcMap::new().insert(1, 10);
        let m0_clone = m0.clone();
        assert!(
            m0.ptr_eq(&m0_clone),
            "clone must share the allocation (O(1) Arc bump), not deep-copy"
        );

        let m1 = m0.insert(2, 20);
        assert!(
            !m0.ptr_eq(&m1),
            "insert must produce a distinct snapshot allocation"
        );
        assert_eq!(m0.get(&2), None, "m0 must still be the pre-insert snapshot");
    }

    #[test]
    fn overwrite_replaces_value_for_existing_key_in_the_new_snapshot_only() {
        let m0: ArcMap<u32, u32> = ArcMap::new().insert(1, 10);
        let m1 = m0.insert(1, 20);
        assert_eq!(m0.get(&1), Some(&10));
        assert_eq!(m1.get(&1), Some(&20));
    }

    #[test]
    fn empty_map_has_no_bindings() {
        let m: ArcMap<u32, u32> = ArcMap::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert_eq!(m.get(&1), None);
    }
}
