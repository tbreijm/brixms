//! Pass 2 — decl lowering (design §"Pass 2 — decl lowering") and the v0
//! defer line (design §"Defer line (v0)").
//!
//! [`lower_decls`] is the single dispatch over `File.decls`: `derive`/
//! `constraint`/`query` get real lowering; the schema-producing decls
//! (`entity`/`rel`/`protocol`/`fn`/`enum`/`type`/`measure`/`unit`/`record`)
//! were already fully consumed by pass 1 ([`crate::lower::schema`]) and are
//! silently skipped here; everything else on the defer list is a
//! skip-with-warning; `Decl::Error` is a silent skip (the parser already
//! reported it).

use brix_ast::ast::{self, Decl};
use brix_diag::{Diagnostic, Span};
use brix_ir::core::{self, Constraint, Query, Rule};
use brix_ir::frontend::{FrontendSource, RelationSchema, SchemaResolver};
use brix_ir::ident::{Ident as IrIdent, QualIdent};
use brix_ir::pattern::{Clause, Pattern};
use brix_ir::types::Ty;

use super::diag;
use super::expr::{self, BodyCtx};
use super::resolve::{LowerMeta, ProgramResolver};
use super::tymap::{self, TyPos};

pub fn lower_decls(
    file: &ast::File,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> FrontendSource {
    let mut source = FrontendSource::new();
    for d in &file.decls {
        match d {
            Decl::Derive(dd) => source.rules.push(lower_derive(dd, resolver, meta, diags)),
            Decl::Constraint(cd) => source
                .constraints
                .push(lower_constraint(cd, resolver, meta, diags)),
            Decl::Query(qd) => source.queries.push(lower_query(qd, resolver, meta, diags)),

            // A user `fn` body: lowered to a checked Core IR `FnDef` so it can
            // execute from source (issue #47). Only total, expression-bodied
            // functions are lowered in Slice 1; block-bodied / partial fns
            // return `None` and stay on the hand-registered path (deferred to
            // Slice 2), exactly as before.
            Decl::Fn(f) => {
                if let Some(def) = lower_fn(f, resolver, meta, diags) {
                    source.functions.push(def);
                }
            }

            // Schema-producing decls: fully handled by pass 1 already.
            Decl::Entity(_)
            | Decl::Rel(_)
            | Decl::Protocol(_)
            | Decl::Enum(_)
            | Decl::Type(_)
            | Decl::Measure(_)
            | Decl::Unit(_)
            | Decl::Record(_) => {}

            // v0 defer line: skip-with-warning (BRX-LOW-0002).
            Decl::Driver(x) => skip(diags, x.span, "driver"),
            Decl::Scenario(x) => skip(diags, x.span, "scenario"),
            Decl::DataRecipe(x) => skip(diags, x.span, "dataRecipe"),
            Decl::Feature(x) => skip(diags, x.span, "feature"),
            Decl::FeatureSet(x) => skip(diags, x.span, "featureSet"),
            Decl::Dataset(x) => skip(diags, x.span, "dataset"),
            Decl::StatModel(x) => skip(diags, x.span, "statModel"),
            Decl::MlWorkflow(x) => skip(diags, x.span, "mlWorkflow"),
            Decl::Experiment(x) => skip(diags, x.span, "experiment"),
            Decl::Visualization(x) => skip(diags, x.span, "visualization"),
            Decl::Let(x) => skip(diags, x.span, "let"),
            Decl::Extension(x) => skip(diags, x.span, "extension"),

            // Silent: the parser already reported this.
            Decl::Error(_, _) => {}
        }
    }
    source
}

fn skip(diags: &mut Vec<Diagnostic>, span: Span, what: &str) {
    diags.push(diag::warning(
        diag::DECL_SKIPPED,
        span,
        format!("`{what}` declarations are not lowered in v0 (skipped)"),
    ));
}

fn role_ty<'a>(schema: Option<&'a RelationSchema>, role: &str) -> Option<&'a Ty> {
    schema.and_then(|s| {
        s.roles
            .iter()
            .find(|(n, _)| n.as_str() == role)
            .map(|(_, t)| t)
    })
}

// ---------------------------------------------------------------------
// Block / clause lowering (shared by rule/constraint/query bodies and
// `from { ... }` comprehension expressions — see `expr::lower_comprehension`).
// ---------------------------------------------------------------------

