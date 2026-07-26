//! Wiring the **structural** (`brix.type`) regime through the incremental
//! engine — a deliberately **conservative first cut** (ADR-0002 §9.2;
//! `Build_Plan_v3_SOC.md` Step 6, carry-over point 6).
//!
//! # Read this before reaching for the O(Δ) gate
//!
//! The literal-equality regime ([`crate::literal`]) is **genuinely
//! incremental**: it runs as a committed coalgebra over an `ExecConfig` world,
//! its footprint is exactly its registered configs, and its `apply` touches
//! only the delta — so the armed O(Δ) gate (`soc-core`'s
//! `tests/o_delta_gate.rs`) is scoped to it and passes on the real engine.
//!
//! The structural regime is **not** that shape. It is a *static projection* of
//! an already-analyzed [`ReflectiveReport`] into SOC judgements, produced all
//! at once and keyed by [`ContextId`] — there is no live world of subjects
//! entering and leaving, so there is no honest per-`|Δ|` workload to measure.
//! Rather than invent one (which would be faking the invariant — forbidden by
//! the Step 6 brief and ADR-0002 §9.1), this adapter wires the regime through
//! the [`IncrementalRegime`] *interface* with an explicitly **conservative**
//! `apply`: it recomputes its whole `HasType` candidate projection internally
//! and returns the diff against what it last emitted, under a
//! [`Footprint::AllConfigs`] footprint (it declines to claim it can be
//! skipped). This satisfies "the structural regime is at least wired through
//! the engine" **without** pretending it is `O(|Δ|)`. It is deliberately
//! **excluded** from the armed O(Δ) gate; do not add it there.
//!
//! What it *does* give, honestly:
//! - a real [`IncrementalRegime`] implementation the engine can drive, whose
//!   materialized candidate view equals the structural projection's `HasType`
//!   edge set (checked below and in `brix-conformance`);
//! - a stable [`soc_core::Candidate`] encoding of each `HasType` projection
//!   (`regime`/`witness`/`successor` handles interned from the projection's
//!   own `RegimeId`/`WitnessId`/`PropositionId` digests), so the structural
//!   regime finally produces engine-shaped candidates at all.

use std::collections::BTreeSet;

use brix_ir::reflect::ReflectiveReport;
use brix_semantic::ContextId;

use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::IncrementalRegime;
use soc_core::intern::{Handle, Interner};
use soc_core::regime::Candidate;

use crate::structural::StructuralRegime;

/// A conservative [`IncrementalRegime`] adapter over [`StructuralRegime`] (see
/// the module docs — this is a wiring, not a genuine `O(|Δ|)` operator).
pub struct StructuralIncremental {
    regime: StructuralRegime,
    ctx: ContextId,
    report: ReflectiveReport,
    /// Owns the handle minting for this adapter's candidates. Self-contained:
    /// the candidate handles are only ever compared within one adapter's own
    /// materialized view, so a private interner is sufficient and keeps the
    /// adapter free-standing.
    interner: Interner,
    /// The candidate set this adapter last emitted, so a (conservative)
    /// recompute can be diffed into a [`CandidateDelta`].
    emitted: BTreeSet<Candidate>,
    /// The regime's own interned identity handle (stable for every candidate).
    regime_handle: Handle,
}

impl StructuralIncremental {
    /// Wire `regime` over `report` under `ctx`. The adapter starts having
    /// emitted nothing; the first [`IncrementalRegime::apply`] materializes
    /// the full `HasType` projection.
    pub fn new(regime: StructuralRegime, report: ReflectiveReport, ctx: ContextId) -> Self {
        let mut interner = Interner::new();
        let regime_handle = interner.intern(regime.regime_id().digest());
        StructuralIncremental {
            regime,
            ctx,
            report,
            interner,
            emitted: BTreeSet::new(),
            regime_handle,
        }
    }

    /// The engine-shaped candidate set of the current `HasType` projection: one
    /// [`Candidate`] per projected `HasType` edge, its `witness`/`successor`
    /// handles interned from the projection's own `WitnessId`/`PropositionId`
    /// digests. This is the full recompute the conservative `apply` diffs.
    fn project_candidates(&mut self) -> BTreeSet<Candidate> {
        let projection = self.regime.project(&self.report, self.ctx);
        let regime_handle = self.regime_handle;
        let mut out = BTreeSet::new();
        for p in &projection.has_type {
            let witness = self.interner.intern(p.witness_id.digest());
            let successor = self.interner.intern(p.proposition.digest());
            out.insert(Candidate {
                regime: regime_handle,
                witness,
                successor,
            });
        }
        out
    }
}

