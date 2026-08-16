//! SLICE 2 of the native type-realization regime (ADR-0005 Stage 2).
//!
//! Native type inference as SOC realization: produces real `HasType` `Derived`
//! judgements through the SOC proof substrate with App/Lam typing, declarative
//! unification, and multi-step composed derivations.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical};
use brix_elaborate::{RealizesTree, TreeObj};
use brix_kernel::{
    ArithOperatorV1, ArithTypingInputV1, CoercionEdgeV1, CoercionKind, NumericResultTypeV1,
    NumericTypeNameV1,
};
use brix_semantic::{
    Authority, ConfigId, ContextId, GeneratorId, GeneratorRegistry, Judgement, Outcome, Realizes,
    Support, TreeDerivation,
};

use crate::tree_audit::audit_tree;

/// The `Bool` type, as a genuine two-variant sum rather than an opaque
/// constructor.
///
/// **Why a sum and not `Ty::Con("Bool")`.** `certify_exhaustive` resolves a
/// scrutinee's constructor set from `Ty::Sum` and returns `Unknown` for
/// anything else, so an opaque `Bool` could never have its coverage certified.
/// A boolean match is exactly where "did you handle both cases" is worth
/// certifying — it is the shape every rules engine is made of — so `Bool`
/// carries its two nullary variants and `match b { true => … false => … }`
/// goes through the ordinary sum path with no special case anywhere.
///
/// Variant order is declaration order, `false` then `true`, and is ABI: it
/// participates in the type's canonical identity.
pub fn bool_ty() -> Ty {
    Ty::Sum(
        "Bool".to_string(),
        vec![("false".to_string(), vec![]), ("true".to_string(), vec![])],
    )
}

/// The comparison operators, as a closed set with frozen ordinals.
///
/// Separate from [`ArithOp`] because the claims differ: an arithmetic operator
/// produces a number in the operands' own numeric type, while a comparison
/// produces a `Bool` regardless of what it compared. Sharing one operator
/// enum would put those two different result rules behind one tag.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl CmpOp {
    /// Frozen ordinal — ABI, never reordered.
    pub const fn ordinal(self) -> u64 {
        match self {
            CmpOp::Lt => 0,
            CmpOp::Le => 1,
            CmpOp::Gt => 2,
            CmpOp::Ge => 3,
            CmpOp::Eq => 4,
            CmpOp::Ne => 5,
        }
    }
}

/// Native representation of types in the type-realization regime (ADR-0005).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ty {
    Con(&'static str),
    Fn(Box<Ty>, Box<Ty>),
    Var(u32),
    Record(Vec<(String, Ty)>),
    Sum(String, Vec<(String, Vec<Ty>)>),
    /// A recursive type, `μX. body` (append-only ordinal 5). The `String` names
    /// the bound variable; occurrences of it inside `body` are [`Ty::RecVar`].
    ///
    /// **Equi-recursive, and self-contained on purpose.** The definition
    /// travels *inside* the type rather than in an environment, so `unfold`
    /// needs no declaration map and nothing has to thread one through
    /// `unify`/`check_coverage`. That is what makes `config List = Nil |
    /// Cons(Int, List)` expressible without changing every signature that
    /// touches a type.
    Rec(String, Box<Ty>),
    /// A bound occurrence of the enclosing [`Ty::Rec`]'s variable
    /// (append-only ordinal 6).
    RecVar(String),
}

impl Ty {
    /// One step of μ-unfolding: `μX. body` becomes `body[X := μX. body]`.
    /// Any other type is returned unchanged, so this is safe to call before
    /// inspecting a type's structure.
    ///
    /// Callers that read a type's *shape* — coverage, pattern binding, field
    /// projection — must unfold first, or a recursive type looks like a
    /// `Rec` with no variants and the check passes vacuously.
    pub fn unfold(&self) -> Ty {
        match self {
            Ty::Rec(var, body) => body.subst_rec(var, self),
            other => other.clone(),
        }
    }

    /// Replace every free `RecVar(var)` in `self` with `replacement`.
    /// Shadowing is respected: an inner `Rec` binding the same name stops the
    /// substitution, so nested recursive types cannot capture each other.
    fn subst_rec(&self, var: &str, replacement: &Ty) -> Ty {
        match self {
            Ty::RecVar(v) if v == var => replacement.clone(),
            Ty::RecVar(_) | Ty::Con(_) | Ty::Var(_) => self.clone(),
            Ty::Fn(a, b) => Ty::Fn(
                Box::new(a.subst_rec(var, replacement)),
                Box::new(b.subst_rec(var, replacement)),
            ),
            Ty::Record(fields) => Ty::Record(
                fields
                    .iter()
                    .map(|(n, t)| (n.clone(), t.subst_rec(var, replacement)))
                    .collect(),
            ),
            Ty::Sum(name, variants) => Ty::Sum(
                name.clone(),
                variants
                    .iter()
                    .map(|(n, fs)| {
                        (
                            n.clone(),
                            fs.iter().map(|f| f.subst_rec(var, replacement)).collect(),
                        )
                    })
                    .collect(),
            ),
            // Shadowed: an inner binder of the same name captures it.
            Ty::Rec(v, _) if v == var => self.clone(),
            Ty::Rec(v, body) => Ty::Rec(v.clone(), Box::new(body.subst_rec(var, replacement))),
        }
    }
}

impl Canonical for Ty {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Ty::Con(name) => {
                w.write_enum(0, |w| w.write_str(name));
            }
            Ty::Fn(param, ret) => {
                w.write_enum(1, |w| {
                    param.canon_write(w);
                    ret.canon_write(w);
                });
            }
            Ty::Var(v) => {
                w.write_enum(2, |w| w.write_uint(*v as u64));
            }
            Ty::Record(fields) => {
                w.write_enum(3, |w| {
                    let mut sorted = fields.clone();
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                    sorted.dedup_by(|a, b| a.0 == b.0);
                    w.write_uint(sorted.len() as u64);
                    for (name, ty) in &sorted {
                        w.write_str(name);
                        ty.canon_write(w);
                    }
                });
            }
            Ty::Rec(var, body) => {
                w.write_enum(5, |w| {
                    w.write_str(var);
                    body.canon_write(w);
                });
            }
            Ty::RecVar(var) => {
                w.write_enum(6, |w| w.write_str(var));
            }
            Ty::Sum(sum_name, variants) => {
                w.write_enum(4, |w| {
                    w.write_str(sum_name);
                    w.write_uint(variants.len() as u64);
                    for (vname, fields) in variants {
                        w.write_str(vname);
                        w.write_uint(fields.len() as u64);
                        for f in fields {
                            f.canon_write(w);
                        }
                    }
                });
            }
        }
    }
}

impl Ty {
    /// Content-addressed type identity (`ConfigId`).
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

/// Binary numeric arithmetic operators. All four share one typing rule
/// (numeric operands in, numeric result out) — the op only affects runtime
/// value, not the type — so they carry a single `g_arith` generator.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOp {
    fn ordinal(self) -> u64 {
        match self {
            ArithOp::Add => 0,
            ArithOp::Sub => 1,
            ArithOp::Mul => 2,
            ArithOp::Div => 3,
        }
    }

    /// This operator in the kernel's own vocabulary, for the
    /// [`ArithTypingInputV1`] source object (ADR-0015 §5 Stage B0).
    ///
    /// A total mapping written out arm by arm rather than by casting an
    /// ordinal: the two enums are frozen independently, and a numeric bridge
    /// would silently re-point every row in a future registry if either side
    /// ever appended a variant.
    fn kernel_operator(self) -> ArithOperatorV1 {
        match self {
            ArithOp::Add => ArithOperatorV1::Add,
            ArithOp::Sub => ArithOperatorV1::Sub,
            ArithOp::Mul => ArithOperatorV1::Mul,
            ArithOp::Div => ArithOperatorV1::Div,
        }
    }
}

/// Pattern representation for match expressions (ADR-0011 Slice 2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Ctor(String, Vec<Pattern>),
}

impl Canonical for Pattern {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Pattern::Wildcard => {
                w.write_enum(0, |_w| {});
            }
            Pattern::Var(name) => {
                w.write_enum(1, |w| w.write_str(name));
            }
            Pattern::Ctor(variant, subpats) => {
                w.write_enum(2, |w| {
                    w.write_str(variant);
                    w.write_uint(subpats.len() as u64);
                    for p in subpats {
                        p.canon_write(w);
                    }
                });
            }
        }
    }
}

/// Expression AST for the native type-realization regime (ADR-0005).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Expr {
    Lit(i64),
    Var(String),
    App(Box<Expr>, Box<Expr>),
    Lam(String, Box<Expr>),
    Record(Vec<(String, Expr)>),
    Field(Box<Expr>, String),
    /// String literal (append-only ordinal 6). Types to `Str`.
    StrLit(String),
    /// Floating-point literal, stored as its source text for canonical identity
    /// (append-only ordinal 7). Types to `Float`.
    FloatLit(String),
    /// Binary numeric arithmetic (append-only ordinal 8). Types to `Int`,
    /// `Float`, or — on a mixed `Int`/`Float` — `Float` via a promotion witness.
    Arith(ArithOp, Box<Expr>, Box<Expr>),
    /// Sum constructor application (append-only ordinal 9).
    Ctor(Ty, String, Vec<Expr>),
    /// Pattern match expression (append-only ordinal 10).
    Match(Box<Expr>, Vec<(Pattern, Expr)>),
    /// Boolean literal (append-only ordinal 11). Types to `Bool`.
    BoolLit(bool),
    /// Comparison (append-only ordinal 12). Types to `Bool` whatever it
    /// compared.
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
}

impl Canonical for Expr {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Expr::Lit(val) => {
                w.write_enum(0, |w| w.write_int(*val));
            }
            Expr::Var(name) => {
                w.write_enum(1, |w| w.write_str(name));
            }
            Expr::App(f, x) => {
                w.write_enum(2, |w| {
                    f.canon_write(w);
                    x.canon_write(w);
                });
            }
            Expr::Lam(param, body) => {
                w.write_enum(3, |w| {
                    w.write_str(param);
                    body.canon_write(w);
                });
            }
            Expr::Record(fields) => {
                w.write_enum(4, |w| {
                    let mut sorted = fields.clone();
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                    sorted.dedup_by(|a, b| a.0 == b.0);
                    w.write_uint(sorted.len() as u64);
                    for (name, val) in &sorted {
                        w.write_str(name);
                        val.canon_write(w);
                    }
                });
            }
            Expr::Field(base, fname) => {
                w.write_enum(5, |w| {
                    base.canon_write(w);
                    w.write_str(fname);
                });
            }
            Expr::StrLit(s) => {
                w.write_enum(6, |w| w.write_str(s));
            }
            Expr::FloatLit(s) => {
                w.write_enum(7, |w| w.write_str(s));
            }
            Expr::Arith(op, a, b) => {
                w.write_enum(8, |w| {
                    w.write_uint(op.ordinal());
                    a.canon_write(w);
                    b.canon_write(w);
                });
            }
            Expr::BoolLit(b) => {
                w.write_enum(11, |w| w.write_bool(*b));
            }
            Expr::Cmp(op, a, b) => {
                w.write_enum(12, |w| {
                    w.write_uint(op.ordinal());
                    a.canon_write(w);
                    b.canon_write(w);
                });
            }
            Expr::Ctor(sum_ty, variant, args) => {
                w.write_enum(9, |w| {
                    sum_ty.canon_write(w);
                    w.write_str(variant);
                    w.write_uint(args.len() as u64);
                    for a in args {
                        a.canon_write(w);
                    }
                });
            }
            Expr::Match(scrutinee, arms) => {
                w.write_enum(10, |w| {
                    scrutinee.canon_write(w);
                    w.write_uint(arms.len() as u64);
                    for (p, b) in arms {
                        p.canon_write(w);
                        b.canon_write(w);
                    }
                });
            }
        }
    }
}

impl Expr {
    /// Content-addressed expression identity (`ConfigId`).
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

/// Typing-rule generator for literal constants (`"type.rule.lit@1"`).
pub fn g_lit() -> GeneratorId {
    GeneratorId::named("type.rule.lit@1")
}

/// Typing-rule generator for string literals (`"type.rule.strlit@1"`).
pub fn g_str_lit() -> GeneratorId {
    GeneratorId::named("type.rule.strlit@1")
}

/// Typing-rule generator for variable lookups (`"type.rule.var@1"`).
pub fn g_var() -> GeneratorId {
    GeneratorId::named("type.rule.var@1")
}

// `g_app` ("type.rule.app@1") and `g_lam` ("type.rule.lam@1") were removed
// here. Neither was ever emitted as a leaf — the application and abstraction
// rules emit `g_app2`, `g_lam_intro` and `g_lam_body` — so both existed only in
// `minted_generators()`, and therefore only in `typing_registry()`, declaring
// generators inference could not produce. See `unify` for the same drift and
// the reason it is worth removing rather than explaining: a declared `𝒢` wider
// than what inference can emit is a fence around nothing.

/// Typing-rule generator for lambda abstraction introduction (`"type.rule.lam.intro@1"`).
pub fn g_lam_intro() -> GeneratorId {
    GeneratorId::named("type.rule.lam.intro@1")
}

/// Typing-rule generator for lambda abstraction closure (`"type.rule.lam.close@1"`).
pub fn g_lam_close() -> GeneratorId {
    GeneratorId::named("type.rule.lam.close@1")
}

/// Typing-rule generator for application splitting (`"type.rule.app.split@1"`).
pub fn g_split() -> GeneratorId {
    GeneratorId::named("type.rule.app.split@1")
}

/// Typing-rule generator for binary application (`"type.rule.app@2"`).
pub fn g_app2() -> GeneratorId {
    GeneratorId::named("type.rule.app@2")
}

/// Typing-rule generator for record literals (`"type.rule.record@1"`).
pub fn g_record() -> GeneratorId {
    GeneratorId::named("type.rule.record@1")
}

/// Typing-rule generator for zero-field record literals (`"type.rule.record.empty@1"`).
///
/// **Discharged tight for typing** on literal-introduction grounds — the same
/// family as [`g_lit`], not the structural product family.
///
/// This doc previously held it out because "the current kernel profile has
/// binary products but no terminal/unit proposition, so `{}` is not an instance
/// of product introduction yet." That is true and it was the wrong test. It
/// looks for a *kernel correspondence*, which is what discharges `g_record` and
/// `g_field` (product introduction and projection, pinned by
/// `structural_generators_are_faithful_kernel_rules`). The literal introduction
/// rules are discharged on a different ground and need no kernel rule:
/// `g_lit` is tight because "an introduction rule *is* the definition of its
/// type", and the kernel has no Int-introduction rule either — `Ty::Con("Int")`
/// is an opaque atom to it.
///
/// `{}` is that same shape, and more sharply so:
///
/// - **Zero premises.** There are no field derivations to compose, so there is
///   nothing for a composition rule to check. This is why it is not product
///   introduction: not because the kernel is missing a unit rule, but because
///   the judgement has no premises to introduce from.
/// - **Exactly one instance.** `src` is always `Atom(Expr(Record([])))` and
///   `dst` always `Atom(Type(Record([])))`. The relation is a single pair, and
///   `zero_arity_intro_generators_are_faithful` pins it.
/// - **Definitional.** `{} : {}` is what the empty record type *means*. There
///   is no host computation, no lookup, and no choice to distrust under
///   ADR-0015 §8.5.
///
/// Not discharged for any other judgment kind (⟨D-JUDGE⟩): this says nothing
/// about a value, and `brix-canon` is not asked for one.
///
/// **This discharge is an interim, and a strictly stronger one is available.**
/// ADR-0025 §1 retired the reason a kernel-checked relation seemed out of reach
/// here — the kernel needs an endpoint's *digest*, not its encoder — so under
/// ⟨D-PINNED⟩ this is the easiest possible case: a **one-row** relation over two
/// pinned constants, each carrying a ⟨D-REDERIVE⟩ manifest entry and a
/// re-derivation test in this crate. That would replace a prose discharge with
/// a membership decision the kernel executes, which is what ADR-0015 §1.2 wants
/// for everything. Recommended for ADR-0025 Stage B; see [`g_ctor_nullary`],
/// which cannot follow because its row set is unbounded.
pub fn g_record_empty() -> GeneratorId {
    GeneratorId::named("type.rule.record.empty@1")
}

/// Typing-rule generator for record field access (`"type.rule.field@1"`).
pub fn g_field() -> GeneratorId {
    GeneratorId::named("type.rule.field@1")
}

/// Typing-rule generator for field-expression decomposition (`"type.rule.field.split@1"`).
pub fn g_field_split() -> GeneratorId {
    GeneratorId::named("type.rule.field.split@1")
}

/// Typing-rule generator for record splitting (`"type.rule.record.split@1"`).
pub fn g_record_split() -> GeneratorId {
    GeneratorId::named("type.rule.record.split@1")
}

/// Typing-rule generator for sum constructor application (`"type.rule.ctor@1"`).
pub fn g_ctor() -> GeneratorId {
    GeneratorId::named("type.rule.ctor@1")
}

/// Typing-rule generator for nullary sum constructors (`"type.rule.ctor.nullary@1"`).
///
/// **Discharged tight for typing** on literal-introduction grounds, for the
/// reason set out on [`g_record_empty`] — the old holdout ("there is no payload
/// proof to inject, and the kernel profile has no nullary/zero coproduct
/// introduction rule") asked for a kernel correspondence, which is the test the
/// *structural* family has to pass, not this one. Having no payload to inject
/// is exactly what makes it a zero-premise introduction rather than a defective
/// coproduct introduction.
///
/// It differs from [`g_record_empty`] in one way that matters. `{}` has a
/// single instance with fixed endpoints; this generator ranges over every
/// user-declared sum type, its `dst` is *projected out of* its `src`, and its
/// leaf carries a precondition the host checked — that `variant` is declared by
/// `sum_ty` with zero fields.
///
/// **That precondition is not verifiable from the leaf.** An earlier version of
/// this doc claimed a checker reading `src` and `dst` could confirm it, because
/// `Expr::Ctor` carries the sum type inline with its variant list. That is
/// wrong for the reason ADR-0025 §1 makes central: a leaf's endpoints are
/// `PropositionId` **digests**, not structures, and a digest does not
/// decompose. What actually holds the claim up is that the precondition is
/// enforced *before emission* — an unknown variant or a nonzero arity is a
/// `TypeError` and no leaf is produced — so every leaf that exists is one whose
/// precondition held.
///
/// That is the same standard the other unbounded discharges meet: `g_var` and
/// `g_ctor` are tight over unboundedly many instances on the strength of their
/// emission being faithful, pinned by a test rather than checked per-leaf. The
/// difference from `g_lit` is honest and worth naming: `g_lit`'s pin is a fixed
/// endpoint pair and is exhaustive, while this one is a *property* —
/// `zero_arity_intro_generators_are_faithful` checks that `dst` is always
/// exactly the `sum_ty` named in `src` across several distinct sums, and that a
/// bogus variant or a nullary spelling of a payload variant yields no
/// derivation at all. A property tested by sampling, not an exhaustive pin.
///
/// **Why not a kernel-checked relation instead.** Not for ADR-0023 §4.1's
/// endpoint-encoding reason — ADR-0025 §1 retired that: the kernel needs the
/// endpoint's *digest*, not its encoder, so ⟨D-PINNED⟩ makes the row route
/// available in general. It is unavailable *here* for a different and more
/// durable reason: the row set would need one row per (sum type, nullary
/// variant) pair over user-declared sums, which is unbounded, and ⟨D-PRIM⟩
/// requires finite exact rows with §8.9 forbidding wildcard rows.
/// [`g_record_empty`] is not subject to that, and should move.
pub fn g_ctor_nullary() -> GeneratorId {
    GeneratorId::named("type.rule.ctor.nullary@1")
}

/// Typing-rule generator for constructor splitting (`"type.rule.ctor.split@1"`).
pub fn g_ctor_split() -> GeneratorId {
    GeneratorId::named("type.rule.ctor.split@1")
}

/// Typing-rule generator for match expressions (`"type.rule.match@1"`).
pub fn g_match() -> GeneratorId {
    GeneratorId::named("type.rule.match@1")
}

/// Typing-rule generator for a top-level wildcard/variable catch-all match arm
/// (`"type.rule.match.catchall@1"`).
///
/// This remains undischarged. A catch-all arm can stand for several coproduct
/// branches, but the current realization tree records that arm only once; it
/// therefore is not yet the kernel's explicit `Case` premise structure.
pub fn g_match_catchall() -> GeneratorId {
    GeneratorId::named("type.rule.match.catchall@1")
}

/// Typing-rule generator for match arm splitting (`"type.rule.match.split@1"`).
pub fn g_match_split() -> GeneratorId {
    GeneratorId::named("type.rule.match.split@1")
}

/// Typing-rule generator for float literals (`"type.rule.floatlit@1"`).
pub fn g_float_lit() -> GeneratorId {
    GeneratorId::named("type.rule.floatlit@1")
}

/// Typing-rule generator for binary numeric arithmetic (`"type.rule.arith@1"`).
pub fn g_arith() -> GeneratorId {
    GeneratorId::named("type.rule.arith@1")
}

/// Typing-rule generator for arithmetic operand splitting (`"type.rule.arith.split@1"`).
/// Typing-rule generator for a boolean literal (`"type.rule.bool.lit@1"`).
///
/// Discharged tight on the same grounds as [`g_lit`], [`g_str_lit`] and
/// [`g_float_lit`]: under ADR-0015 ⟨D-JUDGE⟩ it establishes `HasType(true,
/// Bool)` and nothing else. It asserts no operation, representation, or value
/// semantics, so there is nothing further for it to be capped by.
pub fn g_bool_lit() -> GeneratorId {
    GeneratorId::named("type.rule.bool.lit@1")
}

/// Structural split of a comparison node (`"type.rule.cmp.split@1"`).
///
/// Discharged on the same grounds as the other split generators (ADR-0015
/// ⟨D-SPLIT⟩): the claim is purely structural — a comparison node contains
/// these two ordered subexpressions, and typing it yields those two child
/// obligations in the same context. It selects no coercion and synthesises no
/// result type.
pub fn g_cmp_split() -> GeneratorId {
    GeneratorId::named("type.rule.cmp.split@1")
}

/// Typing-rule generator for a comparison (`"type.rule.cmp@1"`).
///
/// **Deliberately NOT discharged.** Like `g_arith` it asserts an operation
/// semantics — that these operand types are comparable under this operator,
/// and that the result is `Bool` — which is the class of claim ADR-0015 §8.5
/// declines to trust because the host computed it. A comparison therefore
/// types at `@Audited`, and discharging it would need the same ⟨D-PRIM⟩
/// kernel-relation treatment `g_arith` is receiving.
///
/// **The result being a plain `Bool` loses nothing** (ADR-0010 ⟨D-OPARROW⟩).
/// An operation is an *arrow*, not a configuration: `Bool` is the endpoint,
/// and the reason it holds is carried by the judgement rather than by the
/// result's type — `brix why` renders this leaf and its undischarged status.
/// There is deliberately no proposition-valued twin of this generator.
///
/// Note also what the grade means here: `@Audited` grades the **typing**
/// derivation, never the truth of the comparison.
pub fn g_cmp() -> GeneratorId {
    GeneratorId::named("type.rule.cmp@1")
}

pub fn g_arith_split() -> GeneratorId {
    GeneratorId::named("type.rule.arith.split@1")
}

/// Typing-rule generator packaging an arithmetic node's operand types into the
/// [`ArithTypingInputV1`] source object (`"type.rule.arith.input@1"`,
/// ADR-0015 §5 Stage B0).
///
/// **Why this leaf exists.** Stage B0 requires `g_arith`'s source object to be
/// the single `ArithTypingInputV1` atom carrying the operator, the original
/// operand types, and the promotion paths. A `TreeObj::Atom` can never equal a
/// `TreeObj::Prod`, and the node feeding `g_arith` is the operand `Tensor`,
/// whose `dst` is structurally always a `Prod` — so without a bridge the `Seq`
/// middle no longer matches and `TreeDerivation::verify_structure` rejects the
/// tree as malformed.
///
/// The bridge could not be folded into `g_arith_split`: ADR-0015 ⟨D-SPLIT⟩
/// discharges the split on the grounds that it is *purely structural*, and
/// states that "if `g_arith_split` ever selects a promotion, synthesises a
/// result type, or filters operations by unchecked host logic, those parts
/// inherit `g_arith`'s evidence burden and the discharge lapses." Stage C's
/// discharge depends on that staying true, so the promotion selection lives
/// here instead.
///
/// **Not discharged**, and deliberately so: this leaf asserts that the
/// operator and promotion paths the host chose for this node are the right
/// ones, which is exactly the kind of host-computed normalization ADR-0015 §8.5
/// says is not trusted.
///
/// **Nor is it dischargeable by Stage B's mechanism, and that is the finding
/// Stage B surfaced.** Its `src` is `Prod(Atom(Type(a)), Atom(Type(b)))` — a
/// product of *this crate's* `Ty` atoms. A registry row is matched by canonical
/// bytes, so authoring a row for it would require the kernel to reproduce `Ty`'s
/// encoding: a second semantic encoder for a type the TCB does not own. So the
/// earlier note here — that this is "dischargeable as its own kernel primitive,
/// so Stage D needs two relations rather than one" — was too optimistic. A
/// second relation is necessary but not sufficient; what is actually needed is
/// a ruling on **who owns the canonical encoding of a realization endpoint that
/// both the TCB and a regime must name**. See [`g_arith_result`], the bridge on
/// the way out, which shares that obstruction and nothing else.
///
/// **And a shared encoder would still not be enough here — this leaf carries a
/// second obstruction that [`g_arith_result`] does not.** `g_arith_result` is a
/// total injective renaming; give both sides one encoder and it dissolves. This
/// leaf is not a renaming. It also selects the promotion paths and asserts the
/// operator — and *nothing kernel-binds the operator to the expression being
/// typed*. [`g_arith_split`]'s `src` is `Atom(Expr(e))`, with the operator right
/// there inside the expression, but its `dst` is `Prod(Atom(Expr a),
/// Atom(Expr b))` and the operator is gone. This leaf's `src` then carries two
/// types and no operator, while its `dst` carries `op`. The `Seq` chain matches
/// on endpoints, so the operator enters the derivation only *here*, through an
/// undischarged leaf.
///
/// The consequence is concrete: a relation keyed on this `src` would be
/// **non-functional in the operator** — one canonical `src`, four distinct
/// `dst`s — which violates the build-time invariant ADR-0015 §5 Stage B requires
/// of every relation. The kernel could check the promotion paths are right for
/// *some* operator; it could never check the operator is the right one for this
/// node. Carrying the operator forward through the split in kernel-owned
/// vocabulary is what ADR-0025 must settle alongside the encoder; ADR-0023 §4.4
/// records the ruling and why no work builds on that line yet.
pub fn g_arith_input() -> GeneratorId {
    GeneratorId::named("type.rule.arith.input@1")
}

/// Typing-rule generator bridging the kernel's arithmetic result object back to
/// this crate's own type vocabulary (`"type.rule.arith.result@1"`,
/// ADR-0015 §5 Stage B).
///
/// **Why this leaf exists.** Stage B makes `g_arith` a kernel-checked primitive,
/// which requires the kernel to author both endpoints of every registry row.
/// It can only do that for schemas it owns, so `g_arith`'s `dst` became
/// [`NumericResultTypeV1`]. But the enclosing derivation — a `let` binding, a
/// function body, an annotation check — speaks in `Ty`, so something must carry
/// `NumericResultTypeV1(Int)` back to `Ty::Con("Int")`. That is this leaf.
///
/// **Not discharged.** The rename is faithful, but "the host renamed it
/// faithfully" is precisely the kind of claim §8.5 declines to trust, and the
/// kernel cannot check it for the same reason as [`g_arith_input`]: one endpoint
/// — here the `dst` — is a `Ty` atom this crate encodes.
///
/// That shared encoding obstruction is *all* the two bridges have in common,
/// and an earlier version of this doc's "mirror image" framing overstated it.
/// This leaf is a total injective renaming over a closed finite vocabulary, so
/// a single shared encoder dissolves it outright — the relation's destination
/// would simply *be* the `Ty` atom and this leaf would stop existing. Nothing
/// else is wrong with it. [`g_arith_input`] additionally selects the promotion
/// paths and asserts an operator the derivation never bound, so the same
/// encoder is necessary and not sufficient there. See its doc, and ADR-0023 §4.
///
/// **What that means for the arithmetic cap, stated without spin.** Before
/// Stage B, `1 + 2` was capped by two undischarged leaves (`g_arith_input`,
/// `g_arith`). After it, it is capped by two undischarged leaves
/// (`g_arith_input`, `g_arith_result`). No grade moves. What changed is
/// *which* claims are outstanding: the semantic one — that `Div`'s result rule
/// differs from the other three operators' — is now a fact the kernel decides
/// by exact membership, and the residue is two vocabulary renamings that assert
/// nothing about arithmetic. ADR-0015 §5 Stage D's headline gate
/// (`let x = 1 + 2` reaching `@Proven`) is therefore **not reachable by
/// discharging `g_arith` alone**, and that is a gap in the ADR rather than in
/// this implementation: the ADR was written before Stage B0 introduced the
/// first of these bridges. Reported in full, with options, in ADR-0023 §4.
pub fn g_arith_result() -> GeneratorId {
    GeneratorId::named("type.rule.arith.result@1")
}

// ---------------------------------------------------------------------------
// Honest outcome propagation (the SOC tight-generator obligation, ADR-0009/0010).
//
// The proof kernel certifies the *composition* theorem: given the primitive
// typing-rule leaves as generators, the derivation establishes `e : T`. It does
// NOT yet prove the semantic validity of those leaves themselves. In SOC an
// outcome is monotone over composition — a judgement is only as strong as its
// weakest generator — so the honest status of the typing *result* is the meet of
// the composition outcome and every leaf generator's discharge status. Until a
// leaf is discharged to "tight" (a kernel proof of its realization semantics),
// it caps the result at the replay-verified `Audited`. Discharging generators
// one at a time then lifts real programs to `Proven` automatically, with no
// change to this reporting code.
// ---------------------------------------------------------------------------

/// The judgment (proposition) kind a tightness query is scoped to (ADR-0015
/// ⟨D-JUDGE⟩).
///
/// A generator discharge is sound only **relative to the judgment it was
/// established for** — discharging `g_var` for typing shows `Hyp` is a sound
/// typing rule; it says nothing about an evaluation, equality, or totality
/// claim that might one day use the same generator id. Before this type
/// existed the scoping held only *by absence*: no evaluation claim kind lived
/// in the workspace, so nothing could misuse [`generator_is_tight`] — the same
/// shape as the declared-but-unreachable-variant hazard tracked by #254. This
/// enum converts that invariant from "true because nothing violates it" to
/// "true by construction."
///
/// **Deliberately closed.** An unrecognized claim kind must never silently
/// fall back to another kind's discharges — that is exactly the failure mode
/// ADR-0015 §9 rules out ("closed is safer now: an unknown kind cannot
/// silently default to 'discharged'"). Concretely: adding a variant here for
/// a future judgment (e.g. evaluation, settlement, coverage) MUST wire it to
/// return `false` for every generator in [`generator_is_tight`] until a
/// registry is deliberately built and reviewed for that judgment — it must
/// never inherit [`ClaimKind::Typing`]'s answers by falling through a
/// wildcard match arm. [`ClaimKind::Empty`] exists precisely to make that
/// discipline checkable: it is the "no discharges" judgment kind, proven by
/// `claim_kind_typing_discharge_is_not_portable` below to disagree with
/// `Typing` for a generator that is tight there.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ClaimKind {
    /// The `HasType` judgment. The only claim kind with a populated
    /// tightness registry today.
    Typing,
    /// A judgment kind with **no discharges, by construction**. Not a
    /// placeholder to be filled in later — it exists so ⟨D-JUDGE⟩'s scoping
    /// invariant is enforced by the type system and a test, rather than
    /// holding only because no second claim kind happens to exist yet
    /// (ADR-0015 Stage A).
    Empty,
}