pub fn lower_block(ctx: &mut BodyCtx, block: &ast::Block) -> Pattern {
    let mut clauses = Vec::new();
    for c in &block.clauses {
        if let Some(ir_c) = lower_clause(ctx, c) {
            clauses.push(ir_c);
        }
    }
    Pattern::new(clauses)
}

fn lower_clause(ctx: &mut BodyCtx, c: &ast::Clause) -> Option<Clause> {
    let ordinal = ctx.next_clause_ordinal();
    match c {
        ast::Clause::Edge(e) => Some(lower_edge(ctx, e, false)),
        ast::Clause::History(e) => Some(lower_edge(ctx, e, true)),
        ast::Clause::Entity(e) => Some(lower_entity_clause(ctx, e)),
        ast::Clause::Let(l) => Some(lower_let_clause(ctx, l, ordinal)),
        ast::Clause::When(e) => Some(lower_when_clause(ctx, e, ordinal)),
        ast::Clause::Any(blocks) => {
            let patterns = blocks.iter().map(|b| lower_block(ctx, b)).collect();
            Some(Clause::Any(patterns))
        }
        ast::Clause::Exists(b) => Some(Clause::Exists(lower_block(ctx, b))),
        ast::Clause::Without(b) => Some(Clause::Without(lower_block(ctx, b))),
        ast::Clause::Optional(b) => Some(Clause::Optional(lower_block(ctx, b))),
        ast::Clause::Cross(b) => Some(Clause::Cross(lower_block(ctx, b))),
        ast::Clause::Path(p) => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                p.span,
                "path clauses are unsupported in v0",
            ));
            None
        }
        ast::Clause::Error(_, _) => None,
    }
}

fn lower_edge(ctx: &mut BodyCtx, e: &ast::EdgeClause, is_history: bool) -> Clause {
    let relation = ctx.resolver.resolve_path(&e.path);
    let schema = ctx.resolver.relation(&relation);
    let bind = e.alias.as_ref().map(|a| IrIdent::new(a.text.clone()));
    if let Some(b) = &bind {
        ctx.bound.insert(b.clone());
        ctx.edge_aliases.insert(b.clone());
    }
    let mut args = Vec::new();
    for a in &e.args {
        let hint = expr::arg_role(a).and_then(|r| role_ty(schema, r.as_str()));
        if let Some(ra) = expr::resolve_pattern_arg(ctx, a, hint) {
            args.push(ra);
        }
    }
    if is_history {
        Clause::History {
            bind,
            relation,
            args,
        }
    } else {
        Clause::Edge {
            bind,
            relation,
            args,
        }
    }
}

fn lower_entity_clause(ctx: &mut BodyCtx, e: &ast::EntityClause) -> Clause {
    let entity = IrIdent::new(e.ty.text.clone());
    let var = IrIdent::new(e.binder.text.clone());
    ctx.bound.insert(var.clone());
    let entity_qi = QualIdent::simple(e.ty.text.clone());
    let schema = ctx.resolver.relation(&entity_qi);
    let mut fields = Vec::new();
    for a in &e.fields {
        let hint = expr::arg_role(a).and_then(|r| role_ty(schema, r.as_str()));
        if let Some(ra) = expr::resolve_pattern_arg(ctx, a, hint) {
            fields.push(ra);
        }
    }
    Clause::Entity {
        var,
        entity,
        fields,
    }
}

fn lower_let_clause(ctx: &mut BodyCtx, l: &ast::LetClause, _ordinal: u32) -> Clause {
    let binds = match &*l.pattern.kind {
        ast::ExprKind::Ident(p) if p.segments.len() == 1 => {
            IrIdent::new(p.segments[0].text.clone())
        }
        _ => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                l.pattern.span,
                "destructuring `let` patterns are unsupported in v0",
            ));
            IrIdent::new("%error")
        }
    };
    // Value is lowered while `binds` is not yet in scope (it names the
    // *result* of this expression, not an input to it).
    let value = expr::lower_expr(ctx, &l.value);
    ctx.effects = ctx.effects.combine(&expr::effects_of(&value, ctx.resolver));
    if !ctx.bound.insert(binds.clone()) {
        ctx.diags.push(diag::error(
            diag::LET_REBINDS,
            l.span,
            format!("`let {binds}` rebinds an already-bound name (no shadowing)"),
        ));
    }
    Clause::Let { binds, expr: value }
}

fn lower_when_clause(ctx: &mut BodyCtx, e: &ast::Expr, _ordinal: u32) -> Clause {
    let value = expr::lower_expr(ctx, e);
    ctx.effects = ctx.effects.combine(&expr::effects_of(&value, ctx.resolver));
    Clause::When(value)
}

