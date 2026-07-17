//! Whole-declaration checks that compose the per-module analyses against a
//! [`SchemaResolver`] (Appendix E judgments that need Σ). These are the checks
//! that *can* run before a full frontend — they need only resolved schema
//! facts, which [`crate::frontend::TableResolver`] supplies today.

use crate::core::Rule;
use crate::frontend::{RelationSchema, SchemaResolver};
use crate::ident::{Ident, QualIdent};
use crate::pattern::{ReadKind, RelRead};
use crate::types::{check_key_canonical, KeyCanonicalError};
use core::fmt;

/// A static-semantics finding. brix-ir emits these as structured values; the
/// diag lane renders them (this crate does not depend on brix-diag to keep the
/// dependency graph thin — the finding is plain data).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Finding {
    /// A key role's type is not `Canonical` in a key position (Appendix E
    /// `Key`; App. G float-in-key rule).
    NonCanonicalKey {
        relation: QualIdent,
        role: Ident,
        cause: KeyCanonicalError,
    },
    /// A `without`/`optional` absence read over an `open` relation with no
    /// completeness witness in scope (Part III §7; Appendix E `Without`).
    AbsenceWithoutWitness { relation: QualIdent, in_rule: Ident },
    /// A relation named in a pattern has no schema (name resolution gap).
    UnknownRelation { relation: QualIdent, in_rule: Ident },
    /// A rule that reads a graph-derived relation through an ordinary (non-
    /// aggregate) function inside its body would defeat stratification
    /// (Part IV §4 / Appendix E `Ordinary fn`). Represented; the actual call
    /// sites come from the caller's aggregate-read list, so this is raised when
    /// a derived relation is consumed non-strictly by such a call.
    OrdinaryFnOnDerivedRel { relation: QualIdent, in_rule: Ident },
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Finding::NonCanonicalKey {
                relation,
                role,
                cause,
            } => write!(f, "non-Canonical key: {relation}.{role}: {cause}"),
            Finding::AbsenceWithoutWitness { relation, in_rule } => write!(
                f,
                "absence read of open relation {relation} in rule {in_rule} needs a Complete witness"
            ),
            Finding::UnknownRelation { relation, in_rule } => {
                write!(f, "unknown relation {relation} in rule {in_rule}")
            }
            Finding::OrdinaryFnOnDerivedRel { relation, in_rule } => write!(
                f,
                "ordinary fn consumes graph-derived {relation} in rule {in_rule} (use an aggregate fn)"
            ),
        }
    }
}

/// Canonical-in-key checking over a relation schema (Appendix E `Key`). Each
/// key role's declared type must pass [`check_key_canonical`]. This is the
/// check brix-ir owns end-to-end (it needs only brix-canon + the schema).
pub fn check_relation_keys(schema: &RelationSchema) -> Vec<Finding> {
    let mut out = Vec::new();
    for key_role in &schema.key {
        let Some((_, ty)) = schema.roles.iter().find(|(r, _)| r == key_role) else {
            // A key naming a non-existent role is a frontend/name-resolution
            // error, not a canon error; skip (the frontend reports it).
            continue;
        };
        if let Err(errs) = check_key_canonical(ty) {
            for cause in errs {
                out.push(Finding::NonCanonicalKey {
                    relation: schema.name.clone(),
                    role: key_role.clone(),
                    cause,
                });
            }
        }
    }
    out
}