/// Whether a generator's *realization semantics* has been discharged to
/// "tight" for the given `kind` — its soundness established for **that
/// judgment only**, not merely asserted (the tight-generator obligation,
/// ADR-0015 ⟨D-JUDGE⟩). A generator tight for [`ClaimKind::Typing`] carries no
/// claim about any other judgment kind; see [`ClaimKind`]'s doc for why this
/// parameter exists and why the enum is closed.
///
/// For [`ClaimKind::Empty`] this always returns `false`, unconditionally, for
/// every generator — including the twenty-one discharged below.
///
/// For [`ClaimKind::Typing`], discharged:
///
/// 1. The **literal introduction rules** `g_lit`/`g_str_lit`/`g_float_lit` — an
///    introduction rule *is* the definition of its type (`42 : Int`), the
///    irreducible axiom a type theory rests on. Emission-pinned by
///    `literal_intro_generators_are_faithful`.
/// 2. The **simply-typed λ-calculus core** `g_var` (hypothesis), `g_lam_intro` /
///    `g_lam_close` (→I), `g_split` / `g_app2` (→E). These are the *defining
///    rules* of the STLC — likewise the axiomatic base of the type theory — and
///    each corresponds directly to a **primitive natural-deduction rule the
///    proof kernel already accepts as sound**: `g_var` ↔ `Hyp`, `g_lam_*` ↔
///    `Lam` (→I), `g_split`/`g_app2` ↔ product elimination + `App` (→E, i.e.
///    modus ponens `(A→B)×A → B`, a kernel theorem — see
///    `application_rule_is_a_kernel_theorem`). Discharging them is honest: their
///    realization *is* the kernel's own primitive, not a fresh assertion. As a
///    result the pure λ-calculus fragment (`id(42)` etc.) reaches genuine
///    `Proven`.
///
/// 3. The **structural product/coproduct fragment**: `g_record*`/`g_field*`
///    (product introduction/elimination) and `g_ctor*`/`g_match*` (coproduct
///    introduction/elimination). The `*_split` leaves are the canonical
///    structural packaging of an expression's premises into the same
///    right-nested product shape used by the kernel rule; they carry no
///    operation or representation claim of their own. Their emissions, and the
///    four corresponding kernel theorems, are pinned by
///    `structural_generators_are_faithful_kernel_rules`. Consequently records,
///    field projections, constructors, and well-typed matches made solely from
///    this fragment can earn `Proven`.
///    Top-level wildcard/variable catch-all matches are excluded: they emit the
///    distinct, undischarged `g_match_catchall` until their repeated branch
///    premises are represented explicitly.
/// 4. **`g_arith_split`** — on the same structural grounds as the `*_split`
///    leaves above, and **independently of `g_arith`** (ADR-0015 ⟨D-SPLIT⟩,
///    Stage C). Its claim is that an arithmetic node contains this operator and
///    these two ordered subexpressions, and that typing it yields those two
///    child obligations in the same context. The operator is bound because the
///    leaf's `src` is the whole node's configuration; the children are bound
///    because they are its `dst`, in source order. It asserts nothing about
///    what arithmetic *means*.
///
///    **The discharge is conditional and the condition is executable.** It
///    holds only while the split stays purely structural: ⟨D-SPLIT⟩ states
///    that if `g_arith_split` "ever selects a promotion, synthesises a result
///    type, or filters operations by unchecked host logic, those parts inherit
///    `g_arith`'s evidence burden and the discharge lapses."
///    `arithmetic_split_rule_is_a_kernel_primitive` pins exactly that — most
///    directly by deriving the *same* expression under two contexts that force
///    different promotions and asserting the split leaf is byte-identical
///    across them. Stage B0 deliberately routed the promotion selection
///    through the separate `g_arith_input` bridge rather than the split, which
///    is what keeps this condition true.
///
///    Discharging the split while `g_arith` remains capped is safe: the
///    least-discharged leaf still caps the derivation, so `1 + 2` does not
///    move.
///
/// 5. The **zero-premise introductions** `g_record_empty` (`{} : {}`) and
///    `g_ctor_nullary` (`None : Opt`). Discharged on family 1's ground, not
///    family 3's: an introduction rule is the definition of its type, and with
///    no premises there is no composition for a kernel rule to check. Their
///    previous holdout reason — that Profile 1.2 has no terminal/unit or
///    nullary-coproduct rule — asked for the *correspondence* test that
///    `g_record`/`g_ctor` must pass, and having nothing to inject is precisely
///    what makes these introductions rather than defective eliminations. The
///    kernel has no Int-introduction rule either, and `g_lit` is tight.
///    Pinned by `zero_arity_intro_generators_are_faithful`, which checks that
///    each derives as a single leaf, that `g_record_empty`'s endpoints are
///    fixed, that `g_ctor_nullary`'s `dst` is always exactly the sum type
///    named inside its `src`, and that an undeclared variant or a wrong arity
///    produces no derivation at all.
///
/// Deliberately **NOT** discharged: `g_arith`, `g_arith_input`,
/// `g_arith_result`, and the coercion-edge promotions `g_promote_edge` (a real
/// numeric embedding). Any program touching an undischarged generator stays
/// `Audited`; e.g. `1 + 2` remains `Audited`.
///
/// None of these is held out for lack of a *value* semantics — ADR-0015
/// ⟨D-JUDGE⟩ resolved that: tightness is scoped to a proposition kind, and a
/// typing discharge never claims an evaluation equation (which is why
/// `g_float_lit` is honestly tight even though `brix-canon` excludes floats).
/// They are held out because their realization relation is not yet a fact the
/// kernel checks.
///
/// **Where Stage B leaves the arithmetic holdouts.** ADR-0015 §5 Stage B0
/// (landed) made `g_arith`'s source object carry every field that affects
/// admissibility; Stage B (landed) added the kernel's primitive-relation
/// registry, so `TypingArithV2` now decides `g_arith`'s realization by exact
/// membership over the finite `{Add,Sub,Mul,Div} × operand-types` matrix. But
/// **`generator_is_tight` is not the mechanism that consumes it** — ADR-0015 §5
/// Stage D is explicit that a boolean flip here would be too coarse to be
/// authoritative, because it would regrade certificates whose leaves are still
/// `Hyp`. A leaf is closed only when the certificate actually contains the
/// `PrimRealizes` term and the kernel accepted the resulting proof, which is
/// Stage D's work.
///
/// **And Stage D needs more than that.** `g_arith_input` and `g_arith_result`
/// are the regime↔kernel vocabulary bridges on either side of the kernel-checked
/// leaf. Neither is dischargeable by Stage B's mechanism, because each has one
/// endpoint that is a `Ty` atom this crate encodes and the kernel may not
/// reproduce (ADR-0015 §8.5; `DEPS.md`), and `g_arith_input` carries a second
/// obstruction besides — see its doc. So `let x = 1 + 2` reaching `@Proven` was
/// **never reachable by discharging `g_arith`**, however completely Stage B
/// succeeded.
///
/// That is now ruled on rather than merely reported. ADR-0015 §5 Stage D gate 1
/// is re-scoped in place to what the discharge actually buys — a kernel-checked
/// `g_arith` leaf inside an honestly-`@Audited` result — and the `@Proven` goal
/// moved out to its own work item under the endpoint-vocabulary line, which
/// ADR-0025 will pin. **So this list is expected to keep both bridges after
/// Stage D lands, and `1 + 2` is expected to stay `@Audited`.** A change that
/// makes it `@Proven` without removing those two leaves is a bug, not progress.
/// ADR-0023 §4 carries the finding and §4.4 the ruling.
pub fn generator_is_tight(kind: ClaimKind, g: &GeneratorId) -> bool {
    match kind {
        // Fail closed: a claim kind added in the future starts here, at
        // `false` for everything, until a registry is deliberately built and
        // reviewed for it (see `ClaimKind`'s doc). Never add a fall-through
        // wildcard arm that reuses `Typing`'s list.
        ClaimKind::Empty => false,
        ClaimKind::Typing => {
            *g == g_lit()
                || *g == g_str_lit()
                || *g == g_float_lit()
                || *g == g_var()
                || *g == g_lam_intro()
                || *g == g_lam_close()
                || *g == g_split()
                || *g == g_app2()
                || *g == g_record_split()
                || *g == g_record()
                || *g == g_field_split()
                || *g == g_field()
                || *g == g_ctor_split()
                || *g == g_ctor()
                || *g == g_match_split()
                || *g == g_match()
                || *g == g_arith_split()
                || *g == g_bool_lit()
                || *g == g_cmp_split()
                || *g == g_record_empty()
                || *g == g_ctor_nullary()
        }
    }
}

/// Whether every leaf generator in an elaborated derivation is discharged
/// tight **for typing** — the only judgment kind `honest_result_outcome`
/// speaks about (ADR-0015 ⟨D-JUDGE⟩; a future evaluation judgment gets its own
/// caller and its own, separately reviewed, claim kind).
fn all_generators_tight(tree: &RealizesTree) -> bool {
    match tree {
        RealizesTree::Leaf { generator, .. } => generator_is_tight(ClaimKind::Typing, generator),
        RealizesTree::Seq { left, right } | RealizesTree::Tensor { left, right } => {
            all_generators_tight(left) && all_generators_tight(right)
        }
    }
}

/// The honest epistemic status of a typing *result* `e : T`: the composition
/// outcome capped by the least-discharged leaf. The kernel proves `composition`
/// (e.g. `Proven`) *conditional on* the primitive typing-rule leaves, so the
/// result is only `composition` when every leaf is tight; otherwise it is the
/// replay-verified `Audited`. Consults the **typing** tightness index only
/// ([`ClaimKind::Typing`]) — this function is the `HasType` judgment's own
/// honesty check, not a general one (ADR-0015 ⟨D-JUDGE⟩).
pub fn honest_result_outcome(composition: Outcome, tree: &RealizesTree) -> Outcome {
    if all_generators_tight(tree) {
        composition
    } else {
        Outcome::Audited
    }
}

/// A short, human-readable name for a typing-rule generator (ADR-0010 L4:
/// `brix why`/`brix whynot`, issue #43). `GeneratorId` is a one-way content
/// hash (`GeneratorId::named`), so there is no way back from a digest to a
/// name in general; this is a reverse lookup over the *closed* set of
/// generators this regime is known to mint — exactly the set
/// `generator_is_tight` above classifies, plus the open-ended `NUMERIC`/
/// `GRADE` coercion edges. Returns `None` for a generator this module did not
/// mint (a caller should fall back to the raw digest).
pub fn generator_name(g: &GeneratorId) -> Option<String> {
    minted_generators()
        .into_iter()
        .find(|(_, id)| id == g)
        .map(|(name, _)| name)
}

/// The **single closed enumeration** of every generator this regime mints:
/// the named typing rules plus the open-ended `NUMERIC`/`GRADE` promotion
/// edges (ADR-0019 D5).
///
/// One list, two consumers — [`generator_name`]'s reverse lookup and
/// [`typing_registry`]'s membership set. They were separate before, which is
/// how the two could have drifted: a rule reachable by inference but absent
/// from the registry would fail audit, and one present in the registry but
/// never minted would widen `𝒢` silently.
fn minted_generators() -> Vec<(String, GeneratorId)> {
    let named: &[(&str, GeneratorId)] = &[
        ("g_lit", g_lit()),
        ("g_str_lit", g_str_lit()),
        ("g_float_lit", g_float_lit()),
        ("g_var", g_var()),
        ("g_lam_intro", g_lam_intro()),
        ("g_lam_close", g_lam_close()),
        ("g_split", g_split()),
        ("g_app2", g_app2()),
        ("g_record", g_record()),
        ("g_record_empty", g_record_empty()),
        ("g_field", g_field()),
        ("g_field_split", g_field_split()),
        ("g_record_split", g_record_split()),
        ("g_ctor", g_ctor()),
        ("g_ctor_nullary", g_ctor_nullary()),
        ("g_ctor_split", g_ctor_split()),
        ("g_match", g_match()),
        ("g_match_catchall", g_match_catchall()),
        ("g_match_split", g_match_split()),
        ("g_arith", g_arith()),
        ("g_arith_split", g_arith_split()),
        ("g_arith_input", g_arith_input()),
        ("g_arith_result", g_arith_result()),
        ("g_bool_lit", g_bool_lit()),
        ("g_cmp_split", g_cmp_split()),
        ("g_cmp", g_cmp()),
    ];

    let mut out: Vec<(String, GeneratorId)> = named
        .iter()
        .map(|(n, id)| ((*n).to_string(), *id))
        .collect();
    // The display name follows the edge's family, not a fixed word: after
    // ADR-0015 Stage E the lossy edge is not a promotion, and `brix why` should
    // not call it one.
    for (from, to, kind) in NUMERIC.edges.iter().copied() {
        let label = match kind {
            CoercionKind::Exact => format!("promote({from}->{to})"),
            CoercionKind::Lossy => format!("convert_lossy({from}->{to})"),
        };
        out.push((label, NUMERIC.promote_generator(from, to)));
    }
    for (from, to, _) in GRADE.edges.iter().copied() {
        out.push((
            format!("weaken({from}->{to})"),
            GRADE.promote_generator(from, to),
        ));
    }
    out
}

/// The registry `𝒢` of typing-rule generators this regime mints — the
/// membership set `TreeDerivation::verify_structure` checks each leaf
/// against (ADR-0019 D5).
///
/// Built from [`minted_generators`], the regime's own declared enumeration.
/// It is **never** assembled from the leaves of the tree being audited: that
/// would reduce membership to "every cited generator is among the cited
/// generators", which checks nothing.
pub fn typing_registry() -> GeneratorRegistry {
    let mut r = GeneratorRegistry::new();
    for (_, id) in minted_generators() {
        r.insert(id);
    }
    r
}

// ---------------------------------------------------------------------------
// Coercion lattices (ADR-0010): the general type-normalization mechanism.
//
// A `CoercionLattice` is a declared category of witnessed coercions over one
// *sort* of type name — objects = node names, morphisms = the *safe*
// (information-preserving) coercion generators on its edges, each `from ↪ to`.
// Normalization at a multi-input site = `join` (least upper bound in the ↪
// order) + the composed witness-path from each input up to the join. An edge is
// always the SAFE direction; a required coercion with NO up-path is a real
// conflict (numeric: incomparable types; epistemic: illegal strengthening =
// erasure). Only upward moves are ever inserted implicitly.
//
// The numeric tower and the epistemic-grade modality are two instances of the
// SAME code — `Int ↪ Float` and `Proven ↪ Audited` run through one mechanism.
// Up-paths are unique here (the coherence condition), so composed witnesses are
// well-defined; adding a class is a data change to the `edges`, not new logic.
// ---------------------------------------------------------------------------

