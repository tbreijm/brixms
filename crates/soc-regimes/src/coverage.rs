//! Coverage certification (ADR-0011 slices 4–5): **certified exhaustiveness**.
//!
//! Ordinary `match` earns a structural (Audited) coverage result. `match …
//! proving exhaustive` must cross a genuine proof boundary: here we build a
//! **coproduct-eliminator proof term** for the proposition
//!
//! ```text
//!   (P₀ → R) → (P₁ → R) → … → (Pₙ₋₁ → R) → (S → R)
//! ```
//!
//! where `S = P₀ + P₁ + … + Pₙ₋₁` is the scrutinee sum (one abstract atom per
//! variant) and `R` is an abstract result. The proof term is
//! `λh₀…λhₙ₋₁. λs. case s of …`, routing each variant leaf to its variant's
//! handler hypothesis. The **proof kernel independently type-checks** this term:
//! it accepts iff every variant leaf can be routed to a matching-variant handler
//! — i.e. iff the patterns cover every variant. A missing variant leaves a leaf
//! with no handler to apply, so the term cannot be built and the kernel rejects.
//!
//! **Exhaustiveness is provability.** `@Proven` lands on the coverage
//! proposition only (never the match's result type or arm-body correctness), and
//! anything outside the certified fragment returns [`CoverageOutcome::Unknown`]
//! — never a silent structural pass presented as proof.
//!
//! Certified fragment (this slice): a match whose arms are exactly one
//! `Ctor(variant)` pattern per variant of a closed nominal sum, with `Var`/`_`
//! sub-patterns. Wildcard/`Var` catch-alls, duplicates, missing variants, and
//! nested constructor patterns return `Unknown` until their certificate rules
//! exist.

use brix_kernel::{acceptance, Budget, ExplicitTerm, Prop, TermKind, Var, Verdict};
use brix_semantic::{ContextId, PropositionId};

use crate::type_realization::{Expr, Pattern, Ty};

/// The outcome of attempting to kernel-certify a match's exhaustiveness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageOutcome {
    /// The proof kernel independently accepted the coverage certificate — the
    /// coverage proposition is a theorem.
    Proven,
    /// Not certified: outside the certified fragment, or the kernel did not
    /// accept. **Never** a silent structural pass; carries a human reason.
    Unknown(String),
}

/// The abstract proposition atom standing for variant `variant` of sum `sum`.
fn variant_atom(sum: &str, variant: &str) -> Prop {
    Prop::Atom(PropositionId::from_canon(
        format!("brix.coverage.variant:{sum}:{variant}").as_bytes(),
    ))
}

/// The abstract result proposition `R` for a coverage claim over `sum`.
fn result_atom(sum: &str) -> Prop {
    Prop::Atom(PropositionId::from_canon(
        format!("brix.coverage.result:{sum}").as_bytes(),
    ))
}

/// Right-nested coproduct `P₀ + (P₁ + (… + Pₙ₋₁))` (atoms non-empty).
fn nested_sum(atoms: &[Prop]) -> Prop {
    let mut iter = atoms.iter().rev();
    let mut acc = iter.next().expect("non-empty").clone();
    for a in iter {
        acc = Prop::Sum(Box::new(a.clone()), Box::new(acc));
    }
    acc
}

/// The eliminator body for `atoms[start..]` bound to variable `sv`
/// (whose type is `nested_sum(atoms[start..])`): route variant leaf `i` to the
/// handler hypothesis of the arm covering it (`handler_for[i] = Some(arm)`), or
/// — for an **uncovered** variant — to [`TermKind::Unsupported`], which the
/// kernel rejects. The kernel, not this function, decides exhaustiveness.
fn build_eliminator(n: usize, start: usize, sv: &str, handler_for: &[Option<usize>]) -> TermKind {
    let leaf = |var: &str, i: usize| -> TermKind {
        match handler_for[i] {
            Some(arm) => TermKind::App {
                function: Box::new(TermKind::Hyp(Var::Named(format!("h{arm}")))),
                argument: Box::new(TermKind::Hyp(Var::Named(var.to_string()))),
            },
            None => TermKind::Unsupported(format!("no arm covers variant index {i}")),
        }
    };
    if n - start == 1 {
        // sv : P{start}.
        return leaf(sv, start);
    }
    // sv : P{start} + rest.
    let left_var = format!("x{start}");
    let left_body = leaf(&left_var, start);
    let right_var = format!("r{start}");
    let right_body = build_eliminator(n, start + 1, &right_var, handler_for);
    TermKind::Case {
        discriminant: Box::new(TermKind::Hyp(Var::Named(sv.to_string()))),
        left_var: Some(left_var),
        left_body: Box::new(left_body),
        right_var: Some(right_var),
        right_body: Box::new(right_body),
    }
}

