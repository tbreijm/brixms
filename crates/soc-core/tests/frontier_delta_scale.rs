//! Scale and transactional correctness verification for `Frontier::apply_delta` (ADR-0046, P1).
//!
//! Verifies that:
//! 1. `Frontier::apply_delta` scales O(|Δ| log |frontier|) and does NOT clone
//!    existing frontier entries (measured entry clones remain flat at 1 for a
//!    1-addition delta, regardless of candidate frontier size).
//! 2. Transactional rollback restores the exact pre-call frontier on any error
//!    (`InsertConflict`, `RemoveMissing`, `RemoveMismatch`).
//! 3. Totality and least-key selection (`select_K`) are preserved.

use std::cell::Cell;

use brix_canon::{Digest, Domain};
use soc_core::calendar::{Frontier, FrontierDeltaError, Key, KeyConflict};

thread_local! {
    static CLONES: Cell<u64> = const { Cell::new(0) };
}

#[derive(Debug, PartialEq, Eq)]
struct Counted(u64);

impl Clone for Counted {
    fn clone(&self) -> Self {
        CLONES.with(|c| c.set(c.get() + 1));
        Counted(self.0)
    }
}

fn count_clones<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let before = CLONES.with(Cell::get);
    let out = f();
    (out, CLONES.with(Cell::get) - before)
}

fn make_digest(tag: &str, n: u64) -> Digest {
    Digest::of(Domain::Value, format!("{tag}_{n}").as_bytes())
}

#[test]
fn frontier_apply_delta_copies_remain_flat_at_scale() {
    let measure = |size: usize| -> u64 {
        let mut frontier: Frontier<Counted> = Frontier::new();
        for i in 0..size as u64 {
            frontier
                .insert(Key::new(1, 100, make_digest("cand", i)), Counted(i))
                .unwrap();
        }
        let removals = [(Key::new(1, 100, make_digest("cand", 0)), Counted(0))];
        let additions = [(
            Key::new(1, 100, make_digest("new_cand", 9999)),
            Counted(777),
        )];
        let (res, clones) = count_clones(|| frontier.apply_delta(&removals, &additions));
        res.unwrap();
        assert_eq!(frontier.len(), size);
        clones
    };

    let counts: Vec<(usize, u64)> = [500usize, 1_000, 2_000, 4_000]
        .iter()
        .map(|&n| (n, measure(n)))
        .collect();

    for (n, c) in &counts {
        eprintln!("Frontier::apply_delta (1 rm+1 add) N={n:>6}: {c} entry clones");
        // Exactly 1 clone: the single addition is cloned into the map. Zero existing entries copied!
        assert_eq!(
            *c, 1,
            "N={n}: apply_delta must clone exactly 1 value (the added candidate), \
             zero existing entries cloned"
        );
    }
}

#[test]
fn frontier_apply_delta_rollback_on_insert_conflict() {
    let mut frontier = Frontier::new();
    let k1 = Key::new(1, 10, make_digest("cand", 1));
    let k2 = Key::new(1, 20, make_digest("cand", 2));
    let k3 = Key::new(1, 30, make_digest("cand", 3));

    frontier.insert(k1, "v1").unwrap();
    frontier.insert(k2, "v2").unwrap();

    let snapshot_before = frontier.clone();

    // Delta attempts to remove k1 and insert k3, but also attempts to insert k2 with a conflicting value.
    let removals = [(k1, "v1")];
    let additions = [(k3, "v3"), (k2, "conflict_v2")];

    let err = frontier.apply_delta(&removals, &additions).unwrap_err();
    assert_eq!(
        err,
        FrontierDeltaError::InsertConflict(KeyConflict {
            key: k2,
            existing: "v2",
            attempted: "conflict_v2",
        })
    );

    // Frontier must be completely unchanged
    assert_eq!(frontier.len(), 2);
    assert_eq!(frontier.peek_least(), snapshot_before.peek_least());
    assert_eq!(frontier.select_least(), Some((k1, "v1")));
    assert_eq!(frontier.select_least(), Some((k2, "v2")));
    assert_eq!(frontier.select_least(), None);
}

#[test]
fn frontier_apply_delta_rollback_on_remove_missing() {
    let mut frontier = Frontier::new();
    let k1 = Key::new(1, 10, make_digest("cand", 1));
    let k_missing = Key::new(1, 99, make_digest("missing", 99));

    frontier.insert(k1, "v1").unwrap();

    let removals = [(k_missing, "v_missing")];
    let additions = [(Key::new(1, 20, make_digest("new", 1)), "v_new")];

    let err = frontier.apply_delta(&removals, &additions).unwrap_err();
    assert_eq!(err, FrontierDeltaError::RemoveMissing(k_missing));
    assert_eq!(frontier.len(), 1);
    assert_eq!(frontier.select_least(), Some((k1, "v1")));
}

#[test]
fn frontier_apply_delta_rollback_on_remove_mismatch() {
    let mut frontier = Frontier::new();
    let k1 = Key::new(1, 10, make_digest("cand", 1));
    frontier.insert(k1, "real_v1").unwrap();

    let removals = [(k1, "stale_expectation")];
    let additions = [];

    let err = frontier.apply_delta(&removals, &additions).unwrap_err();
    assert_eq!(
        err,
        FrontierDeltaError::RemoveMismatch(KeyConflict {
            key: k1,
            existing: "real_v1",
            attempted: "stale_expectation",
        })
    );
    assert_eq!(frontier.len(), 1);
    assert_eq!(frontier.select_least(), Some((k1, "real_v1")));
}
