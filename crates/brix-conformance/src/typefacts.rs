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

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical, Domain};
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::reflect::{Fact, ReflectiveReport, ScopeId, Subject};
use brix_ir::types::Ty;
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
/// Facts outside these slices' rules (`ExprKindIs`, `ExprChild`, `FnSig`,
/// `RowTail`, `DimTerm`) are not exported — the native package doesn't consume
/// them yet.
pub fn export(report: &ReflectiveReport) -> Export {
    let mut tokens = TokenTable::default();
    let mut ops = Vec::new();

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
