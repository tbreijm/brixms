//! The keyed calendar: `K = (phase/time, priority, canonical-digest
//! tie-break)`, the unique-key deliberation frontier `B^uk_{K,O}`, and
//! `select_K` (ADR-0002 §1 "Dynamics"; §8.1 "The commitment/deliberation
//! split (settled)"; §9.2 "Calendar"):
//!
//! > Independent of the `F_O` choice, SOC fixes the *architecture*:
//! > deliberation lives in the unique-key branching functor `B^uk_{K,O}`, and
//! > `select_K: B^uk_{K,O} ⇒ D_O` commits the least-key candidate.
//! > `K = (phase/time, priority, canonical-digest tie-break)`; the final
//! > component is a unique tie-break, so selection is total and
//! > deterministic. **This split is ratified.**
//! >
//! > Calendar = priority queue keyed by `K = (phase/time, priority,
//! > canonical-digest tie-break)`; least key commits (SOC `select_K`).
//!
//! This module supplies [`Key`] (the ordered tuple itself) and [`Frontier`]
//! (the `B^uk_{K,O}` unique-key deliberation structure: a keyed map that
//! *rejects* a duplicate key mapping to a different value, and pops the
//! least key as `select_K`). [`crate::commit`] builds one [`Frontier`] per
//! tick from the oracle-shared candidate enumeration and commits its least
//! key into the `D_O = 1 + O×X` coalgebra.

use brix_canon::{CanonWriter, Canonical, Digest};
use std::collections::BTreeMap;

/// `K = (phase, priority, tiebreak)` (ADR-0002 §8.1).
///
/// `#[derive(Ord)]` on a struct compares fields **in declaration order** —
/// `phase`, then `priority`, then `tiebreak` — which *is* exactly SOC's key
/// order: phase/time first, priority second, the canonical-digest tie-break
/// last and only consulted when the first two are equal.
///
/// **Ordering convention (caller-encoded):** smaller `priority` = more
/// urgent. `select_K` (via [`Frontier::select_least`]) always pops the
/// numerically least key, so a caller wanting "regime A always outranks
/// regime B at the same phase" encodes A's priority as a smaller number than
/// B's.
///
/// **Totality (ADR-0002 §8.1): "the final component is a unique tie-break, so
/// selection is total and deterministic."** `tiebreak` is a canonical
/// digest, and the calendar's discipline (documented on [`Frontier`]) is that
/// two *distinct* committed candidates in the same tick are keyed with
/// distinct tie-break digests — typically a digest of the candidate's own
/// canonical identity (e.g. its witness+successor handles resolved to
/// digests, or the candidate's own canonical encoding). As long as callers
/// uphold that (see [`Frontier`]'s unique-key discipline, which makes a
/// violation observable rather than silently mis-selecting), no two distinct
/// candidates ever produce an exactly-equal `Key`, so `select_K` always has
/// a unique least element to pick — `BTreeMap`'s total order over `Key`
/// guarantees this holds mechanically, not just by fixture luck.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Key {
    /// Phase/time component — coarsest-grained; ticks typically increment
    /// this monotonically so later ticks never preempt earlier ones.
    pub phase: u64,
    /// Priority within a phase. **Smaller = more urgent** (caller
    /// convention; see module/struct docs).
    pub priority: u64,
    /// The canonical-digest tie-break — the final, uniqueness-making
    /// component (ADR-0002 §8.1).
    pub tiebreak: Digest,
}

impl Key {
    /// Construct a key from its three SOC components.
    pub fn new(phase: u64, priority: u64, tiebreak: Digest) -> Self {
        Key {
            phase,
            priority,
            tiebreak,
        }
    }
}

impl Canonical for Key {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: phase, priority, tiebreak — the same order as
        // the struct's declared (and `Ord`-significant) field order.
        w.write_uint(self.phase);
        w.write_uint(self.priority);
        w.write_bytes(self.tiebreak.as_bytes());
    }
}

/// The `B^uk_{K,O}` unique-key discipline was violated: `key` was already
/// bound to `existing` in the [`Frontier`], and a different `attempted`
/// value was proposed for the *same* key. ADR-0002 §1/§8.1's `B^uk` is a
/// **unique-key** branching functor — a duplicate key MUST observe the same
/// successor; this is the type that reports when it doesn't.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyConflict<V> {
    pub key: Key,
    pub existing: V,
    pub attempted: V,
}

/// The unique-key deliberation frontier `B^uk_{K,O}` for one tick: a
/// `BTreeMap<Key, V>` (deterministic — Ring0 §0, never a `HashMap`) where
/// [`Frontier::insert`] enforces the B^uk invariant ("duplicate key ⇒ same
/// observed successor", ADR-0002 §1/§8.1) and [`Frontier::select_least`] is
/// `select_K`: it pops the globally least key, i.e. the natural
/// transformation `select_K: B^uk_{K,O} ⇒ D_O` applied to this frontier.
/// `None` from `select_least` on an empty frontier is exactly quiescence —
/// the `1` summand of `D_O = 1 + O×X` (see [`crate::commit::Committed`]).
#[derive(Clone, Debug)]
pub struct Frontier<V> {
    entries: BTreeMap<Key, V>,
}

