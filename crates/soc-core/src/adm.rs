//! The admissibility judgment `Adm` (ADR-0002 §1 "Dynamics"; §5 point 5,
//! §5.5 "Governance monotonicity"):
//!
//! > Candidates come from realization filtered by an admissibility judgment
//! > `Adm`.
//! >
//! > Governance monotonicity is a lattice-safe operation. Tightening `Adm`
//! > shrinks `cand(e)` pointwise (SOC "Governance monotonicity"); it can only
//! > *remove* candidates, never fabricate a stronger outcome — the
//! > conservation law of §9's gate.
//!
//! That conservation law — `Adm' ⇒ Adm` implies `cand'(e) ⊆ cand(e)` and
//! `Succ'(e) ⊆ Succ(e)` for every reachable `e` — is this crate's executable
//! gate: `tests/governance_conservation.rs`.

use crate::exec::ExecConfig;
use crate::witness_provider::Candidate;

/// An admissibility predicate over candidates for a given exec config.
pub trait Adm {
    /// Whether `c` is admissible at `e`.
    fn admits(&self, e: &ExecConfig, c: &Candidate) -> bool;
}

/// Admits everything — the loosest possible `Adm`. The baseline every
/// tightened `Adm'` is checked against in the conservation-law gate.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmAll;

impl Adm for AdmAll {
    fn admits(&self, _e: &ExecConfig, _c: &Candidate) -> bool {
        true
    }
}

/// Admits nothing — the tightest possible `Adm`. Useful as a sanity pole for
/// the conservation law (`cand(e)` under `AdmNone` is always empty).
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmNone;

impl Adm for AdmNone {
    fn admits(&self, _e: &ExecConfig, _c: &Candidate) -> bool {
        false
    }
}

/// Admits only candidates whose witness handle is in an explicit allow-list.
/// A concrete, strictly-tighter `Adm'` for exercising the conservation law:
/// any proper-subset allow-list admits a subset of what [`AdmAll`] admits, by
/// construction.
#[derive(Clone, Debug, Default)]
pub struct AdmWitnessAllowlist {
    allowed: std::collections::BTreeSet<crate::intern::Handle>,
}

impl AdmWitnessAllowlist {
    /// Build an allow-list from an iterable of witness handles.
    pub fn new(allowed: impl IntoIterator<Item = crate::intern::Handle>) -> Self {
        AdmWitnessAllowlist {
            allowed: allowed.into_iter().collect(),
        }
    }
}

impl Adm for AdmWitnessAllowlist {
    fn admits(&self, _e: &ExecConfig, c: &Candidate) -> bool {
        self.allowed.contains(&c.witness)
    }
}

/// Admits only candidates whose successor handle passes a caller-supplied
/// predicate. A second, orthogonal tightening — combine with
/// [`AdmWitnessAllowlist`] via [`AndAdm`] to test that composed governance
/// policies still conserve.
pub struct AdmSuccessorFilter<F: Fn(crate::intern::Handle) -> bool> {
    /// The predicate a candidate's successor handle must satisfy.
    pub predicate: F,
}

impl<F: Fn(crate::intern::Handle) -> bool> Adm for AdmSuccessorFilter<F> {
    fn admits(&self, _e: &ExecConfig, c: &Candidate) -> bool {
        (self.predicate)(c.successor)
    }
}

/// Intersection of two admissibility predicates: `AndAdm(a, b)` admits `c` at
/// `e` iff both `a` and `b` do. Intersecting `Adm`s is always at least as
/// tight as either alone, which is how composed governance policies stay
/// inside the conservation law without a bespoke proof per combination.
pub struct AndAdm<A, B>(pub A, pub B);

impl<A: Adm, B: Adm> Adm for AndAdm<A, B> {
    fn admits(&self, e: &ExecConfig, c: &Candidate) -> bool {
        self.0.admits(e, c) && self.1.admits(e, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Interner;
    use brix_canon::{Digest, Domain};

    fn fixture() -> (ExecConfig, Candidate) {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"w"));
        let policy = i.intern(Digest::of(Domain::Value, b"p"));
        let history = Digest::of(Domain::Value, b"h");
        let witness = i.intern(Digest::of(Domain::Value, b"witness"));
        let successor = i.intern(Digest::of(Domain::Value, b"succ"));
        (
            ExecConfig::new(world, policy, history),
            Candidate { witness, successor },
        )
    }

    #[test]
    fn adm_all_admits_everything() {
        let (e, c) = fixture();
        assert!(AdmAll.admits(&e, &c));
    }

    #[test]
    fn adm_none_admits_nothing() {
        let (e, c) = fixture();
        assert!(!AdmNone.admits(&e, &c));
    }

    #[test]
    fn allowlist_admits_only_listed_witnesses() {
        let (e, c) = fixture();
        let allow = AdmWitnessAllowlist::new([c.witness]);
        assert!(allow.admits(&e, &c));

        let deny = AdmWitnessAllowlist::new(std::iter::empty());
        assert!(!deny.admits(&e, &c));
    }

    #[test]
    fn successor_filter_admits_by_predicate() {
        let (e, c) = fixture();
        let allow = AdmSuccessorFilter {
            predicate: |h| h == c.successor,
        };
        assert!(allow.admits(&e, &c));

        let deny = AdmSuccessorFilter {
            predicate: |h| h != c.successor,
        };
        assert!(!deny.admits(&e, &c));
    }

    #[test]
    fn and_adm_is_the_intersection() {
        let (e, c) = fixture();
        let allow = AdmWitnessAllowlist::new([c.witness]);
        let deny_successor = AdmSuccessorFilter {
            predicate: |h| h != c.successor,
        };
        assert!(!AndAdm(allow, deny_successor).admits(&e, &c));
    }
}
