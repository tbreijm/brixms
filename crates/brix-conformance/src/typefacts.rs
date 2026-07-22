//! Ingestion bridge from `brix_ir::reflect`'s structural facts into the
//! native `brix.type` shadow-mode package's Ground relations (#15 vertical
//! slice 1: "native `brix.type` vertical slice 1").
//!
//! `reflect::analyze` produces content-addressed, `Ty`/`Subject`-shaped
//! facts. The native package only ever runs an `engine::Store` — its rows
//! are `String -> engine::Value`, and this module never hands a
//! `Subject`/`Ty` to the engine directly: it flattens each to an **opaque
//! canonical token** (`Value::Str(hex(digest))`, the same content-addressing
//! style `FactId::derive`/`ScopeId::root` already use) and records a `token
//! -> (Subject|Ty|ScopeId)` side table ([`TokenTable`]) so the harness can
//! map a derived row's tokens back to the original values and call
//! `FactId::derive` itself — literal `FactId` set equality, no second
//! encoder (R1/R2 of the #15 slice-1 ruling).
//!
//! #15 native slice 2 (var-at-two-roles, Fable ruling comment 5012408628)
//! adds the package's first non-`Str` column: `Fact::RoleVar::ordinal` is
//! plain structure (a zero-based occurrence index), exported as
//! `Value::Int` directly rather than tokenized — R1 still holds, since rules
//! only ever match it against a literal (`ordinal: 0`) or copy it, never
//! construct or interpret one.
//!
//! Rules in `packages/brix.type/brix.type.brix` only ever *join on* and
//! *copy* these tokens (and the `ordinal` int); they never construct or
//! interpret one — that discipline lives entirely on this side of the
//! bridge.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Domain};
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::reflect::{Fact, ReflectiveReport, ScopeId, Subject};
use brix_ir::types::{Ty, TyVar};
use brix_rt::engine::{Row, TransactionOp, Value};

/// What one opaque token decodes back to (#15 slice-1 R2: "exporter records
/// a `token -> (Subject|Ty|ScopeId)` side table"). #15 native slice 6 adds
/// `Ident` — the first token an identifier round-trips through, needed because
/// `UnknownFieldConflict` surfaces the accessed `field` name in its output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenValue {
    Subject(Subject),
    Ty(Ty),
    Scope(ScopeId),
    Ident(Ident),
}

/// The `token -> origin value` side table an [`export`] run builds. The
/// harness uses this to map a native-derived row's tokens back to the
/// `Subject`/`Ty`/`ScopeId` the exporter started from.
#[derive(Clone, Debug, Default)]
pub struct TokenTable {
    entries: BTreeMap<String, TokenValue>,
}

impl TokenTable {
    pub fn subject(&self, token: &str) -> Option<&Subject> {
        match self.entries.get(token) {
            Some(TokenValue::Subject(subject)) => Some(subject),
            _ => None,
        }
    }

    pub fn ty(&self, token: &str) -> Option<&Ty> {
        match self.entries.get(token) {
            Some(TokenValue::Ty(ty)) => Some(ty),
            _ => None,
        }
    }

    pub fn scope(&self, token: &str) -> Option<ScopeId> {
        match self.entries.get(token) {
            Some(TokenValue::Scope(scope)) => Some(*scope),
            _ => None,
        }
    }

    pub fn ident(&self, token: &str) -> Option<&Ident> {
        match self.entries.get(token) {
            Some(TokenValue::Ident(ident)) => Some(ident),
            _ => None,
        }
    }

    fn record(&mut self, value: TokenValue) -> Value {
        let hex = match &value {
            TokenValue::Subject(subject) => digest_hex(subject),
            TokenValue::Ty(ty) => digest_hex(ty),
            TokenValue::Scope(scope) => digest_hex(scope),
            TokenValue::Ident(ident) => digest_hex(ident),
        };
        self.entries.entry(hex.clone()).or_insert(value);
        Value::Str(hex)
    }
}

