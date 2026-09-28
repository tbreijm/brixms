//! Evaluate `let`-lane bindings through the shared evaluator (ADR-0042).
//!
//! `brix check` type-checks a `let` module through the type-realization
//! regime ([`crate::check_module`]) and, until now, never computed a value.
//! This module gives the `let` lane a *value* for every binding whose
//! expression lies in the exact executable fragment — by lowering the
//! module's `config`s, `fn`s and `let` bindings through the same expression
//! lowering and evaluator the finite-decision lane uses
//! ([`crate::l3_v2::lower_expr_v2`], [`crate::l3_v2::eval`]) — without
//! touching type-realization checking or grades at all: this is a separate,
//! additive pass over the same source, run after [`crate::check_module`] and
//! never consulted by it.
//!
//! **Evaluation never changes a grade.** The grade printed alongside a value
//! always comes from [`crate::CheckResult::outcome`]; this module only adds
//! (or explains the absence of) the `= value` half of `name : Type @Grade =
//! value`.
//!
//! **Never a guess.** A binding whose expression (or any helper it calls)
//! uses a construct outside the fragment this evaluator covers — `Float`
//! arithmetic or `/`, witness composition (`then`/`and`), `prove`/`why`/
//! `audit`, a wildcard/catch-all match arm, or a nested constructor pattern —
//! is reported as [`LetEvalOutcome::NotEvaluated`] with a specific reason,
//! never as a fabricated value. The same holds if evaluation itself faults
//! (an unbound transitive dependency, a call-depth or work-budget fault):
//! these are runtime facts about the *shared* evaluator, reported the same
//! way a decision-lane fault would be, not swallowed.
//!
//! **Generic configs (ADR-0042).** A parameterized `config Stack<T> = …`
//! lowers exactly like a non-generic one: type parameters are erased for
//! evaluation (a value is a nominal constructor or record regardless of the
//! type arguments used to check it), so `variants_of`/`nullary` below are
//! built from arity alone, the same way [`crate::finite_decision::plan`]
//! already does for the finite-decision lane.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use brix_syntax::ast::{self, Item};

use crate::l3_v2::{
    division_not_admitted_reason, eval, lower_expr_v2, EvalEnv, EvalFault, L3FunctionDef,
    L3V2LowerError, L3ValueV2,
};

/// The outcome of attempting to evaluate one `let`/`witness` binding's value
/// through the shared evaluator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LetEvalOutcome {
    /// The binding's value lies in the exact executable fragment (and every
    /// helper it transitively calls does too), and evaluated cleanly.
    Value(L3ValueV2),
    /// The binding is outside the exact executable fragment, or evaluation
    /// itself faulted (including because it transitively depends on a
    /// binding or helper that is). Carries a short, specific reason —
    /// `brix check` prints it, never a value.
    NotEvaluated(String),
}

