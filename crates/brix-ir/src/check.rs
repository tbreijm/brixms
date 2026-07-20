//! Whole-declaration checks that compose the per-module analyses against a
//! [`SchemaResolver`] (Appendix E judgments that need Σ). These are the checks
//! that *can* run before a full frontend — they need only resolved schema
//! facts, which [`crate::frontend::TableResolver`] supplies today.

use crate::core::{Expr, ExprKind, FnDef, Head, Rule};
use crate::effects::{Effect, EffectRow};
use crate::frontend::{RelationSchema, SchemaResolver};
use crate::ident::{Ident, QualIdent};
use crate::pattern::{ReadKind, RelRead};
use crate::types::{check_key_canonical, KeyCanonicalError};
use core::fmt;
use std::collections::BTreeSet;

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
    /// Appendix E `pure(B, H)` violated: an impure effect atom is present in
    /// the rule's combined body/head effect row.
    ImpureRule { rule: Ident },
    /// Appendix E `det(B, H)` violated: a non-deterministic effect atom
    /// (`random`/`clock`/`net`/`fs`/`solver`) or an open effect tail is
    /// present.
    NondeterministicRule { rule: Ident },
    /// Appendix E `nondiverge(B, H)` violated: `diverge` is reachable from
    /// the rule, either as an effect-row atom or via a called fn whose
    /// signature declares `may_diverge`.
    DivergentRule { rule: Ident },
    /// Appendix E `keys(H) ⊆ Bindings` violated: a `keyed by (...)` ident on
    /// a derived-node head is not among the values the body binds.
    UnboundHeadKey { rule: Ident, key: Ident },
    /// Appendix E mask-head side condition violated: `mask(target) by
    /// reason`'s `target`/`reason` is not an edge-bound alias (`x @
    /// R(...)`) produced by the rule body.
    MaskRefNotEdgeBound { rule: Ident, var: Ident },
    /// A user function's body realizes an effect its declared `! { ... }` row
    /// does not permit (issue #47 / Part V): the body calls a fn whose effects
    /// are not a subset of the declaration. Purity/effect containment for
    /// function bodies, the analog of [`Finding::ImpureRule`] for rules.
    UndeclaredFnEffect { function: Ident, effect: Effect },
    /// A `total` function's body can fail — it uses `?` (a failure site) — but
    /// only a `partial fn` may fail (Part V §5). Totality violation.
    TotalFnFallible { function: Ident },
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
            Finding::ImpureRule { rule } => write!(
                f,
                "rule {rule} is not pure: an effect atom reaches its body/head (Appendix E `pure(B, H)`)"
            ),
            Finding::NondeterministicRule { rule } => write!(
                f,
                "rule {rule} is not deterministic: a non-deterministic effect atom or open effect tail is present (Appendix E `det(B, H)`)"
            ),
            Finding::DivergentRule { rule } => write!(
                f,
                "rule {rule} may diverge: `diverge` is reachable from its body (Appendix E `nondiverge(B, H)`)"
            ),
            Finding::UnboundHeadKey { rule, key } => write!(
                f,
                "rule {rule}'s head key `{key}` is not bound by its body (Appendix E `keys(H) ⊆ Bindings`)"
            ),
            Finding::MaskRefNotEdgeBound { rule, var } => write!(
                f,
                "rule {rule}'s mask head references `{var}`, which is not an edge-bound alias in its body (Appendix E mask-head side condition)"
            ),
            Finding::UndeclaredFnEffect { function, effect } => write!(
                f,
                "function {function} realizes effect `{effect}` not permitted by its declared effect row (Part V effect containment)"
            ),
            Finding::TotalFnFallible { function } => write!(
                f,
                "total function {function} can fail (`?`); only a `partial fn` may fail (Part V §5)"
            ),
        }
    }
}

