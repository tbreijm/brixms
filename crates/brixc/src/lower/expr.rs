//! AST `ExprKind` → `core::ExprKind` (design §"Expr table"), plus the
//! shared pattern-argument resolver (design §"Clause table" / "Head")
//! since both need the same var-vs-literal-vs-enum-variant disambiguation.
//!
//! [`BodyCtx`] bundles everything one declaration's body lowering threads
//! through: the read-only resolver, the two mutable side channels
//! (`meta`/`diags`), the flat pattern-variable scope, the edge-alias set
//! (for `mask` validation), the per-decl `SiteAssigner` (design: "One
//! `SiteAssigner::new(decl_name)` per decl; `?` sites left-to-right
//! depth-first"), and the clause-visit ordinal counter mismatch (C)'s side
//! table keys on.

use std::collections::BTreeSet;

use brix_ast::ast;
use brix_diag::{Diagnostic, Span};
use brix_ir::core::{Expr as IrExpr, ExprKind, ExprOrigin, SourceRange};
use brix_ir::effects::EffectRow;
use brix_ir::frontend::SchemaResolver;
use brix_ir::ident::{Ident as IrIdent, QualIdent};
use brix_ir::pattern::{Arg, Lit, RoleArg};
use brix_ir::site::SiteAssigner;
use brix_ir::types::{IntWidth, Row, RowField, Ty};

use super::diag;
use super::resolve::{LowerMeta, ProgramResolver, UnitClass, VariantLookup};

/// Per-declaration lowering state (design: "one flat scope per body").
/// Constructed fresh in `decl.rs` for each `derive`/`constraint`/`query`.
pub struct BodyCtx<'a> {
    /// Declaration owning every expression lowered through this context.
    /// Together with the AST byte range it gives each Core expression a
    /// stable, content-addressed origin (see `core::ExprOrigin`).
    pub decl_name: IrIdent,
    pub resolver: &'a ProgramResolver,
    pub meta: &'a mut LowerMeta,
    pub diags: &'a mut Vec<Diagnostic>,
    /// Pattern-variable scope: first occurrence binds, later occurrences
    /// equijoin (design §"Name/variable resolution"). Flat and monotonic —
    /// nested clauses see and can add to it; `Pattern::bound_vars` (brix-ir,
    /// the IR-side authority) is what ultimately decides what a nested
    /// pattern *exports*, not this set.
    pub bound: BTreeSet<IrIdent>,
    /// Names bound specifically as an edge alias (`x @ R(...)`), a subset
    /// of `bound` — `mask(target) by reason` needs exactly this subset
    /// (design: "both idents MUST be edge-bound aliases").
    pub edge_aliases: BTreeSet<IrIdent>,
    pub sites: SiteAssigner,
    /// Running union of declared `EffectRow`s of fns resolved inside this
    /// body's `let`/`when` values (design: "effects = union of declared
    /// EffectRows of fns resolved in body let/when"), accumulated as each
    /// such clause is lowered (see `decl::lower_let_clause`/
    /// `lower_when_clause`).
    pub effects: EffectRow,
    next_clause: u32,
}

impl<'a> BodyCtx<'a> {
    pub fn new(
        decl_name: IrIdent,
        resolver: &'a ProgramResolver,
        meta: &'a mut LowerMeta,
        diags: &'a mut Vec<Diagnostic>,
    ) -> Self {
        let sites = SiteAssigner::new(decl_name.clone());
        BodyCtx {
            decl_name,
            resolver,
            meta,
            diags,
            bound: BTreeSet::new(),
            edge_aliases: BTreeSet::new(),
            sites,
            effects: EffectRow::empty(),
            next_clause: 0,
        }
    }

    /// (C)'s side-table key: a DFS pre-order ordinal over every clause this
    /// decl's body visits (top-level and nested), stable because clause
    /// lowering always increments it before recursing into children.
    pub fn next_clause_ordinal(&mut self) -> u32 {
        let o = self.next_clause;
        self.next_clause += 1;
        o
    }
}

