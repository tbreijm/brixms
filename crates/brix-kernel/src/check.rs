//! Total, strictly-terminating bidirectional proof-term acceptance checker (ADR-0003 §4, §5).

use brix_semantic::{ContextId, PropositionId};

use crate::certificate::{certificate_id_v1, native_verifier, CertificateMaterialV1};
use crate::prim_registry;
use crate::term::{instantiate, ExplicitTerm, ObjectTerm, Prop, TermKind, Var};
use crate::verdict::{
    Certificate, RejectionReason, ResourceBudgetReason, UnsupportedConstruct, Verdict,
};

/// Resource budget limits for term type-checking to guarantee strict termination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Maximum total evaluation steps across all recursive checks.
    pub max_steps: usize,
    /// Maximum AST / evaluation recursion depth.
    pub max_depth: usize,
}

impl Budget {
    /// Create a new evaluation budget.
    pub fn new(max_steps: usize, max_depth: usize) -> Self {
        Self {
            max_steps,
            max_depth,
        }
    }
}

/// Internal execution state for resource tracking.
struct CheckerState {
    steps: usize,
    depth: usize,
}

impl CheckerState {
    fn step(&mut self) -> Result<(), Verdict> {
        if self.steps == 0 {
            return Err(Verdict::ResourceExhausted(
                ResourceBudgetReason::StepLimitExceeded,
            ));
        }
        self.steps -= 1;
        Ok(())
    }

    /// Take one depth unit, or refuse.
    ///
    /// Paired with [`CheckerState::leave_depth`] rather than wrapping a
    /// closure: the closure form cost a stack frame per level on the one path
    /// in this checker that recurses as deep as a program is large, and the
    /// two call sites hold the unit across a loop or a call that must not be
    /// inside another frame.
    fn enter_depth(&mut self) -> Result<(), Verdict> {
        if self.depth == 0 {
            return Err(Verdict::ResourceExhausted(
                ResourceBudgetReason::DepthLimitExceeded,
            ));
        }
        self.depth -= 1;
        Ok(())
    }

    /// Give back `n` depth units taken by [`CheckerState::enter_depth`].
    fn leave_depth(&mut self, n: usize) {
        self.depth += n;
    }
}

/// Hypotheses context stack \(\Gamma\) holding variable bindings and their types.
type Gamma = Vec<(Option<String>, Prop)>;

/// Total entry point for canonical proof-term verification (ADR-0003 §2, §5).
///
/// Returns an exhaustive [`Verdict`].
pub fn acceptance(
    context: &ContextId,
    proposition: &Prop,
    term: &ExplicitTerm,
    budget: Budget,
) -> Verdict {
    // 1. Context verification (ADR-0003 §3)
    if term.context != *context {
        return Verdict::ContextMismatch {
            claimed: *context,
            term_context: term.context,
        };
    }

    acceptance_with_gamma(
        context,
        proposition,
        term,
        budget,
        Vec::new(),
        native_verifier(),
        || certificate_id_v1(&CertificateMaterialV1::new(context, proposition, term)),
    )
}

/// Check a proof with kernel-validated external hypotheses already in Gamma.
/// This is crate-private so callers cannot manufacture trusted assumptions.
pub(crate) fn acceptance_with_gamma(
    context: &ContextId,
    proposition: &Prop,
    term: &ExplicitTerm,
    budget: Budget,
    mut gamma: Vec<(Option<String>, Prop)>,
    verifier: brix_semantic::VerifierId,
    certificate_id: impl FnOnce() -> brix_semantic::CertificateId,
) -> Verdict {
    if term.context != *context {
        return Verdict::ContextMismatch {
            claimed: *context,
            term_context: term.context,
        };
    }
    let mut state = CheckerState {
        steps: budget.max_steps,
        depth: budget.max_depth,
    };

    // 2. Bidirectional type check of term against proposition
    match check_type(&mut state, &mut gamma, &term.kind, proposition) {
        Ok(()) => Verdict::Accepted(Certificate {
            verifier,
            certificate_id: certificate_id(),
        }),
        Err(verdict) => verdict,
    }
}

fn obj_term_contains(term: &ObjectTerm, target: &ObjectTerm) -> bool {
    if term == target {
        return true;
    }
    match term {
        ObjectTerm::Compose(g2, g1) => {
            obj_term_contains(g2, target) || obj_term_contains(g1, target)
        }
        ObjectTerm::Tensor(left, right) => {
            obj_term_contains(left, target) || obj_term_contains(right, target)
        }
        ObjectTerm::Id(inner) => obj_term_contains(inner, target),
        _ => false,
    }
}

/// Helper to check if an object term `target` occurs free in `prop`.
fn prop_contains_obj_term(prop: &Prop, target: &ObjectTerm) -> bool {
    match prop {
        Prop::Atom(_) => false,
        Prop::Impl(p1, p2) | Prop::Prod(p1, p2) | Prop::Sum(p1, p2) => {
            prop_contains_obj_term(p1, target) || prop_contains_obj_term(p2, target)
        }
        Prop::Eq(t1, t2) => obj_term_contains(t1, target) || obj_term_contains(t2, target),
        Prop::Exists(body) => prop_contains_obj_term(body, target),
        Prop::Applied(_, args) => args.iter().any(|arg| obj_term_contains(arg, target)),
        Prop::Realizes(w, x, y) => {
            obj_term_contains(w, target)
                || obj_term_contains(x, target)
                || obj_term_contains(y, target)
        }
        Prop::Preserves(w, motive) => {
            obj_term_contains(w, target) || prop_contains_obj_term(motive, target)
        }
    }
}

/// Check if `kind` has type `expected` in context `gamma`.
fn check_type(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    kind: &TermKind,
    expected: &Prop,
) -> Result<(), Verdict> {
    // (→I) is peeled iteratively rather than recursively.
    //
    // **Why this rule and not the others.** A lambda checked against an
    // implication is a *tail* position: push the hypothesis, then check the
    // body against the consequent, and the only thing left to do afterwards is
    // pop. Nothing is computed on the way back out, so the native frame the
    // recursive form held there carried nothing.
    //
    // It is also the rule that actually gets deep. `brix-elaborate` discharges
    // a derivation's leaves as hypotheses, emitting `λh₁…λhₘ. body` against
    // `H₁ → … → Hₘ → G` — so the spine's length is the derivation's *leaf
    // count*, which grows with the program. Every other introduction rule here
    // nests only as deep as the proposition's own structure.
    //
    // The budget is accounted exactly as the recursion accounted it: one step
    // and one depth unit per peeled binder, all of them still held while the
    // body is checked, all released together afterwards. `ResourceExhausted`
    // therefore fires on the same terms at the same budgets — the verdict is
    // observable, so this may not be an approximation of the old behaviour.
    let mut kind = kind;
    let mut expected = expected;
    let mut depth_taken = 0usize;
    let mut pushed = 0usize;

    let outcome = loop {
        if let Err(v) = state.step() {
            break Err(v);
        }
        if let Err(v) = state.enter_depth() {
            break Err(v);
        }
        depth_taken += 1;

        match (kind, expected) {
            (TermKind::Lam { var_name, body }, Prop::Impl(param_prop, result_prop)) => {
                gamma.push((var_name.clone(), *param_prop.clone()));
                pushed += 1;
                kind = body;
                expected = result_prop;
            }
            _ => {
                break check_rule(state, gamma, kind, expected);
            }
        }
    };

    for _ in 0..pushed {
        gamma.pop();
    }
    state.leave_depth(depth_taken);
    outcome
}

