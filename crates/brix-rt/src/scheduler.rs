//! Deterministic single-settler scheduler for native and canon-row delta
//! functions.
//!
//! This is the runtime half of the generated-engine shell: transaction input
//! changes are batched by relation, scheduled phase-by-phase, and propagated
//! until no derived row changes remain.  It deliberately works at the
//! [`CanonRow`] ABI boundary; generated tier-A rules can keep typed rows
//! internally and adapt at their one registration point, while tier-B/Driver
//! rules already use this form directly.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain, EdgeId};

use crate::delta::{CanonRow, DeltaAbi, DeltaBatch, DeltaOp, RuleErrorEmission, SupportOp};
use crate::ids::{DataRevision, RelationRef, RuleRef, SupportRef};

/// One source transaction accepted by the generated engine.  Operations are
/// ordered within their transaction; separate transactions publish separate,
/// fully settled revisions.
#[derive(Clone, Debug, Default)]
pub struct Transaction {
    pub intent: Vec<u8>,
    pub ops: Vec<TransactionOp>,
}

impl Transaction {
    pub fn new(intent: impl Into<Vec<u8>>) -> Self {
        Self {
            intent: intent.into(),
            ops: Vec::new(),
        }
    }

    pub fn assert(mut self, relation: impl Into<RelationRef>, row: CanonRow) -> Self {
        self.ops.push(TransactionOp::Assert {
            relation: relation.into(),
            row,
        });
        self
    }

    pub fn retract(mut self, relation: impl Into<RelationRef>, row: CanonRow) -> Self {
        self.ops.push(TransactionOp::Retract {
            relation: relation.into(),
            row,
        });
        self
    }
}

/// One ground fact change in a [`Transaction`].  Source rows are idempotent
/// by their relation-scoped canon identity; duplicate asserts/retracts are
/// no-ops and therefore cannot create schedule-order differences.
#[derive(Clone, Debug)]
pub enum TransactionOp {
    Assert {
        relation: RelationRef,
        row: CanonRow,
    },
    Retract {
        relation: RelationRef,
        row: CanonRow,
    },
}

/// A fully settled runtime revision, suitable for canonical comparison.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settled {
    pub revision: DataRevision,
    pub extents: BTreeMap<RelationRef, BTreeMap<EdgeId, CanonRow>>,
    pub errors: Vec<SettledRuleError>,
}

/// A `?` failure that remains observable at the settled boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettledRuleError {
    pub rule: RuleRef,
    pub error: RuleErrorEmission,
}

/// Canonical bytes for a settled generated-engine revision.  This is kept at
/// the runtime boundary so generated binaries can write byte-comparable dumps
/// without reimplementing ordering or a second serializer.
pub fn dump_bytes(settled: &Settled) -> Vec<u8> {
    let mut writer = CanonWriter::new();
    writer.write_uint(settled.revision.0);
    writer.write_uint(settled.extents.len() as u64);
    for (relation, extent) in &settled.extents {
        relation.canon_write(&mut writer);
        writer.write_uint(extent.len() as u64);
        for (edge, row) in extent {
            writer.write_bytes(edge.digest().as_bytes());
            row.canon_write(&mut writer);
        }
    }
    writer.write_uint(settled.errors.len() as u64);
    for error in &settled.errors {
        error.rule.canon_write(&mut writer);
        error.error.site.canon_write(&mut writer);
        error.error.partial_match.canon_write(&mut writer);
        error.error.error.canon_write(&mut writer);
    }
    writer.finish()
}

/// Digest of [`dump_bytes`], used to localize a differential mismatch to one
/// revision before comparing its complete bytes.
pub fn dump_digest(settled: &Settled) -> Digest {
    Digest::of(Domain::Value, &dump_bytes(settled))
}

struct RegisteredRule {
    phase: u32,
    target: RelationRef,
    implementation: Box<dyn DeltaAbi<Row = CanonRow>>,
}