/// A declared category of witnessed coercions over one sort of type name.
pub struct CoercionLattice {
    /// Hasse edges `(from, to, kind)` in the *safe* coercion direction. The
    /// kind selects which **generator family** names the edge, so an edge's
    /// exactness is lattice data rather than a special case inside a method.
    ///
    /// ADR-0015 Stage E ⟨D-PROMOTE⟩ is what forced the third component. Before
    /// it, exactness was decided by a hardcoded `match` on
    /// `("type.rule.num.promote", "Int", "Float")` — a lossy edge that had to be
    /// recognised by name *after* it had already been given a promotion
    /// generator id. Now the data says which family it belongs to and the id
    /// follows from that, so the two cannot disagree.
    edges: &'static [(&'static str, &'static str, CoercionKind)],
    /// Prefix for per-edge generator ids of **exact** edges, e.g.
    /// `"type.rule.num.promote"`.
    exact_prefix: &'static str,
    /// Prefix for per-edge generator ids of **lossy** edges. Distinct from
    /// [`Self::exact_prefix`] so that a generator id never asserts a promotion
    /// for a map that does not preserve numeric identity (ADR-0015
    /// ⟨D-PROMOTE⟩). `None` for a lattice with no lossy edges, where using one
    /// would be a bug rather than a policy choice.
    lossy_prefix: Option<&'static str>,
}

impl CoercionLattice {
    /// Whether `name` is a node of this lattice.
    fn contains(&self, name: &str) -> bool {
        self.edges.iter().any(|(a, b, _)| *a == name || *b == name)
    }

    /// The canonical `&'static str` node for `name`, if present.
    fn node(&self, name: &str) -> Option<&'static str> {
        self.edges.iter().find_map(|(a, b, _)| {
            if *a == name {
                Some(*a)
            } else if *b == name {
                Some(*b)
            } else {
                None
            }
        })
    }

    /// The witnessed-coercion generator for the edge `from ↪ to`.
    ///
    /// **The family follows the edge's kind** (ADR-0015 Stage E ⟨D-PROMOTE⟩).
    /// An exact edge is named under the promotion family; a lossy one is named
    /// under an explicitly-labelled conversion family, because "`Int→Float`
    /// SHALL NOT be discharged as an embedding or promotion, now or later" and
    /// an id reading `…num.promote.Int_Float…` asserts exactly that.
    ///
    /// An unknown pair falls back to the exact prefix. It is unreachable —
    /// every caller iterates real edges — and it is a fallback rather than a
    /// panic so that a future lattice edit cannot take down the type checker;
    /// but it deliberately does **not** invent a lossy id, since claiming
    /// lossiness for an edge the lattice does not declare would be its own
    /// false record.
    pub fn promote_generator(&self, from: &str, to: &str) -> GeneratorId {
        let prefix = match self.edge_kind(from, to) {
            CoercionKind::Lossy => self
                .lossy_prefix
                .expect("a lattice with a lossy edge must declare a lossy family"),
            CoercionKind::Exact => self.exact_prefix,
        };
        GeneratorId::named(&format!("{prefix}.{from}_{to}@1"))
    }

    /// Reflexive–transitive upward closure of `name` (includes `name`).
    fn ancestors(&self, name: &'static str) -> Vec<&'static str> {
        let mut out = vec![name];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i];
            for (a, b, _) in self.edges {
                if *a == cur && !out.contains(b) {
                    out.push(*b);
                }
            }
            i += 1;
        }
        out
    }

    /// `a ≤ b`: a value of type `a` safely coerces to `b`.
    fn le(&self, a: &'static str, b: &'static str) -> bool {
        self.ancestors(a).contains(&b)
    }

    /// The join (least upper bound), or `None` if incomparable — a real
    /// "cannot coerce" error (numeric mismatch or epistemic erasure).
    fn join(&self, a: &'static str, b: &'static str) -> Option<&'static str> {
        let aa = self.ancestors(a);
        let bb = self.ancestors(b);
        let common: Vec<&'static str> = aa.into_iter().filter(|x| bb.contains(x)).collect();
        common
            .iter()
            .copied()
            .find(|&x| common.iter().all(|&y| self.le(x, y)))
    }

    /// The ordered edge path `(from, to)` from `from` up to `to`
    /// (empty if equal). Up-paths are unique, so the composed witness is unique.
    fn edge_path(&self, from: &'static str, to: &'static str) -> Vec<(&'static str, &'static str)> {
        if from == to {
            return Vec::new();
        }
        let mut reached_via: BTreeMap<&'static str, (&'static str, &'static str)> = BTreeMap::new();
        let mut queue = vec![from];
        let mut i = 0;
        while i < queue.len() {
            let cur = queue[i];
            i += 1;
            for (a, b, _) in self.edges {
                if *a == cur && *b != from && !reached_via.contains_key(b) {
                    reached_via.insert(*b, (*a, *b));
                    if *b == to {
                        queue.clear();
                        break;
                    }
                    queue.push(*b);
                }
            }
        }
        let mut path = Vec::new();
        let mut node = to;
        while node != from {
            let edge = reached_via[node];
            path.push(edge);
            node = edge.0;
        }
        path.reverse();
        path
    }

    /// The witnessed coercion path `from ↪ … ↪ to` as canonical data — one
    /// [`CoercionEdgeV1`] per edge, in order, empty when `from == to`
    /// (ADR-0015 §5 Stage B0).
    ///
    /// This replaced a `coerce` method that spliced one embedding *leaf* per
    /// edge into the operand's derivation. Splicing was what erased the
    /// arithmetic source object: it left both operands presented at the result
    /// type, so the `g_arith` leaf could no longer say what they had started
    /// as. The path is now data inside [`ArithTypingInputV1`] instead, which is
    /// what makes `1.0 + 2.0` and `7 / 2` distinguishable.
    ///
    /// Each edge carries its own [`CoercionKind`] rather than inheriting one
    /// from the lattice: `NUMERIC` mixes the exact tower with the lossy
    /// `Int ↪ Float` branch, and ADR-0015 ⟨D-PROMOTE⟩ rules those to be
    /// different claims. Since Stage E the kind also selects the edge's
    /// generator *family*, so the id and the tag can no longer disagree.
    /// See [`CoercionLattice::edge_kind`].
    fn promotion_path(&self, from: &'static str, to: &'static str) -> Vec<CoercionEdgeV1> {
        self.edge_path(from, to)
            .into_iter()
            .map(|(a, b)| CoercionEdgeV1 {
                generator: self.promote_generator(a, b),
                kind: self.edge_kind(a, b),
            })
            .collect()
    }

    /// Whether the edge `from ↪ to` preserves numeric identity.
    ///
    /// **`Int ↪ Float` is lossy and is recorded as such.** `NUMERIC`'s own doc
    /// calls every one of its edges the "*safe* (information-preserving)"
    /// direction, but ADR-0015 ⟨D-PROMOTE⟩ rules that `Int→Float` "SHALL NOT
    /// be discharged as an embedding or promotion, now or later" — a lossy map
    /// is not injective and does not preserve numeric identity. Since `Div`
    /// routes integer division through `field_of("Int") == "Float"`, `7 / 2`
    /// travels that edge on both operands.
    ///
    /// ADR-0015 defines a promotion path as an ordered sequence of **exact**
    /// promotion-edge ids, so recording that edge unlabelled would encode a
    /// lossy conversion under a name asserting exactness.
    ///
    /// **Stage E finished the job B0 started.** B0 labelled the edge but left
    /// it named `type.rule.num.promote.Int_Float@1` — the tag said "lossy"
    /// while the id said "promote". This now reads the kind straight off the
    /// lattice edge, and [`Self::promote_generator`] derives the family from
    /// it, so the two are one fact rather than two that could drift. An edge
    /// the lattice does not declare is `Exact` by default; that is unreachable
    /// (every caller iterates real edges) and it fails toward the *weaker*
    /// claim, since inventing lossiness for an undeclared edge would be its own
    /// false record.
    fn edge_kind(&self, from: &str, to: &str) -> CoercionKind {
        // Deliberately keyed on the edge, not on a per-lattice flag: `NUMERIC`
        // carries exact and lossy edges side by side, so exactness is a
        // property of the edge and never of the lattice that contains it.
        self.edges
            .iter()
            .find(|(a, b, _)| *a == from && *b == to)
            .map(|(_, _, kind)| *kind)
            .unwrap_or(CoercionKind::Exact)
    }
}

/// The numeric coercion tower ℕ⊂ℤ⊂ℚ⊂ℝ⊂ℂ (safe = widening) plus the pragmatic
/// lossy `Int ↪ Float` branch — `Float` is incomparable to the exact ℚ/ℝ/ℂ
/// nodes, so `join(Float, Rat)` is `None` (mixing them is a type error).
///
/// **`Int ↪ Float` is declared under a different generator family** (ADR-0015
/// Stage E ⟨D-PROMOTE⟩: "`Int→Float` should move to an explicitly-labelled
/// lossy-conversion family rather than sitting in a lattice called `NUMERIC`'s
/// promotion edges"). The edge stays in the *coercion graph* — removing it
/// would change what typechecks, since `join(Int, Float)` and `Div`'s
/// `field_of` both depend on it — but it is no longer a member of the
/// promotion family, so no generator id asserts an embedding for it.
///
/// That reading is the one that moves no grade and no type. The stronger
/// reading — remove the edge from `NUMERIC` outright — would stop `1 + 1.0`
/// typing and leave integer `Div` with no result type, which is a language
/// change ⟨D-PROMOTE⟩ does not ask for and §5 Stage E does not scope. Both
/// readings, and why the narrow one was taken, are recorded in ADR-0024 §2.
pub static NUMERIC: CoercionLattice = CoercionLattice {
    edges: &[
        ("Nat", "Int", CoercionKind::Exact),
        ("Int", "Rat", CoercionKind::Exact),
        ("Rat", "Real", CoercionKind::Exact),
        ("Real", "Complex", CoercionKind::Exact),
        ("Int", "Float", CoercionKind::Lossy),
    ],
    exact_prefix: "type.rule.num.promote",
    lossy_prefix: Some("type.rule.num.convert.lossy"),
};

/// The epistemic-grade modality as a coercion lattice. The *safe* direction is
/// weakening certainty (`Proven ↪ Audited ↪ Derived` — a stronger guarantee may
/// always be forgotten). The forbidden strengthening (`Derived → Proven`
/// without evidence) has no up-path, so `join`/`le` reject it: that is exactly
/// **epistemic erasure**, caught by the same code as a numeric mismatch.
/// Every grade edge is `Exact`: forgetting a stronger guarantee loses
/// information about the *evidence*, but the weakening itself is exact — a
/// `Proven` result genuinely is `Audited`. `lossy_prefix` is `None` because a
/// lossy grade edge would not be a weakening at all, and minting an id for one
/// should be a panic rather than a silently-named generator.
pub static GRADE: CoercionLattice = CoercionLattice {
    edges: &[
        ("Proven", "Audited", CoercionKind::Exact),
        ("Audited", "Derived", CoercionKind::Exact),
    ],
    exact_prefix: "epistemic.grade.weaken",
    lossy_prefix: None,
};

/// Whether an *actual* epistemic grade satisfies an *asserted* one (both as
/// GRADE-lattice node names `"Derived"`/`"Audited"`/`"Proven"`).
///
/// A grade assertion is discharged iff the actual grade may **safely weaken** to
/// the assertion along the GRADE coercion lattice (`Proven ↪ Audited ↪
/// Derived`): you may assert a *weaker-or-equal* grade than you earned
/// (downgrade is free), but asserting a *stronger* grade has no up-path — that is
/// **epistemic erasure** and fails. An `actual` outside the lattice (e.g.
/// `Unknown`) satisfies nothing.
pub fn grade_assertion_satisfied(actual: &str, asserted: &str) -> bool {
    match (GRADE.node(actual), GRADE.node(asserted)) {
        (Some(a), Some(b)) => GRADE.le(a, b),
        // An unknown grade name (e.g. a non-grade outcome) satisfies nothing.
        _ => false,
    }
}

/// The least numeric *field* (closed under division) at or above `name`. The
/// exact field of fractions of `Nat`/`Int` is `Rat`; here it is `Float`, the
/// reachable representation (a fuller tower would return `Rat`).
fn field_of(name: &'static str) -> &'static str {
    match name {
        "Nat" | "Int" => "Float",
        other => other,
    }
}

/// The plan for typing a binary arithmetic node: where operands meet (`base`),
/// the result type (`base`, or its field of fractions for `Div`), each
/// operand's effective type, the witnessed coercion path lifting it to the
/// result type, and whether an operand was an unbound var to unify.
struct ArithPlan {
    base: &'static str,
    result: &'static str,
    eff_a: &'static str,
    eff_b: &'static str,
    /// The ordered coercion edges from `eff_a` up to `result`, empty when the
    /// operand is already there. Carried as data into [`ArithTypingInputV1`]
    /// rather than spliced into the derivation as leaves (ADR-0015 Stage B0).
    path_a: Vec<CoercionEdgeV1>,
    /// The ordered coercion edges from `eff_b` up to `result`.
    path_b: Vec<CoercionEdgeV1>,
    unify_a: bool,
    unify_b: bool,
}

impl ArithPlan {
    /// The [`ArithTypingInputV1`] source object this plan describes — every
    /// field that affects admissibility of the typing judgement, and nothing
    /// else (ADR-0015 §5 Stage B0).
    ///
    /// `lhs_type`/`rhs_type` are the operands' types **before** promotion,
    /// which is precisely what the pre-B0 leaf destroyed by presenting both
    /// operands already coerced to the result type.
    ///
    /// Fails closed with [`TypeError::Mismatch`] if an operand type is not a
    /// node of the numeric tower. `arith_operand` already rejects those, so
    /// this is unreachable today — and it is written as a refusal rather than
    /// an `unwrap` precisely because a future lattice edit that made it
    /// reachable must not be able to panic the type checker or invent a
    /// numeric type the kernel never agreed to.
    fn typing_input(&self, op: ArithOp) -> Result<ArithTypingInputV1, TypeError> {
        let numeric =
            |name: &str| NumericTypeNameV1::from_lattice_node(name).ok_or(TypeError::Mismatch);
        Ok(ArithTypingInputV1 {
            operator: op.kernel_operator(),
            lhs_type: numeric(self.eff_a)?,
            rhs_type: numeric(self.eff_b)?,
            lhs_promotion_path: self.path_a.clone(),
            rhs_promotion_path: self.path_b.clone(),
        })
    }
}

/// Classify a *resolved* operand type: `Ok(Some(node))` for a concrete numeric
/// type, `Ok(None)` for an unbound var (defaults to `base`), `Err(Mismatch)`
/// for a concrete non-numeric type (`Str`/`Fn`/`Record`).
fn arith_operand(resolved: &Ty) -> Result<Option<&'static str>, TypeError> {
    match resolved {
        Ty::Con(n) if NUMERIC.contains(n) => Ok(NUMERIC.node(n)),
        Ty::Var(_) => Ok(None),
        _ => Err(TypeError::Mismatch),
    }
}

/// Compute the arithmetic plan for `op` over resolved operand types `ra`, `rb`.
fn plan_arith(op: ArithOp, ra: &Ty, rb: &Ty) -> Result<ArithPlan, TypeError> {
    let na = arith_operand(ra)?;
    let nb = arith_operand(rb)?;
    let base = match (na, nb) {
        (Some(a), Some(b)) => NUMERIC.join(a, b).ok_or(TypeError::Mismatch)?,
        (Some(a), None) => a,
        (None, Some(b)) => b,
        // Both operands are unbound type vars: default the arithmetic to `Int`.
        (None, None) => "Int",
    };
    let result = if op == ArithOp::Div {
        field_of(base)
    } else {
        base
    };
    let eff_a = na.unwrap_or(base);
    let eff_b = nb.unwrap_or(base);
    Ok(ArithPlan {
        base,
        result,
        eff_a,
        eff_b,
        path_a: NUMERIC.promotion_path(eff_a, result),
        path_b: NUMERIC.promotion_path(eff_b, result),
        unify_a: na.is_none(),
        unify_b: nb.is_none(),
    })
}

/// Immutable typing context mapping variable names to types for variable lookup.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TyCtx(pub BTreeMap<String, Ty>);

impl TyCtx {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn extend(&self, var: impl Into<String>, ty: Ty) -> Self {
        let mut map = self.0.clone();
        map.insert(var.into(), ty);
        Self(map)
    }

    pub fn get(&self, var: &str) -> Option<&Ty> {
        self.0.get(var)
    }
}

/// Errors during type checking in slice 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    Unbound(String),
    Mismatch,
    InfiniteType,
    Unsupported,
    NoField(String),
    /// The built derivation tree is not well-formed (a `Seq` middle does not match),
    /// so it cannot be honestly labelled `Audited`. Arises when a leaf endpoint config
    /// captured at sub-inference time is later refined by unification (ADR-0007 §7 limitation).
    IllFormedDerivation,
    /// Non-exhaustive match expression with list of uncovered variant names.
    NonExhaustive(Vec<String>),
}

/// Binds pattern variables against a scrutinee type under substitution.
pub fn bind_pattern(
    pat: &Pattern,
    scrutinee_ty: &Ty,
    subst: &BTreeMap<u32, Ty>,
) -> Result<Vec<(String, Ty)>, TypeError> {
    match pat {
        Pattern::Wildcard => Ok(vec![]),
        Pattern::Var(x) => Ok(vec![(x.clone(), scrutinee_ty.clone())]),
        Pattern::Ctor(vname, subpats) => {
            // Unfolded so a recursive type's variants are visible; binding a
            // sub-pattern of `Cons(h, t)` is how `t` gets its type at all.
            let resolved = resolve(scrutinee_ty, subst).unfold();
            if let Ty::Sum(_sum_name, variants) = &resolved {
                let (_, declared_fields) = variants
                    .iter()
                    .find(|(name, _)| name == vname)
                    .ok_or(TypeError::Mismatch)?;
                if subpats.len() != declared_fields.len() {
                    return Err(TypeError::Mismatch);
                }
                let mut bindings = Vec::new();
                for (subp, f_ty) in subpats.iter().zip(declared_fields.iter()) {
                    match subp {
                        Pattern::Wildcard => {}
                        Pattern::Var(x) => {
                            bindings.push((x.clone(), zonk(f_ty, subst)));
                        }
                        Pattern::Ctor(_, _) => {
                            return Err(TypeError::Unsupported);
                        }
                    }
                }
                Ok(bindings)
            } else {
                Err(TypeError::Mismatch)
            }
        }
    }
}

/// Checks structural pattern-matrix coverage of match arms against a scrutinee type.
pub fn check_coverage(
    scrutinee_ty: &Ty,
    arms: &[(Pattern, Expr)],
    subst: &BTreeMap<u32, Ty>,
) -> Result<(), TypeError> {
    // Unfold first: a `Rec` has no variants of its own, so reading its shape
    // directly would leave `uncovered` empty and pass a non-exhaustive match
    // vacuously — accepting unsound code rather than rejecting it.
    let resolved = resolve(scrutinee_ty, subst).unfold();
    if let Ty::Sum(_sum_name, variants) = &resolved {
        let mut uncovered: Vec<String> = variants.iter().map(|(vname, _)| vname.clone()).collect();
        for (pat, _) in arms {
            match pat {
                Pattern::Wildcard | Pattern::Var(_) => {
                    uncovered.clear();
                }
                Pattern::Ctor(vname, _) => {
                    if !variants.iter().any(|(n, _)| n == vname) {
                        return Err(TypeError::Mismatch);
                    }
                    if let Some(pos) = uncovered.iter().position(|n| n == vname) {
                        uncovered.remove(pos);
                    }
                }
            }
        }
        if !uncovered.is_empty() {
            Err(TypeError::NonExhaustive(uncovered))
        } else {
            Ok(())
        }
    } else {
        Err(TypeError::Mismatch)
    }
}

/// Immutable inference state containing substitution map and fresh variable counter.
///
/// NOTE (ADR-0005): Unification threads plain `BTreeMap<u32, Ty>` without hashing or
/// `ConfigId` materialization inside the inference loop.
/// A persistent HAMT (e.g. `im::HashMap`) is the future performance path for cheap structural
/// sharing; `BTreeMap` clone-extend is used here for slice-2 correctness without external dependencies.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Infer {
    pub subst: BTreeMap<u32, Ty>,
    pub next_var: u32,
}

impl Infer {
    pub fn new() -> Self {
        Self {
            subst: BTreeMap::new(),
            next_var: 0,
        }
    }

    /// Returns a fresh type variable index and an updated `Infer` state with `next_var` incremented.
    pub fn fresh_var(&self) -> (u32, Self) {
        let v = self.next_var;
        let next_st = Self {
            subst: self.subst.clone(),
            next_var: v + 1,
        };
        (v, next_st)
    }
}

/// Resolves top-level type variable indirections in `ty` under `subst`.
pub fn resolve<'a>(ty: &'a Ty, subst: &'a BTreeMap<u32, Ty>) -> &'a Ty {
    let mut curr = ty;
    while let Ty::Var(v) = curr {
        if let Some(next) = subst.get(v) {
            curr = next;
        } else {
            break;
        }
    }
    curr
}

/// Fully resolves (zonks) a type, recursively replacing all bound variables.
pub fn zonk(ty: &Ty, subst: &BTreeMap<u32, Ty>) -> Ty {
    match resolve(ty, subst) {
        Ty::Con(name) => Ty::Con(name),
        Ty::Var(v) => Ty::Var(*v),
        // Zonked under the binder, never unfolded: unfolding here would not
        // terminate, and a `Rec` body contains no unification variables that
        // its own unfolding would resolve differently.
        Ty::Rec(var, body) => Ty::Rec(var.clone(), Box::new(zonk(body, subst))),
        Ty::RecVar(var) => Ty::RecVar(var.clone()),
        Ty::Fn(a, b) => Ty::Fn(Box::new(zonk(a, subst)), Box::new(zonk(b, subst))),
        Ty::Record(fields) => {
            let mut sorted: Vec<(String, Ty)> = fields
                .iter()
                .map(|(name, t)| (name.clone(), zonk(t, subst)))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            sorted.dedup_by(|a, b| a.0 == b.0);
            Ty::Record(sorted)
        }
        Ty::Sum(sum_name, variants) => {
            let zonked_vars = variants
                .iter()
                .map(|(vname, fields)| {
                    let z_fields = fields.iter().map(|f| zonk(f, subst)).collect();
                    (vname.clone(), z_fields)
                })
                .collect();
            Ty::Sum(sum_name.clone(), zonked_vars)
        }
    }
}

