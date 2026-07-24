//! Focused unit tests for the tricky corners of AST→Core-IR lowering
//! (issue #6 acceptance list): type-directed enum-variant disambiguation,
//! punning, protocol relation synthesis, the v0 defer-line warnings, and
//! totality on `Error`/`Ellipsis` AST nodes. Each test parses a small,
//! self-contained `.brix` snippet (not the flagship) so the assertion is
//! about one mechanism at a time.

use brix_ast::{parse_file, Severity};
use brix_ir::core::{Head, Rule};
use brix_ir::ident::Ident as IrIdent;
use brix_ir::pattern::{Arg, Clause, Lit};
use brixc::lower_file;

fn lower(src: &str) -> brixc::Lowered {
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "fixture must parse cleanly:\n{}",
        parse_diags.render(src, "t")
    );
    lower_file(&file, &parse_diags)
}

fn entity_field<'a>(clauses: &'a [Clause], var: &str) -> &'a [brix_ir::pattern::RoleArg] {
    clauses
        .iter()
        .find_map(|c| match c {
            Clause::Entity { var: v, fields, .. } if v.as_str() == var => Some(fields.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no entity clause bound to `{var}`"))
}

// ---------------------------------------------------------------------
// Expression origin stability (issue #15 PR 1: stable expression origins).
// ---------------------------------------------------------------------

#[test]
fn lowered_expression_carries_stable_source_origin() {
    let src = r#"
package t @ 1.0.0
rel Input { x: Int } key(x)
rel Output { y: Int } key(y)
derive R: Output(y: y) from { Input(x); let y = x + 1 }
"#;
    let lowered = lower(src);
    let rule = &lowered.source.rules[0];
    let expression = rule
        .body
        .clauses
        .iter()
        .find_map(|clause| match clause {
            Clause::Let { expr, .. } => Some(expr),
            _ => None,
        })
        .expect("let expression");
    let start = src.find("x + 1").expect("fixture expression") as u32;
    assert_eq!(
        expression.origin.range,
        Some(brix_ir::core::SourceRange {
            start,
            end: start + "x + 1".len() as u32,
        })
    );
    assert_eq!(
        expression.origin.id,
        brix_ir::core::ExprId::derive(
            &IrIdent::new("R"),
            expression.origin.range.expect("source range"),
        )
    );
}

#[test]
fn repeated_lowering_of_identical_source_yields_byte_identical_origins() {
    // Determinism (App G / Appendix I.2 discipline): `ExprId` is a canonical
    // content digest, not a positional/monotonic counter, so lowering the
    // same source twice — including in two independent `ProgramResolver`
    // passes — must reproduce exactly the same origin ids and ranges. This
    // is the guarantee the `determinism` CI gate (run the suite twice,
    // require a byte-clean tree) exercises at the whole-workspace level;
    // this test pins it down at the unit that will key type facts and
    // provenance to source expressions.
    let src = r#"
package t @ 1.0.0
rel Input { x: Int } key(x)
rel Output { y: Int } key(y)
derive R: Output(y: y) from { Input(x); let y = x + 1 }
"#;

    fn let_expr_origin(lowered: &brixc::Lowered) -> brix_ir::core::ExprOrigin {
        let rule = &lowered.source.rules[0];
        rule.body
            .clauses
            .iter()
            .find_map(|clause| match clause {
                Clause::Let { expr, .. } => Some(expr.origin),
                _ => None,
            })
            .expect("let expression")
    }

    let first = let_expr_origin(&lower(src));
    let second = let_expr_origin(&lower(src));
    assert_eq!(
        first, second,
        "re-lowering identical source must produce byte-identical expression origins"
    );
    assert_eq!(first.id.digest(), second.id.digest());

    // Unrelated edits (adding a clause elsewhere in the body) must not
    // renumber the origin of an expression whose own declaration name and
    // source range are unchanged — the whole point of a content-addressed
    // `ExprId` over a positional counter.
    let src_with_prefix_clause = r#"
package t @ 1.0.0
rel Input { x: Int } key(x)
rel Output { y: Int } key(y)
derive R: Output(y: y) from { when true; Input(x); let y = x + 1 }
"#;
    let start = src_with_prefix_clause.find("x + 1").expect("fixture");
    let range = brix_ir::core::SourceRange {
        start: start as u32,
        end: (start + "x + 1".len()) as u32,
    };
    let recomputed = brix_ir::core::ExprOrigin::source(&IrIdent::new("R"), range);
    assert_eq!(
        recomputed.id,
        brix_ir::core::ExprId::derive(&IrIdent::new("R"), range),
        "ExprId::derive must be a pure function of (declaration, range)"
    );
}

// ---------------------------------------------------------------------
// Type-directed enum-variant disambiguation (mismatch B).
// ---------------------------------------------------------------------

const VARIANT_DISAMBIGUATION_SRC: &str = r#"
package t @ 1.0.0

entity Vehicle { key plate: String; class: VehicleClass }
entity Client  { key code: String; tier: Tier }
enum Tier         { Standard; Key }
enum VehicleClass { Compact; Standard; SUV }

rel Seen { v: Vehicle; c: Client } key(v, c)
derive R: Seen(v: veh, c: cli) from {
  veh: Vehicle { class: Standard }
  cli: Client { tier: Standard }
}
"#;

#[test]
fn variant_disambiguation_uses_the_roles_declared_enum_not_a_global_search() {
    let lowered = lower(VARIANT_DISAMBIGUATION_SRC);
    let rule = &lowered.source.rules[0];

    let veh_class = entity_field(&rule.body.clauses, "veh");
    assert_eq!(veh_class[0].role.as_str(), "class");
    assert_eq!(
        veh_class[0].arg,
        Arg::Lit(Lit::Enum {
            ty: "VehicleClass".into(),
            ordinal: 1, // VehicleClass { Compact=0, Standard=1, SUV=2 }
        }),
        "`class: Standard` on a Vehicle must resolve against VehicleClass, not Tier"
    );

    let cli_tier = entity_field(&rule.body.clauses, "cli");
    assert_eq!(cli_tier[0].role.as_str(), "tier");
    assert_eq!(
        cli_tier[0].arg,
        Arg::Lit(Lit::Enum {
            ty: "Tier".into(),
            ordinal: 0, // Tier { Standard=0, Key=1 }
        }),
        "`tier: Standard` on a Client must resolve against Tier, not VehicleClass"
    );

    // Same surface spelling ("Standard"), different enums, different
    // ordinals — this is exactly what disambiguates the flagship's
    // `v: Vehicle { class: Standard }` from `c: Client { tier: Key }`-style
    // roles without qualification.
    assert_ne!(veh_class[0].arg, cli_tier[0].arg);
}

#[test]
fn unqualified_variant_in_general_expr_position_is_unique_when_only_one_enum_has_it() {
    let src = r#"
package t @ 1.0.0
enum Status { Open; Delivered; Cancelled }
rel Seen { s: Status } key(s)
derive R: Seen(s: Open) from { when true }
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "{:#?}",
        lowered.diags
    );
    let rule = &lowered.source.rules[0];
    match &rule.head {
        Head::Tuple { args, .. } => {
            assert_eq!(
                args[0].arg,
                Arg::Lit(Lit::Enum {
                    ty: "Status".into(),
                    ordinal: 0
                })
            );
        }
        other => panic!("expected tuple head, got {other:?}"),
    }
}

#[test]
fn unqualified_variant_shared_by_two_enums_is_ambiguous() {
    let src = r#"
package t @ 1.0.0
enum Tier         { Standard; Key }
enum VehicleClass { Compact; Standard; SUV }
rel Seen { x: String } key(x)
derive R: Seen(x: "ok") from { when Standard == Standard }
"#;
    let lowered = lower(src);
    assert!(
        lowered
            .diags
            .iter()
            .any(|d| d.code == "BRX-LOW-0004" && d.severity == Severity::Error),
        "expected an ambiguous-variant diagnostic: {:#?}",
        lowered.diags
    );
}

// ---------------------------------------------------------------------
// Punning.
// ---------------------------------------------------------------------

#[test]
fn punned_entity_field_binds_a_var_named_after_the_role() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String; weight: Int }
rel Seen { o: Order } key(o)
derive R: Seen(o: ord) from {
  ord: Order { weight }
}
"#;
    let lowered = lower(src);
    let rule = &lowered.source.rules[0];
    let fields = entity_field(&rule.body.clauses, "ord");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].role.as_str(), "weight");
    assert_eq!(fields[0].arg, Arg::Var(IrIdent::new("weight")));
}

