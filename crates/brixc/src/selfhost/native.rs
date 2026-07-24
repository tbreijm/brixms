//! The `native_typecheck` entry point (#15 Track A slice C).
//!
//! Runs the self-hosted `brix.type` checker — the same `packages/brix.type/
//! src/world.brix` package the parity harness (`selfhost_parity.rs`)
//! shadow-checks against — over an already-lowered program, and turns its
//! derived `*Conflict` extents into ordinary compiler [`Diagnostic`]s. This
//! is the first slice that calls the native checker from the real compiler
//! pipeline rather than only from a test harness; wiring it into the CLI
//! itself is a later slice (Track A slice D).
//!
//! Pipeline: [`typefacts::export`] flattens a [`reflect::analyze`] report
//! into Ground `Assert` ops for the package; those ops are applied against
//! an empty [`GroundState`] and settled; the settled derived extents are
//! decoded back through the token table (`typefacts::resolve_*`) and mapped
//! to a [`Diagnostic`] with a real source span (via `origin`/[`LowerMeta`])
//! and a `BRX-NAT-*` code, distinct from `infer`'s own `BRX-IR-*` codes so a
//! native finding is never mistaken for one `infer_source` already reported.

use std::sync::OnceLock;

use brix_ast::parse_file;
use brix_diag::{Diagnostic, Span};
use brix_ir::reflect::{self, Subject};
use brix_rt::engine::{apply_transaction, settle, GroundState, Program, Transaction};

use crate::lower::{lower_file, LowerMeta, Lowered};
use crate::pipeline::PhaseAssign;
use crate::{emit, AstPhase};

use super::typefacts;

/// `BRX-NAT-0001` — native counterpart of `BRX-IR-0005`'s `Mismatch` case:
/// the `brix.type` package's `MismatchConflict` extent (fed by ordinary
/// role-literal/role-var mismatches AND the slice-8 gap-closure's
/// cross-ctor/same-epistemic-ctor rules).
pub const NAT_MISMATCH: &str = "BRX-NAT-0001";
/// `BRX-NAT-0002` — native counterpart of `TypeError::NonBoolGuard`:
/// `NonBoolConflict`.
pub const NAT_NON_BOOL: &str = "BRX-NAT-0002";
/// `BRX-NAT-0003` — native counterpart of `TypeError::UnknownField`:
/// `UnknownFieldConflict`.
pub const NAT_UNKNOWN_FIELD: &str = "BRX-NAT-0003";
/// `BRX-NAT-0004` — native counterpart of `TypeError::Arity`: `ArityConflict`.
pub const NAT_ARITY: &str = "BRX-NAT-0004";
/// `BRX-NAT-0005` — native counterpart of `TypeError::Occurs`:
/// `OccursConflict`.
pub const NAT_OCCURS: &str = "BRX-NAT-0005";
/// `BRX-NAT-0006` — native counterpart of `TypeError::EpistemicErasure`:
/// `EpistemicErasureConflict`.
pub const NAT_EPISTEMIC_ERASURE: &str = "BRX-NAT-0006";
/// `BRX-NAT-0007` — native counterpart of `TypeError::TryNonResult`:
/// `TryNonResultConflict`.
pub const NAT_TRY_NON_RESULT: &str = "BRX-NAT-0007";
/// `BRX-NAT-0008` — native counterpart of `ConflictKind::Dimension`'s
/// add/sub same-dimension case: `DimensionConflict` (mul/div deferred).
pub const NAT_DIMENSION: &str = "BRX-NAT-0008";
/// `BRX-NAT-0009` — native counterpart of `ConflictKind::ImpureRule`
/// (Appendix E `pure(B, H)`): `ImpureRuleConflict`. Restatement, not
/// re-analysis — see `typefacts.rs`'s `Fact::RuleImpure` export doc.
pub const NAT_RULE_IMPURE: &str = "BRX-NAT-0009";
/// `BRX-NAT-0010` — native counterpart of `ConflictKind::UnboundHeadKey`
/// (Appendix E `keys(H) ⊆ Bindings`): `UnboundHeadKeyConflict`.
pub const NAT_UNBOUND_HEAD_KEY: &str = "BRX-NAT-0010";
/// `BRX-NAT-0011` — native counterpart of `ConflictKind::MaskRefNotEdgeBound`
/// (Appendix E mask-head side condition): `MaskRefNotEdgeBoundConflict`.
pub const NAT_MASK_REF: &str = "BRX-NAT-0011";
/// `BRX-NAT-0012` — native counterpart of `ConflictKind::OrdinaryFnOnDerivedRel`
/// (Appendix E `Ordinary fn`): `OrdinaryFnOnDerivedRelConflict`.
pub const NAT_ORDINARY_FN: &str = "BRX-NAT-0012";