// ---------------------------------------------------------------------
// Head lowering.
// ---------------------------------------------------------------------

fn lower_head(ctx: &mut BodyCtx, h: &ast::Head) -> core::Head {
    match h {
        ast::Head::Tuple { path, args } => {
            let relation = ctx.resolver.resolve_path(path);
            let schema = ctx.resolver.relation(&relation);
            let mut ir_args = Vec::new();
            for a in args {
                let hint = expr::arg_role(a).and_then(|r| role_ty(schema, r.as_str()));
                if let Some(ra) = expr::resolve_pattern_arg(ctx, a, hint) {
                    ir_args.push(ra);
                }
            }
            core::Head::Tuple {
                relation,
                args: ir_args,
            }
        }
        ast::Head::Node {
            binder,
            ty,
            args,
            keyed_by,
        } => {
            let entity_qi = QualIdent::simple(ty.text.clone());
            let schema = ctx.resolver.relation(&entity_qi);
            let mut ir_args = Vec::new();
            for a in args {
                let hint = expr::arg_role(a).and_then(|r| role_ty(schema, r.as_str()));
                if let Some(ra) = expr::resolve_pattern_arg(ctx, a, hint) {
                    ir_args.push(ra);
                }
            }
            core::Head::Node {
                var: IrIdent::new(binder.text.clone()),
                entity: IrIdent::new(ty.text.clone()),
                args: ir_args,
                keyed_by: keyed_by
                    .iter()
                    .map(|k| IrIdent::new(k.text.clone()))
                    .collect(),
            }
        }
        ast::Head::Mask { target, by } => {
            let target_i = IrIdent::new(target.text.clone());
            let by_i = IrIdent::new(by.text.clone());
            if !ctx.edge_aliases.contains(&target_i) {
                ctx.diags.push(diag::error(
                    diag::MASK_NOT_EDGE_BOUND,
                    target.span,
                    format!("mask target `{target_i}` is not an edge-bound alias (`{target_i} @ R(...)`)"),
                ));
            }
            if !ctx.edge_aliases.contains(&by_i) {
                ctx.diags.push(diag::error(
                    diag::MASK_NOT_EDGE_BOUND,
                    by.span,
                    format!("mask reason `{by_i}` is not an edge-bound alias (`{by_i} @ R(...)`)"),
                ));
            }
            core::Head::Mask {
                target: target_i,
                reason: by_i,
            }
        }
    }
}

// ---------------------------------------------------------------------
// Top-level decl -> Core IR node.
// ---------------------------------------------------------------------