/// A token with no reverse-mapping need: `RoleVar`/`RoleLit`/`SchemaRole`
/// join on `relation`/`role` tokens, but no derived relation in this slice
/// ever surfaces one in its output columns, so there is nothing for the
/// harness to map back (R2 only promises `Subject|Ty|ScopeId`).
fn opaque_token(value: &impl Canonical) -> Value {
    Value::Str(digest_hex(value))
}

fn digest_hex(value: &impl Canonical) -> String {
    let mut writer = CanonWriter::new();
    value.canon_write(&mut writer);
    writer.digest(Domain::Value).to_hex()
}

fn relation_token(relation: &QualIdent) -> Value {
    opaque_token(relation)
}

fn role_token(role: &Ident) -> Value {
    opaque_token(role)
}

/// Build one `Assert` op from a relation name and its fields, without
/// spelling out `Row(BTreeMap::from([...]))` at every call site.
fn assert_op(
    relation: &str,
    fields: impl IntoIterator<Item = (&'static str, Value)>,
) -> TransactionOp {
    TransactionOp::Assert {
        relation: relation.to_string(),
        row: Row(fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<BTreeMap<_, _>>()),
    }
}

/// #15 native slice 7 (Occurs): recursively emit `TyChild`/`TyRowChild`
/// structure edges for a `Ty`, memoized (`seen`) so each distinct sub-`Ty` is
/// decomposed — and its token recorded — exactly once per `export` run. The
/// descent set mirrors `solve::occurs` exactly (unary family
/// Option/List/Vector/Set/Bag/Estimate/Missing; binary Result/Map;
/// Record/Rel rows) — this function IS that same descent, just re-expressed
/// as edges a Datalog transitive closure can walk instead of a Rust
/// recursion a Rust function can walk. Scalars and `Ty::Var` leaves record
/// only their own token (via `tokens.record`, at the top) and emit no edges.
fn decompose_ty(
    ty: &Ty,
    tokens: &mut TokenTable,
    ops: &mut Vec<TransactionOp>,
    seen: &mut BTreeSet<String>,
) -> Value {
    let self_tok = tokens.record(TokenValue::Ty(ty.clone()));
    let Value::Str(hex) = self_tok.clone() else {
        unreachable!("TokenTable::record always returns Value::Str")
    };
    if !seen.insert(hex) {
        return self_tok;
    }
    match ty {
        Ty::Option(t)
        | Ty::List(t)
        | Ty::Vector(t)
        | Ty::Set(t)
        | Ty::Bag(t)
        | Ty::Estimate(t)
        | Ty::Missing(t) => {
            let child = decompose_ty(t, tokens, ops, seen);
            ops.push(assert_op(
                "TyChild",
                [
                    ("parent", self_tok.clone()),
                    ("ordinal", Value::Int(0)),
                    ("child", child),
                ],
            ));
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            let ca = decompose_ty(a, tokens, ops, seen);
            ops.push(assert_op(
                "TyChild",
                [
                    ("parent", self_tok.clone()),
                    ("ordinal", Value::Int(0)),
                    ("child", ca),
                ],
            ));
            let cb = decompose_ty(b, tokens, ops, seen);
            ops.push(assert_op(
                "TyChild",
                [
                    ("parent", self_tok.clone()),
                    ("ordinal", Value::Int(1)),
                    ("child", cb),
                ],
            ));
        }
        Ty::Record(row) | Ty::Rel(row) => {
            for field in &row.fields {
                let c = decompose_ty(&field.ty, tokens, ops, seen);
                ops.push(assert_op(
                    "TyRowChild",
                    [
                        ("parent", self_tok.clone()),
                        (
                            "field",
                            tokens.record(TokenValue::Ident(field.name.clone())),
                        ),
                        ("child", c),
                    ],
                ));
            }
        }
        // scalar / Var leaf — records its own token (above), no edges
        _ => {}
    }
    self_tok
}

/// #15 native slice 8 (`step` classification): the top-level constructor tag
/// of a `Ty`, exporter-side only — not `Canonical`, not a reflect `Fact`, the
/// same category as [`decompose_ty`]: a pure function of a token's shape. The
/// package joins on this `Int` (via `TyCtorIs`) rather than reproducing any
/// `Ty` shape matching of its own.
///
/// The set is exactly what `solve::step` distinguishes: `Var`/`Error`
/// recognize-and-exclude the `Bind`/absorb arms (a `step` pair with either
/// side a bare variable or `Error` never reaches `Mismatch`/`Erasure`);
/// `Probability`/`F64`/`Bool` participate in named pairings (the
/// `Probability`/`F64` bridge, the `Probability`/`Bool` erasure);
/// `Estimate`/`Missing` are the epistemic wrappers `step`'s `Erasure` arm
/// singles out; `Option`/`Result`/`Record`/`Rel` exist ONLY so the ordinary-
/// `Mismatch` rule (`UnifyMismatch`) can exclude them via `TyCtorOrdinary`
/// (see the deferred-gap note in `brix.type.brix`); everything else is
/// honestly `Plain` because `step` never singles it out.
#[repr(i64)]
#[derive(Clone, Copy)]
enum TyCtor {
    Plain = 0,
    Var = 1,
    Error = 2,
    Probability = 3,
    F64 = 4,
    Bool = 5,
    Estimate = 6,
    Missing = 7,
    Option = 8,
    Result = 9,
    Record = 10,
    Rel = 11,
}

fn ty_ctor(ty: &Ty) -> i64 {
    (match ty {
        Ty::Var(_) => TyCtor::Var,
        Ty::Error => TyCtor::Error,
        Ty::Probability => TyCtor::Probability,
        Ty::F64 => TyCtor::F64,
        Ty::Bool => TyCtor::Bool,
        Ty::Estimate(_) => TyCtor::Estimate,
        Ty::Missing(_) => TyCtor::Missing,
        Ty::Option(_) => TyCtor::Option,
        Ty::Result(_, _) => TyCtor::Result,
        Ty::Record(_) => TyCtor::Record,
        Ty::Rel(_) => TyCtor::Rel,
        _ => TyCtor::Plain,
    }) as i64
}

/// The exported transaction ops (Ground `Assert`s for `RoleVar`/`RoleLit`/
/// `SchemaRole`/`RootScope`) plus the token table needed to map derived rows
/// back to `Subject`/`Ty`/`ScopeId`.
#[derive(Clone, Debug, Default)]
pub struct Export {
    pub ops: Vec<TransactionOp>,
    pub tokens: TokenTable,
}

/// Flatten a [`ReflectiveReport`]'s relevant structural facts (`Fact::RoleVar`/
/// `Fact::RoleLit`/`Fact::SchemaRole`/`Fact::RequiresBool`/`Fact::Applies`/
/// `Fact::HasType`-on-`Expr`) into Ground `Assert` ops for
/// `packages/brix.type/brix.type.brix`, plus the `RootScope` and `BoolType`
/// singletons (R3: every derived judgment joins the root scope from day one).
///
/// `Fact::RequiresBool`/`Fact::Applies` are exported as `WhenCond`/`OpApply`
/// (#15 native slices 3–4): the native package re-derives them via a
/// `RootScope` join rather than importing `reflect.rs`'s own derived fact
/// directly, matching how `RoleVar`/`RoleLit` feed `HasType`/`MismatchConflict`.
/// `Applies`'s `operator` is a plain `func.to_string()`, not a
/// `Subject`/`Ty`/`ScopeId`, so it is asserted verbatim (`Value::Str`) rather
/// than tokenized — the rule only copies it, never interprets it (R1).
///
/// `Fact::HasType` on a `Subject::Expr` is exported as `ExprType` (#15 native
/// slice 5) — the first import of reflect's *post-inference* types, feeding the
/// `GuardNonBool` rule. `Ty::Var` rows are dropped on this bridge to reproduce
/// reflect's `!is_var` NonBool guard; `BoolType` seeds the `Ty::Bool` constant
/// the rule tests against.
///
/// `Fact::FieldAccess`/`Fact::RowField` are exported as the like-named inputs
/// (#15 native slice 6): `FieldNotInRow` flags an accessed `field` as unknown
/// by the *absence* of a matching `RowField` on the base (a `without` join) —
/// the package's first negation.
///
/// `Fact::BindAttempt` is exported as `BindAttempt` plus, via [`decompose_ty`],
/// the `TyChild`/`TyRowChild` structural decomposition of its (already
/// resolved) `target` — and of `Ty::Var(var)` itself, so the bound
/// variable's own leaf token lines up byte-for-byte with any occurrence of
/// that variable found while decomposing `target` (#15 native slice 7,
/// Occurs). The package's `TyReaches` transitive closure over those edges,
/// joined back against `BindAttempt.var`, is the structural occurs-check —
/// positive recursion, no substitution model needed, because `bind_ty`
/// always resolves `target` before recording the attempt.
///
/// `Fact::UnifyAttempt` is exported as `UnifyAttempt` (#15 native slice 8,
/// native `solve::step` classification) plus a per-token `TyCtorIs` row for
/// each of its two (already zonked) operands — the top-level constructor tag
/// [`ty_ctor`] computes, deduped through a *separate* `ctor_seen` set (NOT
/// the `seen` set `decompose_ty` uses: `UnifyAttempt` facts precede
/// `BindAttempt` facts in `report.facts`, so sharing `seen` would
/// pre-populate a bind-target's hex and make `decompose_ty` short-circuit
/// before emitting its `TyChild`/`TyRowChild` edges). `TyCtorOrdinary` is
/// also seeded here (alongside `RootScope`/`BoolType`) with the three ctors
/// eligible as ordinary-`Mismatch` operands.
///
/// Facts outside these slices' rules (`ExprKindIs`, `ExprChild`, `FnSig`,
/// `RowTail`, `DimTerm`) are not exported — the native package doesn't consume
/// them yet.
pub fn export(report: &ReflectiveReport) -> Export {
    let mut tokens = TokenTable::default();
    let mut ops = Vec::new();
    // #15 native slice 7: memoizes `decompose_ty`'s recursion across every
    // `BindAttempt` in one `export` run — a sub-`Ty` reachable from two
    // different bind attempts is decomposed (and its edges emitted) once.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // #15 native slice 8: memoizes `TyCtorIs` emission across every
    // `UnifyAttempt` operand in one `export` run. Deliberately separate from
    // `seen` above — see the `export` doc comment for why sharing it would
    // corrupt `decompose_ty`'s occurs-check edges.
    let mut ctor_seen: BTreeSet<String> = BTreeSet::new();

    for derivation in &report.facts {
        match &derivation.fact {
            Fact::RoleVar {
                subject,
                relation,
                role,
                ordinal,
            } => {
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("relation".to_string(), relation_token(relation)),
                    ("role".to_string(), role_token(role)),
                    // #15 native slice 2: the package's first non-`Str`
                    // column. R1 (opaque tokens) is intact — an ordinal is
                    // plain structure the exporter asserts, matched against
                    // a literal by the package's rules, never a
                    // constructed/interpreted token.
                    ("ordinal".to_string(), Value::Int(*ordinal as i64)),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "RoleVar".to_string(),
                    row,
                });
            }
            Fact::RoleLit {
                subject,
                relation,
                role,
                ty,
            } => {
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("relation".to_string(), relation_token(relation)),
                    ("role".to_string(), role_token(role)),
                    ("ty".to_string(), tokens.record(TokenValue::Ty(ty.clone()))),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "RoleLit".to_string(),
                    row,
                });
            }
            Fact::SchemaRole { relation, role, ty } => {
                let row = Row(BTreeMap::from([
                    ("relation".to_string(), relation_token(relation)),
                    ("role".to_string(), role_token(role)),
                    ("ty".to_string(), tokens.record(TokenValue::Ty(ty.clone()))),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "SchemaRole".to_string(),
                    row,
                });
            }
            Fact::RequiresBool { subject, scope: _ } => {
                // `scope` is always root — already seeded below by the
                // `RootScope` singleton, so only `subject` needs a row here.
                let row = Row(BTreeMap::from([(
                    "subject".to_string(),
                    tokens.record(TokenValue::Subject(subject.clone())),
                )]));
                ops.push(TransactionOp::Assert {
                    relation: "WhenCond".to_string(),
                    row,
                });
            }
            Fact::Applies {
                subject,
                operator,
                scope: _,
            } => {
                // `scope` is always root (seeded below). `operator` is a plain
                // string, asserted verbatim rather than tokenized — the rule
                // copies it, never interprets it (R1).
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("operator".to_string(), Value::Str(operator.clone())),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "OpApply".to_string(),
                    row,
                });
            }
            // #15 native slice 5: import `reflect.rs`'s post-inference type of
            // an *expression* subject (`expr()` records `HasType{Expr, ty}` for
            // every typed expr) as the `ExprType` input the `GuardNonBool` rule
            // joins. `Ty::Var` rows are dropped here, faithfully reproducing
            // reflect's `!matches!(ty, Ty::Var(_))` NonBool guard on the bridge
            // so the package needs no native type-variable detection. Only
            // `Subject::Expr` HasTypes are relevant (a `when` guard is one);
            // `Binding`/`Head` HasTypes are re-derived natively elsewhere.
            Fact::HasType {
                subject: subject @ Subject::Expr { .. },
                ty,
                scope: _,
            } if !matches!(ty, Ty::Var(_)) => {
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("ty".to_string(), tokens.record(TokenValue::Ty(ty.clone()))),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "ExprType".to_string(),
                    row,
                });
            }
            // #15 native slice 6: the field-access site. `subject`/`field` are
            // reverse-mappable tokens (both surface in the derived
            // `UnknownFieldConflict`); `base` is opaque — it only joins against
            // `RowField.subject`, never surfaced. `field` is recorded as an
            // `Ident` token, the same digest a `RowField.field` gets, so the
            // negation join lines up.
            Fact::FieldAccess {
                subject,
                base,
                field,
            } => {
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("base".to_string(), opaque_token(base)),
                    (
                        "field".to_string(),
                        tokens.record(TokenValue::Ident(field.clone())),
                    ),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "FieldAccess".to_string(),
                    row,
                });
            }
            // #15 native slice 6: the base's resolved row membership. Only
            // `(subject, field)` matters for the UnknownField negation, so `ty`
            // is dropped; both columns are opaque (neither is surfaced — the
            // rule only uses this relation under `without`).
            Fact::RowField {
                subject,
                field,
                ty: _,
            } => {
                let row = Row(BTreeMap::from([
                    ("subject".to_string(), opaque_token(subject)),
                    ("field".to_string(), opaque_token(field)),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "RowField".to_string(),
                    row,
                });
            }
            // #15 native slice 7 (Occurs): the variable-binding attempt
            // itself, plus its structural decomposition. `var_tok` is a pure
            // leaf lookup (`Ty::Var(*var)` never has edges) but MUST go
            // through `decompose_ty` rather than a bare `opaque_token`/
            // `tokens.record` call — it has to be the exact same token
            // `decompose_ty(target, ...)` would produce if it ever reached
            // that same `Ty::Var(*var)` node while descending `target`, and
            // `decompose_ty` is the one place that mapping is defined.
            Fact::BindAttempt {
                subject,
                var,
                target,
            } => {
                let var_tok = decompose_ty(&Ty::Var(*var), &mut tokens, &mut ops, &mut seen);
                let target_tok = decompose_ty(target, &mut tokens, &mut ops, &mut seen);
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("var".to_string(), var_tok),
                    ("target".to_string(), target_tok),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "BindAttempt".to_string(),
                    row,
                });
            }
            // #15 native slice 8 (`step` classification): the unification
            // attempt itself, plus a `TyCtorIs` row per distinct operand
            // token — the native package's only handle on a `Ty`'s
            // top-level shape (it never descends the `Ty` directly; reflect's
            // own `unify` recursion already reaches every leaf, see
            // `brix.type.brix`'s slice-8 block comment).
            Fact::UnifyAttempt {
                subject,
                expect,
                found,
            } => {
                let expect_tok = tokens.record(TokenValue::Ty(expect.clone()));
                let found_tok = tokens.record(TokenValue::Ty(found.clone()));
                for (tok, ty) in [(&expect_tok, expect), (&found_tok, found)] {
                    let Value::Str(hex) = tok else {
                        unreachable!("TokenTable::record always returns Value::Str")
                    };
                    if ctor_seen.insert(hex.clone()) {
                        ops.push(assert_op(
                            "TyCtorIs",
                            [("ty", tok.clone()), ("ctor", Value::Int(ty_ctor(ty)))],
                        ));
                    }
                }
                let row = Row(BTreeMap::from([
                    (
                        "subject".to_string(),
                        tokens.record(TokenValue::Subject(subject.clone())),
                    ),
                    ("expect".to_string(), expect_tok),
                    ("found".to_string(), found_tok),
                ]));
                ops.push(TransactionOp::Assert {
                    relation: "UnifyAttempt".to_string(),
                    row,
                });
            }
            _ => {}
        }
    }

    let scope_token = tokens.record(TokenValue::Scope(ScopeId::root()));
    ops.push(TransactionOp::Assert {
        relation: "RootScope".to_string(),
        row: Row(BTreeMap::from([("scope".to_string(), scope_token)])),
    });

    // The `Ty::Bool` constant `GuardNonBool` tests `found != Bool` against,
    // seeded as a singleton exactly like `RootScope` (#15 native slice 5).
    let bool_token = tokens.record(TokenValue::Ty(Ty::Bool));
    ops.push(TransactionOp::Assert {
        relation: "BoolType".to_string(),
        row: Row(BTreeMap::from([("ty".to_string(), bool_token)])),
    });

    // The three ctors eligible as ordinary-`Mismatch` operands / erasure
    // targets, seeded as singletons exactly like `RootScope`/`BoolType`
    // (#15 native slice 8).
    for ctor in [TyCtor::Plain, TyCtor::F64, TyCtor::Bool] {
        ops.push(assert_op(
            "TyCtorOrdinary",
            [("ctor", Value::Int(ctor as i64))],
        ));
    }

    Export { ops, tokens }
}

