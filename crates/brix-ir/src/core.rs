//! Core IR: a small, closed set of typed nodes with an explicit [`Ty`] on every
//! node, plus a real `Display` (a debugging deliverable per the build plan).
//!
//! The expression IR is intentionally tiny — the function language is "a small,
//! modern, immutable-first expression language" (Part V §1), not a systems
//! language. Every [`Expr`] carries its result [`Ty`] so the IR is fully typed
//! after inference (which is stubbed: the builder sets types it knows and uses
//! [`Ty::Var`] where a real inferencer would solve).
//!
//! Above expressions sit the *declaration* nodes brix-ir actually checks:
//! [`Rule`] (a `derive`), [`Constraint`], and [`Query`]. These are the units
//! the phase lane, oracle, and codegen consume.

use crate::effects::{EffectRow, RuleEffectFlags};
use crate::ident::{Ident, QualIdent};
use crate::pattern::Pattern;
use crate::site::SiteId;
use crate::types::Ty;
use core::fmt;

/// A typed expression node. `ty` is the node's result type; `kind` is the
/// operator. Closed set.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Expr {
    pub ty: Ty,
    pub kind: Box<ExprKind>,
}

impl Expr {
    pub fn new(ty: Ty, kind: ExprKind) -> Self {
        Expr {
            ty,
            kind: Box::new(kind),
        }
    }
}

/// The closed operator set of the expression IR.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ExprKind {
    /// A bound variable reference.
    Var(Ident),
    /// A literal (see [`crate::pattern::Lit`]).
    Lit(crate::pattern::Lit),
    /// Function/aggregate application. `func` is a qualified name; the callee's
    /// effect row folds into the enclosing body's effect row at check time.
    Call { func: QualIdent, args: Vec<Expr> },
    /// A field/role projection (`score.lowerBound`, `o.due`).
    Field { base: Expr, field: Ident },
    /// Structural record literal with explicit field names.
    Record { fields: Vec<(Ident, Expr)> },
    /// `if c { t } else { e }`.
    If { cond: Expr, then: Expr, els: Expr },
    /// A `?` postfix failure site (Part III §9). Carries the stable [`SiteId`]
    /// assigned at build time; on failure it derives a `RuleError` at this
    /// site. The `ty` on the enclosing [`Expr`] is the unwrapped success type.
    Try { inner: Expr, site: SiteId },
    /// A `from { pattern } yield expr` relation comprehension → `Rel<S>`
    /// (Part IV §4). The yielded row type is the enclosing [`Expr::ty`].
    Comprehension {
        pattern: Pattern,
        yields: Option<Expr>,
    },
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Render `expr : Ty` so the debugging Display shows the type on every
        // node, which is the whole point of a typed IR dump.
        write!(f, "{} : {}", self.kind, self.ty)
    }
}

impl fmt::Display for ExprKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprKind::Var(v) => write!(f, "{v}"),
            ExprKind::Lit(l) => write!(f, "{l}"),
            ExprKind::Call { func, args } => {
                write!(f, "{func}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", a.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::Field { base, field } => write!(f, "{}.{field}", base.kind),
            ExprKind::Record { fields } => {
                write!(f, "{{ ")?;
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {}", value.kind)?;
                }
                write!(f, " }}")
            }
            ExprKind::If { cond, then, els } => {
                write!(
                    f,
                    "if {} {{ {} }} else {{ {} }}",
                    cond.kind, then.kind, els.kind
                )
            }
            ExprKind::Try { inner, site } => write!(f, "{}?/*{site}*/", inner.kind),
            ExprKind::Comprehension { pattern, yields } => {
                write!(f, "from {{ {pattern} }}")?;
                if let Some(y) = yields {
                    write!(f, " yield {}", y.kind)?;
                }
                Ok(())
            }
        }
    }
}

/// A rule head (Part IV §2, Appendix D `Head`): the closed set of things a
/// `derive` may produce.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Head {
    /// `R(role: x, ...)` — a relation tuple, a protocol request, or a
    /// constraint violation (all `TupleHead` in the grammar).
    Tuple {
        relation: QualIdent,
        args: Vec<crate::pattern::RoleArg>,
    },
    /// `n: Entity { ... } keyed by (k1, ...)` — a derived (Skolem) node.
    Node {
        var: Ident,
        entity: Ident,
        args: Vec<crate::pattern::RoleArg>,
        keyed_by: Vec<Ident>,
    },
    /// `mask(target) by reason` — both are pattern-bound edge refs
    /// (Part III §6; Appendix E `mask-head` side condition).
    Mask { target: Ident, reason: Ident },
}

impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Head::Tuple { relation, args } => {
                write!(f, "{relation}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ")")
            }
            Head::Node {
                var,
                entity,
                args,
                keyed_by,
            } => {
                write!(f, "{var}: {entity} {{ ")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, " }} keyed by (")?;
                for (i, k) in keyed_by.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}")?;
                }
                write!(f, ")")
            }
            Head::Mask { target, reason } => write!(f, "mask({target}) by {reason}"),
        }
    }
}

/// A `derive` rule (Part IV §2). The central checked IR unit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rule {
    pub name: Ident,
    pub head: Head,
    pub body: Pattern,
    /// The combined effect row over `B ∪ H` (Appendix E), as inferred/declared.
    pub effects: EffectRow,
}

impl Rule {
    /// The Appendix E rule side-condition flags for this rule's body effects.
    pub fn effect_flags(&self) -> RuleEffectFlags {
        self.effects.rule_flags()
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "derive {}: {} from {{ {} }} {}",
            self.name, self.head, self.body, self.effects
        )
    }
}

/// Constraint severity (Part IV §7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Severity {
    Advisory,
    Strict,
    Audit,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Advisory => "advisory",
            Severity::Strict => "strict",
            Severity::Audit => "audit",
        })
    }
}

/// A `constraint` (Part IV §7). Matches derive sealed `Violation` edges.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Constraint {
    pub name: Ident,
    pub severity: Severity,
    pub body: Pattern,
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "constraint {} {} {{ {} }}",
            self.name, self.severity, self.body
        )
    }
}

/// A `query` (Part IV §6): a pure function over a settled snapshot.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Query {
    pub name: Ident,
    pub params: Vec<(Ident, Ty)>,
    pub body: Pattern,
    pub yields: Expr,
    /// Result row type `Rel<Row>`.
    pub result: Ty,
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "query {} -> {} = from {{ {} }} yield {}",
            self.name, self.result, self.body, self.yields.kind
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{edge, Arg, RoleArg};
    use crate::site::SiteAssigner;

    fn rolearg(role: &str, var: &str) -> RoleArg {
        RoleArg {
            role: Ident::new(role),
            arg: Arg::Var(Ident::new(var)),
        }
    }

    #[test]
    fn expr_display_shows_type_on_every_node() {
        let e = Expr::new(Ty::Bool, ExprKind::Var(Ident::new("flag")));
        assert_eq!(e.to_string(), "flag : Bool");
    }

    #[test]
    fn try_expr_renders_its_site() {
        let mut sites = SiteAssigner::new(Ident::new("R"));
        let inner = Expr::new(Ty::option(Ty::Bool), ExprKind::Var(Ident::new("m")));
        let site = sites.next_site();
        let e = Expr::new(Ty::Bool, ExprKind::Try { inner, site });
        let shown = e.to_string();
        assert!(shown.starts_with("m?/*site:"));
        assert!(shown.ends_with(": Bool"));
    }

    #[test]
    fn rule_display_is_a_full_derive_line() {
        let rule = Rule {
            name: Ident::new("FromComputed"),
            head: Head::Tuple {
                relation: QualIdent::from("Price"),
                args: vec![rolearg("order", "o"), rolearg("amount", "a")],
            },
            body: Pattern::new(vec![edge(
                "ComputedPrice",
                &[("order", "o"), ("amount", "a")],
            )]),
            effects: EffectRow::empty(),
        };
        assert_eq!(
            rule.to_string(),
            "derive FromComputed: Price(order: o, amount: a) from { ComputedPrice(order: o, amount: a) } !{}"
        );
        // A pure rule satisfies all Appendix E side conditions.
        let flags = rule.effect_flags();
        assert!(flags.pure && flags.det && flags.nondiverge);
    }

    #[test]
    fn mask_head_display() {
        let h = Head::Mask {
            target: Ident::new("price"),
            reason: Ident::new("manual"),
        };
        assert_eq!(h.to_string(), "mask(price) by manual");
    }

    #[test]
    fn constraint_display() {
        let c = Constraint {
            name: Ident::new("NoPriceConflicts"),
            severity: Severity::Strict,
            body: Pattern::new(vec![edge("KeyConflict", &[("relation", "ComputedPrice")])]),
        };
        assert_eq!(
            c.to_string(),
            "constraint NoPriceConflicts strict { KeyConflict(relation: ComputedPrice) }"
        );
    }
}
