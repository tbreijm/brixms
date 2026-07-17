//! IR pattern representation and pattern read-set analysis (Part IV §3 pattern
//! language; Appendix E `Patterns` judgment; Appendix F edge classification).
//!
//! The one pattern language serves rules, queries, constraints, and
//! comprehensions (Part IV §3). The read-set analysis here computes, per
//! clause, *which relations are read and how* — the input phase-inference
//! (brix-phase, Appendix F) needs to build positive vs. strict vs. mask edges.
//! brix-ir does not build the phase graph (that is brix-phase's lane); it
//! produces the classified read-set that lane consumes.

use crate::ident::{Ident, QualIdent};
use crate::types::Ty;
use core::fmt;

/// An immutable literal value that can appear in a pattern argument or `let`.
/// Floats are stored as canonicalized IEEE **bit patterns**, never as `f64`
/// values, so `Lit` derives `Eq`/`Ord` and never smuggles a float into a
/// semantic comparison path (Part V §8; CONTRIBUTING.md float rule).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Lit {
    Unit,
    Bool(bool),
    Int(i64),
    Str(String),
    /// A canonicalized `f64` bit pattern (NaN already folded to one pattern per
    /// Part V §8). Never dereferenced as a float inside brix-ir.
    F64Bits(u64),
    /// An enum-variant literal (mismatch B): `ty` names the declared enum
    /// (`Tier`, `Status`, ...), `ordinal` is the variant's zero-based
    /// declaration-order index — the App. G canonical encoding ("enums
    /// encode by declaration-order ordinal"), never the variant's name. A
    /// `Str` encoding would poison canonical semantics here: two variants
    /// renamed but not reordered must compare/encode identically to
    /// consumers that only see the ordinal.
    Enum {
        ty: QualIdent,
        ordinal: u32,
    },
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lit::Unit => write!(f, "()"),
            Lit::Bool(b) => write!(f, "{b}"),
            Lit::Int(i) => write!(f, "{i}"),
            Lit::Str(s) => write!(f, "{s:?}"),
            Lit::F64Bits(bits) => write!(f, "f64:0x{bits:016x}"),
            Lit::Enum { ty, ordinal } => write!(f, "{ty}#{ordinal}"),
        }
    }
}

/// An argument to an edge/entity clause: either binds a variable or matches a
/// literal. Role name is carried separately in [`EdgeClause`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Arg {
    /// `role: x` — bind (or equijoin on) variable `x`.
    Var(Ident),
    /// `role: 5` — match a literal.
    Lit(Lit),
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arg::Var(v) => write!(f, "{v}"),
            Arg::Lit(l) => write!(f, "{l}"),
        }
    }
}

/// One `role: arg` binding inside an edge clause. Role order is not semantic
/// (Part IV §1); we keep declaration order for `Display` and rely on role
/// *names* for identity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RoleArg {
    pub role: Ident,
    pub arg: Arg,
}

impl fmt::Display for RoleArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.role, self.arg)
    }
}

/// How a relation is read at a site — the classification Appendix F needs to
/// choose a positive, strict, or mask edge.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ReadKind {
    /// Ordinary live, monotone read (positive edge; App. F rule 1).
    Live,
    /// Read through `without` / `optional`-absence / aggregate / witness
    /// (strict edge; App. F rule 2).
    Strict,
    /// `history R(...)` read — bypasses masks and supersession, creates no
    /// phase dependency (Part III §6.3, App. F rule 3 exclusion).
    History,
    /// `exists { ... }` — a positive existence test (monotone, positive edge).
    Exists,
}

impl fmt::Display for ReadKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ReadKind::Live => "live",
            ReadKind::Strict => "strict",
            ReadKind::History => "history",
            ReadKind::Exists => "exists",
        })
    }
}

/// One entry of a rule/query read-set: a relation read, and how.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RelRead {
    pub relation: QualIdent,
    pub kind: ReadKind,
}

impl fmt::Display for RelRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.relation, self.kind)
    }
}

