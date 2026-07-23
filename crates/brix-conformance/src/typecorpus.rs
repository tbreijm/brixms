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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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

/// Fixture 6.5 (#15 native slice 7): [`occurs_check`]'s row-descent
/// counterpart — same shape (a `Query` forcing `unify(?v, <something
/// containing ?v>)` through the public surface), but the something
/// containing `?v` is reached by descending a `Rel` row rather than the
/// `Option` unary family, exercising `TyRowChild`/row descent (a real
/// soundness case `occurs_check`'s all-`Option` path can't reach — a native
/// checker that only ever walked `TyChild` would silently miss an occurs
/// failure buried inside a row field). `result` is `Rel<{value:
/// Rel<{inner: ?v}>}>`, so `unify(Rel<{value: Rel<{inner: ?v}>}>, ?v)`
/// forces `bind(v, Rel<{value: Rel<{inner: ?v}>}>)`, whose occurs-check must
/// descend two nested `Rel` rows (`value`, then `inner`) to find `?v`. Uses
/// a distinct `TyVar` (9101) from `occurs_check`'s (9100) so the two
/// fixtures' facts never collide if ever exported together.
pub fn occurs_check_row() -> TypeFixture {
    let o = Origins::new("OccursRow");
    let v = TyVar(9101);
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("OccursRow"),
            params: vec![(Ident::new("x"), Ty::Var(v))],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::rel(Row::closed(vec![RowField {
                    name: Ident::new("inner"),
                    ty: Ty::Var(v),
                }])),
            }])),
        }],
    };
    TypeFixture {
        label: "occurs_check_row",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Occurs]),
    }
}

/// Fixture (#15 native slice 9, scalar root): a var bound at three roles of one
/// relation typed Var(A), Var(B), Int — driving unify(Var(A),Var(B)) then
/// unify(resolve(Var(A))=Var(B), Int), i.e. subst {A:Var(B), B:Int}, a genuine
/// 2-hop chain. solve::resolve chases A→Var(B)→Int; the native Bound/Resolved
/// closure must reproduce it. A Constraint (not Query) keeps Reflect::constraint
/// to just pattern() — no yields/result unify noise. Selfhost-only.
pub fn subst_chain_scalar_root() -> TypeFixture {
    let a = TyVar(9200);
    let b = TyVar(9201);
    let relation = QualIdent::from("Chain");
    let resolver = TableResolver::new().with_relation(RelationSchema {
        name: relation.clone(),
        roles: vec![
            (Ident::new("a"), Ty::Var(a)),
            (Ident::new("b"), Ty::Var(b)),
            (Ident::new("c"), Ty::Int(IntWidth::Int)),
        ],
        key: vec![],
        model_closed: true,
        derived: false,
    });
    let source = FrontendSource {
        functions: vec![],
        rules: vec![],
        constraints: vec![Constraint {
            name: Ident::new("Chain"),
            severity: Severity::Strict,
            body: Pattern::new(vec![Clause::Edge {
                bind: None,
                relation: relation.clone(),
                args: vec![
                    RoleArg {
                        role: Ident::new("a"),
                        arg: Arg::Var(Ident::new("x")),
                    },
                    RoleArg {
                        role: Ident::new("b"),
                        arg: Arg::Var(Ident::new("x")),
                    },
                    RoleArg {
                        role: Ident::new("c"),
                        arg: Arg::Var(Ident::new("x")),
                    },
                ],
            }]),
        }],
        queries: vec![],
    };
    TypeFixture {
        label: "subst_chain_scalar_root",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::new(),
    }
}

