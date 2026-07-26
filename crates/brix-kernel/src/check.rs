//! Total, strictly-terminating bidirectional proof-term acceptance checker (ADR-0003 §4, §5).

use brix_semantic::{CertificateId, ContextId, VerifierId};

use crate::term::{ExplicitTerm, Prop, TermKind, Var};
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

/// Check if `kind` has type `expected` in context `gamma`.
fn check_type(
    state: &mut CheckerState,
    gamma: &mut Gamma,
    kind: &TermKind,
    expected: &Prop,
) -> Result<(), Verdict> {
    state.step()?;

    state.enter_depth(|state| match (kind, expected) {
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

        // Out-of-slice unsupported construct placeholder
        TermKind::Unsupported(msg) => Err(Verdict::Unsupported(UnsupportedConstruct::Construct(
            msg.clone(),
        ))),

        _ => Err(Verdict::Rejected(RejectionReason::ProofGoalNotReached)),
    })
}
