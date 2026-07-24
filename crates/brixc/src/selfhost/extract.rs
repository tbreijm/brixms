//! Reflect-independent syntactic fact extraction — the north-star's
//! independence path (retire `brix_ir::reflect`).
//!
//! Today the native `brix.type` package's Ground inputs are produced by
//! [`super::typefacts::export`], which reads a [`brix_ir::reflect`]
//! `ReflectiveReport` — so the package is a *second observer* of reflect, not
//! an independent checker. This module begins decoupling that: it walks a
//! lowered [`FrontendSource`] (plus its schema resolver) **directly** and emits
//! the same Ground `Assert`s the exporter would, for the fact families that are
//! pure program reads — no inference, no substitution, no `reflect::analyze`
//! call.
//!
//! Fact families split into two groups (see the trajectory notes on #15):
//!
//! - **(a) syntactic** — `RoleVar`/`RoleLit`/`SchemaRole` (this slice), and
//!   later `OpApply`/`WhenCond`/`FieldAccess`. Derivable from a walk of the
//!   pattern language + schema lookups; this is exactly what `reflect`'s
//!   `pattern`/`role_arg` do, minus the type-inference bookkeeping.
//! - **(b) inference** — resolved `HasType`, `UnifyAttempt`, `BindAttempt`,
//!   `SubstEdge`, overload argmax. These require the package to run
//!   Hindley-Milner itself and still come from `reflect` until the native HM
//!   driver lands.
//!
//! For the **role-binding fragment** the (b) group is empty — the role
//! fixtures are edge clauses with literal/variable role args and no exprs, so
//! this extractor produces the *complete* fact set reflect does, and the
//! package derives byte-identical `RoleVar`/`RoleLit`/`SchemaRole`/`HasType`/
//! `MismatchConflict` extents from it. That equivalence is the independence
//! proof (`crates/brix-conformance/tests/selfhost_independence.rs`).
//!
//! Tokenization is shared with the exporter (same [`TokenTable`], same
//! `relation_token`/`role_token`/`Ty` digests), so a fact this module emits is
//! byte-for-byte the fact the exporter emits — the difference is only *where
//! the fact came from* (a syntactic walk vs a reflect report).

use std::collections::BTreeMap;

use brix_ir::core::{Constraint, Head, Query, Rule};
use brix_ir::frontend::{FrontendSource, SchemaResolver};
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::pattern::{Arg, Clause, Pattern, RoleArg};
use brix_ir::reflect::{lit_ty, Subject};
use brix_ir::types::Ty;
use brix_rt::engine::TransactionOp;

use super::typefacts::{
    assert_op, relation_token, role_token, seed_singletons, Export, TokenTable, TokenValue,
};

/// The reflect-free counterpart of [`super::typefacts::export`] for the
/// role-binding fragment: extract the role facts syntactically, add the same
/// singleton seeds the exporter does, and return them in the same [`Export`]
/// shape (ops + token side table) the selfhost harness settles. On the role
/// fixtures this transaction is complete — the package derives the identical
/// `HasType`/`MismatchConflict` extents it derives from `export`, with no
/// `reflect::analyze` in the pipeline.
pub fn extract_role(source: &FrontendSource, resolver: &impl SchemaResolver) -> Export {
    let mut tokens = TokenTable::default();
    let mut ops = extract_role_facts(source, resolver, &mut tokens);
    seed_singletons(&mut tokens, &mut ops);
    Export { ops, tokens }
}

/// Extract the role-binding-fragment Ground facts (`SchemaRole`, `RoleVar`,
/// `RoleLit`) from `source` **without** running `reflect::analyze`, tokenizing
/// through the shared `tokens` table so the asserts are byte-identical to the
/// exporter's. The declaration traversal order (rules, then constraints, then
/// queries; clauses in order; nested patterns recursed) matches `reflect`
/// exactly, so the per-`(declaration, variable)` `RoleVar.ordinal` counter
/// lands the same value at every site.
pub fn extract_role_facts(
    source: &FrontendSource,
    resolver: &impl SchemaResolver,
    tokens: &mut TokenTable,
) -> Vec<TransactionOp> {
    let mut ex = RoleExtractor {
        resolver,
        tokens,
        ops: Vec::new(),
        role_ordinals: BTreeMap::new(),
    };
    for rule in &source.rules {
        ex.declaration(&rule_name(rule), &rule.body);
        ex.head(&rule.head);
    }
    for constraint in &source.constraints {
        ex.declaration(&constraint_name(constraint), &constraint.body);
    }
    for query in &source.queries {
        ex.declaration(&query_name(query), &query.body);
    }
    ex.ops
}

fn rule_name(rule: &Rule) -> Ident {
    rule.name.clone()
}

fn constraint_name(constraint: &Constraint) -> Ident {
    constraint.name.clone()
}

fn query_name(query: &Query) -> Ident {
    query.name.clone()
}

struct RoleExtractor<'a, R: SchemaResolver> {
    resolver: &'a R,
    tokens: &'a mut TokenTable,
    ops: Vec<TransactionOp>,
    /// Per-`(declaration, variable)` occurrence counter — the exact `RoleVar`
    /// ordinal source `reflect::Reflect::role_ordinals` maintains. `declaration`
    /// in the key makes it a fresh count per declaration (no two declarations
    /// share an `Ident`), matching reflect.
    role_ordinals: BTreeMap<(Ident, Ident), u32>,
}

