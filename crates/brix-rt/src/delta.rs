//! The delta ABI (Ring0 §1.7, "design carefully first"): typed batches in,
//! emissions + support ops out. This module is the Rust half of the one
//! shared definition; `sdk/driver-wit/delta-abi.wit` is the WIT half,
//! hand-kept in lockstep (see that file's header for why it is hand-kept
//! rather than generated, for now).
//!
//! # Why one shape serves three consumers
//!
//! Part XXVIII §28.1–28.2 and Ring0 §1.7 name three things that all compile
//! against this ABI:
//!
//! - **generated (tier A) delta functions** — one monomorphized semi-naive
//!   delta function per rule per delta source, linked natively;
//! - **tier-B WASM rules** — the same delta function, compiled to WASM
//!   against this *stable* ABI so it can be loaded at an activation boundary
//!   without a supervisor rebuild;
//! - **the Driver SDK** — a guest Driver answers a protocol's `Desired`
//!   requests and reports `Succeeded`/`Failed`/`Cancelled` outcomes
//!   (Appendix H); structurally that is exactly "consume a batch of
//!   requests, produce emissions (the outcome facts) plus the support/claim
//!   bookkeeping they ground" — the same shape as a rule.
//!
//! [`DeltaAbi`] is generic over `Row: Canonical`. Tier A instantiates it
//! with the rule's own generated Rust struct — zero-copy, monomorphized.
//! Tier B and the Driver boundary instantiate it with [`CanonRow`], the
//! canon-encoded byte form transported across the WIT/component boundary;
//! the guest decodes/encodes through `brix-canon`, never a second
//! serializer. Same trait, same output vocabulary, two instantiations —
//! "one Rust trait + one WIT world, generated from a single definition."

use brix_canon::{CanonWriter, Canonical, EdgeId};

use crate::ids::{DataRevision, MatchDigest, RelationRef, RuleRef, SiteId};

/// A canon-encoded row, transported as opaque length-prefixed bytes. This is
/// [`DeltaAbi::Row`] at the tier-B/WASM/Driver boundary — the WIT world's
/// `canon-row` is `list<u8>` of exactly this payload.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct CanonRow(pub Vec<u8>);

impl Canonical for CanonRow {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(&self.0);
    }
}

/// One row-level change flowing into a delta function on one batch (Part
/// XXVIII §28.1: "one monomorphized semi-naive delta function per rule per
/// delta source").
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeltaOp<T> {
    /// A row became live on the delta source: a ground assert, a derived
    /// gain-of-support, or (for a protocol delta source) a new `Desired`
    /// request version.
    Insert(T),
    /// A row left the delta source: a ground retraction/supersession, or a
    /// derived loss-of-support upstream of this delta function.
    Retract(T),
}

/// A batch of changes for one delta source, bound to the revision it was
/// computed against. `ops` is expected in canonical order (the order the
/// producing store already iterates in — `RelationStore::rows` order, or
/// the WIT boundary's arrival order after the host has sorted it); the ABI
/// does not re-sort, so a producer that fails to hand over canonical order
/// is a toolchain bug, not a semantic difference the consumer should paper
/// over (conformance I.1: incremental must equal full recompute bit-for-bit).
#[derive(Clone, Debug)]
pub struct DeltaBatch<T> {
    /// The revision this batch was computed against.
    pub at: DataRevision,
    /// The ordered changes.
    pub ops: Vec<DeltaOp<T>>,
}

impl<T> DeltaBatch<T> {
    /// An empty batch at `at` — the trivial input for a settle pass that
    /// touches no live change on this source.
    pub fn empty(at: DataRevision) -> Self {
        DeltaBatch {
            at,
            ops: Vec::new(),
        }
    }
}

/// What kind of delta source is feeding a [`DeltaAbi`] implementor — for
/// `perf.*`/diag attribution only, never for control flow inside `apply`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeltaSourceKind {
    /// A rule's delta function over one of its read relations.
    Rule {
        /// The rule.
        rule: RuleRef,
        /// The expression site, when the source is tied to one (`?`
        /// failure attribution, Part III §9).
        site: Option<SiteId>,
    },
    /// A protocol's request relation, driving a guest Driver's `on_request`.
    Protocol {
        /// The protocol's stable name.
        protocol: RelationRef,
    },
}

/// Stable identity of what a [`DeltaAbi`] implementor answers to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeltaSource {
    /// The relation the batch's rows belong to.
    pub relation: RelationRef,
    /// Rule vs. protocol attribution.
    pub kind: DeltaSourceKind,
}

/// The grounding triple for one support instance (Part III §11:
/// `Support(edge, rule, match, atRevision)`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SupportRecord {
    /// The supported edge.
    pub edge: EdgeId,
    /// The rule providing support.
    pub rule: RuleRef,
    /// Digest of the match's variable bindings.
    pub match_digest: MatchDigest,
}

/// A support bookkeeping operation (Part III §2: "Supports are counted;
/// removing the last support removes the edge from live views").
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SupportOp {
    /// A match now grounds this support.
    Add(SupportRecord),
    /// A match no longer grounds this support (its inputs retracted, or the
    /// upstream row it depended on lost its own last support).
    Remove(SupportRecord),
}

/// One row this batch causes to become (or remain) live, paired with the
/// support op(s) it grounds. The common case is exactly one `Add` per fresh
/// match; a row can also be re-emitted with an additional `Add` when a
/// second, independent match starts supporting an already-live edge.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Emission<T> {
    /// The emitted row's edge identity.
    pub edge: EdgeId,
    /// The row itself.
    pub row: T,
    /// Support ops this emission grounds (normally one `Add`).
    pub supports: Vec<SupportOp>,
}

