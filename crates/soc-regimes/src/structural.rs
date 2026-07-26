//! The **structural** (`brix.type`) realization regime — `brix.type`
//! semantics returning as a *client regime* (ADR-0002 §7, §12;
//! `Build_Plan_v3_SOC.md` Step 5(b)).
//!
//! This module does **not** reimplement type inference. It CONSUMES the
//! retained old engine's [`brix_ir::reflect::analyze`] — the differential
//! oracle ADR-0002 §12 keeps deliberately — and re-projects its
//! [`ReflectiveReport`] into SOC artifacts:
//!
//! - **`Fact` → `Proposition`.** Every [`Derivation`] becomes a canonical
//!   proposition under a given [`ContextId`]. A [`Fact::HasType`] becomes a
//!   [`Realizes`] proposition (below); every other fact kind gets a general
//!   fact-proposition encoding ([`GenericFactProposition`]) — context-first,
//!   then the fact's own frozen [`brix_ir::reflect::write_fact`] bytes,
//!   verbatim (never a second, competing fact encoder).
//! - **`HasType` → one `Realizes` judgement, `Outcome::Derived`.** For each
//!   `Fact::HasType{subject, ty, scope}`, a [`Witness`] is built whose `src`
//!   is the subject's own canonical [`ConfigId`] and whose `dst` is the
//!   canonical `ConfigId` of the *typed* configuration (subject + the
//!   resolved type together) — a principled, stable mapping: "the subject
//!   (as a bare configuration) realizes the subject-with-this-type (as a
//!   richer configuration) under this regime's HasType witness." The
//!   resulting [`Judgement`] is always `Outcome::Derived` — a regime never
//!   publishes anything stronger (ADR-0002 §5 point 4, §7).
//! - **because-sets → labelled evidence.** The judgement's [`Evidence`] is an
//!   [`Evidence::SettlementReplay`] whose body digests a small
//!   [`HasTypeProvenance`] record naming *which* typing relation produced the
//!   fact (a fixed label) plus the premise [`FactId`]s from
//!   `Derivation.because` — the fact's own provenance, carried through, not
//!   discarded.
//! - **content-addressed context extension through the root anchor.** Every
//!   entry point here takes a [`ContextId`] parameter; the root case is
//!   exactly [`ContextId::root`] (== `ScopeId::root()`, the ADR-0002 §6.1
//!   hinge). A non-root case uses [`ContextId::extend`] (the additive
//!   brix-semantic change this slice adds) — see
//!   `crates/brix-conformance/tests/structural_regime.rs`'s
//!   `ScopedWorldNonLeak` gate for the property this unlocks.
//!
//! **Conflicts** project too, but honestly: a regime may never publish
//! `Refuted` (only `brix-kernel` may, ADR-0002 §4.1) — a structural conflict
//! is exactly a case where the regime has something to *report* but nothing
//! to *certify*, so it projects to an `Outcome::Unknown` judgement (ADR-0002
//! §5 point 3, "any regime may emit `Unknown(reason)`"), retaining the
//! original [`ConflictKind`] for outside callers (e.g. the differential gate)
//! to classify independently — this module does not itself define a
//! `Category`/`RuleCategory` mapping (that vocabulary lives in
//! `brix-conformance`, a downstream crate this one does not depend on).
//!
//! **Scope note (ADR-0002 §11, "not enlarging `brix.type` into a universal
//! library").** This is deliberately a *projection* layer, not a rewritten
//! checker: no unification, no new type algebra, nothing beyond re-encoding
//! `analyze`'s own output as SOC artifacts. It does not (yet) implement
//! `soc_core::Regime`/`SettlementRegime` — unlike [`crate::literal`]'s
//! reflexive regime, a `HasType` candidate is not naturally a *committed
//! coalgebra step* over an `ExecConfig` world/policy/history triple (it is a
//! static re-projection of a whole already-analyzed source), so wiring this
//! regime through `soc_core::commit::commit_tick` is left to a later slice;
//! this module instead provides the projection plus an end-to-end-tested
//! `HasType` → `Realizes` → `Derived` judgement (this file's tests), per the
//! task brief's documented fallback.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_ir::reflect::{
    write_conflict, write_fact, ConflictKind, Fact, FactId, ReflectiveReport, Subject, TypeConflict,
};
use brix_ir::types::Ty;
use brix_semantic::{
    ConfigId, ContextId, Evidence, Judgement, JudgementId, Outcome, PropositionId, Realizes,
    RegimeId, Witness, WitnessId,
};

