//! Multi-file **packages** (issue #42): merge a package's entry file
//! (`src/world.brix`) with its sibling submodule files (`src/<name>.brix`)
//! into one checked program, and — when the package also declares
//! dependencies — fold those in too (reusing [`super::fold_dependency`],
//! the same package-qualification mechanism [`super::lower_graph`] uses).
//!
//! # Submodule qualification
//!
//! A submodule file's path stem becomes its module qualifier: `src/order.brix`
//! exports `order.min`, `order.max`, ... Callers anywhere in the package may
//! reference those either **qualified** (`order.min(a, b)` — this already
//! "just works" through [`super::resolve::ProgramResolver::resolve_path`]'s
//! existing multi-segment fallback, no change needed there) or **bare**
//! (`min(a, b)`) — bare access is wired by auto-registering every submodule
//! export as an import alias (mirroring a hand-written `use order.{min}`),
//! as long as no other file in the package already claims that bare name; a
//! collision is a hard error ([`super::diag::DUPLICATE_EXPORT`]), never
//! silent shadowing.
//!
//! # Two passes across files
//!
//! Every file's **schema** (pass 1: relation/fn signatures, types, units) is
//! registered first, for every file, before any file's **decl bodies** (pass
//! 2) are lowered — exactly like a single file's own forward-reference
//! safety (see `schema.rs`'s own doc comment), just widened to the whole
//! package. This is what lets `interp.brix` call `order.clamp` (or bare
//! `clamp`) regardless of filesystem enumeration order; submodules are
//! sorted by qualifier before either pass runs, so reordering files on disk
//! cannot change the result (issue #42 acceptance).
//!
//! # Diagnostic provenance
//!
//! Unlike [`super::lower_graph`]'s dependency merge (which discards a
//! dependency's own span table — an accepted, pre-existing v0 trade-off),
//! each **local** file here keeps its own diagnostics bucketed under its own
//! path ([`PackageLowered::reports`]), so `brix check`/`build` can render
//! every parse/schema/decl diagnostic against the file it actually came
//! from. Only the final whole-*program* checks (relation-key/rule/function,
//! run once over the fully merged resolver+source) are routed after the
//! fact, by recognizing a finding's qualified name's module-qualifier
//! prefix; a finding with no recognizable prefix (or an `infer_source` type
//! error, which carries no span at all today — single-file or not) lands in
//! the entry file's bucket.

use std::collections::BTreeSet;

use brix_ast::ast::Decl;
use brix_ast::File;
use brix_diag::{Diagnostic, Severity, Span};
use brix_ir::check::{check_function, check_relation_keys, check_rule, Finding};
use brix_ir::core::FnDef;
use brix_ir::frontend::FrontendSource;
use brix_ir::ident::QualIdent;
use brix_ir::infer::infer_source;

use super::resolve::{seed_prelude, LowerMeta, ProgramResolver};
use super::{decl, diag, schema, DepPackage};

/// One local submodule source to fold into [`lower_package`]/[`lower_program`]:
/// its module qualifier (the file stem, e.g. `"order"` for `src/order.brix`),
/// its parsed AST, and that file's own parse diagnostics.
pub struct SubmoduleInput<'a> {
    pub qualifier: String,
    pub file: &'a File,
    pub parse_diags: &'a brix_ast::Diagnostics,
}

/// One file's diagnostics, labeled by the path `brix check`/`build` should
/// render them against.
pub struct FileReport {
    pub label: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// The result of lowering a whole package (entry file + submodules, and —
/// via [`lower_program`] — its dependency graph) into one checked program.
#[derive(Default)]
pub struct PackageLowered {
    pub source: FrontendSource,
    pub resolver: ProgramResolver,
    pub meta: LowerMeta,
    /// One entry per input file (entry first, then submodules in qualifier
    /// order), even when it has no diagnostics.
    pub reports: Vec<FileReport>,
}

impl PackageLowered {
    pub fn has_errors(&self) -> bool {
        self.reports
            .iter()
            .any(|r| r.diagnostics.iter().any(|d| d.severity == Severity::Error))
    }