/// Appendix E rule-side-condition judgments computed once by walking a
/// rule's body, and consumed by both this module's own [`Finding`]s and
/// [`crate::reflect`]'s mirrored `ConflictKind`s — the same "one algorithm,
/// two observers" split [`crate::solve`] uses for the type algebra, so the
/// trusted checker and the reflective analyzer cannot silently diverge on
/// what counts as a violation.
#[derive(Default, Debug)]
pub struct CallEffects {
    /// Whether any fn called from the body declares `may_diverge` (Appendix
    /// E `nondiverge`), independent of whether the rule's own `EffectRow`
    /// also carries a `diverge` atom.
    pub diverges: bool,
    /// Relations read inside a `Comprehension` passed as an argument to an
    /// `aggregate fn` call (Appendix E `Aggregate call`: "in-rule use ⇒
    /// strict dep on every relation in extent(S)") — feeds
    /// [`crate::pattern::Pattern::read_set`]'s `aggregate_reads` parameter.
    pub aggregate_reads: BTreeSet<QualIdent>,
    /// Relations read inside a `Comprehension` passed to an *ordinary*
    /// (non-aggregate) fn call, where the relation is graph-derived
    /// (Appendix E `Ordinary fn`: "in-rule use on graph-derived Rel:
    /// ERROR").
    pub ordinary_on_derived: BTreeSet<QualIdent>,
}

/// Walk every expression in `rule`'s body, classifying calls into a
/// [`CallEffects`] per the Appendix E `Aggregate call`/`Ordinary fn`/
/// `nondiverge` judgments.
pub fn scan_rule_calls(rule: &Rule, resolver: &impl SchemaResolver) -> CallEffects {
    let mut acc = CallEffects::default();
    for expr in rule.body.body_exprs() {
        scan_expr(expr, resolver, &mut acc);
    }
    acc
}

fn scan_expr(expr: &Expr, resolver: &impl SchemaResolver, acc: &mut CallEffects) {
    match &*expr.kind {
        ExprKind::Call { func, args } => {
            // Conservative: union effects / flags across arity-matching overloads.
            for sig in resolver
                .functions(func)
                .iter()
                .filter(|sig| sig.params.len() == args.len())
            {
                if sig.may_diverge {
                    acc.diverges = true;
                }
                for arg in args {
                    if let ExprKind::Comprehension { pattern, .. } = &*arg.kind {
                        let relations: BTreeSet<QualIdent> = pattern
                            .read_set(&[])
                            .into_iter()
                            .map(|r| r.relation)
                            .collect();
                        if sig.is_aggregate {
                            acc.aggregate_reads.extend(relations.clone());
                        } else {
                            for relation in relations {
                                if resolver
                                    .relation(&relation)
                                    .map(|s| s.derived)
                                    .unwrap_or(false)
                                {
                                    acc.ordinary_on_derived.insert(relation);
                                }
                            }
                        }
                    }
                }
            }
            for arg in args {
                scan_expr(arg, resolver, acc);
            }
        }
        ExprKind::Field { base, .. } => scan_expr(base, resolver, acc),
        ExprKind::Record { fields } => {
            for (_, value) in fields {
                scan_expr(value, resolver, acc);
            }
        }
        ExprKind::If { cond, then, els } => {
            scan_expr(cond, resolver, acc);
            scan_expr(then, resolver, acc);
            scan_expr(els, resolver, acc);
        }
        ExprKind::Try { inner, .. } => scan_expr(inner, resolver, acc),
        ExprKind::Comprehension { pattern, yields } => {
            for e in pattern.body_exprs() {
                scan_expr(e, resolver, acc);
            }
            if let Some(y) = yields {
                scan_expr(y, resolver, acc);
            }
        }
        ExprKind::Let { value, body, .. } => {
            scan_expr(value, resolver, acc);
            scan_expr(body, resolver, acc);
        }
        ExprKind::Var(_) | ExprKind::Lit(_) => {}
    }
}