/// Rule-level absence/witness check (Appendix E `Without/Optional`): every
/// strict (absence) read of an `open` relation needs a completeness witness.
/// Uses the rule's classified read-set, so it composes the pattern analysis.
pub fn check_rule_absence(rule: &Rule, resolver: &impl SchemaResolver) -> Vec<Finding> {
    let mut out = Vec::new();
    for RelRead { relation, kind } in rule.body.read_set(&[]) {
        if kind != ReadKind::Strict {
            continue;
        }
        match resolver.relation(&relation) {
            None => out.push(Finding::UnknownRelation {
                relation: relation.clone(),
                in_rule: rule.name.clone(),
            }),
            Some(schema) => {
                if !schema.model_closed && !resolver.has_completeness_witness(&relation) {
                    out.push(Finding::AbsenceWithoutWitness {
                        relation: relation.clone(),
                        in_rule: rule.name.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Run all schema-dependent checks over one rule.
pub fn check_rule(rule: &Rule, resolver: &impl SchemaResolver) -> Vec<Finding> {
    check_rule_absence(rule, resolver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Head, Rule};
    use crate::effects::EffectRow;
    use crate::frontend::TableResolver;
    use crate::pattern::{edge, Clause, Pattern};
    use crate::types::Ty;

    fn rel(name: &str, roles: Vec<(&str, Ty)>, key: &[&str], model_closed: bool) -> RelationSchema {
        RelationSchema {
            name: QualIdent::from(name),
            roles: roles.into_iter().map(|(r, t)| (Ident::new(r), t)).collect(),
            key: key.iter().map(|k| Ident::new(*k)).collect(),
            model_closed,
            derived: false,
        }
    }

    #[test]
    fn float_key_role_is_flagged() {
        let schema = rel(
            "ComputedPrice",
            vec![
                ("order", Ty::NodeRef(Ident::new("Order"))),
                ("amount", Ty::F64),
            ],
            &["amount"],
            true,
        );
        let findings = check_relation_keys(&schema);
        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0], Finding::NonCanonicalKey { .. }));
    }

    #[test]
    fn canonical_key_role_passes() {
        let schema = rel(
            "Order",
            vec![("id", Ty::EventId), ("amount", Ty::F64)],
            &["id"],
            true,
        );
        assert!(check_relation_keys(&schema).is_empty());
    }

    #[test]
    fn without_over_open_relation_without_witness_is_flagged() {
        let resolver = TableResolver::new()
            .with_relation(rel("Order", vec![("id", Ty::EventId)], &["id"], true))
            .with_relation(rel(
                "Delivered",
                vec![("order", Ty::NodeRef(Ident::new("Order")))],
                &["order"],
                false, // open / not model-closed
            ));
        let rule = Rule {
            name: Ident::new("Dun"),
            head: Head::Tuple {
                relation: QualIdent::from("Overdue"),
                args: vec![],
            },
            body: Pattern::new(vec![
                edge("Order", &[("id", "o")]),
                Clause::Without(Pattern::new(vec![edge("Delivered", &[("order", "o")])])),
            ]),
            effects: EffectRow::empty(),
        };
        let findings = check_rule(&rule, &resolver);
        assert!(findings
            .iter()
            .any(|f| matches!(f, Finding::AbsenceWithoutWitness { .. })));
    }

    #[test]
    fn without_over_open_relation_with_witness_passes() {
        let resolver = TableResolver::new()
            .with_relation(rel("Order", vec![("id", Ty::EventId)], &["id"], true))
            .with_relation(rel(
                "Delivered",
                vec![("order", Ty::NodeRef(Ident::new("Order")))],
                &["order"],
                false,
            ))
            .with_witness(QualIdent::from("Delivered"));
        let rule = Rule {
            name: Ident::new("Dun"),
            head: Head::Tuple {
                relation: QualIdent::from("Overdue"),
                args: vec![],
            },
            body: Pattern::new(vec![
                edge("Order", &[("id", "o")]),
                Clause::Without(Pattern::new(vec![edge("Delivered", &[("order", "o")])])),
            ]),
            effects: EffectRow::empty(),
        };
        assert!(check_rule(&rule, &resolver).is_empty());
    }

    #[test]
    fn without_over_model_closed_relation_needs_no_witness() {
        let resolver = TableResolver::new()
            .with_relation(rel("Order", vec![("id", Ty::EventId)], &["id"], true))
            .with_relation(rel(
                "Delivered",
                vec![("order", Ty::NodeRef(Ident::new("Order")))],
                &["order"],
                true, // model-closed: `without` is sound with no ceremony
            ));
        let rule = Rule {
            name: Ident::new("Dun"),
            head: Head::Tuple {
                relation: QualIdent::from("Overdue"),
                args: vec![],
            },
            body: Pattern::new(vec![
                edge("Order", &[("id", "o")]),
                Clause::Without(Pattern::new(vec![edge("Delivered", &[("order", "o")])])),
            ]),
            effects: EffectRow::empty(),
        };
        assert!(check_rule(&rule, &resolver).is_empty());
    }
}
