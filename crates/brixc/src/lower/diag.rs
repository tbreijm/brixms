//! The one diagnostic channel (design §"Error strategy").
//!
//! `brix-diag` owns the one shared diagnostic type. Codes: `BRX-AST-*` (parse,
//! already on the `Diagnostics`
//! handed to [`crate::lower::lower_file`]), `BRX-LOW-*` (lowering, this
//! module), `BRX-IR-*` (rendered [`brix_ir::check::Finding`]s).
//!
//! Order in the final `Vec`: parse diagnostics (source order, already
//! emitted) ++ lowering diagnostics (decl order, pushed as pass 2 walks
//! `File.decls`) ++ rendered `Finding`s (decl order, pushed at the end of
//! [`crate::lower::lower_file`]). Nothing here reorders; callers that want a
//! different order sort afterward.

use brix_diag::{Diagnostic, Span};
use brix_ir::check::Finding;
use brix_ir::ident::{Ident as IrIdent, QualIdent};
use brix_ir::infer::TypeError;

use super::resolve::LowerMeta;

/// `BRX-LOW-0001` — a construct this v0 lowering does not support, found
/// *inside* an otherwise-lowered declaration (Path clause, `match`/closure/
/// block/range in a rule expr, computed head args, destructuring `let`,
/// query `order`/`limit`, ...). Blocks a clean lower (error severity).
pub const UNSUPPORTED_V0: &str = "BRX-LOW-0001";
/// `BRX-LOW-0002` — a whole declaration this v0 lowering defers wholesale
/// (Driver/Scenario/DataRecipe/.../`Decl::Extension`/`Decl::Let`/loose
/// blocks). The decl is omitted from the lowered program; lowering
/// continues. Warning severity — this is what "lowers cleanly" tolerates.
pub const DECL_SKIPPED: &str = "BRX-LOW-0002";
/// `BRX-LOW-0003` — a value identifier in an expression that resolves to
/// neither a bound pattern variable, a declared fn/const, nor an enum
/// variant.
pub const UNBOUND_IDENT: &str = "BRX-LOW-0003";
/// `BRX-LOW-0004` — an unqualified enum-variant name that is ambiguous
/// (matches variants of more than one enum in scope, and no role-type
/// context disambiguates it) or otherwise unresolvable as a variant.
pub const AMBIGUOUS_VARIANT: &str = "BRX-LOW-0004";
/// `BRX-LOW-0005` — a `mask(target) by reason` head whose `target`/`reason`
/// ident is not bound as an edge alias (`x @ R(...)`) in the rule body.
pub const MASK_NOT_EDGE_BOUND: &str = "BRX-LOW-0005";
/// `BRX-LOW-0006` — a `Measured` literal (`<num> <unit>`) whose unit is not
/// in the (stubbed) unit table.
pub const UNKNOWN_UNIT: &str = "BRX-LOW-0006";
/// `BRX-LOW-0007` — an `fn` effect-row entry that is not one of the five
/// known bare atoms (`clock`/`random`/`console`/`panic`/`diverge`); scoped
/// atoms (`net<S>`) are not expressible in the AST's bare-ident effect list.
pub const UNKNOWN_EFFECT: &str = "BRX-LOW-0007";
/// `BRX-LOW-0008` — an integer literal that overflows `i64`.
pub const INT_OVERFLOW: &str = "BRX-LOW-0008";
/// `BRX-LOW-0009` — a `let` that rebinds an already-bound pattern variable
/// (no shadowing, design §"Name/variable resolution").
pub const LET_REBINDS: &str = "BRX-LOW-0009";
/// `BRX-LOW-0010` — `...` (Ellipsis) reached in expression position; the
/// parser round-trips it structurally but lowering cannot compile it.
pub const ELLIPSIS: &str = "BRX-LOW-0010";
/// `BRX-LOW-0011` — a `type` alias whose expansion cycles back to itself.
pub const ALIAS_CYCLE: &str = "BRX-LOW-0011";
/// `BRX-LOW-0012` — a declared-type name (role or fn-sig position) that
/// resolves to neither a builtin, an entity, an enum, nor an alias. Error
/// severity in role position, warning in fn-sig position (design tymap
/// rule).
pub const UNRESOLVED_TYPE: &str = "BRX-LOW-0012";
/// `BRX-LOW-0013` — mismatch (F): a compound unit type (`T / U`) has no
/// `Ty` representation; lowered to `Ty::Var` with this warning.
pub const COMPOUND_UNIT: &str = "BRX-LOW-0013";
/// `BRX-LOW-0014` — a `use` item names a bare identifier that is either (a)
/// ambiguous: imported to two different qualified targets by separate `use`
/// items (issue #42 Slice 2, e.g. `use a.{Widget}` + `use b.{Widget}`), or
/// (b) a duplicate export: imported while a root-local relation/entity/enum/
/// fn/type of the same bare name is also declared in this file. Either way
/// the bare name is not silently resolved to whichever `use` happened to be
/// processed last — the reference must be qualified.
pub const AMBIGUOUS_IMPORT: &str = "BRX-LOW-0014";
/// `BRX-LOW-0015` — a nominal declaration (entity/rel/enum/type/record/
/// protocol) with the same name is declared more than once in a package's
/// merged source files (issue #42 Slice 4: multi-file packages share one flat
/// namespace, so a duplicate nominal name across two files is a duplicate
/// export, caught deterministically rather than silently last-wins). Function
/// declarations are exempt — same name, different signature is an overload.
pub const DUPLICATE_DECL: &str = "BRX-LOW-0015";
/// `BRX-LOW-0016` — a `use` item or reference names a declaration from a
/// dependency package that is package-private (not marked `pub`).
pub const PRIVATE_IMPORT: &str = "BRX-LOW-0016";
/// `BRX-IR-0005` — an expression failed HM/ground-dimension type checking.
pub const TYPE_ERROR: &str = "BRX-IR-0005";
/// `BRX-IR-0006` — Appendix E `pure(B, H)` violated: an impure effect atom
/// reaches the rule's body/head.
pub const RULE_IMPURE: &str = "BRX-IR-0006";
/// `BRX-IR-0007` — Appendix E `det(B, H)` violated.
pub const RULE_NONDETERMINISTIC: &str = "BRX-IR-0007";
/// `BRX-IR-0008` — Appendix E `nondiverge(B, H)` violated.
pub const RULE_DIVERGENT: &str = "BRX-IR-0008";
/// `BRX-IR-0009` — Appendix E `keys(H) ⊆ Bindings` violated: a derived-node
/// head's `keyed by (...)` ident is not bound by the body.
pub const UNBOUND_HEAD_KEY: &str = "BRX-IR-0009";
/// `BRX-IR-0010` — Appendix E mask-head side condition violated: `target`/
/// `reason` is not an edge-bound alias produced by the body.
pub const MASK_REF_NOT_EDGE_BOUND: &str = "BRX-IR-0010";
/// `BRX-IR-0011` — a function body realizes an effect its declared `! { ... }`
/// row does not permit (Part V effect containment; issue #47).
pub const UNDECLARED_FN_EFFECT: &str = "BRX-IR-0011";
/// `BRX-IR-0012` — a `total` function body can fail (`?`); only a `partial fn`
/// may fail (Part V §5; issue #47).
pub const TOTAL_FN_FALLIBLE: &str = "BRX-IR-0012";

