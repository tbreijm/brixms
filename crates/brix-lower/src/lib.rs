//! `brix-lower` — Brix lowering (ADR-0010, L2): the bridge from the surface
//! AST ([`brix_syntax::ast`]) onto SOC realizations.
//!
//! **L2-first slice (this crate, initial):** lower the `{fn, let, Call, Num,
//! Var}` fragment of Brix onto [`soc_regimes::type_realization::Expr`]
//! (`Lam`/`App`/`Lit`/`Var`) and run it through the tree-elaboration path. The
//! kernel proves the *composition* theorem — given the primitive typing-rule
//! leaves, the derivation establishes `e : T` — so the honest status of the
//! typing result is **`Audited`** (see
//! [`soc_regimes::type_realization::honest_result_outcome`]); it upgrades to
//! `Proven` per-result once the leaf generators are discharged to tight (the
//! SOC tight-generator obligation). `fn` definitions are inlined into
//! `App(Lam, arg)`.
//!
//! Deliberately deferred (later L2 slices): `config`/record/`match`/`regime`/
//! `rule`; and the reconciliation of the two internal type reps
//! (`type_realization` for the Proven/positive path vs `soc_regimes::native`
//! for conflict detection/negative path) into one canonical checker.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

pub mod l3;
pub mod l3_audit;
pub mod l3_canon;
pub mod l3_regime;
pub mod l3_run;
pub use l3::{
    lower_l3_plan, L3ConfigBody, L3ConfigDecl, L3LowerError, L3PlanItem, L3PlanV1, L3TypeRef,
    L3ValueV1, PlanLimitsV1, L3_PROFILE_MARKER_RETIRED_V0, L3_PROFILE_MARKER_V1,
};
pub use l3_audit::{
    audit_l3_journal, audit_l3_run, check_l3_audit_receipt_from_source_v1, l3_generator_registry,
    l3_generator_semantics, SourceReceiptError,
};
pub use l3_canon::{
    build_pending, context_id, fact_id, l3_generator_id, l3_generator_preimage, l3_value_id,
    l3_value_preimage, l3_witness_id, policy_id, program_id, program_preimage, rule_id,
    rule_preimage, world_id, FactChainIdV1, FactV1, L3PolicyV1, L3ValueId, L3WorldV1, PendingIdV1,
    PresentationIdV1, ProgramIdV1, RuleId, RunContextV1, L3_FACT_CHAIN_FORMAT_V1,
    L3_FACT_CHAIN_MARKER, L3_GENERATOR_TAG, L3_PENDING_FORMAT_V1, L3_PENDING_MARKER,
    L3_PLAN_FORMAT_V1, L3_PLAN_MARKER, L3_RULE_TAG, L3_RUN_CONTEXT_FORMAT_V1, L3_VALUE_FORMAT_V1,
    L3_VALUE_MARKER, L3_WORLD_FORMAT_V1, L3_WORLD_MARKER,
};
pub use l3_regime::{
    build_l3_observation_profile, build_l3_transition_table, l3_adm, l3_policy, L3Regime,
    L3TransitionTable,
};
pub use l3_run::{
    commit_error_reason, frontier_conflict_reason, run_l3_plan, run_l3_plan_with_interner,
    settlement_run_id, AdapterFailureDetail, L3AdmChoice, L3RunReport, L3UnknownReasonV1,
    SettlementRunId, SettlementRunV1, SettlementStopV1,
};

use brix_elaborate::{elaborate_tree, ElaborationResult, RealizesTree};
use brix_kernel::{Budget, Verdict};
use brix_semantic::{ContextId, Dependency, EvidenceId, Outcome, PropositionId};
use brix_syntax::ast::{self, Item};
use soc_regimes::coverage::certify_exhaustive;
pub use soc_regimes::coverage::CoverageOutcome;
use soc_regimes::type_realization::{
    audited_type_check_tree, grade_assertion_satisfied, honest_result_outcome, infer_tree, zonk,
    ArithOp, CmpOp, Expr as TrExpr, Infer, Pattern as TrPattern, Ty as TrTy, TyCtx, TypeError,
};

/// Lowering context holding top-level functions, config declarations, and constructors.
#[derive(Clone, Copy, Debug)]
pub struct LowerCtx<'a> {
    pub fns: &'a BTreeMap<String, &'a ast::Callable>,
    pub configs: &'a BTreeMap<String, &'a ast::ConfigDecl>,
    pub ctors: &'a BTreeMap<String, (TrTy, String, Vec<TrTy>)>,
    /// Why a constructor is absent from `ctors`, when its declaring `config`
    /// was rejected. Consulted before reporting `Unresolved`, so the
    /// diagnostic names the broken declaration rather than the correct use.
    pub ctor_faults: &'a BTreeMap<String, LowerError>,
    /// The `fn`s currently being inlined, innermost last.
    ///
    /// `fn` bodies are inlined at each call site, so a function that calls
    /// itself inlines forever. This is the guard that turns that into a
    /// diagnostic instead of a stack overflow.
    pub inlining: &'a RefCell<Vec<String>>,
}