/// A clause of the IR pattern language (Part IV §3, Appendix D `Clause`).
/// Closed set — every surface clause lowers to one of these.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Clause {
    /// `e @ R(role: x, ...)` — edge clause; `bind` is the optional edge-ref
    /// binding (the `e @`).
    Edge {
        bind: Option<Ident>,
        relation: QualIdent,
        args: Vec<RoleArg>,
    },
    /// `x: Entity { field, f2: v }` — entity attribute clause.
    Entity {
        var: Ident,
        entity: Ident,
        fields: Vec<RoleArg>,
    },
    /// `let pat = expr` — local binding.
    Let {
        binds: Ident,
        expr: crate::core::Expr,
    },
    /// `when boolExpr` — guard.
    When(crate::core::Expr),
    /// `any { case {...} ... }` — disjunction; each case is a sub-pattern with
    /// compatible bindings.
    Any(Vec<Pattern>),
    /// `exists { ... }` — existence test, no exported bindings.
    Exists(Pattern),
    /// `without { ... }` — stratified negation (strict read).
    Without(Pattern),
    /// `optional { ... }` — bindings become `Option<T>`, absence is strict.
    Optional(Pattern),
    /// `history R(...)` — mask/supersession-bypassing read.
    History {
        bind: Option<Ident>,
        relation: QualIdent,
        args: Vec<RoleArg>,
    },
    /// `cross { ... }` — explicit disconnected conjunction.
    Cross(Pattern),
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Clause::Edge {
                bind,
                relation,
                args,
            } => {
                if let Some(b) = bind {
                    write!(f, "{b} @ ")?;
                }
                write!(f, "{relation}(")?;
                write_args(f, args)?;
                write!(f, ")")
            }
            Clause::Entity {
                var,
                entity,
                fields,
            } => {
                write!(f, "{var}: {entity} {{ ")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{field}")?;
                }
                write!(f, " }}")
            }
            Clause::Let { binds, expr } => write!(f, "let {binds} = {}", expr.kind),
            Clause::When(expr) => write!(f, "when {}", expr.kind),
            Clause::Any(cases) => {
                write!(f, "any {{ ")?;
                for (i, c) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "case {{ {c} }}")?;
                }
                write!(f, " }}")
            }
            Clause::Exists(p) => write!(f, "exists {{ {p} }}"),
            Clause::Without(p) => write!(f, "without {{ {p} }}"),
            Clause::Optional(p) => write!(f, "optional {{ {p} }}"),
            Clause::History {
                bind,
                relation,
                args,
            } => {
                write!(f, "history ")?;
                if let Some(b) = bind {
                    write!(f, "{b} @ ")?;
                }
                write!(f, "{relation}(")?;
                write_args(f, args)?;
                write!(f, ")")
            }
            Clause::Cross(p) => write!(f, "cross {{ {p} }}"),
        }
    }
}

fn write_args(f: &mut fmt::Formatter<'_>, args: &[RoleArg]) -> fmt::Result {
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{a}")?;
    }
    Ok(())
}

/// A pattern: an ordered conjunction of clauses (source order feeds diagnostics
/// and the cost model only — evaluation is set-based, Part IV §3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Pattern {
    pub clauses: Vec<Clause>,
}

impl Pattern {
    pub fn new(clauses: Vec<Clause>) -> Self {
        Pattern { clauses }
    }

    /// Read-set analysis (Appendix E `Patterns ⇒ ...; Reads; ...`). Returns the
    /// classified relation reads in canonical order (sorted, de-duplicated),
    /// so the result is deterministic and directly consumable by brix-phase.
    ///
    /// `aggregate_reads` names relations known (by the caller's Σ) to be read
    /// through an aggregate-fn call inside a `let`; those are promoted to
    /// `Strict` per Appendix E's aggregate rule ("in-rule use ⇒ strict dep on
    /// every relation in extent(S)"). Since the expression IR is opaque here,
    /// the caller supplies them; passing an empty slice is valid.
    pub fn read_set(&self, aggregate_reads: &[QualIdent]) -> Vec<RelRead> {
        let mut reads: Vec<RelRead> = Vec::new();
        self.collect_reads(ReadContext::Live, &mut reads);
        for rel in aggregate_reads {
            reads.push(RelRead {
                relation: rel.clone(),
                kind: ReadKind::Strict,
            });
        }
        // Canonical order: sort by (relation segments, kind), dedup.
        reads.sort();
        reads.dedup();
        reads
    }

