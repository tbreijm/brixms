//! A bootstrap-safe, fact-oriented prototype of the future BrixMS-native
//! type-analysis package.
//!
//! The compiler continues to use [`crate::infer`]. This module deliberately
//! has a narrower job: mirror a useful Core-IR subset as deterministic facts,
//! saturate local typing constraints, and retain each conflict's derivation.
//! It is the executable reference shape for a later `brix.type` rule package;
//! it does not pretend that the package can self-host before `brix build`
//! executes function/rule bodies.
//!
//! The pure unification/dimension algebra lives in [`crate::solve`] and is
//! shared with [`crate::infer`] (#15 PR2: "one algorithm, two observers") —
//! this module supplies only the *observation*: it records [`Fact`]s with
//! [`Derivation`] provenance and [`TypeConflict`]s instead of mutating
//! `Expr.ty` and accumulating a flat error list.
//!
//! #15 PR3 freezes the fact schema a future native `brix.type` analysis
//! package targets:
//! - identity is **content-addressed** ([`FactId`], a `brix_canon::Digest`
//!   over the fact's own bytes — not a positional index), so the same fact
//!   hashes the same way across runs and across a future native derivation;
//! - the structural (input) facts — [`Fact::ExprKindIs`], [`Fact::ExprChild`],
//!   [`Fact::SchemaRole`], [`Fact::FnSig`], [`Fact::RowField`],
//!   [`Fact::RowTail`], [`Fact::DimTerm`] — sit alongside the derived
//!   (output) facts, so the program itself, not just the checker's verdicts,
//!   is present as facts;
//! - [`TypeConflict`] carries the origin [`Subject`] plus an honest
//!   per-kind [`ConflictKind`] payload — no fabricated types, no empty
//!   `because` sets.
//!
//! #15 PR3.5 amends the just-frozen schema, narrowly, before it gets an
//! external consumer: derived judgment facts now carry an assumption-context
//! **scope** as digested payload content, not envelope structure. The
//! invariants this establishes, for every future extension of this module:
//! - **unscoped fact kinds assert root-world judgments.** The structural
//!   (input) facts above never carry [`ScopeId`] — an operator tag, a schema
//!   role, a row shape don't depend on typing assumptions, so they are
//!   scope-invariant / root-world by construction;
//! - **any future assumption-dependent fact kind MUST carry scope inside
//!   its digested payload.** Scope is judgment *content* — it participates
//!   in [`FactId::derive`]'s hash — never something bolted on afterward;
//! - **scope may never be expressed solely as a relation over [`FactId`]s.**
//!   A `DerivedUnder(FactId, ScopeId)` side-relation would re-introduce the
//!   world-aliasing content-addressing exists to kill (one `FactId` meaning
//!   two different judgments depending which side-relation rows happen to
//!   be present) — scope belongs in the fact's own hashed bytes;
//! - **[`ScopeId`] is content-addressed with a constant root.** Never a
//!   counter — see [`ScopeId::root`]'s doc for why;
//! - (forward note, applies once candidate schemes exist) **candidate
//!   schemes must be α-normalized (canonical renaming) before digesting** —
//!   the scheme-level analogue of zonk-before-digest this module already
//!   follows for `Ty` payloads (see [`Reflect::zonk`]'s doc).
//!
//! #15 PR5 (§19.1 "The epistemic type system" / §27.3 "Typed missingness")
//! adds [`ConflictKind::EpistemicErasure`], appended after the PR 4 freeze
//! (new `write_conflict` ordinal, every earlier variant unchanged): the
//! shared [`crate::solve::step`] algebra now names a forbidden
//! epistemic-to-plain conversion (`Estimate<T> -/-> T`,
//! `Missing<T> -/-> T`, `Probability -/-> Bool`) instead of folding it into
//! [`ConflictKind::Mismatch`], and both checkers observe the identical
//! [`crate::solve::Step::Erasure`] the same way they already observe every
//! other shared `Step`.

use std::collections::{BTreeMap, BTreeSet};

use crate::core::{Constraint, Expr, ExprKind, ExprOrigin, Head, Query, Rule};
use crate::frontend::{FrontendSource, SchemaResolver};
use crate::ident::{Ident, QualIdent};
use crate::pattern::{Arg, Clause, Lit, Pattern, RoleArg};
use crate::solve::{self, DimBinaryStep, DimStep, Step};
use crate::types::{
    money_dimensions, quantity_dimensions, Dimensions, IntWidth, Row, RowField, RowTail, Ty, TyVar,
};
use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use core::fmt;

/// A stable, declaration-local subject in the reflective fact graph.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Subject {
    Binding {
        declaration: Ident,
        name: Ident,
    },
    Expr {
        origin: ExprOrigin,
    },
    Head {
        declaration: Ident,
        role: Ident,
    },
    /// A whole rule declaration, as a unit — the subject Appendix E rule
    /// side-condition conflicts (#15 PR4: `pure`/`det`/`nondiverge`,
    /// `keys(H) ⊆ Bindings`, mask-head, `Ordinary fn`) attach to, since
    /// those judgments are over the rule as a whole rather than any one
    /// binding/expr/head-role. Appended after the PR 3 freeze — new
    /// ordinal, existing arms' encodings unchanged.
    Rule {
        declaration: Ident,
    },
}

impl Canonical for Subject {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Subject::Binding { declaration, name } => w.write_enum(0, |w| {
                declaration.canon_write(w);
                name.canon_write(w);
            }),
            // Identity is the expression's own content-addressed `ExprId`
            // digest (declaration + source range, see `core::ExprId::derive`)
            // — sufficient and already collision-resistant, so there is no
            // need to also fold in `range` here.
            Subject::Expr { origin } => w.write_enum(1, |w| {
                w.write_bytes(origin.id.digest().as_bytes());
            }),
            Subject::Head { declaration, role } => w.write_enum(2, |w| {
                declaration.canon_write(w);
                role.canon_write(w);
            }),
            Subject::Rule { declaration } => w.write_enum(3, |w| declaration.canon_write(w)),
        }
    }
}

/// Opaque, content-addressed identity for the assumption context (a "world")
/// a derived judgment fact holds under (#15 PR3.5). **Content-addressed over
/// the assumption context, never a counter** — a counter would make
/// [`FactId`]s run-order-dependent, defeating the whole point of content
/// addressing. [`ScopeId::root`] is the well-known constant for the empty
/// assumption context; `reflect.rs` writes it everywhere today, since there
/// is no scope machinery (trees, hypotheses) yet — only the constant root
/// threaded through so the schema doesn't need to change identity meaning
/// again once a scoped checker exists. See the module freeze-comment
/// invariant list for the discipline this enforces.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ScopeId(Digest);

impl ScopeId {
    /// The constant root scope: the digest of the empty assumption context.
    /// Derived the same deterministic way as [`FactId::derive`]/
    /// [`crate::site::SiteId::derive`]/[`crate::core::ExprId::derive`] — a
    /// canon-encoded marker hashed under `Domain::Value` — so it is stable
    /// across calls, processes, and builds.
    pub fn root() -> Self {
        let mut w = CanonWriter::new();
        w.write_tag("brix.ir.reflect.ScopeId.root");
        ScopeId(w.digest(Domain::Value))
    }
}

impl Canonical for ScopeId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// The closed operator-tag projection of [`ExprKind`] (Core IR's expression
/// discriminants), carried by [`Fact::ExprKindIs`]. Kept as its own small
/// enum rather than re-exporting `ExprKind` because a fact payload should be
/// a plain tag, not a recursive structure the digest would have to walk.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ExprKindTag {
    Var,
    Lit,
    Call,
    Field,
    Record,
    If,
    Try,
    Comprehension,
    Let,
}

impl ExprKindTag {
    fn of(kind: &ExprKind) -> Self {
        match kind {
            ExprKind::Var(_) => ExprKindTag::Var,
            ExprKind::Lit(_) => ExprKindTag::Lit,
            ExprKind::Call { .. } => ExprKindTag::Call,
            ExprKind::Field { .. } => ExprKindTag::Field,
            ExprKind::Record { .. } => ExprKindTag::Record,
            ExprKind::If { .. } => ExprKindTag::If,
            ExprKind::Try { .. } => ExprKindTag::Try,
            ExprKind::Comprehension { .. } => ExprKindTag::Comprehension,
            ExprKind::Let { .. } => ExprKindTag::Let,
        }
    }
}

impl Canonical for ExprKindTag {
    fn canon_write(&self, w: &mut CanonWriter) {
        let ordinal: u8 = match self {
            ExprKindTag::Var => 0,
            ExprKindTag::Lit => 1,
            ExprKindTag::Call => 2,
            ExprKindTag::Field => 3,
            ExprKindTag::Record => 4,
            ExprKindTag::If => 5,
            ExprKindTag::Try => 6,
            ExprKindTag::Comprehension => 7,
            ExprKindTag::Let => 8,
        };
        w.write_uint(ordinal as u64);
    }
}