/// Errors surfaced while lowering a surface construct not yet supported by the
/// current L2 fragment (or an ill-formed reference or type/elaboration failure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LowerError {
    /// A surface construct outside the current L2 fragment.
    Unsupported(String),
    /// A reference (variable / function name) that could not be resolved.
    Unresolved(String),
    /// Type checking error from `soc_regimes`.
    TypeError(TypeError),
    /// `brix_elaborate::elaborate_tree` returned `NotElaborated`: the kernel
    /// did not accept the composition term. Carries the kernel's own
    /// [`Verdict`] rather than a formatted string (ADR-0010 L4, issue #43) so
    /// `brix prove`/`brix whynot` can distinguish, e.g., resource exhaustion
    /// from an outright rejection — **this is the absence of a proof, never
    /// evidence that the proposition is false** (ADR-0002 §5.3).
    ElaborationFailed(Verdict),
    /// `brix_elaborate::elaborate_tree` returned `Refused`: the source judgement
    /// this lowering pass built failed the ADR-0016 §6 authority-publication
    /// fence at the elaboration boundary — a caller-standing failure, not a
    /// kernel rejection (ADR-0016 §6, brix-elaborate's `ElaborationResult`
    /// doc). Not expected to occur on the settled path (`audited_type_check_tree`
    /// always publishes a self-consistent source), but the boundary must stay
    /// total rather than panic.
    ElaborationRefused(brix_semantic::PublicationError),
    /// A declared record field is missing from a record literal.
    MissingField { config: String, field: String },
    /// A record literal field is not present in the declared record config.
    UnknownField { config: String, field: String },
    /// A `@grade` assertion claims a stronger epistemic grade than the binding
    /// earned — an over-claim (epistemic erasure). `actual` may only weaken to
    /// `asserted`, never strengthen.
    GradeErasure { asserted: String, actual: String },
    /// A sum variant's parameter names a type that is neither a builtin nor a
    /// declared `config`.
    ///
    /// Reported against the **declaration**, not against a later use of the
    /// constructor. Before this existed, an unresolvable parameter silently
    /// dropped the whole sum, and the only symptom was `Unresolved` on every
    /// one of its constructors — an error pointing at correct code.
    UnknownVariantType {
        config: String,
        variant: String,
        ty: String,
    },
    /// A binding's declared type disagrees with the type it was inferred to
    /// have.
    ///
    /// Before this existed, `let x: Str = 42` reported `x : Int @Proven`: the
    /// annotation was carried, rendered, and never checked. A declared type
    /// that is not a contract is worse than no declaration, and awarding
    /// `@Proven` over one is worse still.
    TypeAnnotationMismatch { declared: String, inferred: String },
    /// A declared type names something that is neither a builtin nor a
    /// declared `config` — a typo, or a type that was never written.
    UnknownDeclaredType(String),
    /// A record literal's field value does not have the type the `config`
    /// declares for that field.
    ///
    /// Applies equally to a named-field sum variant, whose payload is a record
    /// by desugaring. Before this existed, `Item { name: 1, base: "oops" }`
    /// against `{ name: Str, base: Int }` checked as `@Proven` with the two
    /// field types silently swapped.
    RecordFieldTypeMismatch {
        config: String,
        field: String,
        declared: String,
        inferred: String,
    },
    /// A parameterized `config` used at the wrong number of type arguments,
    /// including a bare `List` where `List<Int>` was required.
    ConfigArityMismatch {
        config: String,
        expected: usize,
        found: usize,
    },
    /// A `fn` calls itself, directly or through other `fn`s.
    ///
    /// Not yet supported, and refused rather than attempted: `fn` bodies are
    /// **inlined** at each call site, so a recursive call inlines forever —
    /// this previously overflowed the stack.
    ///
    /// The obstruction is the lowering strategy, not the proof system. The
    /// standard treatment assumes the function's type, checks the body under
    /// that assumption, and discharges, so the recursive call is a hypothesis
    /// leaf and the derivation stays finite. `cycle` closes on its first
    /// element.
    RecursiveFunction {
        function: String,
        cycle: Vec<String>,
    },
    /// A `config` cycle that runs through **another** config — mutual
    /// recursion, such as `config A = MkA(B)` with `config B = MkB(A)`.
    ///
    /// Direct self-reference (`config List = Nil | Cons(Int, List)`) is
    /// supported and becomes a μ-type. Mutual recursion is not: expansion is
    /// by inlining, so `A` and `B` would each acquire a different μ-type
    /// depending on which was entered first, and the two representations do
    /// not unify. Refused by name rather than approximated — and rather than
    /// overflowing the stack, which is what it did before.
    ///
    /// `cycle` closes on its first element.
    MutuallyRecursiveConfig { config: String, cycle: Vec<String> },
}

impl From<TypeError> for LowerError {
    fn from(err: TypeError) -> Self {
        LowerError::TypeError(err)
    }
}

/// A category of "no proof yet" for `brix whynot` (ADR-0010 L4, issue #43).
/// ADR-0002 §5.3: "a search that has not terminated has proved nothing" —
/// none of these variants is, or may be rendered as, a refutation. Only an
/// actual `Evidence::KernelRefutation` (not produced anywhere in this
/// codebase yet) would license that word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofGap {
    /// The checker found a positive obstruction — a type mismatch, an
    /// unbound/unresolved reference, a missing/unknown record field, an
    /// asserted-grade erasure, a non-exhaustive match, or a source refused at
    /// the ADR-0016 §6 elaboration boundary. Still not a kernel refutation: no
    /// `Refuted` outcome exists for any of these today.
    ///
    /// A refused publication belongs here rather than under
    /// [`ProofGap::AbsenceOfProof`] because it is an *identified* obstruction —
    /// the `PublicationError` names exactly which check failed — not the
    /// result of a search that found nothing.
    Conflict(String),
    /// The construct lies outside the current lowering/execution fragment.
    /// This says nothing about the program's truth.
    UnsupportedFragment(String),
    /// The kernel search exhausted its budget before deciding
    /// (`Verdict::ResourceExhausted`). Establishes nothing (ADR-0002 §5.3).
    ExhaustedSearch(String),
    /// `ElaborationResult::NotElaborated` for a reason other than budget
    /// exhaustion (`Rejected`/`Malformed`/`Unsupported`/`ContextMismatch`):
    /// the absence of a proof, not evidence of falsehood.
    AbsenceOfProof(String),
}

/// Classify a [`LowerError`] into the `brix whynot` vocabulary above. Pure
/// and total: every variant of `LowerError` maps to exactly one `ProofGap`.
pub fn diagnose_gap(err: &LowerError) -> ProofGap {
    match err {
        LowerError::Unsupported(msg) => ProofGap::UnsupportedFragment(msg.clone()),
        LowerError::Unresolved(name) => {
            ProofGap::Conflict(format!("unresolved reference '{name}'"))
        }
        LowerError::TypeError(te) => ProofGap::Conflict(format!("{te:?}")),
        LowerError::MissingField { config, field } => ProofGap::Conflict(format!(
            "record literal for '{config}' is missing declared field '{field}'"
        )),
        LowerError::UnknownField { config, field } => ProofGap::Conflict(format!(
            "record literal for '{config}' has no declared field '{field}'"
        )),
        LowerError::GradeErasure { asserted, actual } => ProofGap::Conflict(format!(
            "asserted grade '@{asserted}' exceeds the earned grade '@{actual}' (epistemic erasure)"
        )),
        LowerError::ElaborationFailed(Verdict::ResourceExhausted(reason)) => {
            ProofGap::ExhaustedSearch(format!("{reason:?}"))
        }
        LowerError::ElaborationFailed(verdict) => ProofGap::AbsenceOfProof(format!("{verdict:?}")),
        LowerError::ElaborationRefused(err) => ProofGap::Conflict(format!(
            "elaboration boundary refused the source judgement: {err:?}"
        )),
        // A positive obstruction: the declaration names a type that does not
        // exist, which no amount of further search would resolve.
        LowerError::UnknownVariantType {
            config,
            variant,
            ty,
        } => ProofGap::Conflict(format!(
            "config '{config}' variant '{variant}' takes '{ty}', which is not a builtin \
             (Int/Str/Float) or a declared config"
        )),
        LowerError::TypeAnnotationMismatch { declared, inferred } => ProofGap::Conflict(format!(
            "declared type '{declared}' does not match the inferred type '{inferred}'"
        )),
        LowerError::RecordFieldTypeMismatch {
            config,
            field,
            declared,
            inferred,
        } => ProofGap::Conflict(format!(
            "field '{field}' of '{config}' is declared '{declared}' but its value is '{inferred}'"
        )),
        LowerError::UnknownDeclaredType(ty) => ProofGap::Conflict(format!(
            "declared type '{ty}' is not a builtin (Int/Str/Float) or a declared config"
        )),
        // Not an obstruction in the program — a fragment the language does not
        // cover yet. The distinction matters: `whynot` must not tell a user
        // their correct program is wrong.
        LowerError::ConfigArityMismatch {
            config,
            expected,
            found,
        } => ProofGap::Conflict(format!(
            "config '{config}' takes {expected} type argument(s), used with {found}"
        )),
        LowerError::RecursiveFunction { function, cycle } => {
            ProofGap::UnsupportedFragment(format!(
                "'{function}' is recursive ({}); `fn` bodies are inlined, so recursion needs \
                 fixpoint typing rather than inlining",
                cycle.join(" -> ")
            ))
        }
        LowerError::MutuallyRecursiveConfig { config, cycle } => {
            ProofGap::UnsupportedFragment(format!(
                "config '{config}' is mutually recursive ({}), which L2 does not support yet — \
                 direct self-reference is supported",
                cycle.join(" -> ")
            ))
        }
    }
}