impl<V> Frontier<V> {
    /// A fresh, empty frontier.
    pub fn new() -> Self {
        Frontier {
            entries: BTreeMap::new(),
        }
    }

    /// Number of distinct keys currently in the frontier.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the frontier holds no candidates.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<V> Default for Frontier<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone + PartialEq> Frontier<V> {
    /// Insert `value` at `key`, enforcing the B^uk unique-key discipline:
    ///
    /// - `key` is new: inserted, returns `Ok(true)`.
    /// - `key` already maps to a value **equal** to `value`: idempotent —
    ///   the frontier is unchanged, returns `Ok(false)`. Two enumeration
    ///   passes (or two candidates that happen to key identically *and*
    ///   observe the same successor) never corrupt the frontier.
    /// - `key` already maps to a value **different** from `value`: rejected
    ///   — inserting would violate "duplicate key ⇒ same observed
    ///   successor" (a keying bug, since `Key`'s tie-break is supposed to be
    ///   unique per distinct candidate, see [`Key`]'s docs). Returns
    ///   `Err(KeyConflict)`; the frontier is left exactly as it was.
    pub fn insert(&mut self, key: Key, value: V) -> Result<bool, KeyConflict<V>> {
        match self.entries.get(&key) {
            None => {
                self.entries.insert(key, value);
                Ok(true)
            }
            Some(existing) if *existing == value => Ok(false),
            Some(existing) => Err(KeyConflict {
                key,
                existing: existing.clone(),
                attempted: value,
            }),
        }
    }

    /// `select_K`: pop and return the frontier's globally least key/value
    /// pair — the committed choice for this tick. `None` means the frontier
    /// is empty: quiescence, the `1` summand of `D_O = 1 + O×X`.
    pub fn select_least(&mut self) -> Option<(Key, V)> {
        self.entries.pop_first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::Domain;

    fn digest(tag: &str) -> Digest {
        Digest::of(Domain::Value, tag.as_bytes())
    }

    #[test]
    fn select_k_picks_the_unique_least_key_by_digest_tiebreak() {
        // Two candidates share phase and priority; only the tie-break
        // differs — exactly the totality case ADR-0002 §8.1 calls out.
        let a = digest("candidate-a");
        let b = digest("candidate-b");
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };

        let mut frontier = Frontier::new();
        frontier.insert(Key::new(3, 7, hi), "hi").unwrap();
        frontier.insert(Key::new(3, 7, lo), "lo").unwrap();

        let (key, value) = frontier.select_least().expect("frontier is non-empty");
        assert_eq!(key.tiebreak, lo, "select_K must pick the smaller digest");
        assert_eq!(value, "lo");

        // And the frontier now holds only the other candidate.
        assert_eq!(frontier.len(), 1);
        let (key2, value2) = frontier.select_least().unwrap();
        assert_eq!(key2.tiebreak, hi);
        assert_eq!(value2, "hi");
        assert!(frontier.is_empty());
    }

    #[test]
    fn select_least_on_empty_frontier_is_quiescence() {
        let mut frontier: Frontier<u64> = Frontier::new();
        assert_eq!(frontier.select_least(), None);
    }

    #[test]
    fn buk_divergent_duplicate_key_is_rejected() {
        let mut frontier = Frontier::new();
        let key = Key::new(0, 0, digest("k"));
        assert!(frontier.insert(key, 10u64).unwrap());

        let err = frontier
            .insert(key, 20u64)
            .expect_err("a duplicate key with a different value must be rejected");
        assert_eq!(err.key, key);
        assert_eq!(err.existing, 10);
        assert_eq!(err.attempted, 20);

        // Rejected insert must not have mutated the frontier.
        assert_eq!(frontier.len(), 1);
        let (_, only_value) = frontier.select_least().unwrap();
        assert_eq!(only_value, 10);
    }

    #[test]
    fn buk_idempotent_duplicate_key_is_accepted() {
        let mut frontier = Frontier::new();
        let key = Key::new(0, 0, digest("k"));
        assert!(frontier.insert(key, 42u64).unwrap());
        // Same key, same (observed) value again — idempotent, not an error.
        assert!(!frontier.insert(key, 42u64).unwrap());
        assert_eq!(frontier.len(), 1);
    }

    #[test]
    fn key_canon_field_order_matches_ord_field_order() {
        // The canonical encoding's field order (phase, priority, tiebreak)
        // must match the `Ord`-significant declaration order — otherwise
        // the history-chain encoding and the calendar's selection order
        // would silently diverge.
        let k = Key::new(1, 2, digest("t"));
        let mut got = CanonWriter::new();
        k.canon_write(&mut got);

        let mut expected = CanonWriter::new();
        expected.write_uint(1);
        expected.write_uint(2);
        expected.write_bytes(digest("t").as_bytes());

        assert_eq!(got.finish(), expected.finish());
    }
}
