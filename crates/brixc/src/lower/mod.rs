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
mod resolve;
mod schema;
mod tymap;

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
    use brix_ir::frontend::FnSignature;
    use brix_ir::ident::{Ident as IrIdent, QualIdent};

    let mut diags: Vec<Diagnostic> = root_parse_diags.iter().cloned().collect();
    let mut meta = LowerMeta::default();

    // Deterministic order, independent of how the caller/filesystem enumerated
    // the graph.
    let mut ordered: Vec<&DepPackage> = deps.iter().collect();
    ordered.sort_by(|a, b| a.name_segments.cmp(&b.name_segments));

    let mut resolver = resolve::seed_prelude(ProgramResolver::new());
    let mut dep_fndefs: Vec<FnDef> = Vec::new();

    for dep in &ordered {
        // Lower and check the dependency in isolation (bare names, own
        // prelude); its errors surface tagged into the graph's channel.
        let dep_lowered = lower_file(dep.file, dep.parse_diags);
        diags.extend(dep_lowered.diags.iter().cloned());

        let qualify = |name: &str| -> QualIdent {
            let mut segs: Vec<IrIdent> = dep
                .name_segments
                .iter()
                .map(|s| IrIdent::new(s.clone()))
                .collect();
            segs.push(IrIdent::new(name.to_string()));
            QualIdent::from_segments(segs)
        };

        // Dependency's own declared relations -> `pkg.Rel` (skip prelude
        // `brix.*` and dotted/protocol-synth names, deferred).
        for schema in dep_lowered.resolver.relations() {
            if schema.name.segments().len() != 1 {
                continue;
            }
            let bare = schema.name.segments()[0].as_str();
            if bare.starts_with("brix") {
                continue;
            }
            let qname = qualify(bare);
            let kind = dep_lowered.resolver.relation_kind(&schema.name);
            let mut qschema = schema.clone();
            qschema.name = qname.clone();
            resolver = resolver
                .with_relation(qschema)
                .with_relation_kind(qname, kind);
        }

        // Dependency's compiled total functions -> `pkg.fn`, bodies carried in.
        for f in &dep_lowered.source.functions {
            let qname = qualify(&f.name.to_string());
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
