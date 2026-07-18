//! The differential fixture corpus (#15 PR6: "conformance: differential
//! fixture corpus as a reusable suite").
//!
//! Before this module, the type-inference fixture corpus lived entirely
//! inside `crates/brix-ir/tests/parity.rs`'s `#[test]` bodies — one Rust
//! source literal per test, unreachable from anywhere else. That worked for
//! a single Rust-vs-Rust harness (`infer::infer_source` vs
//! `reflect::analyze`), but `brix-ir` cannot depend on `brix-conformance`
//! (that would be a cycle — `brix-conformance` already depends on
//! `brix-ir`, verified against both crates' `Cargo.toml`s before this
//! restructuring), so the corpus could never be reused by a second checker
//! living in `brix-conformance` — namely the native `brix.type`
//! shadow-mode package (`crates/brix-conformance/tests/selfhost_parity.rs`,
//! #15 native slice 1).
//!
//! This module re-homes the corpus as **data**, independent of any one
//! harness: each fixture is a [`TypeFixture`] or [`RuleFixture`] — a
//! `FrontendSource`/`Rule` + `SchemaResolver` + the category set both
//! checkers must independently arrive at — built by a plain function, not
//! asserted inside a `#[test]`. Two harnesses now consume this corpus
//! verbatim:
//!
//! - `crates/brix-conformance/tests/type_parity.rs` — the promoted
//!   Rust-vs-Rust parity harness (`assert_parity`/
//!   `assert_rule_side_condition_parity`), formerly
//!   `crates/brix-ir/tests/parity.rs` (now deleted).
//! - `crates/brix-conformance/tests/selfhost_parity.rs` — the native
//!   `brix.type` shadow-mode harness, which reuses the two slice-1 `.brix`
//!   source fixtures ([`NATIVE_ROLE_BINDINGS_FIXTURE`]/
//!   [`NATIVE_ROLE_LIT_MISMATCH_FIXTURE`]) defined here rather than
//!   re-typing them inline.
//!
//! A fixture added once here is checked by every checker that reuses it.

use std::cell::Cell;
use std::collections::BTreeSet;

use brix_ir::core::{
    Constraint, Expr, ExprKind, ExprOrigin, Head, Query, Rule, Severity, SourceRange,
};
use brix_ir::effects::{Effect, EffectRow};
use brix_ir::frontend::{FnSignature, FrontendSource, RelationSchema, TableResolver};
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::pattern::{Arg, Clause, Lit, Pattern, RoleArg};
use brix_ir::site::SiteId;
use brix_ir::types::{
    dimensions_div, money_dimensions, quantity_dimensions, IntWidth, Row, RowField, Ty, TyVar,
};

/// The taxonomy of conformance categories this corpus organizes fixtures
/// under. [`TypeInference`](ConformanceCategory::TypeInference) and
/// [`RuleSideCondition`](ConformanceCategory::RuleSideCondition) tag every
/// fixture below (see [`TypeFixture::category`]/[`RuleFixture::category`]).
///
/// [`ScopedWorldNonLeak`](ConformanceCategory::ScopedWorldNonLeak) is
/// **reserved, name-only** — per the #15 PR 3.5 Fable ruling ("Reserve a
/// 'scoped-world non-leak' conformance category in PR 6 (name now, fixtures
/// post-gate)"). It names, ahead of time, the category that will hold
/// fixtures for the post-gate scoped-world slice: once `TypeScope{id,
/// parent}` + assumption-labeled facts exist (the ruling's "Scoped-worlds
/// (post-gate, do NOT build now)" section), fixtures here will prove that
/// retracting a hypothesis in one scope — dropping a *world* — never leaks
/// a derived judgment into a sibling or parent scope, orthogonally to how
/// masks defeat *edges* within one world. **No fixtures populate this
/// variant yet; do not add assertions against it ahead of the scoped-world
/// implementation landing.**
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ConformanceCategory {
    TypeInference,
    RuleSideCondition,
    /// Reserved post-gate — see the variant's doc above. No fixtures yet.
    ScopedWorldNonLeak,
}

