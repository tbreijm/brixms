//! `brixc::selfhost::native_typecheck` — Track A slice C.
//!
//! Drives the entry point from REAL `.brix` source through `lower_file` (so
//! `meta`/spans are populated, unlike the synthetic `FrontendSource`
//! fixtures `brix-conformance`'s parity harness builds by hand) and checks
//! it against `reflect::analyze` run over that same lowered source, so a
//! disagreement between the two would fail loudly rather than silently.

use brix_ast::parse_file;
use brix_ir::reflect::{analyze, ConflictKind};
use brixc::lower_file;
use brixc::selfhost::native_typecheck;

fn lower(src: &str) -> brixc::Lowered {
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "fixture must parse cleanly:\n{}",
        parse_diags.render(src, "t")
    );
    lower_file(&file, &parse_diags)
}

/// A role-literal type mismatch: `Input.label` is declared `String`, but the
/// rule body binds it to the integer literal `5`. Mirrors
/// `brix_conformance::typecorpus::NATIVE_ROLE_LIT_MISMATCH_FIXTURE` (the
/// parity harness's own fixture for this exact shape) — reproduced here
/// rather than imported since `brixc` does not (and must not) depend on
/// `brix-conformance`.
const ROLE_LIT_MISMATCH_SRC: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count: count, label: 5)
}
"#;

#[test]
fn role_literal_mismatch_yields_one_nat_mismatch_diagnostic_with_a_real_span() {
    let lowered = lower(ROLE_LIT_MISMATCH_SRC);

    // Cross-check: reflect's own report agrees there is exactly one
    // Mismatch conflict for this program (proving native and reflect agree,
    // not just that native fired on *something*).
    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_mismatches: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::Mismatch { .. }))
        .collect();
    assert_eq!(
        reflect_mismatches.len(),
        1,
        "reflect::analyze should report exactly one Mismatch conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    assert_eq!(
        diags.len(),
        1,
        "native_typecheck should report exactly one diagnostic: {:#?}",
        diags
    );
    let diag = &diags[0];
    assert_eq!(diag.code, "BRX-NAT-0001");
    assert_ne!(
        diag.span.start, diag.span.end,
        "the diagnostic span must be non-empty, proving span-mapping through \
         the expr origin worked: {:#?}",
        diag
    );
    assert!(
        diag.message.contains("type mismatch"),
        "unexpected message: {}",
        diag.message
    );
}

/// A clean, well-typed program (the same shape `lower_units.rs`'s stable-
/// origin fixture uses) — `native_typecheck` must report zero diagnostics,
/// proving no false positives.
const CLEAN_SRC: &str = r#"
package t @ 1.0.0

rel Input { x: Int } key(x)
rel Output { y: Int } key(y)

derive R: Output(y: y) from {
    Input(x);
    let y = x + 1
}
"#;

#[test]
fn well_typed_program_yields_zero_native_diagnostics() {
    let lowered = lower(CLEAN_SRC);
    assert!(
        !lowered.has_errors(),
        "fixture must lower cleanly: {:#?}",
        lowered.diags
    );

    let diags = native_typecheck(&lowered);
    assert!(
        diags.is_empty(),
        "expected zero native diagnostics on a clean program, got: {:#?}",
        diags
    );
}

/// An arity mismatch: `f` takes one `Int` parameter, but the call site
/// passes zero arguments. Same fixture shape as `lower_units.rs`'s
/// `call_arity_is_one_targeted_error`.
const ARITY_MISMATCH_SRC: &str = r#"
package t @ 1.0.0

fn f(x: Int) -> Int = x

rel Input { x: Int } key(x)
rel Output { x: Int } key(x)

derive R: Output(x: y) from {
    Input(x);
    let y = f()
}
"#;

#[test]
fn call_arity_mismatch_yields_a_nat_arity_diagnostic() {
    let lowered = lower(ARITY_MISMATCH_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_arities: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::Arity { .. }))
        .collect();
    assert_eq!(
        reflect_arities.len(),
        1,
        "reflect::analyze should report exactly one Arity conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let arity_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0004").collect();
    assert_eq!(
        arity_diags.len(),
        1,
        "expected exactly one BRX-NAT-0004 diagnostic: {:#?}",
        diags
    );
    assert!(
        arity_diags[0].message.contains("arity mismatch"),
        "unexpected message: {}",
        arity_diags[0].message
    );
}

