//! AST → Core-IR lowering (issue #6; design ruling: see the PR description).
//!
//! ```text
//! source -parse_file-> (ast::File, ast::Diagnostics)   [total, never Err]
//!        -lower_file-> Lowered { source, resolver, meta, diags }
//!        -check-> diags ∪ rendered ir::check::Finding    [run inside Lower stage]
//! ```
//!
//! Two passes (design §"Pass 1"/"Pass 2"): [`schema`] walks `File.uses` +
//! `File.decls` into the [`resolve::ProgramResolver`]'s schema tables
//! (relations/fns/enums/aliases/units/imports), never looking inside a rule
//! body; [`decl`] then walks the same decls again, lowering `derive`/
//! `constraint`/`query` bodies into Core IR (via [`expr`]/[`tymap`]) and
//! dispatching the v0 defer line for everything else. [`lower_file`] runs
//! both passes, then folds `brix_ir::check::Finding`s (from
//! `check_relation_keys` over every schema and `check_rule` over every
//! `Rule`) into the one diagnostic channel ([`diag`]).

mod decl;
mod diag;
mod expr;
mod package;
mod resolve;
mod schema;
mod tymap;

pub use package::{lower_package, lower_program, FileReport, PackageLowered, SubmoduleInput};
pub use resolve::{
    FnInfo, LowerMeta, ProgramResolver, RuntimeRelationKind, UnitClass, VariantLookup,
};

use brix_ast::File;
use brix_diag::{Diagnostic, Severity};
use brix_ir::check::{check_function, check_relation_keys, check_rule};
use brix_ir::frontend::FrontendSource;
use brix_ir::infer::infer_source;

use crate::pipeline::{Frontend, Lower, PipelineError};

/// The whole-program lowering output: Core IR ([`FrontendSource`]) plus the
/// resolver it type-checks against, the v0 side tables ([`LowerMeta`]), and
/// the one diagnostic channel (design §"Error strategy": parse diags ++
/// lowering diags (decl order) ++ rendered `Finding`s (decl order)).
#[derive(Default)]
pub struct Lowered {
    pub source: FrontendSource,
    pub resolver: ProgramResolver,
    pub meta: LowerMeta,
    pub diags: Vec<Diagnostic>,
}

impl Lowered {
    /// The one gate between a (possibly poisoned) `Lowered` and any
    /// downstream stage (design: "`Lowered::has_errors()` gate ... is the
    /// ONLY thing between poisoned IR and downstream").
    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }
}

/// Lower an already-parsed file. `parse_diags` rides first in the returned
/// channel (source order), then lowering diagnostics (decl order), then
/// rendered static-semantics `Finding`s (decl order).
pub fn lower_file(file: &File, parse_diags: &brix_ast::Diagnostics) -> Lowered {
    let mut diags: Vec<Diagnostic> = parse_diags.iter().cloned().collect();
    let mut meta = LowerMeta::default();

    let resolver = schema::build(file, &mut meta, &mut diags);
    let mut source = decl::lower_decls(file, &resolver, &mut meta, &mut diags);

    for schema in resolver.relations() {
        for finding in check_relation_keys(schema) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }
    for rule in &source.rules {
        for finding in check_rule(rule, &resolver) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }
    for error in infer_source(&mut source, &resolver) {
        diags.push(diag::render_type_error(&error));
    }
    for function in &source.functions {
        for finding in check_function(function, &resolver) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }

    Lowered {
        source,
        resolver,
        meta,
        diags,
    }
}

/// One dependency package to fold into a graph build (issue #42): its package
/// name as segments (e.g. `["lib"]` or `["acme","lib"]`), its parsed entry
/// file, and that file's parse diagnostics.
pub struct DepPackage<'a> {
    pub name_segments: Vec<String>,
    pub file: &'a File,
    pub parse_diags: &'a brix_ast::Diagnostics,
    /// The dependency's own local submodules (issue #42), when it is itself
    /// a multi-file package. Empty for a single-file dependency — the
    /// pre-#42 shape, unchanged.
    pub submodules: &'a [package::SubmoduleInput<'a>],
}

