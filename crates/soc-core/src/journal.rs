//! Append-only history + deterministic replay (`Build_Plan_v3_SOC.md` Step 4;
//! ADR-0002 §9.2 "State": `h' = H(h_digest, step)`, O(1)/step).
//!
//! Each committed tick ([`crate::commit::run`]) produces one [`CommittedStep`]
//! — enough to let the Lane 2 audit-factorization checker replay the step's
//! endpoints and its recorded (unverified) [`Decomposition`]. [`Journal`] is
//! the append-only log of these steps, chained through [`History`] exactly
//! the way the naive oracle chains its own successor history (same fold,
//! different payload type).
//!
//! **Deterministic replay.** [`Journal::replay_chain`] independently folds a
//! *fresh* [`History`] over a step slice; running the committed loop twice
//! from the same inputs must produce byte-identical
//! [`Journal::step_digests`] — that is the deterministic-replay property the
//! Step 4 gate names, and `tests/calendar_commit.rs` exercises it directly.

use brix_canon::{CanonWriter, Canonical, Digest};
use brix_semantic::{ConfigId, Decomposition, WitnessId};

use crate::calendar::Key;
use crate::commit::Observation;
use crate::history::History;

/// One committed tick's full log record: enough for the audit-factorization
/// checker (Lane 2) to replay the step's `Decomposition` against its
/// recorded endpoints. **Frozen field order (ABI):** `key`, `observation`,
/// `decomposition`, `src`, `dst`, `witness` — see [`Canonical`] impl below.
///
/// **`CostRecord` is deliberately absent.** Cost is purely observational
/// (ADR-0001 stage-4a) and is not part of the behavior signature `O_min`
/// (ADR-0002 §8.3); it rides alongside the journal (see
/// [`crate::commit::run`]'s returned `Vec<CostRecord>`), never inside the
/// history-chain encoding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommittedStep {
    /// The calendar key `select_K` committed this tick under.
    pub key: Key,
    /// The `O_min` observation this step published (outcome class +
    /// committed `JudgementId` digest).
    pub observation: Observation,
    /// The tight `𝒢`-decomposition realizing this step's witness, recorded
    /// (unverified) by the hot loop (ADR-0002 §5.1) — replayed and verified
    /// off the hot path by the audit-factorization checker (Step 4 gate).
    pub decomposition: Decomposition,
    /// The committed step's source configuration.
    pub src: ConfigId,
    /// The committed step's destination configuration.
    pub dst: ConfigId,
    /// The identity of the committed witness.
    pub witness: WitnessId,
}

impl Canonical for CommittedStep {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI — frozen: key, observation, decomposition, src,
        // dst, witness. Never fold CostRecord in here (module docs).
        self.key.canon_write(w);
        self.observation.canon_write(w);
        self.decomposition.canon_write(w);
        self.src.canon_write(w);
        self.dst.canon_write(w);
        self.witness.canon_write(w);
    }
}

/// The append-only journal of committed steps, chained through a running
/// [`History`] digest (`h' = H(h_digest, step)`, O(1) per [`Journal::append`]
/// — [`History::append`]'s own doc explains why this never rescans).
#[derive(Clone, Debug)]
pub struct Journal {
    steps: Vec<CommittedStep>,
    chain: History,
}

impl Journal {
    /// A fresh, empty journal — the chain starts from [`History::empty`].
    pub fn new() -> Self {
        Journal {
            steps: Vec::new(),
            chain: History::empty(),
        }
    }

    /// Append `step`: folds it into the running chain digest (O(1)) and
    /// pushes it onto the log. Append-only — there is no removal API.
    pub fn append(&mut self, step: CommittedStep) {
        self.chain = self.chain.append(&step);
        self.steps.push(step);
    }

    /// The committed steps logged so far, in commit order.
    pub fn steps(&self) -> &[CommittedStep] {
        &self.steps
    }

    /// The current running chain digest — `h` after every step logged so
    /// far.
    pub fn chain_digest(&self) -> Digest {
        self.chain.digest()
    }

    /// Number of steps logged.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether nothing has been appended yet.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The running chain digest **after each step**, in commit order — what
    /// deterministic replay compares. `step_digests()[i]` is the chain
    /// digest immediately after `steps()[i]` was appended;
    /// `step_digests().last()` equals [`Journal::chain_digest`].
    pub fn step_digests(&self) -> Vec<Digest> {
        Self::replay_chain(&self.steps)
    }

    /// Deterministic replay: fold a **fresh** [`History`] over `steps` from
    /// scratch and return the running chain digest after each one. A
    /// [`Journal`] built by [`Journal::append`]ing exactly `steps` in order
    /// must produce a byte-identical [`Vec<Digest>`] to
    /// `Journal::step_digests` on that journal — this is the
    /// deterministic-replay property (`Build_Plan_v3_SOC.md` Step 4 gate).
    pub fn replay_chain(steps: &[CommittedStep]) -> Vec<Digest> {
        let mut chain = History::empty();
        let mut out = Vec::with_capacity(steps.len());
        for step in steps {
            chain = chain.append(step);
            out.push(chain.digest());
        }
        out
    }
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commit::Observation;
    use brix_canon::{Digest, Domain};
    use brix_semantic::{ConfigId, GeneratorId, Outcome, WitnessId};

    fn fixture_step(tag: &str) -> CommittedStep {
        CommittedStep {
            key: Key::new(0, 0, Digest::of(Domain::Value, tag.as_bytes())),
            observation: Observation {
                outcome_class: Outcome::Derived,
                judgement_digest: Digest::of(Domain::Value, tag.as_bytes()),
            },
            decomposition: Decomposition::recorded(
                vec![GeneratorId::named("g@1")],
                vec![ConfigId::from_canon(b"x0"), ConfigId::from_canon(b"x1")],
            )
            .unwrap(),
            src: ConfigId::from_canon(b"x0"),
            dst: ConfigId::from_canon(b"x1"),
            witness: WitnessId::from_canon(tag.as_bytes()),
        }
    }

    #[test]
    fn empty_journal_has_the_empty_history_digest() {
        let j = Journal::new();
        assert!(j.is_empty());
        assert_eq!(j.chain_digest(), History::empty().digest());
        assert!(j.step_digests().is_empty());
    }

    #[test]
    fn append_grows_the_journal_and_advances_the_chain() {
        let mut j = Journal::new();
        let before = j.chain_digest();
        j.append(fixture_step("s0"));
        assert_eq!(j.len(), 1);
        assert_ne!(j.chain_digest(), before);
    }

    #[test]
    fn replay_chain_matches_step_digests_byte_for_byte() {
        let mut j = Journal::new();
        j.append(fixture_step("s0"));
        j.append(fixture_step("s1"));
        j.append(fixture_step("s2"));

        assert_eq!(Journal::replay_chain(j.steps()), j.step_digests());
        assert_eq!(j.step_digests().last().copied().unwrap(), j.chain_digest());
    }

    #[test]
    fn two_journals_built_from_the_same_steps_are_byte_identical() {
        let mut a = Journal::new();
        let mut b = Journal::new();
        for tag in ["s0", "s1", "s2"] {
            a.append(fixture_step(tag));
            b.append(fixture_step(tag));
        }
        assert_eq!(a.step_digests(), b.step_digests());
        assert_eq!(a.chain_digest(), b.chain_digest());
    }
}