/// The single-settler runtime.  Rules are registered with their target
/// relation and phase; the scheduler owns all observable ordering thereafter.
pub struct Scheduler {
    rules: Vec<RegisteredRule>,
    direct: BTreeSet<(RelationRef, EdgeId)>,
    supports: BTreeMap<SupportRef, (RelationRef, EdgeId, CanonRow)>,
    settled: Settled,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            direct: BTreeSet::new(),
            supports: BTreeMap::new(),
            settled: Settled::default(),
        }
    }

    /// Register one generated delta function.  The explicit `phase` is the
    /// compiler's Appendix-F result; rules in a phase are deterministically
    /// ordered by source relation and rule identity.
    pub fn register_rule(
        &mut self,
        phase: u32,
        target: impl Into<RelationRef>,
        implementation: Box<dyn DeltaAbi<Row = CanonRow>>,
    ) {
        self.rules.push(RegisteredRule {
            phase,
            target: target.into(),
            implementation,
        });
        self.rules.sort_by(|left, right| {
            rule_key(left)
                .cmp(&rule_key(right))
                .then_with(|| left.target.cmp(&right.target))
        });
    }

    pub fn current(&self) -> &Settled {
        &self.settled
    }

    /// Commit one transaction, then run all affected rule deltas to a fixed
    /// point.  No intermediate state is observable: callers receive only the
    /// newly published settled revision.
    pub fn commit(&mut self, transaction: Transaction) -> &Settled {
        let revision = self.settled.revision.next();
        // Every phase receives the revision's accumulated deltas. A phase may
        // add more deltas while it reaches its own positive fixed point; those
        // additions are then available to every later phase, never to an
        // earlier one. This is the Appendix-F ordering contract in runtime
        // form and keeps same-phase recursion distinct from stratification.
        let mut changes: BTreeMap<RelationRef, Vec<DeltaOp<CanonRow>>> = BTreeMap::new();

        for operation in transaction.ops {
            match operation {
                TransactionOp::Assert { relation, row } => {
                    let edge = edge_for_row(&relation, &row);
                    let extent = self.settled.extents.entry(relation.clone()).or_default();
                    if extent.insert(edge, row.clone()).is_none() {
                        self.direct.insert((relation.clone(), edge));
                        changes
                            .entry(relation)
                            .or_default()
                            .push(DeltaOp::Insert(row));
                    }
                }
                TransactionOp::Retract { relation, row } => {
                    let edge = edge_for_row(&relation, &row);
                    if !self.direct.remove(&(relation.clone(), edge)) {
                        continue;
                    }
                    if !has_support(&self.supports, &relation, edge) {
                        if let Some(extent) = self.settled.extents.get_mut(&relation) {
                            if let Some(removed) = extent.remove(&edge) {
                                changes
                                    .entry(relation)
                                    .or_default()
                                    .push(DeltaOp::Retract(removed));
                            }
                        }
                    }
                }
            }
        }

        self.settled.revision = revision;
        self.settled.errors.clear();
        let phases: BTreeSet<u32> = self.rules.iter().map(|rule| rule.phase).collect();
        for phase in phases {
            let mut pending = changes.clone();
            while let Some((relation, operations)) = pending.pop_first() {
                let outputs: Vec<_> = self
                    .rules
                    .iter_mut()
                    .filter(|registered| {
                        registered.phase == phase
                            && registered.implementation.source().relation == relation
                    })
                    .map(|registered| {
                        (
                            registered.target.clone(),
                            source_rule(registered.implementation.source()),
                            registered.implementation.apply(DeltaBatch {
                                at: revision,
                                ops: operations.clone(),
                            }),
                        )
                    })
                    .collect();
                for (target, rule, output) in outputs {
                    for error in output.errors {
                        self.settled.errors.push(SettledRuleError {
                            rule: rule.clone(),
                            error,
                        });
                    }
                    for emission in output.emissions {
                        for support in emission.supports {
                            if let Some((relation, operation)) =
                                self.apply_support(support, &target, Some(emission.row.clone()))
                            {
                                pending
                                    .entry(relation.clone())
                                    .or_default()
                                    .push(operation.clone());
                                changes.entry(relation).or_default().push(operation);
                            }
                        }
                    }
                    for support in output.support_ops {
                        if let Some((relation, operation)) =
                            self.apply_support(support, &target, None)
                        {
                            pending
                                .entry(relation.clone())
                                .or_default()
                                .push(operation.clone());
                            changes.entry(relation).or_default().push(operation);
                        }
                    }
                }
            }
        }
        self.settled.errors.sort_by(|left, right| {
            left.rule
                .cmp(&right.rule)
                .then_with(|| left.error.site.cmp(&right.error.site))
                .then_with(|| left.error.partial_match.cmp(&right.error.partial_match))
        });
        &self.settled
    }

    fn apply_support(
        &mut self,
        operation: SupportOp,
        target: &RelationRef,
        row: Option<CanonRow>,
    ) -> Option<(RelationRef, DeltaOp<CanonRow>)> {
        match operation {
            SupportOp::Add(record) => {
                let support = SupportRef::of(record.edge, &record.rule, record.match_digest);
                let row = row?;
                if self
                    .supports
                    .insert(support, (target.clone(), record.edge, row.clone()))
                    .is_none()
                {
                    let extent = self.settled.extents.entry(target.clone()).or_default();
                    if extent.insert(record.edge, row.clone()).is_none() {
                        return Some((target.clone(), DeltaOp::Insert(row)));
                    }
                }
            }
            SupportOp::Remove(record) => {
                let support = SupportRef::of(record.edge, &record.rule, record.match_digest);
                let (relation, edge, row) = self.supports.remove(&support)?;
                if !self.direct.contains(&(relation.clone(), edge))
                    && !has_support(&self.supports, &relation, edge)
                {
                    if let Some(extent) = self.settled.extents.get_mut(&relation) {
                        if extent.remove(&edge).is_some() {
                            return Some((relation, DeltaOp::Retract(row)));
                        }
                    }
                }
            }
        }
        None
    }
}