/// `keyed_by` idents (Appendix E `keys(H) ⊆ Bindings`) not present among the
/// body's exported bindings, for a `Head::Node`. Empty for any other head
/// shape.
pub fn unbound_head_keys(rule: &Rule) -> Vec<Ident> {
    let Head::Node { keyed_by, .. } = &rule.head else {
        return Vec::new();
    };
    let bound = rule.body.bound_vars();
    keyed_by
        .iter()
        .filter(|k| !bound.contains(k))
        .cloned()
        .collect()
}

/// `target`/`reason` idents (Appendix E mask-head side condition) not
/// present among the body's edge-ref bindings, for a `Head::Mask`. Empty for
/// any other head shape.
pub fn unbound_mask_refs(rule: &Rule) -> Vec<Ident> {
    let Head::Mask { target, reason } = &rule.head else {
        return Vec::new();
    };
    let refs = rule.body.edge_refs();
    [target, reason]
        .into_iter()
        .filter(|v| !refs.contains(v))
        .cloned()
        .collect()
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
    let calls = scan_rule_calls(rule, resolver);
    let aggregate_reads: Vec<QualIdent> = calls.aggregate_reads.into_iter().collect();
    for RelRead { relation, kind } in rule.body.read_set(&aggregate_reads) {
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

/// Rule-level effect-discipline check (Appendix E `pure(B, H)` / `det(B, H)`
/// / `nondiverge(B, H)`): the rule's combined effect row (already unioned
/// over body/head by lowering, see [`crate::core::Rule::effect_flags`]) must
/// carry no impure atom, no non-deterministic atom or open tail, and no
/// reachable `diverge` — the last also checked directly against every
/// called fn's [`crate::frontend::FnSignature::may_diverge`], not only the
/// effect row, since the two are independent fields on a hand-built
/// signature.
pub fn check_rule_effects(rule: &Rule, resolver: &impl SchemaResolver) -> Vec<Finding> {
    let mut out = Vec::new();
    let flags = rule.effect_flags();
    let calls = scan_rule_calls(rule, resolver);
    if !flags.pure {
        out.push(Finding::ImpureRule {
            rule: rule.name.clone(),
        });
    }
    if !flags.det {
        out.push(Finding::NondeterministicRule {
            rule: rule.name.clone(),
        });
    }
    if !flags.nondiverge || calls.diverges {
        out.push(Finding::DivergentRule {
            rule: rule.name.clone(),
        });
    }
    out
}

/// Derived-node head key check (Appendix E `keys(H) ⊆ Bindings`).
pub fn check_rule_head_keys(rule: &Rule) -> Vec<Finding> {
    unbound_head_keys(rule)
        .into_iter()
        .map(|key| Finding::UnboundHeadKey {
            rule: rule.name.clone(),
            key,
        })
        .collect()
}

/// Mask-head edge-ref check (Appendix E mask-head side condition).
pub fn check_rule_mask_head(rule: &Rule) -> Vec<Finding> {
    unbound_mask_refs(rule)
        .into_iter()
        .map(|var| Finding::MaskRefNotEdgeBound {
            rule: rule.name.clone(),
            var,
        })
        .collect()
}

/// Run all schema-dependent checks over one rule: absence/witness (`Without`),
/// the effect-discipline triple (`pure`/`det`/`nondiverge`), derived-node
/// head keys, mask-head edge refs, and ordinary-fn-on-derived-relation
/// (Appendix E `Rule`, `Ordinary fn`).
pub fn check_rule(rule: &Rule, resolver: &impl SchemaResolver) -> Vec<Finding> {
    let mut out = check_rule_absence(rule, resolver);
    out.extend(check_rule_effects(rule, resolver));
    out.extend(check_rule_head_keys(rule));
    out.extend(check_rule_mask_head(rule));
    let calls = scan_rule_calls(rule, resolver);
    for relation in calls.ordinary_on_derived {
        out.push(Finding::OrdinaryFnOnDerivedRel {
            relation,
            in_rule: rule.name.clone(),
        });
    }
    out
}

/// Function-body static checks (issue #47), the [`check_rule`] analog for a
/// user [`FnDef`]: **effect containment** (the body's realized effects must be
/// a subset of the declared `! { ... }` row) and **totality** (a `total` fn
/// body must not use `?`). Type checking of the body is done by
/// [`crate::infer::infer_source`]; this covers the effect/totality judgments.
pub fn check_function(def: &FnDef, resolver: &impl SchemaResolver) -> Vec<Finding> {
    let function = Ident::new(def.name.to_string());
    let mut realized = EffectRow::empty();
    let mut has_try = false;
    walk_fn_body(&def.body, resolver, &mut realized, &mut has_try);

    let mut out = Vec::new();
    let declared = def.effects.atoms();
    for atom in realized.atoms() {
        if !declared.contains(atom) {
            out.push(Finding::UndeclaredFnEffect {
                function: function.clone(),
                effect: atom.clone(),
            });
        }
    }
    if !def.is_partial && has_try {
        out.push(Finding::TotalFnFallible { function });
    }
    out
}

/// Walk a function body, unioning every called fn's declared effect row into
/// `realized` and flagging any `?` failure site — the raw material for
/// [`check_function`]'s effect-containment and totality judgments.
fn walk_fn_body(
    expr: &Expr,
    resolver: &impl SchemaResolver,
    realized: &mut EffectRow,
    has_try: &mut bool,
) {
    match &*expr.kind {
        ExprKind::Call { func, args } => {
            for sig in resolver
                .functions(func)
                .iter()
                .filter(|sig| sig.params.len() == args.len())
            {
                *realized = realized.combine(&sig.effects);
            }
            for a in args {
                walk_fn_body(a, resolver, realized, has_try);
            }
        }
        ExprKind::Try { inner, .. } => {
            *has_try = true;
            walk_fn_body(inner, resolver, realized, has_try);
        }
        ExprKind::Field { base, .. } => walk_fn_body(base, resolver, realized, has_try),
        ExprKind::Record { fields } => {
            for (_, e) in fields {
                walk_fn_body(e, resolver, realized, has_try);
            }
        }
        ExprKind::If { cond, then, els } => {
            walk_fn_body(cond, resolver, realized, has_try);
            walk_fn_body(then, resolver, realized, has_try);
            walk_fn_body(els, resolver, realized, has_try);
        }
        ExprKind::Comprehension { yields, .. } => {
            if let Some(y) = yields {
                walk_fn_body(y, resolver, realized, has_try);
            }
        }
        ExprKind::Let { value, body, .. } => {
            walk_fn_body(value, resolver, realized, has_try);
            walk_fn_body(body, resolver, realized, has_try);
        }
        ExprKind::Var(_) | ExprKind::Lit(_) => {}
    }
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

    #[test]
    fn impure_rule_effect_row_is_flagged() {
        use crate::effects::Effect;

        let rule = Rule {
            name: Ident::new("Loud"),
            head: Head::Tuple {
                relation: QualIdent::from("Out"),
                args: vec![],
            },
            body: Pattern::default(),
            effects: EffectRow::from_atoms([Effect::Console]),
        };
        let findings = check_rule(&rule, &TableResolver::new());
        assert!(findings
            .iter()
            .any(|f| matches!(f, Finding::ImpureRule { .. })));
        // `console` is not one of the non-determinism-flagging atoms and
        // carries no `diverge` — only `pure` fails.
        assert!(!findings
            .iter()
            .any(|f| matches!(f, Finding::NondeterministicRule { .. })));
        assert!(!findings
            .iter()
            .any(|f| matches!(f, Finding::DivergentRule { .. })));
    }

    #[test]
    fn diverging_called_fn_is_flagged_even_without_a_diverge_atom() {
        use crate::core::{Expr, ExprKind};
        use crate::frontend::FnSignature;

        let rule = Rule {
            name: Ident::new("Spins"),
            head: Head::Tuple {
                relation: QualIdent::from("Out"),
                args: vec![],
            },
            body: Pattern::new(vec![Clause::Let {
                binds: Ident::new("x"),
                expr: Expr::new(
                    Ty::Int(crate::types::IntWidth::Int),
                    ExprKind::Call {
                        func: QualIdent::from("loopForever"),
                        args: vec![],
                    },
                ),
            }]),
            // Deliberately empty: `may_diverge` on the signature is the only
            // signal here, proving the check does not rely solely on the
            // effect row.
            effects: EffectRow::empty(),
        };
        let resolver = TableResolver::new().with_function(FnSignature {
            name: QualIdent::from("loopForever"),
            params: vec![],
            ret: Ty::Int(crate::types::IntWidth::Int),
            effects: EffectRow::empty(),
            is_aggregate: false,
            may_diverge: true,
        });
        let findings = check_rule(&rule, &resolver);
        assert!(findings
            .iter()
            .any(|f| matches!(f, Finding::DivergentRule { .. })));
    }

    #[test]
    fn unbound_head_key_is_flagged() {
        let rule = Rule {
            name: Ident::new("Mint"),
            head: Head::Node {
                var: Ident::new("n"),
                entity: Ident::new("Widget"),
                args: vec![],
                keyed_by: vec![Ident::new("missing")],
            },
            body: Pattern::default(),
            effects: EffectRow::empty(),
        };
        let findings = check_rule(&rule, &TableResolver::new());
        assert!(findings.iter().any(
            |f| matches!(f, Finding::UnboundHeadKey { key, .. } if key.as_str() == "missing")
        ));
    }

    #[test]
    fn bound_head_key_passes() {
        let rule = Rule {
            name: Ident::new("Mint"),
            head: Head::Node {
                var: Ident::new("n"),
                entity: Ident::new("Widget"),
                args: vec![],
                keyed_by: vec![Ident::new("o")],
            },
            body: Pattern::new(vec![edge("Order", &[("id", "o")])]),
            effects: EffectRow::empty(),
        };
        assert!(check_rule(&rule, &TableResolver::new())
            .iter()
            .all(|f| !matches!(f, Finding::UnboundHeadKey { .. })));
    }

    #[test]
    fn mask_head_referencing_non_edge_bound_var_is_flagged() {
        let rule = Rule {
            name: Ident::new("Override"),
            head: Head::Mask {
                target: Ident::new("price"),
                reason: Ident::new("manual"),
            },
            body: Pattern::default(),
            effects: EffectRow::empty(),
        };
        let findings = check_rule(&rule, &TableResolver::new());
        let flagged: Vec<&Ident> = findings
            .iter()
            .filter_map(|f| match f {
                Finding::MaskRefNotEdgeBound { var, .. } => Some(var),
                _ => None,
            })
            .collect();
        assert_eq!(flagged.len(), 2);
    }

    #[test]
    fn mask_head_with_edge_bound_refs_passes() {
        let rule = Rule {
            name: Ident::new("Override"),
            head: Head::Mask {
                target: Ident::new("price"),
                reason: Ident::new("manual"),
            },
            body: Pattern::new(vec![
                Clause::Edge {
                    bind: Some(Ident::new("price")),
                    relation: QualIdent::from("ComputedPrice"),
                    args: vec![],
                },
                Clause::Edge {
                    bind: Some(Ident::new("manual")),
                    relation: QualIdent::from("ManualPrice"),
                    args: vec![],
                },
            ]),
            effects: EffectRow::empty(),
        };
        assert!(check_rule(&rule, &TableResolver::new())
            .iter()
            .all(|f| !matches!(f, Finding::MaskRefNotEdgeBound { .. })));
    }

    #[test]
    fn ordinary_fn_on_derived_rel_is_flagged() {
        use crate::core::{Expr, ExprKind};
        use crate::frontend::FnSignature;

        let comprehension = Expr::new(
            Ty::rel(crate::types::Row::closed(vec![])),
            ExprKind::Comprehension {
                pattern: Pattern::new(vec![edge("ComputedPrice", &[("order", "o")])]),
                yields: None,
            },
        );
        let rule = Rule {
            name: Ident::new("Summary"),
            head: Head::Tuple {
                relation: QualIdent::from("Out"),
                args: vec![],
            },
            body: Pattern::new(vec![Clause::Let {
                binds: Ident::new("total"),
                expr: Expr::new(
                    Ty::Int(crate::types::IntWidth::Int),
                    ExprKind::Call {
                        func: QualIdent::from("sumUp"),
                        args: vec![comprehension],
                    },
                ),
            }]),
            effects: EffectRow::empty(),
        };
        let resolver = TableResolver::new()
            .with_relation(rel(
                "ComputedPrice",
                vec![("order", Ty::NodeRef(Ident::new("Order")))],
                &[],
                true,
            ))
            .with_function(FnSignature {
                name: QualIdent::from("sumUp"),
                params: vec![Ty::rel(crate::types::Row::closed(vec![]))],
                ret: Ty::Int(crate::types::IntWidth::Int),
                effects: EffectRow::empty(),
                is_aggregate: false,
                may_diverge: false,
            });
        // `derived` defaults to `false` from the shared `rel(...)` helper —
        // patch it to `true` (graph-derived) to exercise the violation.
        let derived_resolver = TableResolver::new()
            .with_relation(RelationSchema {
                derived: true,
                ..rel(
                    "ComputedPrice",
                    vec![("order", Ty::NodeRef(Ident::new("Order")))],
                    &[],
                    true,
                )
            })
            .with_function(FnSignature {
                name: QualIdent::from("sumUp"),
                params: vec![Ty::rel(crate::types::Row::closed(vec![]))],
                ret: Ty::Int(crate::types::IntWidth::Int),
                effects: EffectRow::empty(),
                is_aggregate: false,
                may_diverge: false,
            });
        assert!(check_rule(&rule, &resolver)
            .iter()
            .all(|f| !matches!(f, Finding::OrdinaryFnOnDerivedRel { .. })));
        assert!(check_rule(&rule, &derived_resolver)
            .iter()
            .any(|f| matches!(f, Finding::OrdinaryFnOnDerivedRel { .. })));
    }

    #[test]
    fn aggregate_fn_on_derived_rel_is_not_flagged() {
        use crate::core::{Expr, ExprKind};
        use crate::frontend::FnSignature;

        let comprehension = Expr::new(
            Ty::rel(crate::types::Row::closed(vec![])),
            ExprKind::Comprehension {
                pattern: Pattern::new(vec![edge("ComputedPrice", &[("order", "o")])]),
                yields: None,
            },
        );
        let rule = Rule {
            name: Ident::new("Summary"),
            head: Head::Tuple {
                relation: QualIdent::from("Out"),
                args: vec![],
            },
            body: Pattern::new(vec![Clause::Let {
                binds: Ident::new("total"),
                expr: Expr::new(
                    Ty::Int(crate::types::IntWidth::Int),
                    ExprKind::Call {
                        func: QualIdent::from("count"),
                        args: vec![comprehension],
                    },
                ),
            }]),
            effects: EffectRow::empty(),
        };
        let resolver = TableResolver::new()
            .with_relation(RelationSchema {
                derived: true,
                ..rel(
                    "ComputedPrice",
                    vec![("order", Ty::NodeRef(Ident::new("Order")))],
                    &[],
                    true,
                )
            })
            .with_function(FnSignature {
                name: QualIdent::from("count"),
                params: vec![Ty::rel(crate::types::Row::closed(vec![]))],
                ret: Ty::Int(crate::types::IntWidth::Int),
                effects: EffectRow::empty(),
                is_aggregate: true,
                may_diverge: false,
            });
        let findings = check_rule(&rule, &resolver);
        assert!(findings
            .iter()
            .all(|f| !matches!(f, Finding::OrdinaryFnOnDerivedRel { .. })));
        // The relation is classified as an aggregate read (Appendix E
        // `Aggregate call`), not silently dropped.
        assert!(scan_rule_calls(&rule, &resolver)
            .aggregate_reads
            .contains(&QualIdent::from("ComputedPrice")));
    }
}
