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

/// Rewrite nominal `Ty::Enum` references according to `rename` (a dependency's
/// local enum name -> its package-qualified name), descending through every
/// compound type. A dependency lowered in isolation types its relation roles
/// and function signatures with *bare* enum names (`Enum<Colour>`); when those
/// schemas are re-exported package-qualified we register the enum as
/// `lib.Colour`, so the role/parameter types must be rewritten to match or the
/// merged checker sees `Enum<Colour>` vs `Enum<lib.Colour>` (issue #42 Slice 3).
/// Entity references (`NodeRef`/`EdgeRef`/`ClaimRef`, which carry a bare `Ident`
/// rather than a `QualIdent`) and prelude units are left untouched —
/// cross-package entity-typed roles remain a documented deferral.
fn qualify_ty(
    ty: &brix_ir::types::Ty,
    rename: &std::collections::BTreeMap<brix_ir::ident::QualIdent, brix_ir::ident::QualIdent>,
) -> brix_ir::types::Ty {
    use brix_ir::types::Ty;
    match ty {
        Ty::Enum(name) => Ty::Enum(rename.get(name).cloned().unwrap_or_else(|| name.clone())),
        Ty::Option(t) => Ty::Option(Box::new(qualify_ty(t, rename))),
        Ty::Result(a, b) => Ty::Result(
            Box::new(qualify_ty(a, rename)),
            Box::new(qualify_ty(b, rename)),
        ),
        Ty::List(t) => Ty::List(Box::new(qualify_ty(t, rename))),
        Ty::Vector(t) => Ty::Vector(Box::new(qualify_ty(t, rename))),
        Ty::Set(t) => Ty::Set(Box::new(qualify_ty(t, rename))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(qualify_ty(k, rename)),
            Box::new(qualify_ty(v, rename)),
        ),
        Ty::Bag(t) => Ty::Bag(Box::new(qualify_ty(t, rename))),
        Ty::Rel(r) => Ty::Rel(Box::new(qualify_row(r, rename))),
        Ty::Estimate(t) => Ty::Estimate(Box::new(qualify_ty(t, rename))),
        Ty::Record(r) => Ty::Record(Box::new(qualify_row(r, rename))),
        Ty::Missing(t) => Ty::Missing(Box::new(qualify_ty(t, rename))),
        Ty::Fn {
            params,
            ret,
            effects,
        } => Ty::Fn {
            params: params.iter().map(|t| qualify_ty(t, rename)).collect(),
            ret: Box::new(qualify_ty(ret, rename)),
            effects: effects.clone(),
        },
        other => other.clone(),
    }
}

