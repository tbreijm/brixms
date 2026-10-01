//! L3 v2 — the derivation-capable executable profile (ADR-0027).
//!
//! **Why a separate module and a separate profile.** ADR-0012 §10 forecloses
//! widening v1: "Adding source-level transition guards, parameters,
//! payload-bearing constructors, general regimes, effects, administrative
//! steps, or candidate-work limits requires a new ADR slice and a new
//! execution-profile marker. It cannot broaden this profile by accepting
//! formerly rejected forms under the same `brix.l3.rule-agenda-saturated@1`
//! identity." So v1's lowering in [`crate::l3`] is untouched, and every v1
//! identity keeps reproducing byte-for-byte.
//!
//! **What v2 adds, and the one that matters.** v1's rules are constants: a
//! rule body cannot reference another rule, so the agenda commits each rule
//! once and quiesces. ADR-0012 ⟨D-TAUZERO⟩ states the consequence outright —
//! "saturation over L3 v1 is the identity". v2 admits an **acyclic rule-fact
//! dependency** (ADR-0027 ⟨D-DERIVE⟩): a rule body may name an earlier rule,
//! meaning *read that rule's committed fact*, never *invoke that rule*. That
//! is what makes a derived fact reachable at all.
//!
//! Alongside it, v2 admits the expression forms v1 rejects by name — field
//! access, `match`, payload-bearing constructors, arithmetic and comparison —
//! so a rule body is an **expression** rather than a pre-normalized value.
//!
//! **Stage A scope.** This module is ADR-0027 §10 Stage A: the profile marker,
//! the expression IR, the plan shape, dependency extraction and its acyclicity
//! check. It deliberately contains **no evaluator** (Stage B) and **no
//! runtime** (Stage C): nothing here executes anything, so nothing here can
//! mint a certificate or move a grade.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use brix_syntax::ast;

/// A `config` as Stage A knows it: names and shapes, without resolved payload
/// types.
///
/// Deliberately *not* v1's `L3ConfigDecl`. Reusing it would force a type for
/// every field and variant parameter, and Stage A has not resolved any — the
/// evaluator does that in Stage B. Filling them with a placeholder would put
/// wrong data in the plan, which is worse than absent data: a reader cannot
/// tell a placeholder from a resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3ConfigDeclV2 {
    pub name: String,
    pub body: L3ConfigBodyV2,
}

/// A `config` body's shape at Stage A: which variants exist and how many
/// parameters each takes, or which fields a record declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3ConfigBodyV2 {
    /// `(variant name, parameter count)`, in declaration order.
    Sum(Vec<(String, usize)>),
    /// Field names, in declaration order.
    Record(Vec<String>),
}

/// The v2 execution-profile marker (ADR-0027 ⟨D-V2PROFILE⟩).
///
/// Distinct from [`crate::l3::L3_PROFILE_MARKER_V1`] by construction, which is
/// the whole point: a v2 plan can never be mistaken for a v1 plan, and a
/// verifier holding one profile's expectations refuses the other's artifact
/// rather than reinterpreting it.
pub const L3_PROFILE_MARKER_V2: &str = "brix.l3.rule-agenda-derived@2";

/// The v2 plan format version, distinct from v1's.
pub const L3_PLAN_FORMAT_V2: u64 = 2;

/// A v2 rule body: an **expression**, not a pre-normalized value.
///
/// v1's `L3ValueV1` is a closed value because a v1 rule body is a constant.
/// A v2 body computes, so it needs the forms v1 rejects — and one form v1 has
/// no analogue for at all, [`L3ExprV2::RuleFact`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3ExprV2 {
    Int(i64),
    Str(String),
    Bool(bool),
    /// A prior closed `let` binding, by name.
    LetRef(String),
    /// **The derivation form** (ADR-0027 ⟨D-DERIVE⟩): read the payload of an
    /// earlier rule's already-committed fact.
    ///
    /// Deliberately *not* a call. It names one fact; it does not invoke the
    /// rule, does not re-evaluate it, and cannot recurse. A guard that
    /// quantifies over many facts is the v3 shape and needs a grounding
    /// discipline this AST cannot express (ADR-0027 §7).
    RuleFact(String),
    /// A nullary variant of a declared sum.
    NullaryVariant {
        nominal_sum: String,
        variant: String,
    },
    /// A payload-bearing constructor application.
    Ctor {
        nominal_sum: String,
        variant: String,
        args: Vec<L3ExprV2>,
    },
    /// A record literal, in declaration order regardless of source order.
    Record {
        nominal_config: String,
        fields: Vec<(String, L3ExprV2)>,
    },
    /// Field projection on a record.
    Field(Box<L3ExprV2>, String),
    /// Checked same-domain numeric arithmetic. Infix division accepts F64
    /// and Decimal; integer division requires an explicit rounding builtin.
    Arith(ArithOpV2, Box<L3ExprV2>, Box<L3ExprV2>),
    /// Comparison, yielding a boolean.
    Cmp(CmpOpV2, Box<L3ExprV2>, Box<L3ExprV2>),
    /// Case analysis. Exhaustiveness is required (ADR-0027 §5); a default arm
    /// silently swallowing malformed input is rejected at lowering.
    Match {
        scrutinee: Box<L3ExprV2>,
        arms: Vec<(L3PatternV2, L3ExprV2)>,
    },
    /// A pure function invocation: `func(args...)` (ADR-0032).
    Call {
        func: String,
        args: Vec<L3ExprV2>,
    },
    /// Short-circuiting logical AND: `lhs && rhs`.
    And(Box<L3ExprV2>, Box<L3ExprV2>),
    /// Short-circuiting logical OR: `lhs || rhs`.
    Or(Box<L3ExprV2>, Box<L3ExprV2>),
    /// Logical NOT: `!expr`.
    Not(Box<L3ExprV2>),
    /// Exact signed integer division or modulo with a **named** rounding rule
    /// (ADR-0035): `div_floor`, `div_ceil`, `div_half_even`, `mod_euclid`.
    ///
    /// A distinct node rather than a [`Self::Call`] to a well-known name, so
    /// the operation is pinned structurally in the canonical encoding and
    /// cannot be reached, shadowed, or renamed through the helper table.
    IntDivMod(DivModOpV2, Box<L3ExprV2>, Box<L3ExprV2>),
    /// A deterministic fold over a bounded list (ADR-0037, ADR-0040):
    /// `sum`/`count`/`all`/`any`/`min`/`max`. `binder` is a hygienic lexical
    /// variable scoped to `body`; references to it lower to the ordinary
    /// [`Self::LetRef`] encoding.
    Fold {
        op: FoldOpV2,
        list: Box<L3ExprV2>,
        binder: String,
        body: Box<L3ExprV2>,
    },
    /// `filter(xs, x => cond)` (ADR-0040): keep elements in sequence order
    /// for which `cond` evaluates to `true`.
    Filter {
        list: Box<L3ExprV2>,
        binder: String,
        cond: Box<L3ExprV2>,
    },
    /// `map(xs, x => e)` (ADR-0040): the list of `e` evaluated for each
    /// element, in sequence order.
    Map {
        list: Box<L3ExprV2>,
        binder: String,
        body: Box<L3ExprV2>,
    },
    /// `for x in xs, y in ys where cond yield e` (ADR-0040): nested generator
    /// loops evaluated left to right, in generator order. A later generator
    /// and the `where` clause may reference an earlier binder.
    Comprehension {
        generators: Vec<(String, L3ExprV2)>,
        where_clause: Option<Box<L3ExprV2>>,
        yield_expr: Box<L3ExprV2>,
    },
    /// `[e1, e2, ...]` — a list literal (ADR-0040).
    ListLit(Vec<L3ExprV2>),
    /// `e in xs` — list membership by structural equality (ADR-0040).
    In(Box<L3ExprV2>, Box<L3ExprV2>),
    /// `len(xs)` (ADR-0040).
    Len(Box<L3ExprV2>),
    /// `distinct(xs)` (ADR-0040): first occurrence kept, order otherwise
    /// preserved.
    Distinct(Box<L3ExprV2>),
    /// Explicit numeric construction, conversion, rounding, or negation.
    NumericBuiltin(NumericBuiltinV2, Vec<L3ExprV2>),
}

/// Numeric operations pinned independently of user-defined helper names.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum NumericBuiltinV2 {
    F64,
    Decimal,
    F64FromInt,
    DecimalFromInt,
    DecimalDiv,
    F64Neg,
    DecimalNeg,
}

impl NumericBuiltinV2 {
    pub const ALL: [Self; 7] = [
        Self::F64,
        Self::Decimal,
        Self::F64FromInt,
        Self::DecimalFromInt,
        Self::DecimalDiv,
        Self::F64Neg,
        Self::DecimalNeg,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::F64 => "f64",
            Self::Decimal => "decimal",
            Self::F64FromInt => "f64_from_int",
            Self::DecimalFromInt => "decimal_from_int",
            Self::DecimalDiv => "decimal_div",
            Self::F64Neg => "f64_neg",
            Self::DecimalNeg => "decimal_neg",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }
    pub const fn arity(self) -> usize {
        match self {
            Self::DecimalDiv => 4,
            _ => 1,
        }
    }
    pub const fn ordinal(self) -> u64 {
        match self {
            Self::F64 => 0,
            Self::Decimal => 1,
            Self::F64FromInt => 2,
            Self::DecimalFromInt => 3,
            Self::DecimalDiv => 4,
            Self::F64Neg => 5,
            Self::DecimalNeg => 6,
        }
    }
}

/// Deterministic fold operations over a bounded list. `Sum`/`Count`/`All`/
/// `Any` are ADR-0037's first slice; `Min`/`Max` are ADR-0040's extension.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FoldOpV2 {
    Sum,
    Count,
    All,
    Any,
    Min,
    Max,
}

impl FoldOpV2 {
    /// Every operation, in canonical-ordinal order.
    pub const ALL: [FoldOpV2; 6] = [
        FoldOpV2::Sum,
        FoldOpV2::Count,
        FoldOpV2::All,
        FoldOpV2::Any,
        FoldOpV2::Min,
        FoldOpV2::Max,
    ];

    /// The source name that calls this fold. These names are reserved: a
    /// module may not declare a helper or constructor that shadows one.
    pub const fn name(self) -> &'static str {
        match self {
            FoldOpV2::Sum => "sum",
            FoldOpV2::Count => "count",
            FoldOpV2::All => "all",
            FoldOpV2::Any => "any",
            FoldOpV2::Min => "min",
            FoldOpV2::Max => "max",
        }
    }

    /// Resolve a source name to its fold operation, if it is one of the
    /// reserved names.
    pub fn from_name(name: &str) -> Option<FoldOpV2> {
        FoldOpV2::ALL.into_iter().find(|op| op.name() == name)
    }

    /// Canonical ordinal, appended and never renumbered: `sum`=0, `count`=1,
    /// `all`=2, `any`=3 (ADR-0037), `min`=4, `max`=5 (ADR-0040).
    pub(crate) const fn ordinal(self) -> u64 {
        match self {
            FoldOpV2::Sum => 0,
            FoldOpV2::Count => 1,
            FoldOpV2::All => 2,
            FoldOpV2::Any => 3,
            FoldOpV2::Min => 4,
            FoldOpV2::Max => 5,
        }
    }
}

/// Every reserved list/relational operation name (ADR-0037, ADR-0040): a
/// module may not declare a helper or constructor that shadows one, exactly
/// as ADR-0035 reserves `div_floor`/`div_ceil`/`div_half_even`/`mod_euclid`.
pub fn is_reserved_list_operation_name(name: &str) -> bool {
    FoldOpV2::from_name(name).is_some() || matches!(name, "filter" | "map" | "len" | "distinct")
}

/// Maximum length of a list produced by `filter`, `map`, a comprehension, a
/// list literal, or `distinct` (ADR-0040). A declared input's own `max` bound
/// (ADR-0037) is checked separately and may not exceed 256; this bound guards
/// every *derived* list a running program can build, including the result of
/// a large cartesian product, so evaluation fails closed with a typed fault
/// rather than growing memory without bound.
pub const MAX_DERIVED_LIST_LEN: usize = 4096;

/// Exact signed integer division and modulo operations (ADR-0035).
///
/// `/` itself stays refused. Every operation here names the rounding it
/// performs, because the rounding is the part a policy author has to get
/// right and the part a reader has to be able to check.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DivModOpV2 {
    /// Round the exact quotient toward negative infinity.
    DivFloor,
    /// Round the exact quotient toward positive infinity.
    DivCeil,
    /// Round the exact quotient to the nearest integer, ties to even.
    DivHalfEven,
    /// Euclidean remainder: the `r` in `a = b*q + r` with `0 <= r < |b|`.
    ModEuclid,
}

impl DivModOpV2 {
    /// Every operation, in canonical-ordinal order.
    pub const ALL: [DivModOpV2; 4] = [
        DivModOpV2::DivFloor,
        DivModOpV2::DivCeil,
        DivModOpV2::DivHalfEven,
        DivModOpV2::ModEuclid,
    ];

    /// The source name that calls this operation. These names are reserved:
    /// a module may not declare a helper or constructor that shadows one.
    pub const fn name(self) -> &'static str {
        match self {
            DivModOpV2::DivFloor => "div_floor",
            DivModOpV2::DivCeil => "div_ceil",
            DivModOpV2::DivHalfEven => "div_half_even",
            DivModOpV2::ModEuclid => "mod_euclid",
        }
    }

    /// Resolve a source name to its operation, if it is one of the reserved
    /// names.
    pub fn from_name(name: &str) -> Option<DivModOpV2> {
        DivModOpV2::ALL.into_iter().find(|op| op.name() == name)
    }

    /// Canonical ordinal, appended and never renumbered.
    pub(crate) const fn ordinal(self) -> u64 {
        match self {
            DivModOpV2::DivFloor => 0,
            DivModOpV2::DivCeil => 1,
            DivModOpV2::DivHalfEven => 2,
            DivModOpV2::ModEuclid => 3,
        }
    }
}

/// Why integer `/` is not executable (ADR-0035, ADR-0042, ADR-0045).
/// The research lane may still type Int/Int as historical Float; the
/// executable profile requires explicit integer rounding or F64/Decimal values.
pub(crate) fn division_not_admitted_reason() -> &'static str {
    "integer '/' is not executable: use div_floor, div_ceil, div_half_even, or \
     mod_euclid for an exact integer result. The let lane retains Int / Int -> Float \
     typing without executable Float values; decision '/' requires F64 or Decimal \
     operands (ADR-0035, ADR-0042, ADR-0045)"
}