/// Resolve a `config`-declared type name to its native [`TrTy`], inlining
/// referenced configs so a sum variant can carry another config as a parameter.
///
/// **Why inlining rather than a reference.** `soc_regimes::type_realization::Ty`
/// has no by-name constructor — a sum is `Sum(name, variants)` with its
/// variants inline — so a referenced config must be expanded in place. That is
/// sound because the expansion carries the referent's own name, keeping the
/// type nominal rather than structural; `Sum("Attribute", …)` is still
/// `Attribute` wherever it appears. It is also why a *recursive* config cannot
/// be expanded: the expansion would not terminate. That case is refused by
/// name below rather than approximated.
///
/// `stack` carries the configs currently being expanded, so a cycle is detected
/// at the point it closes rather than by depth-limiting.
fn resolve_config_ty(
    t: &ast::Ty,
    configs: &BTreeMap<String, &ast::ConfigDecl>,
    stack: &mut Vec<String>,
) -> Result<TrTy, ConfigTyError> {
    resolve_config_ty_in(t, configs, stack, &BTreeMap::new())
}

/// [`resolve_config_ty`] under a binding environment for the enclosing
/// declaration's type parameters.
///
/// Inside `config List<T> = …`, `T` resolves to `TrTy::Param("T")`; at a use
/// site `List<Int>`, it resolves to `Int`. That is the whole of what makes a
/// config parameterized — the declaration is a template with `Param`s, and a
/// use substitutes them.
fn resolve_config_ty_in(
    t: &ast::Ty,
    configs: &BTreeMap<String, &ast::ConfigDecl>,
    stack: &mut Vec<String>,
    bindings: &BTreeMap<String, TrTy>,
) -> Result<TrTy, ConfigTyError> {
    match t {
        ast::Ty::App(name, args) => {
            let mut resolved_args = Vec::new();
            for a in args {
                resolved_args.push(resolve_config_ty_in(a, configs, stack, bindings)?);
            }
            let Some(decl) = configs.get(name) else {
                return Err(ConfigTyError::Unknown(name.clone()));
            };
            if decl.params.len() != resolved_args.len() {
                return Err(ConfigTyError::Arity {
                    config: name.clone(),
                    expected: decl.params.len(),
                    found: resolved_args.len(),
                });
            }
            // The recursive occurrence inside the declaration. Its arguments
            // are the declaration's own parameters, which the enclosing `Rec`
            // already binds, so the bound variable carries them.
            if stack.last().is_some_and(|c| c == name) {
                return Ok(TrTy::RecVar(name.clone()));
            }
            if stack.iter().any(|c| c == name) {
                let mut cycle = stack.clone();
                cycle.push(name.clone());
                return Err(ConfigTyError::Mutual(cycle));
            }
            let inner: BTreeMap<String, TrTy> =
                decl.params.iter().cloned().zip(resolved_args).collect();
            stack.push(name.clone());
            let resolved = resolve_body(decl, configs, stack, &inner)?;
            stack.pop();
            if mentions_rec_var(&resolved, name) {
                return Ok(TrTy::Rec(name.clone(), Box::new(resolved)));
            }
            Ok(resolved)
        }
        ast::Ty::Graded(inner, _) => resolve_config_ty_in(inner, configs, stack, bindings),
        // Anonymous, so there is no name to cycle through; its fields are
        // resolved under the same stack so a record reaching back into an
        // enclosing config is still caught.
        ast::Ty::Record(decls) => {
            let mut out = Vec::new();
            for d in decls {
                out.push((
                    d.name.clone(),
                    resolve_config_ty_in(&d.ty, configs, stack, bindings)?,
                ));
            }
            Ok(TrTy::Record(out))
        }
        ast::Ty::Named(n) => {
            // A type parameter of the enclosing declaration shadows everything
            // else — inside `config Box<T>`, `T` is the parameter, not a config
            // that happens to be called `T`.
            if let Some(bound) = bindings.get(n) {
                return Ok(bound.clone());
            }
            match n.as_str() {
                "Int" => return Ok(TrTy::Con("Int")),
                "Str" => return Ok(TrTy::Con("Str")),
                "Float" => return Ok(TrTy::Con("Float")),
                _ => {}
            }
            let Some(decl) = configs.get(n) else {
                return Err(ConfigTyError::Unknown(n.clone()));
            };
            if !decl.params.is_empty() {
                return Err(ConfigTyError::Arity {
                    config: n.clone(),
                    expected: decl.params.len(),
                    found: 0,
                });
            }
            // A back-reference to the config currently being expanded — a
            // DIRECT self-reference. Emitted as the bound variable of the
            // enclosing `Rec`, which is what makes
            // `config List = Nil | Cons(Int, List)` a type rather than a
            // non-terminating expansion.
            if stack.last().is_some_and(|c| c == n) {
                return Ok(TrTy::RecVar(n.clone()));
            }
            // A cycle that runs through *another* config — mutual recursion.
            // Inlining cannot express it: `A` and `B` would each expand to a
            // different μ-type depending on which was entered first, and the
            // two representations do not unify. Refused by name.
            //
            // This previously overflowed the stack, which is the one outcome
            // worse than refusing: a crash establishes nothing and takes the
            // process with it.
            if stack.iter().any(|c| c == n) {
                let mut cycle = stack.clone();
                cycle.push(n.clone());
                return Err(ConfigTyError::Mutual(cycle));
            }
            stack.push(n.clone());
            let resolved = resolve_body(decl, configs, stack, &BTreeMap::new())?;
            stack.pop();
            // Only bind a `Rec` when the body actually refers back — otherwise
            // every config would acquire a vacuous binder and two structurally
            // identical types would stop being equal.
            if mentions_rec_var(&resolved, n) {
                return Ok(TrTy::Rec(n.clone(), Box::new(resolved)));
            }
            Ok(resolved)
        }
    }
}

