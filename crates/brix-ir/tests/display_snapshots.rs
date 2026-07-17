//! Golden `Display` snapshots over hand-built IR values (insta). The IR
//! `Display` is a debugging deliverable (Ring0 build plan); these snapshots
//! make its drift reviewable at a glance, the same way canon vectors do for
//! encoding.

use brix_ir::core::{Constraint, Expr, ExprKind, Head, Query, Rule, Severity};
use brix_ir::effects::{Effect, EffectRow, EffectVar, Scope};
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::pattern::{edge, Arg, Clause, Lit, Pattern, RoleArg};
use brix_ir::site::SiteAssigner;
use brix_ir::types::{IntWidth, Row, RowField, Ty};

fn rolearg(role: &str, var: &str) -> RoleArg {
    RoleArg {
        role: Ident::new(role),
        arg: Arg::Var(Ident::new(var)),
    }
}

/// The flagship's masking rule (Part III §6), hand-built as IR.
#[test]
fn mask_rule_display() {
    let rule = Rule {
        name: Ident::new("Override"),
        head: Head::Mask {
            target: Ident::new("price"),
            reason: Ident::new("manual"),
        },
        body: Pattern::new(vec![
            Clause::Edge {
                bind: Some(Ident::new("price")),
                relation: QualIdent::from("ComputedPrice"),
                args: vec![rolearg("order", "o")],
            },
            Clause::Edge {
                bind: Some(Ident::new("manual")),
                relation: QualIdent::from("ManualPrice"),
                args: vec![rolearg("order", "o")],
            },
        ]),
        effects: EffectRow::empty(),
    };
    insta::assert_snapshot!(rule.to_string(), @"derive Override: mask(price) by manual from { price @ ComputedPrice(order: o); manual @ ManualPrice(order: o) } !{}");
}

/// A rule with a `keyed by` node head and an effectful body row, exercising the
/// effect-row Display in a rule.
#[test]
fn keyed_node_head_with_effects_display() {
    let rule = Rule {
        name: Ident::new("DeriveHub"),
        head: Head::Node {
            var: Ident::new("h"),
            entity: Ident::new("Hub"),
            args: vec![rolearg("region", "r")],
            keyed_by: vec![Ident::new("r")],
        },
        body: Pattern::new(vec![edge("Depot", &[("region", "r")])]),
        effects: EffectRow::from_atoms([Effect::GraphRead(Scope(Ident::new("S")))]),
    };
    insta::assert_snapshot!(rule.to_string(), @"derive DeriveHub: h: Hub { region: r } keyed by (r) from { Depot(region: r) } !{graph.read<S>}");
}

/// A typed expression tree with a `?` failure site and a field projection.
#[test]
fn expr_tree_with_try_site_display() {
    let mut sites = SiteAssigner::new(Ident::new("Pricing"));
    let due = Expr::new(
        Ty::Instant,
        ExprKind::Field {
            base: Expr::new(
                Ty::NodeRef(Ident::new("Order")),
                ExprKind::Var(Ident::new("o")),
            ),
            field: Ident::new("due"),
        },
    );
    let parsed = Expr::new(
        Ty::option(Ty::Int(IntWidth::I64)),
        ExprKind::Call {
            func: QualIdent::from("parse"),
            args: vec![Expr::new(Ty::Str, ExprKind::Lit(Lit::Str("42".into())))],
        },
    );
    let site = sites.next_site();
    let unwrapped = Expr::new(
        Ty::Int(IntWidth::I64),
        ExprKind::Try {
            inner: parsed,
            site,
        },
    );
    let cond = Expr::new(Ty::Bool, ExprKind::Var(Ident::new("flag")));
    let e = Expr::new(
        Ty::Instant,
        ExprKind::If {
            cond,
            then: due,
            els: Expr::new(
                Ty::Instant,
                ExprKind::Call {
                    func: QualIdent::from("epoch"),
                    args: vec![unwrapped],
                },
            ),
        },
    );
    // The `?` site hash is stable, so the snapshot pins the whole tree including
    // the site id — a regression in SiteId derivation would show here.
    insta::assert_snapshot!(e.to_string());
}

/// Effect-row Display forms, including an open polymorphic tail.
#[test]
fn effect_row_forms_display() {
    let closed = EffectRow::from_atoms([
        Effect::Net(Scope(Ident::new("Api"))),
        Effect::Console,
        Effect::Clock,
    ]);
    let open = EffectRow::from_atoms([Effect::Random]).with_tail(EffectVar(7));
    insta::assert_snapshot!(format!("{closed}\n{open}"), @r"
    !{net<Api>, clock, console}
    !{random | ?e7}
    ");
}

/// A row / record type Display and a nested generic type.
#[test]
fn type_display_forms() {
    let row = Row::closed(vec![
        RowField {
            name: Ident::new("vehicle"),
            ty: Ty::NodeRef(Ident::new("Vehicle")),
        },
        RowField {
            name: Ident::new("score"),
            ty: Ty::Estimate(Box::new(Ty::F64)),
        },
    ]);
    let rel = Ty::rel(row);
    let nested = Ty::option(Ty::list(Ty::Money(Ident::new("EUR"))));
    insta::assert_snapshot!(format!("{rel}\n{nested}"), @r"
    Rel<{ vehicle: NodeRef<Vehicle>; score: Estimate<F64> }>
    Option<List<Money<EUR>>>
    ");
}

/// A strict constraint and a query, the other two checked declaration nodes.
#[test]
fn constraint_and_query_display() {
    let c = Constraint {
        name: Ident::new("NoPriceConflicts"),
        severity: Severity::Strict,
        body: Pattern::new(vec![Clause::Edge {
            bind: None,
            relation: QualIdent::from("KeyConflict"),
            args: vec![RoleArg {
                role: Ident::new("relation"),
                arg: Arg::Var(Ident::new("ComputedPrice")),
            }],
        }]),
    };
    let q = Query {
        name: Ident::new("OpenOrders"),
        body: Pattern::new(vec![edge("Order", &[("id", "o")])]),
        yields: Expr::new(
            Ty::NodeRef(Ident::new("Order")),
            ExprKind::Var(Ident::new("o")),
        ),
        result: Ty::rel(Row::closed(vec![RowField {
            name: Ident::new("order"),
            ty: Ty::NodeRef(Ident::new("Order")),
        }])),
    };
    insta::assert_snapshot!(format!("{c}\n{q}"), @r"
    constraint NoPriceConflicts strict { KeyConflict(relation: ComputedPrice) }
    query OpenOrders -> Rel<{ order: NodeRef<Order> }> = from { Order(id: o) } yield o
    ");
}