/// The only place in this crate an `f64` *value* is touched (hard
/// constraint: "the only place an f64 value may be touched is the Float
/// literal lowering"). NaN canonicalizes to Rust's standard quiet-NaN bit
/// pattern (`0x7ff8_0000_0000_0000`, the same constant brix-canon's
/// `total_order_key_f64` folds to) before the *bits* — never the `f64`
/// itself — go into [`Lit::F64Bits`] (Part V §8).
fn canon_f64_bits(x: f64) -> u64 {
    if x.is_nan() {
        f64::NAN.to_bits()
    } else {
        x.to_bits()
    }
}

fn int_lit(i: i128, span: Span, diags: &mut Vec<Diagnostic>) -> Option<Lit> {
    match i64::try_from(i) {
        Ok(v) => Some(Lit::Int(v)),
        Err(_) => {
            diags.push(diag::error(
                diag::INT_OVERFLOW,
                span,
                format!("integer literal `{i}` overflows i64"),
            ));
            None
        }
    }
}

fn poison(meta: &mut LowerMeta) -> IrExpr {
    IrExpr::new(
        Ty::Var(meta.fresh_tyvar()),
        ExprKind::Var(IrIdent::new("%error")),
    )
}

// ---------------------------------------------------------------------
// Pattern-argument resolution (shared by Head/Edge/Entity/History/
// StructLit-field lowering in decl.rs).
// ---------------------------------------------------------------------

/// The parser (see `brix-ast`'s `Parser::arg`) represents *punning*
/// (`amount` meaning `amount: amount`) the same way it represents an
/// explicit `name: value` — both set `arg.name = Some(..)` — because
/// Appendix D shares one `ArgList` production across struct/pattern/head
/// positions (where that conflation is harmless: the role name is
/// `arg.name` either way) *and* call argument lists (where it is not: a
/// bare-ident call argument like `surcharge(w)` must stay positional, not
/// become a named argument for a parameter literally called `w`). The two
/// cases are distinguishable by span: punning constructs `value` with
/// `value.span == name.span` (both point at the same bare identifier
/// token); a real `name: value` has `name.span` strictly before and
/// disjoint from `value.span`. Call-argument lowering uses this to decide
/// whether an arg is genuinely named.
fn is_true_named(arg: &ast::Arg) -> bool {
    matches!(&arg.name, Some(n) if n.span != arg.value.span)
}

/// Split an `ast::Arg` into `(role, value)`, expanding punning (design:
/// "pun amount → role: Var(amount)"). `None` when the arg is punned but its
/// value is not a bare identifier (nothing sane to pun against).
fn arg_role_and_value(arg: &ast::Arg) -> Option<(IrIdent, &ast::Expr)> {
    match &arg.name {
        Some(n) => Some((IrIdent::new(n.text.clone()), &arg.value)),
        None => match &*arg.value.kind {
            ast::ExprKind::Ident(p) if p.segments.len() == 1 => {
                Some((IrIdent::new(p.segments[0].text.clone()), &arg.value))
            }
            _ => None,
        },
    }
}

/// Just the role name half of [`arg_role_and_value`], for callers (`decl.rs`)
/// that need the role name to look up a type hint *before* deciding whether
/// to resolve the value at all.
pub fn arg_role(arg: &ast::Arg) -> Option<IrIdent> {
    arg_role_and_value(arg).map(|(r, _)| r)
}

/// Resolve one `role: value` pattern argument (Head/Edge/Entity/History) to
/// an `ir::pattern::RoleArg`. `role_ty_hint` is the role's declared type
/// when known (from the relation/entity schema) — the type-directed
/// enum-variant disambiguation input (mismatch B). Returns `None` on an
/// unrecoverable v0 error (already diagnosed); the caller drops that one
/// arg rather than the whole clause.
pub fn resolve_pattern_arg(
    ctx: &mut BodyCtx,
    arg: &ast::Arg,
    role_ty_hint: Option<&Ty>,
) -> Option<RoleArg> {
    let Some((role, value)) = arg_role_and_value(arg) else {
        ctx.diags.push(diag::error(
            diag::UNSUPPORTED_V0,
            arg.span,
            "punned argument must be a bare identifier",
        ));
        return None;
    };
    let ir_arg = resolve_arg_value(ctx, value, role_ty_hint)?;
    Some(RoleArg { role, arg: ir_arg })
}