/// Occurs-check: checks if type variable `v` occurs free in `ty` under `subst`.
pub fn occurs(v: u32, ty: &Ty, subst: &BTreeMap<u32, Ty>) -> bool {
    match resolve(ty, subst) {
        Ty::Var(v2) => v == *v2,
        Ty::Con(_) => false,
        Ty::Fn(a, b) => occurs(v, a, subst) || occurs(v, b, subst),
        Ty::Record(fields) => fields.iter().any(|(_, t)| occurs(v, t, subst)),
        Ty::Sum(_, variants) => variants
            .iter()
            .any(|(_, fields)| fields.iter().any(|f| occurs(v, f, subst))),
        // A `RecVar` binds no unification variable; a `Rec` is searched under
        // its binder without unfolding, which terminates.
        Ty::RecVar(_) => false,
        Ty::Rec(_, body) => occurs(v, body, subst),
    }
}

/// Declarative unification as SOC context narrowing over explicit immutable
/// substitution `subst`.
///
/// Returns the updated substitution. Does NOT mutate state or perform hashing
/// inside the unification loop.
///
/// **It used to also return a `Vec<GeneratorId>` of `g_unify()` steps, and every
/// caller discarded it** (`let (s, _) = unify(...)`). Nothing ever built a leaf
/// from those steps, so `g_unify` sat in `minted_generators()` — and therefore
/// in [`typing_registry`] — declaring a generator inference could not emit. A
/// declared `𝒢` wider than what inference can produce is a fence around
/// nothing, and it is the drift the single-source enumeration exists to
/// prevent, so the dead return value went and `g_unify` went with it.
///
/// If unification ever does need to appear in a derivation, it needs a real
/// leaf with real endpoints, not a resurrected step list: a bare sequence of
/// generator ids carries no `src`/`dst` and so cannot be audited by
/// `verify_structure` or discharged by ADR-0015 ⟨D-PRIM⟩'s mechanism.
pub fn unify(t1: &Ty, t2: &Ty, subst: &BTreeMap<u32, Ty>) -> Result<BTreeMap<u32, Ty>, TypeError> {
    let r1 = resolve(t1, subst);
    let r2 = resolve(t2, subst);

    match (r1, r2) {
        (Ty::Var(v1), Ty::Var(v2)) if v1 == v2 => Ok(subst.clone()),
        (Ty::Var(v1), t) => {
            if occurs(*v1, t, subst) {
                Err(TypeError::InfiniteType)
            } else {
                let mut next_subst = subst.clone();
                next_subst.insert(*v1, t.clone());
                Ok(next_subst)
            }
        }
        (t, Ty::Var(v2)) => {
            if occurs(*v2, t, subst) {
                Err(TypeError::InfiniteType)
            } else {
                let mut next_subst = subst.clone();
                next_subst.insert(*v2, t.clone());
                Ok(next_subst)
            }
        }
        (Ty::Con(a), Ty::Con(b)) => {
            if a == b {
                Ok(subst.clone())
            } else {
                Err(TypeError::Mismatch)
            }
        }
        (Ty::Fn(a1, b1), Ty::Fn(a2, b2)) => {
            let s1 = unify(a1, a2, subst)?;
            unify(b1, b2, &s1)
        }
        (Ty::Record(f1), Ty::Record(f2)) => {
            let mut s1_fields = f1.clone();
            s1_fields.sort_by(|a, b| a.0.cmp(&b.0));
            s1_fields.dedup_by(|a, b| a.0 == b.0);

            let mut s2_fields = f2.clone();
            s2_fields.sort_by(|a, b| a.0.cmp(&b.0));
            s2_fields.dedup_by(|a, b| a.0 == b.0);

            if s1_fields.len() != s2_fields.len() {
                return Err(TypeError::Mismatch);
            }
            for (a, b) in s1_fields.iter().zip(s2_fields.iter()) {
                if a.0 != b.0 {
                    return Err(TypeError::Mismatch);
                }
            }
            let mut curr_subst = subst.clone();
            for (a, b) in s1_fields.iter().zip(s2_fields.iter()) {
                curr_subst = unify(&a.1, &b.1, &curr_subst)?;
            }
            Ok(curr_subst)
        }
        // Two recursive types are the same type iff they bind the same name.
        // Names come from `config` declarations, and a name has exactly one
        // declaration in a module, so nominal equality here IS structural
        // equality — and it terminates, which structural descent would not.
        (Ty::Rec(a, _), Ty::Rec(b, _)) => {
            if a == b {
                Ok(subst.clone())
            } else {
                Err(TypeError::Mismatch)
            }
        }
        (Ty::RecVar(a), Ty::RecVar(b)) => {
            if a == b {
                Ok(subst.clone())
            } else {
                Err(TypeError::Mismatch)
            }
        }
        // A bound occurrence and its own binder denote the same type — that is
        // what the binder means. Handled before the unfold arms below, which
        // would otherwise turn this into `RecVar` vs `Sum` and mismatch.
        (Ty::RecVar(a), Ty::Rec(b, _)) | (Ty::Rec(b, _), Ty::RecVar(a)) => {
            if a == b {
                Ok(subst.clone())
            } else {
                Err(TypeError::Mismatch)
            }
        }
        // A `Rec` meeting anything else is unfolded exactly once. The unfolding
        // is a `Sum`, whose recursive fields are `RecVar`s, and `RecVar` vs
        // `RecVar` terminates above — so this cannot loop.
        (Ty::Rec(_, _), other) => {
            let unfolded = r1.unfold();
            unify(&unfolded, &other.clone(), subst)
        }
        (other, Ty::Rec(_, _)) => {
            let unfolded = r2.unfold();
            unify(&other.clone(), &unfolded, subst)
        }
        (Ty::Sum(n1, vs1), Ty::Sum(n2, vs2)) => {
            if n1 != n2 || vs1.len() != vs2.len() {
                return Err(TypeError::Mismatch);
            }
            for ((v1_name, v1_fields), (v2_name, v2_fields)) in vs1.iter().zip(vs2.iter()) {
                if v1_name != v2_name || v1_fields.len() != v2_fields.len() {
                    return Err(TypeError::Mismatch);
                }
            }
            let mut curr_subst = subst.clone();
            for ((_, v1_fields), (_, v2_fields)) in vs1.iter().zip(vs2.iter()) {
                for (f1, f2) in v1_fields.iter().zip(v2_fields.iter()) {
                    curr_subst = unify(f1, f2, &curr_subst)?;
                }
            }
            Ok(curr_subst)
        }
        _ => Err(TypeError::Mismatch),
    }
}

// The flat typing lane — `infer`, `type_check`, `audited_type_check` — was
// removed here by ADR-0018. It built its configuration chain by padding
// (`[src, dst, dst, …, dst]`), so its `Decomposition` misstated its own
// intermediate configurations; `Audited` on a `replay_verified` padded chain
// asserted a replay that this repository's own counterexample showed would
// fail `soc_core::audit_step` under sound generator semantics.
//
// ADR-0007 §1 introduced the tree encoding precisely to remove that padding,
// and §7 kept the flat path only so that no test regressed. It had no caller
// outside its own tests. `infer_tree`/`audited_type_check_tree` below are the
// replacement; the padding-free property is guarded by
// `tree_derivation_carries_no_padded_step`.

/// Deferred-materialization atom for tree leaf endpoints (ADR-0008).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CfgAtom {
    Expr(Expr),
    Type(Ty),
    /// The kernel-owned arithmetic typing source object (ADR-0015 §5 Stage
    /// B0) — **append-only**, never reordered ahead of the two above.
    ///
    /// Unlike `Type`, this atom carries no unification variables and is not
    /// zonked at materialization: its operand types are already resolved when
    /// the plan is built.
    ArithInput(ArithTypingInputV1),
    /// The kernel-owned arithmetic typing *destination* object (ADR-0015 §5
    /// Stage B) — **append-only**, never reordered ahead of the three above.
    ///
    /// A registry row is matched by canonical bytes, so the kernel must be able
    /// to author *both* endpoints of a row. Stage B0 gave it the source
    /// ([`ArithTypingInputV1`]); this gives it the destination. Leaving the
    /// destination as `Type(Ty::Con(result))` would have required the kernel to
    /// reproduce this crate's `Ty` encoding — a second semantic encoder for a
    /// type the TCB does not own, which ADR-0015 §8.5 refuses to trust and
    /// `DEPS.md` forbids.
    ///
    /// Like `ArithInput`, not zonked: the result type is already resolved when
    /// the plan is built.
    ArithResult(NumericResultTypeV1),
}

/// Deferred-materialization tree object (ADR-0008).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TyObj {
    Atom(CfgAtom),
    Prod(Box<TyObj>, Box<TyObj>),
}

/// Deferred-materialization derivation tree (ADR-0008).
#[derive(Clone, PartialEq, Eq, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum TyTree {
    Leaf {
        generator: GeneratorId,
        src: TyObj,
        dst: TyObj,
    },
    Seq {
        left: Box<TyTree>,
        right: Box<TyTree>,
    },
    Tensor {
        left: Box<TyTree>,
        right: Box<TyTree>,
    },
}

fn materialize_obj(obj: &TyObj, subst: &BTreeMap<u32, Ty>) -> TreeObj {
    match obj {
        TyObj::Atom(CfgAtom::Expr(e)) => TreeObj::Atom(e.config_id()),
        TyObj::Atom(CfgAtom::Type(t)) => TreeObj::Atom(zonk(t, subst).config_id()),
        TyObj::Atom(CfgAtom::ArithInput(i)) => TreeObj::Atom(i.config_id()),
        TyObj::Atom(CfgAtom::ArithResult(r)) => TreeObj::Atom(r.config_id()),
        TyObj::Prod(a, b) => TreeObj::Prod(
            Box::new(materialize_obj(a, subst)),
            Box::new(materialize_obj(b, subst)),
        ),
    }
}

/// Materializes a `TyTree` into a `RealizesTree` by zonking all type endpoints against `subst` (ADR-0008).
pub fn materialize(tree: &TyTree, subst: &BTreeMap<u32, Ty>) -> RealizesTree {
    match tree {
        TyTree::Leaf {
            generator,
            src,
            dst,
        } => RealizesTree::Leaf {
            generator: *generator,
            src: materialize_obj(src, subst),
            dst: materialize_obj(dst, subst),
        },
        TyTree::Seq { left, right } => RealizesTree::Seq {
            left: Box::new(materialize(left, subst)),
            right: Box::new(materialize(right, subst)),
        },
        TyTree::Tensor { left, right } => RealizesTree::Tensor {
            left: Box::new(materialize(left, subst)),
            right: Box::new(materialize(right, subst)),
        },
    }
}

/// Infer tree-structured realization derivation with deferred config materialization (ADR-0008).
pub fn infer_tree(expr: &Expr, ctx: &TyCtx, st: Infer) -> Result<(Ty, TyTree, Infer), TypeError> {
    match expr {
        Expr::Lit(n) => Ok((
            Ty::Con("Int"),
            TyTree::Leaf {
                generator: g_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::Lit(*n))),
                dst: TyObj::Atom(CfgAtom::Type(Ty::Con("Int"))),
            },
            st,
        )),
        Expr::StrLit(s) => Ok((
            Ty::Con("Str"),
            TyTree::Leaf {
                generator: g_str_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::StrLit(s.clone()))),
                dst: TyObj::Atom(CfgAtom::Type(Ty::Con("Str"))),
            },
            st,
        )),
        Expr::FloatLit(s) => Ok((
            Ty::Con("Float"),
            TyTree::Leaf {
                generator: g_float_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::FloatLit(s.clone()))),
                dst: TyObj::Atom(CfgAtom::Type(Ty::Con("Float"))),
            },
            st,
        )),
        Expr::BoolLit(b) => Ok((
            bool_ty(),
            TyTree::Leaf {
                generator: g_bool_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::BoolLit(*b))),
                dst: TyObj::Atom(CfgAtom::Type(bool_ty())),
            },
            st,
        )),
        Expr::Cmp(op, a, b) => {
            let (ta, da, s1) = infer_tree(a, ctx, st)?;
            let (tb, db, s2) = infer_tree(b, ctx, s1)?;

            // Both operands must land on the same type. No promotion: mixing
            // `Int` and `Float` under a comparison would need the coercion
            // lattice to decide which side moves, and that is a choice
            // ⟨D-SPLIT⟩ keeps out of a structural split. Refused rather than
            // guessed.
            let subst = unify(&ta, &tb, &s2.subst)?;
            let operand_ty = resolve(&ta, &subst).clone();
            let s3 = Infer {
                subst,
                ..s2.clone()
            };

            let split = TyTree::Leaf {
                generator: g_cmp_split(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::Cmp(
                    *op,
                    Box::new((**a).clone()),
                    Box::new((**b).clone()),
                ))),
                dst: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Expr((**a).clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Expr((**b).clone()))),
                ),
            };
            let operands = TyTree::Tensor {
                left: Box::new(da),
                right: Box::new(db),
            };
            let cmp_leaf = TyTree::Leaf {
                generator: g_cmp(),
                src: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Type(operand_ty.clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Type(operand_ty))),
                ),
                dst: TyObj::Atom(CfgAtom::Type(bool_ty())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(operands),
                    right: Box::new(cmp_leaf),
                }),
            };
            Ok((bool_ty(), tree, s3))
        }
        Expr::Arith(op, a, b) => {
            let (ta, da, s1) = infer_tree(a, ctx, st)?;
            let (tb, db, s2) = infer_tree(b, ctx, s1)?;
            let ra = resolve(&ta, &s2.subst).clone();
            let rb = resolve(&tb, &s2.subst).clone();
            let plan = plan_arith(*op, &ra, &rb)?;

            let mut subst = s2.subst.clone();
            if plan.unify_a {
                let s = unify(&ra, &Ty::Con(plan.base), &subst)?;
                subst = s;
            }
            if plan.unify_b {
                let s = unify(&rb, &Ty::Con(plan.base), &subst)?;
                subst = s;
            }

            let result_ty = Ty::Con(plan.result);

            // ADR-0015 §5 Stage B0. The operands are NOT coerced up to the
            // result type here any more. Splicing one embedding leaf per
            // promotion edge was what erased the arithmetic source object: it
            // presented both operands already at the result type, so the
            // `g_arith` leaf could not say what they had started as, which is
            // why `1.0 + 2.0` and `7 / 2` emitted the identical leaf
            // `Prod(Float, Float) → Float`. The promotion paths are now
            // ordered data inside `ArithTypingInputV1`.
            let split = TyTree::Leaf {
                generator: g_arith_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Expr((**a).clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Expr((**b).clone()))),
                ),
            };
            let tensor = TyTree::Tensor {
                left: Box::new(da),
                right: Box::new(db),
            };
            // The operand types the `Tensor` actually lands on. For an operand
            // that was an unbound var, `eff` is `plan.base` and the operand's
            // own `Var(n)` endpoint zonks to exactly that under the
            // substitution unified above — so the `Seq` middle matches either
            // way.
            let operand_types = TyObj::Prod(
                Box::new(TyObj::Atom(CfgAtom::Type(Ty::Con(plan.eff_a)))),
                Box::new(TyObj::Atom(CfgAtom::Type(Ty::Con(plan.eff_b)))),
            );
            let input = TyObj::Atom(CfgAtom::ArithInput(plan.typing_input(*op)?));
            // The bridge. `TreeObj::Atom` can never equal a `TreeObj::Prod`,
            // and a `Tensor`'s `dst` is structurally always a `Prod`, so the
            // packaging of two operand types into one source object has to be
            // its own leaf. It is deliberately not folded into `g_arith_split`
            // — ⟨D-SPLIT⟩ discharges the split only while it stays purely
            // structural. See `g_arith_input`.
            let input_leaf = TyTree::Leaf {
                generator: g_arith_input(),
                src: operand_types,
                dst: input.clone(),
            };
            // ADR-0015 §5 Stage B. `g_arith`'s `dst` is the kernel's own
            // `NumericResultTypeV1`, not a `Ty` atom: a registry row is matched
            // by canonical bytes, and the kernel may not reproduce this crate's
            // `Ty` encoding to author one (§8.5; DEPS.md, "never a second
            // semantic encoder"). The rename back into `Ty` is the separate,
            // explicitly undischarged `g_arith_result` bridge below.
            let arith_result = NumericResultTypeV1 {
                name: NumericTypeNameV1::from_lattice_node(plan.result)
                    .ok_or(TypeError::Mismatch)?,
            };
            let result_obj = TyObj::Atom(CfgAtom::ArithResult(arith_result));
            let arith_leaf = TyTree::Leaf {
                generator: g_arith(),
                src: input,
                dst: result_obj.clone(),
            };
            let result_leaf = TyTree::Leaf {
                generator: g_arith_result(),
                src: result_obj,
                dst: TyObj::Atom(CfgAtom::Type(result_ty.clone())),
            };
            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(tensor),
                    right: Box::new(TyTree::Seq {
                        left: Box::new(input_leaf),
                        right: Box::new(TyTree::Seq {
                            left: Box::new(arith_leaf),
                            right: Box::new(result_leaf),
                        }),
                    }),
                }),
            };

            Ok((
                result_ty,
                tree,
                Infer {
                    subst,
                    next_var: s2.next_var,
                },
            ))
        }
        Expr::Var(name) => {
            let t = ctx
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::Unbound(name.clone()))?;
            Ok((
                t.clone(),
                TyTree::Leaf {
                    generator: g_var(),
                    src: TyObj::Atom(CfgAtom::Expr(Expr::Var(name.clone()))),
                    dst: TyObj::Atom(CfgAtom::Type(t.clone())),
                },
                st,
            ))
        }
        Expr::Lam(p, body) => {
            let (alpha, st_alpha) = st.fresh_var();
            let ctx_ext = ctx.extend(p.clone(), Ty::Var(alpha));
            let (tb, d_body, st_prime) = infer_tree(body, &ctx_ext, st_alpha)?;
            let param_ty = resolve(&Ty::Var(alpha), &st_prime.subst).clone();
            let fn_ty = Ty::Fn(Box::new(param_ty.clone()), Box::new(tb.clone()));

            let intro = TyTree::Leaf {
                generator: g_lam_intro(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::Lam(p.clone(), body.clone()))),
                dst: TyObj::Atom(CfgAtom::Expr((**body).clone())),
            };
            let close = TyTree::Leaf {
                generator: g_lam_close(),
                src: TyObj::Atom(CfgAtom::Type(tb.clone())),
                dst: TyObj::Atom(CfgAtom::Type(fn_ty.clone())),
            };
            let tree = TyTree::Seq {
                left: Box::new(intro),
                right: Box::new(TyTree::Seq {
                    left: Box::new(d_body),
                    right: Box::new(close),
                }),
            };
            Ok((fn_ty, tree, st_prime))
        }
        Expr::App(f, x) => {
            let (tf, df, s1) = infer_tree(f, ctx, st)?;
            let (tx, dx, s2) = infer_tree(x, ctx, s1)?;
            let (beta, s_beta) = s2.fresh_var();

            let target = Ty::Fn(
                Box::new(resolve(&tx, &s_beta.subst).clone()),
                Box::new(Ty::Var(beta)),
            );

            let s3 = unify(resolve(&tf, &s_beta.subst), &target, &s_beta.subst)?;

            let a = zonk(&tx, &s3);
            let b = zonk(&Ty::Var(beta), &s3);
            let fn_ty = Ty::Fn(Box::new(a.clone()), Box::new(b.clone()));

            let split = TyTree::Leaf {
                generator: g_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Expr((**f).clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Expr((**x).clone()))),
                ),
            };

            let tensor = TyTree::Tensor {
                left: Box::new(df),
                right: Box::new(dx),
            };

            let app = TyTree::Leaf {
                generator: g_app2(),
                src: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Type(fn_ty.clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Type(a.clone()))),
                ),
                dst: TyObj::Atom(CfgAtom::Type(b.clone())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(tensor),
                    right: Box::new(app),
                }),
            };

            Ok((
                b,
                tree,
                Infer {
                    subst: s3,
                    next_var: s_beta.next_var,
                },
            ))
        }
        Expr::Record(fields) => {
            let mut sorted_fields = fields.clone();
            sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
            sorted_fields.dedup_by(|a, b| a.0 == b.0);

            if sorted_fields.is_empty() {
                // No split leaf: `{}` has no subexpressions, so there is
                // nothing to decompose. This branch used to emit
                // `Seq(g_record_split{Expr({}) -> Expr({})}, g_record_empty)`,
                // and that first leaf was a padded step — `src == dst`, the
                // faked intermediate config ADR-0007 §1 calls unsound and its
                // §5 criterion 2 forbids ("no endpoint equals its neighbor by
                // padding"), and the shape ADR-0018 §4 retired the flat lane
                // over. It passed `elaborate_tree`'s `Seq` middle-match for
                // exactly the reason ADR-0007 §1 gives: a padded middle
                // `dst ≡ dst` always matches syntactically.
                //
                // It was also emitted under `g_record_split`, which *is*
                // discharged tight — but on ⟨D-SPLIT⟩'s ground that a split
                // "yields exactly two ordered child obligations", which at
                // zero arity it does not. Nothing was over-graded, because
                // `g_record_empty` capped the result at `@Audited`; the hazard
                // was that discharging `g_record_empty` would have published
                // `@Proven` over the padded step.
                let rec_ty = Ty::Record(vec![]);
                let tree = TyTree::Leaf {
                    generator: g_record_empty(),
                    src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                    dst: TyObj::Atom(CfgAtom::Type(rec_ty.clone())),
                };
                return Ok((rec_ty, tree, st));
            }

            let mut sorted_types = Vec::new();
            let mut expr_atoms = Vec::new();
            let mut type_atoms = Vec::new();
            let mut d_trees = Vec::new();
            let mut curr_st = st;

            for (name, val_expr) in sorted_fields {
                let (t_i, d_i, next_st) = infer_tree(&val_expr, ctx, curr_st)?;
                sorted_types.push((name, t_i.clone()));
                expr_atoms.push(TyObj::Atom(CfgAtom::Expr(val_expr)));
                type_atoms.push(TyObj::Atom(CfgAtom::Type(t_i)));
                d_trees.push(d_i);
                curr_st = next_st;
            }

            let result_ty = Ty::Record(sorted_types);

            let split = TyTree::Leaf {
                generator: g_record_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: right_nest_prod(expr_atoms),
            };

            let fields_tensor = right_nest_tensor(d_trees);

            let record_leaf = TyTree::Leaf {
                generator: g_record(),
                src: right_nest_prod(type_atoms),
                dst: TyObj::Atom(CfgAtom::Type(result_ty.clone())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(fields_tensor),
                    right: Box::new(record_leaf),
                }),
            };

            Ok((result_ty, tree, curr_st))
        }
        Expr::Field(base, fname) => {
            let (t_base, d_base, st1) = infer_tree(base, ctx, st)?;
            let zonked_base = zonk(&t_base, &st1.subst);
            if let Ty::Record(fields) = &zonked_base {
                if let Some((_, t_f)) = fields.iter().find(|(n, _)| n == fname) {
                    let split = TyTree::Leaf {
                        generator: g_field_split(),
                        src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                        dst: TyObj::Atom(CfgAtom::Expr((**base).clone())),
                    };
                    let field_leaf = TyTree::Leaf {
                        generator: g_field(),
                        src: TyObj::Atom(CfgAtom::Type(zonked_base.clone())),
                        dst: TyObj::Atom(CfgAtom::Type(t_f.clone())),
                    };
                    let tree = TyTree::Seq {
                        left: Box::new(split),
                        right: Box::new(TyTree::Seq {
                            left: Box::new(d_base),
                            right: Box::new(field_leaf),
                        }),
                    };
                    Ok((t_f.clone(), tree, st1))
                } else {
                    Err(TypeError::NoField(fname.clone()))
                }
            } else {
                Err(TypeError::Mismatch)
            }
        }
        Expr::Ctor(sum_ty, variant, args) => {
            // Unfolded to read the variants; the *result* type stays the
            // folded `sum_ty`, so the constructed value is a `Nat` rather than
            // a one-step unfolding of one.
            let resolved = resolve(sum_ty, &st.subst).unfold();
            if let Ty::Sum(_sum_name, variants) = &resolved {
                let (_, declared_fields) = variants
                    .iter()
                    .find(|(vname, _)| vname == variant)
                    .ok_or(TypeError::Mismatch)?;
                if args.len() != declared_fields.len() {
                    return Err(TypeError::Mismatch);
                }
                let declared_fields = declared_fields.clone();
                if args.is_empty() {
                    // No split leaf: a nullary constructor has no argument
                    // subexpressions, so there is nothing to decompose. See
                    // the empty-record branch above for the full reasoning —
                    // this branch carried the identical padded step under
                    // `g_ctor_split`, which is likewise discharged tight on a
                    // ⟨D-SPLIT⟩ ground that does not hold at zero arity.
                    let tree = TyTree::Leaf {
                        generator: g_ctor_nullary(),
                        src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                        dst: TyObj::Atom(CfgAtom::Type(sum_ty.clone())),
                    };
                    return Ok((sum_ty.clone(), tree, st));
                }

                let mut expr_atoms = Vec::new();
                let mut type_atoms = Vec::new();
                let mut d_trees = Vec::new();
                let mut curr_st = st;

                for (arg_expr, declared_field_ty) in args.iter().zip(declared_fields.iter()) {
                    let (t_i, d_i, next_st) = infer_tree(arg_expr, ctx, curr_st)?;
                    let next_subst = unify(&t_i, declared_field_ty, &next_st.subst)?;
                    expr_atoms.push(TyObj::Atom(CfgAtom::Expr(arg_expr.clone())));
                    let zonked_field = zonk(declared_field_ty, &next_subst);
                    type_atoms.push(TyObj::Atom(CfgAtom::Type(zonked_field)));
                    d_trees.push(d_i);
                    curr_st = Infer {
                        subst: next_subst,
                        next_var: next_st.next_var,
                    };
                }

                let split = TyTree::Leaf {
                    generator: g_ctor_split(),
                    src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                    dst: right_nest_prod(expr_atoms),
                };

                let args_tensor = right_nest_tensor(d_trees);

                let ctor_leaf = TyTree::Leaf {
                    generator: g_ctor(),
                    src: right_nest_prod(type_atoms),
                    dst: TyObj::Atom(CfgAtom::Type(sum_ty.clone())),
                };

                let tree = TyTree::Seq {
                    left: Box::new(split),
                    right: Box::new(TyTree::Seq {
                        left: Box::new(args_tensor),
                        right: Box::new(ctor_leaf),
                    }),
                };

                Ok((sum_ty.clone(), tree, curr_st))
            } else {
                Err(TypeError::Mismatch)
            }
        }
        Expr::Match(scrutinee, arms) => {
            let (t_s, d_s, s1) = infer_tree(scrutinee, ctx, st)?;
            check_coverage(&t_s, arms, &s1.subst)?;
            let mut curr_st = s1;
            let mut parts_exprs = vec![TyObj::Atom(CfgAtom::Expr((**scrutinee).clone()))];
            let mut parts_derivs = vec![d_s];
            let mut res_ty: Option<Ty> = None;

            for (pat, body) in arms {
                let bindings = bind_pattern(pat, &t_s, &curr_st.subst)?;
                let mut arm_ctx = ctx.clone();
                for (x, t) in bindings {
                    arm_ctx = arm_ctx.extend(x, t);
                }
                let (t_i, d_i, next_st) = infer_tree(body, &arm_ctx, curr_st)?;
                parts_exprs.push(TyObj::Atom(CfgAtom::Expr(body.clone())));
                parts_derivs.push(d_i);
                if let Some(ref r_ty) = res_ty {
                    let next_subst = unify(r_ty, &t_i, &next_st.subst)?;
                    curr_st = Infer {
                        subst: next_subst,
                        next_var: next_st.next_var,
                    };
                } else {
                    res_ty = Some(t_i);
                    curr_st = next_st;
                }
            }

            let result_ty = res_ty.ok_or(TypeError::Mismatch)?;
            let zonked_t_s = zonk(&t_s, &curr_st.subst);
            let zonked_t_res = zonk(&result_ty, &curr_st.subst);

            let mut parts_types = vec![TyObj::Atom(CfgAtom::Type(zonked_t_s))];
            for _ in arms {
                parts_types.push(TyObj::Atom(CfgAtom::Type(zonked_t_res.clone())));
            }

            let split = TyTree::Leaf {
                generator: g_match_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: right_nest_prod(parts_exprs),
            };

            let parts_tensor = right_nest_tensor(parts_derivs);

            let match_generator = if arms
                .iter()
                .any(|(pattern, _)| matches!(pattern, Pattern::Wildcard | Pattern::Var(_)))
            {
                g_match_catchall()
            } else {
                g_match()
            };
            let match_leaf = TyTree::Leaf {
                generator: match_generator,
                src: right_nest_prod(parts_types),
                dst: TyObj::Atom(CfgAtom::Type(result_ty.clone())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(parts_tensor),
                    right: Box::new(match_leaf),
                }),
            };

            Ok((result_ty, tree, curr_st))
        }
    }
}