/// The non-(→I) rules of [`check_type`].
///
/// Split out so the spine loop above stays readable, and so a level pays for
/// the arm it takes rather than for the union of every arm's locals — the
/// debug frame-size cost #319 measured on the inference side. The step and
/// depth units for this level are already taken by the caller.
fn check_rule(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    kind: &TermKind,
    expected: &Prop,
) -> Result<(), Verdict> {
    match (kind, expected) {
        // (=I) Equality Reflexivity
        (TermKind::Refl(t), Prop::Eq(a, b)) => {
            if a == b && a == t {
                Ok(())
            } else {
                Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("{expected:?}"),
                    found: format!("Refl({t:?})"),
                }))
            }
        }

        // (∃I) Existential Pack
        (
            TermKind::Pack {
                witness,
                body_proof,
            },
            Prop::Exists(pred),
        ) => {
            let expected_body_type = instantiate(pred, witness);
            check_type(state, gamma, body_proof, &expected_body_type)
        }

        // (∃E) Existential Unpack with Eigenvariable Freshness
        (
            TermKind::Unpack {
                scrutinee,
                obj_var,
                proof_var,
                body,
            },
            _,
        ) => {
            let scrut_type = infer_type(state, gamma, scrutinee)?;
            let pred = match scrut_type {
                Prop::Exists(pred) => pred,
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Exists(pred)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            // Eigenvariable freshness side condition check for proof_var in gamma
            if let Some(ref name) = proof_var {
                if gamma
                    .iter()
                    .any(|(opt, _)| opt.as_deref() == Some(name.as_str()))
                {
                    return Err(Verdict::Malformed(format!(
                        "Eigenvariable freshness condition failed: '{name}' already present in context"
                    )));
                }
            }

            let fresh_id = PropositionId::from_canon(
                format!(
                    "eigenvar:{}:{}",
                    obj_var.as_deref().unwrap_or("anon"),
                    state.steps
                )
                .as_bytes(),
            );
            let fresh_x = ObjectTerm::Const(fresh_id);

            // Eigenvariable freshness: x MUST NOT occur free in expected (conclusion R)
            if prop_contains_obj_term(expected, &fresh_x) {
                return Err(Verdict::Malformed(
                    "Eigenvariable witness escape: eigenvariable occurs free in conclusion".into(),
                ));
            }

            let hyp_type = instantiate(&pred, &fresh_x);
            gamma.push((proof_var.clone(), hyp_type));
            let res = check_type(state, gamma, body, expected);
            gamma.pop();
            res
        }

        // (-> I) Implication Introduction
        // (→I) is handled by `check_type`'s spine loop and never reaches here.
        // A `Lam` against a non-`Impl` falls through to the synthesis
        // fallback below, exactly as it did when this was one match.

        // (x I) Product Introduction
        (TermKind::Pair { fst, snd }, Prop::Prod(p1, p2)) => {
            check_type(state, gamma, fst, p1)?;
            check_type(state, gamma, snd, p2)
        }

        // (+ I1) Sum Introduction Left
        (TermKind::Inl(inner), Prop::Sum(p1, _)) => check_type(state, gamma, inner, p1),

        // (+ I2) Sum Introduction Right
        (TermKind::Inr(inner), Prop::Sum(_, p2)) => check_type(state, gamma, inner, p2),

        // (+ E) Sum Elimination with Eigenvariable Freshness Side Condition (ADR-0003 §5.2)
        (
            TermKind::Case {
                discriminant,
                left_var,
                left_body,
                right_var,
                right_body,
            },
            _,
        ) => {
            let disc_type = infer_type(state, gamma, discriminant)?;
            let (p1, p2) = match disc_type {
                Prop::Sum(p1, p2) => (*p1, *p2),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Sum (P + Q)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            // Eigenvariable freshness side condition check (ADR-0003 §5.2)
            if let Some(ref name) = left_var {
                if gamma
                    .iter()
                    .any(|(opt, _)| opt.as_deref() == Some(name.as_str()))
                {
                    return Err(Verdict::Malformed(format!(
                        "Eigenvariable freshness condition failed: '{name}' already present in context"
                    )));
                }
            }
            if let Some(ref name) = right_var {
                if gamma
                    .iter()
                    .any(|(opt, _)| opt.as_deref() == Some(name.as_str()))
                {
                    return Err(Verdict::Malformed(format!(
                        "Eigenvariable freshness condition failed: '{name}' already present in context"
                    )));
                }
            }

            // Check left branch
            gamma.push((left_var.clone(), p1));
            let left_res = check_type(state, gamma, left_body, expected);
            gamma.pop();
            left_res?;

            // Check right branch
            gamma.push((right_var.clone(), p2));
            let right_res = check_type(state, gamma, right_body, expected);
            gamma.pop();
            right_res
        }

        // (RealizesComp) Realization Composition (Profile 1.1)
        (TermKind::RealizesComp { left, right }, Prop::Realizes(w, x, z)) => {
            let left_type = infer_type(state, gamma, left)?;
            let (g1, xl, y) = match left_type {
                Prop::Realizes(g1, xl, y) => (g1, xl, y),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Realizes(g1, x, y)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            let right_type = infer_type(state, gamma, right)?;
            let (g2, ys, z2) = match right_type {
                Prop::Realizes(g2, ys, z2) => (g2, ys, z2),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Realizes(g2, y, z)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            // Side condition (iii): Outer source endpoint match
            if xl != *x {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Source endpoint {x:?}"),
                    found: format!("{xl:?}"),
                }));
            }

            // Side condition (i): Middle endpoint match
            if y != ys {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Middle endpoint matching {y:?}"),
                    found: format!("{ys:?}"),
                }));
            }

            // Side condition (iii): Outer target endpoint match
            if z2 != *z {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Target endpoint {z:?}"),
                    found: format!("{z2:?}"),
                }));
            }

            // Side condition (ii): Witness match compose(outer g2, inner g1) via digest comparison
            let expected_witness_id =
                brix_semantic::compose(g2.witness_digest(), g1.witness_digest());
            if w.witness_digest() != expected_witness_id {
                let expected_witness = ObjectTerm::Compose(Box::new(g2), Box::new(g1));
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Witness {expected_witness:?}"),
                    found: format!("{w:?}"),
                }));
            }

            Ok(())
        }

        // (RealizesTensor) Realization Tensor (Profile 1.2)
        (TermKind::RealizesTensor { left, right }, Prop::Realizes(w, x, z)) => {
            let left_type = infer_type(state, gamma, left)?;
            let (w1, x1, y1) = match left_type {
                Prop::Realizes(w1, x1, y1) => (w1, x1, y1),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Realizes(w1, x1, y1)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            let right_type = infer_type(state, gamma, right)?;
            let (w2, x2, y2) = match right_type {
                Prop::Realizes(w2, x2, y2) => (w2, x2, y2),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Realizes(w2, x2, y2)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            // Side condition (a): Source match: *x MUST structurally equal ObjectTerm::Tensor(x1, x2)
            let expected_source = ObjectTerm::Tensor(Box::new(x1), Box::new(x2));
            if *x != expected_source {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Source endpoint {expected_source:?}"),
                    found: format!("{x:?}"),
                }));
            }

            // Side condition (b): Target match: *z MUST structurally equal ObjectTerm::Tensor(y1, y2)
            let expected_target = ObjectTerm::Tensor(Box::new(y1), Box::new(y2));
            if *z != expected_target {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Target endpoint {expected_target:?}"),
                    found: format!("{z:?}"),
                }));
            }

            // Side condition (c): Witness match (digest-based): w.witness_digest() == tensor(w1.witness_digest(), w2.witness_digest())
            let expected_witness_id =
                brix_semantic::tensor(w1.witness_digest(), w2.witness_digest());
            if w.witness_digest() != expected_witness_id {
                let expected_witness = ObjectTerm::Tensor(Box::new(w1), Box::new(w2));
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Witness {expected_witness:?}"),
                    found: format!("{w:?}"),
                }));
            }

            Ok(())
        }

        // (RealizesId) Structural identity realization (ADR-0025 §10.1)
        (TermKind::RealizesId(t), Prop::Realizes(w, x, z)) => {
            let expected_digest = validate_and_digest_endpoint(state, t)?;
            check_endpoints_equal(state, x, t, "Source")?;
            check_endpoints_equal(state, z, t, "Target")?;
            let actual_digest = validate_and_digest_identity_witness(state, w)?;

            if actual_digest != expected_digest {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Identity witness with digest {expected_digest:?}"),
                    found: format!("Identity witness with digest {actual_digest:?}"),
                }));
            }

            Ok(())
        }

        // Out-of-slice unsupported construct placeholder
        (TermKind::Unsupported(msg), _) => Err(Verdict::Unsupported(
            UnsupportedConstruct::Construct(msg.clone()),
        )),

        // Fallback to type synthesis
        _ => {
            let inferred = infer_type(state, gamma, kind)?;
            if inferred == *expected {
                Ok(())
            } else {
                Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("{expected:?}"),
                    found: format!("{inferred:?}"),
                }))
            }
        }
    }
}

