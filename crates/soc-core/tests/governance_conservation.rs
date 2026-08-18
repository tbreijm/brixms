//! Governance-conservation gate (ADR-0002 §5 point 5, §5.5 "Governance
//! monotonicity"; `spec/Build_Plan_v3_SOC.md` Step 3 gate):
//!
//! > Tightening `Adm` shrinks `cand(e)` pointwise — `Adm' ⇒ Adm` implies
//! > `cand'(e) ⊆ cand(e)` and `Succ'(e) ⊆ Succ(e)` for every reachable `e`.
//!
//! This builds a small fixture world with two realization regimes and
//! several admissibility predicates — a loose `Adm` and one-or-more strictly
//! tighter `Adm'`s (including a composed intersection) — enumerates every
//! `e` reachable from an initial `e0` up to a bounded depth (via the loosest
//! `Adm`, which by the conservation law itself must reach a superset of
//! anything any tighter `Adm'` could reach), and asserts `cand'(e) ⊆
//! cand(e)` and `Succ'(e) ⊆ Succ(e)` pointwise for all of them. It also
//! checks the fixture is non-vacuous — the tightened `Adm'` must actually
//! exclude at least one candidate the loose `Adm` admitted, or the test
//! would pass trivially.

use brix_canon::{Digest, Domain};
use soc_core::adm::{AdmAll, AdmWitnessAllowlist, AndAdm};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::oracle::{cand, succ};
use soc_core::witness_provider::{Candidate, WitnessProvider};
use std::collections::{BTreeMap, BTreeSet};

/// A provider whose full `ρ_w` relation is a fixed edge list keyed by world
/// handle — enough control to build a small, deterministic fixture graph
/// without a real `brix.type`-style regime.
struct FixtureRegime {
    edges: BTreeMap<Handle, Vec<(Handle, Handle)>>, // world -> [(witness, successor)]
}

impl FixtureRegime {
    fn witnesses(&self) -> BTreeSet<Handle> {
        self.edges
            .values()
            .flat_map(|edges| edges.iter().map(|(witness, _)| *witness))
            .collect()
    }
}

impl WitnessProvider for FixtureRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.edges
            .get(&e.world)
            .into_iter()
            .flatten()
            .map(|&(witness, successor)| Candidate { witness, successor })
            .collect()
    }
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

/// Build a small acyclic fixture graph over worlds `w0..w4` and two regimes,
/// A and B, whose witness families are distinct (a realistic case: different
/// regimes realize different witnesses over the same worlds, not
/// supersets of one another).
///
/// ```text
///        A          A            A
///  w0 -------> w1 -------> w3 -------> w4
///  |            \                       ^
///  | B           \ B                    |
///  v              `--------------------'
///  w2 -------------------------------> w4
///     A            B
///  w2 ---> w3   w2 ---> w4
/// ```
fn build_fixture() -> (Interner, FixtureRegime, FixtureRegime, ExecConfig) {
    let mut i = Interner::new();
    let w0 = tag(&mut i, "world0");
    let w1 = tag(&mut i, "world1");
    let w2 = tag(&mut i, "world2");
    let w3 = tag(&mut i, "world3");
    let w4 = tag(&mut i, "world4");

    let w01a = tag(&mut i, "w0->w1@a");
    let w13a = tag(&mut i, "w1->w3@a");
    let w23a = tag(&mut i, "w2->w3@a");
    let w34a = tag(&mut i, "w3->w4@a");
    let w02b = tag(&mut i, "w0->w2@b");
    let w14b = tag(&mut i, "w1->w4@b");
    let w24b = tag(&mut i, "w2->w4@b");

    let mut edges_a: BTreeMap<Handle, Vec<(Handle, Handle)>> = BTreeMap::new();
    edges_a.insert(w0, vec![(w01a, w1)]);
    edges_a.insert(w1, vec![(w13a, w3)]);
    edges_a.insert(w2, vec![(w23a, w3)]);
    edges_a.insert(w3, vec![(w34a, w4)]);

    let mut edges_b: BTreeMap<Handle, Vec<(Handle, Handle)>> = BTreeMap::new();
    edges_b.insert(w0, vec![(w02b, w2)]);
    edges_b.insert(w1, vec![(w14b, w4)]);
    edges_b.insert(w2, vec![(w24b, w4)]);

    let policy = tag(&mut i, "policy0");
    let e0 = ExecConfig::new(w0, policy, History::empty().digest());

    (
        i,
        FixtureRegime { edges: edges_a },
        FixtureRegime { edges: edges_b },
        e0,
    )
}

const MAX_DEPTH: usize = 6;