fn resolve_arg_value(
    ctx: &mut BodyCtx,
    value: &ast::Expr,
    role_ty_hint: Option<&Ty>,
) -> Option<Arg> {
    match &*value.kind {
        ast::ExprKind::Ident(p) => resolve_arg_ident(ctx, p, value.span, role_ty_hint),
        ast::ExprKind::Int(i) => int_lit(*i, value.span, ctx.diags).map(Arg::Lit),
        ast::ExprKind::Float(f) => Some(Arg::Lit(Lit::F64Bits(canon_f64_bits(*f)))),
        ast::ExprKind::Str(s) => Some(Arg::Lit(Lit::Str(s.clone()))),
        ast::ExprKind::Bool(b) => Some(Arg::Lit(Lit::Bool(*b))),
        ast::ExprKind::Error(_) => None,
        _ => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                value.span,
                "pattern argument must be a variable or a literal",
            ));
            None
        }
    }
}

fn resolve_arg_ident(
    ctx: &mut BodyCtx,
    p: &ast::Path,
    span: Span,
    role_ty_hint: Option<&Ty>,
) -> Option<Arg> {
    if p.segments.len() == 1 {
        let ident = IrIdent::new(p.segments[0].text.clone());
        // Priority (design §"Enum-variant literals resolved TYPE-DIRECTED"):
        // (1) already bound -> Var; (2) matches the role's declared enum's
        // variant -> Lit::Enum; (3) fresh binding -> Var.
        if ctx.bound.contains(&ident) {
            return Some(Arg::Var(ident));
        }
        if let Some(Ty::Enum(qual)) = role_ty_hint {
            if let Some(ord) = ctx.resolver.variant_ordinal(qual, ident.as_str()) {
                return Some(Arg::Lit(Lit::Enum {
                    ty: qual.clone(),
                    ordinal: ord,
                }));
            }
        }
        ctx.bound.insert(ident.clone());
        return Some(Arg::Var(ident));
    }
    // Qualified (`Tier.Standard`): must resolve as an enum variant.
    let prefix = ast::Path {
        segments: p.segments[..p.segments.len() - 1].to_vec(),
        span,
    };
    let enum_qi = ctx.resolver.resolve_path(&prefix);
    let variant = p.segments.last().unwrap().text.as_str();
    if let Some(ord) = ctx.resolver.variant_ordinal(&enum_qi, variant) {
        return Some(Arg::Lit(Lit::Enum {
            ty: enum_qi,
            ordinal: ord,
        }));
    }
    ctx.diags.push(diag::error(
        diag::UNBOUND_IDENT,
        span,
        format!("unresolved name `{}`", ctx.resolver.resolve_path(p)),
    ));
    None
}

// ---------------------------------------------------------------------
// General expression lowering.
// ---------------------------------------------------------------------