/// Infer/synthesize the type of `kind` in context `gamma`.
fn infer_type(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    kind: &TermKind,
) -> Result<Prop, Verdict> {
    state.step()?;

    // `enter_depth`'s closure is spelled out here rather than used, and the
    // accounting is identical — take a depth unit, run, give it back. The
    // closure is a whole extra frame on the one path in this checker that
    // recurses as deep as a program is large, and this is that path's entry.
    state.enter_depth()?;
    let res = infer_rule(state, gamma, kind);
    state.leave_depth(1);
    res
}

/// The synthesis rules of [`infer_type`], with the step and depth units for
/// this level already taken.
///
/// The rules that carry real work are each their own `#[inline(never)]`
/// function below rather than an arm here. That is the #319 measurement
/// applied to the kernel: a debug build gives one frame every arm's locals, so
/// a level on the recursion's hot path — `RealizesComp`, which is as deep as
/// the derivation has leaves — paid for `Unpack`'s and `PrimRealizes`'s
/// temporaries too. The bodies are moved unchanged; only where they live moved.
fn infer_rule(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    kind: &TermKind,
) -> Result<Prop, Verdict> {
    match kind {
        // (Hyp) Hypothesis lookup
        TermKind::Hyp(var) => match var {
            Var::Index(idx) => match gamma.iter().rev().nth(*idx) {
                Some((_, prop)) => Ok(prop.clone()),
                None => Err(Verdict::Rejected(RejectionReason::HypothesisNotFound(
                    format!("Index {idx} out of bounds (context depth {})", gamma.len()),
                ))),
            },
            Var::Named(name) => {
                match gamma
                    .iter()
                    .rev()
                    .find(|(opt, _)| opt.as_deref() == Some(name.as_str()))
                {
                    Some((_, prop)) => Ok(prop.clone()),
                    None => Err(Verdict::Rejected(RejectionReason::HypothesisNotFound(
                        format!("Variable '{name}' not found in context"),
                    ))),
                }
            }
        },

        // (=I) Equality Reflexivity
        TermKind::Refl(t) => Ok(Prop::Eq(t.clone(), t.clone())),

        // (=E) Equality Substitution
        TermKind::Subst { eq, motive, sub } => infer_subst(state, gamma, eq, motive, sub),

        // (∃E) Existential Unpack synthesis
        TermKind::Unpack {
            scrutinee,
            obj_var,
            proof_var,
            body,
        } => infer_unpack(state, gamma, scrutinee, obj_var, proof_var, body),

        // (Trans-Pres) Transformation Preservation
        TermKind::Pres {
            realizes,
            preserves,
            motive,
            sub,
        } => infer_pres(state, gamma, realizes, preserves, motive, sub),

        // (-> E) Implication Elimination
        TermKind::App { function, argument } => {
            let fun_type = infer_type(state, gamma, function)?;
            match fun_type {
                Prop::Impl(param_type, result_type) => {
                    check_type(state, gamma, argument, &param_type)?;
                    Ok(*result_type)
                }
                other => Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: "Implication (P -> Q)".into(),
                    found: format!("{other:?}"),
                })),
            }
        }

        // (x E1) Product Elimination Left
        TermKind::Proj1(inner) => {
            let inner_type = infer_type(state, gamma, inner)?;
            match inner_type {
                Prop::Prod(p1, _) => Ok(*p1),
                other => Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: "Product (P x Q)".into(),
                    found: format!("{other:?}"),
                })),
            }
        }

        // (x E2) Product Elimination Right
        TermKind::Proj2(inner) => {
            let inner_type = infer_type(state, gamma, inner)?;
            match inner_type {
                Prop::Prod(_, p2) => Ok(*p2),
                other => Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: "Product (P x Q)".into(),
                    found: format!("{other:?}"),
                })),
            }
        }

        // (x I) Product Synthesis
        TermKind::Pair { fst, snd } => {
            let p1 = infer_type(state, gamma, fst)?;
            let p2 = infer_type(state, gamma, snd)?;
            Ok(Prop::Prod(Box::new(p1), Box::new(p2)))
        }

        // (RealizesComp) Realization Composition (Profile 1.1)
        TermKind::RealizesComp { left, right } => infer_realizes_comp(state, gamma, left, right),

        // (RealizesTensor) Realization Tensor (Profile 1.2)
        TermKind::RealizesTensor { left, right } => {
            infer_realizes_tensor(state, gamma, left, right)
        }

        // (PrimRealizes) Primitive realization introduction (ADR-0015 ⟨D-PRIM⟩).
        //
        // Zero premises, and closed: it consults no hypothesis context, so
        // nothing the caller put in Γ can influence the result. Given
        // K[ρ] = (J, g, S, D, R), `src` canonical under S, `dst` canonical
        // under D, and (src, dst) ∈ R, it yields exactly Realizes(g, src, dst).
        //
        // The generator comes from the resolved relation, never from the
        // caller. Checking against an expected proposition goes through the
        // ordinary synthesize-then-compare fallback above — there is no
        // expected-mode arm here, deliberately, because one would let the
        // caller's goal reconstruct a field the registry is supposed to decide.
        TermKind::PrimRealizes { relation, src, dst } => {
            infer_prim_realizes(state, relation, src, dst)
        }

        // (RealizesId) Structural identity realization (ADR-0025 §10.1)
        TermKind::RealizesId(t) => infer_realizes_id(state, t),

        // Out-of-slice unsupported construct placeholder
        TermKind::Unsupported(msg) => Err(Verdict::Unsupported(UnsupportedConstruct::Construct(
            msg.clone(),
        ))),

        _ => Err(Verdict::Rejected(RejectionReason::ProofGoalNotReached)),
    }
}