/// The parity contract's shared error vocabulary — deliberately distinct
/// from either checker's own error type. See the #15 PR2 category map.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Category {
    Mismatch,
    Dimension,
    Arity,
    UnknownField,
    TryNonResult,
    NonBoolGuard,
    Occurs,
    /// #15 PR5: §19.1 / conformance I.22.2's forbidden epistemic-to-plain
    /// erasure (`Estimate<T>`/`Missing<T>` -/-> `T`, `Probability` -/->
    /// `Bool`) — a named category, not folded into `Mismatch`.
    EpistemicErasure,
}

/// #15 PR4's parity vocabulary: Appendix E rule side-condition categories,
/// mirroring [`Category`] but for the `check_rule`/`reflect::analyze` axis.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RuleCategory {
    Impure,
    Nondeterministic,
    Divergent,
    UnboundHeadKey,
    MaskRefNotEdgeBound,
    OrdinaryFnOnDerivedRel,
}

/// One declaration-level fixture for the type-inference parity axis
/// (`infer::infer_source` vs `reflect::analyze`).
pub struct TypeFixture {
    pub label: &'static str,
    pub category: ConformanceCategory,
    pub source: FrontendSource,
    pub resolver: TableResolver,
    /// The category set both checkers must independently arrive at for this
    /// fixture — literal corpus-declared ground truth, checked in addition
    /// to (not instead of) the two checkers agreeing with each other.
    pub expected_categories: BTreeSet<Category>,
}

/// One fixture for the Appendix E rule side-condition parity axis
/// (`check::check_rule` vs `reflect::analyze`).
pub struct RuleFixture {
    pub label: &'static str,
    pub category: ConformanceCategory,
    pub rule: Rule,
    pub resolver: TableResolver,
    pub expected_categories: BTreeSet<RuleCategory>,
}

/// Hands out distinct `ExprOrigin`s (and, via [`Origins::ty_var`], distinct
/// placeholder `TyVar`s) within one fixture. Real lowered programs never
/// need this — `brixc`'s lowering assigns a fresh `TyVar` and a real
/// source-derived `ExprOrigin` per node — so `Origins` exists purely to
/// make hand-built fixtures behave like real ones (see
/// `type_parity.rs`'s module doc for the full explanation of why that
/// matters for the harness built on this corpus).
pub struct Origins {
    declaration: Ident,
    next: Cell<u32>,
}

impl Origins {
    pub fn new(declaration: &str) -> Self {
        Origins {
            declaration: Ident::new(declaration),
            next: Cell::new(0),
        }
    }

    pub fn next_origin(&self) -> ExprOrigin {
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
    pub fn ty_var(&self) -> Ty {
        let n = self.next.get();
        self.next.set(n + 1);
        Ty::Var(TyVar(100_000 + n))
    }

    pub fn var(&self, name: &str) -> Expr {
        Expr::new(self.ty_var(), ExprKind::Var(Ident::new(name))).with_origin(self.next_origin())
    }

    pub fn lit(&self, ty: Ty, lit: Lit) -> Expr {
        Expr::new(ty, ExprKind::Lit(lit)).with_origin(self.next_origin())
    }

    pub fn op(&self, name: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Call {
                func: QualIdent::from(format!("brix.ops.{name}").as_str()),
                args,
            },
        )
        .with_origin(self.next_origin())
    }