/// Fixture (#15 native slice 9, composite root): [`subst_chain_scalar_root`]'s
/// counterpart with a pre-tokenized ground composite root — role `c`'s type is
/// `Option<Int>` rather than plain `Int`, so the chase lands on A→Var(B)→
/// `Option<Int>` instead of a bare scalar, exercising a root that isn't itself
/// a leaf token. Distinct `TyVar`s (9210/9211) from the scalar fixture's
/// (9200/9201) so the two fixtures' facts never collide if ever exported
/// together. Selfhost-only.
pub fn subst_chain_composite_root() -> TypeFixture {
    let a = TyVar(9210);
    let b = TyVar(9211);
    let relation = QualIdent::from("Chain");
    let resolver = TableResolver::new().with_relation(RelationSchema {
        name: relation.clone(),
        roles: vec![
            (Ident::new("a"), Ty::Var(a)),
            (Ident::new("b"), Ty::Var(b)),
            (Ident::new("c"), Ty::option(Ty::Int(IntWidth::Int))),
        ],
        key: vec![],
        model_closed: true,
        derived: false,
    });
    let source = FrontendSource {
        functions: vec![],
        rules: vec![],
        constraints: vec![Constraint {
            name: Ident::new("Chain"),
            severity: Severity::Strict,
            body: Pattern::new(vec![Clause::Edge {
                bind: None,
                relation: relation.clone(),
                args: vec![
                    RoleArg {
                        role: Ident::new("a"),
                        arg: Arg::Var(Ident::new("x")),
                    },
                    RoleArg {
                        role: Ident::new("b"),
                        arg: Arg::Var(Ident::new("x")),
                    },
                    RoleArg {
                        role: Ident::new("c"),
                        arg: Arg::Var(Ident::new("x")),
                    },
                ],
            }]),
        }],
        queries: vec![],
    };
    TypeFixture {
        label: "subst_chain_composite_root",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::new(),
    }
}

/// Fixture (#15 gap D): a 2-hop var chain whose FIRST hop is bound by resolving
/// a user-defined overloaded call — the exact `self.subst = next` commit site
/// (reflect.rs:1719) gap D patches. `pick`'s only candidate is identity
/// `[Var(B)] -> Var(B)`; `pick(x)` with `x: Var(A)` unifies `Var(A) ~ Var(B)`,
/// binding `A := Var(B)` — a var-to-var edge pre-fix swallowed entirely (no
/// BindAttempt/SubstEdge for A). The query `result: Int` then forces the
/// ordinary second hop `B := Int` via the normal query-result unify (already
/// observed either way). Distinct TyVars (9400/9401) from every other fixture.
pub fn overload_bind_chain() -> TypeFixture {
    let o = Origins::new("OverloadChain");
    let a = TyVar(9400);
    let b = TyVar(9401);
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("OverloadChain"),
            params: vec![(Ident::new("x"), Ty::Var(a))],
            body: Pattern::default(),
            yields: o.call("pick", vec![o.var("x")]),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Int(IntWidth::Int),
            }])),
        }],
    };
    let resolver = TableResolver::new().with_function(FnSignature {
        name: QualIdent::from("pick"),
        params: vec![Ty::Var(b)],
        ret: Ty::Var(b),
        effects: EffectRow::empty(),
        is_aggregate: false,
        may_diverge: false,
    });
    TypeFixture {
        label: "overload_bind_chain",
        category: ConformanceCategory::TypeInference,
        source,
        resolver,
        expected_categories: BTreeSet::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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
        functions: Vec::new(),
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

/// Fixture (#15 native slice 8): `Probability` unified against `F64` is the
/// deliberately-kept v1 bridge (solve.rs:163) — Step::Done, NOT a conflict of
/// any kind. Proves the bridge is a non-event natively (zero MismatchConflict,
/// zero EpistemicErasureConflict). Distinct from probability_to_bool_erasure
/// (the OTHER "plain" partner, which IS an erasure). Selfhost-only (like
/// `occurs_check_row`), not added to [`all_type_fixtures`], to keep
/// `type_parity` unchanged.
pub fn probability_f64_bridge_is_not_a_conflict() -> TypeFixture {
    let o = Origins::new("Bridge");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Bridge"),
            params: vec![(Ident::new("p"), Ty::Probability)],
            body: Pattern::default(),
            yields: o.var("p"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::F64,
            }])),
        }],
    };
    TypeFixture {
        label: "probability_f64_bridge_is_not_a_conflict",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::new(),
    }
}