/// One derivable relation in the future `brix.type` package. Open enum: PR 4
/// (Appendix E rule side conditions) and PR 5 (epistemic/`Missing`) add
/// variants here without touching the identity/provenance/encoder envelope
/// ([`FactId`], [`Derivation`], [`write_fact`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fact {
    // --- structural (input) facts: the program itself, as facts ---
    /// The operator tag of an `Expr` node.
    ExprKindIs {
        subject: Subject,
        kind: ExprKindTag,
    },
    /// A direct parent → child edge in an expression tree, at `ordinal`
    /// among the parent's children. Replaces the `path: Vec<u32>` this
    /// module used to compute during traversal and then discard —
    /// transitive paths are recoverable by walking these edges.
    ExprChild {
        parent: Subject,
        ordinal: u32,
        child: Subject,
    },
    /// A relation's declared role type, as resolved from the schema.
    SchemaRole {
        relation: QualIdent,
        role: Ident,
        ty: Ty,
    },
    /// A called function's resolved signature.
    FnSig {
        function: Ident,
        params: Vec<Ty>,
        result: Ty,
        is_aggregate: bool,
    },
    /// One field of a subject's resolved record/relation row type.
    RowField {
        subject: Subject,
        field: Ident,
        ty: Ty,
    },
    /// A subject's resolved row openness (row-polymorphism tail).
    RowTail {
        subject: Subject,
        open: bool,
    },
    /// One ground-dimension exponent term of a subject's resolved type.
    DimTerm {
        subject: Subject,
        dimension: Ident,
        exponent: i32,
    },

    // --- derived (output) facts ---
    HasType {
        subject: Subject,
        ty: Ty,
        scope: ScopeId,
    },
    RequiresBool {
        subject: Subject,
        scope: ScopeId,
    },
    Applies {
        subject: Subject,
        operator: String,
        scope: ScopeId,
    },

    // --- structural (input) facts, appended after the PR 3/3.5 freeze ---
    /// Native `brix.type` vertical slice 1 (#15): which *variable* occupies
    /// a relation role, at `subject` (the binding the variable is unified
    /// against — same [`Subject::Binding`] shape [`Reflect::bind`] uses).
    /// Structural and scope-free like [`Fact::SchemaRole`] — an operator's
    /// role assignment doesn't depend on typing assumptions — appended here
    /// rather than reordered next to `SchemaRole` because the fact schema is
    /// append-only past the PR 3 freeze. Emitted from [`Reflect::role_arg`].
    ///
    /// `ordinal` is #15 native slice 2's var-at-two-roles amendment (Fable
    /// ruling, comment 5012408628): the zero-based occurrence index of this
    /// *variable's* role-bindings within one declaration's body, in
    /// [`Reflect::pattern`] traversal order — first occurrence ⇔ `ordinal ==
    /// 0`. This is a **payload-breaking amendment to the frozen kind 10**:
    /// every existing `RoleVar` `FactId` re-derives. Justified the same way
    /// PR 3.5's scope amendment was — every consumer is in-repo and
    /// recomputes both sides per run — and it is load-bearing: without it, a
    /// variable bound at the same role twice collapses to one indistinguishable
    /// `FactId` (silently under-reporting the program), and the native
    /// package's `TypeOfRoleBinding`/`VarRoleMismatch` rules have no
    /// structural way to prefer the first occurrence. See the module's PR
    /// 3.5 freeze-comment invariant list: identity-relevant content lives in
    /// the fact's own hashed bytes, never solely as a relation over
    /// `FactId`s — the reason this is in-payload rather than a side fact.
    RoleVar {
        subject: Subject,
        relation: QualIdent,
        role: Ident,
        ordinal: u32,
    },
    /// The literal counterpart of [`Fact::RoleVar`]: a role matched against
    /// a literal rather than bound to a variable, with the literal's own
    /// class (`ty = `[`lit_ty`]`(lit)`) recorded structurally so a native
    /// rule can compare it against the role's declared [`Fact::SchemaRole`]
    /// type without re-deriving it. `subject` mirrors the existing
    /// [`ConflictKind::Mismatch`] this arm's caller already raises on a
    /// mismatch: `Subject::Binding { declaration, name: role }` (no
    /// variable exists to name, so the role name stands in for it).
    RoleLit {
        subject: Subject,
        relation: QualIdent,
        role: Ident,
        ty: Ty,
    },
    /// #15 native slice 6 (UnknownField): a record/rel field-access expression
    /// `base.field`, recorded structurally so a native rule can flag the field
    /// as unknown when the base's resolved row (its [`Fact::RowField`]s) has no
    /// matching entry. `subject` is the field-access expr's own
    /// `Subject::Expr`; `base` is the accessed expression's `Subject::Expr`
    /// (the same subject its [`Fact::RowField`]s hang off, via the base's
    /// `HasType`); `field` is the accessed name. Appended after `RoleLit` past
    /// the PR 3 freeze — append-only, no reorder. Emitted from
    /// [`Reflect::expr`]'s `ExprKind::Field` arm.
    FieldAccess {
        subject: Subject,
        base: Subject,
        field: Ident,
    },
    /// #15 native slice 7 (Occurs): a variable-binding attempt inside reflect's
    /// own unification (`Step::Bind`, in `bind_ty`), recorded BEFORE the
    /// occurs-check decides to commit or reject it. `target` is exported already
    /// resolved (bind_ty resolves it first) — structurally subst-free, like
    /// HasType's post-inference ty. The first fact sourced from inside the
    /// algorithm rather than source traversal.
    BindAttempt {
        subject: Subject,
        var: TyVar,
        target: Ty,
    },
    /// #15 native slice 8 (`step` classification): the pair of already-resolved
    /// operands about to be classified by [`crate::solve::step`], recorded at
    /// every entry to [`Reflect::unify`] — mirroring [`Fact::BindAttempt`]'s
    /// placement, an event inside the algorithm recorded BEFORE `solve::step`
    /// classifies it, at every recursion depth (including every `Step::Descend`
    /// leaf `unify` reaches, since this fires on entry rather than only at the
    /// top of a traversal).
    UnifyAttempt {
        subject: Subject,
        expect: Ty,
        found: Ty,
    },
    /// #15 native slice 9 (binding fixpoint): the raw, un-chased edge `bind_ty`
    /// inserts into `subst` — captured at the accepted-branch `subst.insert`
    /// itself (NOT alongside `BindAttempt`, which fires earlier, before the
    /// occurs-check, on both accepted AND rejected attempts). `target` is `ty`
    /// as accepted — it may be a bare `Ty::Var` if the other operand was still
    /// unbound at bind time (a var-to-var edge). Unlike `BindAttempt`, `zonk`
    /// must NOT resolve it further: the point is to expose the one-hop state
    /// `subst` holds at THIS insert, before any later insert extends the chain.
    /// Recording only on the accepted path gives this fact "exactly one row per
    /// var, ever" (write-once), matching `self.subst.insert`'s own guarantee.
    SubstEdge {
        subject: Subject,
        var: TyVar,
        target: Ty,
    },
    /// #15 native Arity: a call site's own argument count, recorded
    /// unconditionally for every call. No scope (pure program structure, like
    /// FieldAccess).
    CallArity {
        subject: Subject,
        argc: u32,
    },
    /// #15 native Arity: one candidate overload's declared param count at its
    /// `ordinal` position in resolver.functions(func). `function` is the plain
    /// func.to_string() string (NOT a digest), so it round-trips byte-identical
    /// to Fact::Applies::operator and the native rule joins them directly.
    FnArity {
        function: String,
        ordinal: u32,
        paramc: u32,
    },
    /// #15 native Dimension (add/sub same-dimension): one same-dimension operator
    /// application over two GROUND-dimensioned operands, recorded regardless of
    /// whether the dims agree — the package decides the conflict (dims-token
    /// inequality). Emitted ONLY when `solve::dims` is `Some` for BOTH operands,
    /// so the Solve (var) and temporal (`Instant`/`Duration`, whose `dims` is
    /// `None`) branches never emit. `op` is the verbatim operation string (like
    /// `Applies::operator`); `left`/`right` are the operand `Ty`s (var-free by the
    /// guard). NOT emitted for mul/div (deferred).
    DimSameOp {
        subject: Subject,
        op: String,
        left: Ty,
        right: Ty,
    },
    /// #15 native rule-side-conditions (restatement): reflect's Appendix-E
    /// findings, emitted alongside the conflict so the package re-derives it via
    /// a RootScope join (completeness, not inference — the effect/binding analysis
    /// stays in crate::check). Subject is always Subject::Rule.
    RuleImpure {
        subject: Subject,
    },
    RuleUnboundHeadKey {
        subject: Subject,
        key: Ident,
    },
    RuleMaskRefNotEdgeBound {
        subject: Subject,
        var: Ident,
    },
    RuleOrdinaryFnOnDerivedRel {
        subject: Subject,
        relation: QualIdent,
    },
    /// #15 native rule-side-conditions (restatement), continued: the last two
    /// Appendix-E findings (both unit — no payload beyond `subject`), reaching
    /// 14/14 `ConflictKind` parity. Same shape as `RuleImpure`.
    RuleNondeterministic {
        subject: Subject,
    },
    RuleDivergent {
        subject: Subject,
    },
    /// #15 native overload no-unique-winner (restatement): reflect's
    /// overload-resolution direct-raise `Mismatch` (`resolve_call`, the
    /// `else` branch when `matches` yields no unique top-scoring candidate —
    /// either zero args-unifying candidates or a score tie). Emitted alongside
    /// that `self.conflict(..)` carrying the identical `expect`/`found` the
    /// conflict does (`arg_types[0]` / `arity_ok[0].params[0]`, or `Ty::Error`
    /// when either is absent), so the package re-derives the Mismatch via a
    /// plain `RootScope` join — completeness, not inference (the overload
    /// argmax stays in `reflect.rs`). Zonked like `UnifyAttempt`, so its two
    /// `Ty` payloads token-match the zonked `Mismatch` conflict byte-for-byte.
    OverloadNoWinner {
        subject: Subject,
        expect: Ty,
        found: Ty,
    },
}

