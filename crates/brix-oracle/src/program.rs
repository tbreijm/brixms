//! The IR-facing interface: the minimal typed IR the oracle evaluator
//! consumes.
//!
//! `brix-ir` (Core IR) is being built in a sibling lane and is not on this
//! branch. This module is the documented seam: it defines relations,
//! rules-as-patterns, and constraints in a form small enough to hand-build
//! directly in Rust for tests, and general enough that a later `brix-ir ->
//! Program` lowering is a thin, mostly-mechanical adapter rather than a
//! redesign. Phase assignment is **not** stored on `Program` — it is
//! computed by [`crate::phase::infer_phases`] following Appendix F, either
//! from this module's rule bodies (what the oracle does today, so it proves
//! itself standalone) or, once brix-phase lands, handed in precomputed and
//! passed straight to [`crate::eval::settle`].
//!
//! Deliberately out of scope for this pass (documented cuts, not silent
//! omissions — see the crate's top-level report): `any`/`exists`/`optional`/
//! `path`/`cross` clause forms, the full expression/type language, and
//! protocol lifecycle (Appendix H). The clause forms implemented are exactly
//! the ones the kernel-semantics checklist requires: plain edge reads,
//! `without`, `when`, `let` (with total and partial/`?` calls and the two
//! stock aggregates `count`/`sum`), and `history`.

use std::collections::BTreeMap;

use crate::row::RoleName;
use crate::value::Value;

pub type RelName = String;
pub type RuleId = String;
pub type ConstraintId = String;
pub type Var = String;
pub type FnName = String;

/// Per-kind conflict behavior differs (Part III §8); the oracle needs the
/// kind on every relation to know which rule applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelKind {
    /// `entity Name { key k: T; ... }` — a keyed unary relation bearing
    /// identity; `NodeId` is a hash of the key fields only.
    Entity,
    /// `rel Name { ... } key(...)` — ground n-ary relation. Conflicting
    /// key + differing row is a **transaction** conflict (Part III §8).
    Ground,
    /// `state rel` — at most one live version per key; `set` supersedes.
    State,
    /// `event rel` — immutable identity; reassert-with-same-content is
    /// idempotent, reassert-with-different-content fails the transaction.
    Event,
    /// A relation produced only by `derive` rules. Conflicting derived
    /// tuples under one key become a sealed `KeyConflict` (Part III §8).
    Derived,
}

/// A relation declaration — the oracle's reduced form of Part IV §1.
#[derive(Clone, Debug)]
pub struct RelationDef {
    pub name: RelName,
    pub kind: RelKind,
    /// Declaration order of *all* roles (used for `Entity` NodeId hashing,
    /// where Appendix G specifies key fields in declaration order).
    pub roles: Vec<RoleName>,
    /// The `key(...)` roles, declaration order.
    pub key: Vec<RoleName>,
    /// `open rel` (Part III §7): absence-sensitive reads need a witness.
    /// The oracle tracks the flag but does not implement `Complete`
    /// witness checking — programs that use `without`/`optional` over an
    /// `open` relation are accepted unconditionally here; that check
    /// belongs to static semantics (brix-ir, Appendix E), not settlement.
    pub open: bool,
}

impl RelationDef {
    pub fn ground(name: impl Into<RelName>, roles: &[&str], key: &[&str]) -> Self {
        RelationDef {
            name: name.into(),
            kind: RelKind::Ground,
            roles: roles.iter().map(|s| s.to_string()).collect(),
            key: key.iter().map(|s| s.to_string()).collect(),
            open: false,
        }
    }
    pub fn state(name: impl Into<RelName>, roles: &[&str], key: &[&str]) -> Self {
        RelationDef {
            kind: RelKind::State,
            ..Self::ground(name, roles, key)
        }
    }
    pub fn event(name: impl Into<RelName>, roles: &[&str], key: &[&str]) -> Self {
        RelationDef {
            kind: RelKind::Event,
            ..Self::ground(name, roles, key)
        }
    }
    pub fn entity(name: impl Into<RelName>, roles: &[&str], key: &[&str]) -> Self {
        RelationDef {
            kind: RelKind::Entity,
            ..Self::ground(name, roles, key)
        }
    }
    pub fn derived(name: impl Into<RelName>, roles: &[&str], key: &[&str]) -> Self {
        RelationDef {
            kind: RelKind::Derived,
            ..Self::ground(name, roles, key)
        }
    }
    pub fn open(mut self) -> Self {
        self.open = true;
        self
    }
}

/// A pattern term: a bound rule variable, or a literal constant.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Term {
    Var(Var),
    Const(Value),
}