/// Read a `Value::Str` token out of a derived row's role, if present.
fn token_str<'a>(row: &'a Row, role: &str) -> Option<&'a str> {
    match row.get(role) {
        Some(Value::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Map one derived `HasType` row (`subject`/`ty`/`scope` tokens) back to the
/// `Fact::HasType` it represents, via `tokens`. `None` if a token is missing
/// from the row or doesn't decode against the table — a fixture bug (the
/// exporter always records both sides of a token it hands to the engine),
/// so the harness should treat `None` as a hard failure, not a soft skip.
pub fn resolve_has_type(tokens: &TokenTable, row: &Row) -> Option<Fact> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let ty = tokens.ty(token_str(row, "ty")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(Fact::HasType { subject, ty, scope })
}

/// Map one derived `RequiresBool` row (`subject`/`scope` tokens) back to the
/// `Fact::RequiresBool` it represents, via `tokens` (#15 native slice 3) —
/// the `RequiresBool` counterpart of [`resolve_has_type`]. `None` if a token
/// is missing from the row or doesn't decode against the table.
pub fn resolve_requires_bool(tokens: &TokenTable, row: &Row) -> Option<Fact> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(Fact::RequiresBool { subject, scope })
}

/// Map one derived `Applies` row back to the `Fact::Applies` it represents
/// (#15 native slice 4). `subject`/`scope` decode through the token table;
/// `operator` is read back verbatim — it was never a token — matching how
/// the exporter asserted it. `None` if a token is missing or doesn't decode.
pub fn resolve_applies(tokens: &TokenTable, row: &Row) -> Option<Fact> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let operator = token_str(row, "operator")?.to_string();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(Fact::Applies {
        subject,
        operator,
        scope,
    })
}