/// A dimension mismatch: `rate / length + surcharge` adds `Money<EUR>/Kilometre²`
/// to `Money<EUR>` — two ground, unequal dimensions. Same shape as
/// `lower_units.rs`'s `pricing_rate_divided_by_length_is_one_dimension_error`
/// (and #15's `flagship_pricing_mutation` parity fixture). Covers the
/// `DimensionConflict` → `BRX-NAT-0008` path of `native_typecheck`.
const DIMENSION_MISMATCH_SRC: &str = r#"
package t @ 1.0.0
use brix.math.units.Kilometre
rel Input { rate: Money<EUR> / Kilometre; length: Quantity<Kilometre>; surcharge: Money<EUR> } key(length)
rel Output { amount: Money<EUR> } key(amount)
derive R: Output(amount: amount) from {
  Input(rate, length, surcharge)
  let amount = rate / length + surcharge
}
"#;

#[test]
fn dimension_mismatch_yields_a_nat_dimension_diagnostic() {
    let lowered = lower(DIMENSION_MISMATCH_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_dims: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::Dimension { .. }))
        .collect();
    assert_eq!(
        reflect_dims.len(),
        1,
        "reflect::analyze should report exactly one Dimension conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let dim_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0008").collect();
    assert_eq!(
        dim_diags.len(),
        1,
        "expected exactly one BRX-NAT-0008 diagnostic: {:#?}",
        diags
    );
    assert!(
        dim_diags[0].message.contains("dimension mismatch"),
        "unexpected message: {}",
        dim_diags[0].message
    );
}

/// An impure rule: `noisy`'s declared effect row carries `console`, and
/// rule `R`'s body calls it — Appendix E `pure(B, H)` is violated. Same
/// shape as `crates/brix-conformance/tests/fixtures/negative/check/
/// effect_violation.brix` (issue #45) and #15's `rule_impure_effect_row`
/// parity fixture. Note lowering ALSO raises its own `BRX-IR-0006` for this
/// program (`check_rule` runs during lowering too) — `native_typecheck`
/// still runs over `lowered.source`/`lowered.resolver` regardless, since
/// `lower()` here only asserts clean *parsing*, not clean lowering. Covers
/// the `ImpureRuleConflict` -> `BRX-NAT-0009` path.
const RULE_IMPURE_SRC: &str = r#"
package t @ 1.0.0

fn noisy(x: Int) -> Int ! { console } = x

rel Input { value: Int } key(value)
rel Output { value: Int } key(value)

derive R: Output(value: y) from {
  Input(value: v)
  let y = noisy(v)
}
"#;

#[test]
fn impure_rule_yields_a_nat_rule_impure_diagnostic() {
    let lowered = lower(RULE_IMPURE_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_impure: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::ImpureRule))
        .collect();
    assert_eq!(
        reflect_impure.len(),
        1,
        "reflect::analyze should report exactly one ImpureRule conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let impure_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0009").collect();
    assert_eq!(
        impure_diags.len(),
        1,
        "expected exactly one BRX-NAT-0009 diagnostic: {:#?}",
        diags
    );
    assert!(
        impure_diags[0].message.contains("impure rule"),
        "unexpected message: {}",
        impure_diags[0].message
    );
}

/// An unbound head key: `Mint`'s `keyed by (missing)` names an ident never
/// bound in the body — Appendix E `keys(H) ⊆ Bindings` is violated. Same
/// shape as #15's `rule_unbound_head_key` parity fixture, translated to
/// real `.brix` surface syntax (`entity ... keyed by (...)` — grammar at
/// `crates/brix-ast/src/parser.rs`'s `head_decl`). Covers the
/// `UnboundHeadKeyConflict` -> `BRX-NAT-0010` path.
const UNBOUND_HEAD_KEY_SRC: &str = r#"
package t @ 1.0.0

entity Widget { key id: String }

rel Input { value: Int } key(value)

derive Mint: n: Widget keyed by (missing) from {
  Input(value: v)
}
"#;

#[test]
fn unbound_head_key_yields_a_nat_unbound_head_key_diagnostic() {
    let lowered = lower(UNBOUND_HEAD_KEY_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_unbound: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::UnboundHeadKey { .. }))
        .collect();
    assert_eq!(
        reflect_unbound.len(),
        1,
        "reflect::analyze should report exactly one UnboundHeadKey conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let unbound_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0010").collect();
    assert_eq!(
        unbound_diags.len(),
        1,
        "expected exactly one BRX-NAT-0010 diagnostic: {:#?}",
        diags
    );
    assert!(
        unbound_diags[0]
            .message
            .contains("unbound head key `missing`"),
        "unexpected message: {}",
        unbound_diags[0].message
    );
}

