//! Differential parity harness (#15 PR2).
//!
//! `infer::infer_source` (the trusted, non-self-hosted bootstrap unification
//! checker) and `reflect::analyze` (the fact-oriented reference analyzer the
//! future native `brix.type` package mirrors) now share one algebra —
//! `brix_ir::solve` — instead of two independent copies that used to
//! silently diverge (Probability↔F64 bridge, dimension-vs-variable solving,
//! row-check symmetry, Option/Result descent, occurs-check depth; see the
//! #15 issue's "Trajectory plan" ruling). This file is the property test
//! that proves the two checkers cannot re-diverge undetected: for every
//! fixture below it asserts the frozen parity contract —
//!
//! 1. **Verdict equivalence**: `analyze(S,Σ).is_consistent() ⟺
//!    infer_source(S,Σ).is_empty()`.
//! 2. **Category-set equivalence**: every `TypeError`/`TypeConflict` maps to
//!    one of seven coarse categories (below), compared as a canonical
//!    *set* — never a sequence. `infer` cascades in body-traversal order and
//!    `reflect`'s facts are derivation-set-valued, so a sequence comparison
//!    would fail on harmless reordering rather than a real divergence.
//! 3. **Type mirror**: every zonked `Expr.ty` `infer_source` leaves behind
//!    has a matching `Fact::HasType{Subject::Expr{origin}, ty}` in
//!    `analyze`'s report, with the identical resolved type.
//!
//! Declaration granularity: every fixture is exactly one declaration (one
//! `Rule`/`Query`), so "declaration" is the fixture itself and is folded in
//! at the harness level (each fixture is its own `#[test]`) rather than
//! extracted from `TypeError`, which does not carry a declaration name or
//! `ExprOrigin` as of this PR — only `reflect::Fact` carries real
//! provenance (`Subject`). Widening `TypeError` to carry the same
//! provenance is natural follow-up work (alongside PR 3's content-addressed
//! `Derivation::id`, which has the identical "positional stopgap, upgrade
//! later" status) rather than something silently dropped here.
//!
//! Real fixtures need every `Expr` node to carry a genuinely distinct
//! [`ExprOrigin`] — `Expr::new` alone stamps every node with the *same*
//! constant synthetic origin, which is fine for infer.rs/reflect.rs's own
//! narrow unit tests (each builds at most one interesting `Expr` node with
//! bare `Ty::Var` placeholders) but collapses this harness's multi-node
//! fixtures into one bogus shared "subject" and, on `infer.rs`'s side,
//! reuses a single placeholder `TyVar` across unrelated nodes within the
//! same declaration (`Infer::expr` unifies each node's *original*
//! lowering-assigned `ty` against its computed type, so a shared
//! placeholder silently binds one node's real answer onto another's). Real
//! lowered programs never hit this — `brixc`'s lowering assigns a fresh
//! `TyVar` and a real source-derived `ExprOrigin` per node — so `Origins`
//! below exists purely to make hand-built fixtures behave like real ones.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use brix_ir::core::{Expr, ExprKind, ExprOrigin, Head, Query, Rule, SourceRange};
use brix_ir::effects::EffectRow;
use brix_ir::frontend::{
    FnSignature, FrontendSource, RelationSchema, SchemaResolver, TableResolver,
};
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::infer::{infer_source, TypeError};
use brix_ir::pattern::{Arg, Clause, Lit, Pattern, RoleArg};
use brix_ir::reflect::{analyze, Fact, Subject};
use brix_ir::types::{
    dimensions_div, money_dimensions, quantity_dimensions, IntWidth, Row, RowField, Ty, TyVar,
};

/// The parity contract's shared error vocabulary — deliberately distinct
/// from either checker's own error type. See the #15 PR2 category map.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Category {
    Mismatch,
    Dimension,
    Arity,
    UnknownField,
    TryNonResult,
    NonBoolGuard,
    Occurs,
}