fn qualify_row(
    r: &brix_ir::types::Row,
    rename: &std::collections::BTreeMap<brix_ir::ident::QualIdent, brix_ir::ident::QualIdent>,
) -> brix_ir::types::Row {
    brix_ir::types::Row {
        fields: r
            .fields
            .iter()
            .map(|f| brix_ir::types::RowField {
                name: f.name.clone(),
                ty: qualify_ty(&f.ty, rename),
            })
            .collect(),
        tail: r.tail.clone(),
    }
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
/// Export surface (issue #42, through Slice 3): a dependency re-exports its
/// relations, protocol-synth relations (`pkg.Proto.request`/`.<outcome>`),
/// entity types, enums, `type` aliases, and self-contained total functions —
/// all package-qualified, with nominal (enum) references in role/parameter
/// types requalified to match. Still deferred: cross-package entity-typed
/// roles (`NodeRef` carries a bare `Ident`), dependency-local rules, and trait
/// `impl`s (no AST decl yet).
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
        // Prepend the package name to an existing (possibly dotted) name, so a
        // protocol-synth relation `Proto.request` becomes `pkg.Proto.request`
        // rather than being dropped (issue #42 Slice 3).
        let qualify_path = |segments: &[IrIdent]| -> QualIdent {
            let mut segs: Vec<IrIdent> = dep
                .name_segments
                .iter()
                .map(|s| IrIdent::new(s.clone()))
                .collect();
            segs.extend(segments.iter().cloned());
            QualIdent::from_segments(segs)
        };
        // A dependency's own prelude (`brix.*`) is seeded per-dep and re-seeded
        // for the graph; never re-export it under `pkg.brix.*`.
        let is_prelude = |name: &QualIdent| name.segments()[0].as_str().starts_with("brix");

        // Map each of the dependency's local enum names to its qualified name,
        // so relation-role and function-signature types that mention them are
        // rewritten to match the qualified enum we register below (issue #42
        // Slice 3 — see `qualify_ty`).
        let mut enum_rename: std::collections::BTreeMap<QualIdent, QualIdent> =
            std::collections::BTreeMap::new();
        for (name, _variants) in dep_lowered.resolver.enums() {
            if is_prelude(name) {
                continue;
            }
            enum_rename.insert(name.clone(), qualify_path(name.segments()));
        }

        // Dependency's own declared relations -> `pkg.Rel`, and protocol-synth
        // relations -> `pkg.Proto.request` / `pkg.Proto.<outcome>` (issue #42
        // Slice 3: dotted names are now qualified, not skipped). Role types are
        // requalified so an enum role points at the qualified enum.
        for schema in dep_lowered.resolver.relations() {
            if is_prelude(&schema.name) {
                continue;
            }
            let qname = qualify_path(schema.name.segments());
            let kind = dep_lowered.resolver.relation_kind(&schema.name);
            let mut qschema = schema.clone();
            qschema.name = qname.clone();
            qschema.roles = qschema
                .roles
                .iter()
                .map(|(n, t)| (n.clone(), qualify_ty(t, &enum_rename)))
                .collect();
            resolver = resolver
                .with_relation(qschema)
                .with_relation_kind(qname, kind);
        }

        // Dependency's entity types, enums, and `type` aliases -> package-
        // qualified names, so `use dep.{Widget, Colour, Meters}` resolves to
        // real nominal types cross-package (issue #42 Slice 3). Entities also
        // came across as relations above; this adds the `type_ns`/entity-set
        // membership those loops don't touch.
        for ent in dep_lowered.resolver.entities() {
            if is_prelude(ent) {
                continue;
            }
            resolver = resolver.with_entity(qualify_path(ent.segments()));
        }
        for (name, variants) in dep_lowered.resolver.enums() {
            if is_prelude(name) {
                continue;
            }
            resolver = resolver.with_enum(qualify_path(name.segments()), variants.to_vec());
        }
        for (name, ty) in dep_lowered.resolver.aliases() {
            if is_prelude(name) {
                continue;
            }
            resolver = resolver.with_alias(qualify_path(name.segments()), ty.clone());
        }

        // Dependency's compiled total functions -> `pkg.fn`, bodies carried in.
        // Signature + body param/ret types are requalified for the same reason
        // as relation roles (an enum-typed parameter must name the qualified
        // enum). Bodies encode enum *values* by ordinal, not name, so they need
        // no rewrite.
        for f in &dep_lowered.source.functions {
            let qname = qualify(&f.name.to_string());
            resolver = resolver.with_function(FnSignature {
                name: qname.clone(),
                params: f
                    .params
                    .iter()
                    .map(|(_, t)| qualify_ty(t, &enum_rename))
                    .collect(),
                ret: qualify_ty(&f.ret, &enum_rename),
                is_aggregate: false,
                may_diverge: f.effects.may_diverge(),
                effects: f.effects.clone(),
            });
            let mut qf = f.clone();
            qf.name = qname;
            qf.params = qf
                .params
                .iter()
                .map(|(n, t)| (n.clone(), qualify_ty(t, &enum_rename)))
                .collect();
            qf.ret = qualify_ty(&qf.ret, &enum_rename);
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