/// The label naming *which* typing relation produced a projected `HasType`
/// fact — carried into the judgement's evidence (module doc). Fixed for now:
/// every `HasType` derivation reflect emits comes from the same relation
/// (`Fact::HasType`); a future slice with multiple structural sub-relations
/// would parameterize this per fact-kind instead.
const HAS_TYPE_PROVENANCE_LABEL: &str = "brix_ir.reflect.Fact.HasType";

/// The label naming a projected [`TypeConflict`]'s provenance, mirroring
/// [`HAS_TYPE_PROVENANCE_LABEL`].
const CONFLICT_PROVENANCE_LABEL: &str = "brix_ir.reflect.TypeConflict";

/// The structural (`brix.type`) realization regime (ADR-0002 §7, §12).
#[derive(Clone, Copy, Debug)]
pub struct StructuralRegime {
    regime_id: RegimeId,
}

impl StructuralRegime {
    /// This regime's canonical name, versioned per ADR-0002 §6
    /// (`RegimeId::named`'s `name@version` convention) — matches the task
    /// brief's `brix.type.structural@0.1`.
    pub const NAME: &'static str = "brix.type.structural@0.1";

    /// Construct the regime, interning its own [`RegimeId`] digest.
    pub fn new() -> Self {
        StructuralRegime {
            regime_id: RegimeId::named(Self::NAME),
        }
    }

    /// This regime's canonical identity.
    pub fn regime_id(&self) -> RegimeId {
        self.regime_id
    }

    /// Project one `Fact::HasType{subject, ty, ..}` into a `Realizes`
    /// judgement under `ctx` (module doc). `fact_id`/`because` are the
    /// originating [`FactId`] and its premise set, threaded into the
    /// judgement's evidence — never discarded.
    fn project_has_type(
        &self,
        subject: &Subject,
        ty: &Ty,
        ctx: ContextId,
        because: &BTreeSet<FactId>,
    ) -> HasTypeProjection {
        let src = ConfigId::of(subject);
        let dst = ConfigId::of(&SubjectHasType { subject, ty });
        let witness = Witness::new(src, dst, self.regime_id);
        let witness_id = witness.id();
        let proposition = Realizes::new(witness_id, src, dst).proposition_id();

        let provenance = HasTypeProvenance {
            label: HAS_TYPE_PROVENANCE_LABEL,
            because,
        };
        let evidence = Evidence::SettlementReplay {
            body: Digest::of(Domain::Value, &provenance.canon_bytes()),
        }
        .id();

        let judgement = Judgement::new(ctx, proposition, Outcome::Derived, evidence);

        HasTypeProjection {
            subject: subject.clone(),
            ty: ty.clone(),
            witness_id,
            proposition,
            judgement,
        }
    }

    /// Project every fact in `report` under `ctx`: `HasType` facts become
    /// [`HasTypeProjection`]s (via [`Self::project_has_type`]); every other
    /// fact kind gets a general fact-proposition id (module doc). Returns the
    /// full [`StructuralProjection`], keyed by each fact's original
    /// [`FactId`] so callers (e.g. the shadow-parity gate) can trace a
    /// projected artifact back to the fact that produced it.
    pub fn project(&self, report: &ReflectiveReport, ctx: ContextId) -> StructuralProjection {
        let mut propositions = BTreeMap::new();
        let mut has_type = Vec::new();
        let mut judgement_ids = BTreeSet::new();

        for derivation in &report.facts {
            match &derivation.fact {
                Fact::HasType { subject, ty, .. } => {
                    let projected = self.project_has_type(subject, ty, ctx, &derivation.because);
                    propositions.insert(derivation.id, projected.proposition);
                    judgement_ids.insert(projected.judgement.id());
                    has_type.push(projected);
                }
                other => {
                    let proposition =
                        PropositionId::of(&GenericFactProposition { ctx, fact: other });
                    propositions.insert(derivation.id, proposition);
                }
            }
        }

        StructuralProjection {
            propositions,
            has_type,
            judgement_ids,
        }
    }

