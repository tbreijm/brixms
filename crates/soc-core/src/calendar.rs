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

/// Why a transactional [`Frontier::apply_delta`] was rejected. On any of these
/// the frontier is left **exactly as it was** (the delta is staged on a copy
/// and only published if every operation succeeds — ADR-0012 §2.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FrontierDeltaError<V> {
    /// A removal named a key that is not present.
    RemoveMissing(Key),
    /// A removal named a key present with a *different* value than expected
    /// (a stale caller expectation — an integrity failure, not a no-op).
    RemoveMismatch(KeyConflict<V>),
    /// An addition collided with an existing, different value at its key
    /// (the B^uk unique-key discipline — see [`KeyConflict`]).
    InsertConflict(KeyConflict<V>),
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

    /// Non-mutating [`select_least`](Self::select_least): the globally least
    /// key/value pair *without* removing it, or `None` if empty. Required by
    /// the L3 adapter (ADR-0012 §2.6, §4.7), which peeks the least candidate,
    /// computes its prospective successor, and only removes it once its commit
    /// succeeds.
    pub fn peek_least(&self) -> Option<(&Key, &V)> {
        self.entries.first_key_value()
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

    /// Atomically apply a candidate delta: first remove each `(key, expected)`
    /// (each key must be present **and equal** to `expected`), then insert each
    /// addition under the same B^uk discipline as [`insert`](Self::insert)
    /// (idempotent on an equal existing value, `InsertConflict` on a different
    /// one). The whole delta is staged on a private copy and only published if
    /// **every** operation succeeds; on any error the frontier is left exactly
    /// as it was. This is the L3 adapter's committed-step frontier maintenance
    /// (ADR-0012 §2.6, §4.7): a committed head candidate is removed and at most
    /// one successor candidate inserted, transactionally.
    pub fn apply_delta(
        &mut self,
        removals: &[(Key, V)],
        additions: &[(Key, V)],
    ) -> Result<(), FrontierDeltaError<V>> {
        let mut staged = self.entries.clone();
        for (key, expected) in removals {
            match staged.get(key) {
                Some(v) if v == expected => {
                    staged.remove(key);
                }
                Some(v) => {
                    return Err(FrontierDeltaError::RemoveMismatch(KeyConflict {
                        key: *key,
                        existing: v.clone(),
                        attempted: expected.clone(),
                    }));
                }
                None => return Err(FrontierDeltaError::RemoveMissing(*key)),
            }
        }
        for (key, value) in additions {
            match staged.get(key) {
                None => {
                    staged.insert(*key, value.clone());
                }
                Some(v) if v == value => {}
                Some(v) => {
                    return Err(FrontierDeltaError::InsertConflict(KeyConflict {
                        key: *key,
                        existing: v.clone(),
                        attempted: value.clone(),
                    }));
                }
            }
        }
        self.entries = staged;
        Ok(())
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

    #[test]
    fn peek_least_returns_the_least_without_removing() {
        let mut f = Frontier::new();
        f.insert(Key::new(0, 1, digest("b")), "b").unwrap();
        f.insert(Key::new(0, 0, digest("a")), "a").unwrap();
        assert_eq!(f.peek_least().map(|(_, v)| *v), Some("a"));
        assert_eq!(f.len(), 2, "peek must not remove");
        // The subsequently-selected least is still the same value.
        assert_eq!(f.select_least().map(|(_, v)| v), Some("a"));
    }

    #[test]
    fn apply_delta_is_transactional_and_rolls_back() {
        let ka = Key::new(0, 0, digest("a"));
        let kb = Key::new(0, 1, digest("b"));
        let kc = Key::new(0, 2, digest("c"));
        let mut f = Frontier::new();
        f.insert(ka, "a").unwrap();

        // Remove a, add b — succeeds atomically.
        f.apply_delta(&[(ka, "a")], &[(kb, "b")]).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f.peek_least().map(|(_, v)| *v), Some("b"));

        // A delta whose removal is now stale fails and leaves f UNCHANGED.
        let err = f.apply_delta(&[(ka, "a")], &[(kc, "c")]).unwrap_err();
        assert!(matches!(err, FrontierDeltaError::RemoveMissing(_)));
        assert_eq!(f.len(), 1, "a failed delta must not mutate");
        assert_eq!(f.peek_least().map(|(_, v)| *v), Some("b"));
    }

    #[test]
    fn apply_delta_rejects_a_removal_naming_an_unexpected_candidate_at_a_key() {
        // ADR-0012 §2.6 / #244 acceptance: removing a key whose current value
        // is not the expected one is an integrity failure, never a silent
        // no-op, and the frontier is left byte-identically unchanged.
        let ka = Key::new(0, 0, digest("a"));
        let kb = Key::new(0, 1, digest("b"));
        let mut f = Frontier::new();
        f.insert(ka, "actual").unwrap();
        f.insert(kb, "b").unwrap();
        let before = f.clone();

        let err = f
            .apply_delta(&[(ka, "expected-but-wrong")], &[])
            .expect_err("a stale/wrong expected value at a present key must be rejected");
        match err {
            FrontierDeltaError::RemoveMismatch(conflict) => {
                assert_eq!(conflict.key, ka);
                assert_eq!(conflict.existing, "actual");
                assert_eq!(conflict.attempted, "expected-but-wrong");
            }
            other => panic!("expected RemoveMismatch, got {other:?}"),
        }
        assert_eq!(f.len(), before.len());
        assert_eq!(
            f.entries, before.entries,
            "a rejected removal must leave the frontier byte-identically unchanged"
        );
    }

    #[test]
    fn apply_delta_rejects_a_conflicting_insertion_not_silently_resolving_it() {
        // ADR-0012 §2.6 / #244 acceptance: two distinct candidates proposed at
        // one key is an error, preserving the B^uk unique-key discipline —
        // never silently resolved by picking one, and the frontier is left
        // exactly as it was.
        let ka = Key::new(0, 0, digest("a"));
        let mut f = Frontier::new();
        f.insert(ka, "existing").unwrap();
        let before = f.clone();

        let err = f
            .apply_delta(&[], &[(ka, "different")])
            .expect_err("two distinct values at one key must be rejected");
        match err {
            FrontierDeltaError::InsertConflict(conflict) => {
                assert_eq!(conflict.key, ka);
                assert_eq!(conflict.existing, "existing");
                assert_eq!(conflict.attempted, "different");
            }
            other => panic!("expected InsertConflict, got {other:?}"),
        }
        assert_eq!(
            f.entries, before.entries,
            "a rejected insertion must leave the frontier byte-identically unchanged"
        );
    }
}