/// (PrimRealizes) Primitive realization introduction (ADR-0015 ⟨D-PRIM⟩).
///
/// Validate and compute the single canonical PropositionId for a PrimRealizes endpoint.
///
/// Adopts ADR-0025 §4 & §10.2 (⟨D-PRODENDPOINT⟩):
/// - `ObjectTerm::Const(id)`: returns `*id` directly. Costs 1 step.
/// - `ObjectTerm::Tensor(left, right)`: strictly bounded constant-only product.
///   Costs 1 step per node and 1 depth per branch level.
///   Rejects `Compose`, `BoundVar`, and `Id` with `TypeMismatch`.
///   Returns `PropositionId::of(term)`.
fn endpoint_proposition_id(
    state: &mut CheckerState,
    term: &ObjectTerm,
    endpoint_name: &str,
) -> Result<PropositionId, Verdict> {
    state.step()?;
    match term {
        ObjectTerm::Const(id) => Ok(*id),
        ObjectTerm::Tensor(left, right) => {
            state.enter_depth()?;
            let l_res = validate_product_endpoint(state, left, endpoint_name);
            state.leave_depth(1);
            l_res?;

            state.enter_depth()?;
            let r_res = validate_product_endpoint(state, right, endpoint_name);
            state.leave_depth(1);
            r_res?;

            Ok(PropositionId::of(term))
        }
        _ => Err(Verdict::Rejected(RejectionReason::TypeMismatch {
            expected: format!(
                "{endpoint_name} endpoint as object constant or Tensor product of constants"
            ),
            found: object_shape(term).into(),
        })),
    }
}

fn validate_product_endpoint(
    state: &mut CheckerState,
    term: &ObjectTerm,
    endpoint_name: &str,
) -> Result<(), Verdict> {
    state.step()?;
    match term {
        ObjectTerm::Const(_) => Ok(()),
        ObjectTerm::Tensor(left, right) => {
            state.enter_depth()?;
            let l_res = validate_product_endpoint(state, left, endpoint_name);
            state.leave_depth(1);
            l_res?;

            state.enter_depth()?;
            let r_res = validate_product_endpoint(state, right, endpoint_name);
            state.leave_depth(1);
            r_res
        }
        _ => Err(Verdict::Rejected(RejectionReason::TypeMismatch {
            expected: format!("{endpoint_name} product containing only constants"),
            found: object_shape(term).into(),
        })),
    }
}

/// (PrimRealizes) Primitive realization introduction (ADR-0015 ⟨D-PRIM⟩; ADR-0025 §4 & §10.2 ⟨D-PRODENDPOINT⟩).
///
/// Takes no `Gamma`, which is the rule's content rather than an omission: it has
/// zero premises and consults no hypothesis context, so nothing the caller put
/// in Γ can influence the result. Traversal and hashing of endpoints are strictly
/// bounded by `CheckerState`.
#[inline(never)]
fn infer_prim_realizes(
    state: &mut CheckerState,
    relation: &crate::prim_registry::PrimitiveRelationId,
    src: &ObjectTerm,
    dst: &ObjectTerm,
) -> Result<Prop, Verdict> {
    let Some(resolved) = prim_registry::resolve(relation) else {
        // Absence, not refutation (§8.8): the kernel has not introduced
        // the fact, which says nothing about its negation.
        return Err(Verdict::Rejected(
            RejectionReason::UnknownPrimitiveRelation(relation.to_hex()),
        ));
    };

    if !resolved.accepts_endpoint_shapes(src, dst) {
        return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
            expected: "endpoint shapes required by the relation schemas".into(),
            found: format!("({}, {})", object_shape(src), object_shape(dst)),
        }));
    }
    let src_id = endpoint_proposition_id(state, src, "Source")?;
    let dst_id = endpoint_proposition_id(state, dst, "Destination")?;

    if !resolved.admits(&src_id, &dst_id) {
        return Err(Verdict::Rejected(RejectionReason::PrimitiveRowNotFound {
            relation: relation.to_hex(),
            src: src_id.to_hex(),
            dst: dst_id.to_hex(),
        }));
    }

    Ok(Prop::Realizes(
        ObjectTerm::Const(PropositionId(resolved.generator.digest())),
        src.clone(),
        dst.clone(),
    ))
}

/// (=E) Equality Substitution, synthesis mode.
#[inline(never)]
fn infer_subst(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    eq: &TermKind,
    motive: &Prop,
    sub: &TermKind,
) -> Result<Prop, Verdict> {
    let eq_type = infer_type(state, gamma, eq)?;
    let (a, b) = match eq_type {
        Prop::Eq(a, b) => (a, b),
        other => {
            return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                expected: "Eq(a, b)".into(),
                found: format!("{other:?}"),
            }));
        }
    };
    let expected_sub_type = instantiate(motive, &a);
    check_type(state, gamma, sub, &expected_sub_type)?;
    Ok(instantiate(motive, &b))
}

/// (∃E) Existential Unpack, synthesis mode.
#[inline(never)]
fn infer_unpack(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    scrutinee: &TermKind,
    obj_var: &Option<String>,
    proof_var: &Option<String>,
    body: &TermKind,
) -> Result<Prop, Verdict> {
    let scrut_type = infer_type(state, gamma, scrutinee)?;
    let pred = match scrut_type {
        Prop::Exists(pred) => pred,
        other => {
            return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                expected: "Exists(pred)".into(),
                found: format!("{other:?}"),
            }));
        }
    };

    if let Some(ref name) = proof_var {
        if gamma
            .iter()
            .any(|(opt, _)| opt.as_deref() == Some(name.as_str()))
        {
            return Err(Verdict::Malformed(format!(
                "Eigenvariable freshness condition failed: '{name}' already present in context"
            )));
        }
    }

    let fresh_id = PropositionId::from_canon(
        format!(
            "eigenvar:{}:{}",
            obj_var.as_deref().unwrap_or("anon"),
            state.steps
        )
        .as_bytes(),
    );
    let fresh_x = ObjectTerm::Const(fresh_id);

    let hyp_type = instantiate(&pred, &fresh_x);
    gamma.push((proof_var.clone(), hyp_type));
    let body_type = infer_type(state, gamma, body);
    gamma.pop();
    let body_type = body_type?;

    // Eigenvariable freshness: x MUST NOT occur free in conclusion body_type
    if prop_contains_obj_term(&body_type, &fresh_x) {
        return Err(Verdict::Malformed(
            "Eigenvariable witness escape: eigenvariable occurs free in conclusion".into(),
        ));
    }

    Ok(body_type)
}

