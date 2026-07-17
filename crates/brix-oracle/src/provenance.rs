//! Provenance as a relation (Part III §11) and the sealed kernel schemas
//! (Appendix A) the oracle is responsible for producing: `Support`, `Claim`,
//! `Masked`, `KeyConflict`, `RuleError`, `Violation`.
//!
//! These are ordinary (small, closed) Rust structs rather than rows in a
//! generic `Extent`, because the oracle is their *producer*, not a program
//! that pattern-matches over them; `brix why` — a stock query over
//! `Support`/`Claim` (Part III §11) — is implemented directly against these
//! structures in [`crate::eval::Settled::why`]. A later integration that
//! wants these exposed as ordinary queryable `Rel<Row>` values can project
//! them through `RelationDef`s that mirror Appendix A field-for-field; that
//! projection is intentionally left to the adapter layer, not built here.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{Digest, EdgeId};

use crate::program::{ConstraintId, RelName, RuleId};
use crate::row::CanonBytes;
use crate::value::Value;

/// One rule-match's support of one derived row (Part III §2, §11).
///
/// `match_digest` is `Digest::of(Domain::Value, canon(sorted rule-body
/// bindings))` — the oracle's `MatchDigest` (Part III §9 reuses the same
/// notion for `RuleError.partialMatch`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SupportRef {
    pub rule: RuleId,
    pub match_digest: Digest,
}

/// `Support(edge, rule, match, atRevision)` (Appendix A).
///
/// `edge` is a raw `Digest` rather than the typed `EdgeId`: a rule head may
/// target either a `Derived` relation (`EdgeId`) or an `Entity` relation via
/// `keyed by (...)` (`NodeId`) — Part III §3 treats a Skolem-keyed node as
/// ordinary rule output, so `Support` must be able to name either.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SupportEdge {
    pub edge: Digest,
    pub relation: RelName,
    pub rule: RuleId,
    pub match_digest: Digest,
    pub at_revision: u64,
}

/// `Claim(edge, source, transaction, atRevision)` (Appendix A). `edge` is a
/// raw `Digest` for the same reason as `SupportEdge::edge` — ground claims
/// mint either an `EdgeId` (`assert`/`set` on a relation) or a `NodeId`
/// (`ensure`/`fresh` on an entity).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClaimEdge {
    pub edge: Digest,
    pub relation: RelName,
    pub claim: brix_canon::ClaimId,
    pub at_revision: u64,
}

/// `Masked(target, by, atPhase, atRevision)` (Appendix A, Part III §6).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MaskedEdge {
    pub target: EdgeId,
    pub by: EdgeId,
    pub relation: RelName,
    pub rule: RuleId,
    pub at_phase: usize,
    pub at_revision: u64,
}

/// `KeyConflict(relation, key, candidates, supports, atRevision)` (Part III
/// §8). `candidates` is `Digest` rather than `EdgeId` because this applies
/// to `Entity` relations too (see `crate::eval::refresh_live`), whose
/// candidates are `NodeId`s.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyConflictEdge {
    pub relation: RelName,
    pub key: CanonBytes,
    pub candidates: BTreeSet<Digest>,
    pub supports: BTreeSet<SupportRef>,
    pub at_revision: u64,
}

/// `RuleError(rule, site, partialMatch, error, atRevision)` (Part III §9).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuleErrorEdge {
    pub rule: RuleId,
    pub site: String,
    pub partial_match: Digest,
    pub error: Value,
    pub at_revision: u64,
}

/// `Violation(constraint, match, atRevision)` (Part IV §7, Appendix A).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ViolationEdge {
    pub constraint: ConstraintId,
    pub match_digest: Digest,
    pub bindings: BTreeMap<String, Value>,
    pub at_revision: u64,
}

/// All sealed provenance/kernel-edge output for one settled revision.
#[derive(Clone, Debug, Default)]
pub struct Provenance {
    pub supports: Vec<SupportEdge>,
    pub claims: Vec<ClaimEdge>,
    pub masked: Vec<MaskedEdge>,
    pub key_conflicts: Vec<KeyConflictEdge>,
    pub rule_errors: Vec<RuleErrorEdge>,
    pub violations: Vec<ViolationEdge>,
    /// `match_digest -> readable bindings`, an **oracle-internal** extension
    /// beyond the sealed `Support(edge, rule, match, atRevision)` schema
    /// (Part III §11), whose `match` field is formally just a `MatchDigest`
    /// hash (Part III §9 reuses the same notion for `RuleError`). The real
    /// engine reconstructs `why` explanations by walking the incidence
    /// index (`brix-rt`, Ring0_Build_Plan §1.7); the oracle has no such
    /// index, so it retains this reverse map purely so `why`/`explain` can
    /// answer without re-running the evaluator. Not part of the sealed
    /// surface — a real adapter would drop or replace this.
    pub match_bindings: BTreeMap<Digest, BTreeMap<String, Value>>,
}

impl Provenance {
    /// `brix why`-style answer: every support of `edge` (Part III §11).
    /// Callers walk further by treating each cited binding's `Value::Edge`/
    /// `Value::Node` entries as new subjects for `why` — the oracle keeps
    /// no cross-revision index, "no caching, no cleverness".
    pub fn why(&self, edge: Digest) -> Vec<&SupportEdge> {
        self.supports.iter().filter(|s| s.edge == edge).collect()
    }

    /// `why`, plus the readable binding environment for each match (see
    /// `match_bindings` docs above).
    pub fn explain(&self, edge: Digest) -> Vec<(&SupportEdge, Option<&BTreeMap<String, Value>>)> {
        self.why(edge)
            .into_iter()
            .map(|s| (s, self.match_bindings.get(&s.match_digest)))
            .collect()
    }

    /// Ground claims backing `edge`, if it is (also) a ground fact.
    pub fn claims_for(&self, edge: Digest) -> Vec<&ClaimEdge> {
        self.claims.iter().filter(|c| c.edge == edge).collect()
    }
}