fn infer_category(error: &TypeError) -> Category {
    match error {
        TypeError::Mismatch { .. } => Category::Mismatch,
        TypeError::Dimension { .. } => Category::Dimension,
        TypeError::Arity { .. } => Category::Arity,
        TypeError::UnknownField { .. } => Category::UnknownField,
        TypeError::NonBoolGuard { .. } => Category::NonBoolGuard,
        TypeError::TryNonResult { .. } => Category::TryNonResult,
        TypeError::Occurs { .. } => Category::Occurs,
    }
}

/// `reflect::TypeConflict::operation` -> `Category`, per the #15 PR2
/// category map: `Mismatch<->{unify,if,call,query-result,role,head}`,
/// `Dimension<->{add,sub,mul,div,eq,ne,lt,le,gt,ge}`, `Arity<->arity`,
/// `UnknownField<->field`, `TryNonResult<->try`, `NonBoolGuard<->when`,
/// `Occurs<->occurs`.
///
/// Two documented departures from a literal reading of the map, both
/// verified against the actual call sites in `reflect.rs`:
/// - `"when"` has exactly one call site (`pattern()`'s guard check) and it
///   is the direct analog of `infer.rs`'s dedicated `NonBoolGuard` variant,
///   so it maps there, not to `Mismatch`.
/// - `"row"` (row-*shape* mismatch) is folded into `UnknownField` alongside
///   `"field"` (single-field lookup miss): `infer.rs` raises the same
///   `TypeError::UnknownField` for both call sites (see
///   `Infer::unify_rows`), so collapsing `reflect.rs`'s finer-grained
///   `field`/`row` split is required for the sets to line up — it is a
///   legitimate taxonomy difference in each checker's *observation*, not a
///   divergence in `solve::match_rows`, the one row-matching algorithm both
///   now call.
/// - `"and"`/`"or"`/`"not"` are plain `unify(..., op, ...)` call sites with
///   no more specific context, exactly like `"unify"` itself, so they fold
///   into `Mismatch` for the same reason.
fn reflect_category(operation: &str) -> Category {
    match operation {
        "unify" | "if" | "call" | "query-result" | "role" | "head" | "and" | "or" | "not" => {
            Category::Mismatch
        }
        "add" | "sub" | "mul" | "div" | "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
            Category::Dimension
        }
        "arity" => Category::Arity,
        "field" | "row" => Category::UnknownField,
        "try" => Category::TryNonResult,
        "when" => Category::NonBoolGuard,
        "occurs" => Category::Occurs,
        other => panic!("parity harness: unmapped reflect.rs conflict operation {other:?}"),
    }
}

fn collect_expr_types(pattern: &Pattern, out: &mut BTreeMap<ExprOrigin, Ty>) {
    for clause in &pattern.clauses {
        match clause {
            Clause::Let { expr, .. } | Clause::When(expr) => collect_expr_type(expr, out),
            Clause::Any(cases) => {
                for case in cases {
                    collect_expr_types(case, out);
                }
            }
            Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                collect_expr_types(p, out)
            }
            _ => {}
        }
    }
}

/// Mirrors `Infer::zonk_expr`'s traversal exactly, so it visits the same
/// node set `Reflect::expr` records `HasType` facts for.
fn collect_expr_type(expr: &Expr, out: &mut BTreeMap<ExprOrigin, Ty>) {
    out.insert(expr.origin, expr.ty.clone());
    match &*expr.kind {
        ExprKind::Call { args, .. } => {
            for a in args {
                collect_expr_type(a, out);
            }
        }
        ExprKind::Field { base, .. } => collect_expr_type(base, out),
        ExprKind::Record { fields } => {
            for (_, v) in fields {
                collect_expr_type(v, out);
            }
        }
        ExprKind::If { cond, then, els } => {
            collect_expr_type(cond, out);
            collect_expr_type(then, out);
            collect_expr_type(els, out);
        }
        ExprKind::Try { inner, .. } => collect_expr_type(inner, out),
        ExprKind::Comprehension { pattern, yields } => {
            collect_expr_types(pattern, out);
            if let Some(y) = yields {
                collect_expr_type(y, out);
            }
        }
        ExprKind::Var(_) | ExprKind::Lit(_) => {}
    }
}

