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
use brix_ir::types::Ty;

use super::diag;
use super::resolve::{
    seed_prelude, FnInfo, LowerMeta, ProgramResolver, RuntimeRelationKind, UnitClass,
};
use super::tymap::{lower_type, TyPos};

pub fn build(
    file: &ast::File,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> ProgramResolver {
    build_onto(file, seed_prelude(ProgramResolver::new()), meta, diags)
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
    recompute_derived(file, resolver, meta)
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
            resolver = resolver.with_unit(
                u.name.text.clone(),
                UnitClass::Quantity(IrIdent::new(u.measure.text.clone())),
            );
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
            _ => {}
        }
    }
    resolver
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