/// Every `e` reachable from `e0` up to `MAX_DEPTH` steps, computed under the
/// loosest `Adm` ([`AdmAll`]) — the maximal reachable universe any tighter
/// `Adm'` could possibly need, since tightening only removes successors.
fn reachable(regimes: &[&dyn WitnessProvider], e0: ExecConfig) -> BTreeSet<ExecConfig> {
    let adm_all = AdmAll;
    let mut visited: BTreeSet<ExecConfig> = BTreeSet::new();
    visited.insert(e0);
    let mut frontier: Vec<ExecConfig> = vec![e0];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for e in &frontier {
            for s in succ(regimes, &adm_all, e) {
                if visited.insert(s) {
                    next.push(s);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    visited
}

#[test]
fn tightening_adm_shrinks_cand_and_succ_pointwise_over_every_reachable_config() {
    let (_i, regime_a, regime_b, e0) = build_fixture();
    let regimes: Vec<&dyn WitnessProvider> = vec![&regime_a, &regime_b];

    let adm_loose = AdmAll;
    // Adm' — strictly tighter: only provider A's witnesses are admissible.
    let adm_tight = AdmWitnessAllowlist::new(regime_a.witnesses());

    let reachable_configs = reachable(&regimes, e0);
    assert!(
        reachable_configs.len() > 1,
        "fixture must actually reach more than e0 for this gate to mean anything"
    );

    let mut saw_strict_shrink = false;

    for e in &reachable_configs {
        let cand_loose = cand(&regimes, &adm_loose, e);
        let cand_tight = cand(&regimes, &adm_tight, e);
        assert!(
            cand_tight.is_subset(&cand_loose),
            "cand'(e) must be a subset of cand(e) at {e:?}: tight={cand_tight:?} loose={cand_loose:?}"
        );
        if cand_tight.len() < cand_loose.len() {
            saw_strict_shrink = true;
        }

        let succ_loose = succ(&regimes, &adm_loose, e);
        let succ_tight = succ(&regimes, &adm_tight, e);
        assert!(
            succ_tight.is_subset(&succ_loose),
            "Succ'(e) must be a subset of Succ(e) at {e:?}"
        );
    }

    assert!(
        saw_strict_shrink,
        "fixture is vacuous: Adm' never actually excluded a candidate that Adm admitted"
    );
}

#[test]
fn a_second_independently_tightened_adm_also_conserves() {
    let (_i, regime_a, regime_b, e0) = build_fixture();
    let regimes: Vec<&dyn WitnessProvider> = vec![&regime_a, &regime_b];

    let adm_loose = AdmAll;
    let adm_b_only = AdmWitnessAllowlist::new(regime_b.witnesses());

    let reachable_configs = reachable(&regimes, e0);
    let mut saw_strict_shrink = false;
    for e in &reachable_configs {
        let cand_loose = cand(&regimes, &adm_loose, e);
        let cand_b = cand(&regimes, &adm_b_only, e);
        assert!(cand_b.is_subset(&cand_loose));
        if cand_b.len() < cand_loose.len() {
            saw_strict_shrink = true;
        }

        let succ_loose = succ(&regimes, &adm_loose, e);
        let succ_b = succ(&regimes, &adm_b_only, e);
        assert!(succ_b.is_subset(&succ_loose));
    }
    assert!(saw_strict_shrink);
}

#[test]
fn composing_two_tightenings_via_and_adm_conserves_transitively() {
    // Adm'' = AndAdm(Adm'_a, Adm'_b) is at least as tight as either alone.
    // provider A's and provider B's witness allow-lists are disjoint, so their
    // intersection admits nothing — the tightest possible Adm — and the
    // chain Adm'' ⊆ Adm'_a ⊆ Adm (and Adm'' ⊆ Adm'_b ⊆ Adm) must all hold
    // pointwise.
    let (_i, regime_a, regime_b, e0) = build_fixture();
    let regimes: Vec<&dyn WitnessProvider> = vec![&regime_a, &regime_b];

    let adm_loose = AdmAll;
    let adm_a = AdmWitnessAllowlist::new(regime_a.witnesses());
    let adm_b = AdmWitnessAllowlist::new(regime_b.witnesses());
    let adm_neither = AndAdm(
        AdmWitnessAllowlist::new(regime_a.witnesses()),
        AdmWitnessAllowlist::new(regime_b.witnesses()),
    );

    let reachable_configs = reachable(&regimes, e0);
    for e in &reachable_configs {
        let cand_loose = cand(&regimes, &adm_loose, e);
        let cand_a = cand(&regimes, &adm_a, e);
        let cand_b = cand(&regimes, &adm_b, e);
        let cand_neither = cand(&regimes, &adm_neither, e);

        assert!(cand_a.is_subset(&cand_loose));
        assert!(cand_b.is_subset(&cand_loose));
        assert!(cand_neither.is_subset(&cand_a));
        assert!(cand_neither.is_subset(&cand_b));
        assert!(
            cand_neither.is_empty(),
            "intersection of two disjoint provider witness allow-lists must admit nothing at {e:?}"
        );

        let succ_loose = succ(&regimes, &adm_loose, e);
        let succ_neither = succ(&regimes, &adm_neither, e);
        assert!(succ_neither.is_subset(&succ_loose));
        assert!(succ_neither.is_empty());
    }
}