/// Lower a **locked package graph** into one checked [`Lowered`] (issue #42).
///
/// The **root** package keeps bare-name decls (identical to [`lower_file`], so
/// every single-package program/test is unchanged); each **dependency** has
/// its declared relations and compiled total functions re-registered under
/// **package-qualified** names (`lib.Widget`, `lib.scale`) and merged into one
/// resolver. The root's `use lib.{Widget, scale}` already rewrites those bare
/// references to `lib.Widget`/`lib.scale` (via `process_uses`), so cross-package
/// resolution "just works" against the qualified symbols. Dependencies are
/// processed in package-name order for determinism.
///
/// Slice-1 scope: dependency exports are single-name relations (builtin role
/// types) and self-contained total functions; dep-local nominal types, dep
/// rules, and dotted/protocol exports are deferred.
pub fn lower_graph(
    root: &File,
    root_parse_diags: &brix_ast::Diagnostics,
    deps: &[DepPackage],
) -> Lowered {
    use brix_ir::core::FnDef;

    let mut diags: Vec<Diagnostic> = root_parse_diags.iter().cloned().collect();
    let mut meta = LowerMeta::default();

    // Deterministic order, independent of how the caller/filesystem enumerated
    // the graph.
    let mut ordered: Vec<&DepPackage> = deps.iter().collect();
    ordered.sort_by(|a, b| a.name_segments.cmp(&b.name_segments));

    let mut resolver = resolve::seed_prelude(ProgramResolver::new());
    let mut dep_fndefs: Vec<FnDef> = Vec::new();

    for dep in &ordered {
        let (r, fndefs) = fold_dependency(resolver, dep, &mut diags);
        resolver = r;
        dep_fndefs.extend(fndefs);
    }

    // Root package (bare names) on top of the prelude + dependency exports.
    resolver = schema::build_onto(root, resolver, &mut meta, &mut diags);
    let mut source = decl::lower_decls(root, &resolver, &mut meta, &mut diags);
    source.functions.extend(dep_fndefs);

    // Whole-graph checks over the merged source + resolver.
    for schema in resolver.relations() {
        for finding in check_relation_keys(schema) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }
    for rule in &source.rules {
        for finding in check_rule(rule, &resolver) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }
    for error in infer_source(&mut source, &resolver) {
        diags.push(diag::render_type_error(&error));
    }
    for function in &source.functions {
        for finding in check_function(function, &resolver) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }

    Lowered {
        source,
        resolver,
        meta,
        diags,
    }
}

