//! Phase inference over the lane-neutral `RuleFacts` input — ported from
//! the scenarios `brix-oracle/tests/settle.rs` exercised indirectly through
//! its own `Program`, plus new tests for the two `PhaseError` paths (in-SCC
//! and cross-component cycles) that had no direct coverage anywhere before.

use brix_phase::{infer_phases, PhaseError, Produces, ReadSite, RuleFacts};
use proptest::prelude::*;

fn live(relation: &str) -> ReadSite {
    ReadSite {
        relation: relation.to_string(),
        strict: false,
        is_mask_target: false,
    }
}

fn strict(relation: &str) -> ReadSite {
    ReadSite {
        relation: relation.to_string(),
        strict: true,
        is_mask_target: false,
    }
}

fn mask_target(relation: &str) -> ReadSite {
    ReadSite {
        relation: relation.to_string(),
        strict: false,
        is_mask_target: true,
    }
}

fn produces(id: &str, relation: &str, reads: Vec<ReadSite>) -> RuleFacts {
    RuleFacts {
        id: id.to_string(),
        produces: Produces::Relation(relation.to_string()),
        reads,
    }
}

fn masks(id: &str, relation: &str, reads: Vec<ReadSite>) -> RuleFacts {
    RuleFacts {
        id: id.to_string(),
        produces: Produces::Mask {
            relation: relation.to_string(),
        },
        reads,
    }
}

/// Errata 0002 witness: `Base`/`Trans` both derive `Reach` (transitive
/// closure) — neither reads the other's output directly beyond `Trans`
/// reading `Reach` itself, but predicate-level condensation must still put
/// them in one phase.
#[test]
fn co_producers_of_one_relation_form_a_single_phase() {
    let rules = vec![
        produces("Base", "Reach", vec![live("Link")]),
        produces("Trans", "Reach", vec![live("Reach"), live("Link")]),
    ];
    let phases = infer_phases(&rules).unwrap();
    assert_eq!(phases.len(), 1, "Base and Trans settle in one phase");
    assert_eq!(
        phases[0].rules,
        vec!["Base".to_string(), "Trans".to_string()]
    );
}

/// Part III §6 phase rule: a mask's producer settles strictly before the
/// mask, and the mask strictly before any ordinary reader of the masked
/// relation — while `FromComputed`/`FromManual` (both producing
/// `EffectivePrice`) still condense into one phase via errata 0002.
#[test]
fn mask_orders_producer_before_masker_before_reader() {
    let rules = vec![
        produces("Compute", "ComputedPrice", vec![live("BaseAmount")]),
        masks(
            "Override",
            "ComputedPrice",
            vec![mask_target("ComputedPrice"), live("ManualPrice")],
        ),
        produces(
            "FromComputed",
            "EffectivePrice",
            vec![live("ComputedPrice")],
        ),
        produces("FromManual", "EffectivePrice", vec![live("ManualPrice")]),
        produces(
            "Waiting",
            "Unpriced",
            vec![live("BaseAmount"), strict("EffectivePrice")],
        ),
    ];
    let phases = infer_phases(&rules).unwrap();
    let phase_of = |id: &str| {
        phases
            .iter()
            .position(|p| p.rules.iter().any(|r| r == id))
            .unwrap()
    };
    assert!(phase_of("Compute") < phase_of("Override"));
    assert!(phase_of("Override") < phase_of("FromComputed"));
    assert!(phase_of("FromComputed") == phase_of("FromManual"));
}

/// A strict edge whose endpoints share one positive-recursion SCC is a
/// direct compile-time cycle error, reported with the shortest witness
/// cycle back through the SCC's positive edges.
#[test]
fn strict_edge_inside_one_scc_is_a_direct_cycle_error() {
    // A produces P, reads Q (live) and Q (strict); B produces Q, reads P
    // (live). Positive edges B->A, A->B put {A,B} in one SCC; the strict
    // read of Q by A adds a strict edge B=>A inside that same SCC.
    let rules = vec![
        produces("A", "P", vec![live("Q"), strict("Q")]),
        produces("B", "Q", vec![live("P")]),
    ];
    let err = infer_phases(&rules).unwrap_err();
    match err {
        PhaseError::CycleThroughNonMonotoneEdge {
            from,
            to,
            reason,
            path,
        } => {
            assert_eq!(from, "B");
            assert_eq!(to, "A");
            assert_eq!(
                reason,
                "strict or mask edge inside one positive-recursion component"
            );
            assert_eq!(path.first(), Some(&"B".to_string()));
            assert_eq!(path.last(), Some(&"B".to_string()));
            assert!(path.contains(&"A".to_string()));
        }
    }
}