#[test]
fn punned_edge_arg_binds_a_var_named_after_the_role() {
    let src = r#"
package t @ 1.0.0
rel TariffRate { tariff: String; rate: Int } key(tariff)
rel Seen { r: Int } key(r)
derive R: Seen(r: rate) from {
  TariffRate(tariff: t, rate)
}
"#;
    let lowered = lower(src);
    let rule = &lowered.source.rules[0];
    let edge = rule
        .body
        .clauses
        .iter()
        .find_map(|c| match c {
            Clause::Edge { relation, args, .. } if relation.to_string() == "TariffRate" => {
                Some(args)
            }
            _ => None,
        })
        .unwrap();
    let rate_arg = edge.iter().find(|a| a.role.as_str() == "rate").unwrap();
    assert_eq!(rate_arg.arg, Arg::Var(IrIdent::new("rate")));
}

#[test]
fn bare_ident_call_argument_stays_positional_not_named() {
    // Regression: the parser represents *punning* (`role: role`) and a
    // plain bare-ident *positional* call argument identically at the
    // `ast::Arg` level (`name: Some(x), value: Ident(x)`) — the only
    // discriminator is span equality (see `expr::is_true_named`). A right
    // fold to "any `name.is_some()` means named" would make
    // `surcharge(w)` (param `weight`, argument var `w`) a bogus "unknown
    // named parameter `w`" error instead of a plain positional call.
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String; weight: Int }
fn surcharge(weight: Int) -> Int = weight
rel Seen { x: Int } key(x)
derive R: Seen(x: amount) from {
  o: Order { weight: w }
  let amount = surcharge(w)
}
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "bare-ident call arg `surcharge(w)` must not be treated as a named-arg mismatch: {:#?}",
        lowered.diags
    );
}

