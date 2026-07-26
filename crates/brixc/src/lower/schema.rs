//! Pass 1 — schema build (design §"Pass 1 — schema build").
//!
//! Walks `File.uses` + `File.decls` and populates the [`ProgramResolver`]'s
//! decl-namespace tables. Never looks inside rule bodies (`derive`/
//! `constraint`/`query` bodies, fn bodies) — that is pass 2's job
//! ([`crate::lower::decl`]).

use std::collections::BTreeSet;

use brix_ast::ast::{self, Decl, RelKind, RelMod, TypeKind};
use brix_diag::Diagnostic;
use brix_ir::effects::{Effect, EffectRow};
use brix_ir::frontend::{FnSignature, RelationSchema, SchemaResolver};
use brix_ir::ident::{Ident as IrIdent, QualIdent};
use brix_ir::traits::{AssocBinding, ImplDef, ImplHead, TraitDef};
use brix_ir::types::Ty;

use super::diag;
use super::resolve::{FnInfo, LowerMeta, ProgramResolver, RuntimeRelationKind, UnitClass};
use super::stdlib;
use super::tymap::{lower_type, TyPos};

pub fn build(
    file: &ast::File,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> ProgramResolver {
    build_onto(file, stdlib::stdlib_resolver().clone(), meta, diags)
}

/// Run pass 1 over `file`, registering its decls into an **already-seeded**
/// `resolver` rather than a fresh prelude. This is the seam `lower_graph`
/// (issue #42) uses to fold a package's decls on top of a resolver that
/// already carries the prelude plus every dependency package's qualified
/// exports.
pub fn build_onto(
    file: &ast::File,
    mut resolver: ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> ProgramResolver {
    resolver = process_uses(file, resolver, diags);
    resolver = register_names(file, resolver);
    resolver = register_aliases(file, resolver, meta, diags);
    resolver = register_units(file, resolver);
    resolver = build_schemas(file, resolver, meta, diags);
    check_impl_conformance(file, &resolver, diags);
    check_impl_orphan(file, &resolver, diags);
    check_scenario_writes(file, &resolver, diags);
    recompute_derived(file, resolver, meta)
}

/// Impl-conformance (issue #111, Part V §3: "plain associated types — an impl
/// provides exactly one type per associated-type name"): each `impl` must bind
/// exactly the associated types its trait declares — none missing, none extra.
/// Runs after [`build_schemas`] so every trait (local, or folded from a
/// dependency in `lower_graph`) is in the resolver's trait env, independent of
/// declaration order. A trait the resolver does not know is skipped (nothing to
/// check against); the impl still registered for coherence.
fn check_impl_conformance(
    file: &ast::File,
    resolver: &ProgramResolver,
    diags: &mut Vec<Diagnostic>,
) {
    let env = resolver.trait_env();
    for d in &file.decls {
        let Decl::Impl(im) = d else { continue };
        let trait_name = im.trait_name.text.as_str();
        // Match on the *resolved* trait name, so a `use`-imported trait finds
        // the dependency's `TraitDef` rather than missing it (issue #111).
        let trait_qual = resolve_trait_name(&im.trait_name, resolver);
        let Some(tr) = env.traits().iter().find(|t| t.name == trait_qual) else {
            continue;
        };
        let declared: BTreeSet<&str> = tr.assoc_types.iter().map(|a| a.as_str()).collect();
        let bound: BTreeSet<&str> = im
            .assoc_bindings
            .iter()
            .map(|b| b.name.text.as_str())
            .collect();
        for missing in declared.difference(&bound) {
            diags.push(diag::error(
                diag::IMPL_CONFORMANCE,
                im.span,
                format!("impl of trait `{trait_name}` is missing associated type `{missing}`"),
            ));
        }
        for extra in bound.difference(&declared) {
            diags.push(diag::error(
                diag::IMPL_CONFORMANCE,
                im.span,
                format!(
                    "impl of trait `{trait_name}` binds associated type `{extra}`, \
                     which the trait does not declare"
                ),
            ));
        }
    }
}

/// The `pub derive` orphan gate (issue #154, errata 0003 ruling): a downstream
/// `impl Trait for Head` may extend a head owned by a **dependency** only if
/// that dependency exported the head `pub derive`. A bare `pub`/`pub read`
/// relation is re-exported for *reference* but sealed against extension — the
/// ruling makes `derive` the one capability that is coherence-affecting and
/// must be granted explicitly, never implied by a bare `pub`.
///
/// The rule mirrors trait coherence (§28.3): the impl is allowed when the trait
/// is local, or the head is local, or the foreign head is `pub derive`. Only an
/// `impl ForeignTrait for ForeignHead` where the head is not `pub derive` is the
/// sealed-extension error (`BRX-LOW-0019`).
///
/// Runs after [`build_schemas`] (imports + schemas registered) with the
/// graph-folded resolver, so [`ProgramResolver::export_cap`] answers `Some` iff
/// the head resolves to a foreign public dependency export — a local or
/// package-private head returns `None` and never trips the gate. When a package
/// is lowered standalone (no dependencies) there are no foreign caps, so this is
/// inert; every cross-package impl is therefore checked exactly once, in the
/// lowering of whichever package declares it.
fn check_impl_orphan(file: &ast::File, resolver: &ProgramResolver, diags: &mut Vec<Diagnostic>) {
    let local_traits: BTreeSet<&str> = file
        .decls
        .iter()
        .filter_map(|d| match d {
            Decl::Trait(t) => Some(t.name.text.as_str()),
            _ => None,
        })
        .collect();
    for d in &file.decls {
        let Decl::Impl(im) = d else { continue };
        // The head must be a named type to have a coherence head at all; row/
        // compound targets already error in `build_schemas` (UNSUPPORTED_V0).
        let TypeKind::Named { path, .. } = &im.target.kind else {
            continue;
        };
        let head_qual = resolver.resolve_path(path);
        // `Some` iff the head is a foreign public export; local and
        // package-private heads are `None` and out of scope for this gate.
        let Some(cap) = resolver.export_cap(&head_qual) else {
            continue;
        };
        if cap == ast::RelVis::Derive {
            continue; // owner opted the head into downstream extension
        }
        if local_traits.contains(im.trait_name.text.as_str()) {
            continue; // local trait: allowed under the orphan rule regardless
        }
        let head = path
            .segments
            .last()
            .map(|s| s.text.as_str())
            .unwrap_or_default();
        diags.push(diag::error(
            diag::ORPHAN_SEALED,
            im.span,
            format!(
                "impl of `{}` for `{head}` extends a head owned by another package \
                 that did not export it `pub derive`; a downstream package may only \
                 extend a foreign head marked `pub derive`",
                im.trait_name.text
            ),
        ));
    }
}

/// The `pub write` gate (issue #154, errata 0003 ruling): a `scenario`
/// transaction that directly *asserts into* a relation owned by a **dependency**
/// (`assert`/`set`/`ensure`) requires that relation to be exported `pub write`.
/// `write` = "assertable" — distinct from the `derive` capability, which covers
/// a downstream *rule* extending the relation (`check_impl_orphan` /
/// `lower_head`).
///
/// This is deliberately a **static name-resolution** check, not execution
/// lowering: `Decl::Scenario` is a v0 defer-line skip (its tx-bodies are never
/// lowered to runtime IR), but the *write surface* is fully present in the
/// parsed AST, so the visibility gate needs only the resolver's `export_caps`
/// and import map — the same inputs `check_impl_orphan` uses. Local write
/// targets are absent from `export_caps` and never gated, so a package writing
/// into its own relations (the common case, incl. the flagship) is unaffected.
fn check_scenario_writes(
    file: &ast::File,
    resolver: &ProgramResolver,
    diags: &mut Vec<Diagnostic>,
) {
    for d in &file.decls {
        let Decl::Scenario(s) = d else { continue };
        // Only the *executable* tx-blocks assert; `seed`/`bind`/`assert`
        // clauses observe, they do not write.
        let blocks = s
            .setup
            .iter()
            .chain(s.steps.iter().map(|st| &st.body))
            .chain(s.ats.iter().map(|at| &at.body));
        for block in blocks {
            for stmt in &block.stmts {
                let tx = match stmt {
                    ast::TxStmt::Let { value, .. } => value,
                    ast::TxStmt::Expr(e) => e,
                    ast::TxStmt::Error(..) => continue,
                };
                check_write_target(tx, resolver, diags);
            }
        }
    }
}

/// Gate one transaction expression's write target against `pub write`. Path-form
/// writes (`set`/`assert R(..)`) resolve through the import map; type-form writes
/// (`ensure`/`fresh`/`assert S {..}`) resolve their bare type name the same way a
/// single-segment path would. `retract`/`supersede` carry their target inside an
/// expression rather than a head path, so they are out of scope for the static
/// gate (revisit if a foreign-relation retract surface is needed).
fn check_write_target(tx: &ast::TxExpr, resolver: &ProgramResolver, diags: &mut Vec<Diagnostic>) {
    let (target, span, name) = match tx {
        ast::TxExpr::Set { path, span, .. } | ast::TxExpr::AssertTuple { path, span, .. } => {
            let name = path
                .segments
                .last()
                .map(|s| s.text.clone())
                .unwrap_or_default();
            (resolver.resolve_path(path), *span, name)
        }
        ast::TxExpr::Ensure { ty, span, .. }
        | ast::TxExpr::Fresh { ty, span, .. }
        | ast::TxExpr::AssertStruct { ty, span, .. } => {
            let target = resolver
                .imported_target(&ty.text)
                .cloned()
                .unwrap_or_else(|| QualIdent::simple(ty.text.clone()));
            (target, *span, ty.text.clone())
        }
        ast::TxExpr::Retract { .. } | ast::TxExpr::Supersede { .. } => return,
    };
    // `Some` iff the target is a foreign public export; local targets are `None`.
    let Some(cap) = resolver.export_cap(&target) else {
        return;
    };
    if cap != ast::RelVis::Write {
        diags.push(diag::error(
            diag::SEALED_WRITE_TARGET,
            span,
            format!(
                "scenario asserts into `{name}`, a relation owned by another package \
                 that did not export it `pub write`; a downstream package may only \
                 assert into a foreign relation marked `pub write`"
            ),
        ));
    }
}

/// The bare names this file declares itself (entity/rel/enum/fn/type/record),
/// independent of the resolver's state — used by [`process_uses`] to catch a
/// `use` item that shadows a root-local declaration of the same name (issue
/// #42 Slice 2's "duplicate export" case). Computed straight off the AST so
/// the check does not depend on decl-registration order within pass 1.
fn local_decl_names(file: &ast::File) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for d in &file.decls {
        let name = match d {
            Decl::Entity(e) => Some(&e.name),
            Decl::Rel(r) => Some(&r.name),
            Decl::Enum(e) => Some(&e.name),
            Decl::Fn(f) => Some(&f.name),
            Decl::Type(t) => Some(&t.name),
            Decl::Record(r) => Some(&r.name),
            _ => None,
        };
        if let Some(name) = name {
            names.insert(name.text.clone());
        }
    }
    names
}

/// Pass 1's `use`-item walk (design §"Pass 1"). Populates the import/prefix
/// maps and, per issue #42 Slice 2, catches the two ways a bare imported
/// name stops being safe to resolve silently: (a) **ambiguous** — two `use`
/// items import the same bare name to different qualified targets, and (b)
/// **duplicate export** — an imported bare name collides with a root-local
/// declaration of the same name in this file. Both emit
/// [`diag::AMBIGUOUS_IMPORT`] at the offending `use` item's span; lowering
/// continues (error severity blocks a *clean* lower via `Lowered::has_errors`,
/// same as every other `BRX-LOW-*` error).
fn process_uses(
    file: &ast::File,
    mut resolver: ProgramResolver,
    diags: &mut Vec<Diagnostic>,
) -> ProgramResolver {
    let locals = local_decl_names(file);
    for u in &file.uses {
        let base: Vec<IrIdent> = u
            .path
            .segments
            .iter()
            .map(|s| IrIdent::new(s.text.clone()))
            .collect();
        if u.items.is_empty() {
            // `use brix.sim` (no `.{...}`): the last segment becomes a
            // prefix alias for the whole qualified path.
            if let Some(alias) = u.path.segments.last() {
                resolver = resolver.with_prefix(alias.text.clone(), QualIdent::from_segments(base));
            }
        } else {
            for item in &u.items {
                let mut segs = base.clone();
                segs.push(IrIdent::new(item.text.clone()));
                let target = QualIdent::from_segments(segs);
                let previous = resolver.imported_target(&item.text).cloned();
                resolver = resolver.with_import(item.text.clone(), target.clone());

                if resolver.is_ambiguous_import(&item.text) {
                    let mut candidates: Vec<String> = Vec::new();
                    if let Some(prev) = &previous {
                        candidates.push(prev.to_string());
                    }
                    candidates.push(target.to_string());
                    candidates.sort();
                    candidates.dedup();
                    diags.push(diag::error(
                        diag::AMBIGUOUS_IMPORT,
                        item.span,
                        format!(
                            "ambiguous import `{}`: could resolve to {}",
                            item.text,
                            candidates.join(" or ")
                        ),
                    ));
                }

                if locals.contains(&item.text) {
                    diags.push(diag::error(
                        diag::AMBIGUOUS_IMPORT,
                        item.span,
                        format!(
                            "import `{}` collides with a local declaration of the same name in this file",
                            item.text
                        ),
                    ));
                }

                if let Some(dep_name) = resolver.is_private_symbol(&target).map(|s| s.to_string()) {
                    diags.push(diag::error(
                        diag::PRIVATE_IMPORT,
                        item.span,
                        format!(
                            "cannot import package-private declaration `{}` from dependency `{dep_name}`",
                            item.text
                        ),
                    ));
                }
            }
        }
    }
    resolver
}

/// Pre-register entity/enum *names* (before any field/role type is
/// lowered) so forward references within the same file resolve (v0: single
/// file, so this is the only ordering hazard tymap needs guarding against).
fn register_names(file: &ast::File, mut resolver: ProgramResolver) -> ProgramResolver {
    for d in &file.decls {
        match d {
            Decl::Entity(e) => {
                resolver = resolver.with_entity(QualIdent::simple(e.name.text.clone()));
            }
            Decl::Enum(e) => {
                let variants: Vec<IrIdent> = e
                    .variants
                    .iter()
                    .map(|v| IrIdent::new(v.name.text.clone()))
                    .collect();
                resolver = resolver.with_enum(QualIdent::simple(e.name.text.clone()), variants);
            }
            _ => {}
        }
    }
    resolver
}

fn register_aliases(
    file: &ast::File,
    mut resolver: ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> ProgramResolver {
    for d in &file.decls {
        match d {
            Decl::Type(t) => {
                if is_self_referential(&t.value, &t.name.text) {
                    diags.push(diag::error(
                        diag::ALIAS_CYCLE,
                        t.span,
                        format!("type alias `{}` refers to itself", t.name.text),
                    ));
                    resolver = resolver.with_alias(
                        QualIdent::simple(t.name.text.clone()),
                        Ty::Var(meta.fresh_tyvar()),
                    );
                } else {
                    let ty = lower_type(&t.value, TyPos::Role, &resolver, meta, diags);
                    resolver = resolver.with_alias(QualIdent::simple(t.name.text.clone()), ty);
                }
                meta.set_decl_span(IrIdent::new(t.name.text.clone()), t.span);
            }
            Decl::Record(r) => {
                // A `record` behaves like a named alias for its row type
                // (v0: no distinct nominal-record `Ty`, and none of the
                // spec corpus's lowered decls construct one by name).
                let fields = r
                    .fields
                    .iter()
                    .map(|f| brix_ir::types::RowField {
                        name: IrIdent::new(f.name.text.clone()),
                        ty: lower_type(&f.ty, TyPos::Role, &resolver, meta, diags),
                    })
                    .collect();
                let row = brix_ir::types::Row::closed(fields);
                resolver =
                    resolver.with_alias(QualIdent::simple(r.name.text.clone()), Ty::record(row));
                meta.set_decl_span(IrIdent::new(r.name.text.clone()), r.span);
            }
            _ => {}
        }
    }
    resolver
}

fn is_self_referential(ty: &ast::Type, name: &str) -> bool {
    matches!(
        &ty.kind,
        TypeKind::Named { path, args }
            if args.is_empty() && path.segments.len() == 1 && path.segments[0].text == name
    )
}

fn register_units(file: &ast::File, mut resolver: ProgramResolver) -> ProgramResolver {
    for d in &file.decls {
        if let Decl::Unit(u) = d {
            let class = match u.measure.text.as_str() {
                "Duration" => UnitClass::Duration,
                "Money" => UnitClass::Money(IrIdent::new(u.name.text.clone())),
                _ => UnitClass::Quantity(IrIdent::new(u.measure.text.clone())),
            };
            let scale = match &*u.value.kind {
                ast::ExprKind::Int(n) => *n as i64,
                _ => 1,
            };
            resolver = resolver.with_unit_scaled(u.name.text.clone(), class, scale);
        }
    }
    resolver
}

fn build_schemas(
    file: &ast::File,
    mut resolver: ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> ProgramResolver {
    for d in &file.decls {
        match d {
            Decl::Entity(e) => {
                let qi = QualIdent::simple(e.name.text.clone());
                let mut roles = Vec::new();
                let mut key = Vec::new();
                for f in &e.fields {
                    let ty = lower_type(&f.ty, TyPos::Role, &resolver, meta, diags);
                    let ident = IrIdent::new(f.name.text.clone());
                    meta.set_role_span(qi.clone(), ident.clone(), f.span);
                    if f.is_key {
                        key.push(ident.clone());
                    }
                    roles.push((ident, ty));
                }
                meta.set_relation_span(qi.clone(), e.span);
                meta.set_decl_span(IrIdent::new(e.name.text.clone()), e.span);
                resolver = resolver.with_relation(RelationSchema {
                    name: qi,
                    roles,
                    key,
                    model_closed: true,
                    derived: false,
                });
            }
            Decl::Enum(e) => {
                meta.set_decl_span(IrIdent::new(e.name.text.clone()), e.span);
            }
            Decl::Rel(r) => {
                let qi = QualIdent::simple(r.name.text.clone());
                let mut roles = Vec::new();
                let mut key: Vec<IrIdent> = Vec::new();
                for f in &r.roles {
                    let ty = lower_type(&f.ty, TyPos::Role, &resolver, meta, diags);
                    let ident = IrIdent::new(f.name.text.clone());
                    meta.set_role_span(qi.clone(), ident.clone(), f.span);
                    if f.is_key && !key.contains(&ident) {
                        key.push(ident.clone());
                    }
                    roles.push((ident, ty));
                }
                for m in &r.mods {
                    if let RelMod::Key(idents) = m {
                        for id in idents {
                            let ii = IrIdent::new(id.text.clone());
                            if !key.contains(&ii) {
                                key.push(ii);
                            }
                        }
                    }
                }
                let model_closed = !matches!(r.kind, RelKind::Open);
                meta.set_relation_span(qi.clone(), r.span);
                meta.set_decl_span(IrIdent::new(r.name.text.clone()), r.span);
                let runtime_kind = match r.kind {
                    RelKind::Ground | RelKind::Open => RuntimeRelationKind::Ground,
                    RelKind::State => RuntimeRelationKind::State,
                    RelKind::Event => RuntimeRelationKind::Event,
                };
                resolver = resolver.with_relation_kind(qi.clone(), runtime_kind);
                resolver = resolver.with_relation(RelationSchema {
                    name: qi,
                    roles,
                    key,
                    model_closed,
                    derived: false,
                });
            }
            Decl::Protocol(p) => {
                let proto_name = IrIdent::new(p.name.text.clone());
                let req_qi =
                    QualIdent::from_segments([proto_name.clone(), IrIdent::new("request")]);
                let mut req_roles = Vec::new();
                for f in &p.request.roles {
                    let ty = lower_type(&f.ty, TyPos::Role, &resolver, meta, diags);
                    let ident = IrIdent::new(f.name.text.clone());
                    meta.set_role_span(req_qi.clone(), ident.clone(), f.span);
                    req_roles.push((ident, ty));
                }
                let req_key: Vec<IrIdent> = p
                    .request
                    .key
                    .iter()
                    .map(|k| IrIdent::new(k.text.clone()))
                    .collect();
                meta.set_relation_span(req_qi.clone(), p.request.span);
                resolver = resolver.with_relation(RelationSchema {
                    name: req_qi.clone(),
                    roles: req_roles.clone(),
                    key: req_key.clone(),
                    model_closed: true,
                    derived: false,
                });

                for o in &p.outcomes {
                    let out_qi = QualIdent::from_segments([
                        proto_name.clone(),
                        IrIdent::new(o.name.text.clone()),
                    ]);
                    let mut roles: Vec<(IrIdent, Ty)> = req_key
                        .iter()
                        .filter_map(|k| req_roles.iter().find(|(n, _)| n == k).cloned())
                        .collect();
                    for f in &o.roles {
                        let ty = lower_type(&f.ty, TyPos::Role, &resolver, meta, diags);
                        let ident = IrIdent::new(f.name.text.clone());
                        meta.set_role_span(out_qi.clone(), ident.clone(), f.span);
                        roles.push((ident, ty));
                    }
                    meta.set_relation_span(out_qi.clone(), o.span);
                    resolver = resolver.with_relation(RelationSchema {
                        name: out_qi,
                        roles,
                        key: req_key.clone(),
                        model_closed: false,
                        derived: false,
                    });
                }
                // protocol `policy`/`methods` (Part 27.8 estimator shape):
                // deferred wholesale, no diagnostic (design: "ignored").
                meta.set_decl_span(proto_name, p.span);
            }
            Decl::Fn(f) => {
                let qi = QualIdent::simple(f.name.text.clone());
                let params: Vec<Ty> = f
                    .params
                    .iter()
                    .map(|p| lower_type(&p.ty, TyPos::FnSig, &resolver, meta, diags))
                    .collect();
                let ret = lower_type(&f.ret, TyPos::FnSig, &resolver, meta, diags);
                let effects = build_effect_row(&f.effects, diags);
                let param_names: Vec<IrIdent> = f
                    .params
                    .iter()
                    .map(|p| IrIdent::new(p.name.text.clone()))
                    .collect();
                meta.set_fn_info(
                    qi.clone(),
                    FnInfo {
                        param_names,
                        is_partial: f.partial,
                        is_aggregate: f.aggregate,
                        body: f.body.clone(),
                    },
                );
                meta.set_decl_span(IrIdent::new(f.name.text.clone()), f.span);
                resolver = resolver.with_function(FnSignature {
                    name: qi,
                    params,
                    ret,
                    may_diverge: effects.may_diverge(),
                    effects,
                    is_aggregate: f.aggregate,
                });
            }
            // Trait/impl registration (issue #111). Traits go into Γ; impls are
            // keyed by (trait, head) and `add_impl` enforces the §28.3 orphan
            // rule right here as they register (BRX-LOW-0017 on overlap). Method
            // bodies are not lowered yet (a follow-on slice).
            //
            // Both halves of the key are *resolved* names, not surface text: a
            // bare name that a `use` imported resolves to the owning package's
            // qualified name, and anything else stays package-local. Keying on
            // surface text made two packages that each declare their own
            // same-named trait and head collide on `(Canonical, Order)` even
            // though neither can see the other (issue #111 follow-on).
            Decl::Trait(t) => {
                let name = QualIdent::simple(t.name.text.clone());
                let span_key = IrIdent::new(t.name.text.clone());
                let assoc_types = t
                    .assoc_types
                    .iter()
                    .map(|a| IrIdent::new(a.name.text.clone()))
                    .collect();
                meta.set_decl_span(span_key, t.span);
                resolver = resolver.with_trait(TraitDef { name, assoc_types });
            }
            Decl::Impl(im) => {
                let assoc = im
                    .assoc_bindings
                    .iter()
                    .map(|b| AssocBinding {
                        name: IrIdent::new(b.name.text.clone()),
                        ty: lower_type(&b.value, TyPos::FnSig, &resolver, meta, diags),
                    })
                    .collect();
                match impl_head(&im.target, &resolver) {
                    Some(head) => {
                        let def = ImplDef {
                            trait_name: resolve_trait_name(&im.trait_name, &resolver),
                            head: ImplHead(head),
                            assoc,
                        };
                        if let Err(e) = resolver.add_impl(def) {
                            diags.push(diag::error(diag::IMPL_COHERENCE, im.span, e.to_string()));
                        }
                    }
                    None => diags.push(diag::error(
                        diag::UNSUPPORTED_V0,
                        im.target.span,
                        "impl target must be a named type \
                         (row/compound targets are not supported in v0)",
                    )),
                }
            }
            _ => {}
        }
    }
    resolver
}

/// The head-constructor name of an `impl ... for Target` — the last path
/// segment of a Named type (`Money<EUR>` -> `Money`). Per the trait design,
/// "generic-parameterized heads reduce to the head constructor name for the
/// coherence key" (`brix_ir::traits`). Row/compound targets have no single head
/// and return `None` (issue #111).
/// The resolved head of an `impl` target. Resolution goes through the import
/// map (the same `resolve_path` `check_impl_orphan` uses), so a `use`-imported
/// head keys on the *owning* package's qualified name rather than the bare
/// surface text it was written with.
fn impl_head(target: &ast::Type, resolver: &ProgramResolver) -> Option<QualIdent> {
    match &target.kind {
        TypeKind::Named { path, .. } => Some(resolver.resolve_path(path)),
        _ => None,
    }
}

/// Resolve a bare trait name the same way. `Path`-shaped references go through
/// `resolve_path`; a trait name is a plain `Ident` in the grammar, so this is
/// the single-segment case spelled out.
fn resolve_trait_name(name: &ast::Ident, resolver: &ProgramResolver) -> QualIdent {
    resolver
        .imported_target(&name.text)
        .cloned()
        .unwrap_or_else(|| QualIdent::simple(name.text.clone()))
}

fn build_effect_row(effects: &Option<Vec<ast::Ident>>, diags: &mut Vec<Diagnostic>) -> EffectRow {
    let mut atoms = Vec::new();
    if let Some(list) = effects {
        for e in list {
            match e.text.as_str() {
                "clock" => atoms.push(Effect::Clock),
                "random" => atoms.push(Effect::Random),
                "console" => atoms.push(Effect::Console),
                "panic" => atoms.push(Effect::Panic),
                "diverge" => atoms.push(Effect::Diverge),
                other => diags.push(diag::error(
                    diag::UNKNOWN_EFFECT,
                    e.span,
                    format!("unknown or scoped effect `{other}` (v0 supports clock/random/console/panic/diverge)"),
                )),
            }
        }
    }
    EffectRow::from_atoms(atoms)
}

/// Sub-pass 1b (design: "scan all derive heads to set `derived` flags —
/// can't know until all heads seen"). A relation is `derived` iff some
/// `derive` in the file targets it as a head; protocol outcome relations
/// are never derive targets by construction, so they correctly stay
/// `false`.
fn recompute_derived(
    file: &ast::File,
    mut resolver: ProgramResolver,
    meta: &mut LowerMeta,
) -> ProgramResolver {
    let _ = meta; // reserved: derive-head resolution needs no meta today.
    let mut targets: BTreeSet<QualIdent> = BTreeSet::new();
    for d in &file.decls {
        if let Decl::Derive(dd) = d {
            match &dd.head {
                ast::Head::Tuple { path, .. } => {
                    targets.insert(resolver.resolve_path(path));
                }
                ast::Head::Node { ty, .. } => {
                    targets.insert(QualIdent::simple(ty.text.clone()));
                }
                ast::Head::Mask { .. } => {}
            }
        }
    }
    let mut updates = Vec::new();
    for qi in &targets {
        if let Some(schema) = resolver.relation(qi) {
            if !schema.derived {
                let mut s = schema.clone();
                s.derived = true;
                updates.push(s);
            }
        }
    }
    for s in updates {
        resolver = resolver.with_relation(s);
    }
    resolver
}