    /// Project one [`TypeConflict`] into an `Outcome::Unknown` judgement
    /// (module doc — a regime may never publish `Refuted`), retaining the
    /// original [`ConflictKind`] for the caller's own classification.
    fn project_conflict(&self, conflict: &TypeConflict, ctx: ContextId) -> ProjectedConflict {
        let proposition = PropositionId::of(&ConflictProposition { ctx, conflict });

        let provenance = ConflictProvenance {
            label: CONFLICT_PROVENANCE_LABEL,
            because: &conflict.because,
        };
        let evidence = Evidence::SettlementReplay {
            body: Digest::of(Domain::Value, &provenance.canon_bytes()),
        }
        .id();

        let judgement = Judgement::new(ctx, proposition, Outcome::Unknown, evidence);

        ProjectedConflict {
            kind: conflict.kind.clone(),
            subject: conflict.subject.clone(),
            proposition,
            judgement,
        }
    }

    /// Project every conflict in `report` under `ctx`.
    pub fn project_conflicts(
        &self,
        report: &ReflectiveReport,
        ctx: ContextId,
    ) -> Vec<ProjectedConflict> {
        report
            .conflicts
            .iter()
            .map(|c| self.project_conflict(c, ctx))
            .collect()
    }
}

impl Default for StructuralRegime {
    fn default() -> Self {
        Self::new()
    }
}

/// The canonical encoding of a `(subject, ty)` pair — the "subject,
/// configured with this resolved type" configuration a `HasType` witness's
/// `dst` digests to (module doc). Field order (`subject`, `ty`) is this
/// module's own ABI (not shared with `reflect::write_fact`'s `HasType` arm,
/// which additionally interleaves `scope` — this is a *projection*
/// encoding, deliberately distinct from the oracle's own fact encoder).
struct SubjectHasType<'a> {
    subject: &'a Subject,
    ty: &'a Ty,
}

impl Canonical for SubjectHasType<'_> {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.subject.canon_write(w);
        self.ty.canon_write(w);
    }
}

/// The general (non-`HasType`) fact-proposition encoding (module doc):
/// context-first, then the fact's own frozen [`write_fact`] bytes verbatim —
/// never a second, competing fact encoder.
struct GenericFactProposition<'a> {
    ctx: ContextId,
    fact: &'a Fact,
}

impl Canonical for GenericFactProposition<'_> {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.ctx.canon_write(w);
        write_fact(self.fact, w);
    }
}

/// The canonical encoding of a projected [`TypeConflict`]'s proposition:
/// context-first, then the conflict's own frozen
/// [`brix_ir::reflect::write_conflict`] bytes verbatim.
struct ConflictProposition<'a> {
    ctx: ContextId,
    conflict: &'a TypeConflict,
}

impl Canonical for ConflictProposition<'_> {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.ctx.canon_write(w);
        write_conflict(self.conflict, w);
    }
}

/// The provenance record folded into a projected `HasType` judgement's
/// evidence (module doc): a fixed label naming the typing relation, plus the
/// premise [`FactId`]s from the fact's own `because` set.
struct HasTypeProvenance<'a> {
    label: &'static str,
    because: &'a BTreeSet<FactId>,
}

impl Canonical for HasTypeProvenance<'_> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_str(self.label);
        w.write_set(self.because.iter().map(|f| f.digest().as_bytes().to_vec()));
    }
}

/// The provenance record folded into a projected conflict's evidence,
/// mirroring [`HasTypeProvenance`].
struct ConflictProvenance<'a> {
    label: &'static str,
    because: &'a BTreeSet<FactId>,
}

impl Canonical for ConflictProvenance<'_> {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_str(self.label);
        w.write_set(self.because.iter().map(|f| f.digest().as_bytes().to_vec()));
    }
}