/// The one canonical encoder `FactId::derive` uses. Never a second fact
/// encoder — later PRs and #20's coverage-matrix work extend this match
/// rather than writing their own. Content is `kind tag ++ payload` per
/// variant; the payload for most variants starts with the fact's own
/// `Subject`.
pub fn write_fact(fact: &Fact, w: &mut CanonWriter) {
    match fact {
        Fact::ExprKindIs { subject, kind } => w.write_enum(0, |w| {
            subject.canon_write(w);
            kind.canon_write(w);
        }),
        Fact::ExprChild {
            parent,
            ordinal,
            child,
        } => w.write_enum(1, |w| {
            parent.canon_write(w);
            w.write_uint(*ordinal as u64);
            child.canon_write(w);
        }),
        Fact::SchemaRole { relation, role, ty } => w.write_enum(2, |w| {
            relation.canon_write(w);
            role.canon_write(w);
            ty.canon_write(w);
        }),
        Fact::FnSig {
            function,
            params,
            result,
            is_aggregate,
        } => w.write_enum(3, |w| {
            function.canon_write(w);
            params.canon_write(w);
            result.canon_write(w);
            w.write_bool(*is_aggregate);
        }),
        Fact::RowField { subject, field, ty } => w.write_enum(4, |w| {
            subject.canon_write(w);
            field.canon_write(w);
            ty.canon_write(w);
        }),
        Fact::RowTail { subject, open } => w.write_enum(5, |w| {
            subject.canon_write(w);
            w.write_bool(*open);
        }),
        Fact::DimTerm {
            subject,
            dimension,
            exponent,
        } => w.write_enum(6, |w| {
            subject.canon_write(w);
            dimension.canon_write(w);
            w.write_int(*exponent as i64);
        }),
        Fact::HasType { subject, ty, scope } => w.write_enum(7, |w| {
            subject.canon_write(w);
            scope.canon_write(w);
            ty.canon_write(w);
        }),
        Fact::RequiresBool { subject, scope } => w.write_enum(8, |w| {
            subject.canon_write(w);
            scope.canon_write(w);
        }),
        Fact::Applies {
            subject,
            operator,
            scope,
        } => w.write_enum(9, |w| {
            subject.canon_write(w);
            scope.canon_write(w);
            w.write_str(operator);
        }),
        Fact::RoleVar {
            subject,
            relation,
            role,
            ordinal,
        } => w.write_enum(10, |w| {
            subject.canon_write(w);
            relation.canon_write(w);
            role.canon_write(w);
            w.write_uint(*ordinal as u64);
        }),
        Fact::RoleLit {
            subject,
            relation,
            role,
            ty,
        } => w.write_enum(11, |w| {
            subject.canon_write(w);
            relation.canon_write(w);
            role.canon_write(w);
            ty.canon_write(w);
        }),
        Fact::FieldAccess {
            subject,
            base,
            field,
        } => w.write_enum(12, |w| {
            subject.canon_write(w);
            base.canon_write(w);
            field.canon_write(w);
        }),
        Fact::BindAttempt {
            subject,
            var,
            target,
        } => w.write_enum(13, |w| {
            subject.canon_write(w);
            var.canon_write(w);
            target.canon_write(w);
        }),
        Fact::UnifyAttempt {
            subject,
            expect,
            found,
        } => w.write_enum(14, |w| {
            subject.canon_write(w);
            expect.canon_write(w);
            found.canon_write(w);
        }),
        Fact::SubstEdge {
            subject,
            var,
            target,
        } => w.write_enum(15, |w| {
            subject.canon_write(w);
            var.canon_write(w);
            target.canon_write(w);
        }),
        // ordinal 16 is an intentionally-unused hole (skipped by #124); append-only continues at 19
        Fact::CallArity { subject, argc } => w.write_enum(17, |w| {
            subject.canon_write(w);
            w.write_uint(*argc as u64);
        }),
        Fact::FnArity {
            function,
            ordinal,
            paramc,
        } => w.write_enum(18, |w| {
            w.write_str(function);
            w.write_uint(*ordinal as u64);
            w.write_uint(*paramc as u64);
        }),
        Fact::DimSameOp {
            subject,
            op,
            left,
            right,
        } => w.write_enum(19, |w| {
            subject.canon_write(w);
            w.write_str(op);
            left.canon_write(w);
            right.canon_write(w);
        }),
        Fact::RuleImpure { subject } => w.write_enum(20, |w| {
            subject.canon_write(w);
        }),
        Fact::RuleUnboundHeadKey { subject, key } => w.write_enum(21, |w| {
            subject.canon_write(w);
            key.canon_write(w);
        }),
        Fact::RuleMaskRefNotEdgeBound { subject, var } => w.write_enum(22, |w| {
            subject.canon_write(w);
            var.canon_write(w);
        }),
        Fact::RuleOrdinaryFnOnDerivedRel { subject, relation } => w.write_enum(23, |w| {
            subject.canon_write(w);
            relation.canon_write(w);
        }),
        Fact::RuleNondeterministic { subject } => w.write_enum(24, |w| {
            subject.canon_write(w);
        }),
        Fact::RuleDivergent { subject } => w.write_enum(25, |w| {
            subject.canon_write(w);
        }),
        Fact::OverloadNoWinner {
            subject,
            expect,
            found,
        } => w.write_enum(26, |w| {
            subject.canon_write(w);
            expect.canon_write(w);
            found.canon_write(w);
        }),
    }
}

/// Content-addressed fact identity: `Hash(Value domain, write_fact(fact))`.
/// Two structurally identical facts — even derived independently, even in
/// separate `analyze` runs — get the same id, mirroring
/// [`crate::site::SiteId::derive`] and [`crate::core::ExprId::derive`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FactId(Digest);

impl FactId {
    /// Derive the id from a fact's own canonical bytes ([`write_fact`]).
    /// Must only be called with a **resolved** fact — see the module-level
    /// note on why `analyze` computes ids in a finalization pass after
    /// solving, not during traversal.
    pub fn derive(fact: &Fact) -> Self {
        let mut w = CanonWriter::new();
        write_fact(fact, &mut w);
        FactId(w.digest(Domain::Value))
    }

    pub fn digest(&self) -> Digest {
        self.0
    }
}

impl fmt::Display for FactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fact:{}", &self.0.to_hex()[..12])
    }
}

/// A fact plus the earlier facts from which it follows, as a canonical set
/// of content-addressed ids (not a positional sequence).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Derivation {
    pub id: FactId,
    pub fact: Fact,
    pub because: BTreeSet<FactId>,
}

/// The honest, per-kind payload of a derived incompatibility. Distinct from
/// [`crate::infer::TypeError`] — see the module doc and the parity harness's
/// category map for how the two vocabularies line up.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConflictKind {
    Mismatch {
        left: Ty,
        right: Ty,
    },
    Arity {
        expected: u32,
        found: u32,
    },
    UnknownField {
        field: Ident,
    },
    NonBool {
        found: Ty,
    },
    Occurs {
        var: TyVar,
        into: Ty,
    },
    Dimension {
        op: String,
        left: Ty,
        right: Ty,
    },
    /// A `?` postfix applied to a non-`Result` type (mirrors `infer.rs`'s
    /// dedicated `TypeError::TryNonResult`). Appended last, after the PR 3
    /// freeze, so the five earlier variants' `write_conflict` ordinals are
    /// unchanged; see the parity harness's `try_non_result_agrees` fixture
    /// for why this needed its own variant rather than folding into
    /// `Mismatch`.
    TryNonResult {
        found: Ty,
    },
    /// #15 PR4: Appendix E `pure(B, H)` violated — mirrors
    /// [`crate::check::Finding::ImpureRule`]. Subject is a
    /// [`Subject::Rule`].
    ImpureRule,
    /// #15 PR4: Appendix E `det(B, H)` violated — mirrors
    /// [`crate::check::Finding::NondeterministicRule`].
    NondeterministicRule,
    /// #15 PR4: Appendix E `nondiverge(B, H)` violated — mirrors
    /// [`crate::check::Finding::DivergentRule`].
    DivergentRule,
    /// #15 PR4: Appendix E `keys(H) ⊆ Bindings` violated — mirrors
    /// [`crate::check::Finding::UnboundHeadKey`].
    UnboundHeadKey {
        key: Ident,
    },
    /// #15 PR4: Appendix E mask-head side condition violated — mirrors
    /// [`crate::check::Finding::MaskRefNotEdgeBound`].
    MaskRefNotEdgeBound {
        var: Ident,
    },
    /// #15 PR4: Appendix E `Ordinary fn` violated — mirrors
    /// [`crate::check::Finding::OrdinaryFnOnDerivedRel`].
    OrdinaryFnOnDerivedRel {
        relation: QualIdent,
    },
    /// #15 PR5 (§19.1 / conformance I.22.2): an implicit conversion from an
    /// epistemic-status-bearing type (`Estimate<T>`, `Missing<T>`,
    /// `Probability`) to its plain payload type (or, for `Probability`,
    /// `Bool`) was attempted — mirrors [`crate::infer::TypeError::EpistemicErasure`].
    /// Appended last, after the PR 4 freeze, so every earlier variant's
    /// `write_conflict` ordinal is unchanged.
    EpistemicErasure {
        from: Ty,
        to: Ty,
    },
}

/// A derived incompatibility. It is intentionally distinct from a kernel key
/// conflict: competing provisional facts can be legitimate while solving.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TypeConflict {
    pub subject: Subject,
    pub kind: ConflictKind,
    pub because: BTreeSet<FactId>,
    pub scope: ScopeId,
}

/// Canonical encoder for [`TypeConflict`], mirroring [`write_fact`]'s shape
/// (kind tag ++ payload, subject-first). Conflicts are not themselves
/// content-addressed (no `ConflictId` exists), but a byte-stable encoding is
/// what the determinism golden test needs, and #20's coverage-matrix work
/// can reuse this the same way it reuses `write_fact`.
pub fn write_conflict(conflict: &TypeConflict, w: &mut CanonWriter) {
    conflict.subject.canon_write(w);
    conflict.scope.canon_write(w);
    match &conflict.kind {
        ConflictKind::Mismatch { left, right } => w.write_enum(0, |w| {
            left.canon_write(w);
            right.canon_write(w);
        }),
        ConflictKind::Arity { expected, found } => w.write_enum(1, |w| {
            w.write_uint(*expected as u64);
            w.write_uint(*found as u64);
        }),
        ConflictKind::UnknownField { field } => w.write_enum(2, |w| field.canon_write(w)),
        ConflictKind::NonBool { found } => w.write_enum(3, |w| found.canon_write(w)),
        ConflictKind::Occurs { var, into } => w.write_enum(4, |w| {
            var.canon_write(w);
            into.canon_write(w);
        }),
        ConflictKind::Dimension { op, left, right } => w.write_enum(5, |w| {
            w.write_str(op);
            left.canon_write(w);
            right.canon_write(w);
        }),
        ConflictKind::TryNonResult { found } => w.write_enum(6, |w| found.canon_write(w)),
        ConflictKind::ImpureRule => w.write_enum(7, |_| {}),
        ConflictKind::NondeterministicRule => w.write_enum(8, |_| {}),
        ConflictKind::DivergentRule => w.write_enum(9, |_| {}),
        ConflictKind::UnboundHeadKey { key } => w.write_enum(10, |w| key.canon_write(w)),
        ConflictKind::MaskRefNotEdgeBound { var } => w.write_enum(11, |w| var.canon_write(w)),
        ConflictKind::OrdinaryFnOnDerivedRel { relation } => {
            w.write_enum(12, |w| relation.canon_write(w))
        }
        ConflictKind::EpistemicErasure { from, to } => w.write_enum(13, |w| {
            from.canon_write(w);
            to.canon_write(w);
        }),
    }
}

/// Saturated facts and explainable conflicts for a Core source. The
/// content-addressed form only — internal positional bookkeeping never
/// escapes [`analyze`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReflectiveReport {
    pub facts: Vec<Derivation>,
    pub conflicts: Vec<TypeConflict>,
}

impl ReflectiveReport {
    pub fn is_consistent(&self) -> bool {
        self.conflicts.is_empty()
    }
}

type Env = BTreeMap<Ident, (Ty, usize)>;
const NO_FACT: usize = usize::MAX;

/// Run the fact-oriented checker. It covers the v1 expression subset that is
/// currently lowered: bindings from relation schemas, lets, guards, heads,
/// literals, records, fields, calls, and the ground-dimensional operators.
pub fn analyze(source: &FrontendSource, resolver: &impl SchemaResolver) -> ReflectiveReport {
    let mut cx = Reflect::default();
    for rule in &source.rules {
        cx.rule(rule, resolver);
    }
    for constraint in &source.constraints {
        cx.constraint(constraint, resolver);
    }
    for query in &source.queries {
        cx.query(query, resolver);
    }
    cx.finish()
}

/// One fact recorded during traversal, still keyed by a positional index
/// (append-only within one `analyze` run — cheap provenance handle while
/// solving is in progress). [`Reflect::finalize`] turns these into
/// content-addressed [`Derivation`]s once every `Ty` is resolved.
struct Draft {
    id: usize,
    fact: Fact,
    because: Vec<usize>,
}

/// The conflict-side counterpart of [`Draft`].
struct DraftConflict {
    subject: Subject,
    kind: ConflictKind,
    because: Vec<usize>,
    scope: ScopeId,
}

