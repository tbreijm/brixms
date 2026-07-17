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