// ---------------------------------------------------------------------
// Type inference / ground dimensions (issue #13).
// ---------------------------------------------------------------------

fn type_errors(lowered: &brixc::Lowered) -> Vec<&brix_ast::Diagnostic> {
    lowered
        .diags
        .iter()
        .filter(|d| d.code == "BRX-IR-0005")
        .collect()
}

#[test]
fn wrong_role_type_is_one_hard_error_without_a_cascade() {
    let lowered = lower(
        r#"
package t @ 1.0.0
rel Input { value: Int } key(value)
rel Output { value: String } key(value)
derive R: Output(value: value) from { Input(value) }
"#,
    );
    let errors = type_errors(&lowered);
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("type mismatch"));
}

#[test]
fn pricing_rate_divided_by_length_is_one_dimension_error() {
    let lowered = lower(
        r#"
package t @ 1.0.0
use brix.math.units.Kilometre
rel Input { rate: Money<EUR> / Kilometre; length: Quantity<Kilometre>; surcharge: Money<EUR> } key(length)
rel Output { amount: Money<EUR> } key(amount)
derive R: Output(amount: amount) from {
  Input(rate, length, surcharge)
  let amount = rate / length + surcharge
}
"#,
    );
    let errors = type_errors(&lowered);
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("dimension error in add"));
}

#[test]
fn non_bool_when_guard_is_one_targeted_error() {
    let lowered = lower(
        r#"
package t @ 1.0.0
rel Input { x: Int } key(x)
rel Output { x: Int } key(x)
derive R: Output(x) from { Input(x); when 1 }
"#,
    );
    let errors = type_errors(&lowered);
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("when guard must be Bool"));
}

#[test]
fn call_arity_is_one_targeted_error() {
    let lowered = lower(
        r#"
package t @ 1.0.0
fn f(x: Int) -> Int = x
rel Input { x: Int } key(x)
rel Output { x: Int } key(x)
derive R: Output(x: y) from { Input(x); let y = f() }
"#,
    );
    let errors = type_errors(&lowered);
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("arity error"));
}

#[test]
fn money_times_money_is_rejected() {
    let lowered = lower(
        r#"
package t @ 1.0.0
rel Input { a: Money<EUR>; b: Money<EUR> } key(a)
rel Output { x: Int } key(x)
derive R: Output(x: 1) from { Input(a, b); let product = a * b }
"#,
    );
    let errors = type_errors(&lowered);
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("dimension error in mul"));
}