/// Run both checkers over one single-declaration fixture and assert the
/// full parity contract.
fn assert_parity(label: &str, source: &FrontendSource, resolver: &impl SchemaResolver) {
    let report = analyze(source, resolver);
    let mut bootstrap = source.clone();
    let errors = infer_source(&mut bootstrap, resolver);

    // 1. Verdict equivalence.
    assert_eq!(
        report.is_consistent(),
        errors.is_empty(),
        "{label}: verdict mismatch — reflect.is_consistent()={}, infer errors={errors:?}",
        report.is_consistent(),
    );

    // 2. Category-set equivalence (canonical sets, not sequences).
    let infer_categories: BTreeSet<Category> = errors.iter().map(infer_category).collect();
    let reflect_categories: BTreeSet<Category> = report
        .conflicts
        .iter()
        .map(|c| reflect_category(&c.operation))
        .collect();
    assert_eq!(
        infer_categories, reflect_categories,
        "{label}: category-set mismatch\ninfer errors: {errors:#?}\nreflect conflicts: {:#?}",
        report.conflicts
    );

    // 3. Type mirror: every zonked `Expr.ty` from `infer` has a matching
    // `Fact::HasType{Subject::Expr{origin}, ty}` in `analyze`, with the
    // equal resolved type.
    let mut infer_types = BTreeMap::new();
    for rule in &bootstrap.rules {
        collect_expr_types(&rule.body, &mut infer_types);
    }
    for query in &bootstrap.queries {
        collect_expr_types(&query.body, &mut infer_types);
        collect_expr_type(&query.yields, &mut infer_types);
    }

    let mut reflect_types = BTreeMap::new();
    for derivation in &report.facts {
        if let Fact::HasType {
            subject: Subject::Expr { origin },
            ty,
        } = &derivation.fact
        {
            reflect_types.insert(*origin, ty.clone());
        }
    }

    for (origin, ty) in &infer_types {
        let reflected = reflect_types.get(origin).unwrap_or_else(|| {
            panic!("{label}: infer zonked {origin:?} to {ty} but reflect recorded no HasType fact for it")
        });
        assert_eq!(
            ty, reflected,
            "{label}: type-mirror mismatch at {origin:?}: infer={ty}, reflect={reflected}"
        );
    }
}

/// Hands out distinct `ExprOrigin`s (and, via [`Origins::ty_var`], distinct
/// placeholder `TyVar`s) within one fixture — see the module doc for why
/// hand-built fixtures need this and real lowered programs don't.
struct Origins {
    declaration: Ident,
    next: Cell<u32>,
}

impl Origins {
    fn new(declaration: &str) -> Self {
        Origins {
            declaration: Ident::new(declaration),
            next: Cell::new(0),
        }
    }

    fn next_origin(&self) -> ExprOrigin {
        let n = self.next.get();
        self.next.set(n + 1);
        ExprOrigin::source(
            &self.declaration,
            SourceRange {
                start: n,
                end: n + 1,
            },
        )
    }

    /// A placeholder `TyVar` guaranteed unused by any fixture's deliberate,
    /// hand-picked `TyVar`s (all of which stay below this offset).
    fn ty_var(&self) -> Ty {
        let n = self.next.get();
        self.next.set(n + 1);
        Ty::Var(TyVar(100_000 + n))
    }

    fn var(&self, name: &str) -> Expr {
        Expr::new(self.ty_var(), ExprKind::Var(Ident::new(name))).with_origin(self.next_origin())
    }

    fn lit(&self, ty: Ty, lit: Lit) -> Expr {
        Expr::new(ty, ExprKind::Lit(lit)).with_origin(self.next_origin())
    }

