//! The lane-neutral input abstraction phase inference runs over.
//!
//! `brix-phase` must not depend on `brix-oracle` or `brix-ir` (both are
//! consumers, and a dependency the other way would cycle or, for `brixc`,
//! force it through the oracle). So instead of reading a typed program
//! directly, `infer_phases` reads a small owned fact list any lane can
//! project its own rule representation into — mirroring how `brix-ir`
//! decouples from `brix-ast` via `FrontendSource`.

/// A rule identifier, as the owning lane names it.
pub type RuleId = String;
/// A relation identifier, as the owning lane names it.
pub type RelId = String;

/// What a rule's head derives — an ordinary tuple into a relation, or a
/// mask over one (Part III §6). Mask heads never add rows to `relation`
/// themselves; they are kept as a distinct node so mask edges (Appendix F
/// #3) can order them relative to the relation's ordinary producers.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Produces {
    Relation(RelId),
    Mask { relation: RelId },
}

/// One relation read inside a rule's body, classified enough to build
/// positive, strict, and mask edges (Appendix F #1-#3).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReadSite {
    /// The relation being read.
    pub relation: RelId,
    /// `true` for a `without`/aggregate sub-pattern read (Appendix F #2);
    /// `false` for an ordinary live read. `history` reads are not
    /// represented at all — they create no edge (Appendix F #3).
    pub strict: bool,
    /// `true` only for a mask rule's own read of its `target` binding —
    /// the read that names *which* row of `relation` this mask covers.
    /// Excluded from mask ordering edges (errata 0002 / Part III §6).
    pub is_mask_target: bool,
}

/// Everything phase inference needs to know about one rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleFacts {
    pub id: RuleId,
    pub produces: Produces,
    pub reads: Vec<ReadSite>,
}