/// (Trans-Pres) Transformation Preservation, synthesis mode.
#[inline(never)]
fn infer_pres(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    realizes: &TermKind,
    preserves: &TermKind,
    motive: &Prop,
    sub: &TermKind,
) -> Result<Prop, Verdict> {
    let realizes_type = infer_type(state, gamma, realizes)?;
    let (w, x, y) = match realizes_type {
        Prop::Realizes(w, x, y) => (w, x, y),
        other => {
            return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                expected: "Realizes(w, x, y)".into(),
                found: format!("{other:?}"),
            }));
        }
    };

    let expected_preserves_type = Prop::Preserves(w.clone(), Box::new(motive.clone()));
    check_type(state, gamma, preserves, &expected_preserves_type)?;

    let expected_sub_type = instantiate(motive, &x);
    check_type(state, gamma, sub, &expected_sub_type)?;

    Ok(instantiate(motive, &y))
}

/// (RealizesComp) Realization Composition (Profile 1.1), synthesis mode.
///
/// Unrolls right-nested `RealizesComp` chains iteratively to prevent stack
/// exhaustion on sequential expression chains (where `Seq` elaborates to a
/// right-nested `RealizesComp` spine).
///
/// The step and depth accounting matches recursive descent exactly:
/// each intermediate `Comp` node consumes one step and one depth unit during
/// descent, which is restored after the chain completes.
#[inline(never)]
fn infer_realizes_comp(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    left: &TermKind,
    right: &TermKind,
) -> Result<Prop, Verdict> {
    // 1. Infer the leftmost term.
    let left_type = infer_type(state, gamma, left)?;
    let (g_left, xl, mut y_curr) = match left_type {
        Prop::Realizes(g1, xl, y) => (g1, xl, y),
        other => {
            return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                expected: "Realizes(g1, x, y)".into(),
                found: format!("{other:?}"),
            }));
        }
    };

    // If `right` is not a `RealizesComp`, handle the base binary case directly.
    let (next_left, next_right) = match right {
        TermKind::RealizesComp {
            left: nl,
            right: nr,
        } => (nl.as_ref(), nr.as_ref()),
        _ => {
            let right_type = infer_type(state, gamma, right)?;
            let (g2, ys, z2) = match right_type {
                Prop::Realizes(g2, ys, z2) => (g2, ys, z2),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Realizes(g2, y, z)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            if y_curr != ys {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Middle endpoint matching {y_curr:?}"),
                    found: format!("{ys:?}"),
                }));
            }

            return Ok(Prop::Realizes(
                ObjectTerm::Compose(Box::new(g2), Box::new(g_left)),
                xl,
                z2,
            ));
        }
    };

    // Otherwise, unroll the right spine iteratively.
    // Each step along the spine corresponds to one `infer_type(Comp)` call
    // that would have occurred recursively.
    let mut witnesses = vec![g_left];
    let mut depth_taken = 0usize;

    let mut curr_left = next_left;
    let mut curr_right = next_right;

    let (final_witness, final_z) = loop {
        // Step and enter depth for this Comp node (matching infer_type(Comp)).
        if let Err(v) = state.step() {
            state.leave_depth(depth_taken);
            return Err(v);
        }
        if let Err(v) = state.enter_depth() {
            state.leave_depth(depth_taken);
            return Err(v);
        }
        depth_taken += 1;

        // Infer `curr_left`.
        let l_type = match infer_type(state, gamma, curr_left) {
            Ok(t) => t,
            Err(e) => {
                state.leave_depth(depth_taken);
                return Err(e);
            }
        };
        let (g_l, ys, y_next) = match l_type {
            Prop::Realizes(g, ys, y2) => (g, ys, y2),
            other => {
                state.leave_depth(depth_taken);
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: "Realizes(g, y, z)".into(),
                    found: format!("{other:?}"),
                }));
            }
        };

        if y_curr != ys {
            state.leave_depth(depth_taken);
            return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                expected: format!("Middle endpoint matching {y_curr:?}"),
                found: format!("{ys:?}"),
            }));
        }
        y_curr = y_next;
        witnesses.push(g_l);

        // Check if `curr_right` continues the right-nested spine.
        match curr_right {
            TermKind::RealizesComp {
                left: nl,
                right: nr,
            } => {
                curr_left = nl.as_ref();
                curr_right = nr.as_ref();
            }
            _ => {
                // Leaf of the right spine: infer `curr_right`.
                let r_type = match infer_type(state, gamma, curr_right) {
                    Ok(t) => t,
                    Err(e) => {
                        state.leave_depth(depth_taken);
                        return Err(e);
                    }
                };
                let (g_r, ys, z) = match r_type {
                    Prop::Realizes(g, ys, z) => (g, ys, z),
                    other => {
                        state.leave_depth(depth_taken);
                        return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                            expected: "Realizes(g, y, z)".into(),
                            found: format!("{other:?}"),
                        }));
                    }
                };

                if y_curr != ys {
                    state.leave_depth(depth_taken);
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: format!("Middle endpoint matching {y_curr:?}"),
                        found: format!("{ys:?}"),
                    }));
                }

                break (g_r, z);
            }
        }
    };

    // Restore all depth units taken for intermediate Comp nodes.
    state.leave_depth(depth_taken);

    // Reconstruct the right-nested witness:
    // Comp(L_0, Comp(L_1, Comp(..., R))) has witness
    // Compose(...Compose(Compose(g_R, g_k), ...), g_0).
    // Starting with final_witness (g_R), fold backwards over witnesses.
    let mut combined_witness = final_witness;
    for w in witnesses.into_iter().rev() {
        combined_witness = ObjectTerm::Compose(Box::new(combined_witness), Box::new(w));
    }

    Ok(Prop::Realizes(combined_witness, xl, final_z))
}

/// (RealizesId) Structural identity realization (ADR-0025 §10.1), synthesis mode.
#[inline(never)]
fn infer_realizes_id(state: &mut CheckerState, t: &ObjectTerm) -> Result<Prop, Verdict> {
    validate_and_digest_endpoint(state, t)?;
    Ok(Prop::Realizes(
        ObjectTerm::Id(Box::new(t.clone())),
        t.clone(),
        t.clone(),
    ))
}