/// One clause of a rule/constraint body.
#[derive(Clone, Debug)]
pub enum Clause {
    /// `R(role: x, ...)` / `e @ R(role: x, ...)` / `x: Entity { ... }`
    /// (Part IV §3) — the oracle unifies edge and entity-attribute clauses:
    /// both bind an identity reference to a variable (`bind_id`) — an
    /// `EdgeRef` for ordinary relations (`e @ R(...)`), a `NodeRef` for
    /// `Entity`-kind relations (`x: Entity { ... }`, where `x` denotes the
    /// entity itself, not a field) — plus ordinary field/role constraints
    /// in `args`.
    Edge {
        rel: RelName,
        bind_id: Option<Var>,
        args: Vec<(RoleName, Term)>,
    },
    /// `without { ... }` (Part IV §3) — stratified negation. Only `Edge`
    /// clauses are supported inside a `without` block in this pass.
    Without(Vec<Clause>),
    /// `history R(...)` (Part IV §3) — bypasses masks and supersession.
    /// Scoped in this pass to `Ground`/`State`/`Event` relations, reading
    /// every version the store ever recorded for them (see
    /// `crate::store::Store`'s ground history log); `Derived` relations do
    /// not retain cross-revision history in the oracle (Part III §2:
    /// "derived caches may be discarded and rebuilt").
    History {
        rel: RelName,
        args: Vec<(RoleName, Term)>,
    },
    /// `when boolExpr`.
    When(Expr),
    /// `let v = expr` — `expr` may be `Expr::Try(...)`, producing a
    /// `RuleError` edge on failure (Part III §9) instead of aborting the
    /// whole rule; only the current match's continuation stops.
    Let(Var, Expr),
}