pub fn lower_expr(ctx: &mut BodyCtx, e: &ast::Expr) -> IrExpr {
    let lowered = match &*e.kind {
        ast::ExprKind::Int(i) => match int_lit(*i, e.span, ctx.diags) {
            Some(lit) => IrExpr::new(Ty::Var(ctx.meta.fresh_tyvar()), ExprKind::Lit(lit)),
            None => poison(ctx.meta),
        },
        ast::ExprKind::Float(f) => {
            IrExpr::new(Ty::F64, ExprKind::Lit(Lit::F64Bits(canon_f64_bits(*f))))
        }
        ast::ExprKind::Str(s) => IrExpr::new(Ty::Str, ExprKind::Lit(Lit::Str(s.clone()))),
        ast::ExprKind::Bool(b) => IrExpr::new(Ty::Bool, ExprKind::Lit(Lit::Bool(*b))),
        ast::ExprKind::Measured { value, unit } => lower_measured(ctx, value, unit, e.span),
        ast::ExprKind::Ident(p) => lower_ident(ctx, p, e.span),
        ast::ExprKind::Unary { op, expr } => {
            let inner = lower_expr(ctx, expr);
            let name = match op {
                ast::UnOp::Neg => "neg",
                ast::UnOp::Not => "not",
            };
            IrExpr::new(
                Ty::Var(ctx.meta.fresh_tyvar()),
                ExprKind::Call {
                    func: QualIdent::from(format!("brix.ops.{name}").as_str()),
                    args: vec![inner],
                },
            )
        }
        ast::ExprKind::Binary { op, lhs, rhs } => lower_binary(ctx, *op, lhs, rhs, e.span),
        ast::ExprKind::Call { callee, args } => lower_call(ctx, callee, args, e.span, None),
        ast::ExprKind::StructLit { path, fields } => lower_struct_lit(ctx, path.as_ref(), fields),
        ast::ExprKind::Field { base, name } => lower_field(ctx, base, name),
        ast::ExprKind::Try(inner) => {
            let inner_e = lower_expr(ctx, inner);
            let site = ctx.sites.next_site();
            let ty = match &inner_e.ty {
                Ty::Result(t, _) => (**t).clone(),
                _ => Ty::Var(ctx.meta.fresh_tyvar()),
            };
            IrExpr::new(
                ty,
                ExprKind::Try {
                    inner: inner_e,
                    site,
                },
            )
        }
        ast::ExprKind::If { cond, then, else_ } => {
            lower_if(ctx, cond, then, else_.as_ref(), e.span)
        }
        ast::ExprKind::Paren(inner) => lower_expr(ctx, inner),
        ast::ExprKind::From { block, yield_ } => lower_comprehension(ctx, block, yield_.as_ref()),
        ast::ExprKind::Ellipsis => {
            ctx.diags.push(diag::error(
                diag::ELLIPSIS,
                e.span,
                "`...` cannot be compiled",
            ));
            poison(ctx.meta)
        }
        // Parser-level recovery node: it already carries the diagnostic.
        ast::ExprKind::Error(_) => poison(ctx.meta),
        // Match / Closure / Block / Range / Succeed / Fail / AdapterScript /
        // Versioned / Generic: no home in v0 rule/query/constraint bodies
        // (design: "none v0 error if reached" / "not reachable in flagship
        // lowered decls" — reachable only from driver/scenario vocabulary,
        // which is deferred wholesale before we ever lower an expr here).
        _ => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                e.span,
                "this expression form is unsupported in v0",
            ));
            poison(ctx.meta)
        }
    };
    lowered.with_origin(ExprOrigin::source(
        &ctx.decl_name,
        SourceRange {
            start: e.span.start,
            end: e.span.end,
        },
    ))
}

fn lower_binary(
    ctx: &mut BodyCtx,
    op: ast::BinOp,
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    span: Span,
) -> IrExpr {
    match op {
        ast::BinOp::Tilde | ast::BinOp::Colon => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                span,
                format!("`{}` operator is unsupported in v0", op.as_str()),
            ));
            return poison(ctx.meta);
        }
        ast::BinOp::Pipe => {
            let a = lower_expr(ctx, lhs);
            return match &*rhs.kind {
                ast::ExprKind::Call { callee, args } => {
                    lower_call(ctx, callee, args, span, Some(a))
                }
                _ => {
                    ctx.diags.push(diag::error(
                        diag::UNSUPPORTED_V0,
                        span,
                        "`|>` right-hand side must be a call",
                    ));
                    poison(ctx.meta)
                }
            };
        }
        _ => {}
    }
    let l = lower_expr(ctx, lhs);
    let r = lower_expr(ctx, rhs);
    let name = match op {
        ast::BinOp::Or => "or",
        ast::BinOp::And => "and",
        ast::BinOp::Eq => "eq",
        ast::BinOp::Ne => "ne",
        ast::BinOp::Lt => "lt",
        ast::BinOp::Le => "le",
        ast::BinOp::Gt => "gt",
        ast::BinOp::Ge => "ge",
        ast::BinOp::In => "in",
        ast::BinOp::Add => "add",
        ast::BinOp::Sub => "sub",
        ast::BinOp::Mul => "mul",
        ast::BinOp::Div => "div",
        ast::BinOp::Pipe | ast::BinOp::Tilde | ast::BinOp::Colon => unreachable!("handled above"),
    };
    IrExpr::new(
        Ty::Var(ctx.meta.fresh_tyvar()),
        ExprKind::Call {
            func: QualIdent::from(format!("brix.ops.{name}").as_str()),
            args: vec![l, r],
        },
    )
}

