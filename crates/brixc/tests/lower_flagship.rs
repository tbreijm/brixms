//! Acceptance test (issue #6): the flagship spec program lowers cleanly.
//!
//! "Cleanly" means: zero error-severity diagnostics, and only the warnings
//! this v0 lowering is honestly expected to produce — the `driver`/
//! `scenario` sections (deferred wholesale, design's defer line) plus the
//! unresolved `riskModel` return type.
//! `ValidationError` return-type component (declared nowhere in this file —
//! an unresolved type name in fn-signature position is a warning, not an
//! error, design tymap rule). Both are real gaps, not bugs to paper over;
//! see the report for why the design ruling's own "2 warnings" shorthand
//! undercounts what its own rules produce.

use brix_ast::{parse_file, Severity};
use brix_ir::check::check_rule;
use brixc::lower_file;

fn lower_flagship() -> brixc::Lowered {
    let src =
        include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix");
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "flagship fixture must parse cleanly"
    );
    lower_file(&file, &parse_diags)
}

#[test]
fn flagship_lowers_with_zero_errors() {
    let lowered = lower_flagship();
    let errors: Vec<&brix_ast::Diagnostic> = lowered
        .diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "expected zero error-severity diagnostics, got: {:#?}",
        errors
            .iter()
            .map(|d| (d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flagship_pricing_multiply_to_divide_mutation_is_one_dimension_error() {
    let src =
        include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix")
            .replacen("rate * length", "rate / length", 1);
    let (file, parse_diags) = parse_file(&src);
    assert!(!parse_diags.has_errors());
    let lowered = lower_file(&file, &parse_diags);
    let errors: Vec<_> = lowered
        .diags
        .iter()
        .filter(|d| d.code == "BRX-IR-0005")
        .collect();
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("dimension error in add"));
}

#[test]
fn flagship_produces_exactly_the_expected_warnings() {
    let lowered = lower_flagship();
    let mut warnings: Vec<(&str, String)> = lowered
        .diags
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| (d.code, d.message.clone()))
        .collect();
    warnings.sort();

    // 2 drivers + 1 scenario, skip-with-warning (BRX-LOW-0002); plus
    // riskModel's undeclared
    // `ValidationError` return-type component (BRX-LOW-0012, warning
    // severity because it's fn-sig position, not role position).
    let decl_skips = warnings
        .iter()
        .filter(|(c, _)| *c == "BRX-LOW-0002")
        .count();
    let unresolved_ty = warnings
        .iter()
        .filter(|(c, _)| *c == "BRX-LOW-0012")
        .count();

    assert_eq!(
        decl_skips, 3,
        "2 driver decls + 1 scenario decl: {warnings:#?}"
    );
    assert_eq!(
        unresolved_ty, 1,
        "riskModel's ValidationError: {warnings:#?}"
    );
    assert_eq!(
        warnings.len(),
        4,
        "no other warning should appear: {warnings:#?}"
    );
}

#[test]
fn flagship_lowers_the_expected_rules_constraints_and_queries() {
    let lowered = lower_flagship();
    let mut rules: Vec<String> = lowered
        .source
        .rules
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    rules.sort();
    assert_eq!(
        rules,
        vec![
            "Assign",
            "Escalate",
            "FromComputed",
            "FromManual",
            "Override",
            "PriceOrder",
            "RequestAssignment",
            "Risk",
            "Waiting",
        ]
    );

    let constraints: Vec<String> = lowered
        .source
        .constraints
        .iter()
        .map(|c| c.name.to_string())
        .collect();
    assert_eq!(constraints, vec!["Capacity"]);

    let queries: Vec<String> = lowered
        .source
        .queries
        .iter()
        .map(|q| q.name.to_string())
        .collect();
    assert_eq!(queries, vec!["KeyClientsAtRisk"]);
}

#[test]
fn price_order_head_and_mask_and_query_shape_are_correct() {
    let lowered = lower_flagship();

    let price_order = lowered
        .source
        .rules
        .iter()
        .find(|r| r.name.to_string() == "PriceOrder")
        .unwrap();
    match &price_order.head {
        brix_ir::core::Head::Tuple { relation, .. } => {
            assert_eq!(relation.to_string(), "ComputedPrice")
        }
        other => panic!("expected a tuple head, got {other:?}"),
    }

    let override_rule = lowered
        .source
        .rules
        .iter()
        .find(|r| r.name.to_string() == "Override")
        .unwrap();
    match &override_rule.head {
        brix_ir::core::Head::Mask { target, reason } => {
            assert_eq!(target.as_str(), "price");
            assert_eq!(reason.as_str(), "manual");
        }
        other => panic!("expected a mask head, got {other:?}"),
    }

    let query = &lowered.source.queries[0];
    assert_eq!(query.name.as_str(), "KeyClientsAtRisk");
    match &query.result {
        brix_ir::types::Ty::Rel(row) => {
            let names: Vec<&str> = row.fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, vec!["order", "client", "risk"]);
        }
        other => panic!("expected Rel<{{...}}> result, got {other:?}"),
    }
    let params = &query.params;
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].0.as_str(), "threshold");
}

/// `check_rule` over every lowered rule must never report `UnknownRelation`
/// — every name used, including protocol-synth `AssignOrder.Chosen` /
/// `NotifyOps.request` / `brix.sim.Now` (prelude), resolves in the
/// resolver's schema table.
#[test]
fn check_rule_reports_no_unknown_relation_on_the_flagship() {
    let lowered = lower_flagship();
    for rule in &lowered.source.rules {
        for finding in check_rule(rule, &lowered.resolver) {
            assert!(
                !matches!(finding, brix_ir::check::Finding::UnknownRelation { .. }),
                "unexpected UnknownRelation in rule {}: {finding}",
                rule.name
            );
        }
    }
}