/// A residual cycle at the condensation level (no single SCC contains it,
/// but strict edges close a cycle across several) is also an error, with
/// the shortest witness path through the condensation graph — not an
/// arbitrary edge.
#[test]
fn residual_cycle_across_components_is_a_cycle_error() {
    // Three independent single-rule components with strict edges forming a
    // cycle C=>A=>B=>C at the condensation level.
    let rules = vec![
        produces("A", "RA", vec![strict("RC")]),
        produces("B", "RB", vec![strict("RA")]),
        produces("C", "RC", vec![strict("RB")]),
    ];
    let err = infer_phases(&rules).unwrap_err();
    match err {
        PhaseError::CycleThroughNonMonotoneEdge { reason, path, .. } => {
            assert_eq!(
                reason,
                "non-monotone edge closes a cycle across several phase components"
            );
            assert_eq!(path.first(), path.last());
            for id in ["A", "B", "C"] {
                assert!(
                    path.contains(&id.to_string()),
                    "path should mention {id}: {path:?}"
                );
            }
        }
    }
}

/// OWNER.md's stated conformance requirement: phase assignment must not
/// depend on the order `RuleFacts` are handed in.
#[test]
fn declaration_order_does_not_change_phase_assignment() {
    let a = vec![
        produces("Base", "Reach", vec![live("Link")]),
        produces("Trans", "Reach", vec![live("Reach"), live("Link")]),
        produces("Compute", "ComputedPrice", vec![live("BaseAmount")]),
        masks(
            "Override",
            "ComputedPrice",
            vec![mask_target("ComputedPrice"), live("ManualPrice")],
        ),
    ];
    let mut b = a.clone();
    b.reverse();

    let phases_a = infer_phases(&a).unwrap();
    let phases_b = infer_phases(&b).unwrap();
    assert_eq!(phases_a, phases_b);
}

fn fixture_rules() -> Vec<RuleFacts> {
    vec![
        produces("Base", "Reach", vec![live("Link")]),
        produces("Trans", "Reach", vec![live("Reach"), live("Link")]),
        produces("Compute", "ComputedPrice", vec![live("BaseAmount")]),
        masks(
            "Override",
            "ComputedPrice",
            vec![mask_target("ComputedPrice"), live("ManualPrice")],
        ),
        produces(
            "FromComputed",
            "EffectivePrice",
            vec![live("ComputedPrice")],
        ),
        produces("FromManual", "EffectivePrice", vec![live("ManualPrice")]),
        produces(
            "Waiting",
            "Unpriced",
            vec![live("BaseAmount"), strict("EffectivePrice")],
        ),
    ]
}

/// A small, dependency-free xorshift64 PRNG — good enough to fuzz
/// declaration order deterministically from a proptest-generated seed
/// without pulling in `rand` as a dependency just for this one test.
fn xorshift_next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn shuffled(seed: u64, rules: &[RuleFacts]) -> Vec<RuleFacts> {
    let mut state = seed | 1; // xorshift requires a nonzero state
    let mut v = rules.to_vec();
    for i in (1..v.len()).rev() {
        let r = (xorshift_next(&mut state) % (i as u64 + 1)) as usize;
        v.swap(i, r);
    }
    v
}

proptest! {
    /// OWNER.md's stated conformance requirement, fuzzed: phase assignment
    /// is invariant under rule declaration order for any shuffle of a
    /// fixed, well-stratified rule set (positive recursion, co-producers,
    /// and a mask all present).
    #[test]
    fn phase_assignment_invariant_under_any_declaration_order(seed in any::<u64>()) {
        let base = infer_phases(&fixture_rules()).unwrap();
        let perturbed = infer_phases(&shuffled(seed, &fixture_rules())).unwrap();
        prop_assert_eq!(base, perturbed);
    }
}
