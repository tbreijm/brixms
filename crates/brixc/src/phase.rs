//! The `phase` pipeline seam (issue #7's `brixc`-wiring requirement):
//! `brix-phase` is the phase authority — this module is only the
//! `Lowered -> Vec<brix_phase::RuleFacts>` adapter and the `Phased`
//! artifact the `plan`/`emit` stages consume next. Never depends on
//! `brix-oracle`; never re-implements Appendix F here.

use brix_ir::core::Head;
use brix_ir::pattern::{Clause, Pattern, ReadKind};

use crate::lower::Lowered;
use crate::pipeline::{PhaseAssign, PipelineError, Stage};

/// Lowered Core IR plus its inferred phase order (Appendix F).
pub struct Phased {
    pub lowered: Lowered,
    pub phases: Vec<brix_phase::Phase>,
}

/// The real `PhaseAssign` seam, backed by `brix-phase`.
pub struct AstPhase;

impl PhaseAssign for AstPhase {
    type Ir = Lowered;
    type Phased = Phased;

    fn assign_phases(&self, ir: Self::Ir) -> Result<Self::Phased, PipelineError> {
        let facts = to_rule_facts(&ir);
        let phases =
            brix_phase::infer_phases(&facts).map_err(|error| PipelineError::Diagnostic {
                stage: Stage::Phase,
                diagnostic: Box::new(error.diagnostic()),
            })?;
        Ok(Phased {
            lowered: ir,
            phases,
        })
    }
}

fn to_rule_facts(lowered: &Lowered) -> Vec<brix_phase::RuleFacts> {
    lowered.source.rules.iter().map(rule_facts).collect()
}

fn rule_facts(rule: &brix_ir::core::Rule) -> brix_phase::RuleFacts {
    // A mask's `target` is an edge-ref bound somewhere in its own body
    // (Part III §6); resolve it to the relation it reads the same way the
    // pre-consolidation brix-oracle adapter did, retargeted to
    // `brix_ir::pattern::Clause`.
    let mask_target = match &rule.head {
        Head::Mask { target, .. } => Some(target.as_str()),
        _ => None,
    };
    let target_relation = mask_target.and_then(|t| resolve_target_relation(&rule.body, t));

    let produces = match &rule.head {
        Head::Tuple { relation, .. } => brix_phase::Produces::Relation(relation.to_string()),
        // A Skolem/derived-node head still derives into its entity relation
        // — an ordinary producer, same as a Tuple head.
        Head::Node { entity, .. } => brix_phase::Produces::Relation(entity.to_string()),
        Head::Mask { .. } => brix_phase::Produces::Mask {
            relation: target_relation.clone().unwrap_or_default(),
        },
    };

    // `read_set` classifies by (relation, kind) only, not by originating
    // clause — so a mask rule with more than one read of its own masked
    // relation (uncommon; canonical mask bodies read it exactly once, as
    // the target binding) would over-mark `is_mask_target`. Documented
    // narrow gap rather than a blocker, per the issue's guidance: closing
    // it precisely needs per-clause read-site identity `Pattern::read_set`
    // does not expose.
    let reads = rule
        .body
        .read_set(&[])
        .into_iter()
        .filter_map(|r| {
            let strict = match r.kind {
                ReadKind::Live | ReadKind::Exists => false,
                ReadKind::Strict => true,
                ReadKind::History => return None,
            };
            let is_mask_target = target_relation
                .as_deref()
                .is_some_and(|tr| tr == r.relation.to_string());
            Some(brix_phase::ReadSite {
                relation: r.relation.to_string(),
                strict,
                is_mask_target,
            })
        })
        .collect();

    brix_phase::RuleFacts {
        id: rule.name.to_string(),
        produces,
        reads,
    }
}

fn resolve_target_relation(body: &Pattern, target: &str) -> Option<String> {
    body.clauses.iter().find_map(|c| match c {
        Clause::Edge {
            bind: Some(b),
            relation,
            ..
        } if b.as_str() == target => Some(relation.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::lower_file;
    use brix_ast::parse_file;

    #[test]
    fn flagship_lowers_and_phase_assigns_with_no_cycle() {
        let source = include_str!(
            "../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix"
        );
        let (file, parse_diags) = parse_file(source);
        let lowered = lower_file(&file, &parse_diags);
        assert!(
            !lowered.has_errors(),
            "flagship must lower cleanly: {:?}",
            lowered.diags
        );

        let phased = AstPhase
            .assign_phases(lowered)
            .expect("flagship must be well-stratified");
        assert!(!phased.phases.is_empty());
    }
}