/// (RealizesTensor) Realization Tensor (Profile 1.2), synthesis mode.
///
/// Unrolls right-nested `RealizesTensor` chains iteratively to prevent stack
/// exhaustion on nested tensor structures (e.g. `Tensor(Id(Op), Tensor(da, db))`).
///
/// Step and depth accounting matches recursive descent exactly:
/// each intermediate `Tensor` node on the right spine consumes one step and one depth unit
/// during descent, which is restored after the chain completes.
#[inline(never)]
fn infer_realizes_tensor(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    left: &TermKind,
    right: &TermKind,
) -> Result<Prop, Verdict> {
    let left_type = infer_type(state, gamma, left)?;
    let (w1, x1, y1) = match left_type {
        Prop::Realizes(w1, x1, y1) => (w1, x1, y1),
        other => {
            return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                expected: "Realizes(w1, x1, y1)".into(),
                found: format!("{other:?}"),
            }));
        }
    };

    let (next_left, next_right) = match right {
        TermKind::RealizesTensor {
            left: nl,
            right: nr,
        } => (nl.as_ref(), nr.as_ref()),
        _ => {
            let right_type = infer_type(state, gamma, right)?;
            let (w2, x2, y2) = match right_type {
                Prop::Realizes(w2, x2, y2) => (w2, x2, y2),
                other => {
                    return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                        expected: "Realizes(w2, x2, y2)".into(),
                        found: format!("{other:?}"),
                    }));
                }
            };

            return Ok(Prop::Realizes(
                ObjectTerm::Tensor(Box::new(w1), Box::new(w2)),
                ObjectTerm::Tensor(Box::new(x1), Box::new(x2)),
                ObjectTerm::Tensor(Box::new(y1), Box::new(y2)),
            ));
        }
    };

    let mut elements = vec![(w1, x1, y1)];
    let mut depth_taken = 0usize;
    let mut curr_left = next_left;
    let mut curr_right = next_right;

    let (final_w, final_x, final_y) = loop {
        if let Err(v) = state.step() {
            state.leave_depth(depth_taken);
            return Err(v);
        }
        if let Err(v) = state.enter_depth() {
            state.leave_depth(depth_taken);
            return Err(v);
        }
        depth_taken += 1;

        let l_type = match infer_type(state, gamma, curr_left) {
            Ok(t) => t,
            Err(e) => {
                state.leave_depth(depth_taken);
                return Err(e);
            }
        };
        let (w_l, x_l, y_l) = match l_type {
            Prop::Realizes(w, x, y) => (w, x, y),
            other => {
                state.leave_depth(depth_taken);
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: "Realizes(w, x, y)".into(),
                    found: format!("{other:?}"),
                }));
            }
        };
        elements.push((w_l, x_l, y_l));

        match curr_right {
            TermKind::RealizesTensor {
                left: nl,
                right: nr,
            } => {
                curr_left = nl.as_ref();
                curr_right = nr.as_ref();
            }
            _ => {
                let r_type = match infer_type(state, gamma, curr_right) {
                    Ok(t) => t,
                    Err(e) => {
                        state.leave_depth(depth_taken);
                        return Err(e);
                    }
                };
                let (w_r, x_r, y_r) = match r_type {
                    Prop::Realizes(w, x, y) => (w, x, y),
                    other => {
                        state.leave_depth(depth_taken);
                        return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                            expected: "Realizes(w, x, y)".into(),
                            found: format!("{other:?}"),
                        }));
                    }
                };
                break (w_r, x_r, y_r);
            }
        }
    };

    state.leave_depth(depth_taken);

    let mut w_res = final_w;
    let mut x_res = final_x;
    let mut y_res = final_y;
    for (w_i, x_i, y_i) in elements.into_iter().rev() {
        w_res = ObjectTerm::Tensor(Box::new(w_i), Box::new(w_res));
        x_res = ObjectTerm::Tensor(Box::new(x_i), Box::new(x_res));
        y_res = ObjectTerm::Tensor(Box::new(y_i), Box::new(y_res));
    }

    Ok(Prop::Realizes(w_res, x_res, y_res))
}

/// Validate that an object term is an admissible structural endpoint (ADR-0025 §10.1 / ADR-0015 §2):
/// only `Const` and finite `Tensor` products thereof (rejecting `Compose`, `BoundVar`, and `Id`).
/// Traversal is strictly budget-bounded: every node costs one step, and product branches cost depth.
/// Returns the canonical identity witness digest for this endpoint.
// This bound also keeps subsequent cloning/canonical encoding of admitted objects
// within a safe native-stack depth, independently of caller-supplied budgets.
const MAX_STRUCTURAL_OBJECT_DEPTH: usize = 128;

fn object_shape(term: &ObjectTerm) -> &'static str {
    match term {
        ObjectTerm::Const(_) => "Const",
        ObjectTerm::BoundVar(_) => "BoundVar",
        ObjectTerm::Compose(..) => "Compose",
        ObjectTerm::Tensor(..) => "Tensor",
        ObjectTerm::Id(_) => "Id",
    }
}

fn structural_depth(state: &CheckerState, depth: usize) -> Result<(), Verdict> {
    if depth > state.depth.min(MAX_STRUCTURAL_OBJECT_DEPTH) {
        Err(Verdict::ResourceExhausted(
            ResourceBudgetReason::DepthLimitExceeded,
        ))
    } else {
        Ok(())
    }
}

fn validate_and_digest_endpoint(
    state: &mut CheckerState,
    term: &ObjectTerm,
) -> Result<brix_semantic::WitnessId, Verdict> {
    identity_digest(state, term, false)
}

/// Iterative, budgeted traversal of endpoints or identity witnesses. The two
/// modes deliberately admit different constructors; neither hashes an unchecked
/// subtree or renders it in an error message.
fn identity_digest(
    state: &mut CheckerState,
    term: &ObjectTerm,
    witness: bool,
) -> Result<brix_semantic::WitnessId, Verdict> {
    enum Frame<'a> {
        Visit(&'a ObjectTerm, usize, bool),
        Tensor,
    }
    let mut pending = vec![Frame::Visit(term, 0, witness)];
    let mut values = Vec::new();
    while let Some(frame) = pending.pop() {
        match frame {
            Frame::Tensor => {
                let right = values.pop().expect("right identity digest");
                let left = values.pop().expect("left identity digest");
                values.push(brix_semantic::tensor(left, right));
            }
            Frame::Visit(term, depth, witness) => {
                structural_depth(state, depth)?;
                state.step()?;
                match (term, witness) {
                    (ObjectTerm::Const(id), false) => {
                        values.push(brix_semantic::id_witness(id.digest().as_bytes()));
                    }
                    (ObjectTerm::Id(inner), true) => {
                        pending.push(Frame::Visit(inner, depth, false));
                    }
                    (ObjectTerm::Tensor(left, right), _) => {
                        pending.push(Frame::Tensor);
                        pending.push(Frame::Visit(right, depth + 1, witness));
                        pending.push(Frame::Visit(left, depth + 1, witness));
                    }
                    _ => {
                        return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                            expected: if witness {
                                "Id or Tensor of Id"
                            } else {
                                "Const or Tensor of Const"
                            }
                            .into(),
                            found: object_shape(term).into(),
                        }))
                    }
                }
            }
        }
    }
    Ok(values.pop().expect("root identity digest"))
}