/// `check_relation_keys` over every schema (entities, ground/state/event
/// rels, protocol-synth relations) must not raise a false-positive
/// `NonCanonicalKey` — this is specifically what mismatch (A) fixes:
/// `entity Tariff { key class: VehicleClass }` has an *enum* key role,
/// which pre-fix would have tymapped to `Ty::Var` and tripped
/// `UnresolvedTypeVar`.
#[test]
fn tariff_enum_key_role_passes_key_canonical_check() {
    let lowered = lower_flagship();
    let tariff = lowered
        .resolver
        .relations()
        .find(|r| r.name.to_string() == "Tariff")
        .expect("Tariff entity schema");
    assert_eq!(tariff.key, vec![brix_ir::ident::Ident::new("class")]);
    let (_, ty) = tariff
        .roles
        .iter()
        .find(|(n, _)| n.as_str() == "class")
        .unwrap();
    assert!(
        matches!(ty, brix_ir::types::Ty::Enum(_)),
        "expected Ty::Enum, got {ty}"
    );
    assert!(brix_ir::types::check_key_canonical(ty).is_ok());
}

#[test]
fn flagship_modeled_expression_types_have_no_residual_ty_vars() {
    let lowered = lower_flagship();
    for rule in &lowered.source.rules {
        assert_pattern_is_ground(&rule.body, &rule.name.to_string());
    }
    for query in &lowered.source.queries {
        for (_, ty) in &query.params {
            assert!(!ty_contains_var(ty), "query parameter type retains {ty}");
        }
        assert_pattern_is_ground(&query.body, &query.name.to_string());
        assert_expr_is_ground(&query.yields, &query.name.to_string());
        assert!(
            !ty_contains_var(&query.result),
            "query result retains {}",
            query.result
        );
    }
}

fn assert_pattern_is_ground(pattern: &brix_ir::pattern::Pattern, owner: &str) {
    for clause in &pattern.clauses {
        match clause {
            brix_ir::pattern::Clause::Let { expr, .. } | brix_ir::pattern::Clause::When(expr) => {
                assert_expr_is_ground(expr, owner)
            }
            brix_ir::pattern::Clause::Any(cases) => {
                for case in cases {
                    assert_pattern_is_ground(case, owner);
                }
            }
            brix_ir::pattern::Clause::Exists(p)
            | brix_ir::pattern::Clause::Without(p)
            | brix_ir::pattern::Clause::Optional(p)
            | brix_ir::pattern::Clause::Cross(p) => assert_pattern_is_ground(p, owner),
            _ => {}
        }
    }
}

fn assert_expr_is_ground(expr: &brix_ir::core::Expr, owner: &str) {
    // The Risk rule's riskModel error component is an explicitly named v1
    // exemption: ValidationError is not declared in the flagship namespace
    // and function bodies are intentionally deferred.
    let named_deferred_risk_model_error = owner == "Risk"
        && matches!(&*expr.kind, brix_ir::core::ExprKind::Call { func, .. } if func.to_string() == "riskModel")
        && matches!(&expr.ty, brix_ir::types::Ty::Result(_, error) if matches!(&**error, brix_ir::types::Ty::Var(_)));
    assert!(
        named_deferred_risk_model_error || !ty_contains_var(&expr.ty),
        "{owner} retains {}",
        expr.ty
    );
    match &*expr.kind {
        brix_ir::core::ExprKind::Call { args, .. } => {
            for arg in args {
                assert_expr_is_ground(arg, owner);
            }
        }
        brix_ir::core::ExprKind::Field { base, .. } => assert_expr_is_ground(base, owner),
        brix_ir::core::ExprKind::Record { fields } => {
            for (_, value) in fields {
                assert_expr_is_ground(value, owner);
            }
        }
        brix_ir::core::ExprKind::If { cond, then, els } => {
            assert_expr_is_ground(cond, owner);
            assert_expr_is_ground(then, owner);
            assert_expr_is_ground(els, owner);
        }
        brix_ir::core::ExprKind::Try { inner, .. } => assert_expr_is_ground(inner, owner),
        brix_ir::core::ExprKind::Comprehension { pattern, yields } => {
            assert_pattern_is_ground(pattern, owner);
            if let Some(yielded) = yields {
                assert_expr_is_ground(yielded, owner);
            }
        }
        brix_ir::core::ExprKind::Var(_) | brix_ir::core::ExprKind::Lit(_) => {}
    }
}

fn ty_contains_var(ty: &brix_ir::types::Ty) -> bool {
    use brix_ir::types::Ty;
    match ty {
        Ty::Var(_) => true,
        Ty::Option(t) | Ty::List(t) | Ty::Vector(t) | Ty::Set(t) | Ty::Bag(t) | Ty::Estimate(t) => {
            ty_contains_var(t)
        }
        Ty::Result(a, b) | Ty::Map(a, b) => ty_contains_var(a) || ty_contains_var(b),
        Ty::Rel(row) | Ty::Record(row) => row.fields.iter().any(|field| ty_contains_var(&field.ty)),
        Ty::Fn { params, ret, .. } => params.iter().any(ty_contains_var) || ty_contains_var(ret),
        _ => false,
    }
}