/// Fixture (#15 native slice 8): a genuine solve::step-driven flat Mismatch
/// reached through Query result-vs-yields row descent (unify_rows → leaf
/// unify(Bool, Int)), NOT role_arg's direct literal compare. query() wraps the
/// scalar `yields` as Rel<{value: Int}> (reflect.rs:1058) and unifies it
/// against `result` = Rel<{value: Bool}>, descending Rows into the leaf. This
/// is the path only UnifyMismatch reproduces (Lit/VarRoleMismatch never do).
/// Selfhost-only (like `occurs_check_row`), not added to
/// [`all_type_fixtures`], to keep `type_parity` unchanged.
pub fn plain_scalar_mismatch() -> TypeFixture {
    let o = Origins::new("PlainMismatch");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("PlainMismatch"),
            params: vec![],
            body: Pattern::default(),
            yields: o.lit(Ty::Int(IntWidth::Int), Lit::Int(1)),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Bool,
            }])),
        }],
    };
    TypeFixture {
        label: "plain_scalar_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// #15 gap-closure (slice 8 B+C, Gap B — container vs a DIFFERENT ctor): the
/// outer `Rel<{value: ..}>` wrapping (both sides) is the same ctor (Rel),
/// descending via `Step::Rows` into the `value` field, where `Result<Bool,
/// Str>` (ctor 9) meets `Option<Int>` (ctor 8) — two different container
/// ctors, neither of which `step` gives a Descend/Rows arm against the
/// other, so it flattens straight to `Mismatch`. Only `UnifyMismatchCrossCtor`
/// reproduces this (`UnifyMismatch` excludes both ctors via `TyCtorOrdinary`).
/// Selfhost-only (like `plain_scalar_mismatch`), not added to
/// [`all_type_fixtures`], to keep `type_parity` unchanged.
pub fn container_vs_container_mismatch() -> TypeFixture {
    let o = Origins::new("ContainerVsContainer");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("ContainerVsContainer"),
            params: vec![(Ident::new("x"), Ty::option(Ty::Int(IntWidth::Int)))],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Result(Box::new(Ty::Bool), Box::new(Ty::Str)),
            }])),
        }],
    };
    TypeFixture {
        label: "container_vs_container_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// #15 gap-closure (slice 8 B+C, Gap B — container vs plain): `Option<Int>`
/// (ctor 8) meets a bare `Int` (ctor 0, Plain) — different ctors, `step`
/// flattens to `Mismatch`. `UnifyMismatch` doesn't reach this either (`Option`
/// isn't in `TyCtorOrdinary`); only `UnifyMismatchCrossCtor` does. Selfhost-only,
/// not added to [`all_type_fixtures`].
pub fn container_vs_plain_mismatch() -> TypeFixture {
    let o = Origins::new("ContainerVsPlain");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("ContainerVsPlain"),
            params: vec![(Ident::new("x"), Ty::Int(IntWidth::Int))],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::option(Ty::Int(IntWidth::Int)),
            }])),
        }],
    };
    TypeFixture {
        label: "container_vs_plain_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// #15 gap-closure (slice 8 B+C, Gap B — epistemic vs container erasure):
/// `Estimate<Int>` (ctor 6) meets `Option<Int>` (ctor 8) — `is_plain`
/// (solve.rs:263) is TRUE for containers, so this is a genuine `Erasure`,
/// not a `Mismatch`. Only the amended `EstimateErasureFwd`/`Bwd` (now joining
/// the broadened `TyCtorPlain` set) reproduce this — the pre-gap-closure
/// rules, restricted to `TyCtorOrdinary`, silently missed it. Selfhost-only,
/// not added to [`all_type_fixtures`].
pub fn estimate_vs_container_erasure() -> TypeFixture {
    let o = Origins::new("EstimateVsContainer");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("EstimateVsContainer"),
            params: vec![(
                Ident::new("x"),
                Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            )],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::option(Ty::Int(IntWidth::Int)),
            }])),
        }],
    };
    TypeFixture {
        label: "estimate_vs_container_erasure",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::EpistemicErasure]),
    }
}

