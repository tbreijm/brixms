//! Authority-constraint generation sketch (Part XII §5).
//!
//! `authority` "compiles to generated constraints, not to a separate
//! mechanism" (Part XII §5). This module is the *shape* of that lowering: given
//! a policy `Y` with a declared authority mode, and the rules that consume `Y`'s
//! suggestions, produce the generated constraints / obligations the checker
//! enforces. It is a sketch (per the bounded deliverable): it constructs the
//! obligation values and can render the advisory-mode strict constraint as a
//! real [`Constraint`]; the gated/autonomous obligations are represented but
//! their pattern-rewrite into the consuming rule is left as a documented TODO.
//!
//! An "acting rule remains an ordinary rule" (Part XII §5) — so everything here
//! produces ordinary [`Constraint`]s and rule-shaped obligations, never a new
//! enforcement surface.

use crate::core::{Constraint, Rule, Severity};
use crate::ident::{Ident, QualIdent};
use crate::pattern::{Clause, Pattern, ReadKind};
use core::fmt;

/// The three authority modes (Part XII §5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Authority {
    /// `authority advisory` (default).
    Advisory,
    /// `authority gated by G` — `G` is the gate relation.
    Gated { gate: QualIdent },
    /// `authority autonomous within Scope` — `scope` names the protocols the
    /// consuming rule may drive.
    Autonomous { scope: Vec<QualIdent> },
}

impl fmt::Display for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Authority::Advisory => write!(f, "authority advisory"),
            Authority::Gated { gate } => write!(f, "authority gated by {gate}"),
            Authority::Autonomous { scope } => {
                write!(f, "authority autonomous within ")?;
                for (i, s) in scope.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{s}")?;
                }
                Ok(())
            }
        }
    }
}

/// A policy's authority declaration plus the identity of the sealed relations
/// its suggestions flow through. The frontend supplies this; brix-ir lowers it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PolicyAuthority {
    /// The policy name `Y` (e.g. `AssignVehicle`).
    pub policy: Ident,
    /// The relation carrying this policy's suggestions (e.g.
    /// `AssignmentAdvice`), the thing a consuming rule reads.
    pub suggestion_relation: QualIdent,
    pub authority: Authority,
}

/// A generated obligation the checker must enforce for a consuming rule. Every
/// variant lowers to ordinary graph structure (a constraint or a required
/// pattern element) — never a bespoke mechanism.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AuthorityObligation {
    /// Advisory: a strict constraint rejecting any program revision containing a
    /// rule that derives a *command* protocol request from this policy's
    /// suggestions (Part XII §5). Fully constructible today.
    RejectCommandFromSuggestion(Constraint),
    /// Gated: the consuming rule's body MUST contain the gate relation `G()`.
    /// The checker verifies presence (Part XII §5). Represented as the required
    /// gate; the pattern-presence check is [`gate_present`].
    RequireGateInBody { rule: Ident, gate: QualIdent },
    /// Autonomous: the consuming rule may act only toward protocols in `scope`,
    /// and a `DecisionApplied(decision, ...)` audit relation must be derived
    /// alongside every command. Represented; the co-derivation rewrite is a
    /// documented TODO for the lowering lane.
    ScopeAndAudit {
        rule: Ident,
        scope: Vec<QualIdent>,
        audit_relation: QualIdent,
    },
}

impl PolicyAuthority {
    /// Generate the obligations for a set of consuming rules. This is the entry
    /// the checker calls once name resolution has identified which rules read
    /// `self.suggestion_relation`.
    pub fn lower(&self, consuming_rules: &[Rule]) -> Vec<AuthorityObligation> {
        match &self.authority {
            Authority::Advisory => {
                // One generated strict constraint per policy: "no rule derives a
                // command request from these suggestions." We model the
                // constraint body as: a rule head that is a `*.request` (command
                // protocol) reading the suggestion relation. The full body needs
                // the meta-schema (meta.Rule descriptors) to range over rules;
                // here we emit the constraint shell keyed by the suggestion
                // relation, which is the stable part of the ruling.
                vec![AuthorityObligation::RejectCommandFromSuggestion(
                    self.advisory_constraint(),
                )]
            }
            Authority::Gated { gate } => consuming_rules
                .iter()
                .map(|r| AuthorityObligation::RequireGateInBody {
                    rule: r.name.clone(),
                    gate: gate.clone(),
                })
                .collect(),
            Authority::Autonomous { scope } => consuming_rules
                .iter()
                .map(|r| AuthorityObligation::ScopeAndAudit {
                    rule: r.name.clone(),
                    scope: scope.clone(),
                    audit_relation: QualIdent::from("DecisionApplied"),
                })
                .collect(),
        }
    }