#[derive(Default)]
struct Reflect {
    drafts: Vec<Draft>,
    draft_conflicts: Vec<DraftConflict>,
    subst: BTreeMap<TyVar, Ty>,
    /// #15 native slice 2: the per-(declaration, variable) occurrence
    /// counter [`Fact::RoleVar::ordinal`] records. Keyed by the pair rather
    /// than threaded as a fresh `BTreeMap<Ident, u32>` argument through
    /// every `pattern`/`role_arg` call site (including the `Comprehension`
    /// boundary in [`Reflect::expr`], which forks a *local* `Env` but stays
    /// within the same declaration) — `declaration` in the key makes this
    /// equivalent to a map reset at the start of each `rule`/`constraint`/
    /// `query` call, since no two declarations share an `Ident`, while
    /// avoiding a parameter added to every intermediate signature. Read in
    /// [`Reflect::role_arg`]'s `Arg::Var` arm, in [`Reflect::pattern`]
    /// traversal order (the single sequential traversal this checker
    /// already performs).
    role_ordinals: BTreeMap<(Ident, Ident), u32>,
}

impl Reflect {
    /// Re-resolve every `Ty` payload recorded during traversal against the
    /// final substitution. Must run **after** traversal completes: a content
    /// digest taken mid-solve would hash an unresolved `TyVar`, and the same
    /// source could then export different `FactId`s depending on traversal
    /// order — the opposite of "same fact ⇒ same digest."
    fn zonk(&mut self) {
        let subst = self.subst.clone();
        for draft in &mut self.drafts {
            match &mut draft.fact {
                Fact::HasType { ty, .. } | Fact::SchemaRole { ty, .. } => {
                    *ty = solve::resolve(&subst, ty.clone());
                }
                Fact::FnSig { params, result, .. } => {
                    for param in params.iter_mut() {
                        *param = solve::resolve(&subst, param.clone());
                    }
                    *result = solve::resolve(&subst, result.clone());
                }
                Fact::ExprKindIs { .. }
                | Fact::ExprChild { .. }
                | Fact::RowField { .. }
                | Fact::RowTail { .. }
                | Fact::DimTerm { .. }
                | Fact::RequiresBool { .. }
                | Fact::Applies { .. }
                | Fact::FieldAccess { .. }
                | Fact::RoleVar { .. }
                | Fact::CallArity { .. }
                | Fact::FnArity { .. }
                | Fact::RuleImpure { .. }
                | Fact::RuleUnboundHeadKey { .. }
                | Fact::RuleMaskRefNotEdgeBound { .. }
                | Fact::RuleOrdinaryFnOnDerivedRel { .. }
                | Fact::RuleNondeterministic { .. }
                | Fact::RuleDivergent { .. } => {}
                Fact::DimSameOp { left, right, .. } => {
                    *left = solve::resolve(&subst, left.clone());
                    *right = solve::resolve(&subst, right.clone());
                }
                // `ty` here is always `lit_ty(lit)` — a concrete literal
                // class (`Unit`/`Bool`/`Int`/`Str`/`F64`/`Enum`) that never
                // contains a `TyVar`, so there is nothing to resolve.
                Fact::RoleLit { .. } => {}
                Fact::BindAttempt { target, .. } => {
                    *target = solve::resolve(&subst, target.clone());
                }
                Fact::UnifyAttempt { expect, found, .. }
                | Fact::OverloadNoWinner { expect, found, .. } => {
                    *expect = solve::resolve(&subst, expect.clone());
                    *found = solve::resolve(&subst, found.clone());
                }
                // #15 native slice 9: deliberately UNCHANGED by zonk. BindAttempt.target is
                // fully re-chased above; SubstEdge.target must instead survive that collapse
                // and read exactly as subst.insert recorded it (the raw one-hop edge the
                // package's own Resolved fixpoint chases). Zonking it would make slice 9
                // vacuous (identical to consuming zonked BindAttempt).
                Fact::SubstEdge { .. } => {}
            }
        }
        for conflict in &mut self.draft_conflicts {
            match &mut conflict.kind {
                ConflictKind::Mismatch { left, right } => {
                    *left = solve::resolve(&subst, left.clone());
                    *right = solve::resolve(&subst, right.clone());
                }
                ConflictKind::NonBool { found } | ConflictKind::TryNonResult { found } => {
                    *found = solve::resolve(&subst, found.clone());
                }
                ConflictKind::Occurs { into, .. } => *into = solve::resolve(&subst, into.clone()),
                ConflictKind::Dimension { left, right, .. } => {
                    *left = solve::resolve(&subst, left.clone());
                    *right = solve::resolve(&subst, right.clone());
                }
                ConflictKind::Arity { .. } | ConflictKind::UnknownField { .. } => {}
                // #15 PR4: no `Ty` payload to resolve — Appendix E rule
                // side-condition conflicts carry only `Ident`/`QualIdent`.
                ConflictKind::ImpureRule
                | ConflictKind::NondeterministicRule
                | ConflictKind::DivergentRule
                | ConflictKind::UnboundHeadKey { .. }
                | ConflictKind::MaskRefNotEdgeBound { .. }
                | ConflictKind::OrdinaryFnOnDerivedRel { .. } => {}
                ConflictKind::EpistemicErasure { from, to } => {
                    *from = solve::resolve(&subst, from.clone());
                    *to = solve::resolve(&subst, to.clone());
                }
            }
        }
    }

    /// Derive [`Fact::RowField`]/[`Fact::RowTail`]/[`Fact::DimTerm`] facts
    /// from every (already zonked) [`Fact::HasType`]'s resolved type. Runs
    /// after [`Reflect::zonk`] so the decomposed row/dimension shape is the
    /// final, solved one.
    fn augment_structural_facts(&mut self) {
        let snapshot: Vec<(usize, Subject, Ty)> = self
            .drafts
            .iter()
            .filter_map(|draft| match &draft.fact {
                Fact::HasType { subject, ty, .. } => Some((draft.id, subject.clone(), ty.clone())),
                _ => None,
            })
            .collect();
        for (origin_id, subject, ty) in snapshot {
            if let Ty::Record(row) | Ty::Rel(row) = &ty {
                for field in &row.fields {
                    self.fact(
                        Fact::RowField {
                            subject: subject.clone(),
                            field: field.name.clone(),
                            ty: field.ty.clone(),
                        },
                        vec![origin_id],
                    );
                }
                let open = matches!(row.tail, RowTail::Open(_));
                self.fact(
                    Fact::RowTail {
                        subject: subject.clone(),
                        open,
                    },
                    vec![origin_id],
                );
            }
            if let Some(dims) = ty_dimensions(&ty) {
                for dim in dims {
                    self.fact(
                        Fact::DimTerm {
                            subject: subject.clone(),
                            dimension: dim.name,
                            exponent: dim.exponent,
                        },
                        vec![origin_id],
                    );
                }
            }
        }
    }

    /// Assign canonical [`FactId`]s over the resolved drafts and rebuild
    /// every `because` from positional indices into a [`BTreeSet<FactId>`].
    /// Safe as a single forward pass: traversal only ever records a `because`
    /// referencing an *already-created* draft (ids are handed out in
    /// strictly increasing append order and used only after creation), so by
    /// the time draft `i` is processed, `id_map` already holds every id it
    /// can reference.
    fn finalize(self) -> ReflectiveReport {
        let mut id_map: BTreeMap<usize, FactId> = BTreeMap::new();
        let mut facts = Vec::with_capacity(self.drafts.len());
        for draft in self.drafts {
            let id = FactId::derive(&draft.fact);
            id_map.insert(draft.id, id);
            let because: BTreeSet<FactId> = draft
                .because
                .iter()
                .filter_map(|positional| id_map.get(positional).copied())
                .collect();
            facts.push(Derivation {
                id,
                fact: draft.fact,
                because,
            });
        }
        let conflicts = self
            .draft_conflicts
            .into_iter()
            .map(|draft| TypeConflict {
                subject: draft.subject,
                kind: draft.kind,
                because: draft
                    .because
                    .iter()
                    .filter_map(|positional| id_map.get(positional).copied())
                    .collect(),
                scope: draft.scope,
            })
            .collect();
        ReflectiveReport { facts, conflicts }
    }

    /// The traversal-to-report pipeline: zonk, derive structural facts from
    /// the zonked types, then assign content-addressed ids.
    fn finish(mut self) -> ReflectiveReport {
        self.zonk();
        self.augment_structural_facts();
        self.finalize()
    }

    fn fact(&mut self, fact: Fact, because: Vec<usize>) -> usize {
        let id = self.drafts.len();
        let because: Vec<usize> = because
            .into_iter()
            .filter(|dependency| *dependency != NO_FACT)
            .collect();
        self.drafts.push(Draft { id, fact, because });
        id
    }

    /// The one conflict-recording entry point — also the single place that
    /// stamps [`ScopeId::root`] onto every [`TypeConflict`] today (#15
    /// PR3.5). Every `reflect.rs` conflict is currently a root-world
    /// judgment; a future scoped checker would thread a real `ScopeId`
    /// through here instead of the constant.
    fn conflict(&mut self, subject: Subject, kind: ConflictKind, because: Vec<usize>) {
        let because: Vec<usize> = because
            .into_iter()
            .filter(|dependency| *dependency != NO_FACT)
            .collect();
        self.draft_conflicts.push(DraftConflict {
            subject,
            kind,
            because,
            scope: ScopeId::root(),
        });
    }

    fn resolve(&self, ty: Ty) -> Ty {
        solve::resolve(&self.subst, ty)
    }

    /// The one unification entry point. [`solve::step`] is the shared
    /// algebra's answer to "what should happen for these two resolved
    /// types"; this method is only the *observation* — record a `Fact`
    /// binding or a [`TypeConflict`] — the algorithm itself never lives
    /// here (see [`crate::infer::Infer::unify`] for the other observer).
    /// `subject` is the origin a resulting conflict should be attributed to.
    fn unify(&mut self, subject: Subject, expected: Ty, found: Ty, because: Vec<usize>) {
        let expected = self.resolve(expected);
        let found = self.resolve(found);
        self.fact(
            Fact::UnifyAttempt {
                subject: subject.clone(),
                expect: expected.clone(),
                found: found.clone(),
            },
            because.clone(),
        );
        match solve::step(expected, found) {
            Step::Done => {}
            Step::Bind(variable, ty) => self.bind_ty(subject, variable, ty, because),
            Step::Rows(left, right) => self.unify_rows(subject, left, right, because),
            Step::Descend(pairs) => {
                for (left, right) in pairs {
                    self.unify(subject.clone(), left, right, because.clone());
                }
            }
            Step::Mismatch(expected, found) => self.conflict(
                subject,
                ConflictKind::Mismatch {
                    left: expected,
                    right: found,
                },
                because,
            ),
            Step::Erasure(from, to) => self.conflict(
                subject,
                ConflictKind::EpistemicErasure { from, to },
                because,
            ),
        }
    }