    pub fn call(&self, func: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Call {
                func: QualIdent::from(func),
                args,
            },
        )
        .with_origin(self.next_origin())
    }

    pub fn field(&self, base: Expr, field: &str) -> Expr {
        Expr::new(
            self.ty_var(),
            ExprKind::Field {
                base,
                field: Ident::new(field),
            },
        )
        .with_origin(self.next_origin())
    }

    pub fn record(&self, fields: Vec<(&str, Expr)>) -> Expr {
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

/// Fixture 1 (#15 acceptance, upgraded by #33): the flagship pricing
/// computation (`rate * length + surcharge`) mutated to
/// `rate / length + surcharge`, the same one-character dimension-breaking
/// mutation `crates/brixc/tests/lower_flagship.rs::
/// flagship_pricing_multiply_to_divide_mutation_is_one_dimension_error`
/// exercises end-to-end through the real `.brix` fixture and lowering
/// pipeline. This is the identical `Money<EUR>/Kilometre` rate shape at the
/// brix-ir level (no brixc/AST dependency available from this crate).
///
/// #33 closed the `infer_source`-never-visits-`constraints` coverage gap,
/// so this fixture also carries the flagship's actual `Capacity` constraint
/// body — `Move(order: o, vehicle: v); o: Order { weight: w }; v: Vehicle {
/// capacity: cap }; when w > cap` — verbatim from
/// `0001-part-i-the-flagship-program.brix`.
pub fn flagship_pricing_mutation() -> TypeFixture {
    let o = Origins::new("Price");
    let eur = Ident::new("EUR");
    let km = Ident::new("Kilometre");
    let mass = Ident::new("Mass");
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

    // Reuses `o`, not a second `Origins`: two `Origins` instances both start
    // their placeholder-`TyVar` counter at the same 100_000 offset (see
    // `Origins::ty_var`'s doc), which is fine when each fixture has exactly
    // one, but two in the same fixture collide across declarations.
    let capacity = Constraint {
        name: Ident::new("Capacity"),
        severity: Severity::Strict,
        body: Pattern::new(vec![
            Clause::Edge {
                bind: None,
                relation: QualIdent::from("Move"),
                args: vec![
                    RoleArg {
                        role: Ident::new("order"),
                        arg: Arg::Var(Ident::new("o")),
                    },
                    RoleArg {
                        role: Ident::new("vehicle"),
                        arg: Arg::Var(Ident::new("v")),
                    },
                ],
            },
            Clause::Entity {
                var: Ident::new("o"),
                entity: Ident::new("Order"),
                fields: vec![RoleArg {
                    role: Ident::new("weight"),
                    arg: Arg::Var(Ident::new("w")),
                }],
            },
            Clause::Entity {
                var: Ident::new("v"),
                entity: Ident::new("Vehicle"),
                fields: vec![RoleArg {
                    role: Ident::new("capacity"),
                    arg: Arg::Var(Ident::new("cap")),
                }],
            },
            Clause::When(o.op("gt", vec![o.var("w"), o.var("cap")])),
        ]),
    };

    let source = FrontendSource {
        rules: vec![],
        constraints: vec![capacity],
        queries: vec![query],
    };
    let resolver = TableResolver::new()
        .with_relation(RelationSchema {
            name: QualIdent::from("Move"),
            roles: vec![
                (Ident::new("order"), Ty::NodeRef(Ident::new("Order"))),
                (Ident::new("vehicle"), Ty::NodeRef(Ident::new("Vehicle"))),
            ],
            key: vec![Ident::new("order")],
            model_closed: true,
            derived: false,
        })
        .with_relation(RelationSchema {
            name: QualIdent::from("Order"),
            roles: vec![(Ident::new("weight"), Ty::Quantity(mass.clone()))],
            key: vec![],
            model_closed: true,
            derived: false,
        })
        .with_relation(RelationSchema {
            name: QualIdent::from("Vehicle"),
            roles: vec![(Ident::new("capacity"), Ty::Quantity(mass))],
            key: vec![],
            model_closed: true,
            derived: false,
        });
    TypeFixture {
        label: "flagship_pricing_mutation",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::from([Category::Dimension]),
    }
}

/// Fixture 2: a `when` guard whose expression is not `Bool`.
pub fn non_bool_guard() -> TypeFixture {
    let o = Origins::new("R");
    let source = FrontendSource {
        rules: vec![Rule {
            name: Ident::new("R"),
            // A tuple head, not a mask head — the guard-typing shape this
            // fixture exercises is orthogonal to the mask-head side
            // condition (#15 PR4), and a mask head here would need `a`/`b`
            // edge-bound to stay green under `MaskRefNotEdgeBound`.
            head: Head::Tuple {
                relation: QualIdent::from("Out"),
                args: vec![],
            },
            body: Pattern::new(vec![Clause::When(
                o.lit(Ty::Int(IntWidth::Int), Lit::Int(1)),
            )]),
            effects: EffectRow::empty(),
        }],
        constraints: vec![],
        queries: vec![],
    };
    TypeFixture {
        label: "non_bool_guard",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::NonBoolGuard]),
    }
}