pub fn error(code: &'static str, span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, span, msg)
}

pub fn warning(code: &'static str, span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::warning(code, span, msg)
}

fn pkg_id(qname: &QualIdent) -> Option<String> {
    if qname.segments().len() > 1 {
        Some(
            qname.segments()[..qname.segments().len() - 1]
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("."),
        )
    } else {
        None
    }
}

/// Render one `brix-ir` static-semantics [`Finding`] into the shared
/// diagnostic type, using `meta`'s span tables to recover a source location
/// (Core IR nodes carry none). Worst case: the whole-decl span, or an empty
/// span at 0 if even that is missing (should not happen for a decl lowering
/// produced itself, but a Finding is plain data and this must never panic).
pub fn render_finding(finding: &Finding, meta: &LowerMeta) -> Diagnostic {
    let (span, code, pkg) = match finding {
        Finding::NonCanonicalKey { relation, role, .. } => (
            meta.role_span(relation, role)
                .or_else(|| meta.decl_span_by_qual(relation))
                .unwrap_or(Span::empty(0)),
            "BRX-IR-0001",
            pkg_id(relation),
        ),
        Finding::AbsenceWithoutWitness { in_rule, .. } => (decl_span(meta, in_rule), "BRX-IR-0002", None),
        Finding::UnknownRelation { in_rule, .. } => (decl_span(meta, in_rule), "BRX-IR-0003", None),
        Finding::OrdinaryFnOnDerivedRel { in_rule, .. } => {
            (decl_span(meta, in_rule), "BRX-IR-0004", None)
        }
        Finding::ImpureRule { rule } => (decl_span(meta, rule), RULE_IMPURE, None),
        Finding::NondeterministicRule { rule } => (decl_span(meta, rule), RULE_NONDETERMINISTIC, None),
        Finding::DivergentRule { rule } => (decl_span(meta, rule), RULE_DIVERGENT, None),
        Finding::UnboundHeadKey { rule, .. } => (decl_span(meta, rule), UNBOUND_HEAD_KEY, None),
        Finding::MaskRefNotEdgeBound { rule, .. } => {
            (decl_span(meta, rule), MASK_REF_NOT_EDGE_BOUND, None)
        }
        Finding::UndeclaredFnEffect { function, .. } => {
            (decl_span(meta, function), UNDECLARED_FN_EFFECT, None)
        }
        Finding::TotalFnFallible { function } => (decl_span(meta, function), TOTAL_FN_FALLIBLE, None),
    };
    let mut diag = Diagnostic::error(code, span, finding.to_string());
    if let Some(p) = pkg {
        diag.source_id = Some(p);
    }
    diag
}

fn decl_span(meta: &LowerMeta, name: &IrIdent) -> Span {
    meta.decl_span(name).unwrap_or(Span::empty(0))
}

/// Render an inference [`TypeError`]. Core IR expression nodes carry no spans,
/// so most type errors still resolve only to an empty span (locating them needs
/// expression-span threading through `infer_source` — a brix-ir change). The
/// exception is the *function-named* errors (arity / overload resolution),
/// whose `QualIdent` we can map back to its declaration span via `meta` — a
/// real source location instead of `0:0` (issue #42 Slice 5).
pub fn render_type_error(error: &TypeError, meta: &LowerMeta) -> Diagnostic {
    let (span, pkg) = match error {
        TypeError::Arity { function, .. }
        | TypeError::NoMatchingOverload { function, .. }
        | TypeError::AmbiguousOverload { function, .. } => (
            meta.decl_span_by_qual(function)
                .or_else(|| {
                    function
                        .segments()
                        .last()
                        .and_then(|seg| meta.decl_span(&IrIdent::new(seg.as_str())))
                })
                .unwrap_or(Span::empty(0)),
            pkg_id(function),
        ),
        _ => (Span::empty(0), None),
    };
    let mut diag = Diagnostic::error(TYPE_ERROR, span, error.to_string());
    if let Some(p) = pkg {
        diag.source_id = Some(p);
    }
    diag
}
