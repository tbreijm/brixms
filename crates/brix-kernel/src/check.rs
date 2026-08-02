//! Total, strictly-terminating bidirectional proof-term acceptance checker (ADR-0003 §4, §5).

use brix_semantic::{ContextId, PropositionId};

use crate::certificate::{certificate_id_v1, native_verifier, CertificateMaterialV1};
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

    fn enter_depth<F, R>(&mut self, f: F) -> Result<R, Verdict>
    where
        F: FnOnce(&mut CheckerState) -> Result<R, Verdict>,
    {
        if self.depth == 0 {
            return Err(Verdict::ResourceExhausted(
                ResourceBudgetReason::DepthLimitExceeded,
            ));
        }
        self.depth -= 1;
        let res = f(self);
        self.depth += 1;
        res
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

    let mut state = CheckerState {
        steps: budget.max_steps,
        depth: budget.max_depth,
    };

    let mut gamma: Gamma = Vec::new();

    // 2. Bidirectional type check of term against proposition
    match check_type(&mut state, &mut gamma, &term.kind, proposition) {
        Ok(()) => Verdict::Accepted(Certificate {
            verifier: native_verifier(),
            certificate_id: certificate_id_v1(&CertificateMaterialV1::new(
                context,
                proposition,
                term,
            )),
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
    state.step()?;

    state.enter_depth(|state| match (kind, expected) {
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
        (TermKind::Pack { witness, body_proof }, Prop::Exists(pred)) => {
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
                if gamma.iter().any(|(opt, _)| opt.as_deref() == Some(name.as_str())) {
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
        (TermKind::Lam { var_name, body }, Prop::Impl(param_prop, result_prop)) => {
            gamma.push((var_name.clone(), *param_prop.clone()));
            let res = check_type(state, gamma, body, result_prop);
            gamma.pop();
            res
        }

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
                if gamma.iter().any(|(opt, _)| opt.as_deref() == Some(name.as_str())) {
                    return Err(Verdict::Malformed(format!(
                        "Eigenvariable freshness condition failed: '{name}' already present in context"
                    )));
                }
            }
            if let Some(ref name) = right_var {
                if gamma.iter().any(|(opt, _)| opt.as_deref() == Some(name.as_str())) {
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
    })
}

/// Infer/synthesize the type of `kind` in context `gamma`.
fn infer_type(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    kind: &TermKind,
) -> Result<Prop, Verdict> {
    state.step()?;

    state.enter_depth(|state| match kind {
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
        TermKind::Subst { eq, motive, sub } => {
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

        // (∃E) Existential Unpack synthesis
        TermKind::Unpack {
            scrutinee,
            obj_var,
            proof_var,
            body,
        } => {
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
                if gamma.iter().any(|(opt, _)| opt.as_deref() == Some(name.as_str())) {
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

        // (Trans-Pres) Transformation Preservation
        TermKind::Pres {
            realizes,
            preserves,
            motive,
            sub,
        } => {
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

            let expected_preserves_type = Prop::Preserves(w.clone(), motive.clone());
            check_type(state, gamma, preserves, &expected_preserves_type)?;

            let expected_sub_type = instantiate(motive, &x);
            check_type(state, gamma, sub, &expected_sub_type)?;

            Ok(instantiate(motive, &y))
        }

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
        TermKind::RealizesComp { left, right } => {
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

            if y != ys {
                return Err(Verdict::Rejected(RejectionReason::TypeMismatch {
                    expected: format!("Middle endpoint matching {y:?}"),
                    found: format!("{ys:?}"),
                }));
            }

            Ok(Prop::Realizes(
                ObjectTerm::Compose(Box::new(g2), Box::new(g1)),
                xl,
                z2,
            ))
        }

        // (RealizesTensor) Realization Tensor (Profile 1.2)
        TermKind::RealizesTensor { left, right } => {
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

            Ok(Prop::Realizes(
                ObjectTerm::Tensor(Box::new(w1), Box::new(w2)),
                ObjectTerm::Tensor(Box::new(x1), Box::new(x2)),
                ObjectTerm::Tensor(Box::new(y1), Box::new(y2)),
            ))
        }

        // Out-of-slice unsupported construct placeholder
        TermKind::Unsupported(msg) => Err(Verdict::Unsupported(UnsupportedConstruct::Construct(
            msg.clone(),
        ))),

        _ => Err(Verdict::Rejected(RejectionReason::ProofGoalNotReached)),
    })
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