/// The resolved shape of one derived `MismatchConflict` row — deliberately
/// not a `reflect::Fact`/`TypeConflict` by itself (there is no `because` set
/// to reconstruct; conflicts aren't content-addressed). The harness builds a
/// comparable `TypeConflict` from this and `reflect::write_conflict`s both
/// sides.
pub struct ResolvedMismatch {
    pub subject: Subject,
    pub expect: Ty,
    pub found: Ty,
    pub scope: ScopeId,
}

pub fn resolve_mismatch(tokens: &TokenTable, row: &Row) -> Option<ResolvedMismatch> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let expect = tokens.ty(token_str(row, "expect")?)?.clone();
    let found = tokens.ty(token_str(row, "found")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(ResolvedMismatch {
        subject,
        expect,
        found,
        scope,
    })
}

/// The resolved shape of one derived `NonBoolConflict` row (#15 native slice
/// 5) — like [`ResolvedMismatch`], not a `Fact`/`TypeConflict` by itself; the
/// harness builds a comparable `ConflictKind::NonBool` `TypeConflict` from it.
pub struct ResolvedNonBool {
    pub subject: Subject,
    pub found: Ty,
    pub scope: ScopeId,
}

pub fn resolve_non_bool(tokens: &TokenTable, row: &Row) -> Option<ResolvedNonBool> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let found = tokens.ty(token_str(row, "found")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(ResolvedNonBool {
        subject,
        found,
        scope,
    })
}

