//! The top-level engine: a namespace's totally-ordered committed
//! `DataRevision` stream (Part III §4), each revision produced by applying
//! one transaction to the prior ground state and re-settling the whole
//! program from scratch (Part III §4: "A revision is published fully settled
//! or not at all").
//!
//! Boring by construction: [`Store`] holds the current committed
//! `GroundState` and the fixed `Program`/phase list; [`Store::commit`]
//! clones the ground state (snapshot isolation), applies the transaction to
//! the clone, settles the candidate revision, and — if no `strict`
//! constraint is violated (Part IV §7) — swaps the clone in and bumps the
//! revision counter. On any failure the clone is dropped and nothing is
//! observable (Appendix I.11).

use std::collections::BTreeMap;

use crate::eval::{settle, Settled};
use crate::phase::{infer_phases, Phase, PhaseError};
use crate::program::{Program, RelName};
use crate::row::Extent;
use crate::txn::{apply, Transaction, TxnError};

/// The committed ground structure of a namespace: the live extents (after
/// supersession/retraction) plus the append-only history log used by
/// `history` clause reads (Part III §2). `Base(r)` in Part III §4 is exactly
/// `live` here.
#[derive(Clone, Debug, Default)]
pub struct GroundState {
    /// Live ground extents by relation (Ground/State/Event/Entity kinds).
    pub live: BTreeMap<RelName, Extent>,
    /// Append-only history per relation — every ground row ever committed,
    /// with merged claims. Read only by `history R(...)` clauses.
    pub history: BTreeMap<RelName, Extent>,
}

/// Why a commit did not publish a revision.
#[derive(Clone, Debug)]
pub enum CommitError {
    /// The transaction itself failed a per-kind key rule (Part III §8) or
    /// used an op on the wrong relation kind.
    Transaction(TxnError),
    /// The candidate revision settled, but a `strict` constraint had a live
    /// `Violation` (Part IV §7).
    StrictViolation { at_revision: u64 },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitError::Transaction(e) => write!(f, "transaction failed: {e}"),
            CommitError::StrictViolation { at_revision } => {
                write!(
                    f,
                    "strict constraint violated at candidate revision {at_revision}"
                )
            }
        }
    }
}
impl std::error::Error for CommitError {}

/// The reference engine for one namespace at one program revision.
pub struct Store {
    program: Program,
    phases: Vec<Phase>,
    ground: GroundState,
    /// Next `DataRevision` to publish. Revision 0 is the empty pre-history
    /// state; the first successful `commit` publishes revision 1.
    next_revision: u64,
    /// The most recently published settled revision, if any.
    current: Option<Settled>,
}

impl Store {
    /// Build a store, running Appendix F phase inference up front (the phase
    /// assignment is a property of the program, invariant across revisions —
    /// Part III §5, so it is computed once). Returns a `PhaseError` if the
    /// program has a cycle through a non-monotone edge.
    pub fn new(program: Program) -> Result<Self, PhaseError> {
        let phases = infer_phases(&program)?;
        let mut store = Store {
            program,
            phases,
            ground: GroundState::default(),
            next_revision: 1,
            current: None,
        };
        // Publish revision 0: the settled empty world, so `current()` is
        // always `Some` after construction and `why` works before any txn.
        let settled = settle(
            &store.program,
            &store.phases,
            &store.ground.live,
            &store.ground.history,
            0,
        );
        store.current = Some(settled);
        Ok(store)
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn phases(&self) -> &[Phase] {
        &self.phases
    }

    /// The currently published settled revision (always `Some` after
    /// construction).
    pub fn current(&self) -> &Settled {
        self.current
            .as_ref()
            .expect("store always has a settled revision")
    }

    /// Apply `txn` and, if it commits, publish and return the new settled
    /// revision. Snapshot-isolated and atomic (Part VII §2): on any error
    /// the store is unchanged.
    pub fn commit(&mut self, txn: &Transaction) -> Result<&Settled, CommitError> {
        // Snapshot isolation: work on a clone of committed ground.
        let candidate_ground =
            apply(&self.ground, &self.program.relations, txn).map_err(CommitError::Transaction)?;

        let revision = self.next_revision;
        let settled = settle(
            &self.program,
            &self.phases,
            &candidate_ground.live,
            &candidate_ground.history,
            revision,
        );

        // Strict constraints reject the candidate (Part IV §7) — evaluated
        // against the fully settled candidate revision.
        if !settled.strict_ok(&self.program) {
            return Err(CommitError::StrictViolation {
                at_revision: revision,
            });
        }

        // Publish.
        self.ground = candidate_ground;
        self.next_revision += 1;
        self.current = Some(settled);
        Ok(self.current())
    }
}
