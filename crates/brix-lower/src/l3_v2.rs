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
    /// Checked integer arithmetic. Division is deliberately absent — ADR-0027
    /// §5 defers it until quotient type, rounding, division-by-zero and
    /// `MIN / -1` are separately pinned.
    Arith(ArithOpV2, Box<L3ExprV2>, Box<L3ExprV2>),
    /// Comparison, yielding a boolean.
    Cmp(CmpOpV2, Box<L3ExprV2>, Box<L3ExprV2>),
    /// Case analysis. Exhaustiveness is required (ADR-0027 §5); a default arm
    /// silently swallowing malformed input is rejected at lowering.
    Match {
        scrutinee: Box<L3ExprV2>,
        arms: Vec<(L3PatternV2, L3ExprV2)>,
    },
}

/// The arithmetic operators v2 admits. **No division** — see [`L3ExprV2`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArithOpV2 {
    Add,
    Sub,
    Mul,
}

impl ArithOpV2 {
    /// Frozen ordinal — ABI.
    pub const fn ordinal(self) -> u64 {
        match self {
            ArithOpV2::Add => 0,
            ArithOpV2::Sub => 1,
            ArithOpV2::Mul => 2,
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
    /// A rule with parameters. Deferred to v3 with a stated reason: a
    /// parameter is a schema, and the AST supplies no quantification domain
    /// (ADR-0027 §7).
    ParameterizedRule(String),
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
    /// **Nor is there a `NonExhaustiveMatch` variant yet.** Exhaustiveness is
    /// required by ADR-0027 §5, and Stage A does not check it — the config
    /// bodies it carries are declaration shapes, not a resolved variant
    /// universe. Declaring the error before it can fire would claim a check
    /// that does not run; it arrives with the evaluator in Stage B.
    UnresolvedReference(String),
    /// A wildcard or binder arm at the top level of a `match`. Rejected so a
    /// default arm cannot silently swallow malformed input.
    DefaultArmNotAllowed,
    /// A nested constructor pattern. Not in Stage A.
    NestedPatternNotAllowed,
    /// Division, deferred until its fault semantics are pinned.
    DivisionNotAllowed,
    /// A float literal. v2 inherits v1's exclusion.
    FloatLiteralNotAllowed(String),
    /// An integer literal outside `i64`.
    IntegerOverflow(String),
    /// A surface form with no v2 lowering (`prove`, `why`, `audit`, `then`,
    /// `and`).
    Unsupported(String),
}

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
            ast::Item::Witness { name, .. } => {
                return Err(L3V2LowerError::ItemNotAllowed(format!("witness {name}")))
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
                    &rules_so_far,
                    &nullary,
                    &variants_of,
                    false,
                )?;
                lets.insert(l.name.clone());
                items.push(L3PlanItemV2::Let {
                    name: l.name.clone(),
                    value,
                });
            }
            ast::Item::Rule(r) => {
                if !r.params.is_empty() {
                    return Err(L3V2LowerError::ParameterizedRule(r.name.clone()));
                }
                let body =
                    lower_expr_v2(&r.body, &lets, &rules_so_far, &nullary, &variants_of, true)?;
                let mut depends_on = Vec::new();
                collect_rule_deps(&body, &mut depends_on);
                if depends_on.iter().any(|d| d == &r.name) {
                    return Err(L3V2LowerError::ForwardOrSelfDependency {
                        rule: r.name.clone(),
                        depends_on: r.name.clone(),
                    });
                }
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

/// The rules a body reads facts from, in first-mention order and deduplicated.
fn collect_rule_deps(e: &L3ExprV2, out: &mut Vec<String>) {
    match e {
        L3ExprV2::RuleFact(name) => {
            if !out.iter().any(|d| d == name) {
                out.push(name.clone());
            }
        }
        L3ExprV2::Int(_)
        | L3ExprV2::Str(_)
        | L3ExprV2::Bool(_)
        | L3ExprV2::LetRef(_)
        | L3ExprV2::NullaryVariant { .. } => {}
        L3ExprV2::Ctor { args, .. } => {
            for a in args {
                collect_rule_deps(a, out);
            }
        }
        L3ExprV2::Record { fields, .. } => {
            for (_, v) in fields {
                collect_rule_deps(v, out);
            }
        }
        L3ExprV2::Field(b, _) => collect_rule_deps(b, out),
        L3ExprV2::Arith(_, a, b) | L3ExprV2::Cmp(_, a, b) => {
            collect_rule_deps(a, out);
            collect_rule_deps(b, out);
        }
        L3ExprV2::Match { scrutinee, arms } => {
            collect_rule_deps(scrutinee, out);
            for (_, body) in arms {
                collect_rule_deps(body, out);
            }
        }
    }
}

/// Lower one surface expression into the v2 IR.
///
/// `in_rule` distinguishes a rule body — which may read earlier rules' facts —
/// from a `let` value, which may not: a `let` is a closed static binding, and
/// letting it depend on a committed fact would make plan construction depend
/// on run order.
fn lower_expr_v2(
    e: &ast::Expr,
    lets: &BTreeSet<String>,
    rules: &BTreeSet<String>,
    nullary: &BTreeMap<String, String>,
    variants_of: &BTreeMap<String, String>,
    in_rule: bool,
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
            if let Some(sum) = nullary.get(name) {
                return Ok(L3ExprV2::NullaryVariant {
                    nominal_sum: sum.clone(),
                    variant: name.clone(),
                });
            }
            if lets.contains(name) {
                return Ok(L3ExprV2::LetRef(name.clone()));
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
                rules,
                nullary,
                variants_of,
                in_rule,
            )?),
            field.clone(),
        )),
        ast::Expr::Record { config, fields } => {
            let mut out = Vec::new();
            for (name, value) in fields {
                out.push((
                    name.clone(),
                    lower_expr_v2(value, lets, rules, nullary, variants_of, in_rule)?,
                ));
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(L3ExprV2::Record {
                nominal_config: config.clone(),
                fields: out,
            })
        }
        ast::Expr::Call { func, args } => {
            let Some(sum) = variants_of.get(func).cloned() else {
                return Err(L3V2LowerError::UnresolvedReference(func.clone()));
            };
            let mut out = Vec::new();
            for a in args {
                out.push(lower_expr_v2(
                    a,
                    lets,
                    rules,
                    nullary,
                    variants_of,
                    in_rule,
                )?);
            }
            Ok(L3ExprV2::Ctor {
                nominal_sum: sum,
                variant: func.clone(),
                args: out,
            })
        }
        ast::Expr::Bin { op, lhs, rhs } => {
            let l = Box::new(lower_expr_v2(
                lhs,
                lets,
                rules,
                nullary,
                variants_of,
                in_rule,
            )?);
            let r = Box::new(lower_expr_v2(
                rhs,
                lets,
                rules,
                nullary,
                variants_of,
                in_rule,
            )?);
            match op {
                ast::BinOp::Add => Ok(L3ExprV2::Arith(ArithOpV2::Add, l, r)),
                ast::BinOp::Sub => Ok(L3ExprV2::Arith(ArithOpV2::Sub, l, r)),
                ast::BinOp::Mul => Ok(L3ExprV2::Arith(ArithOpV2::Mul, l, r)),
                ast::BinOp::Div => Err(L3V2LowerError::DivisionNotAllowed),
                ast::BinOp::Lt => Ok(L3ExprV2::Cmp(CmpOpV2::Lt, l, r)),
                ast::BinOp::Le => Ok(L3ExprV2::Cmp(CmpOpV2::Le, l, r)),
                ast::BinOp::Gt => Ok(L3ExprV2::Cmp(CmpOpV2::Gt, l, r)),
                ast::BinOp::Ge => Ok(L3ExprV2::Cmp(CmpOpV2::Ge, l, r)),
                ast::BinOp::Eq => Ok(L3ExprV2::Cmp(CmpOpV2::Eq, l, r)),
                ast::BinOp::Ne => Ok(L3ExprV2::Cmp(CmpOpV2::Ne, l, r)),
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
                rules,
                nullary,
                variants_of,
                in_rule,
            )?);
            let mut out = Vec::new();
            for arm in arms {
                let pat = match &arm.pattern {
                    ast::Pattern::Ctor { name, args } => {
                        let mut binders = Vec::new();
                        for a in args {
                            match a {
                                ast::Pattern::Wildcard => binders.push(None),
                                ast::Pattern::Var(x) => binders.push(Some(x.clone())),
                                ast::Pattern::Ctor { .. } => {
                                    return Err(L3V2LowerError::NestedPatternNotAllowed)
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
                        return Err(L3V2LowerError::DefaultArmNotAllowed)
                    }
                };
                out.push((
                    pat,
                    lower_expr_v2(&arm.body, lets, rules, nullary, variants_of, in_rule)?,
                ));
            }
            Ok(L3ExprV2::Match {
                scrutinee: s,
                arms: out,
            })
        }
        ast::Expr::Prove(_) => Err(L3V2LowerError::Unsupported("prove".to_string())),
        ast::Expr::Why(_) => Err(L3V2LowerError::Unsupported("why".to_string())),
        ast::Expr::Audit(_) => Err(L3V2LowerError::Unsupported("audit".to_string())),
    }
}