/// Evaluate every top-level `let`/`witness` binding in `m` through the shared
/// evaluator, one outcome per binding, **in declaration order** — the same
/// order and count as [`crate::check_module`]'s result vector, so a caller
/// zips the two by position to pair a binding's checked type/grade with its
/// evaluated value.
pub fn evaluate_let_module(m: &ast::Module) -> Vec<(String, LetEvalOutcome)> {
    // Arity-only config tables (ADR-0042 erasure): built exactly like
    // `finite_decision::plan`'s non-schema config pass, so a generic config
    // is handled identically to a concrete one — nothing here ever looks at
    // `config.params`.
    let mut variants_of: BTreeMap<String, String> = BTreeMap::new();
    let mut nullary: BTreeMap<String, String> = BTreeMap::new();
    for item in &m.items {
        if let Item::Config(c) = item {
            if let ast::ConfigBody::Sum(variants) = &c.body {
                for v in variants {
                    variants_of.insert(v.name.clone(), c.name.clone());
                    if v.params.is_empty() {
                        nullary.insert(v.name.clone(), c.name.clone());
                    }
                }
            }
        }
    }
    // `Bool`'s two nullary constructors are builtin, matching
    // `check_module`'s and the finite-decision lane's own seeding.
    for name in ["true", "false"] {
        nullary.insert(name.to_string(), "Bool".to_string());
    }

    // Every declared `fn`'s arity, built up front (before any body is
    // lowered) so a call to a function declared *later* — or one whose own
    // body fails to lower — still resolves structurally: an unevaluable
    // helper's *callers* fault at evaluation time (`Unbound`), not at
    // lowering time, which is what lets one bad helper's fragment-exclusion
    // propagate as "not evaluated" through everything that calls it, rather
    // than silently mis-lowering.
    let mut function_arities: BTreeMap<String, usize> = BTreeMap::new();
    let mut fn_items: Vec<&ast::Callable> = Vec::new();
    for item in &m.items {
        if let Item::Fn(f) = item {
            function_arities.insert(f.name.clone(), f.params.len());
            fn_items.push(f);
        }
    }

    // Helpers are pure and closed, exactly like the finite-decision lane's
    // (ADR-0032): a body sees only its own parameters, never an outer `let`.
    // A `fn` that does not fit this (or uses an outside-the-fragment
    // construct) simply is not entered into `functions` below, so any
    // binding that calls it faults to `Unbound` and is reported
    // `NotEvaluated` — never guessed.
    let mut functions: BTreeMap<String, L3FunctionDef> = BTreeMap::new();
    for f in &fn_items {
        let param_set: BTreeSet<String> = f.params.iter().map(|p| p.name.clone()).collect();
        let Ok(body) = lower_expr_v2(
            &f.body,
            &param_set,
            &param_set,
            &BTreeSet::new(),
            &nullary,
            &variants_of,
            &function_arities,
            false,
            true,
        ) else {
            continue;
        };
        functions.insert(
            f.name.clone(),
            L3FunctionDef {
                name: f.name.clone(),
                params: f.params.iter().map(|p| (p.name.clone(), None)).collect(),
                ret_contract: None,
                schemas: Arc::new(BTreeMap::new()),
                body,
            },
        );
    }
    let functions = Arc::new(functions);

    let mut let_names: BTreeSet<String> = BTreeSet::new();
    let mut env = EvalEnv::new().with_functions(functions.clone());
    let mut results = Vec::new();

    for item in &m.items {
        let normalized;
        let item = match item {
            Item::Witness { name, value } => {
                normalized = Item::Let(ast::LetDecl {
                    name: name.clone(),
                    ty: None,
                    value: value.clone(),
                });
                &normalized
            }
            other => other,
        };
        let Item::Let(let_decl) = item else { continue };

        let outcome = match lower_expr_v2(
            &let_decl.value,
            &let_names,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &nullary,
            &variants_of,
            &function_arities,
            false,
            true,
        ) {
            Err(e) => LetEvalOutcome::NotEvaluated(describe_lower_error(&e)),
            Ok(expr) => match eval(&expr, &env) {
                Ok(value) => LetEvalOutcome::Value(value),
                Err(fault) => LetEvalOutcome::NotEvaluated(describe_eval_fault(&fault)),
            },
        };

        // Only a genuinely evaluated value is bound forward — a binding that
        // was not evaluated must not let a later reference to it silently
        // resolve to nothing more specific than the same `Unbound` fault,
        // which is exactly what leaving it out of `env` produces.
        if let LetEvalOutcome::Value(v) = &outcome {
            env = env.with_let(let_decl.name.clone(), v.clone());
        }
        let_names.insert(let_decl.name.clone());
        results.push((let_decl.name.clone(), outcome));
    }

    results
}

/// Describe why an expression fell outside the fragment [`lower_expr_v2`]
/// covers, as the reason `brix check` prints instead of a value.
fn describe_lower_error(e: &L3V2LowerError) -> String {
    match e {
        L3V2LowerError::FloatLiteralNotAllowed(_) => {
            "a Float value is outside the exact executable fragment (Float is admitted only by \
             the type-realization checker, never by the shared evaluator)"
                .to_string()
        }
        L3V2LowerError::DivisionNotAllowed => division_not_admitted_reason().to_string(),
        L3V2LowerError::Unsupported(detail) if detail.contains("composition") => {
            "witness composition ('then'/'and') is outside the exact executable fragment"
                .to_string()
        }
        L3V2LowerError::Unsupported(feature) => {
            format!("'{feature}' is outside the exact executable fragment")
        }
        L3V2LowerError::DefaultArmNotAllowed => {
            "a wildcard or variable catch-all match arm is outside the exact executable fragment \
             (it type-checks, but the evaluator requires an explicit constructor pattern for \
             every reached arm)"
                .to_string()
        }
        L3V2LowerError::NestedPatternNotAllowed => {
            "a nested constructor pattern is outside the exact executable fragment".to_string()
        }
        L3V2LowerError::DuplicateMatchBinder(name) => {
            format!(
                "match pattern rebinds '{name}', which is outside the exact executable fragment"
            )
        }
        L3V2LowerError::IntegerOverflow(lit) => {
            format!("integer literal '{lit}' does not fit in the executable fragment's Int")
        }
        L3V2LowerError::UnresolvedReference(name) => {
            format!("depends on '{name}', which is outside the exact executable fragment")
        }
        L3V2LowerError::FunctionArityMismatch { func, .. } => {
            format!("calls '{func}' outside the exact executable fragment (arity mismatch)")
        }
        other => format!("outside the exact executable fragment ({other})"),
    }
}

/// Describe why evaluation itself faulted, as the reason `brix check` prints
/// instead of a value. Reachable when a binding's expression lowers cleanly
/// but transitively depends on something that did not (a helper that itself
/// fell outside the fragment), or when the shared evaluator's own resource
/// bounds are exceeded.
fn describe_eval_fault(f: &EvalFault) -> String {
    match f {
        EvalFault::Unbound(name) => {
            format!("depends on '{name}', which is outside the exact executable fragment")
        }
        EvalFault::CallDepthExceeded { .. } => {
            "exceeded the evaluator's recursion depth bound".to_string()
        }
        EvalFault::ResourceExhausted { .. } => "exceeded the evaluator's work budget".to_string(),
        other => format!("evaluation fault ({other})"),
    }
}