/// Resolve a declaration's body under a parameter binding.
fn resolve_body(
    decl: &ast::ConfigDecl,
    configs: &BTreeMap<String, &ast::ConfigDecl>,
    stack: &mut Vec<String>,
    bindings: &BTreeMap<String, TrTy>,
) -> Result<TrTy, ConfigTyError> {
    Ok(match &decl.body {
        ast::ConfigBody::Sum(variants) => {
            let mut out = Vec::new();
            for v in variants {
                let mut tys = Vec::new();
                for p in &v.params {
                    tys.push(resolve_config_ty_in(p, configs, stack, bindings)?);
                }
                out.push((v.name.clone(), tys));
            }
            TrTy::Sum(decl.name.clone(), out)
        }
        ast::ConfigBody::Record(decls) => {
            let mut out = Vec::new();
            for d in decls {
                out.push((
                    d.name.clone(),
                    resolve_config_ty_in(&d.ty, configs, stack, bindings)?,
                ));
            }
            TrTy::Record(out)
        }
    })
}

/// Whether `ty` contains a free occurrence of `var`, stopping at a shadowing
/// binder of the same name.
fn mentions_rec_var(ty: &TrTy, var: &str) -> bool {
    match ty {
        TrTy::RecVar(v) => v == var,
        TrTy::Con(_) | TrTy::Var(_) | TrTy::Param(_) => false,
        TrTy::Fn(a, b) => mentions_rec_var(a, var) || mentions_rec_var(b, var),
        TrTy::Record(fields) => fields.iter().any(|(_, t)| mentions_rec_var(t, var)),
        TrTy::Sum(_, variants) => variants
            .iter()
            .any(|(_, fs)| fs.iter().any(|f| mentions_rec_var(f, var))),
        TrTy::Rec(v, _) if v == var => false,
        TrTy::Rec(_, body) => mentions_rec_var(body, var),
    }
}

/// Why a variant parameter type could not be resolved. Internal to the sum
/// pass; converted to a [`LowerError`] that names the *declaration* at fault.
enum ConfigTyError {
    /// The name is neither a builtin nor a declared config.
    Unknown(String),
    /// A parameterized config used at the wrong number of type arguments —
    /// including a bare `List` where `List<Int>` was required.
    Arity {
        config: String,
        expected: usize,
        found: usize,
    },
    /// A cycle running through another config — mutual recursion. Carries the
    /// chain, closing on its first element.
    ///
    /// There is deliberately no `Recursive` variant: a *direct* self-reference
    /// is not an error, it resolves to the bound variable of a μ-type.
    Mutual(Vec<String>),
}

fn lower_pattern(p: &ast::Pattern) -> Result<TrPattern, LowerError> {
    match p {
        ast::Pattern::Wildcard => Ok(TrPattern::Wildcard),
        ast::Pattern::Var(x) => Ok(TrPattern::Var(x.clone())),
        ast::Pattern::Ctor { name, args } => {
            let sub = args
                .iter()
                .map(lower_pattern)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(TrPattern::Ctor(name.clone(), sub))
        }
    }
}