/// The resolved shape of one derived `UnknownFieldConflict` row (#15 native
/// slice 6) — like [`ResolvedMismatch`], not a `Fact`/`TypeConflict` by
/// itself; the harness builds a comparable `ConflictKind::UnknownField`
/// `TypeConflict` from it. `field` decodes through the token table's new
/// `Ident` mapping.
pub struct ResolvedUnknownField {
    pub subject: Subject,
    pub field: Ident,
    pub scope: ScopeId,
}

pub fn resolve_unknown_field(tokens: &TokenTable, row: &Row) -> Option<ResolvedUnknownField> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let field = tokens.ident(token_str(row, "field")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(ResolvedUnknownField {
        subject,
        field,
        scope,
    })
}

/// The resolved shape of one derived `OccursConflict` row (#15 native slice
/// 7) — like [`ResolvedMismatch`], not a `Fact`/`TypeConflict` by itself; the
/// harness builds a comparable `ConflictKind::Occurs` `TypeConflict` from it.
/// `var` decodes through the token table's `Ty` mapping and is then
/// unwrapped from its `Ty::Var(_)` wrapper back to a bare `TyVar` — the
/// wrapper is only there so `var`'s token matches `into`'s decomposition
/// leaf-for-leaf (see [`decompose_ty`]).
pub struct ResolvedOccurs {
    pub subject: Subject,
    pub var: TyVar,
    pub into: Ty,
    pub scope: ScopeId,
}