    /// Flatten into a single-source-shaped [`super::Lowered`] (all files'
    /// diagnostics concatenated, entry-file order first) — for stages
    /// downstream of lowering that do not yet distinguish origin files
    /// (phase-assign).
    pub fn into_lowered(self) -> super::Lowered {
        super::Lowered {
            source: self.source,
            resolver: self.resolver,
            meta: self.meta,
            diags: self
                .reports
                .into_iter()
                .flat_map(|r| r.diagnostics)
                .collect(),
        }
    }
}

/// Lower a package's entry file plus its local submodules (issue #42) — no
/// dependencies. See [`lower_program`] for entry + submodules + deps
/// together.
pub fn lower_package(
    entry: &File,
    entry_parse_diags: &brix_ast::Diagnostics,
    entry_label: &str,
    submodules: &[SubmoduleInput],
) -> PackageLowered {
    lower_program(entry, entry_parse_diags, entry_label, &[], submodules)
}

/// Lower a package's full input: its entry file, its resolved dependency
/// graph ([`DepPackage`], issue #42), and its local submodule files. Prelude
/// → dependencies (package-qualified, via [`super::fold_dependency`]) →
/// entry + submodules (two-pass, module-qualified) — one merged resolver and
/// program, checked once at the end.
pub fn lower_program(
    entry: &File,
    entry_parse_diags: &brix_ast::Diagnostics,
    entry_label: &str,
    deps: &[DepPackage],
    submodules: &[SubmoduleInput],
) -> PackageLowered {
    let mut meta = LowerMeta::default();
    let mut resolver = seed_prelude(ProgramResolver::new());

    let mut ordered_deps: Vec<&DepPackage> = deps.iter().collect();
    ordered_deps.sort_by(|a, b| a.name_segments.cmp(&b.name_segments));
    let mut entry_diags: Vec<Diagnostic> = entry_parse_diags.iter().cloned().collect();
    let mut dep_fndefs: Vec<FnDef> = Vec::new();
    for dep in &ordered_deps {
        let (r, fndefs) = super::fold_dependency(resolver, dep, &mut entry_diags);
        resolver = r;
        dep_fndefs.extend(fndefs);
    }

    // Deterministic order, independent of filesystem enumeration (issue #42
    // acceptance: reordering files must not change the result).
    let mut ordered: Vec<&SubmoduleInput> = submodules.iter().collect();
    ordered.sort_by(|a, b| a.qualifier.cmp(&b.qualifier));

    // Every bare top-level name already spoken for — the entry file's own
    // decls first, so a submodule can never shadow it.
    let mut claimed_bare: BTreeSet<String> = decl_names(entry).into_iter().collect();

    struct Prepared {
        label: String,
        qualifier: String,
        file: File,
        diags: Vec<Diagnostic>,
    }

    let mut prepared: Vec<Prepared> = Vec::new();
    for sm in &ordered {
        let mut diags: Vec<Diagnostic> = sm.parse_diags.iter().cloned().collect();
        if let Some(pkg) = &sm.file.package {
            diags.push(diag::error(
                diag::PACKAGE_DECL_OUTSIDE_ROOT,
                pkg.span,
                format!(
                    "`package` declaration is only allowed in the package entry file; `src/{}.brix` may not declare one",
                    sm.qualifier
                ),
            ));
        }
        // Dedupe this file's own bare names first: two `fn` overloads
        // sharing one name (e.g. `min(Int, Int)` and `min(Float, Float)`
        // both in `order.brix`) are one bare-name claim, not two — only a
        // *different* file claiming an already-claimed name is the hard
        // error the module doc promises. Keeps the first occurrence's span
        // so a genuine cross-file collision still points somewhere real.
        let mut this_file_names: std::collections::BTreeMap<String, Span> =
            std::collections::BTreeMap::new();
        for (name, span) in decl_names_with_spans(sm.file) {
            this_file_names.entry(name).or_insert(span);
        }
        for (name, span) in this_file_names {
            if claimed_bare.insert(name.clone()) {
                resolver = resolver.with_import(
                    name.clone(),
                    QualIdent::from(format!("{}.{}", sm.qualifier, name).as_str()),
                );
            } else {
                diags.push(diag::error(
                    diag::DUPLICATE_EXPORT,
                    span,
                    format!(
                        "`{name}` is exported by more than one module in this package; module `{}`'s declaration is dropped",
                        sm.qualifier
                    ),
                ));
            }
        }
        prepared.push(Prepared {
            label: format!("src/{}.brix", sm.qualifier),
            qualifier: sm.qualifier.clone(),
            file: qualify_file(sm.file, &sm.qualifier),
            diags,
        });
    }

    // Pass 1 (all files): register every signature before lowering any body.
    resolver = schema::build_onto(entry, resolver, &mut meta, &mut entry_diags);
    for pf in &mut prepared {
        resolver = schema::build_onto(&pf.file, resolver, &mut meta, &mut pf.diags);
    }

    // Pass 2 (all files): lower bodies against the fully-populated resolver.
    let mut source = decl::lower_decls(entry, &resolver, &mut meta, &mut entry_diags);
    source.functions.extend(dep_fndefs);
    for pf in &mut prepared {
        let sub_source = decl::lower_decls(&pf.file, &resolver, &mut meta, &mut pf.diags);
        source.functions.extend(sub_source.functions);
        source.rules.extend(sub_source.rules);
        source.constraints.extend(sub_source.constraints);
        source.queries.extend(sub_source.queries);
    }

    // Whole-program checks, routed back to the file each finding concerns.
    let mut whole: Vec<(Option<String>, Diagnostic)> = Vec::new();
    for schema in resolver.relations() {
        for finding in check_relation_keys(schema) {
            let origin = finding_origin(&finding);
            whole.push((origin, diag::render_finding(&finding, &meta)));
        }
    }
    for rule in &source.rules {
        for finding in check_rule(rule, &resolver) {
            let origin = finding_origin(&finding);
            whole.push((origin, diag::render_finding(&finding, &meta)));
        }
    }
    for error in infer_source(&mut source, &resolver) {
        whole.push((None, diag::render_type_error(&error)));
    }
    for function in &source.functions {
        for finding in check_function(function, &resolver) {
            let origin = finding_origin(&finding);
            whole.push((origin, diag::render_finding(&finding, &meta)));
        }
    }
    for (origin, d) in whole {
        let target = origin.and_then(|name| {
            prepared
                .iter_mut()
                .find(|pf| name.starts_with(&format!("{}.", pf.qualifier)))
        });
        match target {
            Some(pf) => pf.diags.push(d),
            None => entry_diags.push(d),
        }
    }

    let mut reports = vec![FileReport {
        label: entry_label.to_string(),
        diagnostics: entry_diags,
    }];
    reports.extend(prepared.into_iter().map(|pf| FileReport {
        label: pf.label,
        diagnostics: pf.diags,
    }));

    PackageLowered {
        source,
        resolver,
        meta,
        reports,
    }
}

/// A finding's own qualified/bare name, when it has one — used only to route
/// the rendered diagnostic to the file that declared it (see the module
/// doc). `None` for findings with no nameable target (callers fall back to
/// the entry file's bucket).
fn finding_origin(finding: &Finding) -> Option<String> {
    match finding {
        Finding::NonCanonicalKey { relation, .. }
        | Finding::AbsenceWithoutWitness { relation, .. }
        | Finding::UnknownRelation { relation, .. }
        | Finding::OrdinaryFnOnDerivedRel { relation, .. } => Some(relation.to_string()),
        Finding::ImpureRule { rule }
        | Finding::NondeterministicRule { rule }
        | Finding::DivergentRule { rule }
        | Finding::UnboundHeadKey { rule, .. }
        | Finding::MaskRefNotEdgeBound { rule, .. } => Some(rule.to_string()),
        Finding::UndeclaredFnEffect { function, .. } | Finding::TotalFnFallible { function } => {
            Some(function.to_string())
        }
    }
}

/// Every top-level decl name `file` declares under its own bare spelling
/// (before any submodule-qualification rename) — what a sibling module must
/// not also claim.
fn decl_names(file: &File) -> Vec<String> {
    decl_names_with_spans(file)
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

fn decl_names_with_spans(file: &File) -> Vec<(String, Span)> {
    file.decls
        .iter()
        .filter_map(|d| match d {
            Decl::Fn(f) => Some((f.name.text.clone(), f.span)),
            Decl::Entity(e) => Some((e.name.text.clone(), e.span)),
            Decl::Enum(e) => Some((e.name.text.clone(), e.span)),
            Decl::Type(t) => Some((t.name.text.clone(), t.span)),
            Decl::Record(r) => Some((r.name.text.clone(), r.span)),
            Decl::Rel(r) => Some((r.name.text.clone(), r.span)),
            // Protocols keep their bare name in v0 multi-file packages
            // (qualifying `proto_name` would need to also re-derive its
            // request/outcome relation names consistently — deferred; no
            // spec fixture needs a protocol inside a math-shaped package
            // submodule yet).
            _ => None,
        })
        .collect()
}

/// Clone `file`, renaming every top-level decl this module can qualify (see
/// [`decl_names_with_spans`]) from its bare spelling to `<prefix>.<name>` —
/// a single [`brix_ast::ast::Ident`] whose text contains a literal `.`,
/// which `QualIdent::from` (not `::simple`) splits into real segments at
/// every registration site pass 1/2 use. Nothing else in the file changes:
/// call sites stay bare or dotted exactly as written, and resolve through
/// [`super::resolve::ProgramResolver::resolve_path`] / the auto-registered
/// import alias, unchanged.
fn qualify_file(file: &File, prefix: &str) -> File {
    let mut qualified = file.clone();
    for d in &mut qualified.decls {
        match d {
            Decl::Fn(f) => f.name.text = format!("{prefix}.{}", f.name.text),
            Decl::Entity(e) => e.name.text = format!("{prefix}.{}", e.name.text),
            Decl::Enum(e) => e.name.text = format!("{prefix}.{}", e.name.text),
            Decl::Type(t) => t.name.text = format!("{prefix}.{}", t.name.text),
            Decl::Record(r) => r.name.text = format!("{prefix}.{}", r.name.text),
            Decl::Rel(r) => r.name.text = format!("{prefix}.{}", r.name.text),
            _ => {}
        }
    }
    qualified
}
