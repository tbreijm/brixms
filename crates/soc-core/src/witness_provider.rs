//! Witness discovery providers: the non-ontological enumeration interface.
//!
//! The ADR's target shape for a running discovery provider is a dataflow operator:
//!
//! > As a running engine component a provider is a **dataflow operator**:
//! > `footprint()` + `apply(delta) → candidate delta` (§9.2).
//!
//! That incremental, delta-driven shape is `Build_Plan_v3_SOC.md` Step 6
//! (E3) — later engine work. This module is the **deliberately-naive v1**
//! from Step 3 (S2⋈E2): a [`WitnessProvider`] is asked to enumerate its *entire*
//! candidate set for the current [`ExecConfig`] from scratch on every call.
//! That is correct but O(|world|) rather than O(|Δ|) — exactly the
//! recompute-the-world reference-oracle shape ADR-0002 §3/§9.2 mandates be
//! *retained on purpose* as the differential-test baseline the later
//! incremental engine is checked against. Do not "optimize" this trait; add
//! the delta-driven one beside it when Step 6 lands.

use crate::exec::ExecConfig;
use crate::intern::Handle;

/// A candidate is exactly a witness and its successor configuration. The
/// execution configuration supplies policy/history; a discovery provider is
/// not a semantic constituent of the candidate. Providers **present**
/// possibilities; only the calendar **commits**
/// (`Derived`) and only the proof kernel **certifies** (`Proven`) — a provider
/// may never publish either (ADR-0002 §5 point 4, §7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Candidate {
    /// The witness handle — the identity of `w : A → B`.
    pub witness: Handle,
    /// The successor world/configuration handle this witness would realize.
    pub successor: Handle,
}

/// A non-ontological witness-discovery provider. `candidates` enumerates the
/// *unfiltered* witness possibilities for the given execution configuration;
/// admissibility filtering ([`crate::adm::Adm`]) is the oracle's job
/// (`cand(e)`), not the provider's, so a provider is testable and reusable
/// independent of any particular
/// governance policy.
pub trait WitnessProvider {
    /// Enumerate this provider's candidates for `e`. Naive by design (see
    /// module docs): no incremental state, no memoization, recomputed from
    /// `e` on every call.
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Interner;
    use brix_canon::{Digest, Domain};

    struct ConstantProvider {
        fixed: Vec<(Handle, Handle)>,
    }

    impl WitnessProvider for ConstantProvider {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            self.fixed
                .iter()
                .map(|&(witness, successor)| Candidate { witness, successor })
                .collect()
        }
    }

    #[test]
    fn a_provider_can_be_exercised_directly_without_an_oracle() {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"w0"));
        let policy = i.intern(Digest::of(Domain::Value, b"p0"));
        let history = Digest::of(Domain::Value, b"h0");
        let witness = i.intern(Digest::of(Domain::Value, b"w"));
        let successor = i.intern(Digest::of(Domain::Value, b"w1"));

        let r = ConstantProvider {
            fixed: vec![(witness, successor)],
        };
        let e = ExecConfig::new(world, policy, history);
        let cs = r.candidates(&e);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].successor, successor);
    }
}