/// A `RuleError` sealed edge produced by a `?` failure inside this batch
/// (Part III §9). Carried alongside emissions/support ops rather than
/// folded into either — it is neither a live row nor a support fact, and
/// conflating it with one would make `RuleError` invisible to callers that
/// only care about the graph's live shape.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RuleErrorEmission {
    /// The failing expression site.
    pub site: SiteId,
    /// Digest of the bindings evaluated up to the failing site — the error
    /// edge's support, so fixing the input retracts it (Part III §9).
    pub partial_match: MatchDigest,
    /// The canon-encoded error value.
    pub error: CanonRow,
}

/// What one `apply` call produces: emissions, the support ops they/their
/// retraction imply, and any `RuleError`s. This is literally "emissions +
/// support ops out" from the brief.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DeltaOutput<T> {
    /// New/continuing live rows and the support they ground.
    pub emissions: Vec<Emission<T>>,
    /// Support bookkeeping not tied to a fresh emission — typically
    /// `Remove`, when an input retraction drops a match with no new row to
    /// emit in its place.
    pub support_ops: Vec<SupportOp>,
    /// `RuleError` sealed edges raised while computing this batch.
    pub errors: Vec<RuleErrorEmission>,
}

impl<T> DeltaOutput<T> {
    /// The output of a batch that changed nothing observable.
    pub fn empty() -> Self {
        DeltaOutput {
            emissions: Vec::new(),
            support_ops: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// The delta ABI: the one shape every delta function, tier-B WASM rule, and
/// guest Driver compiles against (Ring0 §1.7). See the module docs for the
/// two instantiations (tier A native row types vs. [`CanonRow`] at the
/// WASM/Driver boundary).
pub trait DeltaAbi {
    /// The row type flowing across this boundary.
    type Row: Canonical;

    /// Stable identity of the delta source this implementor answers to.
    fn source(&self) -> &DeltaSource;

    /// Consume one batch of changes for the bound revision and produce the
    /// emissions + support ops (+ errors) it implies. Implementors must be
    /// pure functions of `(self, batch)` at a fixed revision — no reading of
    /// ambient state Part III §10 does not name (`sim.Now` is a relation,
    /// read like any other; there is no ambient clock).
    fn apply(&mut self, batch: DeltaBatch<Self::Row>) -> DeltaOutput<Self::Row>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial identity delta function: every inserted row is emitted
    /// back out with one `Add` support op; every retracted row produces a
    /// bare `Remove`. Exercises the trait shape end-to-end.
    struct Echo {
        source: DeltaSource,
        rule: RuleRef,
    }

    impl DeltaAbi for Echo {
        type Row = CanonRow;

        fn source(&self) -> &DeltaSource {
            &self.source
        }

        fn apply(&mut self, batch: DeltaBatch<CanonRow>) -> DeltaOutput<CanonRow> {
            let mut out = DeltaOutput::empty();
            for op in batch.ops {
                match op {
                    DeltaOp::Insert(row) => {
                        let edge = EdgeId::from_canon(&row.0);
                        let m = MatchDigest::of(&self.rule, &row.0);
                        out.emissions.push(Emission {
                            edge,
                            row,
                            supports: vec![SupportOp::Add(SupportRecord {
                                edge,
                                rule: self.rule.clone(),
                                match_digest: m,
                            })],
                        });
                    }
                    DeltaOp::Retract(row) => {
                        let edge = EdgeId::from_canon(&row.0);
                        let m = MatchDigest::of(&self.rule, &row.0);
                        out.support_ops.push(SupportOp::Remove(SupportRecord {
                            edge,
                            rule: self.rule.clone(),
                            match_digest: m,
                        }));
                    }
                }
            }
            out
        }
    }

    #[test]
    fn echo_insert_emits_with_add_support() {
        let mut echo = Echo {
            source: DeltaSource {
                relation: RelationRef::from("shipping.Move"),
                kind: DeltaSourceKind::Rule {
                    rule: RuleRef::from("Echo"),
                    site: None,
                },
            },
            rule: RuleRef::from("Echo"),
        };
        let batch = DeltaBatch {
            at: DataRevision(1),
            ops: vec![DeltaOp::Insert(CanonRow(b"row-1".to_vec()))],
        };
        let out = echo.apply(batch);
        assert_eq!(out.emissions.len(), 1);
        assert_eq!(out.emissions[0].row, CanonRow(b"row-1".to_vec()));
        assert_eq!(out.emissions[0].supports.len(), 1);
        assert!(matches!(out.emissions[0].supports[0], SupportOp::Add(_)));
        assert!(out.errors.is_empty());
    }

    #[test]
    fn echo_retract_emits_bare_remove() {
        let mut echo = Echo {
            source: DeltaSource {
                relation: RelationRef::from("shipping.Move"),
                kind: DeltaSourceKind::Rule {
                    rule: RuleRef::from("Echo"),
                    site: None,
                },
            },
            rule: RuleRef::from("Echo"),
        };
        let batch = DeltaBatch {
            at: DataRevision(2),
            ops: vec![DeltaOp::Retract(CanonRow(b"row-1".to_vec()))],
        };
        let out = echo.apply(batch);
        assert!(out.emissions.is_empty());
        assert_eq!(out.support_ops.len(), 1);
        assert!(matches!(out.support_ops[0], SupportOp::Remove(_)));
    }

    #[test]
    fn empty_batch_yields_empty_output() {
        let mut echo = Echo {
            source: DeltaSource {
                relation: RelationRef::from("shipping.Move"),
                kind: DeltaSourceKind::Protocol {
                    protocol: RelationRef::from("Notify"),
                },
            },
            rule: RuleRef::from("Echo"),
        };
        let out = echo.apply(DeltaBatch::<CanonRow>::empty(DataRevision(0)));
        assert_eq!(out, DeltaOutput::empty());
    }
}
