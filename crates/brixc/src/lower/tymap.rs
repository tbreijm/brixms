//! `ast::Type` → `ir::types::Ty` (design §"tymap (AST Type → Ty)").

use brix_ast::ast::{self, TypeArg, TypeKind};
use brix_diag::{Diagnostic, Span};
use brix_ir::ident::Ident as IrIdent;
use brix_ir::types::{
    dimensions_div, money_dimensions, quantity_dimensions, Dimensions, IntWidth, Row, RowField, Ty,
};

use super::diag;
use super::resolve::{LowerMeta, ProgramResolver};

/// Where a type appears — controls whether an unresolved declared-type name
/// is an error or a warning (design: "unresolved Named in role pos → name-
/// res ERROR + Ty::Var; in fn sig → Ty::Var + warning").
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TyPos {
    Role,
    FnSig,
}

pub fn lower_type(
    ty: &ast::Type,
    pos: TyPos,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Ty {
    match &ty.kind {
        TypeKind::Named { path, args } => {
            lower_named(path, args, ty.span, pos, resolver, meta, diags)
        }
        TypeKind::Row { fields, rest } => {
            Ty::record(lower_row(fields, rest, pos, resolver, meta, diags))
        }
        TypeKind::Div(left, right) => {
            let l = lower_type(left, pos, resolver, meta, diags);
            let r = lower_type(right, pos, resolver, meta, diags);
            match (ground_dimensions(&l), ground_dimensions(&r)) {
                (Some(a), Some(b)) => Ty::Dimensioned(dimensions_div(&a, &b)),
                _ => {
                    diags.push(diag::error(diag::COMPOUND_UNIT, ty.span,
                        "compound unit operands must be Quantity, Money, or another ground dimension"));
                    Ty::Var(meta.fresh_tyvar())
                }
            }
        }
    }
}

fn ground_dimensions(ty: &Ty) -> Option<Dimensions> {
    match ty {
        Ty::Quantity(m) => Some(quantity_dimensions(m)),
        Ty::Money(c) => Some(money_dimensions(c)),
        Ty::Dimensioned(ds) => Some(ds.clone()),
        _ => None,
    }
}

fn lower_row(
    fields: &[(ast::Ident, ast::Type)],
    rest: &Option<Box<ast::Type>>,
    pos: TyPos,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Row {
    let out: Vec<RowField> = fields
        .iter()
        .map(|(name, t)| RowField {
            name: IrIdent::new(name.text.clone()),
            ty: lower_type(t, pos, resolver, meta, diags),
        })
        .collect();
    if rest.is_some() {
        Row::open(out, meta.fresh_tyvar())
    } else {
        Row::closed(out)
    }
}

fn lower_named(
    path: &ast::Path,
    args: &[TypeArg],
    span: Span,
    pos: TyPos,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Ty {
    if path.segments.len() == 1 {
        let name = path.segments[0].text.as_str();
        if args.is_empty() {
            if let Some(t) = builtin_ty(name) {
                return t;
            }
        } else if let Some(t) = generic_ty(name, args, span, pos, resolver, meta, diags) {
            return t;
        }
    }

    let qi = resolver.resolve_path(path);
    // Prelude measures are dimension names in type position.  They are not
    // value-level units (which have their own `UnitClass`), so normalize them
    // directly to their one-dimensional quantity type.
    if args.is_empty() {
        if let Some(last) = qi.segments().last() {
            if matches!(last.as_str(), "Mass" | "Kilometre") {
                return Ty::Quantity(last.clone());
            }
        }
    }
    if resolver.is_entity(&qi) {
        let last = qi
            .segments()
            .last()
            .cloned()
            .unwrap_or_else(|| IrIdent::new(""));
        return Ty::NodeRef(last);
    }
    if resolver.is_enum(&qi) {
        // Mismatch (A): the whole reason `Ty::Enum` exists.
        return Ty::Enum(qi);
    }
    if let Some(t) = resolver.alias_ty(&qi) {
        return t.clone();
    }

    let msg = format!("unresolved type `{qi}`");
    match pos {
        TyPos::Role => diags.push(diag::error(diag::UNRESOLVED_TYPE, span, msg)),
        TyPos::FnSig => diags.push(diag::warning(diag::UNRESOLVED_TYPE, span, msg)),
    }
    Ty::Var(meta.fresh_tyvar())
}

#[allow(clippy::too_many_arguments)]
fn generic_ty(
    name: &str,
    args: &[TypeArg],
    span: Span,
    pos: TyPos,
    resolver: &ProgramResolver,
    meta: &mut LowerMeta,
    diags: &mut Vec<Diagnostic>,
) -> Option<Ty> {
    let arg_ty = |i: usize, meta: &mut LowerMeta, diags: &mut Vec<Diagnostic>| -> Ty {
        match args.get(i).and_then(type_arg_type) {
            Some(t) => lower_type(t, pos, resolver, meta, diags),
            None => {
                diags.push(diag::error(
                    diag::UNSUPPORTED_V0,
                    span,
                    format!("`{name}` expects a type argument at position {i}"),
                ));
                Ty::Var(meta.fresh_tyvar())
            }
        }
    };

    Some(match name {
        "Option" => Ty::option(arg_ty(0, meta, diags)),
        "Result" => Ty::Result(
            Box::new(arg_ty(0, meta, diags)),
            Box::new(arg_ty(1, meta, diags)),
        ),
        "List" => Ty::list(arg_ty(0, meta, diags)),
        "Vector" => Ty::Vector(Box::new(arg_ty(0, meta, diags))),
        "Set" => Ty::Set(Box::new(arg_ty(0, meta, diags))),
        "Map" => Ty::Map(
            Box::new(arg_ty(0, meta, diags)),
            Box::new(arg_ty(1, meta, diags)),
        ),
        "Bag" => Ty::Bag(Box::new(arg_ty(0, meta, diags))),
        "Estimate" => Ty::Estimate(Box::new(arg_ty(0, meta, diags))),
        "Rel" => {
            let row = match args.first().and_then(type_arg_type) {
                Some(ast::Type {
                    kind: TypeKind::Row { fields, rest },
                    ..
                }) => lower_row(fields, rest, pos, resolver, meta, diags),
                _ => {
                    diags.push(diag::error(
                        diag::UNSUPPORTED_V0,
                        span,
                        "`Rel<...>` expects a row type argument",
                    ));
                    Row::closed(vec![])
                }
            };
            Ty::rel(row)
        }
        "Quantity" => Ty::Quantity(type_arg_ident(args.first()).unwrap_or_else(|| {
            diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                span,
                "malformed `Quantity<M>`",
            ));
            IrIdent::new("?")
        })),
        "Money" => Ty::Money(type_arg_ident(args.first()).unwrap_or_else(|| {
            diags.push(diag::error(
                diag::UNSUPPORTED_V0,
                span,
                "malformed `Money<C>`",
            ));
            IrIdent::new("?")
        })),
        "Decimal" => {
            let precision = type_arg_lit_u32(args.first()).unwrap_or(0);
            let scale = type_arg_lit_u32(args.get(1)).unwrap_or(0);
            Ty::Decimal { precision, scale }
        }
        _ => return None,
    })
}

fn type_arg_type(arg: &TypeArg) -> Option<&ast::Type> {
    match arg {
        TypeArg::Type(t) => Some(t),
        TypeArg::Lit(_) => None,
    }
}

/// Pull the bare type name out of a `TypeArg::Type(Named{path,args:[]})`
/// (e.g. the `Mass` in `Quantity<Mass>`) — v0 carries it as a raw ident,
/// never resolving whether a `measure`/`unit` by that name actually exists.
fn type_arg_ident(arg: Option<&TypeArg>) -> Option<IrIdent> {
    match arg {
        Some(TypeArg::Type(ast::Type {
            kind: TypeKind::Named { path, .. },
            ..
        })) => path.segments.last().map(|s| IrIdent::new(s.text.clone())),
        _ => None,
    }
}

fn type_arg_lit_u32(arg: Option<&TypeArg>) -> Option<u32> {
    match arg {
        Some(TypeArg::Lit(e)) => match &*e.kind {
            ast::ExprKind::Int(i) => u32::try_from(*i).ok(),
            _ => None,
        },
        _ => None,
    }
}

fn builtin_ty(name: &str) -> Option<Ty> {
    Some(match name {
        "String" => Ty::Str,
        "Bool" => Ty::Bool,
        "Instant" => Ty::Instant,
        "Duration" => Ty::Duration,
        "Date" => Ty::Date,
        "Char" => Ty::Char,
        "Bytes" => Ty::Bytes,
        "EventId" => Ty::EventId,
        "Probability" => Ty::Probability,
        "TimeOfDay" => Ty::TimeOfDay,
        "TimeZone" => Ty::TimeZone,
        "Unit" => Ty::Unit,
        "I8" => Ty::Int(IntWidth::I8),
        "I16" => Ty::Int(IntWidth::I16),
        "I32" => Ty::Int(IntWidth::I32),
        "I64" => Ty::Int(IntWidth::I64),
        "I128" => Ty::Int(IntWidth::I128),
        "U8" => Ty::Int(IntWidth::U8),
        "U16" => Ty::Int(IntWidth::U16),
        "U32" => Ty::Int(IntWidth::U32),
        "U64" => Ty::Int(IntWidth::U64),
        "U128" => Ty::Int(IntWidth::U128),
        "Int" => Ty::Int(IntWidth::Int),
        "Nat" => Ty::Int(IntWidth::Nat),
        "F32" => Ty::F32,
        "F64" => Ty::F64,
        _ => return None,
    })
}