    fn bind_ty(&mut self, subject: Subject, variable: TyVar, ty: Ty, because: Vec<usize>) {
        let ty = self.resolve(ty);
        if ty == Ty::Var(variable) {
            return;
        }
        self.fact(
            Fact::BindAttempt {
                subject: subject.clone(),
                var: variable,
                target: ty.clone(),
            },
            because.clone(),
        );
        if solve::occurs(variable, &ty, &self.subst) {
            self.conflict(
                subject,
                ConflictKind::Occurs {
                    var: variable,
                    into: ty,
                },
                because,
            );
        } else {
            // #15 native slice 9: record the accepted subst.insert itself as
            // its own fact, at this insert — NOT paired with BindAttempt (which
            // also fires on rejected attempts). This placement is what gives
            // SubstEdge "exactly one row per var, ever" (write-once).
            self.fact(
                Fact::SubstEdge {
                    subject: subject.clone(),
                    var: variable,
                    target: ty.clone(),
                },
                because.clone(),
            );
            self.subst.insert(variable, ty);
        }
    }

    /// Row symmetry ruling: [`solve::match_rows`] checks both directions,
    /// so `{a} ~ closed {a,b}` is a mismatch regardless of which side is
    /// `left`/`right`. Each missing field is its own honest
    /// [`ConflictKind::UnknownField`] — mirrors `infer.rs`'s
    /// `Infer::unify_rows`, which raises one `TypeError::UnknownField` per
    /// field rather than one conflict per side.
    fn unify_rows(&mut self, subject: Subject, left: Row, right: Row, because: Vec<usize>) {
        let matched = solve::match_rows(&left, &right);
        for (a, b) in matched.pairs {
            self.unify(subject.clone(), a, b, because.clone());
        }
        for field in matched.missing_in_right {
            self.conflict(
                subject.clone(),
                ConflictKind::UnknownField { field },
                because.clone(),
            );
        }
        for field in matched.missing_in_left {
            self.conflict(
                subject.clone(),
                ConflictKind::UnknownField { field },
                because.clone(),
            );
        }
    }

    fn rule(&mut self, rule: &Rule, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        self.pattern(&rule.name, &rule.body, &mut env, resolver);
        self.head(&rule.name, &rule.head, &env, resolver);
        self.rule_side_conditions(rule, resolver);
    }

    /// #15 PR4: mirror [`crate::check::check_rule`]'s Appendix E rule
    /// side-condition checks (`pure`/`det`/`nondiverge`, `keys(H) ⊆
    /// Bindings`, mask-head, `Ordinary fn`) as [`TypeConflict`]s, using the
    /// exact same shared judgments (`crate::check::scan_rule_calls`,
    /// `unbound_head_keys`, `unbound_mask_refs`, and
    /// `Rule::effect_flags`) the trusted checker uses — "one algorithm, two
    /// observers," same as [`crate::solve`] for the type algebra, so the
    /// two checkers cannot silently diverge on what counts as a violation.
    fn rule_side_conditions(&mut self, rule: &Rule, resolver: &impl SchemaResolver) {
        let subject = Subject::Rule {
            declaration: rule.name.clone(),
        };
        let flags = rule.effect_flags();
        let calls = crate::check::scan_rule_calls(rule, resolver);
        if !flags.pure {
            self.conflict(subject.clone(), ConflictKind::ImpureRule, vec![]);
            self.fact(
                Fact::RuleImpure {
                    subject: subject.clone(),
                },
                vec![],
            );
        }
        if !flags.det {
            self.conflict(subject.clone(), ConflictKind::NondeterministicRule, vec![]);
            self.fact(
                Fact::RuleNondeterministic {
                    subject: subject.clone(),
                },
                vec![],
            );
        }
        if !flags.nondiverge || calls.diverges {
            self.conflict(subject.clone(), ConflictKind::DivergentRule, vec![]);
            self.fact(
                Fact::RuleDivergent {
                    subject: subject.clone(),
                },
                vec![],
            );
        }
        for key in crate::check::unbound_head_keys(rule) {
            self.conflict(
                subject.clone(),
                ConflictKind::UnboundHeadKey { key: key.clone() },
                vec![],
            );
            self.fact(
                Fact::RuleUnboundHeadKey {
                    subject: subject.clone(),
                    key,
                },
                vec![],
            );
        }
        for var in crate::check::unbound_mask_refs(rule) {
            self.conflict(
                subject.clone(),
                ConflictKind::MaskRefNotEdgeBound { var: var.clone() },
                vec![],
            );
            self.fact(
                Fact::RuleMaskRefNotEdgeBound {
                    subject: subject.clone(),
                    var,
                },
                vec![],
            );
        }
        for relation in calls.ordinary_on_derived {
            self.conflict(
                subject.clone(),
                ConflictKind::OrdinaryFnOnDerivedRel {
                    relation: relation.clone(),
                },
                vec![],
            );
            self.fact(
                Fact::RuleOrdinaryFnOnDerivedRel {
                    subject: subject.clone(),
                    relation,
                },
                vec![],
            );
        }
    }

    fn query(&mut self, query: &Query, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        for (name, ty) in &query.params {
            let subject = Subject::Binding {
                declaration: query.name.clone(),
                name: name.clone(),
            };
            let id = self.fact(
                Fact::HasType {
                    subject,
                    ty: ty.clone(),
                    scope: ScopeId::root(),
                },
                vec![],
            );
            env.insert(name.clone(), (ty.clone(), id));
        }
        self.pattern(&query.name, &query.body, &mut env, resolver);
        let (yielded, evidence) = self.expr(&query.name, &query.yields, &env, resolver);
        let expected = Ty::rel(match yielded {
            Ty::Record(row) => *row,
            ty => Row::closed(vec![RowField {
                name: Ident::new("value"),
                ty,
            }]),
        });
        let subject = Subject::Expr {
            origin: query.yields.origin,
        };
        self.unify(subject, query.result.clone(), expected, vec![evidence]);
    }

    fn constraint(&mut self, constraint: &Constraint, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        self.pattern(&constraint.name, &constraint.body, &mut env, resolver);
    }

