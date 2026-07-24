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
pub mod stdlib;
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
        diags.push(diag::render_type_error(&error, &meta));
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

/// Merge the several source files of ONE package into a single [`File`] with a
/// flat declaration namespace (issue #42 Slice 4). A package spread across
/// `src/**/*.brix` compiles as if its files' `use` items and declarations were
/// concatenated; the caller passes the files already sorted by path so
/// filesystem enumeration order can never affect the result. The merged file
/// keeps the first file's `package`/`module` header (headers on the others are
/// flat-model metadata, not separate namespaces).
///
/// Returns the merged file plus any duplicate-declaration diagnostics: a
/// *nominal* decl name (entity/rel/enum/type/record/protocol) declared in more
/// than one place is a duplicate export (`BRX-LOW-0015`), flagged at every
/// occurrence. Functions are exempt — a repeated `fn` name is an overload.
pub fn merge_files(files: &[&File]) -> (File, Vec<Diagnostic>) {
    use brix_ast::ast::Decl;

    let mut uses = Vec::new();
    let mut decls = Vec::new();
    for f in files {
        uses.extend(f.uses.iter().cloned());
        decls.extend(f.decls.iter().cloned());
    }

    // A nominal decl's name, if it introduces one into the shared namespace.
    // `fn` (overloadable) and structural/`Extension`/`Error` decls are skipped.
    fn nominal_name(decl: &Decl) -> Option<&str> {
        match decl {
            Decl::Entity(d) => Some(d.name.text.as_str()),
            Decl::Rel(d) => Some(d.name.text.as_str()),
            Decl::Enum(d) => Some(d.name.text.as_str()),
            Decl::Type(d) => Some(d.name.text.as_str()),
            Decl::Record(d) => Some(d.name.text.as_str()),
            Decl::Protocol(d) => Some(d.name.text.as_str()),
            _ => None,
        }
    }

    // Count nominal names across the whole package, then flag every occurrence
    // of any name seen more than once (deterministic: decls are already in
    // sorted-file then source order).
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for d in &decls {
        if let Some(name) = nominal_name(d) {
            *counts.entry(name).or_insert(0) += 1;
        }
    }
    let mut diags = Vec::new();
    for d in &decls {
        if let Some(name) = nominal_name(d) {
            if counts.get(name).copied().unwrap_or(0) > 1 {
                diags.push(diag::error(
                    diag::DUPLICATE_DECL,
                    d.span(),
                    format!("duplicate declaration `{name}` in this package's source files"),
                ));
            }
        }
    }

    let span = files.first().map(|f| f.span).unwrap_or_default();
    let package = files.first().and_then(|f| f.package.clone());
    let module = files.first().and_then(|f| f.module.clone());
    (
        File {
            span,
            package,
            module,
            uses,
            decls,
        },
        diags,
    )
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

    let mut resolver = stdlib::stdlib_resolver().clone();
    let mut dep_fndefs: Vec<FnDef> = Vec::new();

    for dep in &ordered {
        // Lower and check the dependency in isolation (bare names, own
        // prelude); its errors surface tagged into the graph's channel.
        let dep_lowered = lower_file(dep.file, dep.parse_diags);
        // A dependency's diagnostics carry spans into ITS OWN source, which the
        // build renders against the ROOT source — a wrong caret. Until the
        // diagnostic renderer is source-aware (a diag-lane follow-up), attribute
        // each to its package in the message and drop the cross-source span, so
        // a dependency error reads honestly ("dependency `lib`: ...") rather
        // than pointing at an unrelated root line (issue #42 Slice 5).
        let dep_name = dep.name_segments.join(".");
        diags.extend(dep_lowered.diags.iter().map(|d| {
            let mut d = d.clone();
            d.message = format!("dependency `{dep_name}`: {}", d.message);
            d.span = brix_diag::Span::empty(0);
            d
        }));

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

        let is_pub = |name: &QualIdent| -> bool {
            use brix_ast::ast::Decl;
            let decl_name = name.segments().first().map(|s| s.as_str()).unwrap_or("");
            for d in &dep.file.decls {
                let d_name = match d {
                    Decl::Entity(e) => Some(e.name.text.as_str()),
                    Decl::Rel(r) => Some(r.name.text.as_str()),
                    Decl::Enum(e) => Some(e.name.text.as_str()),
                    Decl::Type(t) => Some(t.name.text.as_str()),
                    Decl::Record(r) => Some(r.name.text.as_str()),
                    Decl::Protocol(p) => Some(p.name.text.as_str()),
                    Decl::Fn(f) => Some(f.name.text.as_str()),
                    Decl::Unit(u) => Some(u.name.text.as_str()),
                    Decl::Measure(m) => Some(m.name.text.as_str()),
                    Decl::DataRecipe(r) => Some(r.name.text.as_str()),
                    Decl::Feature(f) => Some(f.name.text.as_str()),
                    Decl::FeatureSet(f) => Some(f.name.text.as_str()),
                    Decl::Dataset(d) => Some(d.name.text.as_str()),
                    Decl::StatModel(s) => Some(s.name.text.as_str()),
                    Decl::MlWorkflow(m) => Some(m.name.text.as_str()),
                    Decl::Experiment(e) => Some(e.name.text.as_str()),
                    Decl::Visualization(v) => Some(v.name.text.as_str()),
                    _ => None,
                };
                if let Some(dn) = d_name {
                    if dn == decl_name {
                        return d.vis().is_public();
                    }
                }
            }
            false
        };

        // Register all package-private symbols into `private_symbols` for diagnostic checks
        for d in &dep.file.decls {
            if !d.vis().is_public() {
                use brix_ast::ast::Decl;
                let d_name = match d {
                    Decl::Entity(e) => Some(e.name.text.as_str()),
                    Decl::Rel(r) => Some(r.name.text.as_str()),
                    Decl::Enum(e) => Some(e.name.text.as_str()),
                    Decl::Type(t) => Some(t.name.text.as_str()),
                    Decl::Record(r) => Some(r.name.text.as_str()),
                    Decl::Protocol(p) => Some(p.name.text.as_str()),
                    Decl::Fn(f) => Some(f.name.text.as_str()),
                    Decl::Unit(u) => Some(u.name.text.as_str()),
                    Decl::Measure(m) => Some(m.name.text.as_str()),
                    Decl::DataRecipe(r) => Some(r.name.text.as_str()),
                    Decl::Feature(f) => Some(f.name.text.as_str()),
                    Decl::FeatureSet(f) => Some(f.name.text.as_str()),
                    Decl::Dataset(d) => Some(d.name.text.as_str()),
                    Decl::StatModel(s) => Some(s.name.text.as_str()),
                    Decl::MlWorkflow(m) => Some(m.name.text.as_str()),
                    Decl::Experiment(e) => Some(e.name.text.as_str()),
                    Decl::Visualization(v) => Some(v.name.text.as_str()),
                    _ => None,
                };
                if let Some(dn) = d_name {
                    let qname = qualify(dn);
                    resolver = resolver.with_private_symbol(qname, dep_name.clone());
                }
            }
        }

        // Map each of the dependency's local enum names to its qualified name,
        // so relation-role and function-signature types that mention them are
        // rewritten to match the qualified enum we register below (issue #42
        // Slice 3 — see `qualify_ty`).
        let mut enum_rename: std::collections::BTreeMap<QualIdent, QualIdent> =
            std::collections::BTreeMap::new();
        for (name, _variants) in dep_lowered.resolver.enums() {
            if is_prelude(name) || !is_pub(name) {
                continue;
            }
            enum_rename.insert(name.clone(), qualify_path(name.segments()));
        }

        // Dependency's own declared relations -> `pkg.Rel`, and protocol-synth
        // relations -> `pkg.Proto.request` / `pkg.Proto.<outcome>` (issue #42
        // Slice 3: dotted names are now qualified, not skipped). Role types are
        // requalified so an enum role points at the qualified enum.
        for schema in dep_lowered.resolver.relations() {
            if is_prelude(&schema.name) || !is_pub(&schema.name) {
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
            if is_prelude(ent) || !is_pub(ent) {
                continue;
            }
            resolver = resolver.with_entity(qualify_path(ent.segments()));
        }
        for (name, variants) in dep_lowered.resolver.enums() {
            if is_prelude(name) || !is_pub(name) {
                continue;
            }
            resolver = resolver.with_enum(qualify_path(name.segments()), variants.to_vec());
        }
        for (name, ty) in dep_lowered.resolver.aliases() {
            if is_prelude(name) || !is_pub(name) {
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
            let local_qname = QualIdent::simple(f.name.to_string());
            if !is_pub(&local_qname) {
                continue;
            }
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
        diags.push(diag::render_type_error(&error, &meta));
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