/// #15 gap-closure (slice 8 B+C, Gap C — cross-epistemic-wrapper mismatch):
/// `Estimate<Int>` (ctor 6) meets `Missing<Int>` (ctor 7) — two DIFFERENT
/// epistemic wrappers; `solve::epistemic_erasure` requires one side to be
/// `is_plain`, and neither is, so `step` flattens to an ordinary `Mismatch`,
/// never an erasure. Only `UnifyMismatchCrossCtor` reproduces this. Selfhost-only,
/// not added to [`all_type_fixtures`].
pub fn cross_epistemic_wrapper_mismatch() -> TypeFixture {
    let o = Origins::new("CrossEpistemic");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("CrossEpistemic"),
            params: vec![(
                Ident::new("x"),
                Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            )],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Missing(Box::new(Ty::Int(IntWidth::Int))),
            }])),
        }],
    };
    TypeFixture {
        label: "cross_epistemic_wrapper_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// #15 gap-closure (slice 8 B+C, third cell — Estimate vs Estimate): `step`
/// has NO `(Estimate, Estimate)` Descend arm (unlike `Option`/`Result`/
/// `Missing`), so `Estimate<Bool>` vs `Estimate<Int>` falls to the catch-all,
/// where `epistemic_erasure` returns `None` (neither side `is_plain`) —
/// an ordinary `Mismatch` at the container level, with no leaf descent at
/// all. Only the dedicated `EstimateSameCtorMismatch` rule reproduces this
/// (`UnifyMismatchCrossCtor`'s `lc != rc` guard structurally cannot fire on
/// a same-ctor pair). Selfhost-only, not added to [`all_type_fixtures`].
pub fn estimate_same_ctor_mismatch() -> TypeFixture {
    let o = Origins::new("EstimateSameCtor");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("EstimateSameCtor"),
            params: vec![(
                Ident::new("x"),
                Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            )],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::Estimate(Box::new(Ty::Bool)),
            }])),
        }],
    };
    TypeFixture {
        label: "estimate_same_ctor_mismatch",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Mismatch]),
    }
}