    /// Variables bound by this pattern (the `Bindings` output of the Appendix E
    /// `Patterns` judgment). `exists`/`without` export nothing (Part IV §3);
    /// `optional` exports its bindings as `Option<T>` — recorded here as bound
    /// names, the option-wrapping is a typing concern handled in `core`.
    pub fn bound_vars(&self) -> Vec<Ident> {
        let mut out = Vec::new();
        for clause in &self.clauses {
            match clause {
                Clause::Edge { bind, args, .. } | Clause::History { bind, args, .. } => {
                    if let Some(b) = bind {
                        out.push(b.clone());
                    }
                    push_arg_vars(args, &mut out);
                }
                Clause::Entity { var, fields, .. } => {
                    out.push(var.clone());
                    push_arg_vars(fields, &mut out);
                }
                Clause::Let { binds, .. } => out.push(binds.clone()),
                Clause::Optional(p) => out.extend(p.bound_vars()),
                Clause::Any(cases) => {
                    // Disjunction exports the bindings common to every case
                    // ("compatible bindings", Part IV §3). Approximate with the
                    // intersection of per-case bound sets.
                    if let Some((first, rest)) = cases.split_first() {
                        let mut common = first.bound_vars();
                        for case in rest {
                            let cv = case.bound_vars();
                            common.retain(|v| cv.contains(v));
                        }
                        out.extend(common);
                    }
                }
                Clause::Cross(p) => out.extend(p.bound_vars()),
                // exists/without export nothing; when/guards bind nothing.
                Clause::Exists(_) | Clause::Without(_) | Clause::When(_) => {}
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn collect_reads(&self, ctx: ReadContext, out: &mut Vec<RelRead>) {
        for clause in &self.clauses {
            match clause {
                Clause::Edge { relation, .. } => out.push(RelRead {
                    relation: relation.clone(),
                    kind: ctx.live_kind(),
                }),
                Clause::History { relation, .. } => out.push(RelRead {
                    relation: relation.clone(),
                    kind: ReadKind::History,
                }),
                Clause::Exists(p) => p.collect_reads(ctx.enter_exists(), out),
                Clause::Without(p) => p.collect_reads(ReadContext::Strict, out),
                Clause::Optional(p) => p.collect_reads(ReadContext::Strict, out),
                Clause::Any(cases) => {
                    for c in cases {
                        c.collect_reads(ctx, out);
                    }
                }
                Clause::Cross(p) => p.collect_reads(ctx, out),
                Clause::Entity { .. } | Clause::Let { .. } | Clause::When(_) => {}
            }
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, c) in self.clauses.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

/// The read-classification context threaded through nested patterns.
#[derive(Clone, Copy)]
enum ReadContext {
    Live,
    Strict,
    Exists,
}

impl ReadContext {
    fn live_kind(self) -> ReadKind {
        match self {
            ReadContext::Live => ReadKind::Live,
            ReadContext::Strict => ReadKind::Strict,
            ReadContext::Exists => ReadKind::Exists,
        }
    }
    fn enter_exists(self) -> ReadContext {
        // A negation above an exists stays strict; a plain exists is positive.
        match self {
            ReadContext::Strict => ReadContext::Strict,
            _ => ReadContext::Exists,
        }
    }
}

fn push_arg_vars(args: &[RoleArg], out: &mut Vec<Ident>) {
    for ra in args {
        if let Arg::Var(v) = &ra.arg {
            out.push(v.clone());
        }
    }
}

/// Helper: build an edge clause with plain `role: var` args.
pub fn edge(relation: &str, args: &[(&str, &str)]) -> Clause {
    Clause::Edge {
        bind: None,
        relation: QualIdent::from(relation),
        args: args
            .iter()
            .map(|(r, v)| RoleArg {
                role: Ident::new(*r),
                arg: Arg::Var(Ident::new(*v)),
            })
            .collect(),
    }
}

/// The typing environment slot a role fills, kept minimal for the frontend
/// seam: brix-ir asks the AST/schema lane for a role's declared type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoleTy {
    pub role: Ident,
    pub ty: Ty,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> QualIdent {
        QualIdent::from(s)
    }

    #[test]
    fn live_edge_reads_are_positive() {
        let p = Pattern::new(vec![edge("ComputedPrice", &[("order", "o")])]);
        let reads = p.read_set(&[]);
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].kind, ReadKind::Live);
        assert_eq!(reads[0].relation, q("ComputedPrice"));
    }

    #[test]
    fn without_makes_a_strict_read() {
        let p = Pattern::new(vec![
            edge("Order", &[("id", "o")]),
            Clause::Without(Pattern::new(vec![edge("Delivered", &[("order", "o")])])),
        ]);
        let reads = p.read_set(&[]);
        let delivered = reads.iter().find(|r| r.relation == q("Delivered")).unwrap();
        assert_eq!(delivered.kind, ReadKind::Strict);
    }

    #[test]
    fn history_reads_are_classified_history() {
        let p = Pattern::new(vec![Clause::History {
            bind: None,
            relation: q("ComputedPrice"),
            args: vec![],
        }]);
        assert_eq!(p.read_set(&[])[0].kind, ReadKind::History);
    }

    #[test]
    fn aggregate_reads_are_promoted_to_strict() {
        let p = Pattern::new(vec![edge("Move", &[("vehicle", "v")])]);
        let reads = p.read_set(&[q("Move")]);
        // Move now appears twice pre-dedup (live + strict); both survive dedup
        // because kind differs — the phase lane wants both edges.
        assert!(reads.iter().any(|r| r.kind == ReadKind::Live));
        assert!(reads.iter().any(|r| r.kind == ReadKind::Strict));
    }

    #[test]
    fn read_set_is_canonically_ordered_and_deterministic() {
        let p = Pattern::new(vec![
            edge("Zebra", &[("a", "x")]),
            edge("Alpha", &[("b", "y")]),
        ]);
        let reads = p.read_set(&[]);
        assert_eq!(reads[0].relation, q("Alpha"));
        assert_eq!(reads[1].relation, q("Zebra"));
    }

    #[test]
    fn bound_vars_excludes_without_and_exists_bindings() {
        let p = Pattern::new(vec![
            edge("Order", &[("id", "o")]),
            Clause::Without(Pattern::new(vec![edge("Delivered", &[("order", "d")])])),
        ]);
        let vars = p.bound_vars();
        assert!(vars.contains(&Ident::new("o")));
        assert!(
            !vars.contains(&Ident::new("d")),
            "without exports no bindings (Part IV §3)"
        );
    }

    #[test]
    fn enum_lit_displays_type_and_ordinal_not_the_variant_name() {
        // Mismatch (B): the encoding is the ordinal, never the surface name.
        let l = Lit::Enum {
            ty: q("Tier"),
            ordinal: 1,
        };
        assert_eq!(l.to_string(), "Tier#1");
    }

    #[test]
    fn enum_lit_as_an_edge_arg_round_trips_through_display() {
        let p = Pattern::new(vec![Clause::Edge {
            bind: None,
            relation: q("OrderStatus"),
            args: vec![RoleArg {
                role: Ident::new("value"),
                arg: Arg::Lit(Lit::Enum {
                    ty: q("Status"),
                    ordinal: 0,
                }),
            }],
        }]);
        assert_eq!(p.to_string(), "OrderStatus(value: Status#0)");
    }

    #[test]
    fn any_exports_only_common_bindings() {
        let case_a = Pattern::new(vec![edge("A", &[("x", "shared"), ("y", "onlyA")])]);
        let case_b = Pattern::new(vec![edge("B", &[("x", "shared"), ("z", "onlyB")])]);
        let p = Pattern::new(vec![Clause::Any(vec![case_a, case_b])]);
        let vars = p.bound_vars();
        assert!(vars.contains(&Ident::new("shared")));
        assert!(!vars.contains(&Ident::new("onlyA")));
        assert!(!vars.contains(&Ident::new("onlyB")));
    }
}
