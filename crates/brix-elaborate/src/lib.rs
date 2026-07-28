//! Elaboration bridge for BrixMS (Stage A & Stage B2).
//!
//! Bridges candidate proof terms produced by realization regimes into published
//! [`brix_semantic::Outcome::Proven`] judgements across an elaboration boundary.
//!
//! Soundness invariant: ONLY kernel acceptance (`brix_kernel::acceptance` returning
//! `Verdict::Accepted`) mints a `Proven` judgement. No other code path exists.

use brix_kernel::{ExplicitTerm, ObjectTerm, Prop, TermKind, Var};
use brix_semantic::{
    Decomposition, Dependency, EdgeKind, Evidence, Judgement, Outcome, PropositionId,
};

/// Result of attempting to elaborate and publish a proof term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElaborationResult {
    /// acceptance returned Accepted: a Proven judgement + the elaboration-boundary edge linking it to the source.
    Proven {
        judgement: Judgement,
        edge: Dependency,
    },
    /// any non-Accepted verdict: NO Proven produced (carry the verdict for diagnosis).
    NotElaborated(brix_kernel::Verdict),
}

/// Elaborate a candidate proof term against a proposition via `brix_kernel::acceptance`
/// and, upon acceptance, publish a [`Outcome::Proven`] judgement linked to `source` via an
/// [`EdgeKind::ElaborationBoundary`] dependency edge.
///
/// Soundness-critical semantics:
/// 1. Calls `brix_kernel::acceptance(&source.context, proposition, term, budget)`.
/// 2. ONLY if it returns `Verdict::Accepted(certificate)`: construct a NEW Judgement with
///    the SAME context and proposition as `source`, `Outcome::Proven`, and Evidence =
///    `Evidence::KernelCertificate` wrapping that certificate. Also construct a `Dependency`
///    with `EdgeKind::ElaborationBoundary` FROM the new Proven judgement's id TO the source
///    judgement's id.
/// 3. For EVERY other verdict: return `ElaborationResult::NotElaborated(verdict)`.
pub fn elaborate_and_publish(
    source: &Judgement,
    proposition: &brix_kernel::Prop,
    term: &brix_kernel::ExplicitTerm,
    budget: brix_kernel::Budget,
) -> ElaborationResult {
    match brix_kernel::acceptance(&source.context, proposition, term, budget) {
        brix_kernel::Verdict::Accepted(certificate) => {
            let evidence = Evidence::KernelCertificate {
                verifier: certificate.verifier,
                certificate: certificate.certificate_id,
            };
            let judgement = Judgement::new(
                source.context,
                source.proposition,
                Outcome::Proven,
                evidence.id(),
            );
            let edge = Dependency::new(EdgeKind::ElaborationBoundary, source.id().digest());
            ElaborationResult::Proven { judgement, edge }
        }
        verdict => ElaborationResult::NotElaborated(verdict),
    }
}

/// Elaborate an Audited settlement [`Decomposition`] into a kernel-proved implication
/// proposition and publish a [`Outcome::Proven`] judgement (Stage B2).
///
/// Steps:
/// 1. Extract ordered generators g_1..g_n and intermediate configs x_0..x_n from `decomposition`.
///    Build per-step antecedent propositions H_i = Prop::Realizes(Const(g_i), Const(x_{i-1}), Const(x_i)).
/// 2. Build the left-nested composite witness object term:
///    k_term = compose(g_n, compose(g_{n-1}, ... compose(g_2, g_1)...)) using ObjectTerm::Compose (outer, inner).
///    For n=1, k_term = Const(g_1).
/// 3. Build the closed implication proposition: H_1 -> H_2 -> ... -> H_n -> Realizes(k_term, x_0, x_n)
///    (right-associated Prop::Impl nesting).
/// 4. Build the proof term: n nested Lams binding h_1..h_n (Hyp de Bruijn index for h_i is n - i),
///    whose body is the left-nested RealizesComp fold over hypotheses h_1..h_n.
/// 5. Delegate to [`elaborate_and_publish`].
pub fn elaborate_decomposition(
    source: &Judgement,
    decomposition: &Decomposition,
    budget: brix_kernel::Budget,
) -> ElaborationResult {
    let n = decomposition.generators.len();
    if n == 0 {
        return ElaborationResult::NotElaborated(brix_kernel::Verdict::Rejected(
            brix_kernel::RejectionReason::Custom("Empty decomposition".into()),
        ));
    }

    let generators = &decomposition.generators;
    let configs = &decomposition.configs;

    // 1. Build per-step antecedent propositions H_i = Realizes(Const(g_i), Const(x_{i-1}), Const(x_i))
    let mut h_props = Vec::with_capacity(n);
    for i in 0..n {
        let g_term = ObjectTerm::Const(PropositionId(generators[i].digest()));
        let x_prev = ObjectTerm::Const(PropositionId(configs[i].digest()));
        let x_curr = ObjectTerm::Const(PropositionId(configs[i + 1].digest()));
        h_props.push(Prop::Realizes(g_term, x_prev, x_curr));
    }

    // 2. Build composite witness k_term = compose(g_n, compose(g_{n-1}, ... compose(g_2, g_1)...))
    let mut k_term = ObjectTerm::Const(PropositionId(generators[0].digest()));
    for i in 1..n {
        let g_i_term = ObjectTerm::Const(PropositionId(generators[i].digest()));
        k_term = ObjectTerm::Compose(Box::new(g_i_term), Box::new(k_term));
    }

    let x_0 = ObjectTerm::Const(PropositionId(configs[0].digest()));
    let x_n = ObjectTerm::Const(PropositionId(configs[n].digest()));
    let goal_prop = Prop::Realizes(k_term, x_0, x_n);

    // 3. Build closed implication proposition: H_1 -> H_2 -> ... -> H_n -> Realizes(k_term, x_0, x_n)
    let mut implication_prop = goal_prop;
    for h_i in h_props.into_iter().rev() {
        implication_prop = Prop::Impl(Box::new(h_i), Box::new(implication_prop));
    }

    // 4. Build proof term: n nested Lams binding h_1..h_n.
    // De Bruijn index mapping for h_i (1-indexed, i=1..n): index = n - i.
    let mut body = TermKind::Hyp(Var::Index(n - 1)); // h_1 has index n - 1
    for i in 1..n {
        let right_hyp = TermKind::Hyp(Var::Index(n - 1 - i)); // h_{i+1} has index n - 1 - i
        body = TermKind::RealizesComp {
            left: Box::new(body),
            right: Box::new(right_hyp),
        };
    }

    let mut kind = body;
    for i in (0..n).rev() {
        kind = TermKind::Lam {
            var_name: Some(format!("h{}", i + 1)),
            body: Box::new(kind),
        };
    }

    let term = ExplicitTerm::new(source.context, kind);

    // 5. Call existing elaborate_and_publish
    elaborate_and_publish(source, &implication_prop, &term, budget)
}