/// #15 gap-closure (slice 8 B+C, REGRESSION GUARD — the load-bearing `lc !=
/// rc` guard): `Option<Bool>` vs `Option<Int>` — SAME ctor (8) at the
/// container level, which `step`'s `Option`/`Option` arm `Descend`s into the
/// leaf `Bool` vs `Int` pair, the actual `Mismatch`. Proves
/// `UnifyMismatchCrossCtor` does NOT also fire at the container level (which
/// would double-count the conflict): the outer `Rel<{value:..}>` wrapping is
/// likewise same-ctor (Rel), excluded the same way. Exactly ONE
/// `MismatchConflict` must result, at the leaf pair, never the container
/// pair. Selfhost-only, not added to [`all_type_fixtures`].
pub fn same_container_leaf_no_double_count() -> TypeFixture {
    let o = Origins::new("SameContainerLeaf");
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("SameContainerLeaf"),
            params: vec![(Ident::new("x"), Ty::option(Ty::Int(IntWidth::Int)))],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::rel(Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty: Ty::option(Ty::Bool),
            }])),
        }],
    };
    TypeFixture {
        label: "same_container_leaf_no_double_count",
        category: ConformanceCategory::TypeInference,
        source,
        resolver: TableResolver::new(),
        expected_categories: BTreeSet::from([Category::Mismatch]),
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

/// The var-at-two-roles native-slice fixture (#15 slice 2 ruling, Fable
/// comment 5012408628, §5 "Confused" program): the var `count` is
/// role-bound at `Input.count` (`Int`, ordinal 0) then `Input.label`
/// (`String`, ordinal 1). `reflect.rs` records `RoleVar { ordinal: 0 }` and
/// `RoleVar { ordinal: 1 }`, exactly **one** `Fact::HasType` at the binding
/// (`count : Int`, the first occurrence), and exactly **one**
/// `ConflictKind::Mismatch { left: Int, right: Str }` — never two contradictory
/// `HasType`s and never a clean bill of health, the pre-slice-2 native bug
/// this fixture exists to catch. The head `Output(count: count)` unifies
/// `Int ~ Int` cleanly (the env holds `T0 = Int`), so this is the only
/// conflict.
pub const NATIVE_VAR_TWO_ROLES_MISMATCH_FIXTURE: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Confused: Output(count: count) from {
    Input(count: count, label: count)
}
"#;

/// The same-role-twice native-slice fixture (#15 slice 2 ruling follow-up
/// corpus): a var role-bound at the *same* role twice. Before slice 2, the
/// two `RoleVar` facts were byte-identical (no `ordinal`) and collapsed to
/// one `FactId` — this fixture proves the duplicate no longer collapses (2
/// distinct `RoleVar` rows, ordinals 0 and 1), that exactly **one**
/// `HasType` still results (both occurrences declare the same `Int` role
/// type, so `T0 == T1` and no conflict is raised), and — the sharper native
/// regression the ordinal-keyed `RoleVar` key exists to prevent — that the
/// export commits cleanly with **zero** `GroundKeyConflict`s (the two rows
/// are equal on `(subject, relation, role)` but differ on `ordinal`, which
/// is part of the key).
pub const NATIVE_VAR_SAME_ROLE_TWICE_FIXTURE: &str = r#"
package t @ 1.0.0

rel Pair { first: Int; second: Int } key(first)
rel Out { first: Int } key(first)

derive Doubled: Out(first: n) from {
    Pair(first: n, second: n)
}
"#;

/// The three-role native-slice fixture (#15 slice 2 ruling follow-up
/// corpus, §4 "multiplicity check"): a var role-bound at three roles with
/// declared types `Int, String, Bool` in traversal order. `reflect.rs`
/// records one `HasType` (`T0 = Int`) and exactly two oriented conflicts —
/// `Mismatch { left: Int, right: Str }` and `Mismatch { left: Int, right:
/// Bool }` — and, critically, **never** `Mismatch { left: Str, right: Bool
/// }`: later-vs-later pairs are structurally never derived, by either
/// checker.
pub const NATIVE_VAR_THREE_ROLES_FIXTURE: &str = r#"
package t @ 1.0.0

rel Triple { a: Int; b: String; c: Bool } key(a)
rel Out3 { a: Int } key(a)

derive Triplet: Out3(a: x) from {
    Triple(a: x, b: x, c: x)
}
"#;

/// The `when`-clause native-slice fixture (#15 native slice 3, RequiresBool):
/// a single `when` clause whose condition (`flag`) is well-typed `Bool`, so
/// `reflect.rs` records exactly one `Fact::RequiresBool` and zero `NonBool`
/// conflicts for it. `flag` is bound by the preceding `Input` clause, so its
/// type is known (`Bool`) by the time `Clause::When` types the guard — unlike
/// the ill-typed `when 1` guard `crates/brixc/tests/lower_units.rs`
/// (`non_bool_when_guard_is_one_targeted_error`) rejects, this one is a clean `Bool` condition
/// that raises no `NonBool` conflict. The guard is a variable rather than a
/// literal, so the fixture also exercises a `RoleVar`/`HasType` binding
/// alongside it. Exactly one `when` clause, so this produces exactly one
/// `Fact::RequiresBool`.
pub const NATIVE_WHEN_REQUIRES_BOOL_FIXTURE: &str = r#"
package t @ 1.0.0

rel Input { flag: Bool } key(flag)
rel Output { flag: Bool } key(flag)

derive Copy: Output(flag: flag) from {
    Input(flag: flag)
    when flag
}
"#;

/// The operator-application native-slice fixture (#15 native slice 4,
/// Applies): a single `x + 1` call in a `let` binding. `reflect.rs`'s
/// `Reflect::call` records `Fact::Applies { subject: Subject::Expr{origin},
/// operator: func.to_string(), scope: root }` for every call/operator node,
/// so this well-typed `Int + Int` produces the operator `Applies` fact(s) the
/// native `AppliesInRoot` rule mirrors. Kept to one visible operator call so
/// the derived count is small and stable; the test computes the exact
/// expected set from `reflect.rs` rather than hard-coding it.
pub const NATIVE_OPERATOR_APPLIES_FIXTURE: &str = r#"
package t @ 1.0.0

rel Input { x: Int } key(x)
rel Output { y: Int } key(y)

derive Compute: Output(y: y) from {
    Input(x: x)
    let y = x + 1
}
"#;

/// The non-Bool-guard native-slice fixture (#15 native slice 5, NonBool): a
/// `when n` guard whose bound variable `n` has the concrete type `Int` (from
/// the `Input` role binding), not `Bool` and not a type variable. `reflect.rs`
/// records `Fact::HasType { Subject::Expr(n), Int }` for the guard expr and
/// then, since `Int != Bool && !is_var(Int)`, a `ConflictKind::NonBool { found:
/// Int }`. The native `GuardNonBool` rule mirrors that conflict from the
/// imported `ExprType` + the `BoolType` singleton. Exactly one `when` guard, so
/// this produces exactly one NonBool conflict; the test computes the expected
/// set from `reflect.rs` rather than hard-coding it.
pub const NATIVE_GUARD_NON_BOOL_FIXTURE: &str = r#"
package t @ 1.0.0

rel Input { n: Int } key(n)
rel Output { n: Int } key(n)

derive Copy: Output(n: n) from {
    Input(n: n)
    when n
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
