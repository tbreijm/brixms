//! Realization regimes: the `ρ_w` enumeration interface (ADR-0002 §7
//! "Realization regimes", §9.2).
//!
//! The ADR's target shape for a *running* regime is a dataflow operator:
//!
//! > As a running engine component a regime is a **dataflow operator**:
//! > `footprint()` + `apply(delta) → candidate delta` (§9.2).
//!
//! That incremental, delta-driven shape is `Build_Plan_v3_SOC.md` Step 6
//! (E3) — later engine work. This module is the **deliberately-naive v1**
//! from Step 3 (S2⋈E2): a [`Regime`] is asked to enumerate its *entire*
//! candidate set for the current [`ExecConfig`] from scratch on every call.
//! That is correct but O(|world|) rather than O(|Δ|) — exactly the
//! recompute-the-world reference-oracle shape ADR-0002 §3/§9.2 mandates be
//! *retained on purpose* as the differential-test baseline the later
//! incremental engine is checked against. Do not "optimize" this trait; add
//! the delta-driven one beside it when Step 6 lands.

use crate::exec::ExecConfig;
use crate::intern::Handle;

/// A witness candidate a regime proposes: an edge `x → y` under one regime's
/// `ρ_w` interpretation. Regimes **propose**; only the calendar **commits**
/// (`Derived`) and only the proof kernel **certifies** (`Proven`) — a regime
/// may never publish either (ADR-0002 §5 point 4, §7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Candidate {
    /// The regime whose `ρ_w` this candidate is proposed under. A bare
    /// handle for now: the full `RegimeId` canonical artifact (ADR-0002 §6,
    /// "New SOC artifacts") lands in `brix-semantic` in a later slice this
    /// crate does not touch.
    pub regime: Handle,
    /// The witness handle — the identity of `w : A → B`.
    pub witness: Handle,
    /// The successor world/configuration handle this witness would realize.
    pub successor: Handle,
}

/// A realization regime, presenting a class of witnesses under one `ρ_w`
/// interpretation (ADR-0002 §7). `candidates` enumerates the *unfiltered*
/// `ρ_w` relation for the given exec config — admissibility filtering
/// ([`crate::adm::Adm`]) is the oracle's job (`cand(e)`), not the regime's,
/// so a regime is testable and reusable independent of any particular
/// governance policy.
pub trait Regime {
    /// Enumerate this regime's candidates for `e`. Naive by design (see
    /// module docs): no incremental state, no memoization, recomputed from
    /// `e` on every call.
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Interner;
    use brix_canon::{Digest, Domain};

    struct ConstantRegime {
        regime: Handle,
        fixed: Vec<(Handle, Handle)>,
    }

    impl Regime for ConstantRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            self.fixed
                .iter()
                .map(|&(witness, successor)| Candidate {
                    regime: self.regime,
                    witness,
                    successor,
                })
                .collect()
        }
    }

    #[test]
    fn a_regime_can_be_exercised_directly_without_an_oracle() {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"w0"));
        let policy = i.intern(Digest::of(Domain::Value, b"p0"));
        let history = Digest::of(Domain::Value, b"h0");
        let regime_id = i.intern(Digest::of(Domain::Value, b"regime"));
        let witness = i.intern(Digest::of(Domain::Value, b"w"));
        let successor = i.intern(Digest::of(Domain::Value, b"w1"));

        let r = ConstantRegime {
            regime: regime_id,
            fixed: vec![(witness, successor)],
        };
        let e = ExecConfig::new(world, policy, history);
        let cs = r.candidates(&e);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].successor, successor);
    }
}