/// Fixture 3: calling a declared function with the wrong number of
/// arguments.
pub fn arity_mismatch() -> TypeFixture {
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
    TypeFixture {
        label: "arity_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::from([Category::Arity]),
    }
}

/// Fixture 4: an edge clause's literal role argument does not match the
/// relation schema's declared role type. Uses a `Rule`, not a `Constraint`,
/// purely for fixture variety — `constraint_role_mismatch` below exercises
/// the same shape through a `Constraint` body.
pub fn role_mismatch() -> TypeFixture {
    let source = FrontendSource {
        rules: vec![Rule {
            name: Ident::new("RoleGuard"),
            head: Head::Tuple {
                relation: QualIdent::from("Out"),
                args: vec![],
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
    TypeFixture {
        label: "role_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// Fixture 5: a field access on a record type that does not declare that
/// field.
pub fn field_failure() -> TypeFixture {
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
    TypeFixture {
        label: "field_failure",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::UnknownField]),
    }
}

/// Fixture 6: a genuine occurs-check failure reached through the public
/// `Query` surface (not by poking either checker's private `unify`
/// directly) — `result` expects `Rel<{value: Option<?v>}>` while `yields`
/// resolves to plain `?v` (the same variable, via `params`), forcing
/// `unify(Option<?v>, ?v)` to attempt binding `?v := Option<?v>`.
pub fn occurs_check() -> TypeFixture {
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
    TypeFixture {
        label: "occurs_check",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Occurs]),
    }
}

/// Fixture 7 (row symmetry, conflicting side): `query.result` declares a
/// *closed* `{a}` row but the yielded record is `{a, b}` — an extra field
/// on the *found* side of a closed row. Catching this requires the
/// symmetric row check (ruling: reflect.rs's two-directional
/// `solve::match_rows` wins) — the old, left-only `infer.rs` check would
/// have missed it, since every field the closed `{a}` side lists (`a`) is
/// present on the other side.
pub fn closed_row_extra_field() -> TypeFixture {
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
    TypeFixture {
        label: "closed_row_extra_field",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::UnknownField]),
    }
}

/// Fixture 8 (row symmetry, admissible side): the same extra-field shape as
/// fixture 7, but `query.result` declares an *open* row — row polymorphism
/// admits the extra field on the found side, so both checkers must agree
/// this type-checks cleanly.
pub fn open_row_extra_field() -> TypeFixture {
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
    TypeFixture {
        label: "open_row_extra_field",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::new(),
    }
}

/// Fixture 9 (#33): a `when` guard whose expression is not `Bool`, this time
/// inside a `Constraint` body rather than a `Rule` body — the exact shape
/// `infer_source` used to silently skip entirely (it never visited
/// `FrontendSource::constraints`) while `reflect::analyze` caught it.
pub fn constraint_non_bool_guard() -> TypeFixture {
    let o = Origins::new("Guard");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![Constraint {
            name: Ident::new("Guard"),
            severity: Severity::Strict,
            body: Pattern::new(vec![Clause::When(
                o.lit(Ty::Int(IntWidth::Int), Lit::Int(1)),
            )]),
        }],
        queries: vec![],
    };
    TypeFixture {
        label: "constraint_non_bool_guard",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::NonBoolGuard]),
    }
}