/// Relation-scoped edge identity for a canonical ABI row. Generated native
/// delta modules use this when they emit a target-row support record.
pub fn edge_for_row(relation: &RelationRef, row: &CanonRow) -> EdgeId {
    let mut writer = CanonWriter::new();
    relation.canon_write(&mut writer);
    row.canon_write(&mut writer);
    EdgeId::from_canon(&writer.finish())
}

fn has_support(
    supports: &BTreeMap<SupportRef, (RelationRef, EdgeId, CanonRow)>,
    relation: &RelationRef,
    edge: EdgeId,
) -> bool {
    supports
        .values()
        .any(|(supported_relation, supported_edge, _)| {
            supported_relation == relation && *supported_edge == edge
        })
}

fn rule_key(rule: &RegisteredRule) -> (u32, &RelationRef, RuleRef) {
    (
        rule.phase,
        &rule.implementation.source().relation,
        source_rule(rule.implementation.source()),
    )
}

fn source_rule(source: &crate::delta::DeltaSource) -> RuleRef {
    match &source.kind {
        crate::delta::DeltaSourceKind::Rule { rule, .. } => rule.clone(),
        crate::delta::DeltaSourceKind::Protocol { protocol } => {
            RuleRef(protocol.as_str().to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{DeltaOutput, DeltaSource, DeltaSourceKind, Emission, SupportRecord};
    use crate::ids::MatchDigest;

    struct Echo {
        source: DeltaSource,
        rule: RuleRef,
        target: RelationRef,
    }

    impl DeltaAbi for Echo {
        type Row = CanonRow;

        fn source(&self) -> &DeltaSource {
            &self.source
        }

        fn apply(&mut self, batch: DeltaBatch<CanonRow>) -> DeltaOutput<CanonRow> {
            let mut output = DeltaOutput::empty();
            for operation in batch.ops {
                match operation {
                    DeltaOp::Insert(row) => {
                        let edge = edge_for_row(&self.target, &row);
                        let digest = MatchDigest::of(&self.rule, &row.0);
                        output.emissions.push(Emission {
                            edge,
                            row,
                            supports: vec![SupportOp::Add(SupportRecord {
                                edge,
                                rule: self.rule.clone(),
                                match_digest: digest,
                            })],
                        });
                    }
                    DeltaOp::Retract(row) => {
                        let edge = edge_for_row(&self.target, &row);
                        output.support_ops.push(SupportOp::Remove(SupportRecord {
                            edge,
                            rule: self.rule.clone(),
                            match_digest: MatchDigest::of(&self.rule, &row.0),
                        }));
                    }
                }
            }
            output
        }
    }

    fn scheduler() -> Scheduler {
        let mut scheduler = Scheduler::new();
        scheduler.register_rule(
            0,
            "Derived",
            Box::new(Echo {
                source: DeltaSource {
                    relation: RelationRef::from("Input"),
                    kind: DeltaSourceKind::Rule {
                        rule: RuleRef::from("Copy"),
                        site: None,
                    },
                },
                rule: RuleRef::from("Copy"),
                target: RelationRef::from("Derived"),
            }),
        );
        scheduler
    }

    #[test]
    fn settles_source_deltas_and_retracts_last_support() {
        let row = CanonRow(b"one".to_vec());
        let mut scheduler = scheduler();
        let first = scheduler.commit(Transaction::new(b"t1").assert("Input", row.clone()));
        assert_eq!(first.revision, DataRevision(1));
        assert_eq!(first.extents[&RelationRef::from("Derived")].len(), 1);
        let first_dump = dump_bytes(first);
        assert_eq!(first_dump, dump_bytes(first));

        let second = scheduler.commit(Transaction::new(b"t2").retract("Input", row));
        assert_eq!(second.revision, DataRevision(2));
        assert!(second.extents[&RelationRef::from("Derived")].is_empty());
    }

    #[test]
    fn duplicate_ground_assert_is_idempotent() {
        let row = CanonRow(b"one".to_vec());
        let mut scheduler = scheduler();
        scheduler.commit(Transaction::new(b"t1").assert("Input", row.clone()));
        let settled = scheduler.commit(Transaction::new(b"t2").assert("Input", row));
        assert_eq!(settled.extents[&RelationRef::from("Input")].len(), 1);
        assert_eq!(settled.extents[&RelationRef::from("Derived")].len(), 1);
    }

    #[test]
    fn later_phase_output_never_reenters_an_earlier_phase() {
        let mut scheduler = Scheduler::new();
        // This deliberately backward registration would be rejected by
        // brix-phase for a real program. It proves the scheduler itself also
        // respects the phase barrier rather than accidentally looping over
        // newly created higher-phase rows in an earlier phase.
        scheduler.register_rule(
            0,
            "Skipped",
            Box::new(Echo {
                source: DeltaSource {
                    relation: RelationRef::from("Intermediate"),
                    kind: DeltaSourceKind::Rule {
                        rule: RuleRef::from("TooEarly"),
                        site: None,
                    },
                },
                rule: RuleRef::from("TooEarly"),
                target: RelationRef::from("Skipped"),
            }),
        );
        scheduler.register_rule(
            1,
            "Intermediate",
            Box::new(Echo {
                source: DeltaSource {
                    relation: RelationRef::from("Input"),
                    kind: DeltaSourceKind::Rule {
                        rule: RuleRef::from("Later"),
                        site: None,
                    },
                },
                rule: RuleRef::from("Later"),
                target: RelationRef::from("Intermediate"),
            }),
        );

        let settled =
            scheduler.commit(Transaction::new(b"t").assert("Input", CanonRow(b"x".to_vec())));
        assert_eq!(settled.extents[&RelationRef::from("Intermediate")].len(), 1);
        assert!(settled
            .extents
            .get(&RelationRef::from("Skipped"))
            .is_none_or(BTreeMap::is_empty));
    }
}