    /// The advisory-mode generated strict constraint (Part XII §5). Named
    /// `<Policy>NoAutonomousCommand`, severity `strict`, body matching the
    /// meta-descriptor of a rule that both reads the suggestion relation and
    /// heads a command request. The body here is the checkable skeleton; the
    /// meta.Rule join is the part that needs the reflection schema.
    pub fn advisory_constraint(&self) -> Constraint {
        Constraint {
            name: Ident::new(format!("{}NoAutonomousCommand", self.policy)),
            severity: Severity::Strict,
            body: Pattern::new(vec![Clause::Edge {
                bind: None,
                relation: self.suggestion_relation.clone(),
                args: vec![],
            }]),
        }
    }
}

/// Gated-mode verification: is the gate relation present as a live read in the
/// consuming rule's body? (Part XII §5: "the compiler verifies presence.")
pub fn gate_present(rule: &Rule, gate: &QualIdent) -> bool {
    rule.body
        .read_set(&[])
        .iter()
        .any(|r| &r.relation == gate && matches!(r.kind, ReadKind::Live | ReadKind::Exists))
}

impl fmt::Display for AuthorityObligation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthorityObligation::RejectCommandFromSuggestion(c) => {
                write!(f, "generate: {c}")
            }
            AuthorityObligation::RequireGateInBody { rule, gate } => {
                write!(f, "require: rule {rule} must read gate {gate}")
            }
            AuthorityObligation::ScopeAndAudit {
                rule,
                scope,
                audit_relation,
            } => {
                write!(f, "require: rule {rule} acts only within {{")?;
                for (i, s) in scope.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{s}")?;
                }
                write!(f, "}} and co-derives {audit_relation}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Head, Rule};
    use crate::effects::EffectRow;
    use crate::pattern::{edge, Arg, RoleArg};

    fn consuming_rule(with_gate: bool) -> Rule {
        let mut clauses = vec![edge(
            "AssignmentAdvice",
            &[("order", "o"), ("vehicle", "v")],
        )];
        if with_gate {
            clauses.push(edge("AutoAssignmentEnabled", &[]));
        }
        Rule {
            name: Ident::new("AutoAssign"),
            head: Head::Tuple {
                relation: QualIdent::from("AssignVehicleCommand.request"),
                args: vec![RoleArg {
                    role: Ident::new("order"),
                    arg: Arg::Var(Ident::new("o")),
                }],
            },
            body: Pattern::new(clauses),
            effects: EffectRow::empty(),
        }
    }

    #[test]
    fn advisory_lowers_to_one_strict_constraint() {
        let pa = PolicyAuthority {
            policy: Ident::new("AssignVehicle"),
            suggestion_relation: QualIdent::from("AssignmentAdvice"),
            authority: Authority::Advisory,
        };
        let obligations = pa.lower(&[]);
        assert_eq!(obligations.len(), 1);
        match &obligations[0] {
            AuthorityObligation::RejectCommandFromSuggestion(c) => {
                assert_eq!(c.severity, Severity::Strict);
                assert_eq!(c.name.as_str(), "AssignVehicleNoAutonomousCommand");
            }
            other => panic!("expected a generated constraint, got {other}"),
        }
    }

    #[test]
    fn gated_requires_the_gate_and_presence_check_works() {
        let pa = PolicyAuthority {
            policy: Ident::new("AssignVehicle"),
            suggestion_relation: QualIdent::from("AssignmentAdvice"),
            authority: Authority::Gated {
                gate: QualIdent::from("AutoAssignmentEnabled"),
            },
        };
        let rule = consuming_rule(true);
        let obligations = pa.lower(std::slice::from_ref(&rule));
        assert_eq!(obligations.len(), 1);
        assert!(gate_present(
            &rule,
            &QualIdent::from("AutoAssignmentEnabled")
        ));
        assert!(!gate_present(
            &consuming_rule(false),
            &QualIdent::from("AutoAssignmentEnabled")
        ));
    }

    #[test]
    fn autonomous_carries_scope_and_audit_relation() {
        let pa = PolicyAuthority {
            policy: Ident::new("AssignVehicle"),
            suggestion_relation: QualIdent::from("AssignmentAdvice"),
            authority: Authority::Autonomous {
                scope: vec![QualIdent::from("AssignVehicleCommand")],
            },
        };
        let rule = consuming_rule(false);
        let obligations = pa.lower(std::slice::from_ref(&rule));
        match &obligations[0] {
            AuthorityObligation::ScopeAndAudit { audit_relation, .. } => {
                assert_eq!(audit_relation, &QualIdent::from("DecisionApplied"));
            }
            other => panic!("expected scope+audit, got {other}"),
        }
    }
}