/// Apply an exact integer division or modulo.
///
/// Computed in `i128` throughout, so no intermediate can overflow before the
/// single range check at the end: the operands are `i64`, and `|a| + |b|` and
/// `2*|r|` all fit in `i128` comfortably. That is what lets the result be
/// judged on its own rather than on whether a temporary happened to fit.
pub(crate) fn eval_div_mod(op: DivModOpV2, a: i64, b: i64) -> Result<i64, EvalFault> {
    if b == 0 {
        return Err(EvalFault::DivisionByZero(op));
    }
    let (a, b) = (i128::from(a), i128::from(b));
    // Truncating quotient and remainder: `r` carries the sign of `a`.
    let q = a / b;
    let r = a % b;

    let out: i128 = match op {
        // `mod_euclid` is defined by `a = b*q + r` with `0 <= r < |b|`, and is
        // deliberately **not** paired with any one quotient here: for negative
        // `b` the Euclidean quotient is not the floor one. Normalising `r`
        // directly keeps the identity true for every sign combination.
        DivModOpV2::ModEuclid => {
            if r < 0 {
                r + b.abs()
            } else {
                r
            }
        }
        DivModOpV2::DivFloor => {
            if r != 0 && ((a < 0) != (b < 0)) {
                q - 1
            } else {
                q
            }
        }
        DivModOpV2::DivCeil => {
            if r != 0 && ((a < 0) == (b < 0)) {
                q + 1
            } else {
                q
            }
        }
        DivModOpV2::DivHalfEven => {
            if r == 0 {
                q
            } else {
                // Away-from-zero neighbour of the truncated quotient.
                let step = if (a < 0) != (b < 0) { -1 } else { 1 };
                let twice = 2 * r.abs();
                let mag = b.abs();
                if twice > mag {
                    q + step
                } else if twice < mag {
                    q
                } else if q % 2 == 0 {
                    // Exactly halfway: take whichever neighbour is even.
                    q
                } else {
                    q + step
                }
            }
        }
    };

    i64::try_from(out).map_err(|_| EvalFault::DivisionOverflow(op))
}

/// Type category of an [`L3ValueV2`] for contract uniformity checking.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum L3ValueType {
    Int,
    Str,
    Bool,
    Sum(String),
    Record(String),
    /// A bounded list (ADR-0037, ADR-0040). Deliberately flat: unlike `Sum`/
    /// `Record`, this category carries no element type. Two lists are always
    /// "the same type" for the shallow contract checks that use this type
    /// (proposal value uniformity, declared input types); the element type
    /// and length are enforced separately, by the declared input's element
    /// schema and `max` (ADR-0037) and by each operation's own runtime
    /// [`EvalFault::OperandShape`] check. This sidesteps a genuine ambiguity
    /// a payload-carrying variant would have to resolve for its own sake: an
    /// empty runtime list has no element it could infer a type from.
    List,
    F64,
    Decimal,
}

/// A recursively checked schema type used by structured inputs and helper
/// contracts. `Named` refers to a closed config definition in the plan schema
/// table; builtin leaves retain their scalar identity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum L3SchemaType {
    Int,
    Bool,
    Str,
    Named(String),
    F64,
    Decimal,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum L3SchemaBody {
    Sum(Vec<(String, Vec<L3SchemaType>)>),
    Record(BTreeMap<String, L3SchemaType>),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct L3Schema {
    pub name: String,
    pub body: L3SchemaBody,
}

impl L3SchemaType {
    pub fn display_name(&self) -> String {
        match self {
            Self::Int => "Int".to_string(),
            Self::Bool => "Bool".to_string(),
            Self::Str => "Str".to_string(),
            Self::Named(name) => name.clone(),
            Self::F64 => "F64".to_string(),
            Self::Decimal => "Decimal".to_string(),
        }
    }
}

/// Validate a runtime value against a closed schema, retaining a precise
/// nested path for contract and external-input diagnostics.
pub fn validate_l3_value(
    value: &L3ValueV2,
    expected: &L3SchemaType,
    schemas: &BTreeMap<String, L3Schema>,
    path: &str,
) -> Result<(), String> {
    match (value, expected) {
        (L3ValueV2::Int(_), L3SchemaType::Int)
        | (L3ValueV2::Bool(_), L3SchemaType::Bool)
        | (L3ValueV2::Str(_), L3SchemaType::Str)
        | (L3ValueV2::F64(_), L3SchemaType::F64)
        | (L3ValueV2::Decimal(_), L3SchemaType::Decimal) => Ok(()),
        (L3ValueV2::Int(_), _)
        | (L3ValueV2::Bool(_), _)
        | (L3ValueV2::Str(_), _)
        | (L3ValueV2::F64(_), _)
        | (L3ValueV2::Decimal(_), _) => Err(format!(
            "{path}: expected {}, found primitive",
            expected.display_name()
        )),
        (
            L3ValueV2::Ctor {
                nominal_sum,
                variant,
                args,
            },
            L3SchemaType::Named(name),
        ) => {
            let Some(schema) = schemas.get(name) else {
                return Err(format!("{path}: unknown schema '{name}'"));
            };
            let L3SchemaBody::Sum(variants) = &schema.body else {
                return Err(format!("{path}: expected record '{name}', found sum"));
            };
            if nominal_sum != name {
                return Err(format!(
                    "{path}: expected nominal '{name}', found '{nominal_sum}'"
                ));
            }
            let Some((_, payloads)) = variants.iter().find(|(candidate, _)| candidate == variant)
            else {
                return Err(format!("{path}: unknown variant '{variant}' of '{name}'"));
            };
            if payloads.len() != args.len() {
                return Err(format!(
                    "{path}: variant '{variant}' expects {} payload(s), found {}",
                    payloads.len(),
                    args.len()
                ));
            }
            for (index, (arg, arg_ty)) in args.iter().zip(payloads).enumerate() {
                validate_l3_value(arg, arg_ty, schemas, &format!("{path}.args[{index}]"))?;
            }
            Ok(())
        }
        (
            L3ValueV2::Record {
                nominal_config,
                fields,
            },
            L3SchemaType::Named(name),
        ) => {
            let Some(schema) = schemas.get(name) else {
                return Err(format!("{path}: unknown schema '{name}'"));
            };
            let L3SchemaBody::Record(expected_fields) = &schema.body else {
                return Err(format!("{path}: expected sum '{name}', found record"));
            };
            if nominal_config != name {
                return Err(format!(
                    "{path}: expected nominal '{name}', found '{nominal_config}'"
                ));
            }
            if fields.len() != expected_fields.len() {
                return Err(format!(
                    "{path}: record '{name}' expects {} field(s), found {}",
                    expected_fields.len(),
                    fields.len()
                ));
            }
            for (field, field_ty) in expected_fields {
                let Some(actual) = fields
                    .iter()
                    .find(|(candidate, _)| candidate == field)
                    .map(|(_, value)| value)
                else {
                    return Err(format!("{path}.{field}: missing field"));
                };
                validate_l3_value(actual, field_ty, schemas, &format!("{path}.{field}"))?;
            }
            for (field, _) in fields {
                if !expected_fields.contains_key(field) {
                    return Err(format!("{path}.{field}: unknown field"));
                }
            }
            Ok(())
        }
        (L3ValueV2::Ctor { nominal_sum, .. }, expected)
        | (
            L3ValueV2::Record {
                nominal_config: nominal_sum,
                ..
            },
            expected,
        ) => Err(format!(
            "{path}: expected {}, found '{nominal_sum}'",
            expected.display_name()
        )),
        // Lists are never nested inside a record/sum schema or a helper
        // contract (ADR-0037 §Scope defers nested lists); reaching here means
        // a list value was passed where a scalar or nominal schema type was
        // expected.
        (L3ValueV2::List(_), expected) => Err(format!(
            "{path}: expected {}, found list",
            expected.display_name()
        )),
    }
}

impl fmt::Display for L3ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => write!(f, "Int"),
            Self::Str => write!(f, "Str"),
            Self::Bool => write!(f, "Bool"),
            Self::Sum(s) => write!(f, "Sum({s})"),
            Self::Record(r) => write!(f, "Record({r})"),
            Self::List => write!(f, "List"),
            Self::F64 => write!(f, "F64"),
            Self::Decimal => write!(f, "Decimal"),
        }
    }
}

/// Compute the nominal/primitive type category of an evaluated value.
pub fn type_of_value(v: &L3ValueV2) -> L3ValueType {
    match v {
        L3ValueV2::Int(_) => L3ValueType::Int,
        L3ValueV2::Str(_) => L3ValueType::Str,
        L3ValueV2::Bool(_) => L3ValueType::Bool,
        L3ValueV2::Ctor { nominal_sum, .. } => L3ValueType::Sum(nominal_sum.clone()),
        L3ValueV2::Record { nominal_config, .. } => L3ValueType::Record(nominal_config.clone()),
        L3ValueV2::List(_) => L3ValueType::List,
        L3ValueV2::F64(_) => L3ValueType::F64,
        L3ValueV2::Decimal(_) => L3ValueType::Decimal,
    }
}

/// Evaluation bounds (ADR-0032, raised and joined by an evaluation-thread
/// stack guarantee under ADR-0042 to admit direct and mutual helper
/// recursion).
///
/// `MAX_CALL_DEPTH` is the number of simultaneously outstanding (uncompleted)
/// [`L3ExprV2::Call`] invocations — the bound a recursive helper actually
/// hits, e.g. `length` over a list of more than this many elements. It is
/// the primary, user-meaningful recursion bound.
pub const MAX_CALL_DEPTH: usize = 1_000;
/// The bound on total `eval_internal` nesting, counting every expression
/// form (`match`, arithmetic, field projection, …), not only calls. Each
/// logical call level costs several of these (evaluating the scrutinee, the
/// chosen arm, the call's own arguments, then the callee's body), so this is
/// set well above `MAX_CALL_DEPTH` — it is a backstop against unbounded
/// native-stack use from *any* expression shape, not the bound a program is
/// expected to reach through calls alone.
pub const MAX_EVAL_RECURSION_DEPTH: usize = 20_000;
/// Total evaluation steps per evaluation (ADR-0032's `tick_step`): the bound
/// on work. Sized so a helper recursing near `MAX_CALL_DEPTH` can complete,
/// and so a full join of two maximum-size list inputs (256 x 256 pairs, a
/// few steps each) fits, while a three-way join of them does not.
pub const MAX_CALL_STEPS: usize = 2_000_000;

/// Total evaluation steps across one deliberation run, shared by every
/// evaluation in it (see [`EvalEnv::with_run_work`]). A per-evaluation bound
/// alone would let a `decide` block multiply it by its instance count; this
/// keeps the work a whole run may do bounded no matter how it is split.
pub const MAX_RUN_STEPS: usize = 50_000_000;

/// Steps an evaluation accumulates locally before adding them to the shared
/// run counter, so the shared atomic is touched rarely.
const RUN_WORK_FLUSH_STEPS: usize = 4096;
pub const MAX_VALUE_DEPTH: usize = 128;
pub const MAX_EVAL_VALUE_NODES: usize = 10_000;
pub const MAX_EVAL_VALUE_BYTES: usize = 1_000_000;

/// The stack size a bounded evaluation runs on (ADR-0042), independent of the
/// calling thread's own stack.
///
/// **Why a dedicated thread at all.** `MAX_CALL_DEPTH`/`MAX_EVAL_RECURSION_DEPTH`
/// above bound the *logical* nesting `eval_internal` is willing to reach, but
/// each logical level costs several *native* stack frames. "Never a stack
/// overflow" (ADR-0042 — a resource fault must fail closed to `Unknown`, never
/// crash the process) cannot depend on how much stack the caller's thread
/// happens to have: the CLI runs on the process main thread (no custom stack
/// size), a test harness thread's default varies by platform, and an embedder
/// is free to call in from anywhere. Rather than tune the depth bounds down to
/// whatever the smallest thread anyone might call from could survive — which
/// would make ordinary, obviously-terminating recursion (e.g. `length` over a
/// few hundred elements) fail for a reason with nothing to do with the
/// program — a bounded evaluation always runs on a freshly spawned thread with
/// this fixed, generous stack. The budget above, not the caller's thread, is
/// then what decides whether a program is admitted.
///
/// The stack is reserved, not committed: only the pages a deep recursion
/// actually touches are backed by memory. To avoid paying a thread spawn per
/// expression, a caller that evaluates many expressions (a whole
/// deliberation run) enters the evaluation thread once through
/// [`with_eval_stack`]; `eval` calls inside it run inline.
const EVAL_THREAD_STACK_BYTES: usize = 256 * 1024 * 1024;