fn lower_call(
    ctx: &mut BodyCtx,
    callee: &ast::Expr,
    args: &[ast::Arg],
    span: Span,
    prefix: Option<IrExpr>,
) -> IrExpr {
    // `count(from { ... })` (design: "count(from{..})→Call{count,
    // [Comprehension]}").
    if prefix.is_none() {
        if let ast::ExprKind::Ident(p) = &*callee.kind {
            if p.segments.len() == 1
                && p.segments[0].text == "count"
                && args.len() == 1
                && args[0].name.is_none()
            {
                if let ast::ExprKind::From { .. } = &*args[0].value.kind {
                    let comp = lower_expr(ctx, &args[0].value);
                    return IrExpr::new(
                        Ty::Int(IntWidth::I64),
                        ExprKind::Call {
                            func: QualIdent::simple("count"),
                            args: vec![comp],
                        },
                    );
                }
            }
        }
    }

    let func = match &*callee.kind {
        ast::ExprKind::Ident(p) => ctx.resolver.resolve_path(p),
        _ => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                callee.span,
                "call target must be a name",
            ));
            return poison(ctx.meta);
        }
    };

    let fn_info = ctx.meta.fn_info(&func).cloned();
    let mut lowered_args: Vec<IrExpr> = Vec::new();
    if let Some(p) = prefix {
        lowered_args.push(p);
    }

    let has_named = args.iter().any(is_true_named);
    if has_named {
        match &fn_info {
            Some(info) => {
                let mut by_name: std::collections::BTreeMap<&str, &ast::Arg> =
                    std::collections::BTreeMap::new();
                for a in args {
                    if is_true_named(a) {
                        by_name.insert(a.name.as_ref().unwrap().text.as_str(), a);
                    } else {
                        ctx.diags.push(diag::error(
                            diag::UNSUPPORTED_V0,
                            a.span,
                            "cannot mix positional and named arguments in one call",
                        ));
                    }
                }
                for pname in &info.param_names {
                    match by_name.remove(pname.as_str()) {
                        Some(a) => lowered_args.push(lower_expr(ctx, &a.value)),
                        None => ctx.diags.push(diag::error(
                            diag::UNSUPPORTED_V0,
                            span,
                            format!("call to `{func}` is missing argument `{pname}`"),
                        )),
                    }
                }
                for (name, a) in by_name {
                    ctx.diags.push(diag::error(
                        diag::UNSUPPORTED_V0,
                        a.span,
                        format!("`{func}` has no parameter named `{name}`"),
                    ));
                }
            }
            None => {
                // design: "named args to unknown fn=error"
                ctx.diags.push(diag::error(
                    diag::UNSUPPORTED_V0,
                    span,
                    format!("cannot resolve named arguments: unknown function `{func}`"),
                ));
                for a in args {
                    lowered_args.push(lower_expr(ctx, &a.value));
                }
            }
        }
    } else {
        for a in args {
            lowered_args.push(lower_expr(ctx, &a.value));
        }
    }

    let ret_ty = ctx
        .resolver
        .function(&func)
        .map(|s| s.ret.clone())
        .unwrap_or_else(|| Ty::Var(ctx.meta.fresh_tyvar()));
    IrExpr::new(
        ret_ty,
        ExprKind::Call {
            func,
            args: lowered_args,
        },
    )
}