impl IncrementalRegime for StructuralIncremental {
    /// [`Footprint::AllConfigs`] — the adapter declines to claim it can be
    /// skipped (module docs: this is the conservative wiring, not a genuine
    /// footprint feed).
    fn footprint(&self) -> Footprint {
        Footprint::AllConfigs
    }

    /// **Conservative** (module docs): recompute the whole `HasType`
    /// projection and return the diff against what was last emitted, ignoring
    /// the delta's structure. Idempotent after the first call for a fixed
    /// report — the second `apply` yields an empty candidate delta. This is
    /// *not* an `O(|Δ|)` operator and is excluded from the armed O(Δ) gate.
    fn apply(&mut self, _delta: &Delta) -> CandidateDelta {
        let next = self.project_candidates();
        let mut cd = CandidateDelta::new();
        for c in next.difference(&self.emitted) {
            cd.added.insert(*c);
        }
        for c in self.emitted.difference(&next) {
            cd.removed.insert(*c);
        }
        self.emitted = next;
        cd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_ir::frontend::{FrontendSource, TableResolver};
    use brix_ir::ident::Ident;
    use brix_ir::reflect::analyze;
    use brix_ir::types::{IntWidth, Ty};
    use soc_core::engine::IncrementalEngine;

    fn one_has_type_report() -> ReflectiveReport {
        use brix_ir::core::{Query, SourceRange};
        use brix_ir::pattern::Pattern;
        let origin = brix_ir::core::ExprOrigin::source(
            &Ident::new("Probe"),
            SourceRange { start: 0, end: 1 },
        );
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![Query {
                name: Ident::new("Probe"),
                params: vec![(Ident::new("n"), Ty::Int(IntWidth::Int))],
                body: Pattern::default(),
                yields: brix_ir::core::Expr::new(
                    Ty::Int(IntWidth::Int),
                    brix_ir::core::ExprKind::Var(Ident::new("n")),
                )
                .with_origin(origin),
                result: Ty::rel(brix_ir::types::Row::closed(vec![
                    brix_ir::types::RowField {
                        name: Ident::new("value"),
                        ty: Ty::Int(IntWidth::Int),
                    },
                ])),
            }],
        };
        analyze(&source, &TableResolver::new())
    }

    #[test]
    fn footprint_is_all_configs_conservative_wiring() {
        let adapter = StructuralIncremental::new(
            StructuralRegime::new(),
            one_has_type_report(),
            ContextId::root(),
        );
        assert_eq!(adapter.footprint(), Footprint::AllConfigs);
    }

    #[test]
    fn first_apply_materializes_the_has_type_edges_then_is_idempotent() {
        let report = one_has_type_report();
        let mut adapter =
            StructuralIncremental::new(StructuralRegime::new(), report, ContextId::root());

        let first = adapter.apply(&Delta::new());
        assert!(
            !first.added.is_empty(),
            "the structural projection must yield at least one HasType candidate"
        );
        assert!(first.removed.is_empty());

        // Fixed report ⇒ a second apply is a no-op (conservative recompute
        // finds no change).
        let second = adapter.apply(&Delta::new());
        assert!(
            second.is_empty(),
            "a fixed report must produce an empty candidate delta on re-apply"
        );
    }

    #[test]
    fn driven_through_the_engine_the_view_is_the_has_type_edge_set() {
        let report = one_has_type_report();
        let expected = {
            let mut a = StructuralIncremental::new(
                StructuralRegime::new(),
                report.clone(),
                ContextId::root(),
            );
            a.project_candidates()
        };
        let adapter =
            StructuralIncremental::new(StructuralRegime::new(), report, ContextId::root());
        let mut engine = IncrementalEngine::new(vec![Box::new(adapter)]);
        // Any non-empty delta triggers the AllConfigs regime's recompute.
        let mut i = Interner::new();
        let probe = i.intern(brix_canon::Digest::of(brix_canon::Domain::Value, b"probe"));
        engine.step(&Delta::of_added([probe]));
        assert_eq!(engine.view(), &expected);
        assert!(!engine.view().is_empty());
    }
}