/// Lower a surface AST expression into a native [`soc_regimes::type_realization::Expr`].
pub fn lower_expr(e: &ast::Expr, ctx: LowerCtx) -> Result<TrExpr, LowerError> {
    match e {
        ast::Expr::Num(s) => {
            if let Ok(n) = s.parse::<i64>() {
                Ok(TrExpr::Lit(n))
            } else if s.parse::<f64>().is_ok() {
                // A well-formed decimal literal that is not an integer → Float.
                Ok(TrExpr::FloatLit(s.clone()))
            } else {
                Err(LowerError::Unsupported(format!(
                    "unrecognized numeric literal '{s}'"
                )))
            }
        }
        ast::Expr::Var(name) => {
            if let Some((sum_ty, variant, field_tys)) = ctx.ctors.get(name) {
                if field_tys.is_empty() {
                    return Ok(TrExpr::Ctor(sum_ty.clone(), variant.clone(), vec![]));
                }
            }
            // A nullary constructor whose `config` was rejected would otherwise
            // fall through to `Var` and surface as `Unbound` — an error about
            // the use rather than the declaration. `ctor_faults` only ever
            // holds names declared as variants, so this cannot capture an
            // ordinary variable.
            if let Some(fault) = ctx.ctor_faults.get(name) {
                return Err(fault.clone());
            }
            Ok(TrExpr::Var(name.clone()))
        }
        ast::Expr::Call { func, args } => {
            if let Some(c) = ctx.fns.get(func) {
                // A call back into a `fn` already being inlined. Refused by
                // name: inlining cannot express it, and the derivation it
                // would build is genuinely infinite.
                //
                // The proof system is not the obstruction. The standard
                // treatment assumes the function's type, checks the body under
                // that assumption, and discharges — so the recursive call is a
                // hypothesis leaf and the tree stays finite, exactly as
                // `g_lam_intro`/`g_lam_close` already do for lambdas. That is
                // the fix; this guard is what stops the crash until it lands.
                if ctx.inlining.borrow().iter().any(|f| f == func) {
                    let mut cycle = ctx.inlining.borrow().clone();
                    cycle.push(func.clone());
                    return Err(LowerError::RecursiveFunction {
                        function: func.clone(),
                        cycle,
                    });
                }
                if c.params.len() != args.len() {
                    return Err(LowerError::Unsupported(format!(
                        "arity mismatch for function '{func}': expected {}, got {}",
                        c.params.len(),
                        args.len()
                    )));
                }
                ctx.inlining.borrow_mut().push(func.clone());
                let lowered = lower_fn(c, ctx);
                ctx.inlining.borrow_mut().pop();
                let mut acc = lowered?;
                for arg in args {
                    let lowered_arg = lower_expr(arg, ctx)?;
                    acc = TrExpr::App(Box::new(acc), Box::new(lowered_arg));
                }
                Ok(acc)
            } else if let Some((sum_ty, variant, field_tys)) = ctx.ctors.get(func) {
                if args.len() != field_tys.len() {
                    return Err(LowerError::Unsupported(format!(
                        "constructor '{func}' expects {} args, got {}",
                        field_tys.len(),
                        args.len()
                    )));
                }
                let lowered_args = args
                    .iter()
                    .map(|arg| lower_expr(arg, ctx))
                    .collect::<Result<Vec<_>, LowerError>>()?;
                Ok(TrExpr::Ctor(sum_ty.clone(), variant.clone(), lowered_args))
            } else if let Some(fault) = ctx.ctor_faults.get(func) {
                Err(fault.clone())
            } else {
                Err(LowerError::Unresolved(func.clone()))
            }
        }
        ast::Expr::Str(s) => Ok(TrExpr::StrLit(s.clone())),
        ast::Expr::Record { config, fields } => {
            // `MonsterData { frame: Effect }` — a named-field variant, whose
            // declaration desugared to one record-typed parameter. Construction
            // has to desugar the same way or the two halves disagree.
            if let Some((sum_ty, variant, field_tys)) = ctx.ctors.get(config) {
                if let [TrTy::Record(decls)] = field_tys.as_slice() {
                    for (name, _) in decls {
                        if !fields.iter().any(|(f, _)| f == name) {
                            return Err(LowerError::MissingField {
                                config: config.clone(),
                                field: name.clone(),
                            });
                        }
                    }
                    for (name, _) in fields {
                        if !decls.iter().any(|(d, _)| d == name) {
                            return Err(LowerError::UnknownField {
                                config: config.clone(),
                                field: name.clone(),
                            });
                        }
                    }
                    let payload = fields
                        .iter()
                        .map(|(name, e)| Ok((name.clone(), lower_expr(e, ctx)?)))
                        .collect::<Result<Vec<_>, LowerError>>()?;
                    return Ok(TrExpr::Ctor(
                        sum_ty.clone(),
                        variant.clone(),
                        vec![TrExpr::Record(payload)],
                    ));
                }
            }
            if let Some(fault) = ctx.ctor_faults.get(config) {
                return Err(fault.clone());
            }
            if let Some(body) = ctx.configs.get(config) {
                match &body.body {
                    ast::ConfigBody::Sum(_) => {
                        return Err(LowerError::Unsupported(format!(
                            "'{config}' is a sum config, not a record"
                        )));
                    }
                    ast::ConfigBody::Record(decls) => {
                        // The declared field types must at least name something
                        // real. `config Item = { base: Money }` previously
                        // accepted an undeclared `Money` and inferred the field
                        // from the value instead.
                        for decl in decls {
                            let mut stack = vec![config.clone()];
                            match resolve_config_ty(&decl.ty, ctx.configs, &mut stack) {
                                Ok(_) => {}
                                Err(ConfigTyError::Unknown(name)) => {
                                    return Err(LowerError::UnknownDeclaredType(name));
                                }
                                Err(ConfigTyError::Arity {
                                    config,
                                    expected,
                                    found,
                                }) => {
                                    return Err(LowerError::ConfigArityMismatch {
                                        config,
                                        expected,
                                        found,
                                    })
                                }
                                Err(ConfigTyError::Mutual(cycle)) => {
                                    return Err(LowerError::MutuallyRecursiveConfig {
                                        config: config.clone(),
                                        cycle,
                                    });
                                }
                            }
                        }
                        for decl in decls {
                            if !fields.iter().any(|(name, _)| name == &decl.name) {
                                return Err(LowerError::MissingField {
                                    config: config.clone(),
                                    field: decl.name.clone(),
                                });
                            }
                        }
                        for (name, _) in fields {
                            if !decls.iter().any(|decl| &decl.name == name) {
                                return Err(LowerError::UnknownField {
                                    config: config.clone(),
                                    field: name.clone(),
                                });
                            }
                        }
                    }
                }
            }
            let lowered_fields: Result<Vec<(String, TrExpr)>, LowerError> = fields
                .iter()
                .map(|(name, e)| Ok((name.clone(), lower_expr(e, ctx)?)))
                .collect();
            Ok(TrExpr::Record(lowered_fields?))
        }
        ast::Expr::Field(base, name) => Ok(TrExpr::Field(
            Box::new(lower_expr(base, ctx)?),
            name.clone(),
        )),
        ast::Expr::Bin { op, lhs, rhs } if op.is_comparison() => {
            let cmp_op = match op {
                ast::BinOp::Lt => CmpOp::Lt,
                ast::BinOp::Le => CmpOp::Le,
                ast::BinOp::Gt => CmpOp::Gt,
                ast::BinOp::Ge => CmpOp::Ge,
                ast::BinOp::Eq => CmpOp::Eq,
                ast::BinOp::Ne => CmpOp::Ne,
                // Unreachable under the guard; kept total rather than
                // panicking on a future BinOp.
                other => {
                    return Err(LowerError::Unsupported(format!(
                        "'{other:?}' is not a comparison"
                    )))
                }
            };
            Ok(TrExpr::Cmp(
                cmp_op,
                Box::new(lower_expr(lhs, ctx)?),
                Box::new(lower_expr(rhs, ctx)?),
            ))
        }
        ast::Expr::Bool(b) => Ok(TrExpr::BoolLit(*b)),
        ast::Expr::Bin { op, lhs, rhs } => {
            let arith_op = match op {
                ast::BinOp::Add => ArithOp::Add,
                ast::BinOp::Sub => ArithOp::Sub,
                ast::BinOp::Mul => ArithOp::Mul,
                ast::BinOp::Div => ArithOp::Div,
                // `then`/`and` are witness composition, not numeric arithmetic —
                // deferred to the L4 witness/proof surface.
                ast::BinOp::Then | ast::BinOp::And => {
                    return Err(LowerError::Unsupported(
                        "witness composition ('then'/'and') not in L2-first fragment".to_string(),
                    ))
                }
                // Handled by the guarded arm above.
                other => {
                    return Err(LowerError::Unsupported(format!(
                        "unexpected comparison operator '{other:?}' in arithmetic position"
                    )))
                }
            };
            Ok(TrExpr::Arith(
                arith_op,
                Box::new(lower_expr(lhs, ctx)?),
                Box::new(lower_expr(rhs, ctx)?),
            ))
        }
        ast::Expr::Match {
            scrutinee,
            arms,
            proving_exhaustive: _,
        } => {
            let scrutinee_tr = lower_expr(scrutinee, ctx)?;
            let arms_tr = arms
                .iter()
                .map(|arm| {
                    let pat_tr = lower_pattern(&arm.pattern)?;
                    let body_tr = lower_expr(&arm.body, ctx)?;
                    Ok((pat_tr, body_tr))
                })
                .collect::<Result<Vec<_>, LowerError>>()?;
            Ok(TrExpr::Match(Box::new(scrutinee_tr), arms_tr))
        }
        ast::Expr::Prove(..) => Err(LowerError::Unsupported(
            "Prove not in L2-first fragment".to_string(),
        )),
        ast::Expr::Why(..) => Err(LowerError::Unsupported(
            "Why not in L2-first fragment".to_string(),
        )),
        ast::Expr::Audit(..) => Err(LowerError::Unsupported(
            "Audit not in L2-first fragment".to_string(),
        )),
    }
}