fn lower_derive(
    d: &ast::DeriveDecl,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Rule {
    let name = IrIdent::new(d.name.text.clone());
    meta.set_decl_span(name.clone(), d.span);
    let mut ctx = BodyCtx::new(name.clone(), resolver, meta, diags);
    let body = lower_block(&mut ctx, &d.body);
    let head = lower_head(&mut ctx, &d.head);
    let effects = ctx.effects.clone();
    Rule {
        name,
        head,
        body,
        effects,
    }
}

fn lower_constraint(
    d: &ast::ConstraintDecl,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Constraint {
    let name = IrIdent::new(d.name.text.clone());
    meta.set_decl_span(name.clone(), d.span);
    let mut ctx = BodyCtx::new(name.clone(), resolver, meta, diags);
    let body = lower_block(&mut ctx, &d.body);
    let severity = match d.kind {
        ast::ConstraintKind::Advisory => core::Severity::Advisory,
        ast::ConstraintKind::Strict => core::Severity::Strict,
        ast::ConstraintKind::Audit => core::Severity::Audit,
    };
    Constraint {
        name,
        severity,
        body,
    }
}

fn lower_query(
    d: &ast::QueryDecl,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Query {
    let name = IrIdent::new(d.name.text.clone());
    meta.set_decl_span(name.clone(), d.span);
    if let Some(order) = &d.order {
        diags.push(diag::error(
            diag::UNSUPPORTED_V0,
            order.span,
            "query `order`/`limit` is unsupported in v0",
        ));
    }
    // Mismatch (E): `core::Query` has no `params` field yet.
    let result = tymap::lower_type(&d.ret, TyPos::Role, resolver, meta, diags);
    let params: Vec<(IrIdent, Ty)> = d
        .params
        .iter()
        .map(|p| {
            let ty = tymap::lower_type(&p.ty, TyPos::Role, resolver, meta, diags);
            (IrIdent::new(p.name.text.clone()), ty)
        })
        .collect();
    let mut ctx = BodyCtx::new(name.clone(), resolver, meta, diags);
    // Query params are in scope for the body/yield (design: params live in
    // `LowerMeta` — mismatch (E) — but the *names* still participate in the
    // ordinary pattern-variable scope so `when risk > threshold` resolves).
    for (p, _) in &params {
        ctx.bound.insert(p.clone());
    }
    let body = lower_block(&mut ctx, &d.from);
    let yields = expr::lower_expr(&mut ctx, &d.yield_);
    drop(ctx);

    Query {
        name,
        params,
        body,
        yields,
        result,
    }
}

/// Lower a user `fn` body into a checked Core IR [`core::FnDef`] (issue #47).
///
/// Slice 1 handles only **total, expression-bodied** functions (`fn f(..) -> T
/// = <expr>`): these reuse the whole rule-body expression-lowering engine
/// ([`expr::lower_expr`]), with the parameters seeded into scope exactly as
/// [`lower_query`] seeds query params. Block-bodied (`{ .. }`) or `partial`
/// functions return `None` — they are *not* an error (that would break the
/// flagship's `riskModel`); they simply stay unlowered and hand-registered
/// until Slice 2 grows blocks / partial-result / runtime provenance.
fn lower_fn(
    f: &ast::FnDecl,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Option<core::FnDef> {
    let body_expr = match &f.body {
        Some(ast::FnBody::Expr(e)) if !f.partial => e,
        _ => return None,
    };
    let qi = QualIdent::simple(f.name.text.clone());
    // Pass 1 already built and recorded the signature (lowered param/ret types,
    // effect row); reuse it so the FnDef agrees with what the checker sees.
    let sig = resolver.function(&qi)?.clone();
    let params: Vec<(IrIdent, Ty)> = f
        .params
        .iter()
        .map(|p| IrIdent::new(p.name.text.clone()))
        .zip(sig.params.iter().cloned())
        .collect();
    let name_ir = IrIdent::new(f.name.text.clone());
    meta.set_decl_span(name_ir.clone(), f.span);
    let mut ctx = BodyCtx::new(name_ir, resolver, meta, diags);
    for (p, _) in &params {
        ctx.bound.insert(p.clone());
    }
    let body = expr::lower_expr(&mut ctx, body_expr);
    drop(ctx);

    // A `Measured` *literal* (`3500 kg`, `150 EUR`) is now scaled at lowering
    // and executes from source (issue #47 Slice 1.5). Only a *non-literal*
    // unit value (`x EUR`), which cannot be constant-folded yet, still defers
    // the whole function to its hand-registered path.
    if body_defers_unit_ctor(&body) {
        return None;
    }

    Some(core::FnDef {
        name: qi,
        params,
        ret: sig.ret,
        effects: sig.effects,
        is_partial: f.partial,
        body,
    })
}

/// Whether a lowered function body contains an *unscalable* unit constructor —
/// a `brix.units.*` call whose value is not a constant `Int` literal, so it
/// couldn't be folded to the canonical minor unit at lowering. Such a body
/// stays deferred to its hand-registered path (see [`lower_fn`]); a body whose
/// unit literals are all constants executes from source.
fn body_defers_unit_ctor(expr: &core::Expr) -> bool {
    match &*expr.kind {
        core::ExprKind::Call { func, args } => {
            let unscalable = func.to_string().starts_with("brix.units.")
                && !matches!(
                    args.first().map(|a| &*a.kind),
                    Some(core::ExprKind::Lit(brix_ir::pattern::Lit::Int(_)))
                );
            unscalable || args.iter().any(body_defers_unit_ctor)
        }
        core::ExprKind::If { cond, then, els } => {
            body_defers_unit_ctor(cond) || body_defers_unit_ctor(then) || body_defers_unit_ctor(els)
        }
        core::ExprKind::Field { base, .. } => body_defers_unit_ctor(base),
        core::ExprKind::Record { fields } => fields.iter().any(|(_, e)| body_defers_unit_ctor(e)),
        core::ExprKind::Try { inner, .. } => body_defers_unit_ctor(inner),
        core::ExprKind::Comprehension { yields, .. } => {
            yields.as_ref().is_some_and(body_defers_unit_ctor)
        }
        core::ExprKind::Var(_) | core::ExprKind::Lit(_) => false,
    }
}