/// Fixture 10 (#33): an edge clause's literal role argument does not match
/// the relation schema's declared role type, this time inside a
/// `Constraint` body rather than a `Rule` body (mirrors fixture 4's shape).
pub fn constraint_role_mismatch() -> TypeFixture {
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![Constraint {
            name: Ident::new("RoleGuard"),
            severity: Severity::Strict,
            body: Pattern::new(vec![Clause::Edge {
                bind: None,
                relation: QualIdent::from("R"),
                args: vec![RoleArg {
                    role: Ident::new("n"),
                    arg: Arg::Lit(Lit::Bool(true)),
                }],
            }]),
        }],
        queries: vec![],
    };
    let resolver = TableResolver::new().with_relation(RelationSchema {
        name: QualIdent::from("R"),
        roles: vec![(Ident::new("n"), Ty::Int(IntWidth::Int))],
        key: vec![],
        model_closed: true,
        derived: false,
    });
    TypeFixture {
        label: "constraint_role_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// Fixture 11: a `?` postfix applied to a non-`Result` value (mirrors what
/// triggers `infer.rs:274`'s dedicated `TypeError::TryNonResult`).
pub fn try_non_result() -> TypeFixture {
    let o = Origins::new("TryNonResult");
    let inner = o.lit(Ty::Int(IntWidth::Int), Lit::Int(1));
    let try_expr = Expr::new(
        o.ty_var(),
        ExprKind::Try {
            inner,
            site: SiteId::derive(&Ident::new("TryNonResult"), 0),
        },
    )
    .with_origin(o.next_origin());
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("TryNonResult"),
            params: vec![],
            body: Pattern::default(),
            yields: try_expr,
            result: o.ty_var(),
        }],
    };
    TypeFixture {
        label: "try_non_result",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::TryNonResult]),
    }
}

/// Fixture 12 (#15 PR4): Appendix E `pure(B, H)` violated — the rule's
/// effect row carries an impure atom (`console`, which is neither `panic`
/// nor `diverge`, so `det`/`nondiverge` stay satisfied and only `pure`
/// fails).
pub fn rule_impure_effect_row() -> RuleFixture {
    let rule = Rule {
        name: Ident::new("Loud"),
        head: Head::Tuple {
            relation: QualIdent::from("Out"),
            args: vec![],
        },
        body: Pattern::default(),
        effects: EffectRow::from_atoms([Effect::Console]),
    };
    RuleFixture {
        label: "rule_impure_effect_row",
        category: ConformanceCategory::RuleSideCondition,
        rule,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([RuleCategory::Impure]),
    }
}

/// Fixture 13 (#15 PR4): Appendix E `keys(H) ⊆ Bindings` violated — a
/// derived-node head's `keyed by (...)` ident is not among the body's
/// bound values.
pub fn rule_unbound_head_key() -> RuleFixture {
    let rule = Rule {
        name: Ident::new("Mint"),
        head: Head::Node {
            var: Ident::new("n"),
            entity: Ident::new("Widget"),
            args: vec![],
            keyed_by: vec![Ident::new("missing")],
        },
        body: Pattern::default(),
        effects: EffectRow::empty(),
    };
    RuleFixture {
        label: "rule_unbound_head_key",
        category: ConformanceCategory::RuleSideCondition,
        rule,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([RuleCategory::UnboundHeadKey]),
    }
}

/// Fixture 14 (#15 PR4): Appendix E mask-head side condition violated —
/// `mask(target) by reason` where neither `target` nor `reason` is an
/// edge-bound alias produced by the body.
pub fn rule_mask_ref_not_edge_bound() -> RuleFixture {
    let rule = Rule {
        name: Ident::new("Override"),
        head: Head::Mask {
            target: Ident::new("price"),
            reason: Ident::new("manual"),
        },
        body: Pattern::default(),
        effects: EffectRow::empty(),
    };
    RuleFixture {
        label: "rule_mask_ref_not_edge_bound",
        category: ConformanceCategory::RuleSideCondition,
        rule,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([RuleCategory::MaskRefNotEdgeBound]),
    }
}