/// A mask head whose `target`/`reason` are both plain reads, not `@`
/// edge-bound aliases — Appendix E's mask-head side condition is violated
/// for BOTH idents. Same shape as `crates/brix-ast/tests/fixtures/spec/
/// 0002-6-the-mask-primitive.brix`'s WELL-formed mask (which uses
/// `price @ ComputedPrice(...)` edge aliases), with the `@` bindings
/// deliberately dropped so neither `price` nor `manual` is edge-bound.
/// Covers the `MaskRefNotEdgeBoundConflict` -> `BRX-NAT-0011` path.
const MASK_REF_SRC: &str = r#"
package t @ 1.0.0

rel ComputedPrice { order: Int; amount: Int } key(order)
rel ManualPrice { order: Int; amount: Int } key(order)

derive Override: mask(price) by manual from {
  ComputedPrice(order: o, amount: a1)
  ManualPrice(order: o, amount: a2)
}
"#;

#[test]
fn mask_ref_not_edge_bound_yields_two_nat_mask_ref_diagnostics() {
    let lowered = lower(MASK_REF_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_mask_refs: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::MaskRefNotEdgeBound { .. }))
        .collect();
    assert_eq!(
        reflect_mask_refs.len(),
        2,
        "reflect::analyze should report exactly two MaskRefNotEdgeBound conflicts \
         (target `price` and reason `manual`): {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let mask_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0011").collect();
    assert_eq!(
        mask_diags.len(),
        2,
        "expected exactly two BRX-NAT-0011 diagnostics: {:#?}",
        diags
    );
    assert!(
        mask_diags
            .iter()
            .all(|d| d.message.contains("not edge-bound")),
        "unexpected messages: {mask_diags:#?}"
    );
}

/// An ordinary (non-`aggregate`) fn call consuming a `Comprehension` over
/// `ComputedPrice`, a relation that is `derived: true` because it is itself
/// a `derive` head in this same file (`derived` is never a `.brix` keyword —
/// `recompute_derived` infers it) — Appendix E `Ordinary fn` is violated.
/// Same shape as #15's `rule_ordinary_fn_on_derived_rel` parity fixture.
/// `sumUp`'s declared param row doesn't match the comprehension's actual
/// (empty) row, so this fixture ALSO raises an incidental `BRX-NAT-0001`
/// Mismatch — irrelevant to what this test covers, so the assertions filter
/// specifically for `BRX-NAT-0012` rather than requiring a clean diagnostic
/// set (mirrors `call_arity_mismatch_yields_a_nat_arity_diagnostic`'s
/// filter-by-code pattern). Covers the `OrdinaryFnOnDerivedRelConflict` ->
/// `BRX-NAT-0012` path.
const ORDINARY_FN_ON_DERIVED_REL_SRC: &str = r#"
package t @ 1.0.0

rel Order { id: Int } key(id)
rel ComputedPrice { order: Int } key(order)
rel PriceSummary { order: Int; total: Int } key(order)

fn sumUp(r: Rel<{order: Int}>) -> Int = 0

derive PriceOrder: ComputedPrice(order: o) from { Order(id: o) }
derive Summary: PriceSummary(order: o, total: t) from {
  Order(id: o)
  let t = sumUp(from { ComputedPrice(order: o) })
}
"#;

#[test]
fn ordinary_fn_on_derived_rel_yields_a_nat_ordinary_fn_diagnostic() {
    let lowered = lower(ORDINARY_FN_ON_DERIVED_REL_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_ordinary: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::OrdinaryFnOnDerivedRel { .. }))
        .collect();
    assert_eq!(
        reflect_ordinary.len(),
        1,
        "reflect::analyze should report exactly one OrdinaryFnOnDerivedRel conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let ordinary_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0012").collect();
    assert_eq!(
        ordinary_diags.len(),
        1,
        "expected exactly one BRX-NAT-0012 diagnostic: {:#?}",
        diags
    );
    assert!(
        ordinary_diags[0]
            .message
            .contains("ordinary fn on derived relation `ComputedPrice`"),
        "unexpected message: {}",
        ordinary_diags[0].message
    );
}