/// Lower a function definition (`ast::Callable`) to a curried [`soc_regimes::type_realization::Expr::Lam`].
pub fn lower_fn(c: &ast::Callable, ctx: LowerCtx) -> Result<TrExpr, LowerError> {
    let body_tr = lower_expr(&c.body, ctx)?;
    Ok(c.params.iter().rfold(body_tr, |acc, param| {
        TrExpr::Lam(param.name.clone(), Box::new(acc))
    }))
}

/// The outcome of lowering and checking a `let` binding in a Brix module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    /// The name of the `let` binding.
    pub name: String,
    /// The final elaboration outcome (e.g. `Proven`).
    pub outcome: Outcome,
    /// The inferred type of the binding value.
    pub ty: Option<TrTy>,
    /// For a top-level `match … proving exhaustive` value: the kernel-certified
    /// coverage outcome (a separate proposition from the typing result).
    pub coverage: Option<CoverageOutcome>,
    /// L4 (ADR-0010; issue #43) — additive fields surfacing what
    /// `elaborate_tree` already computes so `brix prove`/`brix why` can
    /// report it instead of discarding it. **Not necessarily "`name : ty`"
    /// itself**: `elaborate_tree` always proves the *composition* theorem
    /// "leaves ⇒ name : ty" (this field), and the unconditional result only
    /// inherits that grade when every leaf generator is independently tight
    /// — i.e. exactly when `outcome == Outcome::Proven`
    /// (`soc_regimes::type_realization::honest_result_outcome`). A caller
    /// that prints `certificate`/`proposition` as proof of `name : ty` when
    /// `outcome != Outcome::Proven` would be rounding `Audited` up to
    /// `Proven` — the one thing ADR-0012 §9 forbids.
    ///
    /// The exact proposition the kernel certified.
    pub proposition: PropositionId,
    /// The context the composition was proved in.
    pub context: ContextId,
    /// The kernel-certificate evidence identity backing `proposition` in
    /// `context`. Always present whenever this `CheckResult` is `Ok` — a
    /// composition certificate exists regardless of whether `outcome` itself
    /// reached `Proven`.
    pub certificate: EvidenceId,
    /// The elaboration-boundary edge (ADR-0001 §5.5) from the
    /// composition-certified judgement back to the settlement-side `Audited`
    /// judgement it elaborates — provenance, kept structurally separate from
    /// `certificate` (proof) per ADR-0002 §4.1/§5.
    pub elaboration_edge: Dependency,
    /// The tree-structured realization derivation (ADR-0007) whose leaves are
    /// the primitive typing-rule generators — provenance, not proof. `brix
    /// why` walks this to name which leaf(s), if any, capped the result below
    /// `Proven` (`soc_regimes::type_realization::generator_is_tight`, queried
    /// against `ClaimKind::Typing` — this is a typing derivation, ADR-0015
    /// ⟨D-JUDGE⟩).
    pub derivation: RealizesTree,
}

/// Lower each `let` binding in a parsed surface AST module to native SOC expressions,
/// type-check them, and elaborate them to `Proven HasType`.
/// The kernel-certified coverage outcome for a top-level
/// `match … proving exhaustive` value, or `None` if the value is not a
/// proving-exhaustive match at the top level.
fn coverage_for(value: &ast::Expr, tr_expr: &TrExpr, ctx: LowerCtx) -> Option<CoverageOutcome> {
    match value {
        ast::Expr::Match {
            proving_exhaustive: true,
            ..
        } => {}
        _ => return None,
    }
    let TrExpr::Match(_scrutinee, arms) = tr_expr else {
        return None;
    };
    // Resolve the scrutinee sum type from the first constructor pattern.
    let sum_ty = arms.iter().find_map(|(p, _)| match p {
        TrPattern::Ctor(vname, _) => ctx.ctors.get(vname).map(|(t, _, _)| t.clone()),
        _ => None,
    });
    Some(match sum_ty {
        Some(sum_ty) => {
            certify_exhaustive(&sum_ty, arms, ContextId::root(), Budget::new(4000, 4000))
        }
        None => CoverageOutcome::Unknown(
            "could not resolve the scrutinee sum for `proving exhaustive`".into(),
        ),
    })
}

/// Whether an inferred type satisfies a declared one.
///
/// **Deliberately permissive about unresolved variables.** An inferred type
/// still containing a `Ty::Var` has not been pinned down by inference, so
/// nothing has been established to contradict; reporting a mismatch there
/// would reject correct polymorphic code. Every *concrete* disagreement is
/// reported.
///
/// Records compare by field **name**, not position: the inferred record type
/// arrives in canonical (sorted) order while a declaration is written in
/// whatever order reads best, and those are the same type.
fn types_agree(declared: &TrTy, inferred: &TrTy) -> bool {
    // Delegated to unification rather than re-implemented.
    //
    // A hand-written comparison drifted from `unify` the moment parameterized
    // configs arrived: it compared recursive types by NAME, so a declared
    // `List<Str>` agreed with an inferred `List<Int>` and the contract passed
    // silently. `unify` already decides this correctly — equi-recursively,
    // with an assumption set — and it is permissive about unresolved
    // variables by construction, which is the other property this needs.
    soc_regimes::type_realization::unify(declared, inferred, &BTreeMap::new()).is_ok()
}

/// Render a type for a diagnostic. `Debug` leaks the internal representation
/// into user-facing output, so the surface spelling is reconstructed here.
fn render_ty(t: &TrTy) -> String {
    render_ty_at(t, true)
}