/// Fixture 15 (#15 PR4): Appendix E `Ordinary fn` violated — a non-
/// `aggregate` fn call consumes a graph-derived `Rel` (a `Comprehension`
/// over a relation the schema marks `derived: true`) inside a rule body.
pub fn rule_ordinary_fn_on_derived_rel() -> RuleFixture {
    let o = Origins::new("Summary");
    let comprehension = Expr::new(
        Ty::rel(Row::closed(vec![])),
        ExprKind::Comprehension {
            pattern: Pattern::new(vec![Clause::Edge {
                bind: None,
                relation: QualIdent::from("ComputedPrice"),
                args: vec![RoleArg {
                    role: Ident::new("order"),
                    arg: Arg::Var(Ident::new("o")),
                }],
            }]),
            yields: None,
        },
    )
    .with_origin(o.next_origin());
    let rule = Rule {
        name: Ident::new("Summary"),
        head: Head::Tuple {
            relation: QualIdent::from("Out"),
            args: vec![],
        },
        body: Pattern::new(vec![Clause::Let {
            binds: Ident::new("total"),
            expr: o.call("sumUp", vec![comprehension]),
        }]),
        effects: EffectRow::empty(),
    };
    let resolver = TableResolver::new()
        .with_relation(RelationSchema {
            name: QualIdent::from("ComputedPrice"),
            roles: vec![(Ident::new("order"), Ty::NodeRef(Ident::new("Order")))],
            key: vec![],
            model_closed: true,
            derived: true,
        })
        .with_function(FnSignature {
            name: QualIdent::from("sumUp"),
            params: vec![Ty::rel(Row::closed(vec![]))],
            ret: Ty::Int(IntWidth::Int),
            effects: EffectRow::empty(),
            is_aggregate: false,
            may_diverge: false,
        });
    RuleFixture {
        label: "rule_ordinary_fn_on_derived_rel",
        category: ConformanceCategory::RuleSideCondition,
        rule,
        resolver,
        expected_categories: BTreeSet::from([RuleCategory::OrdinaryFnOnDerivedRel]),
    }
}

/// Fixture 16 (#15 PR5, §19.1): `Estimate<T>` unified against its own plain
/// payload type (`query.result` declares `{value: Int}`, but the yielded
/// param is `Estimate<Int>`) is a named erasure, not a generic mismatch.
pub fn estimate_to_plain_erasure() -> TypeFixture {
    let o = Origins::new("Erasure");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Erasure"),
            params: vec![(
                Ident::new("x"),
                Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            )],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Int(IntWidth::Int),
            }])),
        }],
    };
    TypeFixture {
        label: "estimate_to_plain_erasure",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::EpistemicErasure]),
    }
}

/// Fixture 17 (#15 PR5, §19.1): `Probability` unified against `Bool` is a
/// named erasure — distinct from (and must not be confused with) the
/// separate, deliberately-kept `Probability ~ F64` v1 bridge exercised by
/// `flagship_pricing_mutation`'s `Escalate`-shaped guard elsewhere in the
/// flagship.
pub fn probability_to_bool_erasure() -> TypeFixture {
    let o = Origins::new("Erasure");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Erasure"),
            params: vec![(Ident::new("p"), Ty::Probability)],
            body: Pattern::default(),
            yields: o.var("p"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Bool,
            }])),
        }],
    };
    TypeFixture {
        label: "probability_to_bool_erasure",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::EpistemicErasure]),
    }
}