/// The minimal expression language `let`/`when` bodies use.
#[derive(Clone, Debug)]
pub enum Expr {
    Var(Var),
    Const(Value),
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    /// A registered total, pure function call (`Program::fns`).
    Call(FnName, Vec<Expr>),
    /// A registered partial, pure function call (`Program::partial_fns`);
    /// only legal directly under `Clause::Let`'s `?`. `Ok`/`Err` become
    /// (bound value) / (RuleError-and-stop-this-match), matching Part III
    /// §9's `Err(e)? / None?` semantics — `None` is modeled as the
    /// function returning `Err(Value::Unit)`, canonicalized in the error
    /// edge as `MissingValue` by the evaluator.
    Try(FnName, Vec<Expr>),
    /// `count(from { ... })` (Part IV §4) — a stock aggregate over a
    /// sub-pattern read as a complete-read (strict phase dependency on
    /// every relation the sub-pattern touches, Appendix F #2).
    Count(Vec<Clause>),
    /// `sum(from { ... } yield expr)`.
    Sum(Vec<Clause>, Box<Expr>),
    /// `if cond { then } else { els }` — needed to evaluate function bodies
    /// compiled from source (issue #47; the flagship's `surcharge` is one `if`).
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    /// `let name = value in body` — a compiled function block's binding
    /// (issue #47 Slice 2).
    Let {
        name: Var,
        value: Box<Expr>,
        body: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// A rule head (Part IV §2): a relation tuple, or a `mask(...) by ...`.
/// `keyed by (...)` derived-node heads are not a distinct case here — they
/// are `Tuple` heads into an `Entity`-kind relation; `Entity` `NodeId`s are
/// always a hash of key fields (Skolem identity is just what that hash *is*
/// for a rule-derived row, vs. transaction-minted for a ground one — Part
/// III §3).
#[derive(Clone, Debug)]
pub enum Head {
    Tuple {
        rel: RelName,
        args: Vec<(RoleName, Term)>,
    },
    /// `mask(target) by reason` — both bound as `EdgeRef`s in the body
    /// (Part III §6). `relation` is the masked relation (the relation the
    /// `target` clause reads).
    Mask {
        relation: RelName,
        target: Var,
        reason: Var,
    },
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub id: RuleId,
    pub head: Head,
    pub body: Vec<Clause>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Advisory,
    Strict,
    Audit,
}

#[derive(Clone, Debug)]
pub struct Constraint {
    pub id: ConstraintId,
    pub severity: Severity,
    pub body: Vec<Clause>,
}

/// A registered pure function. Hand-built programs register Rust closures
/// directly; there is no expression compiler in this pass (that is
/// brix-ir's `fn` lowering). Determinism is the caller's obligation, same as
/// every other "hand-built program" concession in this crate.
pub type TotalFn = fn(&[Value]) -> Value;
/// A registered partial function; `Err(v)` becomes the `RuleError.error`
/// payload for the failing site (Part III §9).
pub type PartialFn = fn(&[Value]) -> Result<Value, Value>;

/// A function compiled from BrixMS source (issue #47): parameter names plus a
/// body [`Expr`] the evaluator runs by binding actuals into a fresh env. Lets a
/// total fn execute from source instead of a hand-registered [`TotalFn`], and
/// is resolved *before* the `fns` table in [`crate::eval`], so a source-defined
/// fn shadows a registered one of the same name.
#[derive(Clone, Debug)]
pub struct FnDef {
    pub params: Vec<Var>,
    pub body: Expr,
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub relations: BTreeMap<RelName, RelationDef>,
    pub rules: BTreeMap<RuleId, Rule>,
    pub constraints: BTreeMap<ConstraintId, Constraint>,
    pub fns: BTreeMap<FnName, TotalFn>,
    pub partial_fns: BTreeMap<FnName, PartialFn>,
    /// Functions compiled from source (issue #47).
    pub fn_defs: BTreeMap<FnName, FnDef>,
}

impl Program {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_relation(mut self, def: RelationDef) -> Self {
        self.relations.insert(def.name.clone(), def);
        self
    }

    pub fn with_rule(mut self, rule: Rule) -> Self {
        self.rules.insert(rule.id.clone(), rule);
        self
    }

    pub fn with_constraint(mut self, c: Constraint) -> Self {
        self.constraints.insert(c.id.clone(), c);
        self
    }

    pub fn with_fn(mut self, name: impl Into<FnName>, f: TotalFn) -> Self {
        self.fns.insert(name.into(), f);
        self
    }

    pub fn with_partial_fn(mut self, name: impl Into<FnName>, f: PartialFn) -> Self {
        self.partial_fns.insert(name.into(), f);
        self
    }

    /// Rules whose head is `mask(...)` targeting `relation` — `M(R)` in
    /// Part III §6 / Appendix F #3.
    pub fn mask_producers(&self, relation: &str) -> Vec<&Rule> {
        self.rules
            .values()
            .filter(|r| matches!(&r.head, Head::Mask { relation: rel, .. } if rel == relation))
            .collect()
    }

    /// Rules whose head is an ordinary tuple into `relation` (excludes mask
    /// rules, which do not add rows to `relation`).
    pub fn producers(&self, relation: &str) -> Vec<&Rule> {
        self.rules
            .values()
            .filter(|r| matches!(&r.head, Head::Tuple { rel, .. } if rel == relation))
            .collect()
    }

    /// Cheap structural checks the oracle relies on but does not otherwise
    /// enforce (full static semantics — Appendix E — is brix-ir's job, not
    /// reproduced here): every clause/head relation name is declared, and
    /// "rules cannot assert ground claims" (Part IV §2) — a `Tuple` head may
    /// only target an `Entity` or `Derived`-kind relation.
    pub fn validate(&self) -> Result<(), ProgramError> {
        for rule in self.rules.values() {
            match &rule.head {
                Head::Tuple { rel, .. } => {
                    let def = self
                        .relations
                        .get(rel)
                        .ok_or_else(|| ProgramError::UnknownRelation(rel.clone()))?;
                    if !matches!(def.kind, RelKind::Entity | RelKind::Derived) {
                        return Err(ProgramError::RuleHeadNotDerivable {
                            rule: rule.id.clone(),
                            relation: rel.clone(),
                        });
                    }
                }
                Head::Mask { relation, .. } => {
                    if !self.relations.contains_key(relation) {
                        return Err(ProgramError::UnknownRelation(relation.clone()));
                    }
                }
            }
            check_clauses(&rule.body, self)?;
        }
        for c in self.constraints.values() {
            check_clauses(&c.body, self)?;
        }
        Ok(())
    }
}

fn check_clauses(clauses: &[Clause], program: &Program) -> Result<(), ProgramError> {
    for clause in clauses {
        match clause {
            Clause::Edge { rel, .. } | Clause::History { rel, .. } => {
                if !program.relations.contains_key(rel) {
                    return Err(ProgramError::UnknownRelation(rel.clone()));
                }
            }
            Clause::Without(inner) => check_clauses(inner, program)?,
            Clause::When(_) | Clause::Let(_, _) => {}
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramError {
    UnknownRelation(RelName),
    RuleHeadNotDerivable { rule: RuleId, relation: RelName },
}

impl std::fmt::Display for ProgramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgramError::UnknownRelation(r) => write!(f, "unknown relation `{r}`"),
            ProgramError::RuleHeadNotDerivable { rule, relation } => write!(
                f,
                "rule `{rule}` targets `{relation}`, which is not an Entity or Derived \
                 relation — rules cannot assert ground claims (Part IV §2)"
            ),
        }
    }
}
impl std::error::Error for ProgramError {}