thread_local! {
    /// Whether this thread is an evaluation thread with the
    /// [`EVAL_THREAD_STACK_BYTES`] stack.
    static ON_EVAL_STACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` on an evaluation thread with the [`EVAL_THREAD_STACK_BYTES`]
/// stack, so every [`eval`] inside it runs inline rather than spawning its
/// own thread. Runs `f` directly when already on one.
///
/// Returns `None` if the operating system refuses to create the thread, or
/// `f` panics; callers report either as a resource fault.
pub fn with_eval_stack<R: Send>(f: impl FnOnce() -> R + Send) -> Option<R> {
    if ON_EVAL_STACK.with(|on| on.get()) {
        return Some(f());
    }
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(EVAL_THREAD_STACK_BYTES)
            .spawn_scoped(scope, move || {
                ON_EVAL_STACK.with(|on| on.set(true));
                f()
            })
            .ok()?
            .join()
            .ok()
    })
}

/// Normalized pure function definition in L3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3FunctionDef {
    pub name: String,
    pub params: Vec<(String, Option<L3SchemaType>)>,
    pub ret_contract: Option<L3SchemaType>,
    pub schemas: Arc<BTreeMap<String, L3Schema>>,
    pub body: L3ExprV2,
}

/// The arithmetic operators v2 admits. **No division** — see [`L3ExprV2`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArithOpV2 {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOpV2 {
    /// Frozen ordinal — ABI.
    pub const fn ordinal(self) -> u64 {
        match self {
            ArithOpV2::Add => 0,
            ArithOpV2::Sub => 1,
            ArithOpV2::Mul => 2,
            ArithOpV2::Div => 3,
        }
    }
}

/// The comparison operators v2 admits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CmpOpV2 {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl CmpOpV2 {
    /// Frozen ordinal — ABI.
    pub const fn ordinal(self) -> u64 {
        match self {
            CmpOpV2::Lt => 0,
            CmpOpV2::Le => 1,
            CmpOpV2::Gt => 2,
            CmpOpV2::Ge => 3,
            CmpOpV2::Eq => 4,
            CmpOpV2::Ne => 5,
        }
    }
}

/// A v2 match pattern. Constructor patterns bind their arguments positionally;
/// nested constructor patterns are not admitted in Stage A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3PatternV2 {
    /// `Variant(x, y)` — sub-patterns are binders or wildcards only.
    Ctor {
        variant: String,
        binders: Vec<Option<String>>,
    },
}

/// One item of a lowered v2 plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3PlanItemV2 {
    Config(L3ConfigDeclV2),
    Let {
        name: String,
        value: L3ExprV2,
    },
    Rule {
        name: String,
        body: L3ExprV2,
        /// The rules this rule reads facts from, in first-mention order.
        ///
        /// Extracted statically and stored in the plan so eligibility is a
        /// plan property rather than a runtime discovery — a rule is a
        /// candidate only once every dependency has committed.
        depends_on: Vec<String>,
    },
}

/// A lowered v2 plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3PlanV2 {
    pub profile: String,
    pub format: u64,
    pub items: Vec<L3PlanItemV2>,
}

/// Every distinguishable way a module can fall outside the v2 fragment.
///
/// One variant per reason, following v1's discipline: no silent omission, and
/// a test can assert the exact failure rather than "it did not lower".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3V2LowerError {
    ProfileMismatch {
        expected: String,
        found: String,
    },
    /// A top-level item outside `{config, let, rule}`.
    ItemNotAllowed(String),
    /// A rule parameter that does not name an earlier rule.
    ///
    /// A v2 parameter is a **declared dependency**, not a schema variable: it
    /// names exactly one earlier rule and binds that rule's committed fact. So
    /// there is no quantification domain to supply and none of ADR-0027 §7's
    /// grounding problem arises — that objection is about parameters ranging
    /// over *values*, which v2 still does not have.
    UndeclaredDependency {
        rule: String,
        param: String,
    },
    /// A rule body reads a fact the signature does not declare.
    ///
    /// The body may read only its declared parameters, which is what keeps the
    /// plan's dependency list from drifting from what the body actually does.
    UndeclaredFactRead {
        rule: String,
        fact: String,
    },
    /// A rule reads a fact from a rule declared later, or from itself.
    ///
    /// v2's dependency graph is acyclic and order-respecting by construction:
    /// a fact must exist before it can be read.
    ForwardOrSelfDependency {
        rule: String,
        depends_on: String,
    },
    /// A reference resolving to nothing — neither a prior `let`, a prior rule,
    /// nor a nullary constructor.
    ///
    /// **There is deliberately no `DependencyCycle` variant.** Acyclicity is a
    /// property of construction rather than of a later graph walk: only rules
    /// already lowered are in scope, so a cycle cannot be built and a forward
    /// or self reference surfaces here instead. A variant that can never be
    /// constructed is a latent liability, not a safety net (#254).
    ///
    UnresolvedReference(String),
    /// A `match` that does not cover every variant of its scrutinee's sum.
    ///
    /// Declared in Stage B, where `check_exhaustive` can actually fire it —
    /// Stage A deliberately left it out rather than claim a check that did not
    /// run.
    NonExhaustiveMatch {
        sum: String,
        missing: Vec<String>,
    },
    /// A wildcard or binder arm at the top level of a `match`. Rejected so a
    /// default arm cannot silently swallow malformed input.
    DefaultArmNotAllowed,
    /// A nested constructor pattern. Not in Stage A.
    NestedPatternNotAllowed,
    /// `/` (ADR-0042): the operator has an exact meaning — `Int / Int → Float`
    /// in the type-realization (`let`) lane — but `Float` values are not
    /// admitted in a finite-decision program, so the operator is refused here
    /// by name rather than silently rounded or coerced.
    DivisionNotAllowed,
    /// A float literal. v2 inherits v1's exclusion.
    FloatLiteralNotAllowed(String),
    /// An integer literal outside `i64`.
    IntegerOverflow(String),
    /// A surface form with no v2 lowering (`prove`, `why`, `audit`, `then`,
    /// `and`).
    Unsupported(String),
    /// A function was invoked with the wrong number of arguments.
    FunctionArityMismatch {
        func: String,
        expected: usize,
        found: usize,
    },
    /// A pattern contains duplicate match variable binders.
    DuplicateMatchBinder(String),
    /// A `sum`/`count`/`all`/`any`/`min`/`max`/`filter`/`map` call site did
    /// not have the required shape `form(list, x => expr)` (ADR-0037,
    /// ADR-0040): wrong arity is reported as [`Self::FunctionArityMismatch`]
    /// instead, so this is specifically a missing or malformed binder.
    ListFormShape {
        form: &'static str,
        detail: &'static str,
    },
    /// A lambda (`x => expr`) reached lowering outside the one position that
    /// admits it: the second argument of a recognized fold/filter/map call
    /// (ADR-0037, ADR-0040). Lambdas are not first-class values.
    LambdaNotAllowed,
}

impl fmt::Display for L3V2LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileMismatch { expected, found } => {
                write!(
                    f,
                    "profile mismatch: expected '{expected}', found '{found}'"
                )
            }
            Self::ItemNotAllowed(item) => write!(f, "item not allowed in v2: {item}"),
            Self::UndeclaredDependency { rule, param } => {
                write!(f, "rule '{rule}' has undeclared dependency '{param}'")
            }
            Self::UndeclaredFactRead { rule, fact } => {
                write!(f, "rule '{rule}' reads undeclared fact '{fact}'")
            }
            Self::ForwardOrSelfDependency { rule, depends_on } => {
                write!(
                    f,
                    "rule '{rule}' has forward or self dependency on '{depends_on}'"
                )
            }
            Self::UnresolvedReference(name) => write!(f, "unresolved reference: {name}"),
            Self::NonExhaustiveMatch { sum, missing } => {
                write!(f, "non-exhaustive match on '{sum}', missing: {missing:?}")
            }
            Self::DefaultArmNotAllowed => write!(f, "default arm not allowed in match"),
            Self::NestedPatternNotAllowed => write!(f, "nested pattern not allowed in match"),
            Self::DivisionNotAllowed => write!(f, "{}", division_not_admitted_reason()),
            Self::FloatLiteralNotAllowed(lit) => write!(f, "float literal not allowed: {lit}"),
            Self::IntegerOverflow(lit) => write!(f, "integer literal overflow: {lit}"),
            Self::Unsupported(feature) => write!(f, "unsupported feature: {feature}"),
            Self::FunctionArityMismatch {
                func,
                expected,
                found,
            } => {
                write!(
                    f,
                    "function '{func}' expected {expected} argument(s), found {found}"
                )
            }
            Self::DuplicateMatchBinder(name) => {
                write!(f, "duplicate match pattern binder: '{name}'")
            }
            Self::ListFormShape { form, detail } => {
                write!(
                    f,
                    "'{form}' requires the form '{form}(list, x => expr)': {detail}"
                )
            }
            Self::LambdaNotAllowed => write!(
                f,
                "a lambda 'x => expr' is fold/filter/map syntax only and cannot appear here"
            ),
        }
    }
}

impl std::error::Error for L3V2LowerError {}

/// Lower a module into a v2 plan, or reject it with the reason it falls
/// outside the fragment.
///
/// Deliberately total and side-effect free: this builds a plan, it does not
/// run one. Nothing here can mint a certificate.
pub fn lower_l3_plan_v2(module: &ast::Module, profile: &str) -> Result<L3PlanV2, L3V2LowerError> {
    if profile != L3_PROFILE_MARKER_V2 {
        return Err(L3V2LowerError::ProfileMismatch {
            expected: L3_PROFILE_MARKER_V2.to_string(),
            found: profile.to_string(),
        });
    }

    // Pass 0 — item admissibility. Every rejected item is named, never
    // dropped, which is the discipline v1 established in its §6.1.
    for item in &module.items {
        match item {
            ast::Item::Config(_) | ast::Item::Let(_) | ast::Item::Rule(_) => {}
            ast::Item::Fn(c) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("fn {}", c.name)))
            }
            ast::Item::Regime(r) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("regime {}", r.name)))
            }
            ast::Item::Show(_) => return Err(L3V2LowerError::ItemNotAllowed("show".to_string())),
            // Imports are resolved before lowering; one arriving here means
            // resolution was skipped.
            ast::Item::Use(path) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("use {path}")))
            }
            ast::Item::Witness { name, .. } => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("witness {name}")))
            }
            ast::Item::Propose(p) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!(
                    "propose {}",
                    p.name
                )))
            }
            ast::Item::Commit(c) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("commit {}", c.name)))
            }
            ast::Item::Input(i) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("input {}", i.name)))
            }
            ast::Item::Decide(d) => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("decide {}", d.name)))
            }
        }
    }

    // Names visible to a body, accumulated in declaration order so a forward
    // reference is a *rejection* rather than a silent resolution.
    let mut lets: BTreeSet<String> = BTreeSet::new();
    let mut rules_so_far: BTreeSet<String> = BTreeSet::new();
    // Every declared variant, for constructor application; and the nullary
    // subset, for a bare reference. A payload-bearing constructor is not a
    // value on its own, so the two are not interchangeable.
    let mut variants_of: BTreeMap<String, String> = BTreeMap::new();
    let mut nullary: BTreeMap<String, String> = BTreeMap::new();
    let mut items = Vec::new();

    for item in &module.items {
        match item {
            ast::Item::Config(c) => {
                if let ast::ConfigBody::Sum(variants) = &c.body {
                    for v in variants {
                        if NumericBuiltinV2::from_name(&v.name).is_some() {
                            return Err(L3V2LowerError::Unsupported(format!(
                                "constructor '{}' shadows a reserved numeric builtin",
                                v.name
                            )));
                        }
                        variants_of.insert(v.name.clone(), c.name.clone());
                        if v.params.is_empty() {
                            nullary.insert(v.name.clone(), c.name.clone());
                        }
                    }
                }
                // Carry the declaration's real shape. An empty stub here
                // would make the plan silently disagree with the module it
                // claims to be a lowering of — and Stage B's exhaustiveness
                // check reads exactly this.
                //
                // Payload TYPES are not resolved in Stage A (that is the
                // evaluator's business), so variant arities are carried and
                // their element types are left to Stage B rather than being
                // guessed here.
                let body = match &c.body {
                    ast::ConfigBody::Sum(variants) => L3ConfigBodyV2::Sum(
                        variants
                            .iter()
                            .map(|v| (v.name.clone(), v.params.len()))
                            .collect(),
                    ),
                    ast::ConfigBody::Record(fields) => {
                        L3ConfigBodyV2::Record(fields.iter().map(|f| f.name.clone()).collect())
                    }
                };
                items.push(L3PlanItemV2::Config(L3ConfigDeclV2 {
                    name: c.name.clone(),
                    body,
                }));
            }
            ast::Item::Let(l) => {
                let value = lower_expr_v2(
                    &l.value,
                    &lets,
                    &BTreeSet::new(),
                    &rules_so_far,
                    &nullary,
                    &variants_of,
                    &BTreeMap::new(),
                    false,
                    false,
                )?;
                lets.insert(l.name.clone());
                items.push(L3PlanItemV2::Let {
                    name: l.name.clone(),
                    value,
                });
            }
            ast::Item::Rule(r) => {
                // The parameter list IS the dependency list. Each parameter
                // names an earlier rule and binds that rule's committed fact,
                // so `depends_on` is DECLARED rather than extracted from the
                // body — the plan cannot disagree with what the body reads.
                //
                // A parameter here is not a schema variable: it names exactly
                // one rule and binds exactly one fact, so there is no
                // quantification domain to supply. ADR-0027 §7's grounding
                // problem is about parameters ranging over *values*, which v2
                // still does not have.
                let mut depends_on: Vec<String> = Vec::new();
                for param in &r.params {
                    if param.name == r.name {
                        return Err(L3V2LowerError::ForwardOrSelfDependency {
                            rule: r.name.clone(),
                            depends_on: param.name.clone(),
                        });
                    }
                    if !rules_so_far.contains(&param.name) {
                        return Err(L3V2LowerError::UndeclaredDependency {
                            rule: r.name.clone(),
                            param: param.name.clone(),
                        });
                    }
                    if !depends_on.contains(&param.name) {
                        depends_on.push(param.name.clone());
                    }
                }
                // Only the declared dependencies are readable in the body.
                let readable: BTreeSet<String> = depends_on.iter().cloned().collect();
                let body = lower_expr_v2(
                    &r.body,
                    &lets,
                    &BTreeSet::new(),
                    &readable,
                    &nullary,
                    &variants_of,
                    &BTreeMap::new(),
                    true,
                    false,
                )
                .map_err(|e| match e {
                    // A bare reference to a rule that exists but was not
                    // declared as a parameter: named as the undeclared
                    // read it is, rather than as an unresolved name.
                    L3V2LowerError::UnresolvedReference(n) if rules_so_far.contains(&n) => {
                        L3V2LowerError::UndeclaredFactRead {
                            rule: r.name.clone(),
                            fact: n,
                        }
                    }
                    other => other,
                })?;
                rules_so_far.insert(r.name.clone());
                items.push(L3PlanItemV2::Rule {
                    name: r.name.clone(),
                    body,
                    depends_on,
                });
            }
            _ => unreachable!("pass 0 admitted only config/let/rule"),
        }
    }

    Ok(L3PlanV2 {
        profile: profile.to_string(),
        format: L3_PLAN_FORMAT_V2,
        items,
    })
}

/// Validate and destructure a `form(list_expr, binder => body_expr)` call
/// site shared by `sum`/`count`/`all`/`any`/`min`/`max`/`filter`/`map`
/// (ADR-0037, ADR-0040). `form` is the reserved name that matched, supplied
/// by the caller so the diagnostic names it even though the callee is a
/// runtime `&String`.
fn expect_list_form<'a>(
    form: &'static str,
    args: &'a [ast::Expr],
) -> Result<(&'a ast::Expr, &'a str, &'a ast::Expr), L3V2LowerError> {
    if args.len() != 2 {
        return Err(L3V2LowerError::FunctionArityMismatch {
            func: form.to_string(),
            expected: 2,
            found: args.len(),
        });
    }
    match &args[1] {
        ast::Expr::Lambda { param, body } => Ok((&args[0], param.as_str(), body.as_ref())),
        _ => Err(L3V2LowerError::ListFormShape {
            form,
            detail: "expected a binder of the form `x => expr` as the second argument",
        }),
    }
}

/// Lower one surface expression into the v2 IR.
///
/// `in_rule` distinguishes a rule body — which may read earlier rules' facts —
/// from a `let` value, which may not: a `let` is a closed static binding, and
/// letting it depend on a committed fact would make plan construction depend
/// on run order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_expr_v2(
    e: &ast::Expr,
    lets: &BTreeSet<String>,
    locals: &BTreeSet<String>,
    rules: &BTreeSet<String>,
    nullary: &BTreeMap<String, String>,
    variants_of: &BTreeMap<String, String>,
    functions: &BTreeMap<String, usize>,
    in_rule: bool,
    helper_enabled: bool,
) -> Result<L3ExprV2, L3V2LowerError> {
    match e {
        ast::Expr::Num(s) => {
            if let Ok(n) = s.parse::<i64>() {
                Ok(L3ExprV2::Int(n))
            } else if s.parse::<f64>().is_ok() {
                Err(L3V2LowerError::FloatLiteralNotAllowed(s.clone()))
            } else {
                Err(L3V2LowerError::IntegerOverflow(s.clone()))
            }
        }
        ast::Expr::Str(s) => Ok(L3ExprV2::Str(s.clone())),
        ast::Expr::Bool(b) => Ok(L3ExprV2::Bool(*b)),
        ast::Expr::Var(name) => {
            if helper_enabled {
                // In helper-enabled lowering, parameters and match binders take lexical precedence.
                if locals.contains(name) {
                    return Ok(L3ExprV2::LetRef(name.clone()));
                }
                if let Some(sum) = nullary.get(name) {
                    return Ok(L3ExprV2::NullaryVariant {
                        nominal_sum: sum.clone(),
                        variant: name.clone(),
                    });
                }
                if lets.contains(name) {
                    return Ok(L3ExprV2::LetRef(name.clone()));
                }
            } else {
                // Function-free profiles retain prior precedence: nullary constructors resolve first.
                if let Some(sum) = nullary.get(name) {
                    return Ok(L3ExprV2::NullaryVariant {
                        nominal_sum: sum.clone(),
                        variant: name.clone(),
                    });
                }
                if locals.contains(name) || lets.contains(name) {
                    return Ok(L3ExprV2::LetRef(name.clone()));
                }
            }
            // The derivation form. Only a rule body may read a fact, and only
            // from a rule declared before it — so the dependency graph is
            // acyclic and order-respecting by construction rather than by a
            // later check.
            if rules.contains(name) {
                if !in_rule {
                    return Err(L3V2LowerError::UnresolvedReference(name.clone()));
                }
                return Ok(L3ExprV2::RuleFact(name.clone()));
            }
            Err(L3V2LowerError::UnresolvedReference(name.clone()))
        }
        ast::Expr::Field(base, field) => Ok(L3ExprV2::Field(
            Box::new(lower_expr_v2(
                base,
                lets,
                locals,
                rules,
                nullary,
                variants_of,
                functions,
                in_rule,
                helper_enabled,
            )?),
            field.clone(),
        )),
        ast::Expr::Record { config, fields } => {
            let mut out = Vec::new();
            for (name, value) in fields {
                out.push((
                    name.clone(),
                    lower_expr_v2(
                        value,
                        lets,
                        locals,
                        rules,
                        nullary,
                        variants_of,
                        functions,
                        in_rule,
                        helper_enabled,
                    )?,
                ));
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(L3ExprV2::Record {
                nominal_config: config.clone(),
                fields: out,
            })
        }
        ast::Expr::Call { func, args } => {
            if let Some(op) = NumericBuiltinV2::from_name(func) {
                if args.len() != op.arity() {
                    return Err(L3V2LowerError::FunctionArityMismatch {
                        func: func.clone(),
                        expected: op.arity(),
                        found: args.len(),
                    });
                }
                let args = args
                    .iter()
                    .map(|a| {
                        lower_expr_v2(
                            a,
                            lets,
                            locals,
                            rules,
                            nullary,
                            variants_of,
                            functions,
                            in_rule,
                            helper_enabled,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(L3ExprV2::NumericBuiltin(op, args));
            }

            // Reserved names resolve first and unconditionally. Nothing can
            // shadow them: a helper or constructor claiming one of these names
            // is refused where it is declared, so this branch is never a
            // silent override of something the author wrote.
            if let Some(op) = FoldOpV2::from_name(func) {
                let (list_expr, binder, body_expr) = expect_list_form(op.name(), args)?;
                let list = Box::new(lower_expr_v2(
                    list_expr,
                    lets,
                    locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
                let mut body_locals = locals.clone();
                body_locals.insert(binder.to_string());
                let body = Box::new(lower_expr_v2(
                    body_expr,
                    lets,
                    &body_locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
                return Ok(L3ExprV2::Fold {
                    op,
                    list,
                    binder: binder.to_string(),
                    body,
                });
            }
            if func == "filter" {
                let (list_expr, binder, cond_expr) = expect_list_form("filter", args)?;
                let list = Box::new(lower_expr_v2(
                    list_expr,
                    lets,
                    locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
                let mut body_locals = locals.clone();
                body_locals.insert(binder.to_string());
                let cond = Box::new(lower_expr_v2(
                    cond_expr,
                    lets,
                    &body_locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
                return Ok(L3ExprV2::Filter {
                    list,
                    binder: binder.to_string(),
                    cond,
                });
            }
            if func == "map" {
                let (list_expr, binder, body_expr) = expect_list_form("map", args)?;
                let list = Box::new(lower_expr_v2(
                    list_expr,
                    lets,
                    locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
                let mut body_locals = locals.clone();
                body_locals.insert(binder.to_string());
                let body = Box::new(lower_expr_v2(
                    body_expr,
                    lets,
                    &body_locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
                return Ok(L3ExprV2::Map {
                    list,
                    binder: binder.to_string(),
                    body,
                });
            }
            if func == "len" {
                if args.len() != 1 {
                    return Err(L3V2LowerError::FunctionArityMismatch {
                        func: func.clone(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                let inner = lower_expr_v2(
                    &args[0],
                    lets,
                    locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?;
                return Ok(L3ExprV2::Len(Box::new(inner)));
            }
            if func == "distinct" {
                if args.len() != 1 {
                    return Err(L3V2LowerError::FunctionArityMismatch {
                        func: func.clone(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                let inner = lower_expr_v2(
                    &args[0],
                    lets,
                    locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?;
                return Ok(L3ExprV2::Distinct(Box::new(inner)));
            }
            if let Some(op) = DivModOpV2::from_name(func) {
                if args.len() != 2 {
                    return Err(L3V2LowerError::FunctionArityMismatch {
                        func: func.clone(),
                        expected: 2,
                        found: args.len(),
                    });
                }
                let mut out = Vec::new();
                for a in args {
                    out.push(lower_expr_v2(
                        a,
                        lets,
                        locals,
                        rules,
                        nullary,
                        variants_of,
                        functions,
                        in_rule,
                        helper_enabled,
                    )?);
                }
                let mut it = out.into_iter();
                let lhs = it.next().expect("arity checked above");
                let rhs = it.next().expect("arity checked above");
                return Ok(L3ExprV2::IntDivMod(op, Box::new(lhs), Box::new(rhs)));
            }
            if let Some(sum) = variants_of.get(func).cloned() {
                let mut out = Vec::new();
                for a in args {
                    out.push(lower_expr_v2(
                        a,
                        lets,
                        locals,
                        rules,
                        nullary,
                        variants_of,
                        functions,
                        in_rule,
                        helper_enabled,
                    )?);
                }
                Ok(L3ExprV2::Ctor {
                    nominal_sum: sum,
                    variant: func.clone(),
                    args: out,
                })
            } else if let Some(&expected_arity) = functions.get(func) {
                if args.len() != expected_arity {
                    return Err(L3V2LowerError::FunctionArityMismatch {
                        func: func.clone(),
                        expected: expected_arity,
                        found: args.len(),
                    });
                }
                let mut out = Vec::new();
                for a in args {
                    out.push(lower_expr_v2(
                        a,
                        lets,
                        locals,
                        rules,
                        nullary,
                        variants_of,
                        functions,
                        in_rule,
                        helper_enabled,
                    )?);
                }
                Ok(L3ExprV2::Call {
                    func: func.clone(),
                    args: out,
                })
            } else {
                Err(L3V2LowerError::UnresolvedReference(func.clone()))
            }
        }
        ast::Expr::Bin { op, lhs, rhs } => {
            let l = Box::new(lower_expr_v2(
                lhs,
                lets,
                locals,
                rules,
                nullary,
                variants_of,
                functions,
                in_rule,
                helper_enabled,
            )?);
            let r = Box::new(lower_expr_v2(
                rhs,
                lets,
                locals,
                rules,
                nullary,
                variants_of,
                functions,
                in_rule,
                helper_enabled,
            )?);
            match op {
                ast::BinOp::Add => Ok(L3ExprV2::Arith(ArithOpV2::Add, l, r)),
                ast::BinOp::Sub => Ok(L3ExprV2::Arith(ArithOpV2::Sub, l, r)),
                ast::BinOp::Mul => Ok(L3ExprV2::Arith(ArithOpV2::Mul, l, r)),
                ast::BinOp::Div => {
                    if matches!((&*l, &*r), (L3ExprV2::Int(_), L3ExprV2::Int(_))) {
                        Err(L3V2LowerError::DivisionNotAllowed)
                    } else {
                        Ok(L3ExprV2::Arith(ArithOpV2::Div, l, r))
                    }
                }
                ast::BinOp::Lt => Ok(L3ExprV2::Cmp(CmpOpV2::Lt, l, r)),
                ast::BinOp::Le => Ok(L3ExprV2::Cmp(CmpOpV2::Le, l, r)),
                ast::BinOp::Gt => Ok(L3ExprV2::Cmp(CmpOpV2::Gt, l, r)),
                ast::BinOp::Ge => Ok(L3ExprV2::Cmp(CmpOpV2::Ge, l, r)),
                ast::BinOp::Eq => Ok(L3ExprV2::Cmp(CmpOpV2::Eq, l, r)),
                ast::BinOp::Ne => Ok(L3ExprV2::Cmp(CmpOpV2::Ne, l, r)),
                ast::BinOp::AndAnd => Ok(L3ExprV2::And(l, r)),
                ast::BinOp::OrOr => Ok(L3ExprV2::Or(l, r)),
                ast::BinOp::In => Ok(L3ExprV2::In(l, r)),
                ast::BinOp::Then | ast::BinOp::And => Err(L3V2LowerError::Unsupported(
                    "witness composition has no executable meaning in v2".to_string(),
                )),
            }
        }
        ast::Expr::Match {
            scrutinee, arms, ..
        } => {
            let s = Box::new(lower_expr_v2(
                scrutinee,
                lets,
                locals,
                rules,
                nullary,
                variants_of,
                functions,
                in_rule,
                helper_enabled,
            )?);
            let mut out = Vec::new();
            for arm in arms {
                let pat = match &arm.pattern {
                    ast::Pattern::Ctor { name, args } => {
                        let mut binders = Vec::new();
                        let mut seen_binders = BTreeSet::new();
                        for a in args {
                            match a {
                                ast::Pattern::Wildcard => binders.push(None),
                                ast::Pattern::Var(x) => {
                                    if helper_enabled && !seen_binders.insert(x.clone()) {
                                        return Err(L3V2LowerError::DuplicateMatchBinder(
                                            x.clone(),
                                        ));
                                    }
                                    binders.push(Some(x.clone()));
                                }
                                ast::Pattern::Ctor { .. } => {
                                    return Err(L3V2LowerError::NestedPatternNotAllowed);
                                }
                            }
                        }
                        L3PatternV2::Ctor {
                            variant: name.clone(),
                            binders,
                        }
                    }
                    // A top-level wildcard or binder is a default arm. Refused
                    // so malformed input cannot be swallowed by a catch-all.
                    ast::Pattern::Wildcard | ast::Pattern::Var(_) => {
                        return Err(L3V2LowerError::DefaultArmNotAllowed);
                    }
                };
                // The arm's binders are in scope in its body. Without this,
                // `match b { MkBox(v) => v }` reports `v` unresolved — the
                // pattern binds it and the body could not see it.
                let mut arm_locals = locals.clone();
                let L3PatternV2::Ctor { binders, .. } = &pat;
                for b in binders.iter().flatten() {
                    arm_locals.insert(b.clone());
                }
                out.push((
                    pat,
                    lower_expr_v2(
                        &arm.body,
                        lets,
                        &arm_locals,
                        rules,
                        nullary,
                        variants_of,
                        functions,
                        in_rule,
                        helper_enabled,
                    )?,
                ));
            }
            Ok(L3ExprV2::Match {
                scrutinee: s,
                arms: out,
            })
        }
        ast::Expr::Not(inner) => {
            let expr = Box::new(lower_expr_v2(
                inner,
                lets,
                locals,
                rules,
                nullary,
                variants_of,
                functions,
                in_rule,
                helper_enabled,
            )?);
            Ok(L3ExprV2::Not(expr))
        }
        ast::Expr::Prove(_) => Err(L3V2LowerError::Unsupported("prove".to_string())),
        ast::Expr::Why(_) => Err(L3V2LowerError::Unsupported("why".to_string())),
        ast::Expr::Audit(_) => Err(L3V2LowerError::Unsupported("audit".to_string())),
        // A lambda reaches lowering directly only when it appears somewhere
        // other than the recognized second argument of a fold/filter/map
        // call — every valid occurrence is consumed structurally above,
        // before recursing. Lambdas are fold syntax only (ADR-0037): they
        // cannot escape, be stored, or be passed as ordinary function values.
        ast::Expr::Lambda { .. } => Err(L3V2LowerError::LambdaNotAllowed),
        ast::Expr::ListLit(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(lower_expr_v2(
                    item,
                    lets,
                    locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?);
            }
            Ok(L3ExprV2::ListLit(out))
        }
        ast::Expr::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            let mut gen_locals = locals.clone();
            let mut out_generators = Vec::with_capacity(generators.len());
            for (binder, source) in generators {
                let source_lowered = lower_expr_v2(
                    source,
                    lets,
                    &gen_locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?;
                out_generators.push((binder.clone(), source_lowered));
                // A later generator or the `where` clause may reference an
                // earlier binder — the join case ADR-0040 names — so each
                // binder joins scope before the next generator's source is
                // lowered.
                gen_locals.insert(binder.clone());
            }
            let where_lowered = match where_clause {
                Some(w) => Some(Box::new(lower_expr_v2(
                    w,
                    lets,
                    &gen_locals,
                    rules,
                    nullary,
                    variants_of,
                    functions,
                    in_rule,
                    helper_enabled,
                )?)),
                None => None,
            };
            let yield_lowered = Box::new(lower_expr_v2(
                yield_expr,
                lets,
                &gen_locals,
                rules,
                nullary,
                variants_of,
                functions,
                in_rule,
                helper_enabled,
            )?);
            Ok(L3ExprV2::Comprehension {
                generators: out_generators,
                where_clause: where_lowered,
                yield_expr: yield_lowered,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Stage B — the evaluator (ADR-0027 §10)
// ---------------------------------------------------------------------------

/// A v2 value: what a rule body evaluates to, and what a committed fact
/// carries.
///
/// Record and constructor identity include the declaring nominal config, so
/// structurally identical values from different declarations never collapse —
/// the discipline v1 established for `L3ValueV1`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum L3ValueV2 {
    Int(i64),
    Str(String),
    Bool(bool),
    Ctor {
        nominal_sum: String,
        variant: String,
        args: Vec<L3ValueV2>,
    },
    Record {
        nominal_config: String,
        fields: Vec<(String, L3ValueV2)>,
    },
    /// A bounded list value (ADR-0037, ADR-0040). Elements carry their own
    /// nominal identity as usual; the list itself has none — it is a
    /// structural sequence, in evaluation order.
    ///
    /// Shared, not owned: reading a list input or fact, or binding it in a
    /// fold, costs a reference-count increment rather than a deep copy.
    /// Equality, ordering, and every canonical encoding depend only on the
    /// elements.
    List(Arc<[L3ValueV2]>),
    F64(brix_canon::FiniteF64),
    Decimal(brix_canon::Decimal),
}

/// Why an evaluation could not produce a value.
///
/// **Every variant is a refusal, never a weaker result.** ADR-0027 §9 requires
/// a runtime fault to surface as `Unknown(EvaluationFault)` through a
/// distinguished path — never as an empty frontier, never as quiescence, and
/// never as a refutation. Absence of a value is not evidence of anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalFault {
    /// Checked arithmetic overflowed.
    ///
    /// Not wrapped, not saturated, not truncated (ADR-0027 §9.7). A wrapping
    /// result would be a *false record*: the fact would claim a value the
    /// arithmetic did not produce.
    Overflow(ArithOpV2),
    /// A binding or fact that is not in scope at evaluation time.
    Unbound(String),
    /// A field projected from something that is not a record, or a field the
    /// record does not have.
    NoSuchField(String),
    /// A `match` whose scrutinee matched no arm.
    ///
    /// Reachable only if exhaustiveness was not established — which is why
    /// `check_exhaustive` exists and runs before a plan is accepted.
    NoMatchingArm,
    /// An operator applied to operands of the wrong shape.
    ///
    /// Total rather than a panic: the evaluator must stay total on any plan it
    /// is handed, including one whose types were never checked.
    OperandShape(&'static str),
    /// A division or modulo by zero (ADR-0035).
    ///
    /// Its own variant rather than an `Overflow`: the quotient does not exist
    /// at all, which is a different fact from one that exists and does not
    /// fit.
    DivisionByZero(DivModOpV2),
    /// A quotient that is mathematically defined but does not fit in `i64`
    /// (ADR-0035) — reachable only at `i64::MIN / -1`.
    ///
    /// `mod_euclid` never raises this: its result is bounded by `|b|`, so
    /// `mod_euclid(i64::MIN, -1)` is `0` rather than a fault.
    DivisionOverflow(DivModOpV2),
    /// A function call exceeded the maximum call stack depth.
    CallDepthExceeded { limit: usize, func: String },
    /// Evaluation exceeded maximum allowed computational steps.
    ResourceExhausted { limit: usize, detail: String },
    /// A contract violation at function call boundary.
    ContractViolation {
        func: String,
        param: Option<String>,
        expected: Box<str>,
        found: Box<str>,
        path: Option<Box<str>>,
    },
    /// `min`/`max` of an empty list (ADR-0040). Never a default value: an
    /// empty aggregate has no minimum or maximum to report, so this is a
    /// typed evaluation fault rather than `0` or any other placeholder.
    EmptyAggregate(FoldOpV2),
    /// An invalid numeric value, overflow, zero divisor, or inexact decimal result.
    Numeric(String),
}

impl fmt::Display for EvalFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow(op) => write!(f, "arithmetic overflow ({op:?})"),
            Self::Numeric(detail) => write!(f, "numeric fault: {detail}"),
            Self::Unbound(name) => write!(f, "unbound identifier: {name}"),
            Self::NoSuchField(field) => write!(f, "no such field: {field}"),
            Self::NoMatchingArm => write!(f, "no matching arm in match expression"),
            Self::OperandShape(detail) => write!(f, "operand shape error: {detail}"),
            Self::DivisionByZero(op) => {
                write!(f, "division by zero in {}", op.name())
            }
            Self::DivisionOverflow(op) => {
                write!(f, "quotient does not fit in Int in {}", op.name())
            }
            Self::CallDepthExceeded { limit, func } => {
                write!(f, "call depth limit ({limit}) exceeded calling '{func}'")
            }
            Self::ResourceExhausted { limit, detail } => {
                write!(f, "resource limit ({limit}) exhausted: {detail}")
            }
            Self::ContractViolation {
                func,
                param,
                expected,
                found,
                path,
            } => {
                let location = path
                    .as_deref()
                    .map(|p| format!(" at {p}"))
                    .unwrap_or_default();
                if let Some(p) = param {
                    write!(
                        f,
                        "contract violation in function '{func}' for parameter '{p}'{location}: expected {expected}, found {found}"
                    )
                } else {
                    write!(
                        f,
                        "return contract violation in function '{func}'{location}: expected {expected}, found {found}"
                    )
                }
            }
            Self::EmptyAggregate(op) => {
                write!(f, "{}() of an empty list has no value", op.name())
            }
        }
    }
}

impl std::error::Error for EvalFault {}

/// The bindings an evaluation runs under: prior `let` values, and the facts
/// earlier rules have committed.
///
/// The two are separate on purpose. A `let` is a closed static binding fixed
/// when the plan is built; a fact exists only once its rule has committed, and
/// reading one is precisely what ⟨D-DERIVE⟩ admits.
#[derive(Clone, Debug, Default)]
pub struct EvalEnv {
    // Inputs, lets, and facts are fixed for a whole evaluation and shared
    // behind `Arc`, so the per-element environment a fold, filter, map, or
    // match arm builds copies only `locals`, not every value in scope.
    inputs: Arc<BTreeMap<String, L3ValueV2>>,
    lets: Arc<BTreeMap<String, L3ValueV2>>,
    facts: Arc<BTreeMap<String, L3ValueV2>>,
    locals: BTreeMap<String, L3ValueV2>,
    /// Steps spent so far by every evaluation in the enclosing run, if the
    /// caller bounds a whole run (see [`MAX_RUN_STEPS`]).
    run_work: Option<Arc<std::sync::atomic::AtomicUsize>>,
    functions: Arc<BTreeMap<String, L3FunctionDef>>,
    schemas: Arc<BTreeMap<String, L3Schema>>,
}

impl EvalEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_input(mut self, name: impl Into<String>, v: L3ValueV2) -> Self {
        Arc::make_mut(&mut self.inputs).insert(name.into(), v);
        self
    }

    pub fn with_let(mut self, name: impl Into<String>, v: L3ValueV2) -> Self {
        Arc::make_mut(&mut self.lets).insert(name.into(), v);
        self
    }

    pub fn with_fact(mut self, rule: impl Into<String>, v: L3ValueV2) -> Self {
        Arc::make_mut(&mut self.facts).insert(rule.into(), v);
        self
    }

    pub fn with_function(mut self, name: impl Into<String>, def: L3FunctionDef) -> Self {
        let mut map = (*self.functions).clone();
        map.insert(name.into(), def);
        self.functions = Arc::new(map);
        self
    }

    /// Share one work counter across every evaluation that uses this
    /// environment (or a clone of it), bounding their total steps by
    /// [`MAX_RUN_STEPS`].
    pub fn with_run_work(mut self, counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        self.run_work = Some(counter);
        self
    }

    pub fn with_functions(mut self, functions: Arc<BTreeMap<String, L3FunctionDef>>) -> Self {
        self.functions = functions;
        self
    }

    pub fn with_schemas(mut self, schemas: Arc<BTreeMap<String, L3Schema>>) -> Self {
        self.schemas = schemas;
        self
    }

    /// Bound external inputs in the environment.
    pub fn inputs(&self) -> &BTreeMap<String, L3ValueV2> {
        &self.inputs
    }

    /// Registered functions in the environment.
    pub fn functions(&self) -> &Arc<BTreeMap<String, L3FunctionDef>> {
        &self.functions
    }

    pub fn schemas(&self) -> &Arc<BTreeMap<String, L3Schema>> {
        &self.schemas
    }

    /// Whether every rule in `deps` has committed a fact — the eligibility
    /// condition ⟨D-DERIVE⟩ states.
    pub fn satisfies(&self, deps: &[String]) -> bool {
        deps.iter().all(|d| self.facts.contains_key(d))
    }

    /// Bind a name into the local scope (function parameters, `match`-arm
    /// binders). Mirrors exactly what [`eval_internal_body`]'s `Call` and
    /// `Match` cases already do to `locals` inline; exposed so an
    /// explanation builder can reconstruct the identical scope a helper
    /// body or taken arm actually ran under, without duplicating evaluation
    /// logic. Never consulted by `eval` itself beyond the `locals` map it
    /// already reads — purely a scope-construction helper.
    pub(crate) fn with_local(mut self, name: impl Into<String>, v: L3ValueV2) -> Self {
        self.locals.insert(name.into(), v);
        self
    }

    /// Whether `name` resolves through the local scope (a function
    /// parameter or `match`-arm binder) — never a top-level `let`, rule
    /// fact, or external input.
    ///
    /// Provenance only, for an explanation to decide whether a reference is
    /// "already shown" via its call/arm site or is a top-level fact worth
    /// listing on its own. Never used to decide a *value*, so it cannot
    /// change evaluation semantics.
    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.locals.contains_key(name)
    }

    /// Whether `name` resolves to a top-level `let` binding (and is not
    /// shadowed by a local). See [`Self::is_local`].
    pub(crate) fn is_let_binding(&self, name: &str) -> bool {
        !self.locals.contains_key(name) && self.lets.contains_key(name)
    }

    /// Whether `name` resolves to a bound external input (and is not
    /// shadowed by a local). See [`Self::is_local`].
    pub(crate) fn is_bound_input(&self, name: &str) -> bool {
        !self.locals.contains_key(name)
            && !self.lets.contains_key(name)
            && self.inputs.contains_key(name)
    }
}

/// Evaluate a v2 expression to a value.
///
/// **Big-step, and deliberately so.** A rule body evaluates atomically inside
/// one commit, which is what keeps every committed step *realizing* and leaves
/// `𝒢_τ = ∅` — so ⟨D-TAUZERO⟩ and the O(Δ) gate carry over from v1 unchanged
/// (ADR-0027 §2 erratum). A small-step machine would put administrative steps
/// in the journal and forfeit both, to guard a non-termination case
/// ⟨D-DERIVE⟩ has already made unreachable.
///
/// Operands evaluate **left to right**, which is ABI: it fixes which fault a
/// program with two faulty operands reports.
/// Execution bounds tracker for function-enabled evaluations (ADR-0032).
#[derive(Clone, Debug)]
struct EvalBudget {
    pub steps: usize,
    pub max_steps: usize,
    pub recursion_depth: usize,
    pub max_recursion_depth: usize,
    pub call_depth: usize,
    pub max_call_depth: usize,
    pub allocated_nodes: usize,
    pub max_nodes: usize,
    pub allocated_bytes: usize,
    pub max_bytes: usize,
    pub max_value_depth: usize,
    run_work: Option<Arc<std::sync::atomic::AtomicUsize>>,
    unflushed_steps: usize,
}

impl Default for EvalBudget {
    fn default() -> Self {
        Self {
            steps: 0,
            max_steps: MAX_CALL_STEPS,
            recursion_depth: 0,
            max_recursion_depth: MAX_EVAL_RECURSION_DEPTH,
            call_depth: 0,
            max_call_depth: MAX_CALL_DEPTH,
            allocated_nodes: 0,
            max_nodes: MAX_EVAL_VALUE_NODES,
            allocated_bytes: 0,
            max_bytes: MAX_EVAL_VALUE_BYTES,
            max_value_depth: MAX_VALUE_DEPTH,
            run_work: None,
            unflushed_steps: 0,
        }
    }
}

impl EvalBudget {
    /// Charge reading or binding a value the evaluation already holds: one
    /// step of work, and nothing against the allocation budgets. Lists are
    /// shared ([`L3ValueV2::List`]) and any other copy is transient, so
    /// counting reads as allocation would refuse a join of two list inputs
    /// on memory it never keeps. Newly built values are charged in full by
    /// [`Self::charge_value`].
    fn charge_clone(&mut self, _val: &L3ValueV2) -> Result<(), EvalFault> {
        self.tick_step()
    }

    fn charge_allocation(&mut self, nodes: usize, bytes: usize) -> Result<(), EvalFault> {
        if nodes > self.max_nodes.saturating_sub(self.allocated_nodes) {
            return Err(EvalFault::ResourceExhausted {
                limit: self.max_nodes,
                detail: "value node budget exhausted".into(),
            });
        }
        if bytes > self.max_bytes.saturating_sub(self.allocated_bytes) {
            return Err(EvalFault::ResourceExhausted {
                limit: self.max_bytes,
                detail: "value byte budget exhausted".into(),
            });
        }
        self.allocated_nodes += nodes;
        self.allocated_bytes += bytes;
        Ok(())
    }

    /// Charge a newly built value in full: its node count, byte size, and
    /// depth, walked iteratively so a deep value cannot overflow the stack.
    fn charge_value(&mut self, val: &L3ValueV2, depth: usize) -> Result<(), EvalFault> {
        let mut stack: Vec<(&L3ValueV2, usize)> = vec![(val, depth)];
        let mut count = 0usize;
        let mut bytes = 0usize;
        let mut max_depth_seen = 1usize;

        while let Some((node, depth)) = stack.pop() {
            if depth > max_depth_seen {
                max_depth_seen = depth;
            }
            if max_depth_seen > self.max_value_depth {
                return Err(EvalFault::ResourceExhausted {
                    limit: self.max_value_depth,
                    detail: format!(
                        "value nesting depth limit exceeded ({max_depth_seen} > {})",
                        self.max_value_depth
                    ),
                });
            }
            count += 1;
            if self.allocated_nodes + count > self.max_nodes {
                return Err(EvalFault::ResourceExhausted {
                    limit: self.max_nodes,
                    detail: format!(
                        "value node allocation limit exceeded ({} > {})",
                        self.allocated_nodes + count,
                        self.max_nodes
                    ),
                });
            }

            match node {
                L3ValueV2::Int(_)
                | L3ValueV2::Bool(_)
                | L3ValueV2::F64(_)
                | L3ValueV2::Decimal(_) => {
                    bytes += 8;
                }
                L3ValueV2::Str(s) => {
                    bytes += 8 + s.len();
                }
                L3ValueV2::Ctor {
                    args,
                    nominal_sum,
                    variant,
                } => {
                    bytes += 16 + nominal_sum.len() + variant.len();
                    for a in args {
                        stack.push((a, depth + 1));
                    }
                }
                L3ValueV2::Record {
                    fields,
                    nominal_config,
                } => {
                    bytes += 16 + nominal_config.len();
                    for (f, v) in fields {
                        bytes += f.len();
                        stack.push((v, depth + 1));
                    }
                }
                L3ValueV2::List(items) => {
                    bytes += 16;
                    for item in items.iter() {
                        stack.push((item, depth + 1));
                    }
                }
            }

            if self.allocated_bytes + bytes > self.max_bytes {
                return Err(EvalFault::ResourceExhausted {
                    limit: self.max_bytes,
                    detail: format!(
                        "value byte allocation limit exceeded ({} > {})",
                        self.allocated_bytes + bytes,
                        self.max_bytes
                    ),
                });
            }
        }

        self.allocated_nodes += count;
        self.allocated_bytes += bytes;
        Ok(())
    }

    pub fn tick_step(&mut self) -> Result<(), EvalFault> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(EvalFault::ResourceExhausted {
                limit: self.max_steps,
                detail: "evaluation step limit exceeded".to_string(),
            });
        }
        self.unflushed_steps += 1;
        if self.unflushed_steps >= RUN_WORK_FLUSH_STEPS {
            self.flush_run_work()?;
        }
        Ok(())
    }

    /// Add this evaluation's unreported steps to the shared run counter and
    /// fail once the run as a whole passes [`MAX_RUN_STEPS`].
    fn flush_run_work(&mut self) -> Result<(), EvalFault> {
        let pending = std::mem::take(&mut self.unflushed_steps);
        let Some(counter) = &self.run_work else {
            return Ok(());
        };
        let total = counter
            .fetch_add(pending, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(pending);
        if total > MAX_RUN_STEPS {
            return Err(EvalFault::ResourceExhausted {
                limit: MAX_RUN_STEPS,
                detail: "run step limit exceeded".to_string(),
            });
        }
        Ok(())
    }
}

/// Evaluate a v2 expression to a value.
///
/// **Big-step, and deliberately so.** A rule body evaluates atomically inside
/// one commit, which is what keeps every committed step *realizing* and leaves
/// `𝒢_τ = ∅` — so ⟨D-TAUZERO⟩ and the O(Δ) gate carry over from v1 unchanged
/// (ADR-0027 §2 erratum). A small-step machine would put administrative steps
/// in the journal and forfeit both, to guard a non-termination case
/// ⟨D-DERIVE⟩ has already made unreachable.
///
/// Operands evaluate **left to right**, which is ABI: it fixes which fault a
/// program with two faulty operands reports.
pub fn eval(e: &L3ExprV2, env: &EvalEnv) -> Result<L3ValueV2, EvalFault> {
    if env.functions.is_empty() && env.schemas.is_empty() && !expr_has_list_form(e) {
        // No helpers in scope and no list/relational form anywhere in `e`:
        // nothing here can recurse through a `Call`, and there is no
        // cartesian-product-shaped multiplier either, so the fast, unbudgeted
        // path this profile has always used is unchanged.
        let mut budget = None;
        return eval_internal(e, env, &mut budget, None);
    }
    let budgeted = move || {
        let mut budget = Some(EvalBudget {
            run_work: env.run_work.clone(),
            ..EvalBudget::default()
        });
        let value = eval_internal(e, env, &mut budget, None)?;
        budget.as_mut().map_or(Ok(()), EvalBudget::flush_run_work)?;
        Ok(value)
    };
    // Only a helper call can recurse (ADR-0042); without helpers, native
    // stack depth is bounded by expression nesting, so the budgeted
    // evaluation runs on the caller's thread.
    if env.functions.is_empty() {
        return budgeted();
    }
    // See `EVAL_THREAD_STACK_BYTES`: with helpers in scope, evaluation runs
    // on a dedicated, generously sized stack rather than trusting the
    // caller's thread — the budget decides admission, not the native stack.
    // `eval_internal` is total, so a failure here means the OS refused the
    // thread or a defect panicked; both are reported as a resource fault,
    // never propagated as a panic.
    with_eval_stack(budgeted).unwrap_or_else(|| {
        Err(EvalFault::ResourceExhausted {
            limit: EVAL_THREAD_STACK_BYTES,
            detail: "the bounded evaluation thread could not run".to_string(),
        })
    })
}

/// Whether `e` reaches a list/relational form anywhere in its tree
/// (ADR-0037, ADR-0040).
///
/// A finite-decision program without functions or nominal schemas otherwise
/// runs with no evaluator work budget at all, exactly reproducing its
/// pre-ADR-0032 behavior. List iteration can multiply the work a *bounded*
/// expression performs well past its own static node count — a large
/// cartesian product is the sharpest case — so any expression that reaches
/// one of these forms must have the shared budget active regardless of
/// whether it also uses functions or schemas. Programs that use none of these
/// forms are unaffected: this returns `false` for exactly the expressions
/// [`eval`] treated as unbounded before this change.
fn expr_has_list_form(e: &L3ExprV2) -> bool {
    match e {
        L3ExprV2::Fold { .. }
        | L3ExprV2::Filter { .. }
        | L3ExprV2::Map { .. }
        | L3ExprV2::Comprehension { .. }
        | L3ExprV2::ListLit(_)
        | L3ExprV2::In(..)
        | L3ExprV2::Len(_)
        | L3ExprV2::Distinct(_) => true,
        L3ExprV2::Int(_)
        | L3ExprV2::Str(_)
        | L3ExprV2::Bool(_)
        | L3ExprV2::LetRef(_)
        | L3ExprV2::RuleFact(_)
        | L3ExprV2::NullaryVariant { .. } => false,
        L3ExprV2::Ctor { args, .. }
        | L3ExprV2::Call { args, .. }
        | L3ExprV2::NumericBuiltin(_, args) => args.iter().any(expr_has_list_form),
        L3ExprV2::Record { fields, .. } => fields.iter().any(|(_, v)| expr_has_list_form(v)),
        L3ExprV2::Field(base, _) | L3ExprV2::Not(base) => expr_has_list_form(base),
        L3ExprV2::Arith(_, a, b)
        | L3ExprV2::Cmp(_, a, b)
        | L3ExprV2::IntDivMod(_, a, b)
        | L3ExprV2::And(a, b)
        | L3ExprV2::Or(a, b) => expr_has_list_form(a) || expr_has_list_form(b),
        L3ExprV2::Match { scrutinee, arms } => {
            expr_has_list_form(scrutinee) || arms.iter().any(|(_, body)| expr_has_list_form(body))
        }
    }
}

fn numeric_fault(error: brix_canon::NumericError) -> EvalFault {
    EvalFault::Numeric(error.to_string())
}

/// Arithmetic never coerces values between numeric domains.
pub(crate) fn eval_numeric_arith(
    op: ArithOpV2,
    x: L3ValueV2,
    y: L3ValueV2,
) -> Result<L3ValueV2, EvalFault> {
    use brix_canon::{decimal_add, decimal_div_exact, decimal_mul, decimal_sub};
    match (x, y) {
        (L3ValueV2::Int(x), L3ValueV2::Int(y)) => {
            let result = match op {
                ArithOpV2::Add => x.checked_add(y),
                ArithOpV2::Sub => x.checked_sub(y),
                ArithOpV2::Mul => x.checked_mul(y),
                ArithOpV2::Div => {
                    return Err(EvalFault::OperandShape(
                        "Int division requires div_floor, div_ceil, or div_half_even",
                    ))
                }
            };
            result.map(L3ValueV2::Int).ok_or(EvalFault::Overflow(op))
        }
        (L3ValueV2::F64(x), L3ValueV2::F64(y)) => {
            let result = match op {
                ArithOpV2::Add => x.checked_add(y),
                ArithOpV2::Sub => x.checked_sub(y),
                ArithOpV2::Mul => x.checked_mul(y),
                ArithOpV2::Div => x.checked_div(y),
            };
            result.map(L3ValueV2::F64).map_err(numeric_fault)
        }
        (L3ValueV2::Decimal(x), L3ValueV2::Decimal(y)) => {
            let result = match op {
                ArithOpV2::Add => decimal_add(x, y),
                ArithOpV2::Sub => decimal_sub(x, y),
                ArithOpV2::Mul => decimal_mul(x, y),
                ArithOpV2::Div => decimal_div_exact(x, y),
            };
            result.map(L3ValueV2::Decimal).map_err(numeric_fault)
        }
        _ => Err(EvalFault::OperandShape(
            "arithmetic requires operands of the same numeric type",
        )),
    }
}

fn numeric_order(x: &L3ValueV2, y: &L3ValueV2) -> Result<std::cmp::Ordering, EvalFault> {
    match (x, y) {
        (L3ValueV2::Int(x), L3ValueV2::Int(y)) => Ok(x.cmp(y)),
        (L3ValueV2::F64(x), L3ValueV2::F64(y)) => Ok(x.cmp(y)),
        (L3ValueV2::Decimal(x), L3ValueV2::Decimal(y)) => Ok(x.cmp(y)),
        _ => Err(EvalFault::OperandShape(
            "ordering requires operands of the same numeric type",
        )),
    }
}

pub(crate) fn eval_numeric_builtin(
    op: NumericBuiltinV2,
    args: &[L3ValueV2],
) -> Result<L3ValueV2, EvalFault> {
    use brix_canon::{decimal_div_round, decimal_from_i64, decimal_neg, decimal_parse, FiniteF64};
    match (op, args) {
        (NumericBuiltinV2::F64, [L3ValueV2::Str(text)]) => text
            .parse::<FiniteF64>()
            .map(L3ValueV2::F64)
            .map_err(numeric_fault),
        (NumericBuiltinV2::Decimal, [L3ValueV2::Str(text)]) => decimal_parse(text)
            .map(L3ValueV2::Decimal)
            .map_err(numeric_fault),
        (NumericBuiltinV2::F64FromInt, [L3ValueV2::Int(value)]) => {
            Ok(L3ValueV2::F64(FiniteF64::from_i64(*value)))
        }
        (NumericBuiltinV2::DecimalFromInt, [L3ValueV2::Int(value)]) => {
            Ok(L3ValueV2::Decimal(decimal_from_i64(*value)))
        }
        (
            NumericBuiltinV2::DecimalDiv,
            [L3ValueV2::Decimal(a), L3ValueV2::Decimal(b), L3ValueV2::Int(scale), L3ValueV2::Str(mode)],
        ) => {
            let scale = u8::try_from(*scale)
                .ok()
                .filter(|scale| *scale <= 18)
                .ok_or_else(|| EvalFault::Numeric("decimal scale must be in 0..=18".into()))?;
            decimal_div_round(*a, *b, scale, mode)
                .map(L3ValueV2::Decimal)
                .map_err(numeric_fault)
        }
        (NumericBuiltinV2::F64Neg, [L3ValueV2::F64(value)]) => value
            .checked_neg()
            .map(L3ValueV2::F64)
            .map_err(numeric_fault),
        (NumericBuiltinV2::DecimalNeg, [L3ValueV2::Decimal(value)]) => decimal_neg(*value)
            .map(L3ValueV2::Decimal)
            .map_err(numeric_fault),
        _ => Err(EvalFault::OperandShape(
            "invalid numeric builtin argument types or arity",
        )),
    }
}

fn eval_internal(
    e: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    if let Some(b) = budget.as_mut() {
        b.tick_step()?;
        b.recursion_depth += 1;
        if b.recursion_depth > b.max_recursion_depth {
            b.recursion_depth -= 1;
            return Err(EvalFault::CallDepthExceeded {
                limit: b.max_recursion_depth,
                func: current_func
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<eval>".to_string()),
            });
        }
    }

    let res = eval_internal_body(e, env, budget, current_func);

    if let Some(b) = budget.as_mut() {
        b.recursion_depth = b.recursion_depth.saturating_sub(1);
    }

    res
}

fn eval_internal_body(
    e: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    match e {
        L3ExprV2::Int(n) => Ok(L3ValueV2::Int(*n)),
        L3ExprV2::Str(s) => {
            if let Some(b) = budget.as_mut() {
                b.charge_allocation(1, s.len())?;
            }
            Ok(L3ValueV2::Str(s.clone()))
        }
        L3ExprV2::Bool(b) => Ok(L3ValueV2::Bool(*b)),
        L3ExprV2::LetRef(name) => {
            let val = env
                .locals
                .get(name)
                .or_else(|| env.lets.get(name))
                .or_else(|| env.inputs.get(name))
                .ok_or_else(|| EvalFault::Unbound(name.clone()))?;
            if let Some(b) = budget.as_mut() {
                b.charge_clone(val)?;
            }
            Ok(val.clone())
        }
        L3ExprV2::RuleFact(rule) => {
            let val = env
                .facts
                .get(rule)
                .ok_or_else(|| EvalFault::Unbound(rule.clone()))?;
            if let Some(b) = budget.as_mut() {
                b.charge_clone(val)?;
            }
            Ok(val.clone())
        }
        L3ExprV2::NullaryVariant {
            nominal_sum,
            variant,
        } => {
            if let Some(b) = budget.as_mut() {
                b.charge_allocation(1, nominal_sum.len() + variant.len())?;
            }
            Ok(L3ValueV2::Ctor {
                nominal_sum: nominal_sum.clone(),
                variant: variant.clone(),
                args: Vec::new(),
            })
        }
        L3ExprV2::Ctor {
            nominal_sum,
            variant,
            args,
        } => {
            if let Some(b) = budget.as_mut() {
                b.charge_allocation(
                    1,
                    nominal_sum.len()
                        + variant.len()
                        + args.len().saturating_mul(std::mem::size_of::<L3ValueV2>()),
                )?;
            }
            let mut out = Vec::with_capacity(args.len());
            for a in args {
                let value = eval_internal(a, env, budget, current_func)?;
                if let Some(b) = budget.as_mut() {
                    b.charge_value(&value, 2)?;
                }
                out.push(value);
            }
            Ok(L3ValueV2::Ctor {
                nominal_sum: nominal_sum.clone(),
                variant: variant.clone(),
                args: out,
            })
        }
        L3ExprV2::Record {
            nominal_config,
            fields,
        } => {
            if let Some(b) = budget.as_mut() {
                b.charge_allocation(
                    1,
                    nominal_config.len()
                        + fields
                            .len()
                            .saturating_mul(std::mem::size_of::<(String, L3ValueV2)>()),
                )?;
            }
            let mut out = Vec::with_capacity(fields.len());
            for (name, expr) in fields {
                if let Some(b) = budget.as_mut() {
                    b.charge_allocation(0, name.len())?;
                }
                let value = eval_internal(expr, env, budget, current_func)?;
                if let Some(b) = budget.as_mut() {
                    b.charge_value(&value, 2)?;
                }
                out.push((name.clone(), value));
            }
            Ok(L3ValueV2::Record {
                nominal_config: nominal_config.clone(),
                fields: out,
            })
        }
        L3ExprV2::Field(base, field) => match eval_internal(base, env, budget, current_func)? {
            L3ValueV2::Record { fields, .. } => fields
                .into_iter()
                .find(|(n, _)| n == field)
                .map(|(_, v)| v)
                .ok_or_else(|| EvalFault::NoSuchField(field.clone())),
            _ => Err(EvalFault::NoSuchField(field.clone())),
        },
        L3ExprV2::Arith(op, a, b) => {
            let (x, y) = (
                eval_internal(a, env, budget, current_func)?,
                eval_internal(b, env, budget, current_func)?,
            );
            eval_numeric_arith(*op, x, y)
        }
        L3ExprV2::Cmp(op, a, b) => {
            let (x, y) = (
                eval_internal(a, env, budget, current_func)?,
                eval_internal(b, env, budget, current_func)?,
            );
            // Numeric domains never coerce, including equality. Existing
            // nonnumeric structural equality retains its historical behavior.
            if (matches!(x, L3ValueV2::F64(_) | L3ValueV2::Decimal(_))
                || matches!(y, L3ValueV2::F64(_) | L3ValueV2::Decimal(_)))
                && type_of_value(&x) != type_of_value(&y)
            {
                return Err(EvalFault::OperandShape(
                    "comparison requires operands of the same numeric type",
                ));
            }
            let out = match op {
                CmpOpV2::Eq => x == y,
                CmpOpV2::Ne => x != y,
                _ => {
                    let ordering = numeric_order(&x, &y)?;
                    match op {
                        CmpOpV2::Lt => ordering.is_lt(),
                        CmpOpV2::Le => ordering.is_le(),
                        CmpOpV2::Gt => ordering.is_gt(),
                        CmpOpV2::Ge => ordering.is_ge(),
                        CmpOpV2::Eq | CmpOpV2::Ne => unreachable!("handled above"),
                    }
                }
            };
            Ok(L3ValueV2::Bool(out))
        }
        L3ExprV2::And(a, b) => {
            let val_a = eval_internal(a, env, budget, current_func)?;
            let L3ValueV2::Bool(bool_a) = val_a else {
                return Err(EvalFault::OperandShape(
                    "logical AND requires Bool operands",
                ));
            };
            if !bool_a {
                return Ok(L3ValueV2::Bool(false));
            }
            let val_b = eval_internal(b, env, budget, current_func)?;
            let L3ValueV2::Bool(bool_b) = val_b else {
                return Err(EvalFault::OperandShape(
                    "logical AND requires Bool operands",
                ));
            };
            Ok(L3ValueV2::Bool(bool_b))
        }
        L3ExprV2::Or(a, b) => {
            let val_a = eval_internal(a, env, budget, current_func)?;
            let L3ValueV2::Bool(bool_a) = val_a else {
                return Err(EvalFault::OperandShape("logical OR requires Bool operands"));
            };
            if bool_a {
                return Ok(L3ValueV2::Bool(true));
            }
            let val_b = eval_internal(b, env, budget, current_func)?;
            let L3ValueV2::Bool(bool_b) = val_b else {
                return Err(EvalFault::OperandShape("logical OR requires Bool operands"));
            };
            Ok(L3ValueV2::Bool(bool_b))
        }
        L3ExprV2::NumericBuiltin(op, args) => {
            if args.len() != op.arity() {
                return Err(EvalFault::OperandShape("numeric builtin arity mismatch"));
            }
            let args = args
                .iter()
                .map(|a| eval_internal(a, env, budget, current_func))
                .collect::<Result<Vec<_>, _>>()?;
            eval_numeric_builtin(*op, &args)
        }
        L3ExprV2::IntDivMod(op, a, b) => {
            let (x, y) = (
                eval_internal(a, env, budget, current_func)?,
                eval_internal(b, env, budget, current_func)?,
            );
            let (L3ValueV2::Int(x), L3ValueV2::Int(y)) = (x, y) else {
                return Err(EvalFault::OperandShape(
                    "integer division requires Int operands",
                ));
            };
            eval_div_mod(*op, x, y).map(L3ValueV2::Int)
        }
        L3ExprV2::Not(a) => {
            let val_a = eval_internal(a, env, budget, current_func)?;
            let L3ValueV2::Bool(bool_a) = val_a else {
                return Err(EvalFault::OperandShape("logical NOT requires Bool operand"));
            };
            Ok(L3ValueV2::Bool(!bool_a))
        }
        L3ExprV2::Match { scrutinee, arms } => {
            let v = eval_internal(scrutinee, env, budget, current_func)?;
            let (variant, args) = match &v {
                L3ValueV2::Ctor { variant, args, .. } => (variant.as_str(), args.as_slice()),
                // A boolean scrutinee matches the `true`/`false` constructors,
                // which is how `Bool` is spelled as a two-variant sum.
                L3ValueV2::Bool(b) => (if *b { "true" } else { "false" }, [].as_slice()),
                _ => return Err(EvalFault::OperandShape("match requires a sum value")),
            };
            for (pat, body) in arms {
                let L3PatternV2::Ctor {
                    variant: pv,
                    binders,
                } = pat;
                if pv != variant {
                    continue;
                }
                if binders.len() != args.len() {
                    return Err(EvalFault::OperandShape("constructor arity mismatch"));
                }
                // Cloning the environment copies only `locals`; inputs, lets,
                // and facts are shared (see `EvalEnv`).
                if let Some(b) = budget.as_mut() {
                    for (name, value) in env.locals.iter() {
                        b.charge_allocation(0, name.len())?;
                        b.charge_clone(value)?;
                    }
                }
                let mut arm_env = env.clone();
                for (b, a) in binders.iter().zip(args.iter()) {
                    if let Some(name) = b {
                        if let Some(b_budget) = budget.as_mut() {
                            b_budget.charge_clone(a)?;
                        }
                        arm_env.locals.insert(name.clone(), a.clone());
                    }
                }
                return eval_internal(body, &arm_env, budget, current_func);
            }
            Err(EvalFault::NoMatchingArm)
        }
        L3ExprV2::Call { func, args } => {
            // Borrow def directly without cloning from functions map.
            let def = match env.functions.get(func) {
                Some(d) => d,
                None => return Err(EvalFault::Unbound(func.clone())),
            };
            if def.params.len() != args.len() {
                return Err(EvalFault::OperandShape("function arity mismatch"));
            }

            // Eager call-by-value argument evaluation in caller's environment.
            // Charge the result slots before reserving the vector so a large
            // arity cannot bypass the value-growth limits through container
            // capacity alone.
            if let Some(b) = budget.as_mut() {
                b.charge_allocation(
                    args.len(),
                    args.len().saturating_mul(std::mem::size_of::<L3ValueV2>()),
                )?;
            }
            let mut evaluated_args = Vec::with_capacity(args.len());
            for a in args {
                evaluated_args.push(eval_internal(a, env, budget, current_func)?);
            }

            // Parameter contract checks.
            for (val, (param_name, contract)) in evaluated_args.iter().zip(def.params.iter()) {
                if let Some(expected_ty) = contract {
                    if let Err(detail) = validate_l3_value(val, expected_ty, &def.schemas, "<root>")
                    {
                        let path = if matches!(
                            expected_ty,
                            L3SchemaType::Int | L3SchemaType::Bool | L3SchemaType::Str
                        ) {
                            None
                        } else {
                            Some(detail.into_boxed_str())
                        };
                        return Err(EvalFault::ContractViolation {
                            func: func.clone(),
                            param: Some(param_name.clone()),
                            expected: expected_ty.display_name().into_boxed_str(),
                            found: type_of_value(val).to_string().into_boxed_str(),
                            path,
                        });
                    }
                }
            }

            if let Some(b) = budget.as_mut() {
                b.call_depth += 1;
                if b.call_depth > b.max_call_depth {
                    b.call_depth -= 1;
                    return Err(EvalFault::CallDepthExceeded {
                        limit: b.max_call_depth,
                        func: func.clone(),
                    });
                }
            }

            // Closed execution environment for function body:
            // only binds parameters into locals, with access to functions.
            // Caller locals, lets, facts, and inputs are not accessible.
            if let Some(b) = budget.as_mut() {
                let key_bytes = def
                    .params
                    .iter()
                    .fold(0usize, |total, (name, _)| total.saturating_add(name.len()));
                b.charge_allocation(
                    def.params.len(),
                    key_bytes.saturating_add(
                        def.params
                            .len()
                            .saturating_mul(std::mem::size_of::<(String, L3ValueV2)>()),
                    ),
                )?;
            }
            let mut fn_locals = BTreeMap::new();
            for ((param_name, _), val) in def.params.iter().zip(evaluated_args) {
                fn_locals.insert(param_name.clone(), val);
            }
            let fn_env = EvalEnv {
                inputs: Arc::default(),
                lets: Arc::default(),
                facts: Arc::default(),
                locals: fn_locals,
                run_work: env.run_work.clone(),
                functions: env.functions.clone(),
                schemas: def.schemas.clone(),
            };

            let ret_res = eval_internal(&def.body, &fn_env, budget, Some(func.as_str()));

            if let Some(b) = budget.as_mut() {
                b.call_depth -= 1;
            }

            let ret_val = ret_res?;

            // Return contract check.
            if let Some(expected_ret_ty) = &def.ret_contract {
                if let Err(detail) =
                    validate_l3_value(&ret_val, expected_ret_ty, &def.schemas, "<root>")
                {
                    let path = if matches!(
                        expected_ret_ty,
                        L3SchemaType::Int | L3SchemaType::Bool | L3SchemaType::Str
                    ) {
                        None
                    } else {
                        Some(detail.into_boxed_str())
                    };
                    return Err(EvalFault::ContractViolation {
                        func: func.clone(),
                        param: None,
                        expected: expected_ret_ty.display_name().into_boxed_str(),
                        found: type_of_value(&ret_val).to_string().into_boxed_str(),
                        path,
                    });
                }
            }

            Ok(ret_val)
        }
        L3ExprV2::Fold {
            op,
            list,
            binder,
            body,
        } => eval_fold(*op, list, binder, body, env, budget, current_func),
        L3ExprV2::Filter { list, binder, cond } => {
            eval_filter(list, binder, cond, env, budget, current_func)
        }
        L3ExprV2::Map { list, binder, body } => {
            eval_map(list, binder, body, env, budget, current_func)
        }
        L3ExprV2::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => eval_comprehension(
            generators,
            where_clause,
            yield_expr,
            env,
            budget,
            current_func,
        ),
        L3ExprV2::ListLit(items) => eval_list_lit(items, env, budget, current_func),
        L3ExprV2::In(needle, haystack) => eval_in(needle, haystack, env, budget, current_func),
        L3ExprV2::Len(list) => eval_len(list, env, budget, current_func),
        L3ExprV2::Distinct(list) => eval_distinct(list, env, budget, current_func),
    }
}

// The list/relational evaluators below are pulled out of `eval_internal_body`
// (rather than inlined as match arms, as every other form in this evaluator
// is) so that function's own per-call stack frame — which every recursive
// evaluation depth is charged against, including deep non-list recursion
// such as chained helper calls — stays close to its pre-ADR-0037 size. A
// debug build does not reuse one match arm's stack slots for another, so a
// large arm here would otherwise widen every recursive call in this
// evaluator, including ones that never touch a list, and a bound like
// `MAX_EVAL_RECURSION_DEPTH` is only as tight as the frame size it was
// chosen against.
#[inline(never)]
fn eval_fold(
    op: FoldOpV2,
    list: &L3ExprV2,
    binder: &str,
    body: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let list_val = eval_internal(list, env, budget, current_func)?;
    let L3ValueV2::List(items) = list_val else {
        return Err(EvalFault::OperandShape("fold requires a List operand"));
    };
    match op {
        FoldOpV2::Sum => {
            let mut acc: i64 = 0;
            for item in items.iter().cloned() {
                if let Some(b) = budget.as_mut() {
                    b.charge_clone(&item)?;
                }
                let mut item_env = env.clone();
                item_env.locals.insert(binder.to_string(), item);
                let v = eval_internal(body, &item_env, budget, current_func)?;
                let L3ValueV2::Int(n) = v else {
                    return Err(EvalFault::OperandShape("sum body must evaluate to Int"));
                };
                acc = acc
                    .checked_add(n)
                    .ok_or(EvalFault::Overflow(ArithOpV2::Add))?;
            }
            Ok(L3ValueV2::Int(acc))
        }
        FoldOpV2::Count => {
            let mut acc: i64 = 0;
            for item in items.iter().cloned() {
                if let Some(b) = budget.as_mut() {
                    b.charge_clone(&item)?;
                }
                let mut item_env = env.clone();
                item_env.locals.insert(binder.to_string(), item);
                let v = eval_internal(body, &item_env, budget, current_func)?;
                let L3ValueV2::Bool(keep) = v else {
                    return Err(EvalFault::OperandShape("count body must evaluate to Bool"));
                };
                if keep {
                    acc = acc
                        .checked_add(1)
                        .ok_or(EvalFault::Overflow(ArithOpV2::Add))?;
                }
            }
            Ok(L3ValueV2::Int(acc))
        }
        FoldOpV2::All => {
            for item in items.iter().cloned() {
                if let Some(b) = budget.as_mut() {
                    b.charge_clone(&item)?;
                }
                let mut item_env = env.clone();
                item_env.locals.insert(binder.to_string(), item);
                let v = eval_internal(body, &item_env, budget, current_func)?;
                let L3ValueV2::Bool(keep) = v else {
                    return Err(EvalFault::OperandShape("all body must evaluate to Bool"));
                };
                if !keep {
                    return Ok(L3ValueV2::Bool(false));
                }
            }
            Ok(L3ValueV2::Bool(true))
        }
        FoldOpV2::Any => {
            for item in items.iter().cloned() {
                if let Some(b) = budget.as_mut() {
                    b.charge_clone(&item)?;
                }
                let mut item_env = env.clone();
                item_env.locals.insert(binder.to_string(), item);
                let v = eval_internal(body, &item_env, budget, current_func)?;
                let L3ValueV2::Bool(keep) = v else {
                    return Err(EvalFault::OperandShape("any body must evaluate to Bool"));
                };
                if keep {
                    return Ok(L3ValueV2::Bool(true));
                }
            }
            Ok(L3ValueV2::Bool(false))
        }
        FoldOpV2::Min | FoldOpV2::Max => {
            if items.is_empty() {
                return Err(EvalFault::EmptyAggregate(op));
            }
            let mut acc: Option<i64> = None;
            for item in items.iter().cloned() {
                if let Some(b) = budget.as_mut() {
                    b.charge_clone(&item)?;
                }
                let mut item_env = env.clone();
                item_env.locals.insert(binder.to_string(), item);
                let v = eval_internal(body, &item_env, budget, current_func)?;
                let L3ValueV2::Int(n) = v else {
                    return Err(EvalFault::OperandShape("min/max body must evaluate to Int"));
                };
                acc = Some(match acc {
                    None => n,
                    Some(cur) if op == FoldOpV2::Min => cur.min(n),
                    Some(cur) => cur.max(n),
                });
            }
            Ok(L3ValueV2::Int(acc.expect("non-empty checked above")))
        }
    }
}

#[inline(never)]
fn eval_filter(
    list: &L3ExprV2,
    binder: &str,
    cond: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let list_val = eval_internal(list, env, budget, current_func)?;
    let L3ValueV2::List(items) = list_val else {
        return Err(EvalFault::OperandShape("filter requires a List operand"));
    };
    let mut out = Vec::new();
    for item in items.iter().cloned() {
        if let Some(b) = budget.as_mut() {
            b.charge_clone(&item)?;
        }
        let mut item_env = env.clone();
        item_env.locals.insert(binder.to_string(), item.clone());
        let v = eval_internal(cond, &item_env, budget, current_func)?;
        let L3ValueV2::Bool(keep) = v else {
            return Err(EvalFault::OperandShape(
                "filter condition must evaluate to Bool",
            ));
        };
        if keep {
            if out.len() >= MAX_DERIVED_LIST_LEN {
                return Err(EvalFault::ResourceExhausted {
                    limit: MAX_DERIVED_LIST_LEN,
                    detail: "derived list length limit exceeded".to_string(),
                });
            }
            out.push(item);
        }
    }
    Ok(L3ValueV2::List(out.into()))
}

#[inline(never)]
fn eval_map(
    list: &L3ExprV2,
    binder: &str,
    body: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let list_val = eval_internal(list, env, budget, current_func)?;
    let L3ValueV2::List(items) = list_val else {
        return Err(EvalFault::OperandShape("map requires a List operand"));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items.iter().cloned() {
        if let Some(b) = budget.as_mut() {
            b.charge_clone(&item)?;
        }
        let mut item_env = env.clone();
        item_env.locals.insert(binder.to_string(), item);
        let v = eval_internal(body, &item_env, budget, current_func)?;
        if out.len() >= MAX_DERIVED_LIST_LEN {
            return Err(EvalFault::ResourceExhausted {
                limit: MAX_DERIVED_LIST_LEN,
                detail: "derived list length limit exceeded".to_string(),
            });
        }
        out.push(v);
    }
    Ok(L3ValueV2::List(out.into()))
}

/// Nested generator loops, left to right, evaluated recursively so a later
/// generator's environment already carries every earlier binder — the join
/// case ADR-0040 names. Its own function (not nested inside
/// [`eval_comprehension`]) for the same stack-frame-size reason the sibling
/// `eval_*` functions here are pulled out of `eval_internal_body`.
#[inline(never)]
fn eval_comprehension_generators(
    gens: &[(String, L3ExprV2)],
    where_clause: &Option<Box<L3ExprV2>>,
    yield_expr: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
    out: &mut Vec<L3ValueV2>,
) -> Result<(), EvalFault> {
    match gens.split_first() {
        None => {
            let keep = match where_clause {
                None => true,
                Some(w) => {
                    let v = eval_internal(w, env, budget, current_func)?;
                    let L3ValueV2::Bool(b) = v else {
                        return Err(EvalFault::OperandShape(
                            "comprehension 'where' must evaluate to Bool",
                        ));
                    };
                    b
                }
            };
            if keep {
                let v = eval_internal(yield_expr, env, budget, current_func)?;
                if out.len() >= MAX_DERIVED_LIST_LEN {
                    return Err(EvalFault::ResourceExhausted {
                        limit: MAX_DERIVED_LIST_LEN,
                        detail: "derived list length limit exceeded".to_string(),
                    });
                }
                out.push(v);
            }
            Ok(())
        }
        Some(((binder, source), rest)) => {
            let list_val = eval_internal(source, env, budget, current_func)?;
            let L3ValueV2::List(items) = list_val else {
                return Err(EvalFault::OperandShape(
                    "comprehension generator requires a List operand",
                ));
            };
            for item in items.iter().cloned() {
                if let Some(b) = budget.as_mut() {
                    b.charge_clone(&item)?;
                }
                let mut next_env = env.clone();
                next_env.locals.insert(binder.clone(), item);
                eval_comprehension_generators(
                    rest,
                    where_clause,
                    yield_expr,
                    &next_env,
                    budget,
                    current_func,
                    out,
                )?;
            }
            Ok(())
        }
    }
}

#[inline(never)]
fn eval_comprehension(
    generators: &[(String, L3ExprV2)],
    where_clause: &Option<Box<L3ExprV2>>,
    yield_expr: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let mut out = Vec::new();
    eval_comprehension_generators(
        generators,
        where_clause,
        yield_expr,
        env,
        budget,
        current_func,
        &mut out,
    )?;
    Ok(L3ValueV2::List(out.into()))
}

#[inline(never)]
fn eval_list_lit(
    items: &[L3ExprV2],
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    if items.len() > MAX_DERIVED_LIST_LEN {
        return Err(EvalFault::ResourceExhausted {
            limit: MAX_DERIVED_LIST_LEN,
            detail: "derived list length limit exceeded".to_string(),
        });
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let v = eval_internal(item, env, budget, current_func)?;
        if let Some(b) = budget.as_mut() {
            b.charge_value(&v, 2)?;
        }
        out.push(v);
    }
    Ok(L3ValueV2::List(out.into()))
}

#[inline(never)]
fn eval_in(
    needle: &L3ExprV2,
    haystack: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let needle_val = eval_internal(needle, env, budget, current_func)?;
    let haystack_val = eval_internal(haystack, env, budget, current_func)?;
    let L3ValueV2::List(items) = haystack_val else {
        return Err(EvalFault::OperandShape(
            "'in' requires a List right operand",
        ));
    };
    let mut found = false;
    for item in items.iter() {
        if let Some(b) = budget.as_mut() {
            b.tick_step()?;
        }
        if *item == needle_val {
            found = true;
            break;
        }
    }
    Ok(L3ValueV2::Bool(found))
}

#[inline(never)]
fn eval_len(
    list: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let list_val = eval_internal(list, env, budget, current_func)?;
    let L3ValueV2::List(items) = list_val else {
        return Err(EvalFault::OperandShape("len requires a List operand"));
    };
    Ok(L3ValueV2::Int(items.len() as i64))
}

#[inline(never)]
fn eval_distinct(
    list: &L3ExprV2,
    env: &EvalEnv,
    budget: &mut Option<EvalBudget>,
    current_func: Option<&str>,
) -> Result<L3ValueV2, EvalFault> {
    let list_val = eval_internal(list, env, budget, current_func)?;
    let L3ValueV2::List(items) = list_val else {
        return Err(EvalFault::OperandShape("distinct requires a List operand"));
    };
    // First occurrence kept, order otherwise preserved (ADR-0040). A
    // `BTreeSet` gives an O(log n) membership check without a hash map
    // (`disallowed_types`); the insertion-ordered `out` is the value this
    // expression actually produces.
    let mut seen: BTreeSet<L3ValueV2> = BTreeSet::new();
    let mut out = Vec::new();
    for item in items.iter().cloned() {
        if let Some(b) = budget.as_mut() {
            b.tick_step()?;
            b.charge_clone(&item)?;
        }
        if seen.insert(item.clone()) {
            if out.len() >= MAX_DERIVED_LIST_LEN {
                return Err(EvalFault::ResourceExhausted {
                    limit: MAX_DERIVED_LIST_LEN,
                    detail: "derived list length limit exceeded".to_string(),
                });
            }
            out.push(item);
        }
    }
    Ok(L3ValueV2::List(out.into()))
}

/// Check that every `match` in `plan` is exhaustive over its scrutinee's sum.
///
/// ADR-0027 §5 requires exhaustiveness, and Stage A deliberately declared no
/// error for it because Stage A could not check it. This is that check, using
/// the variant universe the plan now carries.
///
/// Deliberately **conservative about what it can see**: a `match` whose
/// scrutinee's sum cannot be determined from the plan is left alone rather
/// than guessed at. It will fault at evaluation as `NoMatchingArm` if a case
/// is missing, which is a refusal, not a wrong answer.
pub fn check_exhaustive(plan: &L3PlanV2) -> Result<(), L3V2LowerError> {
    let mut sum_of_variant: BTreeMap<String, String> = BTreeMap::new();
    let mut variants_of_sum: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in &plan.items {
        if let L3PlanItemV2::Config(c) = item {
            if let L3ConfigBodyV2::Sum(variants) = &c.body {
                for (v, _) in variants {
                    sum_of_variant.insert(v.clone(), c.name.clone());
                }
                variants_of_sum.insert(
                    c.name.clone(),
                    variants.iter().map(|(v, _)| v.clone()).collect(),
                );
            }
        }
    }
    // `Bool` is a builtin two-variant sum; a boolean match must cover both.
    variants_of_sum.insert("Bool".to_string(), vec!["false".into(), "true".into()]);
    sum_of_variant.insert("true".to_string(), "Bool".to_string());
    sum_of_variant.insert("false".to_string(), "Bool".to_string());

    for item in &plan.items {
        let body = match item {
            L3PlanItemV2::Let { value, .. } => value,
            L3PlanItemV2::Rule { body, .. } => body,
            L3PlanItemV2::Config(_) => continue,
        };
        check_exhaustive_expr(body, &sum_of_variant, &variants_of_sum)?;
    }
    Ok(())
}

pub(crate) fn check_exhaustive_expr(
    e: &L3ExprV2,
    sum_of_variant: &BTreeMap<String, String>,
    variants_of_sum: &BTreeMap<String, Vec<String>>,
) -> Result<(), L3V2LowerError> {
    match e {
        L3ExprV2::Match { scrutinee, arms } => {
            check_exhaustive_expr(scrutinee, sum_of_variant, variants_of_sum)?;
            for (_, body) in arms {
                check_exhaustive_expr(body, sum_of_variant, variants_of_sum)?;
            }
            let covered: BTreeSet<&str> = arms
                .iter()
                .map(|(L3PatternV2::Ctor { variant, .. }, _)| variant.as_str())
                .collect();
            // The sum is identified from the arms, not from the scrutinee:
            // Stage A does not type expressions, and an arm's constructor
            // names its sum unambiguously.
            let Some(first) = arms.first() else {
                return Err(L3V2LowerError::NonExhaustiveMatch {
                    sum: "<empty match>".to_string(),
                    missing: Vec::new(),
                });
            };
            let L3PatternV2::Ctor { variant, .. } = &first.0;
            let Some(sum) = sum_of_variant.get(variant) else {
                return Ok(()); // unknown sum — left alone rather than guessed
            };
            let Some(all) = variants_of_sum.get(sum) else {
                return Ok(());
            };
            let missing: Vec<String> = all
                .iter()
                .filter(|v| !covered.contains(v.as_str()))
                .cloned()
                .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                Err(L3V2LowerError::NonExhaustiveMatch {
                    sum: sum.clone(),
                    missing,
                })
            }
        }
        L3ExprV2::Ctor { args, .. }
        | L3ExprV2::Call { args, .. }
        | L3ExprV2::NumericBuiltin(_, args) => {
            for a in args {
                check_exhaustive_expr(a, sum_of_variant, variants_of_sum)?;
            }
            Ok(())
        }
        L3ExprV2::Record { fields, .. } => {
            for (_, v) in fields {
                check_exhaustive_expr(v, sum_of_variant, variants_of_sum)?;
            }
            Ok(())
        }
        L3ExprV2::Field(b, _) => check_exhaustive_expr(b, sum_of_variant, variants_of_sum),
        L3ExprV2::Arith(_, a, b)
        | L3ExprV2::Cmp(_, a, b)
        | L3ExprV2::IntDivMod(_, a, b)
        | L3ExprV2::And(a, b)
        | L3ExprV2::Or(a, b) => {
            check_exhaustive_expr(a, sum_of_variant, variants_of_sum)?;
            check_exhaustive_expr(b, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::Not(a) => check_exhaustive_expr(a, sum_of_variant, variants_of_sum),
        L3ExprV2::In(a, b) => {
            check_exhaustive_expr(a, sum_of_variant, variants_of_sum)?;
            check_exhaustive_expr(b, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::Len(a) | L3ExprV2::Distinct(a) => {
            check_exhaustive_expr(a, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::ListLit(items) => {
            for item in items {
                check_exhaustive_expr(item, sum_of_variant, variants_of_sum)?;
            }
            Ok(())
        }
        L3ExprV2::Fold { list, body, .. } => {
            check_exhaustive_expr(list, sum_of_variant, variants_of_sum)?;
            check_exhaustive_expr(body, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::Filter { list, cond, .. } => {
            check_exhaustive_expr(list, sum_of_variant, variants_of_sum)?;
            check_exhaustive_expr(cond, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::Map { list, body, .. } => {
            check_exhaustive_expr(list, sum_of_variant, variants_of_sum)?;
            check_exhaustive_expr(body, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            for (_, source) in generators {
                check_exhaustive_expr(source, sum_of_variant, variants_of_sum)?;
            }
            if let Some(w) = where_clause {
                check_exhaustive_expr(w, sum_of_variant, variants_of_sum)?;
            }
            check_exhaustive_expr(yield_expr, sum_of_variant, variants_of_sum)
        }
        L3ExprV2::Int(_)
        | L3ExprV2::Str(_)
        | L3ExprV2::Bool(_)
        | L3ExprV2::LetRef(_)
        | L3ExprV2::RuleFact(_)
        | L3ExprV2::NullaryVariant { .. } => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Stage C — the runner (ADR-0027 §10)
// ---------------------------------------------------------------------------

/// One committed fact: a rule's body evaluated, published, and available for
/// later rules to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedFactV2 {
    /// Commit order, from zero. Part of the record rather than derived from
    /// position, so a reader never has to trust the container's ordering.
    pub ordinal: u64,
    pub rule: String,
    pub value: L3ValueV2,
    /// The facts this one was derived from, as declared in the plan.
    pub depends_on: Vec<String>,
}

/// Why a v2 run stopped.
///
/// **None of these is a quiescence certificate**, and the names deliberately
/// avoid borrowing that word. ADR-0012 §5 makes `Quiescent` the only decided
/// negative *and* makes it certificate-backed: an empty frontier is a decided
/// negative only when a checker can re-derive that it was empty. This runner
/// produces no certificate — it is driven directly rather than through
/// `run_saturated` — so it reports what it observed and claims nothing more.
/// Wiring it to the saturated driver, and earning the certificate, is a later
/// stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStopV2 {
    /// Every rule in the plan committed.
    AllRulesCommitted,
    /// No rule is eligible, and some remain uncommitted.
    ///
    /// Reachable when a rule depends on one that faulted. Not an error in
    /// itself — it is the honest report that the run ran out of eligible work
    /// with obligations outstanding, which ADR-0012 ⟨D-RESIDUE⟩ treats as a
    /// result that MUST qualify the output rather than be presented as
    /// success.
    NoEligibleRule { pending: Vec<String> },
    /// A rule's body faulted. The run stops: a later rule may depend on this
    /// fact, and continuing would silently publish a world in which the
    /// dependency never resolved.
    Faulted { rule: String, fault: EvalFault },
}

/// The result of running a v2 plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3RunV2 {
    pub facts: Vec<CommittedFactV2>,
    pub stop: RunStopV2,
}

/// Run a v2 plan to completion.
///
/// **Terminates structurally** (ADR-0027 §4 ⟨D-TERMINATES⟩): each rule commits
/// at most once over an acyclic dependency graph, so a plan of `N` rules
/// admits at most `N` commits. There is no budget parameter because there is
/// no unbounded case to bound — a measure would be machinery guarding a case
/// that cannot arise.
///
/// **Deterministic.** Among eligible rules the runner always takes the one
/// declared earliest, so the same plan produces the same fact order every
/// time. That ordering is semantic: it is what a later stage's journal and
/// certificate identities would be built from.
pub fn run_l3_plan_v2(plan: &L3PlanV2) -> L3RunV2 {
    // Rules in declaration order, which is also the selection order.
    let rules: Vec<(&str, &L3ExprV2, &[String])> = plan
        .items
        .iter()
        .filter_map(|i| match i {
            L3PlanItemV2::Rule {
                name,
                body,
                depends_on,
            } => Some((name.as_str(), body, depends_on.as_slice())),
            _ => None,
        })
        .collect();

    // `let` bindings are closed and evaluated once, before any rule runs.
    let mut env = EvalEnv::new();
    for item in &plan.items {
        if let L3PlanItemV2::Let { name, value } = item {
            match eval(value, &env) {
                Ok(v) => env = env.with_let(name.clone(), v),
                Err(fault) => {
                    return L3RunV2 {
                        facts: Vec::new(),
                        stop: RunStopV2::Faulted {
                            rule: name.clone(),
                            fault,
                        },
                    }
                }
            }
        }
    }

    let mut committed: BTreeSet<String> = BTreeSet::new();
    let mut facts: Vec<CommittedFactV2> = Vec::new();

    loop {
        // Eligibility is exactly ⟨D-DERIVE⟩'s condition: uncommitted, and
        // every declared dependency already committed.
        let next = rules
            .iter()
            .find(|(name, _, deps)| {
                !committed.contains(*name) && deps.iter().all(|d| committed.contains(d))
            })
            .copied();

        let Some((name, body, deps)) = next else {
            let pending: Vec<String> = rules
                .iter()
                .filter(|(n, _, _)| !committed.contains(*n))
                .map(|(n, _, _)| (*n).to_string())
                .collect();
            return L3RunV2 {
                facts,
                stop: if pending.is_empty() {
                    RunStopV2::AllRulesCommitted
                } else {
                    RunStopV2::NoEligibleRule { pending }
                },
            };
        };

        match eval(body, &env) {
            Ok(value) => {
                facts.push(CommittedFactV2 {
                    ordinal: facts.len() as u64,
                    rule: name.to_string(),
                    value: value.clone(),
                    depends_on: deps.to_vec(),
                });
                env = env.with_fact(name.to_string(), value);
                committed.insert(name.to_string());
            }
            Err(fault) => {
                return L3RunV2 {
                    facts,
                    stop: RunStopV2::Faulted {
                        rule: name.to_string(),
                        fault,
                    },
                }
            }
        }
    }
}