    fn pattern(
        &mut self,
        declaration: &Ident,
        pattern: &Pattern,
        env: &mut Env,
        resolver: &impl SchemaResolver,
    ) {
        for clause in &pattern.clauses {
            match clause {
                Clause::Edge { relation, args, .. } | Clause::History { relation, args, .. } => {
                    if let Some(schema) = resolver.relation(relation) {
                        for arg in args {
                            if let Some((_, ty)) =
                                schema.roles.iter().find(|(name, _)| name == &arg.role)
                            {
                                self.fact(
                                    Fact::SchemaRole {
                                        relation: relation.clone(),
                                        role: arg.role.clone(),
                                        ty: ty.clone(),
                                    },
                                    vec![],
                                );
                                self.role_arg(declaration, relation, arg, ty.clone(), env);
                            }
                        }
                    }
                }
                Clause::Entity {
                    var,
                    entity,
                    fields,
                } => {
                    self.bind(declaration, var, Ty::NodeRef(entity.clone()), env, vec![]);
                    let relation = QualIdent::simple(entity.as_str());
                    if let Some(schema) = resolver.relation(&relation) {
                        for arg in fields {
                            if let Some((_, ty)) =
                                schema.roles.iter().find(|(name, _)| name == &arg.role)
                            {
                                self.fact(
                                    Fact::SchemaRole {
                                        relation: relation.clone(),
                                        role: arg.role.clone(),
                                        ty: ty.clone(),
                                    },
                                    vec![],
                                );
                                self.role_arg(declaration, &relation, arg, ty.clone(), env);
                            }
                        }
                    }
                }
                Clause::Let { binds, expr } => {
                    let (ty, evidence) = self.expr(declaration, expr, env, resolver);
                    self.bind(declaration, binds, ty, env, vec![evidence]);
                }
                Clause::When(expr) => {
                    let (ty, evidence) = self.expr(declaration, expr, env, resolver);
                    let subject = Subject::Expr {
                        origin: expr.origin,
                    };
                    self.fact(
                        Fact::RequiresBool {
                            subject: subject.clone(),
                            scope: ScopeId::root(),
                        },
                        vec![evidence],
                    );
                    if ty != Ty::Bool && !matches!(ty, Ty::Var(_)) {
                        self.conflict(subject, ConflictKind::NonBool { found: ty }, vec![evidence]);
                    }
                }
                Clause::Any(cases) => {
                    for case in cases {
                        self.pattern(declaration, case, env, resolver);
                    }
                }
                Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                    self.pattern(declaration, p, env, resolver)
                }
            }
        }
    }

    /// `relation` identifies the schema role `arg.role` is drawn from — the
    /// caller already resolved it to emit [`Fact::SchemaRole`]; this method
    /// additionally records exactly which variable (or literal) occupies
    /// that role (#15 native `brix.type` vertical slice 1: [`Fact::RoleVar`]
    /// / [`Fact::RoleLit`]), in addition to the existing bind/mismatch
    /// behavior below, which is unchanged.
    fn role_arg(
        &mut self,
        declaration: &Ident,
        relation: &QualIdent,
        arg: &RoleArg,
        expected: Ty,
        env: &mut Env,
    ) {
        match &arg.arg {
            Arg::Var(name) => {
                self.bind(declaration, name, expected, env, vec![]);
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
                self.fact(
                    Fact::RoleVar {
                        subject,
                        relation: relation.clone(),
                        role: arg.role.clone(),
                        ordinal,
                    },
                    vec![],
                );
            }
            Arg::Lit(lit) => {
                let found = lit_ty(lit);
                let subject = Subject::Binding {
                    declaration: declaration.clone(),
                    name: arg.role.clone(),
                };
                self.fact(
                    Fact::RoleLit {
                        subject: subject.clone(),
                        relation: relation.clone(),
                        role: arg.role.clone(),
                        ty: found.clone(),
                    },
                    vec![],
                );
                if found != expected {
                    self.conflict(
                        subject,
                        ConflictKind::Mismatch {
                            left: expected,
                            right: found,
                        },
                        vec![],
                    );
                }
            }
        }
    }

    fn bind(
        &mut self,
        declaration: &Ident,
        name: &Ident,
        ty: Ty,
        env: &mut Env,
        because: Vec<usize>,
    ) {
        if let Some((old, old_fact)) = env.get(name).cloned() {
            let subject = Subject::Binding {
                declaration: declaration.clone(),
                name: name.clone(),
            };
            self.unify(subject, old, ty, vec![old_fact]);
            return;
        }
        let subject = Subject::Binding {
            declaration: declaration.clone(),
            name: name.clone(),
        };
        let id = self.fact(
            Fact::HasType {
                subject,
                ty: ty.clone(),
                scope: ScopeId::root(),
            },
            because,
        );
        env.insert(name.clone(), (ty, id));
    }

    fn head(
        &mut self,
        declaration: &Ident,
        head: &Head,
        env: &Env,
        resolver: &impl SchemaResolver,
    ) {
        let Head::Tuple { relation, args } = head else {
            return;
        };
        let Some(schema) = resolver.relation(relation) else {
            return;
        };
        for arg in args {
            let Some((_, expected)) = schema.roles.iter().find(|(name, _)| name == &arg.role)
            else {
                continue;
            };
            let (found, because) = match &arg.arg {
                Arg::Var(name) => env.get(name).cloned().unwrap_or((Ty::Error, NO_FACT)),
                Arg::Lit(lit) => (lit_ty(lit), NO_FACT),
            };
            let subject = Subject::Head {
                declaration: declaration.clone(),
                role: arg.role.clone(),
            };
            self.fact(
                Fact::SchemaRole {
                    relation: relation.clone(),
                    role: arg.role.clone(),
                    ty: expected.clone(),
                },
                vec![],
            );
            let head_fact = self.fact(
                Fact::HasType {
                    subject: subject.clone(),
                    ty: expected.clone(),
                    scope: ScopeId::root(),
                },
                vec![because],
            );
            self.unify(subject, expected.clone(), found, vec![head_fact, because]);
        }
    }

    fn expr(
        &mut self,
        declaration: &Ident,
        expr: &Expr,
        env: &Env,
        resolver: &impl SchemaResolver,
    ) -> (Ty, usize) {
        let subject = Subject::Expr {
            origin: expr.origin,
        };
        self.fact(
            Fact::ExprKindIs {
                subject: subject.clone(),
                kind: ExprKindTag::of(&expr.kind),
            },
            vec![],
        );
        let (ty, because) = match &*expr.kind {
            ExprKind::Var(name) => env.get(name).cloned().unwrap_or((expr.ty.clone(), NO_FACT)),
            ExprKind::Lit(lit) => (lit_ty(lit), NO_FACT),
            ExprKind::Record { fields } => {
                let mut row = Vec::new();
                let mut deps = Vec::new();
                for (ordinal, (name, value)) in fields.iter().enumerate() {
                    let (ty, id) = self.expr(declaration, value, env, resolver);
                    self.fact(
                        Fact::ExprChild {
                            parent: subject.clone(),
                            ordinal: ordinal as u32,
                            child: Subject::Expr {
                                origin: value.origin,
                            },
                        },
                        vec![],
                    );
                    row.push(RowField {
                        name: name.clone(),
                        ty,
                    });
                    deps.push(id);
                }
                (
                    Ty::record(Row::closed(row)),
                    deps.first().copied().unwrap_or(NO_FACT),
                )
            }
            ExprKind::Field { base, field } => {
                let (base_ty, id) = self.expr(declaration, base, env, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 0,
                        child: Subject::Expr {
                            origin: base.origin,
                        },
                    },
                    vec![],
                );
                // #15 native slice 6: record the field-access structurally so a
                // native rule can flag an unknown field by the *absence* of a
                // matching `Fact::RowField` on the base. `base` names the
                // accessed expression's subject — the same subject the base's
                // `RowField`s hang off (via its `HasType`).
                self.fact(
                    Fact::FieldAccess {
                        subject: subject.clone(),
                        base: Subject::Expr {
                            origin: base.origin,
                        },
                        field: field.clone(),
                    },
                    vec![id],
                );
                match self.resolve(base_ty) {
                    Ty::Record(row) | Ty::Rel(row) => {
                        if let Some(found) = row.fields.iter().find(|x| &x.name == field) {
                            (found.ty.clone(), id)
                        } else {
                            self.conflict(
                                subject.clone(),
                                ConflictKind::UnknownField {
                                    field: field.clone(),
                                },
                                vec![id],
                            );
                            (Ty::Error, id)
                        }
                    }
                    found => {
                        if !matches!(found, Ty::Var(_)) {
                            self.conflict(
                                subject.clone(),
                                ConflictKind::UnknownField {
                                    field: field.clone(),
                                },
                                vec![id],
                            );
                        }
                        (Ty::Error, id)
                    }
                }
            }
            ExprKind::If { cond, then, els } => {
                let (cond_ty, cond_id) = self.expr(declaration, cond, env, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 0,
                        child: Subject::Expr {
                            origin: cond.origin,
                        },
                    },
                    vec![],
                );
                self.unify(subject.clone(), Ty::Bool, cond_ty, vec![cond_id]);
                let (then_ty, then_id) = self.expr(declaration, then, env, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 1,
                        child: Subject::Expr {
                            origin: then.origin,
                        },
                    },
                    vec![],
                );
                let (else_ty, else_id) = self.expr(declaration, els, env, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 2,
                        child: Subject::Expr { origin: els.origin },
                    },
                    vec![],
                );
                self.unify(
                    subject.clone(),
                    then_ty.clone(),
                    else_ty,
                    vec![then_id, else_id],
                );
                (self.resolve(then_ty), then_id)
            }
            ExprKind::Try { inner, .. } => {
                let (inner_ty, id) = self.expr(declaration, inner, env, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 0,
                        child: Subject::Expr {
                            origin: inner.origin,
                        },
                    },
                    vec![],
                );
                match self.resolve(inner_ty) {
                    Ty::Result(ok, _) => (*ok, id),
                    found => {
                        if !matches!(found, Ty::Var(_)) {
                            self.conflict(
                                subject.clone(),
                                ConflictKind::TryNonResult { found },
                                vec![id],
                            );
                        }
                        (Ty::Error, id)
                    }
                }
            }
            ExprKind::Comprehension { pattern, yields } => {
                let mut nested = env.clone();
                self.pattern(declaration, pattern, &mut nested, resolver);
                let (row, evidence) = match yields {
                    Some(yielded) => {
                        let (ty, evidence) = self.expr(declaration, yielded, &nested, resolver);
                        self.fact(
                            Fact::ExprChild {
                                parent: subject.clone(),
                                ordinal: 0,
                                child: Subject::Expr {
                                    origin: yielded.origin,
                                },
                            },
                            vec![],
                        );
                        match ty {
                            Ty::Record(row) => (*row, evidence),
                            ty => (
                                Row::closed(vec![RowField {
                                    name: Ident::new("value"),
                                    ty,
                                }]),
                                evidence,
                            ),
                        }
                    }
                    None => (Row::closed(vec![]), NO_FACT),
                };
                (Ty::rel(row), evidence)
            }
            ExprKind::Call { func, args } => {
                self.call(expr.origin, declaration, func, args, env, resolver)
            }
            ExprKind::Let { name, value, body } => {
                let (value_ty, value_id) = self.expr(declaration, value, env, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 0,
                        child: Subject::Expr {
                            origin: value.origin,
                        },
                    },
                    vec![],
                );
                let mut inner = env.clone();
                inner.insert(name.clone(), (value_ty, value_id));
                let (body_ty, body_id) = self.expr(declaration, body, &inner, resolver);
                self.fact(
                    Fact::ExprChild {
                        parent: subject.clone(),
                        ordinal: 1,
                        child: Subject::Expr {
                            origin: body.origin,
                        },
                    },
                    vec![],
                );
                (body_ty, body_id)
            }
        };
        let ty = self.resolve(ty);
        let id = self.fact(
            Fact::HasType {
                subject,
                ty: ty.clone(),
                scope: ScopeId::root(),
            },
            vec![because],
        );
        (ty, id)
    }

    fn call(
        &mut self,
        origin: ExprOrigin,
        declaration: &Ident,
        func: &QualIdent,
        args: &[Expr],
        env: &Env,
        resolver: &impl SchemaResolver,
    ) -> (Ty, usize) {
        let subject = Subject::Expr { origin };
        let mut arg_types = Vec::new();
        let mut deps = Vec::new();
        for (ordinal, arg) in args.iter().enumerate() {
            let (ty, id) = self.expr(declaration, arg, env, resolver);
            self.fact(
                Fact::ExprChild {
                    parent: subject.clone(),
                    ordinal: ordinal as u32,
                    child: Subject::Expr { origin: arg.origin },
                },
                vec![],
            );
            arg_types.push(ty);
            deps.push(id);
        }
        let op_fact = self.fact(
            Fact::Applies {
                subject: subject.clone(),
                operator: func.to_string(),
                scope: ScopeId::root(),
            },
            deps.clone(),
        );
        self.fact(
            Fact::CallArity {
                subject: subject.clone(),
                argc: arg_types.len() as u32,
            },
            vec![],
        );
        if let Some(op) = func.to_string().strip_prefix("brix.ops.") {
            return (self.operator(subject, op, &arg_types, &deps), op_fact);
        }
        let candidates = resolver.functions(func);
        for (ordinal, sig) in candidates.iter().enumerate() {
            self.fact(
                Fact::FnArity {
                    function: func.to_string(),
                    ordinal: ordinal as u32,
                    paramc: sig.params.len() as u32,
                },
                vec![],
            );
        }
        if candidates.is_empty() {
            return (Ty::Error, op_fact);
        }
        let arity_ok: Vec<_> = candidates
            .iter()
            .filter(|sig| sig.params.len() == arg_types.len())
            .collect();
        if arity_ok.is_empty() {
            self.conflict(
                subject,
                ConflictKind::Arity {
                    expected: candidates[0].params.len() as u32,
                    found: arg_types.len() as u32,
                },
                deps,
            );
            return (Ty::Error, op_fact);
        }
        let mut matches = Vec::new();
        for sig in &arity_ok {
            if let Some(next) = solve::try_unify_args(&self.subst, &arg_types, &sig.params) {
                let mut score = 0i32;
                for (a, p) in arg_types.iter().zip(sig.params.iter()) {
                    let a = solve::resolve(&next, a.clone());
                    let p = solve::resolve(&next, p.clone());
                    if a == p {
                        score += 10;
                    } else if !matches!(p, Ty::Var(_)) {
                        score += 5;
                    }
                }
                matches.push((*sig, next, score));
            }
        }
        let Some((sig, _, _)) = ({
            match matches.len() {
                0 => None,
                1 => matches.pop(),
                _ => {
                    let best = matches.iter().map(|(_, _, s)| *s).max().unwrap_or(0);
                    let mut top: Vec<_> =
                        matches.into_iter().filter(|(_, _, s)| *s == best).collect();
                    if top.len() == 1 {
                        top.pop()
                    } else {
                        None
                    }
                }
            }
        }) else {
            let expect = arg_types.first().cloned().unwrap_or(Ty::Error);
            let found = arity_ok[0].params.first().cloned().unwrap_or(Ty::Error);
            // #15 native overload no-unique-winner: restate the direct-raise
            // Mismatch as a Fact so the native package re-derives it (the same
            // `expect`/`found`, both zonked identically to the conflict below).
            self.fact(
                Fact::OverloadNoWinner {
                    subject: subject.clone(),
                    expect: expect.clone(),
                    found: found.clone(),
                },
                deps.clone(),
            );
            self.conflict(
                subject,
                ConflictKind::Mismatch {
                    left: expect,
                    right: found,
                },
                deps,
            );
            return (Ty::Error, op_fact);
        };
        // #15 gap D fix: re-run the winner's arg unifications through the one
        // canonical `self.unify` entry point instead of wholesale-assigning the
        // trial substitution `next`. This reaches the identical subst as `next`
        // (same base self.subst — trials clone; same solve::step dispatch; same
        // left-to-right arg order) and can never emit a conflict the trial didn't
        // already rule out (a candidate only reaches this point if try_unify_args
        // returned Some — every arg unified with no occurs/mismatch/missing-field
        // failure). Unlike `self.subst = next`, this routes each bind through
        // `bind_ty`, so the vars the overload binds now emit BindAttempt (slice-7
        // occurs) and SubstEdge (slice-9 binding fixpoint) — the facts the old
        // wholesale assignment silently swallowed. MAINTENANCE: nothing may be
        // inserted between candidate selection above and this loop that reads
        // self.subst expecting it already post-call — the loop is what makes it so.
        for ((arg, param), dep) in arg_types.iter().zip(sig.params.iter()).zip(deps.iter()) {
            self.unify(subject.clone(), arg.clone(), param.clone(), vec![*dep]);
        }
        self.fact(
            Fact::FnSig {
                function: Ident::new(func.to_string()),
                params: sig.params.clone(),
                result: sig.ret.clone(),
                is_aggregate: sig.is_aggregate,
            },
            vec![],
        );
        (sig.ret.clone(), op_fact)
    }

    /// Dimension-vs-variable ruling: when one side of a same-dimension
    /// operator lacks ground dimensions, [`solve::same_dimension_step`]
    /// solves/unifies it rather than reporting a conflict. Mirrors
    /// `Infer::same_dimension` exactly (down to returning [`Ty::Error`] on
    /// a real conflict, not the stale left-hand operand) — the two must
    /// stay in lockstep since both are "observers" turning the same
    /// [`solve::DimStep`] into their own record of what happened, and this
    /// PR's parity harness (`crates/brix-ir/tests/parity.rs`) asserts
    /// their zonked/mirrored types agree even on a conflicting expression.
    fn same_dimension(
        &mut self,
        subject: Subject,
        operation: &str,
        a: &Ty,
        b: &Ty,
        deps: &[usize],
    ) -> Ty {
        if solve::dims(a).is_some() && solve::dims(b).is_some() {
            self.fact(
                Fact::DimSameOp {
                    subject: subject.clone(),
                    op: operation.to_owned(),
                    left: a.clone(),
                    right: b.clone(),
                },
                deps.to_vec(),
            );
        }
        match solve::same_dimension_step(a, b) {
            DimStep::Ok(t) => t,
            DimStep::Conflict => {
                self.conflict(
                    subject,
                    ConflictKind::Dimension {
                        op: operation.to_owned(),
                        left: a.clone(),
                        right: b.clone(),
                    },
                    deps.to_vec(),
                );
                Ty::Error
            }
            DimStep::Solve(x, y) => {
                self.unify(subject, x.clone(), y, deps.to_vec());
                x
            }
        }
    }

    fn operator(&mut self, subject: Subject, op: &str, args: &[Ty], deps: &[usize]) -> Ty {
        let want = if matches!(op, "not" | "neg") { 1 } else { 2 };
        if args.len() != want {
            self.conflict(
                subject,
                ConflictKind::Arity {
                    expected: want as u32,
                    found: args.len() as u32,
                },
                deps.to_vec(),
            );
            return Ty::Error;
        }
        match op {
            "add" | "sub" | "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
                let result = self.same_dimension(subject, op, &args[0], &args[1], deps);
                if matches!(op, "eq" | "ne" | "lt" | "le" | "gt" | "ge") {
                    Ty::Bool
                } else {
                    result
                }
            }
            "mul" | "div" => match solve::dimension_binary_step(&args[0], &args[1], op == "mul") {
                DimBinaryStep::Ok(t) => t,
                DimBinaryStep::Conflict => {
                    self.conflict(
                        subject,
                        ConflictKind::Dimension {
                            op: op.to_owned(),
                            left: args[0].clone(),
                            right: args[1].clone(),
                        },
                        deps.to_vec(),
                    );
                    Ty::Error
                }
                DimBinaryStep::Solve(x, y) => {
                    self.unify(subject, x.clone(), y, deps.to_vec());
                    x
                }
            },
            "and" | "or" => {
                for (ty, dep) in args.iter().zip(deps) {
                    self.unify(subject.clone(), Ty::Bool, ty.clone(), vec![*dep]);
                }
                Ty::Bool
            }
            "not" => {
                self.unify(subject, Ty::Bool, args[0].clone(), deps.to_vec());
                Ty::Bool
            }
            "neg" => args[0].clone(),
            _ => Ty::Error,
        }
    }
}