fn right_nest_prod(items: Vec<TyObj>) -> TyObj {
    assert!(
        !items.is_empty(),
        "right_nest_prod requires at least 1 element"
    );
    let mut iter = items.into_iter().rev();
    let last = iter.next().unwrap();
    iter.fold(last, |acc, elem| TyObj::Prod(Box::new(elem), Box::new(acc)))
}

fn right_nest_tensor(items: Vec<TyTree>) -> TyTree {
    assert!(
        !items.is_empty(),
        "right_nest_tensor requires at least 1 element"
    );
    let mut iter = items.into_iter().rev();
    let last = iter.next().unwrap();
    iter.fold(last, |acc, elem| TyTree::Tensor {
        left: Box::new(elem),
        right: Box::new(acc),
    })
}

/// Upgrades a native `infer_tree` derivation to an `Audited` `Judgement` and `RealizesTree` (ADR-0008).
pub fn audited_type_check_tree(
    expr: &Expr,
    ctx: &TyCtx,
    context: ContextId,
) -> Result<(Judgement, TreeDerivation), TypeError> {
    let (ty, ty_tree, st) = infer_tree(expr, ctx, Infer::new())?;
    let tree = materialize(&ty_tree, &st.subst);
    let final_ty = zonk(&ty, &st.subst);
    if !tree.well_formed()
        || tree.src() != TreeObj::Atom(expr.config_id())
        || tree.dst() != TreeObj::Atom(final_ty.config_id())
    {
        return Err(TypeError::IllFormedDerivation);
    }
    let witness_id = tree.witness_id();
    let prop = Realizes::new(witness_id, expr.config_id(), final_ty.config_id()).proposition_id();

    // The audit that earns the evidence (ADR-0017 §5 D3). It re-checks
    // well-formedness and endpoints — the conditions above already established
    // them, and a checker that took the caller's word for its own inputs would
    // be checking nothing — and adds the leaf-generator membership check the
    // tree lane was missing (§4 row c). It does **not** check any leaf's ρ_g;
    // see `tree_audit`'s module doc.
    let derivation = audit_tree(&tree, expr.config_id(), final_ty.config_id())
        .map_err(|_| TypeError::IllFormedDerivation)?;

    let audited = Judgement::publish(
        Authority::AuditChecker,
        context,
        prop,
        Outcome::Audited,
        Support::Tree(&derivation),
    )
    .map_err(|_| TypeError::IllFormedDerivation)?;
    Ok((audited, derivation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_elaborate::ElaborationResult;
    use brix_kernel::Budget;
    use brix_semantic::{Authority, EdgeKind};

    #[test]
    fn generator_name_finds_tight_and_untight_generators() {
        assert_eq!(generator_name(&g_lit()).as_deref(), Some("g_lit"));
        assert_eq!(generator_name(&g_var()).as_deref(), Some("g_var"));
        // The honest holdouts named in ADR-0010/#43: g_arith, the coercion
        // edges, and g_match_catchall.
        assert_eq!(generator_name(&g_arith()).as_deref(), Some("g_arith"));
        assert_eq!(
            generator_name(&g_match_catchall()).as_deref(),
            Some("g_match_catchall")
        );
        // Stage E ⟨D-PROMOTE⟩: the lossy edge is not a promotion, and `brix why`
        // must not call it one. An exact edge still is.
        assert_eq!(
            generator_name(&NUMERIC.promote_generator("Int", "Float")).as_deref(),
            Some("convert_lossy(Int->Float)")
        );
        assert_eq!(
            generator_name(&NUMERIC.promote_generator("Nat", "Int")).as_deref(),
            Some("promote(Nat->Int)")
        );
    }

    #[test]
    fn generator_name_is_none_for_an_unminted_generator() {
        assert_eq!(
            generator_name(&GeneratorId::named("not-a-real-generator@1")),
            None
        );
    }

    /// The declared generator set must not be wider than what inference can
    /// emit, and these three are the drift that was.
    ///
    /// `type.rule.app@1`, `type.rule.lam@1` and `type.rule.unify@1` were in
    /// `minted_generators()` — and so in [`typing_registry`] — while no code
    /// path produced a leaf for any of them: application and abstraction emit
    /// `g_app2`/`g_lam_intro`/`g_lam_body`, and `unify`'s `g_unify` step vector
    /// was discarded by every caller. Nothing was unsound, because nothing
    /// emitted them, but a declared `𝒢` wider than the emittable set is a fence
    /// around nothing, and the single-source enumeration exists to prevent
    /// exactly this.
    ///
    /// Ids spelled as literals on purpose: the constructors are gone, so a
    /// re-addition is what this test has to catch, and it can only do that by
    /// naming the strings.
    #[test]
    fn the_retired_generators_are_not_declared() {
        for name in ["type.rule.app@1", "type.rule.lam@1", "type.rule.unify@1"] {
            let g = GeneratorId::named(name);
            assert!(
                !typing_registry().contains(&g),
                "{name} was retired and must not be declared again without a producer"
            );
            assert_eq!(
                generator_name(&g),
                None,
                "{name} must not be nameable either"
            );
        }
    }

    /// The counterexample ADR-0018 §4 preserves, over a corpus rather than one
    /// expression.
    ///
    /// The retired flat lane padded its configuration chain to
    /// `[src, dst, dst, …, dst]`, which passes syntactic `RealizesComp`
    /// (a `dst == dst` middle always matches) but fails a sound audit,
    /// because no generator realizes `(dst, dst)`. The deleted
    /// `test_multi_step_elaboration_tree_vs_linear_tension` demonstrated
    /// exactly that, using a `NonPaddedSemantics` whose whole content was
    /// `src != dst`. ADR-0007 §1 states it directly — "faking intermediate
    /// configs is unsound" — and its §5 criterion 2 requires that no endpoint
    /// equal its neighbour by padding.
    ///
    /// **This test used to run over `(\x. x) 42` alone, and its claim that it
    /// would fire "if inference ever starts padding tree endpoints" was
    /// therefore false.** Inference *was* padding, in the two zero-arity
    /// branches: `{}` emitted `g_record_split{Expr({}) -> Expr({})}` and a
    /// nullary constructor emitted the matching `g_ctor_split` step, both under
    /// generators discharged tight. Neither was over-graded, because the
    /// sibling leaf capped the result — but that made the defect invisible
    /// exactly until someone discharged the sibling. The corpus below is what
    /// makes the claim true; extend it whenever a branch of `infer_tree` gains
    /// a new derivation shape.
    #[test]
    fn tree_derivation_carries_no_padded_step() {
        let opt = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );

        let corpus: Vec<(&str, Expr, TyCtx)> = vec![
            // The expression the retired test used.
            (
                "application of a lambda",
                Expr::App(
                    Box::new(Expr::Lam(
                        "x".to_string(),
                        Box::new(Expr::Var("x".to_string())),
                    )),
                    Box::new(Expr::Lit(42)),
                ),
                TyCtx::new(),
            ),
            ("literal", Expr::Lit(1), TyCtx::new()),
            // The two zero-arity branches this test previously never reached.
            ("empty record", Expr::Record(vec![]), TyCtx::new()),
            (
                "nullary constructor",
                Expr::Ctor(opt.clone(), "None".into(), vec![]),
                TyCtx::new(),
            ),
            // Their non-empty counterparts, so a regression that padded the
            // general path instead would also be caught.
            (
                "record with fields",
                Expr::Record(vec![("a".into(), Expr::Lit(1))]),
                TyCtx::new(),
            ),
            (
                "constructor with a payload",
                Expr::Ctor(opt.clone(), "Some".into(), vec![Expr::Lit(1)]),
                TyCtx::new(),
            ),
            // Recursive configs (#298) reach these branches through `.unfold()`;
            // the corpus has to follow the language.
            (
                "nullary variant of a recursive config",
                Expr::Ctor(
                    Ty::Rec(
                        "List".into(),
                        Box::new(Ty::Sum(
                            "List".into(),
                            vec![
                                ("Nil".into(), vec![]),
                                (
                                    "Cons".into(),
                                    vec![Ty::Con("Int"), Ty::RecVar("List".into())],
                                ),
                            ],
                        )),
                    ),
                    "Nil".into(),
                    vec![],
                ),
                TyCtx::new(),
            ),
            (
                "field access",
                Expr::Field(
                    Box::new(Expr::Record(vec![("a".into(), Expr::Lit(1))])),
                    "a".into(),
                ),
                TyCtx::new(),
            ),
            (
                "arithmetic",
                Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
                TyCtx::new(),
            ),
        ];

        for (label, expr, ctx) in corpus {
            let (_, derivation) = audited_type_check_tree(&expr, &ctx, ContextId::root())
                .unwrap_or_else(|e| panic!("{label} must type: {e:?}"));

            for leaf in derivation.tree().leaves() {
                match leaf {
                    RealizesTree::Leaf {
                        generator,
                        src,
                        dst,
                    } => assert_ne!(
                        src,
                        dst,
                        "{label}: leaf {:?} is a degenerate src == dst step — the padding \
                         ADR-0007 §1 removed and ADR-0018 retired the flat lane over",
                        generator_name(generator).unwrap_or_else(|| generator.to_hex())
                    ),
                    other => panic!("leaves() must yield only Leaf nodes, got {other:?}"),
                }
            }
        }
    }

    /// The two zero-arity branches derive as a single leaf, with no split.
    ///
    /// Stated separately from the padding test because it pins the *shape*, not
    /// just the absence of a degenerate step: there are no subexpressions to
    /// decompose, so emitting a split at all was the error. If either branch
    /// regains one, this fires even if that split were somehow non-degenerate.
    #[test]
    fn the_zero_arity_branches_emit_no_split() {
        let opt = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );

        for (label, expr, want) in [
            ("empty record", Expr::Record(vec![]), g_record_empty()),
            (
                "nullary constructor",
                Expr::Ctor(opt, "None".into(), vec![]),
                g_ctor_nullary(),
            ),
        ] {
            let (_, derivation) =
                audited_type_check_tree(&expr, &TyCtx::new(), ContextId::root()).expect("types");
            let leaves = derivation.tree().leaves();
            assert_eq!(leaves.len(), 1, "{label}: expected a single leaf");
            match leaves[0] {
                RealizesTree::Leaf { generator, .. } => {
                    assert_eq!(generator, &want, "{label}: wrong generator");
                }
                other => panic!("{label}: expected a Leaf, got {other:?}"),
            }
        }
    }

    // --- inference-property coverage carried over from the retired flat lane
    // (ADR-0018 §3). `unify`/`occurs` are shared, so these properties are still
    // production behaviour; only their flat-path tests went with the deletion.

    #[test]
    fn unbound_var_is_a_type_error() {
        let res = infer_tree(&Expr::Var("nope".to_string()), &TyCtx::new(), Infer::new());
        match res {
            Err(TypeError::Unbound(name)) => assert_eq!(name, "nope"),
            other => panic!("an unbound variable must be a type error, got {other:?}"),
        }
    }

    #[test]
    fn applying_a_non_function_is_a_mismatch() {
        let expr = Expr::App(Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        match infer_tree(&expr, &TyCtx::new(), Infer::new()) {
            Err(TypeError::Mismatch) => {}
            other => panic!("applying a literal must be a mismatch, got {other:?}"),
        }
    }

    #[test]
    fn self_application_is_rejected_by_the_occurs_check() {
        // \x. x x — typing it would need `a = a -> b`, the infinite type the
        // occurs check exists to refuse. Never a panic, never a judgement.
        let expr = Expr::Lam(
            "x".to_string(),
            Box::new(Expr::App(
                Box::new(Expr::Var("x".to_string())),
                Box::new(Expr::Var("x".to_string())),
            )),
        );
        match infer_tree(&expr, &TyCtx::new(), Infer::new()) {
            Err(TypeError::InfiniteType) | Err(TypeError::Mismatch) => {}
            other => panic!("self-application must be refused, got {other:?}"),
        }
    }

    #[test]
    fn tree_typing_is_deterministic() {
        let expr = Expr::App(Box::new(Expr::Var("f".to_string())), Box::new(Expr::Lit(1)));
        let ctx = TyCtx::new().extend(
            "f",
            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))),
        );
        let (j1, d1) = audited_type_check_tree(&expr, &ctx, ContextId::root()).unwrap();
        let (j2, d2) = audited_type_check_tree(&expr, &ctx, ContextId::root()).unwrap();
        assert_eq!(
            j1.id(),
            j2.id(),
            "the same program must yield the same judgement"
        );
        assert_eq!(d1.id(), d2.id(), "and the same derivation artifact");
    }

    #[test]
    fn test_tree_elaboration_end_to_end() {
        let expr = Expr::App(Box::new(Expr::Var("f".to_string())), Box::new(Expr::Lit(1)));
        let ctx = TyCtx::new().extend(
            "f",
            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))),
        );
        let context = ContextId::root();

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);

        // Inspect tree: g_app2 leaf src config equals Prod(Atom(Fn(Int,Bool).config_id()), Atom(Int.config_id()))
        if let RealizesTree::Seq { right, .. } = tree.tree() {
            if let RealizesTree::Seq {
                right: app_leaf, ..
            } = right.as_ref()
            {
                if let RealizesTree::Leaf { generator, src, .. } = app_leaf.as_ref() {
                    assert_eq!(*generator, g_app2());
                    let expected_src = TreeObj::Prod(
                        Box::new(TreeObj::Atom(
                            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))).config_id(),
                        )),
                        Box::new(TreeObj::Atom(Ty::Con("Int").config_id())),
                    );
                    assert_eq!(*src, expected_src);
                } else {
                    panic!("Expected app leaf");
                }
            } else {
                panic!("Expected inner Seq");
            }
        } else {
            panic!("Expected outer Seq");
        }

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(
                    judgement.outcome.authority(),
                    brix_semantic::Authority::ProofKernel
                );
                assert_eq!(edge.kind, brix_semantic::EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, aud.id().digest());
            }
            other => panic!("Expected Proven, got {:?}", other),
        }

        // Determinism check
        let (aud2, tree2) = audited_type_check_tree(&expr, &ctx, context).unwrap();
        assert_eq!(aud, aud2);
        assert_eq!(tree, tree2);
    }

    #[test]
    fn test_lambda_end_to_end() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let (aud, tree) = audited_type_check_tree(&expr, &ctx, ContextId::root()).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
                assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, aud.id().digest());
            }
            other => panic!("Expected Proven, got {:?}", other),
        }

        let expected_prop = Realizes::new(
            brix_elaborate::witness_object(tree.tree()).witness_digest(),
            expr.config_id(),
            Ty::Con("Int").config_id(),
        )
        .proposition_id();
        assert_eq!(aud.proposition, expected_prop);
    }

    #[test]
    fn test_lambda_zonk_fired() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let (_, ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).unwrap();
        let tree = materialize(&ty_tree, &st.subst);

        if let RealizesTree::Seq { right: app_seq, .. } = &tree {
            if let RealizesTree::Seq { left: tensor, .. } = app_seq.as_ref() {
                if let RealizesTree::Tensor { left: df, .. } = tensor.as_ref() {
                    if let RealizesTree::Seq { right: lam_seq, .. } = df.as_ref() {
                        if let RealizesTree::Seq {
                            right: close_box, ..
                        } = lam_seq.as_ref()
                        {
                            if let RealizesTree::Leaf { generator, dst, .. } = close_box.as_ref() {
                                assert_eq!(*generator, g_lam_close());
                                let expected_fn_config =
                                    Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Int")))
                                        .config_id();
                                assert_eq!(*dst, TreeObj::Atom(expected_fn_config));
                                return;
                            }
                        }
                    }
                }
            }
        }
        panic!("Failed to navigate tree to g_lam_close leaf");
    }

    #[test]
    fn test_bare_lambda_audited() {
        let expr = Expr::Lam("x".to_string(), Box::new(Expr::Var("x".to_string())));
        let ctx = TyCtx::new();
        let (aud, tree) = audited_type_check_tree(&expr, &ctx, ContextId::root()).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.tree().well_formed());
    }

    #[test]
    fn test_record_2_fields_proven() {
        // Record literal with 2 fields in reverse order: {y: 2, x: 1}
        let expr = Expr::Record(vec![
            ("y".to_string(), Expr::Lit(2)),
            ("x".to_string(), Expr::Lit(1)),
        ]);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer record");
        let final_ty = zonk(&ty, &st.subst);
        let expected_ty = Ty::Record(vec![
            ("x".to_string(), Ty::Con("Int")),
            ("y".to_string(), Ty::Con("Int")),
        ]);
        assert_eq!(final_ty, expected_ty);

        // Canonical identity check: {y: 2, x: 1} and {x: 1, y: 2} have the same config_id
        let expr_sorted = Expr::Record(vec![
            ("x".to_string(), Expr::Lit(1)),
            ("y".to_string(), Expr::Lit(2)),
        ]);
        assert_eq!(expr.config_id(), expr_sorted.config_id());

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited record");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_record_1_field_and_nested_proven() {
        // 1-field record: {a: 42}
        let expr1 = Expr::Record(vec![("a".to_string(), Expr::Lit(42))]);
        let ctx = TyCtx::new();
        let (aud1, tree1) = audited_type_check_tree(&expr1, &ctx, ContextId::root()).unwrap();
        assert!(tree1.tree().well_formed());
        assert!(matches!(
            brix_elaborate::elaborate_tree(&aud1, &tree1, Budget::new(1000, 1000)),
            ElaborationResult::Proven { .. }
        ));

        // Nested record: {inner: {val: 7}}
        let expr_nested = Expr::Record(vec![(
            "inner".to_string(),
            Expr::Record(vec![("val".to_string(), Expr::Lit(7))]),
        )]);
        let (aud2, tree2) = audited_type_check_tree(&expr_nested, &ctx, ContextId::root()).unwrap();
        assert!(tree2.tree().well_formed());
        let res2 = brix_elaborate::elaborate_tree(&aud2, &tree2, Budget::new(2000, 2000));
        assert!(matches!(res2, ElaborationResult::Proven { .. }));
    }

    #[test]
    fn test_field_access_proven() {
        // r.base where r: {base: Int, name: Int}
        let r_ty = Ty::Record(vec![
            ("base".to_string(), Ty::Con("Int")),
            ("name".to_string(), Ty::Con("Int")),
        ]);
        let ctx = TyCtx::new().extend("r", r_ty);
        let expr = Expr::Field(Box::new(Expr::Var("r".to_string())), "base".to_string());
        let context = ContextId::root();

        let (ty, _tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer field access");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, Ty::Con("Int"));

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited field");
        assert!(tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_field_access_missing_field_type_error() {
        let r_ty = Ty::Record(vec![("x".to_string(), Ty::Con("Int"))]);
        let ctx = TyCtx::new().extend("r", r_ty);
        let expr = Expr::Field(Box::new(Expr::Var("r".to_string())), "y".to_string());

        let res = audited_type_check_tree(&expr, &ctx, ContextId::root());
        assert_eq!(res, Err(TypeError::NoField("y".to_string())));
    }

    #[test]
    fn numeric_lattice_join_paths_and_fields() {
        // Joins along the chain and across the Int↪Float branch.
        assert_eq!(NUMERIC.join("Int", "Float"), Some("Float"));
        assert_eq!(NUMERIC.join("Int", "Rat"), Some("Rat"));
        assert_eq!(NUMERIC.join("Nat", "Complex"), Some("Complex"));
        assert_eq!(NUMERIC.join("Rat", "Real"), Some("Real"));
        assert_eq!(NUMERIC.join("Int", "Int"), Some("Int"));
        // Float is incomparable to the exact ℚ/ℝ/ℂ nodes → no join (a type error).
        assert_eq!(NUMERIC.join("Float", "Rat"), None);
        assert_eq!(NUMERIC.join("Float", "Real"), None);
        assert_eq!(NUMERIC.join("Float", "Complex"), None);

        // Witness paths: empty when already there, one edge per hop.
        assert_eq!(NUMERIC.edge_path("Int", "Int"), Vec::new());
        assert_eq!(NUMERIC.edge_path("Int", "Float"), vec![("Int", "Float")]);
        assert_eq!(
            NUMERIC.edge_path("Nat", "Real"),
            vec![("Nat", "Int"), ("Int", "Rat"), ("Rat", "Real")]
        );

        // Div forces at least the field of fractions.
        assert_eq!(field_of("Int"), "Float");
        assert_eq!(field_of("Nat"), "Float");
        assert_eq!(field_of("Rat"), "Rat");
        assert_eq!(field_of("Complex"), "Complex");
    }

    #[test]
    fn epistemic_grades_ride_the_same_coercion_lattice() {
        // Weakening certainty is the safe direction: Proven ↪ Audited ↪ Derived.
        assert!(GRADE.le("Proven", "Derived"));
        assert!(GRADE.le("Audited", "Derived"));
        assert!(GRADE.le("Proven", "Proven"));

        // Combining two grades meets at the weaker guarantee (their join).
        assert_eq!(GRADE.join("Proven", "Audited"), Some("Audited"));
        assert_eq!(GRADE.join("Derived", "Proven"), Some("Derived"));
        assert_eq!(GRADE.join("Audited", "Audited"), Some("Audited"));

        // Strengthening without evidence has NO up-path — this is epistemic
        // erasure, rejected by the very same code as a numeric mismatch.
        assert!(!GRADE.le("Derived", "Proven"));
        assert!(!GRADE.le("Audited", "Proven"));
        assert_eq!(GRADE.edge_path("Proven", "Derived").len(), 2);

        // Erasure surfaces as "no join in the strengthening direction": there is
        // no witness from Derived up to Proven, so a Proven-demanding site fed a
        // Derived value cannot be coerced. (join is symmetric; the *directed*
        // check `le(from, to)` is the erasure guard.)
        assert!(!GRADE.le("Derived", "Proven"));
    }

    #[test]
    fn arith_over_exact_tower_nodes_promotes_and_proves() {
        // x:Rat, y:Int → x + y : Rat, recording a witnessed Int↪Rat promotion
        // (as an `ArithTypingInputV1` path since ADR-0015 Stage B0, not as a
        // spliced leaf), and the resulting tree elaborates to Proven.
        // Exercises lattice nodes not yet reachable from surface literals.
        let ctx = TyCtx::new()
            .extend("x", Ty::Con("Rat"))
            .extend("y", Ty::Con("Int"));
        let expr = Expr::Arith(
            ArithOp::Add,
            Box::new(Expr::Var("x".to_string())),
            Box::new(Expr::Var("y".to_string())),
        );

        let (ty, _tree, _st) = infer_tree(&expr, &ctx, Infer::new()).unwrap();
        assert_eq!(ty, Ty::Con("Rat"));

        let (judgement, tree) =
            audited_type_check_tree(&expr, &ctx, ContextId::root()).expect("audited arith");
        assert_eq!(judgement.outcome, Outcome::Audited);

        let res = brix_elaborate::elaborate_tree(&judgement, &tree, Budget::new(2000, 2000));
        assert!(
            matches!(res, ElaborationResult::Proven { .. }),
            "expected Proven, got {res:?}"
        );
    }

    #[test]
    fn integer_division_promotes_to_the_field_of_fractions() {
        // 7 / 2 : Float (Div forces the field of fractions), reaching Proven.
        let expr = Expr::Arith(ArithOp::Div, Box::new(Expr::Lit(7)), Box::new(Expr::Lit(2)));
        let ctx = TyCtx::new();
        let (ty, _, _) = infer_tree(&expr, &ctx, Infer::new()).unwrap();
        assert_eq!(ty, Ty::Con("Float"));

        let (judgement, tree) =
            audited_type_check_tree(&expr, &ctx, ContextId::root()).expect("audited div");
        assert_eq!(judgement.outcome, Outcome::Audited);
        let res = brix_elaborate::elaborate_tree(&judgement, &tree, Budget::new(2000, 2000));
        assert!(
            matches!(res, ElaborationResult::Proven { .. }),
            "expected Proven, got {res:?}"
        );
    }

    // -----------------------------------------------------------------------
    // ADR-0015 §5 Stage B0 — the re-schemaed `g_arith` source object.
    // -----------------------------------------------------------------------

    /// The `src`/`dst` endpoints of the unique leaf citing `want`, from the
    /// pre-materialization tree (so the `ArithInput` payload is readable
    /// rather than already hashed to a `ConfigId`).
    fn leaf_endpoints(tree: &TyTree, want: &GeneratorId) -> (TyObj, TyObj) {
        fn walk(tree: &TyTree, want: &GeneratorId, out: &mut Vec<(TyObj, TyObj)>) {
            match tree {
                TyTree::Leaf {
                    generator,
                    src,
                    dst,
                } => {
                    if generator == want {
                        out.push((src.clone(), dst.clone()));
                    }
                }
                TyTree::Seq { left, right } | TyTree::Tensor { left, right } => {
                    walk(left, want, out);
                    walk(right, want, out);
                }
            }
        }
        let mut found = Vec::new();
        walk(tree, want, &mut found);
        assert_eq!(
            found.len(),
            1,
            "expected exactly one {:?} leaf, found {}",
            generator_name(want),
            found.len()
        );
        found.pop().expect("checked non-empty")
    }

    /// The `ArithTypingInputV1` an expression's `g_arith` leaf actually
    /// carries.
    fn arith_input_of(expr: &Expr, ctx: &TyCtx) -> ArithTypingInputV1 {
        let (_, tree, _) = infer_tree(expr, ctx, Infer::new()).expect("infers");
        match leaf_endpoints(&tree, &g_arith()).0 {
            TyObj::Atom(CfgAtom::ArithInput(i)) => i,
            other => panic!("g_arith's src must be the ArithInput atom, got {other:?}"),
        }
    }

    #[test]
    fn arith_leaf_round_trips_every_material_field() {
        // ADR-0015 Stage B0 gate 1: the emitted leaf round-trips every
        // material field. Checkable as two properties — the payload equals
        // what the operator and lattice independently say it should be, and
        // changing any one field alone changes the leaf's `src` identity. The
        // second half is what a registry row will rest on: a row keyed on a
        // `ConfigId` blind to a field could be satisfied by an input differing
        // in it.
        let cases: [(ArithOp, &str, &str, &str); 6] = [
            // (op, lhs type, rhs type, expected result)
            (ArithOp::Add, "Int", "Int", "Int"),
            (ArithOp::Sub, "Nat", "Int", "Int"),
            (ArithOp::Mul, "Rat", "Real", "Real"),
            (ArithOp::Div, "Int", "Int", "Float"),
            (ArithOp::Div, "Rat", "Rat", "Rat"),
            (ArithOp::Add, "Float", "Float", "Float"),
        ];

        for (op, lhs, rhs, want_result) in cases {
            let ctx = TyCtx::new()
                .extend("l", Ty::Con(lhs))
                .extend("r", Ty::Con(rhs));
            let expr = Expr::Arith(
                op,
                Box::new(Expr::Var("l".to_string())),
                Box::new(Expr::Var("r".to_string())),
            );

            let (ty, tree, _) = infer_tree(&expr, &ctx, Infer::new()).expect("infers");
            assert_eq!(ty, Ty::Con(want_result), "{op:?} {lhs} {rhs}");

            let (src, dst) = leaf_endpoints(&tree, &g_arith());
            let input = match &src {
                TyObj::Atom(CfgAtom::ArithInput(i)) => i.clone(),
                other => panic!("g_arith's src must be the ArithInput atom, got {other:?}"),
            };

            // dst is the exact result type, per ADR-0015 Stage B0 — now spelled
            // in the kernel's own destination schema (Stage B), because the
            // kernel must be able to author both endpoints of a registry row
            // and may not reproduce this crate's `Ty` encoding to do it.
            let want_result_name =
                NumericTypeNameV1::from_lattice_node(want_result).expect("numeric");
            assert_eq!(
                dst,
                TyObj::Atom(CfgAtom::ArithResult(NumericResultTypeV1 {
                    name: want_result_name,
                })),
                "{op:?} {lhs} {rhs}"
            );

            // …and the bridge back into this crate's `Ty` vocabulary lands on
            // exactly that type, so the chain is still pinned end to end.
            let (bridge_src, bridge_dst) = leaf_endpoints(&tree, &g_arith_result());
            assert_eq!(bridge_src, dst, "the bridge starts where g_arith ends");
            assert_eq!(
                bridge_dst,
                TyObj::Atom(CfgAtom::Type(Ty::Con(want_result))),
                "{op:?} {lhs} {rhs}"
            );

            // Reconstructed independently from the operator and the lattice,
            // not read back from the tree that produced it.
            let expected = ArithTypingInputV1 {
                operator: op.kernel_operator(),
                lhs_type: NumericTypeNameV1::from_lattice_node(lhs).expect("numeric"),
                rhs_type: NumericTypeNameV1::from_lattice_node(rhs).expect("numeric"),
                lhs_promotion_path: NUMERIC.promotion_path(lhs, want_result),
                rhs_promotion_path: NUMERIC.promotion_path(rhs, want_result),
            };
            assert_eq!(input, expected, "{op:?} {lhs} {rhs}");

            // Every field is bound into the leaf's `src` identity.
            let base_id = input.config_id();
            let mutations: Vec<(&str, ArithTypingInputV1)> = vec![
                (
                    "operator",
                    ArithTypingInputV1 {
                        operator: if op == ArithOp::Div {
                            ArithOperatorV1::Add
                        } else {
                            ArithOperatorV1::Div
                        },
                        ..input.clone()
                    },
                ),
                (
                    "lhs type",
                    ArithTypingInputV1 {
                        lhs_type: NumericTypeNameV1::Complex,
                        ..input.clone()
                    },
                ),
                (
                    "rhs type",
                    ArithTypingInputV1 {
                        rhs_type: NumericTypeNameV1::Complex,
                        ..input.clone()
                    },
                ),
                (
                    "lhs promotion path",
                    ArithTypingInputV1 {
                        lhs_promotion_path: NUMERIC.promotion_path("Nat", "Complex"),
                        ..input.clone()
                    },
                ),
                (
                    "rhs promotion path",
                    ArithTypingInputV1 {
                        rhs_promotion_path: NUMERIC.promotion_path("Nat", "Complex"),
                        ..input.clone()
                    },
                ),
                (
                    "operand order",
                    ArithTypingInputV1 {
                        lhs_type: input.rhs_type,
                        rhs_type: input.lhs_type,
                        lhs_promotion_path: input.rhs_promotion_path.clone(),
                        rhs_promotion_path: input.lhs_promotion_path.clone(),
                        ..input.clone()
                    },
                ),
            ];
            for (field, mutated) in mutations {
                if mutated == input {
                    // A symmetric case where the mutation is a no-op (e.g.
                    // swapping two identical operands). Nothing to assert —
                    // the field is exercised by the asymmetric cases above.
                    continue;
                }
                assert_ne!(
                    mutated.config_id(),
                    base_id,
                    "{op:?} {lhs} {rhs}: mutating {field} must change the leaf's src identity"
                );
            }
        }
    }

    #[test]
    fn float_addition_and_integer_division_emit_distinguishable_leaves() {
        // ADR-0015 Stage B0 gate 2, the ADR's named fixture. Before B0 both of
        // these emitted the byte-identical leaf `Prod(Float, Float) → Float`:
        // `Div` has a different result rule from the other three
        // (`field_of`: Int/Int → Float), so the two expressions agreed on
        // every field the old source object carried while disagreeing on the
        // operator, the operand types, and the promotions. A relation keyed on
        // `(operator, lhs, rhs, promotions) → result` was unreachable from it.
        let ctx = TyCtx::new();
        let float_add = Expr::Arith(
            ArithOp::Add,
            Box::new(Expr::FloatLit("1.0".to_string())),
            Box::new(Expr::FloatLit("2.0".to_string())),
        );
        let int_div = Expr::Arith(ArithOp::Div, Box::new(Expr::Lit(7)), Box::new(Expr::Lit(2)));

        // Both still result in Float — that is what made them confusable.
        for expr in [&float_add, &int_div] {
            let (ty, _, _) = infer_tree(expr, &ctx, Infer::new()).expect("infers");
            assert_eq!(ty, Ty::Con("Float"));
        }

        let add_input = arith_input_of(&float_add, &ctx);
        let div_input = arith_input_of(&int_div, &ctx);

        assert_ne!(
            add_input.config_id(),
            div_input.config_id(),
            "`1.0 + 2.0` and `7 / 2` must emit distinguishable g_arith leaves"
        );

        // And distinguishable *because* of the fields B0 added, not by
        // accident: the operator, the operand types, and the promotion paths
        // each differ.
        assert_eq!(add_input.operator, ArithOperatorV1::Add);
        assert_eq!(div_input.operator, ArithOperatorV1::Div);
        assert_eq!(add_input.lhs_type, NumericTypeNameV1::Float);
        assert_eq!(div_input.lhs_type, NumericTypeNameV1::Int);
        assert!(add_input.lhs_promotion_path.is_empty());
        assert!(!div_input.lhs_promotion_path.is_empty());

        // The defect this stage closes, reconstructed rather than asserted in
        // prose: the pre-B0 source object was keyed on the result type alone,
        // and both expressions result in `Float` — so the old key *collides*
        // for two programs that disagree on the operator and both operand
        // types. Keep this alongside the inequality above; without it the test
        // shows that the leaves differ but not that they ever failed to.
        let old_key = |result: &'static str| {
            TyObj::Prod(
                Box::new(TyObj::Atom(CfgAtom::Type(Ty::Con(result)))),
                Box::new(TyObj::Atom(CfgAtom::Type(Ty::Con(result)))),
            )
        };
        assert_eq!(
            old_key("Float"),
            old_key("Float"),
            "the pre-B0 source object collided for these two programs — that \
             collision is what Stage B0 exists to remove"
        );
    }

    #[test]
    fn arith_promotion_path_records_edge_exactness() {
        // ADR-0015 ⟨D-PROMOTE⟩: the exact tower edges and the lossy
        // `Int→Float` branch are different claims, and `NUMERIC` carries both
        // while describing all of its edges as information-preserving. `Div`
        // routes integer division through `field_of("Int") == "Float"`, so
        // `7 / 2` travels the lossy edge on both operands. Recording it
        // unlabelled inside a field ADR-0015 defines as a sequence of *exact*
        // promotion-edge ids would assert an exactness that does not hold.
        let div = arith_input_of(
            &Expr::Arith(ArithOp::Div, Box::new(Expr::Lit(7)), Box::new(Expr::Lit(2))),
            &TyCtx::new(),
        );
        for path in [&div.lhs_promotion_path, &div.rhs_promotion_path] {
            assert_eq!(path.len(), 1, "Int→Float is one edge");
            assert_eq!(
                path[0].kind,
                CoercionKind::Lossy,
                "Int→Float is not an embedding (ADR-0015 ⟨D-PROMOTE⟩)"
            );
            assert_eq!(
                path[0].generator,
                NUMERIC.promote_generator("Int", "Float"),
                "the edge is named by the generator that witnesses it"
            );
        }

        // The exact tower is recorded as exact.
        let ctx = TyCtx::new()
            .extend("x", Ty::Con("Rat"))
            .extend("y", Ty::Con("Int"));
        let add = arith_input_of(
            &Expr::Arith(
                ArithOp::Add,
                Box::new(Expr::Var("x".to_string())),
                Box::new(Expr::Var("y".to_string())),
            ),
            &ctx,
        );
        assert!(
            add.lhs_promotion_path.is_empty(),
            "Rat is already at the result type"
        );
        assert_eq!(add.rhs_promotion_path.len(), 1, "Int→Rat is one edge");
        assert_eq!(add.rhs_promotion_path[0].kind, CoercionKind::Exact);

        // A two-edge exact path keeps both edges, in order.
        let long = NUMERIC.promotion_path("Nat", "Rat");
        assert_eq!(long.len(), 2);
        assert_eq!(long[0].generator, NUMERIC.promote_generator("Nat", "Int"));
        assert_eq!(long[1].generator, NUMERIC.promote_generator("Int", "Rat"));
        assert!(long.iter().all(|e| e.kind == CoercionKind::Exact));
    }

    #[test]
    fn the_arith_input_bridge_is_minted_but_not_discharged() {
        // The bridge leaf Stage B0 introduces. It is a real generator of this
        // regime — so `verify_structure` accepts a tree citing it — but its
        // realization relation is not a fact the kernel checks, so it is not
        // tight for any claim kind. `1 + 2` stays `Audited` on its account as
        // much as on `g_arith`'s.
        assert!(crate::tree_audit::is_minted_generator(&g_arith_input()));
        assert_eq!(
            generator_name(&g_arith_input()).as_deref(),
            Some("g_arith_input")
        );
        assert!(typing_registry().contains(&g_arith_input()));

        for kind in [ClaimKind::Typing, ClaimKind::Empty] {
            assert!(
                !generator_is_tight(kind, &g_arith_input()),
                "g_arith_input must not be discharged for {kind:?}"
            );
        }

        // It is emitted exactly once per arithmetic node, between the operand
        // tensor and `g_arith`.
        let expr = Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let (_, tree, _) = infer_tree(&expr, &TyCtx::new(), Infer::new()).expect("infers");
        let (bridge_src, bridge_dst) = leaf_endpoints(&tree, &g_arith_input());
        assert_eq!(
            bridge_src,
            TyObj::Prod(
                Box::new(TyObj::Atom(CfgAtom::Type(Ty::Con("Int")))),
                Box::new(TyObj::Atom(CfgAtom::Type(Ty::Con("Int")))),
            ),
            "the bridge consumes the operands' own types"
        );
        assert_eq!(
            bridge_dst,
            leaf_endpoints(&tree, &g_arith()).0,
            "the bridge's dst is exactly g_arith's src — this is the Seq middle"
        );
    }

    #[test]
    fn the_arith_result_bridge_is_minted_but_not_discharged() {
        // Stage B's mirror of the input bridge. `g_arith`'s `dst` is now the
        // kernel's own `NumericResultTypeV1`, so something has to carry it back
        // into this crate's `Ty` vocabulary for the enclosing derivation. That
        // rename is faithful, but "the host renamed it faithfully" is a
        // host-computed claim (ADR-0015 §8.5), and the kernel cannot check it:
        // the `dst` is a `Ty` atom this crate encodes, and reproducing that
        // encoding in the TCB would be a second semantic encoder.
        assert!(crate::tree_audit::is_minted_generator(&g_arith_result()));
        assert_eq!(
            generator_name(&g_arith_result()).as_deref(),
            Some("g_arith_result")
        );
        assert!(typing_registry().contains(&g_arith_result()));

        for kind in [ClaimKind::Typing, ClaimKind::Empty] {
            assert!(
                !generator_is_tight(kind, &g_arith_result()),
                "g_arith_result must not be discharged for {kind:?}"
            );
        }

        let expr = Expr::Arith(ArithOp::Div, Box::new(Expr::Lit(7)), Box::new(Expr::Lit(2)));
        let (ty, tree, _) = infer_tree(&expr, &TyCtx::new(), Infer::new()).expect("infers");
        assert_eq!(ty, Ty::Con("Float"));

        let (bridge_src, bridge_dst) = leaf_endpoints(&tree, &g_arith_result());
        assert_eq!(
            bridge_src,
            leaf_endpoints(&tree, &g_arith()).1,
            "the bridge's src is exactly g_arith's dst — this is the Seq middle"
        );
        assert_eq!(
            bridge_dst,
            TyObj::Atom(CfgAtom::Type(Ty::Con("Float"))),
            "the bridge lands on this crate's own type vocabulary"
        );
    }

    /// The 30 operand pairs that have a join, with the type the operation is
    /// performed at — spelled out rather than computed from `NUMERIC`, so the
    /// gate below is an independent statement of the matrix rather than a
    /// restatement of the code it exercises.
    const JOINABLE: &[(&str, &str, &str)] = &[
        ("Nat", "Nat", "Nat"),
        ("Nat", "Int", "Int"),
        ("Nat", "Rat", "Rat"),
        ("Nat", "Real", "Real"),
        ("Nat", "Complex", "Complex"),
        ("Int", "Nat", "Int"),
        ("Int", "Int", "Int"),
        ("Int", "Rat", "Rat"),
        ("Int", "Real", "Real"),
        ("Int", "Complex", "Complex"),
        ("Rat", "Nat", "Rat"),
        ("Rat", "Int", "Rat"),
        ("Rat", "Rat", "Rat"),
        ("Rat", "Real", "Real"),
        ("Rat", "Complex", "Complex"),
        ("Real", "Nat", "Real"),
        ("Real", "Int", "Real"),
        ("Real", "Rat", "Real"),
        ("Real", "Real", "Real"),
        ("Real", "Complex", "Complex"),
        ("Complex", "Nat", "Complex"),
        ("Complex", "Int", "Complex"),
        ("Complex", "Rat", "Complex"),
        ("Complex", "Real", "Complex"),
        ("Complex", "Complex", "Complex"),
        ("Nat", "Float", "Float"),
        ("Int", "Float", "Float"),
        ("Float", "Nat", "Float"),
        ("Float", "Int", "Float"),
        ("Float", "Float", "Float"),
    ];

    /// ADR-0015 §5 Stage B gate 1 — `arithmetic_rule_is_a_kernel_primitive`.
    ///
    /// For the **exhaustive** finite matrix, not a sample: invoke the real
    /// generator, take its real `g_arith` leaf, build the real `PrimRealizes`
    /// proof term, submit it to the real kernel, and assert `Accepted` for the
    /// precise realization proposition. The result type is compared against the
    /// kernel relation — by asking the relation which result it admits — rather
    /// than against a snapshot string.
    ///
    /// This is the moment the discharge stops being prose. Before it, "`g_arith`
    /// is sound" was a doc comment; after it, it is a membership decision a
    /// checker executes over kernel-owned data.
    ///
    /// It still moves no grade — see `stage_b_moves_no_grade` below.
    #[test]
    fn arithmetic_rule_is_a_kernel_primitive() {
        use brix_kernel::{
            acceptance, resolve_primitive_relation, typing_arith_v2, Budget, CoercionEdgeV1,
            ExplicitTerm, NumericResultTypeV1, ObjectTerm, Prop, TermKind, Verdict,
        };
        use brix_semantic::PropositionId;

        // Stage E: the emission names the lossy edge under the conversion
        // family, so the live relation is V2. The superseded V1 was retired
        // (ADR-0024 §3), so instead of resolving it, each relocated row is
        // re-spelled below under the *legacy* naming and the current relation
        // is required to reject it — the immutability discipline visibly
        // working rather than merely asserted, and without a second row table.
        let relation = resolve_primitive_relation(&typing_arith_v2()).expect("TypingArithV2");
        let mut relocated = 0;
        let ctx_id = ContextId::root();
        let ops = [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div];

        let mut checked = 0;
        for op in ops {
            for (lhs, rhs, base) in JOINABLE {
                let ctx = TyCtx::new()
                    .extend("l", Ty::Con(lhs))
                    .extend("r", Ty::Con(rhs));
                let expr = Expr::Arith(
                    op,
                    Box::new(Expr::Var("l".to_string())),
                    Box::new(Expr::Var("r".to_string())),
                );

                let (inferred, tree, _) =
                    infer_tree(&expr, &ctx, Infer::new()).expect("a joinable pair infers");

                // The real leaf, straight out of the real generator.
                let (src_obj, dst_obj) = leaf_endpoints(&tree, &g_arith());
                let src_cfg = match &src_obj {
                    TyObj::Atom(CfgAtom::ArithInput(i)) => i.config_id(),
                    other => panic!("g_arith's src must be an ArithInput atom, got {other:?}"),
                };
                let dst_cfg = match &dst_obj {
                    TyObj::Atom(CfgAtom::ArithResult(r)) => r.config_id(),
                    other => panic!("g_arith's dst must be an ArithResult atom, got {other:?}"),
                };
                let src = PropositionId(src_cfg.digest());
                let dst = PropositionId(dst_cfg.digest());

                // The precise conclusion: the generator comes from the relation,
                // the endpoints from the leaf.
                let goal = Prop::Realizes(
                    ObjectTerm::Const(PropositionId(relation.generator.digest())),
                    ObjectTerm::Const(src),
                    ObjectTerm::Const(dst),
                );
                let term = ExplicitTerm::new(
                    ctx_id,
                    TermKind::PrimRealizes {
                        relation: typing_arith_v2(),
                        src: ObjectTerm::Const(src),
                        dst: ObjectTerm::Const(dst),
                    },
                );

                let verdict = acceptance(&ctx_id, &goal, &term, Budget::new(10_000, 256));
                assert!(
                    matches!(verdict, Verdict::Accepted(_)),
                    "{op:?} {lhs} {rhs}: the real leaf must be a kernel primitive, got {verdict:?}"
                );

                // The relation's generator is `g_arith`'s — the relation
                // identity fixes it, and the emission agrees.
                assert_eq!(relation.generator, g_arith());

                // Compare the *result type* against the kernel relation rather
                // than a snapshot: exactly one result type is admitted for this
                // source object, and it is the one inference produced.
                let want = if op == ArithOp::Div {
                    match *base {
                        "Nat" | "Int" => "Float",
                        other => other,
                    }
                } else {
                    base
                };
                assert_eq!(inferred, Ty::Con(want), "{op:?} {lhs} {rhs}");

                let admitted: Vec<&str> = ["Nat", "Int", "Rat", "Real", "Complex", "Float"]
                    .into_iter()
                    .filter(|name| {
                        let candidate = NumericResultTypeV1 {
                            name: NumericTypeNameV1::from_lattice_node(name).expect("numeric"),
                        };
                        relation.admits(&src, &PropositionId(candidate.config_id().digest()))
                    })
                    .collect();
                assert_eq!(
                    admitted,
                    vec![want],
                    "{op:?} {lhs} {rhs}: the relation must admit exactly one result type"
                );

                // Re-spell this row's paths the way the retired V1 did, with
                // every edge in the promotion family. If the path crosses the
                // relocated edge that is a different source object, and the
                // current relation must not admit it.
                let input = match &src_obj {
                    TyObj::Atom(CfgAtom::ArithInput(i)) => i.clone(),
                    _ => unreachable!("checked above"),
                };
                let as_legacy = |path: &[CoercionEdgeV1]| -> Vec<CoercionEdgeV1> {
                    path.iter()
                        .map(|e| CoercionEdgeV1 {
                            generator: match e.kind {
                                CoercionKind::Exact => e.generator,
                                CoercionKind::Lossy => {
                                    GeneratorId::named("type.rule.num.promote.Int_Float@1")
                                }
                            },
                            kind: e.kind,
                        })
                        .collect()
                };
                let crosses_lossy = input
                    .lhs_promotion_path
                    .iter()
                    .chain(input.rhs_promotion_path.iter())
                    .any(|e| e.kind == CoercionKind::Lossy);
                let legacy = ArithTypingInputV1 {
                    lhs_promotion_path: as_legacy(&input.lhs_promotion_path),
                    rhs_promotion_path: as_legacy(&input.rhs_promotion_path),
                    ..input.clone()
                };
                let legacy_src = PropositionId(legacy.config_id().digest());
                assert_eq!(
                    legacy_src == src,
                    !crosses_lossy,
                    "{op:?} {lhs} {rhs}: only a lossy-crossing path is re-spelled"
                );
                if crosses_lossy {
                    assert!(
                        !relation.admits(&legacy_src, &dst),
                        "{op:?} {lhs} {rhs}: the legacy spelling must not be admitted"
                    );
                    relocated += 1;
                }

                checked += 1;
            }
        }

        assert_eq!(
            relocated, 20,
            "Stage E relocates exactly the rows whose path crosses Int -> Float"
        );

        assert_eq!(
            checked,
            JOINABLE.len() * ops.len(),
            "the matrix must be exhaustive, not sampled"
        );
        assert_eq!(checked, relation.rows.len());
    }

    /// The pairs with no join produce no derivation at all, so no leaf exists to
    /// submit. Gate 3's companion on the emission side: the absent rows are
    /// unreachable rather than merely unmatched.
    #[test]
    fn arithmetic_on_an_unjoinable_pair_never_reaches_a_leaf() {
        for (lhs, rhs) in [
            ("Float", "Rat"),
            ("Rat", "Float"),
            ("Float", "Real"),
            ("Complex", "Float"),
        ] {
            for op in [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div] {
                let ctx = TyCtx::new()
                    .extend("l", Ty::Con(lhs))
                    .extend("r", Ty::Con(rhs));
                let expr = Expr::Arith(
                    op,
                    Box::new(Expr::Var("l".to_string())),
                    Box::new(Expr::Var("r".to_string())),
                );
                assert_eq!(
                    infer_tree(&expr, &ctx, Infer::new()).err(),
                    Some(TypeError::Mismatch),
                    "{op:?} {lhs} {rhs} has no join and must not type"
                );
            }
        }
    }

    /// **Stage B moves no grade.** ADR-0015 §5 Stage D is explicit that a
    /// boolean flip in `generator_is_tight` is the wrong mechanism, and §7 that
    /// "shipping the registry upgrades nothing retroactively": a leaf is closed
    /// only when a certificate actually contains the `PrimRealizes` term and the
    /// kernel accepted the resulting proof. `elaborate_tree` still emits every
    /// leaf as a `Hyp`, so nothing here regrades.
    ///
    /// Two further reasons `1 + 2` is still capped, and they are the finding
    /// Stage B surfaced: `g_arith_input` and `g_arith_result` are the
    /// regime↔kernel vocabulary bridges either side of the kernel-checked leaf,
    /// and neither is dischargeable by this mechanism.
    #[test]
    fn stage_b_moves_no_grade() {
        let expr = Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let (_, tree, st) = infer_tree(&expr, &TyCtx::new(), Infer::new()).expect("infers");
        let realizes = materialize(&tree, &st.subst);

        assert_eq!(
            honest_result_outcome(Outcome::Proven, &realizes),
            Outcome::Audited,
            "shipping the registry must not lift the arithmetic cap"
        );

        for g in [g_arith(), g_arith_input(), g_arith_result()] {
            assert!(
                !generator_is_tight(ClaimKind::Typing, &g),
                "{:?} must still not be tight for typing",
                generator_name(&g)
            );
        }

        // …and the split, discharged in Stage C, is unaffected in the other
        // direction: a landed discharge does not lapse because a neighbour
        // changed.
        assert!(generator_is_tight(ClaimKind::Typing, &g_arith_split()));
    }

    #[test]
    fn literal_intro_generators_are_faithful() {
        // Each discharged literal generator is emitted at exactly one site with a
        // fixed `literal → Con` endpoint. This pins the discharge: if a literal
        // arm ever emitted a different generator or result type, tightness would
        // no longer be honest and this test would catch it.
        let cases: [(Expr, GeneratorId, Ty); 3] = [
            (Expr::Lit(7), g_lit(), Ty::Con("Int")),
            (Expr::StrLit("hi".to_string()), g_str_lit(), Ty::Con("Str")),
            (
                Expr::FloatLit("1.5".to_string()),
                g_float_lit(),
                Ty::Con("Float"),
            ),
        ];
        for (expr, gen, ty) in cases {
            assert!(
                generator_is_tight(ClaimKind::Typing, &gen),
                "{gen:?} should be discharged"
            );
            let (_, tree, _) = infer_tree(&expr, &TyCtx::new(), Infer::new()).unwrap();
            match tree {
                TyTree::Leaf {
                    generator,
                    src,
                    dst,
                } => {
                    assert_eq!(generator, gen);
                    assert_eq!(src, TyObj::Atom(CfgAtom::Expr(expr)));
                    assert_eq!(dst, TyObj::Atom(CfgAtom::Type(ty)));
                }
                other => panic!("expected a single faithful leaf, got {other:?}"),
            }
        }
    }

    #[test]
    fn literal_binding_earns_proven_but_composite_stays_audited() {
        // A pure literal rests only on a discharged (tight) generator, so its
        // honest result outcome is the kernel's Proven composition.
        let (_, lit_tree) =
            audited_type_check_tree(&Expr::Lit(42), &TyCtx::new(), ContextId::root()).unwrap();
        assert_eq!(
            honest_result_outcome(Outcome::Proven, lit_tree.tree()),
            Outcome::Proven,
            "a discharged literal should earn Proven"
        );

        // `(λx.x) 42` rests on the discharged λ-calculus core (g_var/g_lam_*/
        // g_split/g_app2) + g_lit — all tight — so it now earns Proven too.
        let app = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let (_, app_tree) =
            audited_type_check_tree(&app, &TyCtx::new(), ContextId::root()).unwrap();
        assert_eq!(
            honest_result_outcome(Outcome::Proven, app_tree.tree()),
            Outcome::Proven,
            "the discharged λ-calculus core should earn Proven"
        );

        // `1 + 2` uses g_arith, which asserts operation semantics and is NOT
        // discharged, so it is honestly capped at Audited.
        let arith = Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let (_, arith_tree) =
            audited_type_check_tree(&arith, &TyCtx::new(), ContextId::root()).unwrap();
        assert_eq!(
            honest_result_outcome(Outcome::Proven, arith_tree.tree()),
            Outcome::Audited,
            "an undischarged operation generator (g_arith) must cap at Audited"
        );
    }

    /// The soundness evidence for discharging `g_record_empty` and
    /// `g_ctor_nullary` — the zero-premise introduction family.
    ///
    /// These are discharged on `g_lit`'s ground, not on a kernel
    /// correspondence: an introduction rule *is* the definition of its type,
    /// and with no premises there is no composition for a kernel rule to
    /// check. What has to be pinned instead is that the emission stays faithful
    /// to that story, so this test carries the four obligations the story
    /// implies.
    #[test]
    fn zero_arity_intro_generators_are_faithful() {
        let ctx_id = ContextId::root();

        // (1) Zero premises: each derives as a single leaf, with no
        //     subderivation and no split. If either ever composes something,
        //     the "nothing to check" argument lapses and so does the discharge.
        let (_, empty) = audited_type_check_tree(&Expr::Record(vec![]), &TyCtx::new(), ctx_id)
            .expect("empty record");
        assert_eq!(empty.tree().leaves().len(), 1);

        let opt = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let (_, none) = audited_type_check_tree(
            &Expr::Ctor(opt.clone(), "None".into(), vec![]),
            &TyCtx::new(),
            ctx_id,
        )
        .expect("nullary ctor");
        assert_eq!(none.tree().leaves().len(), 1);

        // (2) `g_record_empty` has exactly one instance: a fixed endpoint pair,
        //     the same pin `literal_intro_generators_are_faithful` applies to
        //     the literal rules.
        match empty.tree() {
            RealizesTree::Leaf {
                generator,
                src,
                dst,
            } => {
                assert_eq!(*generator, g_record_empty());
                assert_eq!(*src, TreeObj::Atom(Expr::Record(vec![]).config_id()));
                assert_eq!(*dst, TreeObj::Atom(Ty::Record(vec![]).config_id()));
            }
            other => panic!("expected a single leaf, got {other:?}"),
        }

        // (3) `g_ctor_nullary` ranges over sum types, so instead of a fixed
        //     endpoint the invariant is that `dst` is *always exactly* the sum
        //     type named inside `src`. Checked across several distinct sums so
        //     a hardcoded result type would fail.
        // A recursive config is included deliberately. #298 made this branch
        // read its variants through `.unfold()`, so the precondition is now
        // "declared by *unfold(sum_ty)* with zero fields" — strictly more host
        // logic between `src` and the emitted claim than when this gate was
        // written, and unfolding is exactly where a bug would admit a variant
        // that is not really there. The `dst` must stay the **folded** type:
        // `Nil` is a `List`, not a one-step unfolding of one.
        let list = Ty::Rec(
            "List".into(),
            Box::new(Ty::Sum(
                "List".into(),
                vec![
                    ("Nil".into(), vec![]),
                    (
                        "Cons".into(),
                        vec![Ty::Con("Int"), Ty::RecVar("List".into())],
                    ),
                ],
            )),
        );

        for (sum, variant) in [
            (list.clone(), "Nil"),
            (opt.clone(), "None"),
            (
                Ty::Sum(
                    "Bool".into(),
                    vec![("True".into(), vec![]), ("False".into(), vec![])],
                ),
                "False",
            ),
            (
                Ty::Sum("Unit".into(), vec![("Only".into(), vec![])]),
                "Only",
            ),
        ] {
            let expr = Expr::Ctor(sum.clone(), variant.into(), vec![]);
            let (_, derivation) =
                audited_type_check_tree(&expr, &TyCtx::new(), ctx_id).expect("types");
            match derivation.tree() {
                RealizesTree::Leaf {
                    generator,
                    src,
                    dst,
                } => {
                    assert_eq!(*generator, g_ctor_nullary());
                    assert_eq!(*src, TreeObj::Atom(expr.config_id()));
                    assert_eq!(
                        *dst,
                        TreeObj::Atom(sum.config_id()),
                        "dst must be exactly the sum type named in src"
                    );
                }
                other => panic!("expected a single leaf, got {other:?}"),
            }
        }

        // (4) The precondition is enforced *before* emission, so a leaf never
        //     occurs for a variant the sum does not declare or for one whose
        //     arity disagrees. This is what holds the claim up — not
        //     per-leaf verifiability, which a digest endpoint cannot offer
        //     (ADR-0025 §1): every leaf that exists is one whose precondition
        //     held at emission.
        for (label, expr) in [
            (
                "undeclared variant",
                Expr::Ctor(opt.clone(), "Nope".into(), vec![]),
            ),
            (
                "nullary spelling of a payload variant",
                Expr::Ctor(opt.clone(), "Some".into(), vec![]),
            ),
            (
                "undeclared variant of a recursive config",
                Expr::Ctor(list.clone(), "Empty".into(), vec![]),
            ),
            (
                "nullary spelling of a recursive payload variant",
                Expr::Ctor(list.clone(), "Cons".into(), vec![]),
            ),
        ] {
            assert_eq!(
                audited_type_check_tree(&expr, &TyCtx::new(), ctx_id).map(|_| ()),
                Err(TypeError::Mismatch),
                "{label} must not derive at all"
            );
        }

        // (5) Both are tight for typing only — the discharge does not travel to
        //     another judgment kind (⟨D-JUDGE⟩).
        for g in [g_record_empty(), g_ctor_nullary()] {
            assert!(generator_is_tight(ClaimKind::Typing, &g));
            assert!(!generator_is_tight(ClaimKind::Empty, &g));
        }
    }

    #[test]
    fn application_rule_is_a_kernel_theorem() {
        // Soundness evidence for discharging g_app2: its realization is modus
        // ponens `((A→B) × A) → B`, a kernel theorem witnessed by λp.(π₁p)(π₂p).
        use brix_kernel::{ExplicitTerm, Prop, TermKind, Var, Verdict};
        use brix_semantic::PropositionId;

        let a = Prop::Atom(PropositionId::from_canon(b"A"));
        let b = Prop::Atom(PropositionId::from_canon(b"B"));
        let a_to_b = Prop::Impl(Box::new(a.clone()), Box::new(b.clone()));
        let goal = Prop::Impl(
            Box::new(Prop::Prod(Box::new(a_to_b), Box::new(a))),
            Box::new(b),
        );
        let term = TermKind::Lam {
            var_name: Some("p".to_string()),
            body: Box::new(TermKind::App {
                function: Box::new(TermKind::Proj1(Box::new(TermKind::Hyp(Var::Named(
                    "p".to_string(),
                ))))),
                argument: Box::new(TermKind::Proj2(Box::new(TermKind::Hyp(Var::Named(
                    "p".to_string(),
                ))))),
            }),
        };
        let ctx = ContextId::root();
        let explicit = ExplicitTerm::new(ctx, term);
        assert!(
            matches!(
                brix_kernel::acceptance(&ctx, &goal, &explicit, Budget::new(1000, 1000)),
                Verdict::Accepted(_)
            ),
            "modus ponens must be a kernel theorem"
        );
    }

    #[test]
    fn structural_generators_are_faithful_kernel_rules() {
        // The discharged structural rules are precisely the primitive rules the
        // kernel accepts: product introduction/projection and coproduct
        // introduction/elimination.  The n-ary Brix forms use right-nested
        // binary products/coproducts, so the examples below pin that encoding
        // rather than assuming an n-ary rule in the kernel.
        use brix_kernel::{ExplicitTerm, Prop, TermKind, Var, Verdict};
        use brix_semantic::PropositionId;

        let atom = |name| Prop::Atom(PropositionId::from_canon(name));
        let a = atom(b"A");
        let b = atom(b"B");
        let c = atom(b"C");
        let r = atom(b"R");
        let ctx = ContextId::root();
        let accepts = |goal, term| {
            matches!(
                brix_kernel::acceptance(
                    &ctx,
                    &goal,
                    &ExplicitTerm::new(ctx, term),
                    Budget::new(1_000, 1_000),
                ),
                Verdict::Accepted(_)
            )
        };

        // g_record: A -> B -> A × B (product introduction).
        let product_intro = Prop::Impl(
            Box::new(a.clone()),
            Box::new(Prop::Impl(
                Box::new(b.clone()),
                Box::new(Prop::Prod(Box::new(a.clone()), Box::new(b.clone()))),
            )),
        );
        assert!(accepts(
            product_intro,
            TermKind::Lam {
                var_name: Some("a".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("b".into()),
                    body: Box::new(TermKind::Pair {
                        fst: Box::new(TermKind::Hyp(Var::Named("a".into()))),
                        snd: Box::new(TermKind::Hyp(Var::Named("b".into()))),
                    }),
                }),
            },
        ));

        // g_field: the final field of A × (B × C) is selected with the
        // right-nested product projection π₂(π₂ p).
        let nested_product = Prop::Prod(
            Box::new(a.clone()),
            Box::new(Prop::Prod(Box::new(b.clone()), Box::new(c.clone()))),
        );
        assert!(accepts(
            Prop::Impl(Box::new(nested_product), Box::new(c.clone())),
            TermKind::Lam {
                var_name: Some("p".into()),
                body: Box::new(TermKind::Proj2(Box::new(TermKind::Proj2(Box::new(
                    TermKind::Hyp(Var::Named("p".into())),
                ))))),
            },
        ));

        // g_ctor: inject the middle alternative into A + (B + C), matching
        // Brix's right-nested nominal-sum representation.
        let nested_sum = Prop::Sum(
            Box::new(a.clone()),
            Box::new(Prop::Sum(Box::new(b.clone()), Box::new(c.clone()))),
        );
        assert!(accepts(
            Prop::Impl(Box::new(b.clone()), Box::new(nested_sum.clone())),
            TermKind::Lam {
                var_name: Some("b".into()),
                body: Box::new(TermKind::Inr(Box::new(TermKind::Inl(Box::new(
                    TermKind::Hyp(Var::Named("b".into())),
                ))))),
            },
        ));

        // g_match: (A -> R) -> (B -> R) -> (A + B -> R), the kernel's
        // coproduct eliminator. Pattern coverage is deliberately separate: a
        // `proving exhaustive` annotation may still have no coverage
        // certificate even when ordinary match typing is `Proven`.
        let match_elim = Prop::Impl(
            Box::new(Prop::Impl(Box::new(a.clone()), Box::new(r.clone()))),
            Box::new(Prop::Impl(
                Box::new(Prop::Impl(Box::new(b.clone()), Box::new(r.clone()))),
                Box::new(Prop::Impl(
                    Box::new(Prop::Sum(Box::new(a.clone()), Box::new(b.clone()))),
                    Box::new(r),
                )),
            )),
        );
        assert!(accepts(
            match_elim,
            TermKind::Lam {
                var_name: Some("ha".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("hb".into()),
                    body: Box::new(TermKind::Lam {
                        var_name: Some("s".into()),
                        body: Box::new(TermKind::Case {
                            discriminant: Box::new(TermKind::Hyp(Var::Named("s".into()))),
                            left_var: Some("x".into()),
                            left_body: Box::new(TermKind::App {
                                function: Box::new(TermKind::Hyp(Var::Named("ha".into()))),
                                argument: Box::new(TermKind::Hyp(Var::Named("x".into()))),
                            }),
                            right_var: Some("y".into()),
                            right_body: Box::new(TermKind::App {
                                function: Box::new(TermKind::Hyp(Var::Named("hb".into()))),
                                argument: Box::new(TermKind::Hyp(Var::Named("y".into()))),
                            }),
                        }),
                    }),
                }),
            },
        ));

        // The empty record and a nullary constructor are still NOT smuggled into
        // those binary rules — Profile 1.2 lacks unit/zero rules, and that has
        // not changed. What changed is that they no longer need one: both are
        // discharged as zero-premise *introductions* (see their docs and
        // `zero_arity_intro_generators_are_faithful`), which is the ground
        // `g_lit` stands on, not a kernel correspondence. The assertions below
        // pin that they reach `Proven` **without** appearing in any of the four
        // structural rules above.
        fn has_leaf(tree: &RealizesTree, want: &GeneratorId) -> bool {
            match tree {
                RealizesTree::Leaf { generator, .. } => generator == want,
                RealizesTree::Seq { left, right } | RealizesTree::Tensor { left, right } => {
                    has_leaf(left, want) || has_leaf(right, want)
                }
            }
        }

        let empty_record = Expr::Record(vec![]);
        let (_, empty_tree) =
            audited_type_check_tree(&empty_record, &TyCtx::new(), ctx).expect("empty record");
        assert!(has_leaf(empty_tree.tree(), &g_record_empty()));
        assert!(
            !has_leaf(empty_tree.tree(), &g_record())
                && !has_leaf(empty_tree.tree(), &g_record_split()),
            "the empty record must not borrow the binary product rules"
        );
        assert_eq!(
            honest_result_outcome(Outcome::Proven, empty_tree.tree()),
            Outcome::Proven
        );

        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        let nullary_ctor = Expr::Ctor(bool_ty, "True".into(), vec![]);
        let (_, nullary_tree) =
            audited_type_check_tree(&nullary_ctor, &TyCtx::new(), ctx).expect("nullary ctor");
        assert!(has_leaf(nullary_tree.tree(), &g_ctor_nullary()));
        assert!(
            !has_leaf(nullary_tree.tree(), &g_ctor())
                && !has_leaf(nullary_tree.tree(), &g_ctor_split()),
            "a nullary constructor must not borrow the binary coproduct rules"
        );
        assert_eq!(
            honest_result_outcome(Outcome::Proven, nullary_tree.tree()),
            Outcome::Proven
        );
    }

    #[test]
    fn arithmetic_split_rule_is_a_kernel_primitive() {
        // ADR-0015 Stage C / ⟨D-SPLIT⟩ — the soundness evidence for
        // discharging `g_arith_split`. Four obligations, in the ADR's own
        // order: exactly two ordered child obligations; context and operator
        // preserved; no promotion chosen and no result type synthesised; a
        // malformed arity or forged child rejected.
        use brix_kernel::{ExplicitTerm, Prop, TermKind, Var, Verdict};
        use brix_semantic::PropositionId;

        let ctx_id = ContextId::root();

        // (i) Exactly two ordered child obligations, and the packaging is the
        //     kernel's own binary product introduction — the same rule the
        //     other `*_split` leaves rest on (`g_record_split` /
        //     `g_field_split` / `g_ctor_split` / `g_match_split`), which is
        //     precisely the "same structural grounds" ⟨D-SPLIT⟩ appeals to.
        let atom = |name| Prop::Atom(PropositionId::from_canon(name));
        let (lhs, rhs) = (atom(b"lhs obligation"), atom(b"rhs obligation"));
        let pair_formation = Prop::Impl(
            Box::new(lhs.clone()),
            Box::new(Prop::Impl(
                Box::new(rhs.clone()),
                Box::new(Prop::Prod(Box::new(lhs), Box::new(rhs))),
            )),
        );
        assert!(
            matches!(
                brix_kernel::acceptance(
                    &ctx_id,
                    &pair_formation,
                    &ExplicitTerm::new(
                        ctx_id,
                        TermKind::Lam {
                            var_name: Some("l".into()),
                            body: Box::new(TermKind::Lam {
                                var_name: Some("r".into()),
                                body: Box::new(TermKind::Pair {
                                    fst: Box::new(TermKind::Hyp(Var::Named("l".into()))),
                                    snd: Box::new(TermKind::Hyp(Var::Named("r".into()))),
                                }),
                            }),
                        },
                    ),
                    Budget::new(1_000, 1_000),
                ),
                Verdict::Accepted(_)
            ),
            "the split's packaging must be the kernel's product introduction"
        );

        let split_of = |expr: &Expr, ctx: &TyCtx| {
            let (_, tree, _) = infer_tree(expr, ctx, Infer::new()).expect("infers");
            leaf_endpoints(&tree, &g_arith_split())
        };

        let one_plus_two =
            Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let (src, dst) = split_of(&one_plus_two, &TyCtx::new());
        assert_eq!(src, TyObj::Atom(CfgAtom::Expr(one_plus_two.clone())));
        assert_eq!(
            dst,
            TyObj::Prod(
                Box::new(TyObj::Atom(CfgAtom::Expr(Expr::Lit(1)))),
                Box::new(TyObj::Atom(CfgAtom::Expr(Expr::Lit(2)))),
            ),
            "exactly two children, in source order"
        );

        // (ii) The operator is preserved — it is bound because `src` is the
        //      whole node, so two nodes differing only in operator have
        //      different splits. And operand order is preserved likewise.
        let one_minus_two =
            Expr::Arith(ArithOp::Sub, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let two_minus_one =
            Expr::Arith(ArithOp::Sub, Box::new(Expr::Lit(2)), Box::new(Expr::Lit(1)));
        assert_ne!(
            split_of(&one_plus_two, &TyCtx::new()).0,
            split_of(&one_minus_two, &TyCtx::new()).0,
            "a different operator must be a different split source"
        );
        assert_ne!(
            split_of(&one_minus_two, &TyCtx::new()).1,
            split_of(&two_minus_one, &TyCtx::new()).1,
            "swapped operands must be a different split target"
        );

        // (iii) No promotion is chosen and no result type is synthesised.
        //
        //       The direct demonstration: one expression, two contexts that
        //       force *different* promotions and different result types, and a
        //       byte-identical split leaf. `x + y` types as `Int` under the
        //       first (no promotion) and `Rat` under the second (splicing
        //       Int↪Rat). If the split had any promotion or result-type
        //       content, these could not agree — and ⟨D-SPLIT⟩'s conditional
        //       would have lapsed.
        let x_plus_y = Expr::Arith(
            ArithOp::Add,
            Box::new(Expr::Var("x".to_string())),
            Box::new(Expr::Var("y".to_string())),
        );
        let int_ctx = TyCtx::new()
            .extend("x", Ty::Con("Int"))
            .extend("y", Ty::Con("Int"));
        let rat_ctx = TyCtx::new()
            .extend("x", Ty::Con("Rat"))
            .extend("y", Ty::Con("Int"));
        assert_eq!(
            infer_tree(&x_plus_y, &int_ctx, Infer::new()).unwrap().0,
            Ty::Con("Int")
        );
        assert_eq!(
            infer_tree(&x_plus_y, &rat_ctx, Infer::new()).unwrap().0,
            Ty::Con("Rat"),
            "the second context must genuinely force a different result"
        );
        assert_eq!(
            split_of(&x_plus_y, &int_ctx),
            split_of(&x_plus_y, &rat_ctx),
            "the split must not vary with the promotion or the result type"
        );

        // Structurally: neither endpoint mentions a type or an arithmetic
        // input at all. A `Type` or `ArithInput` atom anywhere in the split
        // would be a representation claim it is not entitled to make.
        fn only_expressions(obj: &TyObj) -> bool {
            match obj {
                TyObj::Atom(CfgAtom::Expr(_)) => true,
                TyObj::Atom(CfgAtom::Type(_))
                | TyObj::Atom(CfgAtom::ArithInput(_))
                | TyObj::Atom(CfgAtom::ArithResult(_)) => false,
                TyObj::Prod(l, r) => only_expressions(l) && only_expressions(r),
            }
        }
        let (src, dst) = split_of(&x_plus_y, &rat_ctx);
        assert!(only_expressions(&src) && only_expressions(&dst));

        // (iv) A forged child is rejected. Substituting a different
        //      subexpression into the split's target breaks the `Seq` middle
        //      against the operand tensor, and the audit refuses to produce an
        //      artifact rather than downgrading one.
        let (_, real_tree) =
            audited_type_check_tree(&one_plus_two, &TyCtx::new(), ctx_id).expect("audited");
        let forged = forge_split_child(real_tree.tree(), &Expr::Lit(99).config_id());
        match audit_tree(
            &forged,
            one_plus_two.config_id(),
            Ty::Con("Int").config_id(),
        ) {
            Err(crate::tree_audit::TreeAuditError::MalformedTree) => {}
            other => panic!("a forged split child must never audit clean, got {other:?}"),
        }

        // And the discharge moves no grade: `1 + 2` still rests on the
        // undischarged `g_arith`/`g_arith_input`, so it stays capped.
        assert!(generator_is_tight(ClaimKind::Typing, &g_arith_split()));
        assert!(!generator_is_tight(ClaimKind::Empty, &g_arith_split()));
        assert_eq!(
            honest_result_outcome(Outcome::Proven, real_tree.tree()),
            Outcome::Audited,
            "discharging the split alone must not lift the arithmetic cap"
        );
    }

    /// Replace the right child of the `g_arith_split` leaf's target with
    /// `impostor`, leaving everything else intact.
    fn forge_split_child(tree: &RealizesTree, impostor: &ConfigId) -> RealizesTree {
        match tree {
            RealizesTree::Leaf {
                generator,
                src,
                dst,
            } if *generator == g_arith_split() => {
                let forged_dst = match dst {
                    TreeObj::Prod(left, _) => {
                        TreeObj::Prod(left.clone(), Box::new(TreeObj::Atom(*impostor)))
                    }
                    other => other.clone(),
                };
                RealizesTree::Leaf {
                    generator: *generator,
                    src: src.clone(),
                    dst: forged_dst,
                }
            }
            RealizesTree::Leaf { .. } => tree.clone(),
            RealizesTree::Seq { left, right } => RealizesTree::Seq {
                left: Box::new(forge_split_child(left, impostor)),
                right: Box::new(forge_split_child(right, impostor)),
            },
            RealizesTree::Tensor { left, right } => RealizesTree::Tensor {
                left: Box::new(forge_split_child(left, impostor)),
                right: Box::new(forge_split_child(right, impostor)),
            },
        }
    }

    #[test]
    fn structural_tree_endpoints_match_the_checked_expression() {
        // The tree handed to elaboration must prove exactly the same source and
        // target as the audited typing claim. In particular, field projection
        // needs an explicit `g_field_split`: without it the old tree began at
        // the base expression rather than at `base.field`.
        let assert_endpoints = |expr: Expr, ctx: TyCtx| {
            let (ty, _tree, state) = infer_tree(&expr, &ctx, Infer::new()).expect("infer");
            let expected_src = TreeObj::Atom(expr.config_id());
            let expected_dst = TreeObj::Atom(zonk(&ty, &state.subst).config_id());
            let (_audited, tree) =
                audited_type_check_tree(&expr, &ctx, ContextId::root()).expect("audited");
            assert_eq!(
                tree.tree().src(),
                expected_src,
                "wrong tree source for {expr:?}"
            );
            assert_eq!(
                tree.tree().dst(),
                expected_dst,
                "wrong tree target for {expr:?}"
            );
        };

        assert_endpoints(Expr::Record(vec![("x".into(), Expr::Lit(1))]), TyCtx::new());
        assert_endpoints(
            Expr::Record(vec![("x".into(), Expr::Lit(1)), ("y".into(), Expr::Lit(2))]),
            TyCtx::new(),
        );
        assert_endpoints(
            Expr::Field(Box::new(Expr::Var("r".into())), "x".into()),
            TyCtx::new().extend("r", Ty::Record(vec![("x".into(), Ty::Con("Int"))])),
        );

        let opt = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        assert_endpoints(
            Expr::Ctor(opt.clone(), "Some".into(), vec![Expr::Lit(1)]),
            TyCtx::new(),
        );
        let pair = Ty::Sum(
            "Pair".into(),
            vec![("MkPair".into(), vec![Ty::Con("Int"), Ty::Con("Str")])],
        );
        assert_endpoints(
            Expr::Ctor(
                pair,
                "MkPair".into(),
                vec![Expr::Lit(1), Expr::StrLit("x".into())],
            ),
            TyCtx::new(),
        );
        assert_endpoints(
            Expr::Match(
                Box::new(Expr::Var("o".into())),
                vec![
                    (Pattern::Ctor("None".into(), vec![]), Expr::Lit(0)),
                    (
                        Pattern::Ctor("Some".into(), vec![Pattern::Var("n".into())]),
                        Expr::Var("n".into()),
                    ),
                ],
            ),
            TyCtx::new().extend("o", opt),
        );
        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        assert_endpoints(
            Expr::Match(
                Box::new(Expr::Var("b".into())),
                vec![
                    (Pattern::Ctor("True".into(), vec![]), Expr::Lit(1)),
                    (Pattern::Wildcard, Expr::Lit(0)),
                ],
            ),
            TyCtx::new().extend("b", bool_ty),
        );
    }

    /// Renamed from `test_ctor_nullary_bool_stays_audited`: the composition
    /// outcome is now `Proven`, because `g_ctor_nullary` is discharged as a
    /// zero-premise introduction. The *audited* judgement stays `Audited` —
    /// that is `audited_type_check_tree`'s own level and is unrelated to the
    /// cap.
    #[test]
    fn test_ctor_nullary_bool_reaches_proven() {
        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        let expr = Expr::Ctor(bool_ty.clone(), "True".into(), vec![]);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer ctor nullary");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, bool_ty);

        let (aud, tree) =
            audited_type_check_tree(&expr, &ctx, context).expect("audited ctor nullary");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert_eq!(
            honest_result_outcome(Outcome::Proven, tree.tree()),
            Outcome::Proven,
            "a nullary constructor is a zero-premise introduction, discharged on \
             the same ground as a literal rather than as coproduct introduction"
        );
        assert!(tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_ctor_opt_some_proven() {
        let opt_ty = Ty::Sum(
            "Option".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Ctor(opt_ty.clone(), "Some".into(), vec![Expr::Lit(3)]);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer ctor opt");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, opt_ty);

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited ctor opt");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_ctor_pair_proven() {
        let pair_ty = Ty::Sum(
            "Pair".into(),
            vec![("MkPair".into(), vec![Ty::Con("Int"), Ty::Con("Str")])],
        );
        let expr = Expr::Ctor(
            pair_ty.clone(),
            "MkPair".into(),
            vec![Expr::Lit(1), Expr::StrLit("x".into())],
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer ctor pair");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, pair_ty);

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited ctor pair");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_ctor_arg_type_mismatch() {
        let opt_ty = Ty::Sum(
            "Option".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Ctor(opt_ty, "Some".into(), vec![Expr::StrLit("bad".into())]);
        let ctx = TyCtx::new();

        let res = audited_type_check_tree(&expr, &ctx, ContextId::root());
        assert_eq!(res, Err(TypeError::Mismatch));
    }

    #[test]
    fn test_ctor_unknown_variant() {
        let opt_ty = Ty::Sum(
            "Option".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Ctor(opt_ty, "Nope".into(), vec![]);
        let ctx = TyCtx::new();

        let res = audited_type_check_tree(&expr, &ctx, ContextId::root());
        assert_eq!(res, Err(TypeError::Mismatch));
    }

    #[test]
    fn structural_generators_are_tight() {
        for generator in [
            g_record_split(),
            g_record(),
            g_field_split(),
            g_field(),
            g_ctor_split(),
            g_ctor(),
            g_match_split(),
            g_match(),
        ] {
            assert!(
                generator_is_tight(ClaimKind::Typing, &generator),
                "{generator:?} should be discharged"
            );
        }
        // The two zero-arity introductions are discharged, but on
        // literal-introduction grounds rather than as part of this structural
        // family — see their docs and
        // `zero_arity_intro_generators_are_faithful`.
        for generator in [g_record_empty(), g_ctor_nullary()] {
            assert!(
                generator_is_tight(ClaimKind::Typing, &generator),
                "{generator:?} is discharged as a zero-premise introduction"
            );
        }
        assert!(
            !generator_is_tight(ClaimKind::Typing, &g_match_catchall()),
            "g_match_catchall must remain undischarged: ADR-0015 §4 lists it as \
             a non-goal until repeated branch premises are represented"
        );
    }

    #[test]
    fn claim_kind_typing_discharge_is_not_portable() {
        // ADR-0015 Stage A gate: a generator tight for typing is NOT tight for
        // the empty claim kind. This is the entire point of `ClaimKind` — it
        // makes ⟨D-JUDGE⟩'s scoping invariant true by construction rather
        // than true only because no second claim kind exists yet.
        let tight_for_typing = [
            g_lit(),
            g_str_lit(),
            g_float_lit(),
            g_var(),
            g_lam_intro(),
            g_lam_close(),
            g_split(),
            g_app2(),
            g_record_split(),
            g_record(),
            g_field_split(),
            g_field(),
            g_ctor_split(),
            g_ctor(),
            g_match_split(),
            g_match(),
            // ADR-0015 Stage C ⟨D-SPLIT⟩ — discharged on the same structural
            // grounds as the `*_split` leaves above, independently of
            // `g_arith`.
            g_arith_split(),
            // Zero-premise introductions, discharged on `g_lit`'s ground
            // rather than as product/coproduct introduction.
            g_record_empty(),
            g_ctor_nullary(),
            // L2 (#296, #297).
            g_bool_lit(),
            g_cmp_split(),
        ];

        // **Exhaustive by construction, not by a hand-maintained count.** This
        // assertion used to be `tight_for_typing.len() == N`, which measured
        // the literal above against a number written beside it and so could
        // never detect an omission — `g_bool_lit` and `g_cmp_split` were
        // discharged in #296/#297 and went unlisted here while it passed.
        // Deriving the actual set from the single-source enumeration means a
        // newly-discharged generator fails this test until it is classified.
        let declared_tight: std::collections::BTreeSet<GeneratorId> = minted_generators()
            .into_iter()
            .map(|(_, g)| g)
            .filter(|g| generator_is_tight(ClaimKind::Typing, g))
            .collect();
        let listed: std::collections::BTreeSet<GeneratorId> =
            tight_for_typing.iter().cloned().collect();
        assert_eq!(
            listed,
            declared_tight,
            "every typing-tight generator must be listed here; missing = {:?}, stale = {:?}",
            declared_tight.difference(&listed).collect::<Vec<_>>(),
            listed.difference(&declared_tight).collect::<Vec<_>>(),
        );
        for generator in tight_for_typing {
            assert!(
                generator_is_tight(ClaimKind::Typing, &generator),
                "{generator:?} should be discharged for Typing"
            );
            assert!(
                !generator_is_tight(ClaimKind::Empty, &generator),
                "{generator:?} is tight for Typing but must NOT be tight for the empty claim kind"
            );
        }

        // The empty claim kind also returns false for the generators that are
        // undischarged even for typing, and for an arbitrary generator id —
        // "not discharged" for every generator, unconditionally.
        for generator in [
            g_record_empty(),
            g_ctor_nullary(),
            g_match_catchall(),
            g_arith(),
            g_arith_split(),
            g_arith_input(),
        ] {
            assert!(
                !generator_is_tight(ClaimKind::Empty, &generator),
                "{generator:?} must not be tight for the empty claim kind"
            );
        }
    }

    #[test]
    fn test_match_opt_some_var_binding_proven() {
        let opt_ty = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        // match o { None => 0, Some(k) => k }
        let expr = Expr::Match(
            Box::new(Expr::Var("o".into())),
            vec![
                (Pattern::Ctor("None".into(), vec![]), Expr::Lit(0)),
                (
                    Pattern::Ctor("Some".into(), vec![Pattern::Var("k".into())]),
                    Expr::Var("k".into()),
                ),
            ],
        );
        let ctx = TyCtx::new().extend("o", opt_ty);
        let context = ContextId::root();

        let (ty, _tree, _st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer match opt");
        assert_eq!(ty, Ty::Con("Int"));

        let (aud, real_tree) =
            audited_type_check_tree(&expr, &ctx, context).expect("audited match opt");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(real_tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &real_tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_match_wildcard_catch_all_stays_audited() {
        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        // match b { True => 1, _ => 0 }
        let expr = Expr::Match(
            Box::new(Expr::Var("b".into())),
            vec![
                (Pattern::Ctor("True".into(), vec![]), Expr::Lit(1)),
                (Pattern::Wildcard, Expr::Lit(0)),
            ],
        );
        let ctx = TyCtx::new().extend("b", bool_ty);
        let context = ContextId::root();

        let (ty, _tree, _st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer match wildcard");
        assert_eq!(ty, Ty::Con("Int"));

        let (aud, real_tree) =
            audited_type_check_tree(&expr, &ctx, context).expect("audited match wildcard");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert_eq!(
            honest_result_outcome(Outcome::Proven, real_tree.tree()),
            Outcome::Audited,
            "a catch-all arm is not yet represented as explicit Case premises"
        );
        assert!(real_tree.tree().well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &real_tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_match_non_exhaustive() {
        let opt_ty = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        // match o { None => 0 } -> missing Some
        let expr = Expr::Match(
            Box::new(Expr::Var("o".into())),
            vec![(Pattern::Ctor("None".into(), vec![]), Expr::Lit(0))],
        );
        let ctx = TyCtx::new().extend("o", opt_ty);

        let res = infer_tree(&expr, &ctx, Infer::new());
        assert_eq!(
            res.unwrap_err(),
            TypeError::NonExhaustive(vec!["Some".into()])
        );
    }

    #[test]
    fn test_match_on_non_sum() {
        // match (1) { _ => 0 } -> 1 is Int, not a Sum
        let expr = Expr::Match(
            Box::new(Expr::Lit(1)),
            vec![(Pattern::Wildcard, Expr::Lit(0))],
        );
        let ctx = TyCtx::new();

        let res = infer_tree(&expr, &ctx, Infer::new());
        assert_eq!(res.unwrap_err(), TypeError::Mismatch);
    }

    #[test]
    fn test_nested_ctor_pattern_unsupported() {
        let opt_ty = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Match(
            Box::new(Expr::Var("o".into())),
            vec![
                (
                    Pattern::Ctor("Some".into(), vec![Pattern::Ctor("Some".into(), vec![])]),
                    Expr::Lit(0),
                ),
                (Pattern::Wildcard, Expr::Lit(0)),
            ],
        );
        let ctx = TyCtx::new().extend("o", opt_ty);

        let res = infer_tree(&expr, &ctx, Infer::new());
        assert_eq!(res.unwrap_err(), TypeError::Unsupported);
    }
}