/// Render a type. `unfold_rec` shows a recursive type's payloads one level
/// deep, which is what distinguishes `List<Int>` from `List<Str>` — both are
/// bound as `List`, so rendering the name alone produces the useless
/// "declared 'List' but inferred 'List'".
fn render_ty_at(t: &TrTy, unfold_rec: bool) -> String {
    match t {
        TrTy::Con(c) => (*c).to_string(),
        TrTy::Var(v) => format!("?{v}"),
        TrTy::Param(name) => name.clone(),
        TrTy::Fn(a, b) => format!("{} -> {}", render_ty_at(a, false), render_ty_at(b, false)),
        TrTy::RecVar(name) => name.clone(),
        TrTy::Rec(name, _) if !unfold_rec => name.clone(),
        TrTy::Rec(name, _) => {
            let TrTy::Sum(_, variants) = t.unfold() else {
                return name.clone();
            };
            let payloads: Vec<String> = variants
                .iter()
                .filter(|(_, fs)| !fs.is_empty())
                .map(|(v, fs)| {
                    let inner: Vec<String> = fs.iter().map(|f| render_ty_at(f, false)).collect();
                    format!("{v}({})", inner.join(", "))
                })
                .collect();
            if payloads.is_empty() {
                name.clone()
            } else {
                format!("{name}[{}]", payloads.join(" | "))
            }
        }
        TrTy::Sum(name, _) => name.clone(),
        TrTy::Record(fields) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{n}: {}", render_ty_at(t, false)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Check every record literal in `e` against the field types its `config`
/// declares.
///
/// Surface-directed rather than folded into inference: a record literal lowers
/// to a structural `TrExpr::Record` that no longer knows which `config` it was
/// written against, so the declared type has to be consulted while the surface
/// form is still in hand. Each field value is inferred in isolation under the
/// bindings in scope, which is exact for the field's own type.
///
/// Named-field sum variants are covered too — their payload is a record by
/// desugaring, so the same declaration is checked through `ctors`.
fn check_declared_field_types(
    e: &ast::Expr,
    ctx: LowerCtx,
    ty_ctx: &TyCtx,
) -> Result<(), LowerError> {
    match e {
        ast::Expr::Record { config, fields } => {
            for (_, value) in fields {
                check_declared_field_types(value, ctx, ty_ctx)?;
            }

            // The declared field types, from either a record config or the
            // record payload of a named-field variant.
            let declared: Vec<(String, TrTy)> = if let Some(ast::ConfigBody::Record(decls)) =
                ctx.configs.get(config).map(|d| &d.body)
            {
                let mut out = Vec::new();
                for d in decls {
                    let mut stack = vec![config.clone()];
                    match resolve_config_ty(&d.ty, ctx.configs, &mut stack) {
                        Ok(ty) => out.push((d.name.clone(), ty)),
                        // Unresolvable declarations are reported by the paths
                        // that own them; nothing to compare against here.
                        Err(_) => return Ok(()),
                    }
                }
                out
            } else if let Some((_, _, field_tys)) = ctx.ctors.get(config) {
                match field_tys.as_slice() {
                    [TrTy::Record(decls)] => decls.clone(),
                    _ => return Ok(()),
                }
            } else {
                return Ok(());
            };

            for (name, declared_ty) in &declared {
                let Some((_, value)) = fields.iter().find(|(f, _)| f == name) else {
                    continue; // a missing field is `MissingField`, not this.
                };
                let tr = lower_expr(value, ctx)?;
                let (ty, _tree, st) = infer_tree(&tr, ty_ctx, Infer::new())?;
                let inferred = zonk(&ty, &st.subst);
                if !types_agree(declared_ty, &inferred) {
                    return Err(LowerError::RecordFieldTypeMismatch {
                        config: config.clone(),
                        field: name.clone(),
                        declared: render_ty(declared_ty),
                        inferred: render_ty(&inferred),
                    });
                }
            }
            Ok(())
        }
        ast::Expr::Call { args, .. } => {
            for a in args {
                check_declared_field_types(a, ctx, ty_ctx)?;
            }
            Ok(())
        }
        ast::Expr::Field(base, _) => check_declared_field_types(base, ctx, ty_ctx),
        ast::Expr::Bin { lhs, rhs, .. } => {
            check_declared_field_types(lhs, ctx, ty_ctx)?;
            check_declared_field_types(rhs, ctx, ty_ctx)
        }
        ast::Expr::Match {
            scrutinee, arms, ..
        } => {
            check_declared_field_types(scrutinee, ctx, ty_ctx)?;
            for arm in arms {
                check_declared_field_types(&arm.body, ctx, ty_ctx)?;
            }
            Ok(())
        }
        ast::Expr::Prove(inner) | ast::Expr::Why(inner) | ast::Expr::Audit(inner) => {
            check_declared_field_types(inner, ctx, ty_ctx)
        }
        ast::Expr::Num(_) | ast::Expr::Str(_) | ast::Expr::Bool(_) | ast::Expr::Var(_) => Ok(()),
    }
}

/// The grade a `let` annotation asserts (the outer grade of a `Graded` type),
/// as a GRADE-lattice node name, or `None` if the binding is unannotated /
/// annotated without a grade.
fn asserted_grade(ty: &Option<ast::Ty>) -> Option<&'static str> {
    match ty {
        Some(ast::Ty::Graded(_, g)) => Some(match g {
            ast::Grade::Derived => "Derived",
            ast::Grade::Audited => "Audited",
            ast::Grade::Proven => "Proven",
        }),
        _ => None,
    }
}

/// The GRADE-lattice node name for an actual outcome (non-grade outcomes map to
/// a sentinel that satisfies no assertion).
fn outcome_grade_name(o: Outcome) -> &'static str {
    match o {
        Outcome::Proven => "Proven",
        Outcome::Audited => "Audited",
        Outcome::Derived => "Derived",
        _ => "Unknown",
    }
}