/// The dimension vector a resolved `Ty` denotes, if any — used by
/// [`Reflect::augment_structural_facts`] to emit [`Fact::DimTerm`]s for
/// `Money`/`Quantity` (single-term dimension vectors, via the same
/// `money_dimensions`/`quantity_dimensions` helpers `solve` uses) as well as
/// already-compound `Dimensioned` types. Judgment call: only the top-level
/// type is decomposed (not, say, a `Dimensioned` nested inside an `Option`),
/// matching the granularity `Fact::RowField`/`Fact::RowTail` use for rows.
fn ty_dimensions(ty: &Ty) -> Option<Dimensions> {
    match ty {
        Ty::Money(currency) => Some(money_dimensions(currency)),
        Ty::Quantity(measure) => Some(quantity_dimensions(measure)),
        Ty::Dimensioned(dims) => Some(dims.clone()),
        _ => None,
    }
}

fn lit_ty(lit: &Lit) -> Ty {
    match lit {
        Lit::Unit => Ty::Unit,
        Lit::Bool(_) => Ty::Bool,
        Lit::Int(_) => Ty::Int(IntWidth::Int),
        Lit::Str(_) => Ty::Str,
        Lit::F64Bits(_) => Ty::F64,
        Lit::Enum { ty, .. } => Ty::Enum(ty.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Expr, ExprKind, Severity};
    use crate::frontend::FnSignature;
    use crate::infer::{infer_source, TypeError};
    use crate::types::{dimensions_div, money_dimensions, quantity_dimensions};

    fn var(name: &str) -> Expr {
        Expr::new(Ty::Var(TyVar(9)), ExprKind::Var(Ident::new(name)))
    }
    fn op(name: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Ty::Var(TyVar(10)),
            ExprKind::Call {
                func: QualIdent::from(format!("brix.ops.{name}").as_str()),
                args,
            },
        )
    }

    #[test]
    fn reflective_pricing_conflict_agrees_with_bootstrap_checker_and_has_provenance() {
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
            yields: op(
                "add",
                vec![
                    op("div", vec![var("rate"), var("length")]),
                    var("surcharge"),
                ],
            ),
            result: Ty::Var(TyVar(11)),
        };
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![query],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        let mut bootstrap = source.clone();
        let errors = infer_source(&mut bootstrap, &crate::frontend::TableResolver::new());
        assert_eq!(report.conflicts.len(), 1, "{report:#?}");
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(error, TypeError::Dimension { .. }))
                .count(),
            1,
            "{errors:?}"
        );
        match &report.conflicts[0].kind {
            ConflictKind::Dimension { op, .. } => assert_eq!(op, "add"),
            other => panic!("expected a Dimension conflict, got {other:?}"),
        }
        assert!(!report.conflicts[0].because.is_empty());
    }

    #[test]
    fn reflective_query_result_and_unknown_field_match_bootstrap_rejections() {
        let record = Ty::record(Row::closed(vec![RowField {
            name: Ident::new("present"),
            ty: Ty::Int(IntWidth::Int),
        }]));
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![
                Query {
                    name: Ident::new("BadResult"),
                    params: vec![],
                    body: Pattern::default(),
                    yields: Expr::new(Ty::Int(IntWidth::Int), ExprKind::Lit(Lit::Int(1))),
                    result: Ty::rel(Row::closed(vec![RowField {
                        name: Ident::new("value"),
                        ty: Ty::Bool,
                    }])),
                },
                Query {
                    name: Ident::new("MissingField"),
                    params: vec![(Ident::new("record"), record)],
                    body: Pattern::default(),
                    yields: Expr::new(
                        Ty::Var(TyVar(12)),
                        ExprKind::Field {
                            base: var("record"),
                            field: Ident::new("absent"),
                        },
                    ),
                    result: Ty::Var(TyVar(13)),
                },
            ],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        let mut bootstrap = source.clone();
        let errors = infer_source(&mut bootstrap, &crate::frontend::TableResolver::new());
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict.kind, ConflictKind::Mismatch { .. })));
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict.kind, ConflictKind::UnknownField { .. })));
        assert!(errors
            .iter()
            .any(|error| matches!(error, TypeError::Mismatch { .. })));
        assert!(errors
            .iter()
            .any(|error| matches!(error, TypeError::UnknownField { .. })));
    }

    #[test]
    fn reflective_constraints_comprehensions_and_call_arity_are_checked() {
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![Constraint {
                name: Ident::new("Guard"),
                severity: Severity::Strict,
                body: Pattern::new(vec![Clause::When(Expr::new(
                    Ty::Int(IntWidth::Int),
                    ExprKind::Lit(Lit::Int(1)),
                ))]),
            }],
            queries: vec![
                Query {
                    name: Ident::new("Comp"),
                    params: vec![],
                    body: Pattern::default(),
                    yields: Expr::new(
                        Ty::Var(TyVar(14)),
                        ExprKind::Comprehension {
                            pattern: Pattern::default(),
                            yields: Some(Expr::new(
                                Ty::Int(IntWidth::Int),
                                ExprKind::Lit(Lit::Int(1)),
                            )),
                        },
                    ),
                    result: Ty::Var(TyVar(15)),
                },
                Query {
                    name: Ident::new("Arity"),
                    params: vec![],
                    body: Pattern::default(),
                    yields: Expr::new(
                        Ty::Var(TyVar(16)),
                        ExprKind::Call {
                            func: QualIdent::from("f"),
                            args: vec![],
                        },
                    ),
                    result: Ty::Var(TyVar(17)),
                },
            ],
        };
        let resolver = crate::frontend::TableResolver::new().with_function(FnSignature {
            name: QualIdent::from("f"),
            params: vec![Ty::Int(IntWidth::Int)],
            ret: Ty::Int(IntWidth::Int),
            effects: crate::effects::EffectRow::empty(),
            is_aggregate: false,
            may_diverge: false,
        });
        let report = analyze(&source, &resolver);
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict.kind, ConflictKind::NonBool { .. })));
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict.kind, ConflictKind::Arity { .. })));
        assert!(report
            .facts
            .iter()
            .any(|fact| matches!(fact.fact, Fact::HasType { ty: Ty::Rel(_), .. })));
    }

    #[test]
    fn reflective_unifier_solves_variables_detects_cycles_and_admits_open_rows() {
        let variable = TyVar(40);
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![Query {
                name: Ident::new("Solve"),
                params: vec![(Ident::new("x"), Ty::Var(variable))],
                body: Pattern::default(),
                yields: var("x"),
                result: Ty::rel(Row::closed(vec![RowField {
                    name: Ident::new("value"),
                    ty: Ty::Int(IntWidth::Int),
                }])),
            }],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        assert!(report.is_consistent(), "{report:#?}");
        assert!(report.facts.iter().any(|fact| matches!(
            fact.fact,
            Fact::HasType {
                ty: Ty::Int(IntWidth::Int),
                ..
            }
        )));

        let cycle_subject = Subject::Binding {
            declaration: Ident::new("Cycle"),
            name: Ident::new("x"),
        };
        let mut cycle = Reflect::default();
        cycle.unify(
            cycle_subject,
            Ty::Var(variable),
            Ty::option(Ty::Var(variable)),
            vec![],
        );
        assert!(cycle
            .draft_conflicts
            .iter()
            .any(|conflict| matches!(conflict.kind, ConflictKind::Occurs { .. })));

        let rows_subject = Subject::Binding {
            declaration: Ident::new("Rows"),
            name: Ident::new("r"),
        };
        let mut rows = Reflect::default();
        rows.unify(
            rows_subject,
            Ty::record(Row::open(
                vec![RowField {
                    name: Ident::new("x"),
                    ty: Ty::Int(IntWidth::Int),
                }],
                TyVar(41),
            )),
            Ty::record(Row::closed(vec![
                RowField {
                    name: Ident::new("x"),
                    ty: Ty::Int(IntWidth::Int),
                },
                RowField {
                    name: Ident::new("y"),
                    ty: Ty::Bool,
                },
            ])),
            vec![],
        );
        assert!(rows.draft_conflicts.is_empty());
    }

    /// The Probability↔F64 bridge (ruling: kept in both checkers) used to
    /// be entirely absent from `reflect.rs` — this exercises it directly so
    /// the two checkers cannot silently re-diverge on it.
    #[test]
    fn reflective_probability_f64_bridge_matches_bootstrap_checker() {
        let subject = Subject::Binding {
            declaration: Ident::new("Test"),
            name: Ident::new("x"),
        };
        let mut cx = Reflect::default();
        cx.unify(subject.clone(), Ty::Probability, Ty::F64, vec![]);
        assert!(cx.draft_conflicts.is_empty());

        let mut cx = Reflect::default();
        cx.unify(subject, Ty::F64, Ty::Probability, vec![]);
        assert!(cx.draft_conflicts.is_empty());
    }

    /// #15 PR5: `Estimate<T>` unified against its own plain payload type is
    /// a named [`ConflictKind::EpistemicErasure`], not a generic
    /// [`ConflictKind::Mismatch`] — the two checkers must agree it is the
    /// *erasure* category (see `parity.rs`'s `estimate_to_plain_erasure_agrees`
    /// for the full cross-checker fixture).
    #[test]
    fn reflective_estimate_to_plain_is_named_epistemic_erasure() {
        let subject = Subject::Binding {
            declaration: Ident::new("Test"),
            name: Ident::new("x"),
        };
        let mut cx = Reflect::default();
        cx.unify(
            subject,
            Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            Ty::Int(IntWidth::Int),
            vec![],
        );
        assert_eq!(cx.draft_conflicts.len(), 1);
        assert!(matches!(
            cx.draft_conflicts[0].kind,
            ConflictKind::EpistemicErasure { .. }
        ));
    }

    /// #15 PR5: `Missing<T>` must not silently coerce to `T` (conformance
    /// I.22.2) — same named erasure family as `Estimate<T>`/`Probability`.
    #[test]
    fn reflective_missing_to_plain_is_named_epistemic_erasure() {
        let subject = Subject::Binding {
            declaration: Ident::new("Test"),
            name: Ident::new("x"),
        };
        let mut cx = Reflect::default();
        cx.unify(subject, Ty::missing(Ty::Bool), Ty::Bool, vec![]);
        assert_eq!(cx.draft_conflicts.len(), 1);
        match &cx.draft_conflicts[0].kind {
            ConflictKind::EpistemicErasure { from, to } => {
                assert_eq!(*from, Ty::missing(Ty::Bool));
                assert_eq!(*to, Ty::Bool);
            }
            other => panic!("expected EpistemicErasure, got {other:?}"),
        }
    }

    /// #15 PR5: two `Missing<T>`s over the same payload type unify cleanly
    /// — `Missing<T>` is a real, unifiable type, not just an erasure trap.
    #[test]
    fn reflective_missing_of_equal_inner_type_unifies_cleanly() {
        let subject = Subject::Binding {
            declaration: Ident::new("Test"),
            name: Ident::new("x"),
        };
        let mut cx = Reflect::default();
        cx.unify(
            subject,
            Ty::missing(Ty::Bool),
            Ty::missing(Ty::Bool),
            vec![],
        );
        assert!(cx.draft_conflicts.is_empty());
    }

    /// Dimension-vs-variable ruling: a variable side must solve, not
    /// conflict, against a ground-dimensioned side.
    #[test]
    fn reflective_dimension_vs_variable_solves() {
        let km = Ty::Quantity(Ident::new("Kilometre"));
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![Query {
                name: Ident::new("Solve"),
                params: vec![
                    (Ident::new("a"), km.clone()),
                    (Ident::new("b"), Ty::Var(TyVar(50))),
                ],
                body: Pattern::default(),
                yields: op("add", vec![var("a"), var("b")]),
                result: Ty::rel(Row::closed(vec![RowField {
                    name: Ident::new("value"),
                    ty: km,
                }])),
            }],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        assert!(report.is_consistent(), "{report:#?}");
    }

    /// Option/Result descent ruling: `Option<?t> ~ Option<Int>` must solve
    /// `?t := Int`, not report a top-level mismatch.
    #[test]
    fn reflective_option_descent_solves_the_inner_variable() {
        let subject = Subject::Binding {
            declaration: Ident::new("Test"),
            name: Ident::new("x"),
        };
        let mut cx = Reflect::default();
        cx.unify(
            subject,
            Ty::option(Ty::Int(IntWidth::Int)),
            Ty::option(Ty::Var(TyVar(60))),
            vec![],
        );
        assert!(cx.draft_conflicts.is_empty());
        assert_eq!(cx.resolve(Ty::Var(TyVar(60))), Ty::Int(IntWidth::Int));
    }

    /// #15 PR3: `FactId` is a pure function of a fact's own content (subject,
    /// kind tag, and payload), so re-running `analyze` on identical input
    /// must export byte-identical facts, `because` sets, and conflicts — no
    /// dependence on allocation addresses, hash-map iteration, or any other
    /// run-to-run nondeterminism.
    #[test]
    fn golden_export_is_byte_identical_across_two_independent_analyze_runs() {
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
            yields: op(
                "add",
                vec![
                    op("div", vec![var("rate"), var("length")]),
                    var("surcharge"),
                ],
            ),
            result: Ty::Var(TyVar(11)),
        };
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![query],
        };
        let resolver = crate::frontend::TableResolver::new();

        fn export(report: &ReflectiveReport) -> Vec<u8> {
            let mut w = CanonWriter::new();
            w.write_uint(report.facts.len() as u64);
            for derivation in &report.facts {
                w.write_bytes(derivation.id.digest().as_bytes());
                write_fact(&derivation.fact, &mut w);
                w.write_uint(derivation.because.len() as u64);
                for because in &derivation.because {
                    w.write_bytes(because.digest().as_bytes());
                }
            }
            w.write_uint(report.conflicts.len() as u64);
            for conflict in &report.conflicts {
                write_conflict(conflict, &mut w);
                w.write_uint(conflict.because.len() as u64);
                for because in &conflict.because {
                    w.write_bytes(because.digest().as_bytes());
                }
            }
            w.finish()
        }

        let a = analyze(&source, &resolver);
        let b = analyze(&source, &resolver);
        assert!(!a.facts.is_empty());
        assert!(!a.conflicts.is_empty(), "fixture must exercise a conflict");
        // Structural facts actually landed, not just the derived HasType ones.
        assert!(a
            .facts
            .iter()
            .any(|d| matches!(d.fact, Fact::ExprKindIs { .. })));
        assert!(a
            .facts
            .iter()
            .any(|d| matches!(d.fact, Fact::DimTerm { .. })));
        assert_eq!(
            export(&a),
            export(&b),
            "identical source must export identical bytes"
        );
        assert_eq!(
            a, b,
            "identical source must export a structurally identical report"
        );
    }

    /// #15 PR3.5: `ScopeId::root()` is a constant, not a counter — stable
    /// across independent calls — and scope genuinely participates in
    /// `FactId`'s digest: two `HasType` facts identical except for `scope`
    /// must not collide into the same `FactId`. If this ever fails after a
    /// refactor, `scope` has been dropped from (or moved outside) the
    /// digested payload, silently re-introducing the world-aliasing that
    /// content-addressed scope exists to kill.
    #[test]
    fn scope_id_root_is_stable_and_genuinely_participates_in_fact_identity() {
        assert_eq!(
            ScopeId::root(),
            ScopeId::root(),
            "ScopeId::root() must be a well-known constant, stable across calls"
        );

        let subject = Subject::Binding {
            declaration: Ident::new("Test"),
            name: Ident::new("x"),
        };
        let root_fact = Fact::HasType {
            subject: subject.clone(),
            ty: Ty::Int(IntWidth::Int),
            scope: ScopeId::root(),
        };
        let mut w = CanonWriter::new();
        w.write_tag("some-other-scope");
        let other_scope = ScopeId(w.digest(Domain::Value));
        assert_ne!(
            other_scope,
            ScopeId::root(),
            "test fixture must exercise a genuinely different scope"
        );
        let other_scoped_fact = Fact::HasType {
            subject,
            ty: Ty::Int(IntWidth::Int),
            scope: other_scope,
        };

        assert_ne!(
            FactId::derive(&root_fact),
            FactId::derive(&other_scoped_fact),
            "facts identical except for scope must produce different FactIds \
             — otherwise scope is not really inside the digest"
        );
    }
}