pub fn resolve_occurs(tokens: &TokenTable, row: &Row) -> Option<ResolvedOccurs> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let var = match tokens.ty(token_str(row, "var")?)? {
        Ty::Var(v) => *v,
        _ => return None,
    };
    let into = tokens.ty(token_str(row, "into")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(ResolvedOccurs {
        subject,
        var,
        into,
        scope,
    })
}

/// The resolved shape of one derived `EpistemicErasureConflict` row (#15
/// native slice 8) — like [`ResolvedMismatch`], not a `Fact`/`TypeConflict`
/// by itself; the harness builds a comparable `ConflictKind::EpistemicErasure`
/// `TypeConflict` from it.
pub struct ResolvedErasure {
    pub subject: Subject,
    pub from: Ty,
    pub to: Ty,
    pub scope: ScopeId,
}

pub fn resolve_epistemic_erasure(tokens: &TokenTable, row: &Row) -> Option<ResolvedErasure> {
    let subject = tokens.subject(token_str(row, "subject")?)?.clone();
    let from = tokens.ty(token_str(row, "from")?)?.clone();
    let to = tokens.ty(token_str(row, "to")?)?.clone();
    let scope = tokens.scope(token_str(row, "scope")?)?;
    Some(ResolvedErasure {
        subject,
        from,
        to,
        scope,
    })
}
