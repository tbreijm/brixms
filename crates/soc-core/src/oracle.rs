//! The naive reference oracle: `cand(e)` and `Succ(e)` (ADR-0002 §3, §9.2
//! "Reference oracle"; `Build_Plan_v3_SOC.md` Step 3, S2⋈E2):
//!
//! > The naive recompute-the-world loop is deliberately reimplemented as the
//! > reference oracle; the fast engine is differential-tested against it
//! > (SOC settlement discipline, made testable).
//!
//! `brix-oracle`'s role reborn at the SOC layer: single-threaded,
//! recompute-the-world, **correct, not fast**. `cand(e)` is the union over
//! regimes of their (naive) [`crate::regime::Regime::candidates`], filtered
//! by an [`crate::adm::Adm`]. `Succ(e)` is the observed successor-config
//! set: every admissible candidate applied to `e`. This is deliberately the
//! differential-test baseline the later incremental engine (`Build_Plan_v3_SOC.md`
//! Step 6, E3/E4) is checked against — never "optimize" this module; a
//! faster engine belongs beside it, not instead of it.

use crate::adm::Adm;
use crate::exec::ExecConfig;
use crate::history::History;
use crate::intern::Handle;
use crate::regime::{Candidate, Regime};
use brix_canon::{Canonical, CanonWriter, Digest};
use std::collections::BTreeSet;

/// `cand(e)` = the union over `regimes` of `regime.candidates(e)`, filtered
/// by `adm`. A `BTreeSet` (Ring0 §0 determinism discipline — no `HashSet`)
/// so two calls with identical inputs return byte-identical results in the
/// same iteration order.
pub fn cand(regimes: &[&dyn Regime], adm: &dyn Adm, e: &ExecConfig) -> BTreeSet<Candidate> {
    let mut out = BTreeSet::new();
    for regime in regimes {
        for c in regime.candidates(e) {
            if adm.admits(e, &c) {
                out.insert(c);
            }
        }
    }
    out
}

/// `Succ(e)` — the observed successor-config set: every [`ExecConfig`]
/// reached by applying an admissible candidate from `cand(e)`. Each
/// candidate's witness+successor pair is folded into the history digest
/// chain ([`History::append`]) to produce the successor's `h` component,
/// exactly the same fold the calendar/commit step (`Build_Plan_v3_SOC.md`
/// Step 4) will use for the *singular committed* step — here, every
/// admissible candidate is "applied" (this is the deliberation frontier
/// `B^uk_{K,O}`, not `select_K`'s singular commit).
///
/// Because each successor's world/policy/history is a pure function of `e`
/// and the candidate alone (never of `adm`), `Succ(e)` is exactly the image
/// of `cand(e)` under that function — which is *why* the governance
/// conservation law (`cand'(e) ⊆ cand(e) ⟹ Succ'(e) ⊆ Succ(e)`) holds
/// structurally here, not merely by the test fixture's luck.
pub fn succ(regimes: &[&dyn Regime], adm: &dyn Adm, e: &ExecConfig) -> BTreeSet<ExecConfig> {
    cand(regimes, adm, e)
        .into_iter()
        .map(|c| apply(e, &c))
        .collect()
}

/// Apply one candidate to `e`, producing its successor `ExecConfig`. Policy
/// carries over unchanged; the world becomes the candidate's successor
/// handle; the history digest folds in the applied witness.
fn apply(e: &ExecConfig, c: &Candidate) -> ExecConfig {
    ExecConfig::new(c.successor, e.policy, next_history(e, c))
}

fn next_history(e: &ExecConfig, c: &Candidate) -> Digest {
    History::from_digest(e.history, 0)
        .append(&CandidateStep {
            witness: c.witness,
            successor: c.successor,
        })
        .digest()
}

/// The canonical payload folded into the history chain when a candidate is
/// applied: the witness and successor handles' raw indices. Stable within a
/// single run of a fixed [`crate::intern::Interner`] — the only context this
/// crate uses it in (see [`Handle::raw`]'s docs).
struct CandidateStep {
    witness: Handle,
    successor: Handle,
}

impl Canonical for CandidateStep {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_record([
            ("successor", (self.successor.raw() as u64).canon_bytes()),
            ("witness", (self.witness.raw() as u64).canon_bytes()),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adm::{AdmAll, AdmNone};
    use crate::intern::Interner;
    use brix_canon::Domain;

    struct FixedRegime {
        regime: Handle,
        out: Vec<(Handle, Handle)>,
    }

    impl Regime for FixedRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            self.out
                .iter()
                .map(|&(witness, successor)| Candidate {
                    regime: self.regime,
                    witness,
                    successor,
                })
                .collect()
        }
    }

    fn setup() -> (Interner, FixedRegime, ExecConfig) {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"w0"));
        let policy = i.intern(Digest::of(Domain::Value, b"p0"));
        let regime = i.intern(Digest::of(Domain::Value, b"r"));
        let witness = i.intern(Digest::of(Domain::Value, b"wit"));
        let successor = i.intern(Digest::of(Domain::Value, b"w1"));
        let e = ExecConfig::new(world, policy, History::empty().digest());
        (
            i,
            FixedRegime {
                regime,
                out: vec![(witness, successor)],
            },
            e,
        )
    }

    #[test]
    fn cand_under_adm_none_is_always_empty() {
        let (_i, regime, e) = setup();
        let regimes: Vec<&dyn Regime> = vec![&regime];
        assert!(cand(&regimes, &AdmNone, &e).is_empty());
        assert!(succ(&regimes, &AdmNone, &e).is_empty());
    }

    #[test]
    fn cand_under_adm_all_returns_the_regimes_candidates() {
        let (_i, regime, e) = setup();
        let regimes: Vec<&dyn Regime> = vec![&regime];
        let cs = cand(&regimes, &AdmAll, &e);
        assert_eq!(cs.len(), 1);
    }

    #[test]
    fn succ_advances_the_world_handle_and_the_history_digest() {
        let (_i, regime, e) = setup();
        let regimes: Vec<&dyn Regime> = vec![&regime];
        let successors = succ(&regimes, &AdmAll, &e);
        assert_eq!(successors.len(), 1);
        let s = successors.into_iter().next().unwrap();
        assert_eq!(s.world, regime.out[0].1);
        assert_eq!(s.policy, e.policy);
        assert_ne!(s.history, e.history, "applying a step must advance history");
    }

    #[test]
    fn succ_is_deterministic() {
        let (_i, regime, e) = setup();
        let regimes: Vec<&dyn Regime> = vec![&regime];
        assert_eq!(succ(&regimes, &AdmAll, &e), succ(&regimes, &AdmAll, &e));
    }
}