impl<R: SchemaResolver> RoleExtractor<'_, R> {
    fn declaration(&mut self, declaration: &Ident, body: &Pattern) {
        self.pattern(declaration, body);
    }

    /// Mirror `reflect::Reflect::pattern`, restricted to the clauses that
    /// produce role-fragment facts. `Let`/`When` carry only exprs (no role
    /// args, and they never touch the ordinal counter), so they are skipped;
    /// the grouping clauses are recursed exactly as reflect recurses them so
    /// the visitation order — and hence the ordinals — is identical.
    fn pattern(&mut self, declaration: &Ident, pattern: &Pattern) {
        for clause in &pattern.clauses {
            match clause {
                Clause::Edge { relation, args, .. } | Clause::History { relation, args, .. } => {
                    self.edge(declaration, relation, args)
                }
                Clause::Entity { entity, fields, .. } => {
                    let relation = QualIdent::simple(entity.as_str());
                    self.edge(declaration, &relation, fields);
                }
                Clause::Any(cases) => {
                    for case in cases {
                        self.pattern(declaration, case);
                    }
                }
                Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                    self.pattern(declaration, p)
                }
                Clause::Let { .. } | Clause::When(_) => {}
            }
        }
    }

    /// One edge/entity clause: for every role arg whose role the schema
    /// declares, emit `SchemaRole` (the declared role type) and then the
    /// per-arg `RoleVar`/`RoleLit`. Exactly `reflect`'s Edge/Entity arm minus
    /// the `bind`/`env` inference bookkeeping.
    fn edge(&mut self, declaration: &Ident, relation: &QualIdent, args: &[RoleArg]) {
        let Some(schema) = self.resolver.relation(relation) else {
            return;
        };
        for arg in args {
            let Some((_, ty)) = schema.roles.iter().find(|(name, _)| name == &arg.role) else {
                continue;
            };
            let ty = ty.clone();
            self.schema_role(relation, &arg.role, &ty);
            self.role_arg(declaration, relation, arg);
        }
    }

    /// Mirror `reflect::Reflect::head`'s *fact* emission for a rule head:
    /// each `Head::Tuple` arg whose role the schema declares contributes a
    /// `SchemaRole` (the declared role type). Reflect additionally emits a
    /// `HasType{Head}` and unifies the head arg — both inference bookkeeping
    /// with no Ground-fact footprint (head `HasType` is on a `Subject::Head`,
    /// which the exporter does not export), so the extractor omits them. Only
    /// `Head::Tuple` carries schema roles (matching reflect's early return).
    fn head(&mut self, head: &Head) {
        let Head::Tuple { relation, args } = head else {
            return;
        };
        let Some(schema) = self.resolver.relation(relation) else {
            return;
        };
        for arg in args {
            if let Some((_, ty)) = schema.roles.iter().find(|(name, _)| name == &arg.role) {
                let ty = ty.clone();
                self.schema_role(relation, &arg.role, &ty);
            }
        }
    }

    fn schema_role(&mut self, relation: &QualIdent, role: &Ident, ty: &Ty) {
        let ty_tok = self.tokens.record(TokenValue::Ty(ty.clone()));
        self.ops.push(assert_op(
            "SchemaRole",
            [
                ("relation", relation_token(relation)),
                ("role", role_token(role)),
                ("ty", ty_tok),
            ],
        ));
    }

    fn role_arg(&mut self, declaration: &Ident, relation: &QualIdent, arg: &RoleArg) {
        match &arg.arg {
            Arg::Var(name) => {
                let subject = Subject::Binding {
                    declaration: declaration.clone(),
                    name: name.clone(),
                };
                let counter = self
                    .role_ordinals
                    .entry((declaration.clone(), name.clone()))
                    .or_insert(0);
                let ordinal = *counter;
                *counter += 1;
                let subject_tok = self.tokens.record(TokenValue::Subject(subject));
                self.ops.push(assert_op(
                    "RoleVar",
                    [
                        ("subject", subject_tok),
                        ("relation", relation_token(relation)),
                        ("role", role_token(&arg.role)),
                        ("ordinal", brix_rt::engine::Value::Int(ordinal as i64)),
                    ],
                ));
            }
            Arg::Lit(lit) => {
                // reflect keys the literal subject by the ROLE name, not a
                // variable name (`role_arg`'s `Arg::Lit` arm) — a literal has no
                // binding identifier of its own.
                let subject = Subject::Binding {
                    declaration: declaration.clone(),
                    name: arg.role.clone(),
                };
                let found = lit_ty(lit);
                let subject_tok = self.tokens.record(TokenValue::Subject(subject));
                let ty_tok = self.tokens.record(TokenValue::Ty(found));
                self.ops.push(assert_op(
                    "RoleLit",
                    [
                        ("subject", subject_tok),
                        ("relation", relation_token(relation)),
                        ("role", role_token(&arg.role)),
                        ("ty", ty_tok),
                    ],
                ));
            }
        }
    }
}