pub fn check_module(m: &ast::Module) -> Vec<Result<CheckResult, (String, LowerError)>> {
    let mut fns = BTreeMap::new();
    let mut configs = BTreeMap::new();
    for item in &m.items {
        match item {
            Item::Fn(c) => {
                fns.insert(c.name.clone(), c);
            }
            Item::Config(c) => {
                configs.insert(c.name.clone(), c);
            }
            _ => {}
        }
    }

    let mut sums = BTreeMap::new();
    let mut ctors = BTreeMap::new();
    let mut ambiguous_ctors = BTreeSet::new();

    // Why a sum was dropped, keyed by each of its constructor names, so a later
    // use reports the declaration's real fault instead of `Unresolved`.
    let mut ctor_faults: BTreeMap<String, LowerError> = BTreeMap::new();
    let inlining: RefCell<Vec<String>> = RefCell::new(Vec::new());

    for item in &m.items {
        if let Item::Config(c) = item {
            if let ast::ConfigBody::Sum(variants) = &c.body {
                // A parameterized declaration is a TEMPLATE: its own type
                // parameters resolve to `Param`, which every use site later
                // instantiates to fresh variables. For an ordinary config the
                // binding map is empty and this is the previous behaviour.
                let bindings: BTreeMap<String, TrTy> = c
                    .params
                    .iter()
                    .map(|p| (p.clone(), TrTy::Param(p.clone())))
                    .collect();
                let mut stack = vec![c.name.clone()];
                let fault = match resolve_body(c, &configs, &mut stack, &bindings) {
                    Ok(body) => {
                        // Bind the recursion at the declaration. A variant
                        // param resolved to `RecVar(name)`; without this
                        // binder that occurrence would be free, and unifying
                        // it against the sum would mismatch.
                        let sum_ty = if mentions_rec_var(&body, &c.name) {
                            TrTy::Rec(c.name.clone(), Box::new(body))
                        } else {
                            body
                        };
                        sums.insert(c.name.clone(), sum_ty);
                        None
                    }
                    Err(ConfigTyError::Unknown(ty)) => Some(LowerError::UnknownVariantType {
                        config: c.name.clone(),
                        variant: variants.first().map(|v| v.name.clone()).unwrap_or_default(),
                        ty,
                    }),
                    Err(ConfigTyError::Arity {
                        config,
                        expected,
                        found,
                    }) => Some(LowerError::ConfigArityMismatch {
                        config,
                        expected,
                        found,
                    }),
                    Err(ConfigTyError::Mutual(cycle)) => {
                        Some(LowerError::MutuallyRecursiveConfig {
                            config: c.name.clone(),
                            cycle,
                        })
                    }
                };
                if let Some(err) = fault {
                    // The sum is unusable, but every constructor it declared
                    // now carries the reason rather than going silent.
                    for v in variants {
                        ctor_faults.entry(v.name.clone()).or_insert(err.clone());
                    }
                }
            }
        }
    }

    // `Bool`'s two nullary constructors are builtin, so a boolean `match`
    // resolves its scrutinee sum through the same path a declared config does
    // — which is what lets `proving exhaustive` certify it. Seeded first so a
    // (currently unconstructable) user variant of the same name would collide
    // rather than silently shadow: `true`/`false` are keywords, so
    // `parse_variant` cannot accept them as variant names today.
    for name in ["true", "false"] {
        ctors.insert(
            name.to_string(),
            (
                soc_regimes::type_realization::bool_ty(),
                name.to_string(),
                vec![],
            ),
        );
    }

    for sum_ty in sums.values() {
        // Unfolded so a recursive declaration's constructors are registered
        // with the *unfolded* payload types — `Succ`'s parameter must be the
        // whole `Nat`, not a free bound variable.
        let unfolded = sum_ty.unfold();
        if let TrTy::Sum(_, variants) = &unfolded {
            for (vname, field_tys) in variants {
                if ambiguous_ctors.contains(vname) {
                    continue;
                }
                if ctors.contains_key(vname) {
                    ctors.remove(vname);
                    ambiguous_ctors.insert(vname.clone());
                } else {
                    // The constructed value's type is the FOLDED type — `Nat`,
                    // not its one-step unfolding — while the payload types
                    // come from the unfolded body. Registering the unfolded
                    // form as the value's type would leave a free `RecVar` in
                    // it and mismatch against every other mention of `Nat`.
                    ctors.insert(
                        vname.clone(),
                        (sum_ty.clone(), vname.clone(), field_tys.clone()),
                    );
                }
            }
        }
    }

    let ctx = LowerCtx {
        fns: &fns,
        configs: &configs,
        ctors: &ctors,
        ctor_faults: &ctor_faults,
        inlining: &inlining,
    };

    let mut ty_ctx = TyCtx::new();
    let mut results = Vec::new();

    for item in &m.items {
        if let Item::Let(let_decl) = item {
            let res = (|| {
                check_declared_field_types(&let_decl.value, ctx, &ty_ctx)?;
                let tr_expr = lower_expr(&let_decl.value, ctx)?;
                let (ty, _ty_tree, st) = infer_tree(&tr_expr, &ty_ctx, Infer::new())?;
                let inferred_ty = zonk(&ty, &st.subst);

                // A declared type is a contract. Previously only the *grade*
                // half of an annotation was discharged, so `let x: Str = 42`
                // reported `x : Int @Proven` — the annotation carried,
                // rendered, and never checked.
                if let Some(declared) = &let_decl.ty {
                    let mut stack = Vec::new();
                    match resolve_config_ty(declared, ctx.configs, &mut stack) {
                        Ok(declared_ty) => {
                            if !types_agree(&declared_ty, &inferred_ty) {
                                return Err(LowerError::TypeAnnotationMismatch {
                                    declared: render_ty(&declared_ty),
                                    inferred: render_ty(&inferred_ty),
                                });
                            }
                        }
                        Err(ConfigTyError::Unknown(name)) => {
                            return Err(LowerError::UnknownDeclaredType(name));
                        }
                        Err(ConfigTyError::Arity {
                            config,
                            expected,
                            found,
                        }) => {
                            return Err(LowerError::ConfigArityMismatch {
                                config,
                                expected,
                                found,
                            })
                        }
                        Err(ConfigTyError::Mutual(cycle)) => {
                            return Err(LowerError::MutuallyRecursiveConfig {
                                config: cycle.first().cloned().unwrap_or_default(),
                                cycle,
                            });
                        }
                    }
                }

                // `match … proving exhaustive` on a top-level value: request a
                // kernel-certified coverage certificate. The typing result's grade
                // is unchanged (the match is a value like any other); coverage is a
                // *separate* proposition, @Proven only when the kernel accepts.
                let coverage = coverage_for(&let_decl.value, &tr_expr, ctx);

                // `audited_type_check_tree` now returns the checked
                // `TreeDerivation` artifact its judgement's evidence names
                // (ADR-0017), not a bare tree — `elaborate_tree` binds the
                // source to it at the boundary.
                let (audited_judgement, derivation) =
                    audited_type_check_tree(&tr_expr, &ty_ctx, ContextId::root())?;
                match elaborate_tree(&audited_judgement, &derivation, Budget::new(2000, 2000)) {
                    ElaborationResult::Proven { judgement, edge } => {
                        // The kernel proves the *composition* (judgement.outcome,
                        // e.g. Proven) conditional on the primitive typing-rule
                        // leaves. The honest status of the typing result is that
                        // capped by leaf discharge — Audited until the leaves are
                        // proven tight (the SOC tight-generator obligation).
                        let outcome = honest_result_outcome(judgement.outcome, derivation.tree());

                        // Discharge any `@grade` assertion against the earned grade
                        // via the GRADE coercion lattice: the actual grade may only
                        // WEAKEN to the assertion (downgrade is free); asserting a
                        // stronger grade than earned is epistemic erasure.
                        if let Some(asserted) = asserted_grade(&let_decl.ty) {
                            let actual = outcome_grade_name(outcome);
                            if !grade_assertion_satisfied(actual, asserted) {
                                return Err(LowerError::GradeErasure {
                                    asserted: asserted.to_string(),
                                    actual: actual.to_string(),
                                });
                            }
                        }

                        Ok((
                            CheckResult {
                                name: let_decl.name.clone(),
                                outcome,
                                ty: Some(inferred_ty.clone()),
                                coverage,
                                proposition: judgement.proposition,
                                context: judgement.context,
                                certificate: judgement.evidence,
                                elaboration_edge: edge,
                                derivation: derivation.tree().clone(),
                            },
                            inferred_ty,
                        ))
                    }
                    ElaborationResult::NotElaborated(verdict) => {
                        Err(LowerError::ElaborationFailed(verdict))
                    }
                    ElaborationResult::Refused(err) => Err(LowerError::ElaborationRefused(err)),
                }
            })();

            match res {
                Ok((check_res, inferred_ty)) => {
                    ty_ctx = ty_ctx.extend(let_decl.name.clone(), inferred_ty);
                    results.push(Ok(check_res));
                }
                Err(err) => {
                    results.push(Err((let_decl.name.clone(), err)));
                }
            }
        }
    }

    results
}
