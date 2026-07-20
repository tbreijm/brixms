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
use brix_ir::ident::Ident as IrIdent;
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
/// `BRX-PKG-0001` — a multi-file package's non-entry source (`src/<mod>.brix`)
/// declares its own `package NAME @ VERSION`; only the package entry
/// (`src/world.brix`) may carry package identity (issue #42).
pub const PACKAGE_DECL_OUTSIDE_ROOT: &str = "BRX-PKG-0001";
/// `BRX-PKG-0002` — two files in the same package export a decl of the same
/// bare name (one module's export would silently shadow another's); the
/// second declaration is rejected rather than picked arbitrarily.
pub const DUPLICATE_EXPORT: &str = "BRX-PKG-0002";
/// `BRX-PKG-0003` — a `reimport` in the package entry file names a
/// submodule/item this package does not have (unknown qualifier, or an
/// unknown bare name inside a submodule that *is* known).
pub const UNKNOWN_REIMPORT_TARGET: &str = "BRX-PKG-0003";
/// `BRX-PKG-0004` — `reimport` appears outside the package entry file
/// (`src/world.brix`); only the entry may publish the package-root surface
/// (mirrors [`PACKAGE_DECL_OUTSIDE_ROOT`]'s "entry only" rule).
pub const REIMPORT_OUTSIDE_ROOT: &str = "BRX-PKG-0004";
/// `BRX-PKG-0005` — two `reimport`s (or a `reimport` and an entry-file decl)
/// publish the same package-root bare name from different targets.
pub const DUPLICATE_REIMPORT: &str = "BRX-PKG-0005";
/// `BRX-LOW-0014` — two `use` declarations in the same file claim the same
/// local alias/prefix (whether introduced by an explicit `as Ident` or by
/// the default last-segment alias of a bare `use path`) for different
/// targets — the whole point of `as` is to let same-named symbols from
/// different places coexist, so a colliding alias is a hard error rather
/// than last-write-wins.
pub const DUPLICATE_USE_ALIAS: &str = "BRX-LOW-0014";

pub fn error(code: &'static str, span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(code, span, msg)
}

pub fn warning(code: &'static str, span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::warning(code, span, msg)
}

/// Render one `brix-ir` static-semantics [`Finding`] into the shared
/// diagnostic type, using `meta`'s span tables to recover a source location
/// (Core IR nodes carry none). Worst case: the whole-decl span, or an empty
/// span at 0 if even that is missing (should not happen for a decl lowering
/// produced itself, but a Finding is plain data and this must never panic).
pub fn render_finding(finding: &Finding, meta: &LowerMeta) -> Diagnostic {
    let (span, code) = match finding {
        Finding::NonCanonicalKey { relation, role, .. } => (
            meta.role_span(relation, role)
                .or_else(|| meta.decl_span_by_qual(relation))
                .unwrap_or(Span::empty(0)),
            "BRX-IR-0001",
        ),
        Finding::AbsenceWithoutWitness { in_rule, .. } => (decl_span(meta, in_rule), "BRX-IR-0002"),
        Finding::UnknownRelation { in_rule, .. } => (decl_span(meta, in_rule), "BRX-IR-0003"),
        Finding::OrdinaryFnOnDerivedRel { in_rule, .. } => {
            (decl_span(meta, in_rule), "BRX-IR-0004")
        }
        Finding::ImpureRule { rule } => (decl_span(meta, rule), RULE_IMPURE),
        Finding::NondeterministicRule { rule } => (decl_span(meta, rule), RULE_NONDETERMINISTIC),
        Finding::DivergentRule { rule } => (decl_span(meta, rule), RULE_DIVERGENT),
        Finding::UnboundHeadKey { rule, .. } => (decl_span(meta, rule), UNBOUND_HEAD_KEY),
        Finding::MaskRefNotEdgeBound { rule, .. } => {
            (decl_span(meta, rule), MASK_REF_NOT_EDGE_BOUND)
        }
        Finding::UndeclaredFnEffect { function, .. } => {
            (decl_span(meta, function), UNDECLARED_FN_EFFECT)
        }
        Finding::TotalFnFallible { function } => (decl_span(meta, function), TOTAL_FN_FALLIBLE),
    };
    Diagnostic::error(code, span, finding.to_string())
}

fn decl_span(meta: &LowerMeta, name: &IrIdent) -> Span {
    meta.decl_span(name).unwrap_or(Span::empty(0))
}

pub fn render_type_error(error: &TypeError) -> Diagnostic {
    Diagnostic::error(TYPE_ERROR, Span::empty(0), error.to_string())
}