#[test]
fn dividing_distinct_currencies_is_rejected() {
    let lowered = lower(
        r#"
package t @ 1.0.0
rel Input { eur: Money<EUR>; usd: Money<USD> } key(eur)
rel Output { x: Int } key(x)
derive R: Output(x: 1) from { Input(eur, usd); let exchange = eur / usd }
"#,
    );
    let errors = type_errors(&lowered);
    assert_eq!(errors.len(), 1, "{:#?}", lowered.diags);
    assert!(errors[0].message.contains("dimension error in div"));
}
// ---------------------------------------------------------------------
// Protocol relation synthesis.
// ---------------------------------------------------------------------

#[test]
fn protocol_synthesizes_request_and_outcome_relations() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String }
entity Vehicle { key plate: String }
protocol AssignOrder {
  request { order: Order } key(order)
  outcome Chosen     { vehicle: Vehicle }
  outcome NoCapacity { }
}
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "{:#?}",
        lowered.diags
    );

    let request = lowered
        .resolver
        .relations()
        .find(|r| r.name.to_string() == "AssignOrder.request")
        .unwrap();
    assert_eq!(request.key, vec![IrIdent::new("order")]);
    assert!(request.model_closed);
    let role_names: Vec<&str> = request.roles.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(role_names, vec!["order"]);

    let chosen = lowered
        .resolver
        .relations()
        .find(|r| r.name.to_string() == "AssignOrder.Chosen")
        .unwrap();
    assert_eq!(chosen.key, vec![IrIdent::new("order")]);
    assert!(
        !chosen.model_closed,
        "outcome relations are not model-closed"
    );
    assert!(
        !chosen.derived,
        "outcomes are asserted externally, never derived"
    );
    let chosen_roles: Vec<&str> = chosen.roles.iter().map(|(n, _)| n.as_str()).collect();
    // request KEY roles ++ the outcome's own roles.
    assert_eq!(chosen_roles, vec!["order", "vehicle"]);

    let no_capacity = lowered
        .resolver
        .relations()
        .find(|r| r.name.to_string() == "AssignOrder.NoCapacity")
        .unwrap();
    let nc_roles: Vec<&str> = no_capacity.roles.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(nc_roles, vec!["order"]);
}

#[test]
fn a_derive_targeting_a_protocol_request_marks_it_derived() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String }
protocol AssignOrder {
  request { order: Order } key(order)
  outcome Chosen { }
}
rel Ready { order: Order } key(order)
derive Ask: AssignOrder.request(order: o) from { Ready(order: o) }
"#;
    let lowered = lower(src);
    let request = lowered
        .resolver
        .relations()
        .find(|r| r.name.to_string() == "AssignOrder.request")
        .unwrap();
    assert!(
        request.derived,
        "a derive targeting `AssignOrder.request` should flip its `derived` flag (sub-pass 1b)"
    );
}

// ---------------------------------------------------------------------
// v0 defer line.
// ---------------------------------------------------------------------

#[test]
fn a_driver_decl_is_skipped_with_exactly_one_warning_and_no_errors() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String }
protocol AssignOrder {
  request { order: Order } key(order)
  outcome Chosen { }
}
driver P for AssignOrder needs Net<"x"> {
  on request(req, cancel) {
    succeed Chosen { }
  }
}
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "{:#?}",
        lowered.diags
    );
    let skips: Vec<_> = lowered
        .diags
        .iter()
        .filter(|d| d.code == "BRX-LOW-0002")
        .collect();
    assert_eq!(skips.len(), 1);
}

#[test]
fn a_scenario_decl_is_skipped_with_exactly_one_warning_and_no_errors() {
    let src = r#"
package t @ 1.0.0
scenario S {
  seed 1
  assert always { true }
}
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "{:#?}",
        lowered.diags
    );
    let skips: Vec<_> = lowered
        .diags
        .iter()
        .filter(|d| d.code == "BRX-LOW-0002")
        .collect();
    assert_eq!(skips.len(), 1);
}

// ---------------------------------------------------------------------
// Trait/impl coherence (issue #111): the §28.3 orphan rule enforced at
// pass-1 registration. trait/impl are no longer BRX-LOW-0002 skips.
// ---------------------------------------------------------------------