/// Fixture 18 (#15 PR5, §27.3 / conformance I.22.2): `Missing<T>` must not
/// silently coerce to `T` — same named `EpistemicErasure` family as
/// `Estimate<T>`/`Probability` above, not a generic mismatch.
pub fn missing_to_plain_implicit_coercion() -> TypeFixture {
    let o = Origins::new("Erasure");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Erasure"),
            params: vec![(Ident::new("m"), Ty::missing(Ty::Bool))],
            body: Pattern::default(),
            yields: o.var("m"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Bool,
            }])),
        }],
    };
    TypeFixture {
        label: "missing_to_plain_implicit_coercion",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::EpistemicErasure]),
    }
}

/// Fixture 19 (#15 PR5, §27.3): a well-typed `Missing<T>` flow — the yielded
/// param and the declared result agree on the identical `Missing<Int>` type,
/// no coercion attempted — both checkers must accept it with zero
/// conflicts.
pub fn missing_well_typed_flow() -> TypeFixture {
    let o = Origins::new("MissingFlow");
    let source = FrontendSource {
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("MissingFlow"),
            params: vec![(Ident::new("m"), Ty::missing(Ty::Int(IntWidth::Int)))],
            body: Pattern::default(),
            yields: o.var("m"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::missing(Ty::Int(IntWidth::Int)),
            }])),
        }],
    };
    TypeFixture {
        label: "missing_well_typed_flow",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::new(),
    }
}

/// The smallest native-slice fixture (#15 slice-1 ruling): a two-role body
/// clause with both roles bound to variables. `reflect.rs` records exactly
/// two `Fact::HasType(Subject::Binding)` facts for it (`count`, `label`) —
/// the native package, driven only by the `RoleVar` facts `role_arg`
/// emits, must derive `FactId`-for-`FactId` the same two.
///
/// Raw `.brix` source (not a [`TypeFixture`]) because the native shadow
/// harness drives it through the real `brix_ast::parse_file` ->
/// `brixc::lower_file` pipeline, not the hand-built `FrontendSource`
/// surface the Rust-vs-Rust fixtures above use.
pub const NATIVE_ROLE_BINDINGS_FIXTURE: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count, label)
}
"#;

/// The twin native-slice fixture: `label`'s role argument is a literal of
/// the wrong class (`Int` where the schema declares `String`) instead of a
/// variable. `count` stays a plain variable so the rule head (`Output`,
/// keyed on `count`) still binds cleanly — the #15 ruling explicitly allows
/// mismatch on `count` *or* `label`; `label` is chosen here so the twin
/// doesn't also need to work around an unrelated unbound-head-key
/// Appendix-E finding. `reflect.rs` raises exactly one
/// `ConflictKind::Mismatch` for this, at `Subject::Binding { declaration:
/// "Copy", name: "label" }` — the native package's `LitRoleMismatch` rule,
/// driven by the new `RoleLit` fact, must derive exactly one oriented
/// `MismatchConflict` row that decodes back to the identical (subject,
/// expect, found, scope).
pub const NATIVE_ROLE_LIT_MISMATCH_FIXTURE: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count: count, label: 5)
}
"#;

/// All 15 type-inference-axis fixtures, in corpus order. Convenience for
/// consumers that want to iterate the whole axis rather than naming each
/// fixture function individually.
pub fn all_type_fixtures() -> Vec<TypeFixture> {
    vec![
        flagship_pricing_mutation(),
        non_bool_guard(),
        arity_mismatch(),
        role_mismatch(),
        field_failure(),
        occurs_check(),
        closed_row_extra_field(),
        open_row_extra_field(),
        constraint_non_bool_guard(),
        constraint_role_mismatch(),
        try_non_result(),
        estimate_to_plain_erasure(),
        probability_to_bool_erasure(),
        missing_to_plain_implicit_coercion(),
        missing_well_typed_flow(),
    ]
}

/// All 4 rule-side-condition-axis fixtures, in corpus order.
pub fn all_rule_fixtures() -> Vec<RuleFixture> {
    vec![
        rule_impure_effect_row(),
        rule_unbound_head_key(),
        rule_mask_ref_not_edge_bound(),
        rule_ordinary_fn_on_derived_rel(),
    ]
}