/// Lower one dependency package in isolation (bare names, own prelude) and
/// re-register its exported relations/functions under `pkg.name` onto
/// `resolver` — the mechanism both [`lower_graph`] and
/// [`package::lower_program`] (issue #42) share. Returns the updated
/// resolver plus the dependency's compiled `FnDef`s (already renamed),
/// which the caller folds into its own [`brix_ir::frontend::FrontendSource`].
///
/// Slice-1 scope: dependency exports are single-name relations (builtin role
/// types) and self-contained total functions; dep-local nominal types, dep
/// rules, and dotted/protocol relation exports are deferred. A dependency
/// that is itself a multi-file package may already export dotted function
/// names (`order.min`); those splice on as clean extra segments (not one
/// dot-joined segment), so `use brix.math.order.{min}` resolves identically
/// to a same-package reference.
pub(crate) fn fold_dependency(
    mut resolver: ProgramResolver,
    dep: &DepPackage,
    diags: &mut Vec<Diagnostic>,
) -> (ProgramResolver, Vec<brix_ir::core::FnDef>) {
    use brix_ir::frontend::{FnSignature, SchemaResolver};
    use brix_ir::ident::{Ident as IrIdent, QualIdent};
    use std::collections::BTreeMap;

    let mut dep_fndefs = Vec::new();
    // Package-root re-exports (`reimport`, issue #93-style): captured from
    // `PackageLowered` *before* `into_lowered()` discards everything that
    // isn't shaped like a single-file `Lowered` — a dependency with no
    // submodules can never declare a `reimport` (nothing to promote), so
    // this stays empty on that branch.
    let mut dep_reexports: BTreeMap<String, QualIdent> = BTreeMap::new();
    let dep_lowered = if dep.submodules.is_empty() {
        lower_file(dep.file, dep.parse_diags)
    } else {
        let pkg = package::lower_package(dep.file, dep.parse_diags, "<dependency>", dep.submodules);
        dep_reexports = pkg.reexports.clone();
        pkg.into_lowered()
    };
    diags.extend(dep_lowered.diags.iter().cloned());

    let qualify = |name_segs: &[IrIdent]| -> QualIdent {
        let mut segs: Vec<IrIdent> = dep
            .name_segments
            .iter()
            .map(|s| IrIdent::new(s.clone()))
            .collect();
        segs.extend(name_segs.iter().cloned());
        QualIdent::from_segments(segs)
    };

    for schema in dep_lowered.resolver.relations() {
        if schema.name.segments().len() != 1 {
            continue;
        }
        let bare = schema.name.segments()[0].as_str();
        if bare.starts_with("brix") {
            continue;
        }
        let qname = qualify(&[IrIdent::new(bare.to_string())]);
        let kind = dep_lowered.resolver.relation_kind(&schema.name);
        let mut qschema = schema.clone();
        qschema.name = qname.clone();
        resolver = resolver
            .with_relation(qschema)
            .with_relation_kind(qname, kind);
    }

    for f in &dep_lowered.source.functions {
        let qname = qualify(f.name.segments());
        resolver = resolver.with_function(FnSignature {
            name: qname.clone(),
            params: f.params.iter().map(|(_, t)| t.clone()).collect(),
            ret: f.ret.clone(),
            is_aggregate: false,
            may_diverge: f.effects.may_diverge(),
            effects: f.effects.clone(),
        });
        let mut qf = f.clone();
        qf.name = qname;
        dep_fndefs.push(qf);
    }

    // Publish each `reimport`ed name at the package root too: the *same*
    // already-lowered target signature(s)/body (there may be more than one
    // typed overload, e.g. `order.clamp(Int, Int, Int)` and `(Float, Float,
    // Float)`) under a second, shorter qualified name — `brix.math.clamp`
    // alongside the target's own `brix.math.order.clamp` — never a cloned
    // source declaration.
    for (bare, target) in &dep_reexports {
        let qname = qualify(&[IrIdent::new(bare.clone())]);
        for sig in dep_lowered.resolver.functions(target) {
            resolver = resolver.with_function(FnSignature {
                name: qname.clone(),
                params: sig.params.clone(),
                ret: sig.ret.clone(),
                is_aggregate: sig.is_aggregate,
                may_diverge: sig.may_diverge,
                effects: sig.effects.clone(),
            });
        }
        for f in &dep_lowered.source.functions {
            if f.name == *target {
                let mut qf = f.clone();
                qf.name = qname.clone();
                dep_fndefs.push(qf);
            }
        }
    }

    (resolver, dep_fndefs)
}

/// The `Frontend` seam (design §"Seams"): parsing is total, so this is
/// always `Ok`; parse diagnostics ride inside the artifact, not the
/// `Result`.
pub struct AstFrontend;

impl Frontend for AstFrontend {
    type Ast = (File, brix_ast::Diagnostics);
    fn parse(&self, source: &str) -> Result<Self::Ast, PipelineError> {
        Ok(brix_ast::parse_file(source))
    }
}

/// The `Lower` seam: lowering is also total (never `Err`) — a decl that
/// can't be lowered degrades to a skip/error diagnostic, never a `Result`
/// short-circuit. `PhaseAssign` (App. F) is the stage that refuses to run
/// when [`Lowered::has_errors`].
pub struct AstLower;

impl Lower for AstLower {
    type Ast = (File, brix_ast::Diagnostics);
    type Ir = Lowered;
    fn lower(&self, ast: Self::Ast) -> Result<Self::Ir, PipelineError> {
        let (file, parse_diags) = ast;
        Ok(lower_file(&file, &parse_diags))
    }
}