/// The compiled `packages/brix.type/src/world.brix` package, compiled once
/// per process. The package is known-good (Track A slice B proved it
/// compiles and checks cleanly, and `selfhost_parity`'s `compiled_package()`
/// exercises the identical path on every test run), so a failure here is a
/// compiler bug, not user input — `.expect(...)` throughout is deliberate,
/// mirroring `compiled_package()`'s own asserts exactly.
fn brix_type_program() -> &'static Program {
    static PROGRAM: OnceLock<Program> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        const SRC: &str = include_str!("../../../../packages/brix.type/src/world.brix");
        let (file, parse_diags) = parse_file(SRC);
        assert!(
            !parse_diags.has_errors(),
            "brix.type package must parse cleanly: {:#?}",
            parse_diags.iter().collect::<Vec<_>>()
        );
        let lowered = lower_file(&file, &parse_diags);
        assert!(
            !lowered.has_errors(),
            "brix.type package must lower and type-check cleanly: {:#?}",
            lowered.diags
        );
        let phased = AstPhase
            .assign_phases(lowered)
            .expect("brix.type package must be well-stratified (Appendix F)");
        emit::project_program(&phased)
    })
}

/// A conflict's [`Subject`] resolved to a real source [`Span`], via
/// `origin`'s byte range for an expression subject, or `meta`'s decl-span
/// table for a binding/head/rule subject (mirrors `lower::diag::decl_span`,
/// which is not reachable here — that module is private to `lower`).
fn subject_span(subject: &Subject, meta: &LowerMeta) -> Span {
    match subject {
        Subject::Expr { origin } => origin
            .range
            .map(|r| Span::new(r.start, r.end))
            .unwrap_or(Span::empty(0)),
        Subject::Binding { declaration, .. }
        | Subject::Head { declaration, .. }
        | Subject::Rule { declaration } => meta.decl_span(declaration).unwrap_or(Span::empty(0)),
    }
}

/// Run the self-hosted `brix.type` checker over `lowered` and map its
/// derived conflicts to compiler diagnostics. Iterates the 12 `*Conflict`
/// extents in a fixed order, and rows within an extent in the extent's own
/// (content-addressed, already deterministic) `BTreeMap` order, so the
/// returned `Vec`'s order is stable across runs for the same input.
pub fn native_typecheck(lowered: &Lowered) -> Vec<Diagnostic> {
    let program = brix_type_program();
    let report = reflect::analyze(&lowered.source, &lowered.resolver);
    let export = typefacts::export(&report);

    let mut txn = Transaction::new(b"brixc-native-typecheck".to_vec());
    txn.ops = export.ops;
    let Ok(ground) = apply_transaction(program, &GroundState::default(), &txn) else {
        // The exporter only ever emits well-formed Ground asserts (they are
        // literal `TransactionOp::Assert`s over the package's own declared
        // relations) — a failure here would be a compiler bug, not user
        // input. Non-fatal: don't take down the whole compile over it.
        return Vec::new();
    };
    let settled = settle(program, &ground, 1);
    let tokens = &export.tokens;
    let meta = &lowered.meta;

    let mut diags = Vec::new();

    if let Some(extent) = settled.extents.get("MismatchConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_mismatch(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_MISMATCH,
                    subject_span(&r.subject, meta),
                    format!(
                        "type mismatch: expected `{}`, found `{}`",
                        r.expect, r.found
                    ),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("NonBoolConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_non_bool(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_NON_BOOL,
                    subject_span(&r.subject, meta),
                    format!("guard is not `Bool`: found `{}`", r.found),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("UnknownFieldConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_unknown_field(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_UNKNOWN_FIELD,
                    subject_span(&r.subject, meta),
                    format!("unknown field `{}`", r.field),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("ArityConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_arity(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_ARITY,
                    subject_span(&r.subject, meta),
                    format!(
                        "arity mismatch: expected `{}` argument(s), found `{}`",
                        r.expected, r.found
                    ),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("OccursConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_occurs(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_OCCURS,
                    subject_span(&r.subject, meta),
                    format!("occurs check: `{}` occurs in `{}`", r.var, r.into),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("EpistemicErasureConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_epistemic_erasure(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_EPISTEMIC_ERASURE,
                    subject_span(&r.subject, meta),
                    format!("epistemic erasure: `{}` to `{}`", r.from, r.to),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("TryNonResultConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_try_non_result(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_TRY_NON_RESULT,
                    subject_span(&r.subject, meta),
                    format!("`?` on non-`Result` value: `{}`", r.found),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("DimensionConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_dimension(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_DIMENSION,
                    subject_span(&r.subject, meta),
                    format!(
                        "dimension mismatch on `{}`: `{}` vs `{}`",
                        r.op, r.left, r.right
                    ),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("ImpureRuleConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_rule_impure(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_RULE_IMPURE,
                    subject_span(&r.subject, meta),
                    "impure rule".to_string(),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("UnboundHeadKeyConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_unbound_head_key(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_UNBOUND_HEAD_KEY,
                    subject_span(&r.subject, meta),
                    format!("unbound head key `{}`", r.key),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("MaskRefNotEdgeBoundConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_mask_ref(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_MASK_REF,
                    subject_span(&r.subject, meta),
                    format!("mask ref `{}` not edge-bound", r.var),
                ));
            }
        }
    }
    if let Some(extent) = settled.extents.get("OrdinaryFnOnDerivedRelConflict") {
        for record in extent.values() {
            if let Some(r) = typefacts::resolve_ordinary_fn(tokens, &record.row) {
                diags.push(Diagnostic::error(
                    NAT_ORDINARY_FN,
                    subject_span(&r.subject, meta),
                    format!("ordinary fn on derived relation `{}`", r.relation),
                ));
            }
        }
    }

    diags
}