#[test]
fn two_overlapping_impls_for_the_same_head_are_one_coherence_error() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String }
trait Canonical { type Item }
impl Canonical for Order { type Item = String }
impl Canonical for Order { type Item = String }
"#;
    let lowered = lower(src);
    let coherence: Vec<_> = lowered
        .diags
        .iter()
        .filter(|d| d.code == "BRX-LOW-0017")
        .collect();
    assert_eq!(coherence.len(), 1, "{:#?}", lowered.diags);
    assert_eq!(coherence[0].severity, Severity::Error);
}

#[test]
fn distinct_heads_and_a_lone_impl_are_coherent_no_error() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String }
entity Invoice { key ref: String }
trait Canonical { type Item }
impl Canonical for Order { type Item = String }
impl Canonical for Invoice { type Item = String }
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.code != "BRX-LOW-0017"),
        "distinct heads must not collide: {:#?}",
        lowered.diags
    );
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "{:#?}",
        lowered.diags
    );
}

#[test]
fn trait_and_impl_are_no_longer_brx_low_0002_skips() {
    let src = r#"
package t @ 1.0.0
entity Order { key ref: String }
trait Canonical { type Item }
impl Canonical for Order { type Item = String }
"#;
    let lowered = lower(src);
    assert!(
        lowered.diags.iter().all(|d| d.code != "BRX-LOW-0002"),
        "trait/impl are handled in pass 1, not deferred: {:#?}",
        lowered.diags
    );
    assert!(
        lowered.diags.iter().all(|d| d.severity != Severity::Error),
        "a coherent trait+impl lowers cleanly: {:#?}",
        lowered.diags
    );
}

// ---------------------------------------------------------------------
// Totality: `Error`/`Ellipsis` AST nodes never panic lowering.
// ---------------------------------------------------------------------

#[test]
fn garbage_at_top_level_recovers_and_lowers_without_panicking_or_new_errors() {
    // `%%%` is not the start of any known declaration. `brix-ast`'s
    // recovery is permissive enough that this lands as `Decl::Extension`
    // (itself on the v0 defer list) rather than the rarer `Decl::Error`
    // (reachable only when the parser makes truly zero progress, which
    // its `Extension` catch-all makes hard to hit from a well-formed
    // token stream) — either way, lowering must not panic, and must add
    // only the expected skip *warning*, never a new error.
    let src = "package t @ 1.0.0\n%%% garbage %%%\nentity Order { key ref: String }\n";
    let (file, parse_diags) = parse_file(src);
    assert!(
        parse_diags.has_errors(),
        "expected the parser to flag the garbage and recover"
    );
    let parse_error_count = parse_diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let lowered = lower_file(&file, &parse_diags);
    assert_eq!(
        lowered
            .diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        parse_error_count,
        "lowering must not add a new error on top of the parser's own: {:#?}",
        lowered.diags
    );
    // And the well-formed decl after the garbage still lowers.
    assert!(lowered
        .resolver
        .relations()
        .any(|r| r.name.to_string() == "Order"));
}

#[test]
fn ellipsis_in_expression_position_lowers_without_panicking() {
    let src = r#"
package t @ 1.0.0
rel Seen { x: Int } key(x)
derive R: Seen(x: n) from {
  let n = ...
}
"#;
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "`...` round-trips structurally: {}",
        parse_diags.render(src, "t")
    );
    let lowered = lower_file(&file, &parse_diags);
    assert!(
        lowered
            .diags
            .iter()
            .any(|d| d.code == "BRX-LOW-0010" && d.severity == Severity::Error),
        "expected the Ellipsis diagnostic: {:#?}",
        lowered.diags
    );
    // Still produced exactly one rule (best-effort/poisoned, not dropped).
    assert_eq!(lowered.source.rules.len(), 1);
}

#[test]
fn a_clause_error_node_is_skipped_without_panicking() {
    // An unparseable clause inside an otherwise-good body recovers as
    // `Clause::Error`; lowering must skip just that clause, not panic or
    // drop the whole rule.
    let src = "package t @ 1.0.0\nrel Seen { x: Int } key(x)\nderive R: Seen(x: n) from {\n  @@@ not a clause @@@\n  when true\n}\n";
    let (file, parse_diags) = parse_file(src);
    let lowered = lower_file(&file, &parse_diags);
    assert_eq!(
        lowered.source.rules.len(),
        1,
        "the rule must still lower despite one bad clause"
    );
    let rule: &Rule = &lowered.source.rules[0];
    assert!(
        !rule.body.clauses.is_empty(),
        "the good `when true` clause should still be present"
    );
}