fn check_endpoints_equal(
    state: &mut CheckerState,
    actual: &ObjectTerm,
    expected: &ObjectTerm,
    name: &str,
) -> Result<(), Verdict> {
    let mut pending = vec![(actual, expected, 0)];
    while let Some((actual, expected, depth)) = pending.pop() {
        structural_depth(state, depth)?;
        state.step()?;
        match (actual, expected) {
            (ObjectTerm::Const(a), ObjectTerm::Const(e)) if a == e => {}
            (ObjectTerm::Tensor(al, ar), ObjectTerm::Tensor(el, er)) => {
                pending.push((ar, er, depth + 1));
                pending.push((al, el, depth + 1));
            }
            _ => {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("{name} endpoint matching {}", object_shape(expected)),
                    found: object_shape(actual).into(),
                }))
            }
        }
    }
    Ok(())
}

fn validate_and_digest_identity_witness(
    state: &mut CheckerState,
    w: &ObjectTerm,
) -> Result<brix_semantic::WitnessId, Verdict> {
    identity_digest(state, w, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_semantic::{compose_chain, GeneratorId, WitnessId};

    #[test]
    fn test_realizes_comp_const_composite_witness_accepted() {
        let ctx = ContextId::from_canon(b"test_context");
        let g1_gen = GeneratorId::named("gen_1");
        let g2_gen = GeneratorId::named("gen_2");

        let g1 = ObjectTerm::Const(PropositionId(g1_gen.digest()));
        let g2 = ObjectTerm::Const(PropositionId(g2_gen.digest()));

        let x0 = ObjectTerm::Const(PropositionId::from_canon(b"x0"));
        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));

        let p1 = Prop::Realizes(g1, x0.clone(), x1.clone());
        let p2 = Prop::Realizes(g2, x1, x2.clone());

        let composite_witness_id = compose_chain(&[g1_gen, g2_gen]).unwrap();
        let goal_witness = ObjectTerm::Const(PropositionId(composite_witness_id.digest()));
        let goal = Prop::Realizes(goal_witness, x0, x2);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesComp {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Accepted(_)),
            "RealizesComp with Const of composite identity witness should be Accepted, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_comp_wrong_const_composite_witness_rejected() {
        let ctx = ContextId::from_canon(b"test_context");
        let g1_gen = GeneratorId::named("gen_1");
        let g2_gen = GeneratorId::named("gen_2");

        let g1 = ObjectTerm::Const(PropositionId(g1_gen.digest()));
        let g2 = ObjectTerm::Const(PropositionId(g2_gen.digest()));

        let x0 = ObjectTerm::Const(PropositionId::from_canon(b"x0"));
        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));

        let p1 = Prop::Realizes(g1, x0.clone(), x1.clone());
        let p2 = Prop::Realizes(g2, x1, x2.clone());

        let wrong_composite_witness =
            ObjectTerm::Const(PropositionId::from_canon(b"wrong_composite_id"));
        let goal = Prop::Realizes(wrong_composite_witness, x0, x2);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesComp {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Rejected(_)),
            "RealizesComp with wrong Const composite witness should be Rejected, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_accept() {
        let ctx = ContextId::from_canon(b"test_context");
        let w1 = ObjectTerm::Const(PropositionId::from_canon(b"w1"));
        let w2 = ObjectTerm::Const(PropositionId::from_canon(b"w2"));

        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let y1 = ObjectTerm::Const(PropositionId::from_canon(b"y1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));
        let y2 = ObjectTerm::Const(PropositionId::from_canon(b"y2"));

        let p1 = Prop::Realizes(w1.clone(), x1.clone(), y1.clone());
        let p2 = Prop::Realizes(w2.clone(), x2.clone(), y2.clone());

        let goal_w = ObjectTerm::Tensor(Box::new(w1), Box::new(w2));
        let goal_x = ObjectTerm::Tensor(Box::new(x1), Box::new(x2));
        let goal_y = ObjectTerm::Tensor(Box::new(y1), Box::new(y2));
        let goal = Prop::Realizes(goal_w, goal_x, goal_y);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesTensor {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Accepted(_)),
            "RealizesTensor should be Accepted, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_const_composite_witness_accepted() {
        let ctx = ContextId::from_canon(b"test_context");
        let w1_gen = GeneratorId::named("gen_1");
        let w2_gen = GeneratorId::named("gen_2");

        let w1 = ObjectTerm::Const(PropositionId(w1_gen.digest()));
        let w2 = ObjectTerm::Const(PropositionId(w2_gen.digest()));

        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let y1 = ObjectTerm::Const(PropositionId::from_canon(b"y1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));
        let y2 = ObjectTerm::Const(PropositionId::from_canon(b"y2"));

        let p1 = Prop::Realizes(w1, x1.clone(), y1.clone());
        let p2 = Prop::Realizes(w2, x2.clone(), y2.clone());

        let tens_witness_id =
            brix_semantic::tensor(WitnessId::from(w1_gen), WitnessId::from(w2_gen));
        let goal_witness = ObjectTerm::Const(PropositionId(tens_witness_id.digest()));

        let goal_x = ObjectTerm::Tensor(Box::new(x1), Box::new(x2));
        let goal_y = ObjectTerm::Tensor(Box::new(y1), Box::new(y2));
        let goal = Prop::Realizes(goal_witness, goal_x, goal_y);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesTensor {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Accepted(_)),
            "RealizesTensor with Const of tensored witness identity should be Accepted, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_reject_wrong_witness() {
        let ctx = ContextId::from_canon(b"test_context");
        let w1 = ObjectTerm::Const(PropositionId::from_canon(b"w1"));
        let w2 = ObjectTerm::Const(PropositionId::from_canon(b"w2"));

        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let y1 = ObjectTerm::Const(PropositionId::from_canon(b"y1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));
        let y2 = ObjectTerm::Const(PropositionId::from_canon(b"y2"));

        let p1 = Prop::Realizes(w1, x1.clone(), y1.clone());
        let p2 = Prop::Realizes(w2, x2.clone(), y2.clone());

        let wrong_witness = ObjectTerm::Const(PropositionId::from_canon(b"wrong_witness"));
        let goal_x = ObjectTerm::Tensor(Box::new(x1), Box::new(x2));
        let goal_y = ObjectTerm::Tensor(Box::new(y1), Box::new(y2));
        let goal = Prop::Realizes(wrong_witness, goal_x, goal_y);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesTensor {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Rejected(_)),
            "RealizesTensor with wrong witness should be Rejected, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_reject_swapped_order() {
        let ctx = ContextId::from_canon(b"test_context");
        let w1 = ObjectTerm::Const(PropositionId::from_canon(b"w1"));
        let w2 = ObjectTerm::Const(PropositionId::from_canon(b"w2"));

        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let y1 = ObjectTerm::Const(PropositionId::from_canon(b"y1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));
        let y2 = ObjectTerm::Const(PropositionId::from_canon(b"y2"));

        let p1 = Prop::Realizes(w1.clone(), x1.clone(), y1.clone());
        let p2 = Prop::Realizes(w2.clone(), x2.clone(), y2.clone());

        let goal_w = ObjectTerm::Tensor(Box::new(w1), Box::new(w2));
        // Swapped source: Tensor(x2, x1) instead of Tensor(x1, x2)
        let swapped_source = ObjectTerm::Tensor(Box::new(x2), Box::new(x1));
        let goal_y = ObjectTerm::Tensor(Box::new(y1), Box::new(y2));
        let goal = Prop::Realizes(goal_w, swapped_source, goal_y);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesTensor {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Rejected(_)),
            "RealizesTensor with swapped source order should be Rejected, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_reject_wrong_source_or_target() {
        let ctx = ContextId::from_canon(b"test_context");
        let w1 = ObjectTerm::Const(PropositionId::from_canon(b"w1"));
        let w2 = ObjectTerm::Const(PropositionId::from_canon(b"w2"));

        let x1 = ObjectTerm::Const(PropositionId::from_canon(b"x1"));
        let y1 = ObjectTerm::Const(PropositionId::from_canon(b"y1"));
        let x2 = ObjectTerm::Const(PropositionId::from_canon(b"x2"));
        let y2 = ObjectTerm::Const(PropositionId::from_canon(b"y2"));

        let p1 = Prop::Realizes(w1.clone(), x1.clone(), y1.clone());
        let p2 = Prop::Realizes(w2.clone(), x2.clone(), y2.clone());

        let goal_w = ObjectTerm::Tensor(Box::new(w1), Box::new(w2));
        let wrong_source = ObjectTerm::Const(PropositionId::from_canon(b"wrong_source"));
        let goal_y = ObjectTerm::Tensor(Box::new(y1), Box::new(y2));
        let goal = Prop::Realizes(goal_w, wrong_source, goal_y);

        let full_goal = Prop::Impl(
            Box::new(p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesTensor {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Rejected(_)),
            "RealizesTensor with wrong source should be Rejected, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_reject_non_realizes_branch() {
        let ctx = ContextId::from_canon(b"test_context");
        let non_realizes_p1 = Prop::Atom(PropositionId::from_canon(b"atom"));
        let p2 = Prop::Realizes(
            ObjectTerm::Const(PropositionId::from_canon(b"w2")),
            ObjectTerm::Const(PropositionId::from_canon(b"x2")),
            ObjectTerm::Const(PropositionId::from_canon(b"y2")),
        );

        let goal = Prop::Atom(PropositionId::from_canon(b"dummy"));

        let full_goal = Prop::Impl(
            Box::new(non_realizes_p1),
            Box::new(Prop::Impl(Box::new(p2), Box::new(goal))),
        );

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("h1".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h2".into()),
                    body: Box::new(TermKind::RealizesTensor {
                        left: Box::new(TermKind::Hyp(Var::Named("h1".into()))),
                        right: Box::new(TermKind::Hyp(Var::Named("h2".into()))),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Rejected(_)),
            "RealizesTensor with non-Realizes branch should be Rejected, got {verdict:?}"
        );
    }

    #[test]
    fn test_realizes_tensor_adequacy_app_shape() {
        let ctx = ContextId::from_canon(b"test_context");

        let w_f = ObjectTerm::Const(PropositionId::from_canon(b"w_f"));
        let w_x = ObjectTerm::Const(PropositionId::from_canon(b"w_x"));
        let g_split = ObjectTerm::Const(PropositionId::from_canon(b"g_split"));
        let g_app = ObjectTerm::Const(PropositionId::from_canon(b"g_app"));

        let cfg_f_x = ObjectTerm::Const(PropositionId::from_canon(b"cfg(f_x)"));
        let cfg_f = ObjectTerm::Const(PropositionId::from_canon(b"cfg(f)"));
        let cfg_x = ObjectTerm::Const(PropositionId::from_canon(b"cfg(x)"));
        let cfg_a_to_b = ObjectTerm::Const(PropositionId::from_canon(b"cfg(A->B)"));
        let cfg_a = ObjectTerm::Const(PropositionId::from_canon(b"cfg(A)"));
        let cfg_b = ObjectTerm::Const(PropositionId::from_canon(b"cfg(B)"));

        let tens_f_x_src = ObjectTerm::Tensor(Box::new(cfg_f.clone()), Box::new(cfg_x.clone()));
        let tens_arr_a_dst =
            ObjectTerm::Tensor(Box::new(cfg_a_to_b.clone()), Box::new(cfg_a.clone()));

        // Premises
        let p_df = Prop::Realizes(w_f.clone(), cfg_f, cfg_a_to_b);
        let p_dx = Prop::Realizes(w_x.clone(), cfg_x, cfg_a);
        let p_split = Prop::Realizes(g_split.clone(), cfg_f_x.clone(), tens_f_x_src);
        let p_app = Prop::Realizes(g_app.clone(), tens_arr_a_dst, cfg_b.clone());

        // Composite witness: compose(g_app, compose(tensor(w_f, w_x), g_split))
        let w_tens = ObjectTerm::Tensor(Box::new(w_f), Box::new(w_x));
        let w_inner_comp = ObjectTerm::Compose(Box::new(w_tens), Box::new(g_split));
        let w_total = ObjectTerm::Compose(Box::new(g_app), Box::new(w_inner_comp));

        let goal = Prop::Realizes(w_total, cfg_f_x, cfg_b);

        let full_goal = Prop::Impl(
            Box::new(p_df),
            Box::new(Prop::Impl(
                Box::new(p_dx),
                Box::new(Prop::Impl(
                    Box::new(p_split),
                    Box::new(Prop::Impl(Box::new(p_app), Box::new(goal))),
                )),
            )),
        );

        // term: \df. \dx. \gsplit. \gapp. compose(gapp, compose(tensor(df, dx), gsplit))
        let inner_tensor = TermKind::RealizesTensor {
            left: Box::new(TermKind::Hyp(Var::Named("df".into()))),
            right: Box::new(TermKind::Hyp(Var::Named("dx".into()))),
        };
        let inner_comp = TermKind::RealizesComp {
            left: Box::new(TermKind::Hyp(Var::Named("gsplit".into()))),
            right: Box::new(inner_tensor),
        };
        let total_comp = TermKind::RealizesComp {
            left: Box::new(inner_comp),
            right: Box::new(TermKind::Hyp(Var::Named("gapp".into()))),
        };

        let term = ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("df".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("dx".into()),
                    body: Box::new(TermKind::Lam {
                        var_name: Some("gsplit".into()),
                        body: Box::new(TermKind::Lam {
                            var_name: Some("gapp".into()),
                            body: Box::new(total_comp),
                        }),
                    }),
                }),
            },
        );

        let budget = Budget::new(100, 100);
        let verdict = acceptance(&ctx, &full_goal, &term, budget);

        assert!(
            matches!(verdict, Verdict::Accepted(_)),
            "App-shape adequacy tensor/compose derivation should be Accepted, got {verdict:?}"
        );
    }
}
