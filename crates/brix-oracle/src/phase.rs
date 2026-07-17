//! Phase inference — a thin adapter from the oracle's `Program` into
//! `brix-phase`'s lane-neutral `RuleFacts` input, delegating the actual
//! Appendix F algorithm (including the errata-0002 predicate-level
//! condensation fix) to `brix_phase::infer_phases`. See
//! `crates/brix-phase/src/lib.rs` for the algorithm itself; this module is
//! only the `Program -> RuleFacts` projection.

use crate::program::{Clause, Expr, Head, Program, Rule};

pub use brix_phase::{Phase, PhaseError};

/// Run Appendix F over `program` and return the phase order.
pub fn infer_phases(program: &Program) -> Result<Vec<Phase>, PhaseError> {
    brix_phase::infer_phases(&to_rule_facts(program))
}

fn to_rule_facts(program: &Program) -> Vec<brix_phase::RuleFacts> {
    program.rules.values().map(rule_facts).collect()
}

fn rule_facts(rule: &Rule) -> brix_phase::RuleFacts {
    let mask_target: Option<(&str, &str)> = match &rule.head {
        Head::Mask {
            relation, target, ..
        } => Some((relation.as_str(), target.as_str())),
        _ => None,
    };
    let produces = match &rule.head {
        Head::Tuple { rel, .. } => brix_phase::Produces::Relation(rel.clone()),
        Head::Mask { relation, .. } => brix_phase::Produces::Mask {
            relation: relation.clone(),
        },
    };
    let mut reads = Vec::new();
    walk_clauses(&rule.body, false, mask_target, &mut reads);
    brix_phase::RuleFacts {
        id: rule.id.clone(),
        produces,
        reads,
    }
}

/// Collect every relation this rule's body reads, with enough detail to
/// build both the positive/strict edges and the mask edges. `History`
/// clauses are walked but contribute nothing (Appendix F #3, Part III §6
/// rule 3: no dependency).
fn walk_clauses(
    clauses: &[Clause],
    strict: bool,
    mask_target: Option<(&str, &str)>,
    out: &mut Vec<brix_phase::ReadSite>,
) {
    for c in clauses {
        match c {
            Clause::Edge { rel, bind_id, .. } => {
                let is_mask_target = mask_target
                    .map(|(mrel, mvar)| mrel == rel.as_str() && bind_id.as_deref() == Some(mvar))
                    .unwrap_or(false);
                out.push(brix_phase::ReadSite {
                    relation: rel.clone(),
                    strict,
                    is_mask_target,
                });
            }
            Clause::Without(inner) => walk_clauses(inner, true, mask_target, out),
            Clause::History { .. } => {}
            Clause::When(e) => walk_expr(e, out),
            Clause::Let(_, e) => walk_expr(e, out),
        }
    }
}

fn walk_expr(e: &Expr, out: &mut Vec<brix_phase::ReadSite>) {
    match e {
        Expr::Var(_) | Expr::Const(_) => {}
        Expr::BinOp(_, a, b) => {
            walk_expr(a, out);
            walk_expr(b, out);
        }
        Expr::Call(_, args) | Expr::Try(_, args) => {
            for a in args {
                walk_expr(a, out);
            }
        }
        Expr::Count(clauses) => walk_clauses(clauses, true, None, out),
        Expr::Sum(clauses, yield_expr) => {
            walk_clauses(clauses, true, None, out);
            walk_expr(yield_expr, out);
        }
    }
}
