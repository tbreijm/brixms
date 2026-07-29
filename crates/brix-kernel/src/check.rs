//! Total, strictly-terminating bidirectional proof-term acceptance checker (ADR-0003 §4, §5).

use brix_semantic::{CertificateId, ContextId, PropositionId, VerifierId};

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
        Ok(()) => {
            let cert_payload = format!("{context:?}:{proposition:?}:{term:?}");
            let certificate_id = CertificateId::from_canon(cert_payload.as_bytes());
            let verifier = VerifierId::named("brix.kernel@0.1");

            Verdict::Accepted(Certificate {
                verifier,
                certificate_id,
            })
        }
        Err(verdict) => verdict,
    }
}

fn obj_term_contains(term: &ObjectTerm, target: &ObjectTerm) -> bool {
    if term == target {
        return true;
    }
    if let ObjectTerm::Compose(g2, g1) = term {
        return obj_term_contains(g2, target) || obj_term_contains(g1, target);
    }
    false
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
    use brix_semantic::{compose_chain, GeneratorId};

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
}