fn lower_ident(ctx: &mut BodyCtx, p: &ast::Path, span: Span) -> IrExpr {
    if p.segments.len() == 1 {
        let ident = IrIdent::new(p.segments[0].text.clone());
        if ctx.bound.contains(&ident) {
            return IrExpr::new(Ty::Var(ctx.meta.fresh_tyvar()), ExprKind::Var(ident));
        }
        match ctx.resolver.find_unique_variant(ident.as_str()) {
            VariantLookup::Unique(qual, ord) => {
                return IrExpr::new(
                    Ty::Enum(qual.clone()),
                    ExprKind::Lit(Lit::Enum {
                        ty: qual,
                        ordinal: ord,
                    }),
                );
            }
            VariantLookup::Ambiguous => {
                ctx.diags.push(diag::error(
                    diag::AMBIGUOUS_VARIANT,
                    span,
                    format!("`{ident}` matches variants of more than one enum in scope; qualify it, e.g. `Enum.{ident}`"),
                ));
                return poison(ctx.meta);
            }
            VariantLookup::None => {}
        }
        let qi = QualIdent::simple(ident.as_str());
        if ctx.resolver.function(&qi).is_some() {
            return IrExpr::new(
                Ty::Var(ctx.meta.fresh_tyvar()),
                ExprKind::Call {
                    func: qi,
                    args: vec![],
                },
            );
        }
        ctx.diags.push(diag::error(
            diag::UNBOUND_IDENT,
            span,
            format!("unresolved name `{ident}`"),
        ));
        return poison(ctx.meta);
    }

    // Qualified: an enum variant (`Tier.Standard`) or a qualified fn/const.
    let prefix = ast::Path {
        segments: p.segments[..p.segments.len() - 1].to_vec(),
        span,
    };
    let enum_qi = ctx.resolver.resolve_path(&prefix);
    let variant = p.segments.last().unwrap().text.as_str();
    if let Some(ord) = ctx.resolver.variant_ordinal(&enum_qi, variant) {
        return IrExpr::new(
            Ty::Enum(enum_qi.clone()),
            ExprKind::Lit(Lit::Enum {
                ty: enum_qi,
                ordinal: ord,
            }),
        );
    }
    let qi = ctx.resolver.resolve_path(p);
    if ctx.resolver.function(&qi).is_some() {
        return IrExpr::new(
            Ty::Var(ctx.meta.fresh_tyvar()),
            ExprKind::Call {
                func: qi,
                args: vec![],
            },
        );
    }
    ctx.diags.push(diag::error(
        diag::UNBOUND_IDENT,
        span,
        format!("unresolved name `{qi}`"),
    ));
    poison(ctx.meta)
}

fn lower_measured(ctx: &mut BodyCtx, value: &ast::Expr, unit: &ast::Ident, span: Span) -> IrExpr {
    let v = lower_expr(ctx, value);
    let class = ctx.resolver.unit_class(unit.text.as_str()).cloned();
    let ctor = || {
        QualIdent::from_segments([
            IrIdent::new("brix"),
            IrIdent::new("units"),
            IrIdent::new(unit.text.clone()),
        ])
    };
    match class {
        Some(UnitClass::Quantity(measure)) => IrExpr::new(
            Ty::Quantity(measure),
            ExprKind::Call {
                func: ctor(),
                args: vec![v],
            },
        ),
        Some(UnitClass::Duration) => IrExpr::new(
            Ty::Duration,
            ExprKind::Call {
                func: ctor(),
                args: vec![v],
            },
        ),
        Some(UnitClass::Money(currency)) => IrExpr::new(
            Ty::Money(currency),
            ExprKind::Call {
                func: ctor(),
                args: vec![v],
            },
        ),
        None => {
            ctx.diags.push(diag::error(
                diag::UNKNOWN_UNIT,
                span,
                format!("unknown unit `{}`", unit.text),
            ));
            poison(ctx.meta)
        }
    }
}

/// Struct literals retain their field names in Core IR so row inference does
/// not depend on a positional lowering convention.
fn lower_struct_lit(ctx: &mut BodyCtx, _path: Option<&ast::Path>, fields: &[ast::Arg]) -> IrExpr {
    let mut named: Vec<(IrIdent, IrExpr)> = Vec::new();
    for a in fields {
        let Some((role, value)) = arg_role_and_value(a) else {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                a.span,
                "punned struct-literal field must be a bare identifier",
            ));
            continue;
        };
        let v = lower_expr(ctx, value);
        named.push((role, v));
    }
    let row = Row::closed(
        named
            .iter()
            .map(|(n, e)| RowField {
                name: n.clone(),
                ty: e.ty.clone(),
            })
            .collect(),
    );
    IrExpr::new(Ty::record(row), ExprKind::Record { fields: named })
}