/// Attempt to kernel-certify that `arms` exhaustively cover `sum_ty`.
///
/// Returns [`CoverageOutcome::Proven`] only if the proof kernel independently
/// accepts the coverage certificate; otherwise [`CoverageOutcome::Unknown`]
/// with a reason. Never returns `Proven` on a structural check alone.
pub fn certify_exhaustive(
    sum_ty: &Ty,
    arms: &[(Pattern, Expr)],
    context: ContextId,
    budget: Budget,
) -> CoverageOutcome {
    // Unfolded first: a recursive scrutinee is a `Rec`, which carries no
    // variants of its own, and reading its shape directly would report "not a
    // sum" for a type that plainly is one.
    let unfolded = sum_ty.unfold();
    let (sum_name, variants) = match &unfolded {
        Ty::Sum(name, vs) => (name, vs),
        _ => return CoverageOutcome::Unknown("scrutinee is not a sum type".into()),
    };
    if variants.is_empty() {
        return CoverageOutcome::Unknown("empty sum".into());
    }

    // Certified fragment: every arm is a Ctor pattern of a real variant, with
    // Var/Wildcard sub-patterns only. Anything else → Unknown (not certified).
    for (pat, _) in arms {
        match pat {
            Pattern::Ctor(vname, subs) => {
                if !variants.iter().any(|(n, _)| n == vname) {
                    return CoverageOutcome::Unknown(format!("unknown variant '{vname}'"));
                }
                if !subs
                    .iter()
                    .all(|s| matches!(s, Pattern::Wildcard | Pattern::Var(_)))
                {
                    return CoverageOutcome::Unknown(
                        "nested constructor patterns are outside the certified fragment".into(),
                    );
                }
            }
            Pattern::Wildcard | Pattern::Var(_) => {
                return CoverageOutcome::Unknown(
                    "wildcard / catch-all patterns are outside the certified fragment yet".into(),
                );
            }
        }
    }

    // NB: exhaustiveness is NOT decided here — it is the kernel's call. We build
    // one handler hypothesis PER ARM (typed by the variant that arm matches) and
    // a term routing each variant leaf to its covering arm; an uncovered variant
    // routes to `Unsupported`, so the kernel rejects a non-exhaustive match. This
    // never falls back to a structural pass.
    let r = result_atom(sum_name);
    let atoms: Vec<Prop> = variants
        .iter()
        .map(|(v, _)| variant_atom(sum_name, v))
        .collect();
    let s_prop = nested_sum(&atoms);
    let n = atoms.len();

    // The variant index each arm matches (arms are Ctor(variant) — checked above).
    let arm_variant: Vec<usize> = arms
        .iter()
        .map(|(p, _)| match p {
            Pattern::Ctor(vname, _) => variants
                .iter()
                .position(|(v, _)| v == vname)
                .expect("variant validated above"),
            _ => unreachable!("fragment check rejects non-Ctor arms"),
        })
        .collect();

    // The certified fragment has exactly one handler premise per variant. A
    // duplicate arm is ordinary-match syntax but is not the canonical coverage
    // proof shape, so it must not obtain a certificate by silently ignoring a
    // handler below.
    for (arm, &variant) in arm_variant.iter().enumerate() {
        if arm_variant[..arm].contains(&variant) {
            return CoverageOutcome::Unknown(format!(
                "duplicate arm for variant '{}' is outside the certified fragment",
                variants[variant].0
            ));
        }
    }

    // For each variant, the first arm that covers it (None ⇒ uncovered).
    let handler_for: Vec<Option<usize>> = (0..n)
        .map(|i| arm_variant.iter().position(|&vi| vi == i))
        .collect();

    // Proposition: H₀ → … → H_{m-1} → (S → R), Hₐ = P_{variant(a)} → R.
    let mut prop = Prop::Impl(Box::new(s_prop), Box::new(r.clone()));
    for &vi in arm_variant.iter().rev() {
        let ha = Prop::Impl(Box::new(atoms[vi].clone()), Box::new(r.clone()));
        prop = Prop::Impl(Box::new(ha), Box::new(prop));
    }

    // Term: λh₀ … λh_{m-1}. λs. <eliminator>.
    let mut term = TermKind::Lam {
        var_name: Some("s".to_string()),
        body: Box::new(build_eliminator(n, 0, "s", &handler_for)),
    };
    for a in (0..arms.len()).rev() {
        term = TermKind::Lam {
            var_name: Some(format!("h{a}")),
            body: Box::new(term),
        };
    }

    let explicit = ExplicitTerm::new(context, term);
    match acceptance(&context, &prop, &explicit, budget) {
        Verdict::Accepted(_) => CoverageOutcome::Proven,
        other => CoverageOutcome::Unknown(format!("kernel did not accept: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_realization::{Expr, Pattern, Ty};

    fn opt_ty() -> Ty {
        Ty::Sum(
            "Opt".to_string(),
            vec![
                ("None".to_string(), vec![]),
                ("Some".to_string(), vec![Ty::Con("Int")]),
            ],
        )
    }

    fn arm(variant: &str, subs: Vec<Pattern>) -> (Pattern, Expr) {
        (Pattern::Ctor(variant.to_string(), subs), Expr::Lit(0))
    }

    #[test]
    fn exhaustive_two_variant_match_is_kernel_certified() {
        let arms = vec![
            arm("None", vec![]),
            arm("Some", vec![Pattern::Var("k".into())]),
        ];
        let out = certify_exhaustive(
            &opt_ty(),
            &arms,
            ContextId::root(),
            Budget::new(10_000, 10_000),
        );
        assert_eq!(
            out,
            CoverageOutcome::Proven,
            "exhaustive match must be Proven"
        );
    }

    #[test]
    fn missing_variant_is_not_certified() {
        // Only None — Some is uncovered. Must NOT be Proven.
        let arms = vec![arm("None", vec![])];
        let out = certify_exhaustive(
            &opt_ty(),
            &arms,
            ContextId::root(),
            Budget::new(10_000, 10_000),
        );
        // Crucially, the KERNEL must be the one that rejects it (no structural
        // "good enough" fallback): the reason comes from kernel non-acceptance.
        match out {
            CoverageOutcome::Unknown(reason) => assert!(
                reason.contains("kernel did not accept"),
                "a missing variant must be rejected by the kernel, not a structural gate; got: {reason}"
            ),
            CoverageOutcome::Proven => panic!("a missing variant must never be Proven"),
        }
    }

    #[test]
    fn duplicate_variant_is_not_certified() {
        let arms = vec![
            arm("None", vec![]),
            arm("Some", vec![Pattern::Var("k".into())]),
            arm("Some", vec![Pattern::Wildcard]),
        ];
        let out = certify_exhaustive(
            &opt_ty(),
            &arms,
            ContextId::root(),
            Budget::new(10_000, 10_000),
        );
        assert!(matches!(
            out,
            CoverageOutcome::Unknown(reason) if reason.contains("duplicate arm for variant 'Some'")
        ));
    }

    #[test]
    fn wildcard_is_outside_the_certified_fragment() {
        let arms = vec![arm("None", vec![]), (Pattern::Wildcard, Expr::Lit(0))];
        let out = certify_exhaustive(
            &opt_ty(),
            &arms,
            ContextId::root(),
            Budget::new(10_000, 10_000),
        );
        assert!(
            matches!(out, CoverageOutcome::Unknown(_)),
            "wildcard is not certified yet, got {out:?}"
        );
    }

    #[test]
    fn three_variant_exhaustive_is_certified() {
        let sign = Ty::Sum(
            "Sign".to_string(),
            vec![
                ("Neg".to_string(), vec![]),
                ("Zero".to_string(), vec![]),
                ("Pos".to_string(), vec![]),
            ],
        );
        let arms = vec![arm("Neg", vec![]), arm("Zero", vec![]), arm("Pos", vec![])];
        let out = certify_exhaustive(&sign, &arms, ContextId::root(), Budget::new(10_000, 10_000));
        assert_eq!(out, CoverageOutcome::Proven);
    }
}
