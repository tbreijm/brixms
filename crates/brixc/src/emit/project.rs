//! `Lowered` (real, lowered + checked Core IR) -> `emit`'s codegen-input
//! descriptors (`RelationDesc`/`RuleDesc`). Relations are read from
//! `Lowered.resolver.relations()` (the schema tables — `FrontendSource`
//! itself has no relations field, by design: brix-ir never invents
//! schema), rules from `Lowered.source.rules`.

use brix_ir::core::Rule;
use brix_ir::frontend::RelationSchema;
use brix_ir::pattern::ReadKind;

use crate::lower::Lowered;

use super::rust_type::rust_type_of;
use super::{ColumnDesc, RelationDesc, RuleDesc};

/// Project a fully lowered program into `emit`'s codegen descriptors.
/// Deterministic: relations come from the resolver's canonical
/// (`QualIdent`-sorted) order, columns from role declaration order, and
/// rule delta sources from `Pattern::read_set`'s canonically sorted output
/// — never from hash-map iteration order.
pub fn project(lowered: &Lowered) -> (Vec<RelationDesc>, Vec<RuleDesc>) {
    let relations = lowered.resolver.relations().map(project_relation).collect();
    let rules = lowered.source.rules.iter().map(project_rule).collect();
    (relations, rules)
}

fn project_relation(schema: &RelationSchema) -> RelationDesc {
    RelationDesc {
        name: schema.name.to_string(),
        columns: schema
            .roles
            .iter()
            .map(|(name, ty)| ColumnDesc {
                name: name.to_string(),
                rust_type: rust_type_of(ty),
            })
            .collect(),
        key: schema.key.iter().map(|k| k.to_string()).collect(),
    }
}

fn project_rule(rule: &Rule) -> RuleDesc {
    RuleDesc {
        name: rule.name.to_string(),
        delta_sources: rule
            .body
            .read_set(&[])
            .into_iter()
            // `History` reads bypass masks/supersession and drive no
            // semi-naive re-evaluation (Part III §6.3) — the same
            // exclusion brix-phase's own adapter applies
            // (crates/brixc/src/phase.rs).
            .filter(|r| r.kind != ReadKind::History)
            .map(|r| r.relation.to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::lower_file;
    use brix_ast::parse_file;

    fn lower_flagship() -> Lowered {
        let source = include_str!(
            "../../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix"
        );
        let (file, parse_diags) = parse_file(source);
        lower_file(&file, &parse_diags)
    }

    #[test]
    fn flagship_relations_and_rules_project_without_panicking() {
        let lowered = lower_flagship();
        assert!(!lowered.has_errors());
        let (relations, rules) = project(&lowered);
        assert!(!relations.is_empty());
        assert!(!rules.is_empty());
    }

    #[test]
    fn projection_is_deterministic() {
        let lowered = lower_flagship();
        let (rel_a, rule_a) = project(&lowered);
        let (rel_b, rule_b) = project(&lowered);
        assert_eq!(rel_a.len(), rel_b.len());
        for (a, b) in rel_a.iter().zip(rel_b.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.key, b.key);
        }
        assert_eq!(rule_a.len(), rule_b.len());
    }
}