fn lower_field(ctx: &mut BodyCtx, base: &ast::Expr, name: &ast::Ident) -> IrExpr {
    // design: "only when base head segment is a bound var; else whole chain
    // resolves as QualIdent first."
    if let ast::ExprKind::Ident(p) = &*base.kind {
        if p.segments.len() == 1 {
            let ident = IrIdent::new(p.segments[0].text.clone());
            if ctx.bound.contains(&ident) {
                let base_e = IrExpr::new(Ty::Var(ctx.meta.fresh_tyvar()), ExprKind::Var(ident));
                return IrExpr::new(
                    Ty::Var(ctx.meta.fresh_tyvar()),
                    ExprKind::Field {
                        base: base_e,
                        field: IrIdent::new(name.text.clone()),
                    },
                );
            }
            let mut segs = p.segments.clone();
            segs.push(name.clone());
            let full = ast::Path {
                segments: segs,
                span: name.span,
            };
            return lower_ident(ctx, &full, name.span);
        }
    }
    ctx.diags.push(diag::error(
        diag::UNSUPPORTED_V0,
        base.span,
        "field base must be a bound variable or a qualified name",
    ));
    poison(ctx.meta)
}

fn lower_if(
    ctx: &mut BodyCtx,
    cond: &ast::Expr,
    then: &ast::IfBody,
    else_: Option<&ast::Expr>,
    span: Span,
) -> IrExpr {
    let c = lower_expr(ctx, cond);
    let then_e = match then {
        ast::IfBody::Then(e) => lower_expr(ctx, e),
        ast::IfBody::Block(_) => {
            ctx.diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                span,
                "block-bodied `if` is unsupported in v0",
            ));
            poison(ctx.meta)
        }
    };
    let els_e = match else_ {
        Some(e) => lower_expr(ctx, e),
        None => IrExpr::new(Ty::Unit, ExprKind::Lit(Lit::Unit)),
    };
    let ty = then_e.ty.clone();
    IrExpr::new(
        ty,
        ExprKind::If {
            cond: c,
            then: then_e,
            els: els_e,
        },
    )
}

fn lower_comprehension(
    ctx: &mut BodyCtx,
    block: &ast::Block,
    yield_: Option<&ast::Expr>,
) -> IrExpr {
    let pattern = super::decl::lower_block(ctx, block);
    let yields = yield_.map(|y| lower_expr(ctx, y));
    let ty = match &yields {
        Some(y) => Ty::rel(Row::closed(match &y.ty {
            Ty::Record(row) => row.fields.clone(),
            _ => vec![],
        })),
        None => Ty::Var(ctx.meta.fresh_tyvar()),
    };
    IrExpr::new(ty, ExprKind::Comprehension { pattern, yields })
}

/// Walk a lowered `core::Expr` tree collecting the union of declared
/// `EffectRow`s of every `Call` whose callee resolves to a known fn
/// (design: "effects = union of declared EffectRows of fns resolved in
/// body let/when; unknown fn → empty row, NOT open tail"). Does not descend
/// into a nested `Comprehension`'s pattern (its own `let`/`when` bodies are
/// lowered, and their effects collected, independently by `decl.rs` when it
/// processes that nested block's clauses).
pub fn effects_of(e: &IrExpr, resolver: &ProgramResolver) -> EffectRow {
    let mut acc = EffectRow::empty();
    collect_effects(e, resolver, &mut acc);
    acc
}

fn collect_effects(e: &IrExpr, resolver: &ProgramResolver, acc: &mut EffectRow) {
    match &*e.kind {
        ExprKind::Call { func, args } => {
            if let Some(sig) = resolver.function(func) {
                *acc = acc.combine(&sig.effects);
            }
            for a in args {
                collect_effects(a, resolver, acc);
            }
        }
        ExprKind::Field { base, .. } => collect_effects(base, resolver, acc),
        ExprKind::If { cond, then, els } => {
            collect_effects(cond, resolver, acc);
            collect_effects(then, resolver, acc);
            collect_effects(els, resolver, acc);
        }
        ExprKind::Try { inner, .. } => collect_effects(inner, resolver, acc),
        ExprKind::Comprehension { yields, .. } => {
            if let Some(y) = yields {
                collect_effects(y, resolver, acc);
            }
        }
        ExprKind::Record { fields } => {
            for (_, value) in fields {
                collect_effects(value, resolver, acc);
            }
        }
        ExprKind::Var(_) | ExprKind::Lit(_) => {}
    }
}