    fn op(&self, name: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Call {
                func: QualIdent::from(format!("brix.ops.{name}").as_str()),
                args,
            },
        )
        .with_origin(self.next_origin())
    }

    fn call(&self, func: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Call {
                func: QualIdent::from(func),
                args,
            },
        )
        .with_origin(self.next_origin())
    }

    fn field(&self, base: Expr, field: &str) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Field {
                base,
                field: Ident::new(field),
            },
        )
        .with_origin(self.next_origin())
    }

    fn record(&self, fields: Vec<(&str, Expr)>) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Record {
                fields: fields
                    .into_iter()
                    .map(|(name, value)| (Ident::new(name), value))
                    .collect(),
            },
        )
        .with_origin(self.next_origin())
    }
}

/// Fixture 1 (#15 acceptance): the flagship pricing computation
/// (`rate * length + surcharge`) mutated to `rate / length + surcharge`,
/// the same one-character dimension-breaking mutation
/// `crates/brixc/tests/lower_flagship.rs::
/// flagship_pricing_multiply_to_divide_mutation_is_one_dimension_error`
/// exercises end-to-end through the real `.brix` fixture and lowering
/// pipeline. This is the identical `Money<EUR>/Kilometre` rate shape at the
/// brix-ir level (no brixc/AST dependency available from this crate).
#[test]
fn flagship_pricing_mutation_agrees() {
    let o = Origins::new("Price");
    let eur = Ident::new("EUR");
    let km = Ident::new("Kilometre");
    let rate = Ty::Dimensioned(dimensions_div(
        &money_dimensions(&eur),
        &quantity_dimensions(&km),
    ));
    let query = Query {
        name: Ident::new("Price"),
        params: vec![
            (Ident::new("rate"), rate),
            (Ident::new("length"), Ty::Quantity(km)),
            (Ident::new("surcharge"), Ty::Money(eur)),
        ],
        body: Pattern::default(),
        yields: o.op(
            "add",
            vec![
                o.op("div", vec![o.var("rate"), o.var("length")]),
                o.var("surcharge"),
            ],
        ),
        result: o.ty_var(),
    };
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![query],
    };
    assert_parity("flagship_pricing_mutation", &source, &TableResolver::new());
}

/// Fixture 2: a `when` guard whose expression is not `Bool`.
#[test]
fn non_bool_guard_agrees() {
    let o = Origins::new("R");
    let source = FrontendSource {
        rules: vec![Rule {
            name: Ident::new("R"),
            head: Head::Mask {
                target: Ident::new("a"),
                reason: Ident::new("b"),
            },
            body: Pattern::new(vec![Clause::When(
                o.lit(Ty::Int(IntWidth::Int), Lit::Int(1)),
            )]),
            effects: EffectRow::empty(),
        }],
        constraints: vec![],
        queries: vec![],
    };
    assert_parity("non_bool_guard", &source, &TableResolver::new());
}

/// Fixture 3: calling a declared function with the wrong number of
/// arguments.
#[test]
fn arity_mismatch_agrees() {
    let o = Origins::new("Arity");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Arity"),
            params: vec![],
            body: Pattern::default(),
            yields: o.call("f", vec![]),
            result: o.ty_var(),
        }],
    };
    let resolver = TableResolver::new().with_function(FnSignature {
        name: QualIdent::from("f"),
        params: vec![Ty::Int(IntWidth::Int)],
        ret: Ty::Int(IntWidth::Int),
        effects: EffectRow::empty(),
        is_aggregate: false,
        may_diverge: false,
    });
    assert_parity("arity_mismatch", &source, &resolver);
}