/// One projected `HasType` fact: the retained `(subject, ty)` content (so
/// callers can independently reconstruct the originating `FactId` — see the
/// shadow-parity gate), the witness/proposition chain, and the resulting
/// `Outcome::Derived` `Realizes` judgement.
#[derive(Clone, Debug)]
pub struct HasTypeProjection {
    pub subject: Subject,
    pub ty: Ty,
    pub witness_id: WitnessId,
    pub proposition: PropositionId,
    pub judgement: Judgement,
}

/// One projected [`TypeConflict`]: the original [`ConflictKind`] (retained,
/// not hidden — this module does not itself define a `Category`/
/// `RuleCategory` mapping, see module doc), the conflict's own subject, its
/// projected proposition, and its `Outcome::Unknown` judgement.
#[derive(Clone, Debug)]
pub struct ProjectedConflict {
    pub kind: ConflictKind,
    pub subject: Subject,
    pub proposition: PropositionId,
    pub judgement: Judgement,
}

/// The result of projecting a whole [`ReflectiveReport`] under one
/// [`ContextId`] (module doc).
#[derive(Clone, Debug, Default)]
pub struct StructuralProjection {
    /// Every fact's projected proposition, keyed by its original `FactId`.
    pub propositions: BTreeMap<FactId, PropositionId>,
    /// `HasType` facts specifically, each realized as a `Derived` `Realizes`
    /// judgement.
    pub has_type: Vec<HasTypeProjection>,
    /// The `JudgementId`s of every `HasType` judgement in this projection —
    /// used by the `ScopedWorldNonLeak` gate to check non-leak membership.
    pub judgement_ids: BTreeSet<JudgementId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_ir::frontend::TableResolver;
    use brix_ir::ident::Ident;
    use brix_ir::reflect::analyze;
    use brix_ir::types::IntWidth;

    /// A minimal `FrontendSource` with exactly one query parameter — one
    /// `HasType` fact, zero conflicts. Enough to exercise the projection
    /// end-to-end without pulling in the whole corpus (that lives in
    /// `brix-conformance`'s differential gate).
    fn one_has_type_source() -> brix_ir::frontend::FrontendSource {
        use brix_ir::core::{Query, SourceRange};
        use brix_ir::pattern::Pattern;
        use brix_ir::types::Ty;

        let origin = brix_ir::core::ExprOrigin::source(
            &Ident::new("Probe"),
            SourceRange { start: 0, end: 1 },
        );
        brix_ir::frontend::FrontendSource {
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
        }
    }

    #[test]
    fn regime_id_is_the_named_structural_id() {
        let r = StructuralRegime::new();
        assert_eq!(r.regime_id(), RegimeId::named(StructuralRegime::NAME));
    }

    #[test]
    fn projecting_a_has_type_fact_yields_one_derived_realizes_judgement() {
        let source = one_has_type_source();
        let resolver = TableResolver::new();
        let report = analyze(&source, &resolver);
        assert!(report.is_consistent());

        let regime = StructuralRegime::new();
        let projection = regime.project(&report, ContextId::root());

        // At least one HasType fact (the "n" param binding); every one must
        // be Outcome::Derived under the root context.
        assert!(!projection.has_type.is_empty());
        for p in &projection.has_type {
            assert_eq!(p.judgement.outcome, Outcome::Derived);
            assert_eq!(p.judgement.context, ContextId::root());
            assert_eq!(p.judgement.proposition, p.proposition);
        }
    }

    #[test]
    fn same_subject_and_type_give_the_same_witness_and_proposition() {
        let regime = StructuralRegime::new();
        let subject = Subject::Binding {
            declaration: Ident::new("D"),
            name: Ident::new("x"),
        };
        let ty = Ty::Int(IntWidth::Int);
        let because = BTreeSet::new();

        let a = regime.project_has_type(&subject, &ty, ContextId::root(), &because);
        let b = regime.project_has_type(&subject, &ty, ContextId::root(), &because);
        assert_eq!(a.witness_id, b.witness_id);
        assert_eq!(a.proposition, b.proposition);
        assert_eq!(a.judgement, b.judgement);
    }

    #[test]
    fn distinct_types_give_distinct_witnesses_and_propositions() {
        let regime = StructuralRegime::new();
        let subject = Subject::Binding {
            declaration: Ident::new("D"),
            name: Ident::new("x"),
        };
        let because = BTreeSet::new();

        let int_proj = regime.project_has_type(
            &subject,
            &Ty::Int(IntWidth::Int),
            ContextId::root(),
            &because,
        );
        let bool_proj = regime.project_has_type(&subject, &Ty::Bool, ContextId::root(), &because);
        assert_ne!(int_proj.witness_id, bool_proj.witness_id);
        assert_ne!(int_proj.proposition, bool_proj.proposition);
    }

    #[test]
    fn distinct_contexts_give_distinct_judgements_over_the_same_proposition() {
        let regime = StructuralRegime::new();
        let subject = Subject::Binding {
            declaration: Ident::new("D"),
            name: Ident::new("x"),
        };
        let ty = Ty::Int(IntWidth::Int);
        let because = BTreeSet::new();

        let root_proj = regime.project_has_type(&subject, &ty, ContextId::root(), &because);
        let child_ctx = ContextId::root().extend(b"assumption");
        let child_proj = regime.project_has_type(&subject, &ty, child_ctx, &because);

        // Same underlying Realizes proposition — only the context differs.
        assert_eq!(root_proj.proposition, child_proj.proposition);
        assert_ne!(root_proj.judgement.context, child_proj.judgement.context);
        assert_ne!(root_proj.judgement.id(), child_proj.judgement.id());
    }

    #[test]
    fn because_set_changes_the_evidence_and_therefore_the_judgement() {
        let regime = StructuralRegime::new();
        let subject = Subject::Binding {
            declaration: Ident::new("D"),
            name: Ident::new("x"),
        };
        let ty = Ty::Int(IntWidth::Int);

        let empty = BTreeSet::new();
        let mut with_premise = BTreeSet::new();
        with_premise.insert(FactId::derive(&Fact::RuleImpure {
            subject: Subject::Rule {
                declaration: Ident::new("R"),
            },
        }));

        let a = regime.project_has_type(&subject, &ty, ContextId::root(), &empty);
        let b = regime.project_has_type(&subject, &ty, ContextId::root(), &with_premise);

        // Same proposition (subject/ty/regime unchanged), different evidence
        // (the because-set is folded into evidence, not the proposition).
        assert_eq!(a.proposition, b.proposition);
        assert_ne!(a.judgement.evidence, b.judgement.evidence);
        assert_ne!(a.judgement.id(), b.judgement.id());
    }

    #[test]
    fn conflicts_project_to_unknown_never_refuted_or_derived() {
        // Build a source with a genuine conflict (a NonBool guard).
        use brix_ir::core::ExprKind;
        use brix_ir::pattern::{Clause, Pattern};
        let source = brix_ir::frontend::FrontendSource {
            functions: Vec::new(),
            rules: vec![brix_ir::core::Rule {
                name: Ident::new("R"),
                head: brix_ir::core::Head::Tuple {
                    relation: brix_ir::ident::QualIdent::from("Out"),
                    args: vec![],
                },
                body: Pattern::new(vec![Clause::When(
                    brix_ir::core::Expr::new(
                        Ty::Int(IntWidth::Int),
                        ExprKind::Lit(brix_ir::pattern::Lit::Int(1)),
                    )
                    .with_origin(brix_ir::core::ExprOrigin::source(
                        &Ident::new("R"),
                        brix_ir::core::SourceRange { start: 0, end: 1 },
                    )),
                )]),
                effects: brix_ir::effects::EffectRow::empty(),
            }],
            constraints: vec![],
            queries: vec![],
        };
        let resolver = TableResolver::new();
        let report = analyze(&source, &resolver);
        assert!(!report.is_consistent());

        let regime = StructuralRegime::new();
        let projected = regime.project_conflicts(&report, ContextId::root());
        assert!(!projected.is_empty());
        for p in &projected {
            assert_eq!(p.judgement.outcome, Outcome::Unknown);
            assert_eq!(p.judgement.context, ContextId::root());
        }
    }
}