/// Fixture 4: an edge clause's literal role argument does not match the
/// relation schema's declared role type.
///
/// Uses a `Rule`, not a `Constraint`: `infer_source` does not visit
/// `FrontendSource::constraints` at all (only `rules`/`queries`) while
/// `reflect::analyze` does — a real, independently-discovered divergence
/// this harness caught, but a *coverage* gap (which declarations get
/// checked), not one of the five unification-algebra divergences #15 PR2
/// scopes. Left for a follow-up PR; flagged in the PR report rather than
/// fixed here or silently routed around.
#[test]
fn role_mismatch_agrees() {
    let source = FrontendSource {
        rules: vec![Rule {
            name: Ident::new("RoleGuard"),
            head: Head::Mask {
                target: Ident::new("t"),
                reason: Ident::new("r"),
            },
            body: Pattern::new(vec![Clause::Edge {
                bind: None,
                relation: QualIdent::from("R"),
                args: vec![RoleArg {
                    role: Ident::new("n"),
                    arg: Arg::Lit(Lit::Bool(true)),
                }],
            }]),
            effects: EffectRow::empty(),
        }],
        constraints: vec![],
        queries: vec![],
    };
    let resolver = TableResolver::new().with_relation(RelationSchema {
        name: QualIdent::from("R"),
        roles: vec![(Ident::new("n"), Ty::Int(IntWidth::Int))],
        key: vec![],
        model_closed: true,
        derived: false,
    });
    assert_parity("role_mismatch", &source, &resolver);
}

/// Fixture 5: a field access on a record type that does not declare that
/// field.
#[test]
fn field_failure_agrees() {
    let o = Origins::new("MissingField");
    let record = Ty::record(Row::closed(vec![RowField {
        name: Ident::new("present"),
        ty: Ty::Int(IntWidth::Int),
    }]));
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("MissingField"),
            params: vec![(Ident::new("record"), record)],
            body: Pattern::default(),
            yields: o.field(o.var("record"), "absent"),
            result: o.ty_var(),
        }],
    };
    assert_parity("field_failure", &source, &TableResolver::new());
}

/// Fixture 6: a genuine occurs-check failure reached through the public
/// `Query` surface (not by poking either checker's private `unify`
/// directly) — `result` expects `Rel<{value: Option<?v>}>` while `yields`
/// resolves to plain `?v` (the same variable, via `params`), forcing
/// `unify(Option<?v>, ?v)` to attempt binding `?v := Option<?v>`.
#[test]
fn occurs_check_agrees() {
    let o = Origins::new("Occurs");
    let v = TyVar(9100);
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Occurs"),
            params: vec![(Ident::new("x"), Ty::Var(v))],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::option(Ty::Var(v)),
            }])),
        }],
    };
    assert_parity("occurs_check", &source, &TableResolver::new());
}

/// Fixture 7 (row symmetry, conflicting side): `query.result` declares a
/// *closed* `{a}` row but the yielded record is `{a, b}` — an extra field
/// on the *found* side of a closed row. Catching this requires the
/// symmetric row check (ruling: reflect.rs's two-directional
/// `solve::match_rows` wins) — the old, left-only `infer.rs` check would
/// have missed it, since every field the closed `{a}` side lists (`a`) is
/// present on the other side.
#[test]
fn closed_row_extra_field_is_a_mismatch() {
    let o = Origins::new("ClosedRow");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("ClosedRow"),
            params: vec![],
            body: Pattern::default(),
            yields: o.record(vec![
                ("a", o.lit(Ty::Int(IntWidth::Int), Lit::Int(1))),
                ("b", o.lit(Ty::Bool, Lit::Bool(true))),
            ]),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("a"),
                ty: Ty::Int(IntWidth::Int),
            }])),
        }],
    };
    assert_parity("closed_row_extra_field", &source, &TableResolver::new());
}

/// Fixture 8 (row symmetry, admissible side): the same extra-field shape as
/// fixture 7, but `query.result` declares an *open* row — row polymorphism
/// admits the extra field on the found side, so both checkers must agree
/// this type-checks cleanly.
#[test]
fn open_row_extra_field_is_admitted() {
    let o = Origins::new("OpenRow");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("OpenRow"),
            params: vec![],
            body: Pattern::default(),
            yields: o.record(vec![
                ("a", o.lit(Ty::Int(IntWidth::Int), Lit::Int(1))),
                ("b", o.lit(Ty::Bool, Lit::Bool(true))),
            ]),
            result: Ty::rel(Row::open(
                vec![RowField {
                    name: Ident::new("a"),
                    ty: Ty::Int(IntWidth::Int),
                }],
                TyVar(9009),
            )),
        }],
    };
    assert_parity("open_row_extra_field", &source, &TableResolver::new());
}
