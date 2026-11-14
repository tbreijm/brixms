//! Bounded validation of canonical closed type payloads used by typed contexts.
//!
//! The decoder consumes the existing `Ty` encoding from `type_realization`;
//! it does not define a second encoding. Inference variables, template
//! parameters, and free recursion variables are not admissible assumptions.

use brix_canon::{CanonError, CanonReader};
use brix_kernel::KERNEL_PROFILE_V1;

use brix_semantic::{
    validate_context_model_v2, AssumptionValidatorV2, CheckedContextV2, ContextDimensionsV2,
    ContextId, ContextResolverV2, ModelError, ModelLimits, TypedAssumptionV2,
};

use crate::type_realization::{self, Ty, TyCtx, TypeError};

/// Schema tag used for canonical native type assumptions.
pub const TYPE_ASSUMPTION_SCHEMA_V1: &str = "brix.type.ty@1";

/// Resource limits for untrusted canonical type bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TyDecodeLimits {
    /// Maximum input byte length.
    pub max_bytes: usize,
    /// Maximum decoded type nodes.
    pub max_nodes: usize,
    /// Maximum recursive nesting depth.
    pub max_depth: usize,
    /// Maximum entries in any record or sum.
    pub max_members: usize,
}

const HARD_MAX_TY_BYTES: usize = 1 << 20;
const HARD_MAX_TY_NODES: usize = 16_384;
const HARD_MAX_TY_DEPTH: usize = 128;
const HARD_MAX_TY_MEMBERS: usize = 4_096;
const HARD_MAX_INFERENCE_STEPS: u64 = 16_384;
const HARD_MAX_INFERENCE_DEPTH: u64 = 128;

impl Default for TyDecodeLimits {
    fn default() -> Self {
        Self {
            max_bytes: HARD_MAX_TY_BYTES,
            max_nodes: HARD_MAX_TY_NODES,
            max_depth: HARD_MAX_TY_DEPTH,
            max_members: HARD_MAX_TY_MEMBERS,
        }
    }
}

/// Why a canonical type payload could not become a closed `Ty`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TyDecodeError {
    /// The canonical framing was malformed, truncated, nonminimal, or had a tail.
    Canon(CanonError),
    /// The requested byte, node, depth, or collection budget was exceeded.
    LimitExceeded,
    /// The payload contained an unsupported type ordinal.
    UnknownType(u64),
    /// The type contains a non-closed inference variable or template parameter.
    OpenType,
    /// A recursive variable has no enclosing binder.
    UnboundRecursionVariable,
    /// Recursive binding has no intervening type constructor, so unfolding cannot progress.
    UnguardedRecursion,
    /// A constant is not one of the native regime's frozen scalar constants.
    UnknownConstant(String),
    /// A record, sum, or recursive binder has an empty or duplicate name.
    InvalidName,
    /// Record field names were not strictly ordered by canonical UTF-8 bytes.
    UnsortedRecord,
}

/// Decode one complete canonical `Ty`, accepting only bounded closed types.
pub fn decode_closed_ty(bytes: &[u8], limits: TyDecodeLimits) -> Result<Ty, TyDecodeError> {
    let limits = TyDecodeLimits {
        max_bytes: limits.max_bytes.min(HARD_MAX_TY_BYTES),
        max_nodes: limits.max_nodes.min(HARD_MAX_TY_NODES),
        max_depth: limits.max_depth.min(HARD_MAX_TY_DEPTH),
        max_members: limits.max_members.min(HARD_MAX_TY_MEMBERS),
    };
    if bytes.len() > limits.max_bytes {
        return Err(TyDecodeError::LimitExceeded);
    }
    let mut decoder = Decoder {
        reader: CanonReader::new(bytes),
        limits,
        nodes: 0,
        binders: Vec::new(),
    };
    let ty = decoder.ty(0)?;
    if !decoder.reader.is_empty() {
        return Err(TyDecodeError::Canon(CanonError::BadLength));
    }
    Ok(ty)
}

struct Decoder<'a> {
    reader: CanonReader<'a>,
    limits: TyDecodeLimits,
    nodes: usize,
    binders: Vec<(String, bool)>,
}

impl Decoder<'_> {
    fn ty(&mut self, depth: usize) -> Result<Ty, TyDecodeError> {
        if depth > self.limits.max_depth {
            return Err(TyDecodeError::LimitExceeded);
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or(TyDecodeError::LimitExceeded)?;
        if self.nodes > self.limits.max_nodes {
            return Err(TyDecodeError::LimitExceeded);
        }
        let tag = self.reader.read_uint().map_err(TyDecodeError::Canon)?;
        if matches!(tag, 1 | 3 | 4 | 8) {
            for (_, guarded) in &mut self.binders {
                *guarded = true;
            }
        }
        match tag {
            0 => {
                let name = self.string()?;
                let con = match name.as_str() {
                    "Nat" => "Nat",
                    "Int" => "Int",
                    "Rat" => "Rat",
                    "Real" => "Real",
                    "Float" => "Float",
                    "Complex" => "Complex",
                    "Str" => "Str",
                    "Bool" => "Bool",
                    _ => return Err(TyDecodeError::UnknownConstant(name)),
                };
                Ok(Ty::Con(con))
            }
            1 => Ok(Ty::Fn(
                Box::new(self.ty(depth + 1)?),
                Box::new(self.ty(depth + 1)?),
            )),
            2 | 7 => Err(TyDecodeError::OpenType),
            3 => {
                let count = self.count()?;
                let mut fields = Vec::with_capacity(count);
                let mut previous: Option<String> = None;
                for _ in 0..count {
                    let name = self.string()?;
                    if name.is_empty() {
                        return Err(TyDecodeError::InvalidName);
                    }
                    if previous
                        .as_ref()
                        .is_some_and(|p| p.as_bytes() >= name.as_bytes())
                    {
                        return Err(TyDecodeError::UnsortedRecord);
                    }
                    previous = Some(name.clone());
                    fields.push((name, self.ty(depth + 1)?));
                }
                Ok(Ty::Record(fields))
            }
            4 => {
                let name = self.string()?;
                if name.is_empty() {
                    return Err(TyDecodeError::InvalidName);
                }
                let count = self.count()?;
                let mut variants = Vec::with_capacity(count);
                let mut seen = std::collections::BTreeSet::new();
                for _ in 0..count {
                    let variant = self.string()?;
                    if variant.is_empty() || !seen.insert(variant.clone()) {
                        return Err(TyDecodeError::InvalidName);
                    }
                    let fields = self.count()?;
                    let mut tys = Vec::with_capacity(fields);
                    for _ in 0..fields {
                        tys.push(self.ty(depth + 1)?);
                    }
                    variants.push((variant, tys));
                }
                Ok(Ty::Sum(name, variants))
            }
            5 => {
                let name = self.string()?;
                if name.is_empty() {
                    return Err(TyDecodeError::InvalidName);
                }
                self.binders.push((name.clone(), false));
                let body = self.ty(depth + 1);
                self.binders.pop();
                Ok(Ty::Rec(name, Box::new(body?)))
            }
            6 => {
                let name = self.string()?;
                match self.binders.iter().rev().find(|(bound, _)| bound == &name) {
                    Some((_, true)) => Ok(Ty::RecVar(name)),
                    Some((_, false)) => Err(TyDecodeError::UnguardedRecursion),
                    None => Err(TyDecodeError::UnboundRecursionVariable),
                }
            }
            8 => Ok(Ty::Prod(
                Box::new(self.ty(depth + 1)?),
                Box::new(self.ty(depth + 1)?),
            )),
            other => Err(TyDecodeError::UnknownType(other)),
        }
    }

    fn count(&mut self) -> Result<usize, TyDecodeError> {
        let count = usize::try_from(self.reader.read_uint().map_err(TyDecodeError::Canon)?)
            .map_err(|_| TyDecodeError::LimitExceeded)?;
        if count > self.limits.max_members {
            return Err(TyDecodeError::LimitExceeded);
        }
        Ok(count)
    }

    fn string(&mut self) -> Result<String, TyDecodeError> {
        String::from_utf8(
            self.reader
                .read_bytes()
                .map_err(TyDecodeError::Canon)?
                .to_vec(),
        )
        .map_err(|_| TyDecodeError::Canon(CanonError::BadLength))
    }
}

/// Validator for closed native type assumptions in a V2 semantic context.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClosedTyAssumptionValidator;

impl AssumptionValidatorV2 for ClosedTyAssumptionValidator {
    fn validate(&self, schema: &str, value: &[u8], budget: &mut usize) -> bool {
        if schema != TYPE_ASSUMPTION_SCHEMA_V1 || value.len() > *budget {
            return false;
        }
        let limits = TyDecodeLimits {
            max_bytes: value.len(),
            ..TyDecodeLimits::default()
        };
        if decode_closed_ty(value, limits).is_err() {
            return false;
        }
        *budget -= value.len();
        true
    }
}

/// Errors returned by checked-context type inference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextTypeError {
    /// The semantic context failed identity, dimensions, artifact, or scope validation.
    Context(ModelError),
    /// The context names a kernel, typing, or axiom profile not implemented here.
    UnsupportedProfile,
    /// The expression exceeds the checked context's step or depth budget.
    InferenceBudgetExceeded,
    /// A validated assumption could not be materialized as a closed native type.
    InvalidAssumption(String),
    /// Native type inference or audit failed.
    Type(TypeError),
}

/// Validate the semantic envelope and materialize a `TyCtx` with the envelope's exact identity.
pub fn ty_context_from_model_v2<R: ContextResolverV2>(
    bytes: &[u8],
    expected_id: ContextId,
    expected_dimensions: &ContextDimensionsV2,
    limits: ModelLimits,
    resolver: &R,
) -> Result<TyCtx, ContextTypeError> {
    check_supported_profiles(expected_dimensions)?;
    let checked = validate_context_model_v2(
        bytes,
        expected_id,
        expected_dimensions,
        limits,
        resolver,
        &ClosedTyAssumptionValidator,
    )
    .map_err(ContextTypeError::Context)?;
    ty_context_from_checked_v2(&checked)
}

/// Materialize an already-checked context, preserving its identity and shadowing rules.
pub fn ty_context_from_checked_v2(checked: &CheckedContextV2) -> Result<TyCtx, ContextTypeError> {
    check_supported_profiles(checked.model().dimensions())?;
    let mut bindings = std::collections::BTreeMap::new();
    for (name, assumption) in checked.assumptions_in_scope() {
        if assumption.schema != TYPE_ASSUMPTION_SCHEMA_V1 {
            return Err(ContextTypeError::InvalidAssumption(name));
        }
        let ty = decode_closed_ty(&assumption.canonical_value, TyDecodeLimits::default())
            .map_err(|_| ContextTypeError::InvalidAssumption(name.clone()))?;
        bindings.insert(name, ty);
    }
    Ok(TyCtx::from_validated(bindings, checked.id()))
}

fn check_supported_profiles(dimensions: &ContextDimensionsV2) -> Result<(), ContextTypeError> {
    if dimensions.regime_profile != TYPE_REGIME_PROFILE_V1
        || dimensions.kernel_profile != KERNEL_PROFILE_V1
        || dimensions.axiom_profile != NO_AXIOM_PROFILE_V1
    {
        return Err(ContextTypeError::UnsupportedProfile);
    }
    Ok(())
}

/// Run native inference using a semantically checked V2 context.
pub fn infer_tree_in_context_v2<R: ContextResolverV2>(
    expr: &type_realization::Expr,
    bytes: &[u8],
    expected_id: ContextId,
    expected_dimensions: &ContextDimensionsV2,
    limits: ModelLimits,
    resolver: &R,
    infer: type_realization::Infer,
) -> Result<(Ty, type_realization::TyTree, type_realization::Infer), ContextTypeError> {
    let ctx = ty_context_from_model_v2(bytes, expected_id, expected_dimensions, limits, resolver)?;
    check_expression_budget(
        expr,
        expected_dimensions.step_budget,
        expected_dimensions.depth_budget,
    )?;
    type_realization::infer_tree(expr, &ctx, infer).map_err(ContextTypeError::Type)
}

/// Infer and audit a derivation using a semantically checked V2 context.
pub fn audited_type_check_tree_in_context_v2<R: ContextResolverV2>(
    expr: &type_realization::Expr,
    bytes: &[u8],
    expected_id: ContextId,
    expected_dimensions: &ContextDimensionsV2,
    limits: ModelLimits,
    resolver: &R,
) -> Result<(brix_semantic::Judgement, brix_semantic::TreeDerivation), ContextTypeError> {
    let ctx = ty_context_from_model_v2(bytes, expected_id, expected_dimensions, limits, resolver)?;
    check_expression_budget(
        expr,
        expected_dimensions.step_budget,
        expected_dimensions.depth_budget,
    )?;
    type_realization::audited_type_check_tree(expr, &ctx).map_err(ContextTypeError::Type)
}

/// Supported native typing profile for this regime.
pub const TYPE_REGIME_PROFILE_V1: &str = "brix.type@1";
/// The native typing regime does not assert extra axioms.
pub const NO_AXIOM_PROFILE_V1: &str = "none";

fn push_expr<'a>(
    child: &'a type_realization::Expr,
    parent_depth: u64,
    pending: &mut Vec<(&'a type_realization::Expr, u64)>,
    visited: u64,
    steps: u64,
) -> Result<(), ContextTypeError> {
    let next = parent_depth
        .checked_add(1)
        .ok_or(ContextTypeError::InferenceBudgetExceeded)?;
    if visited
        .checked_add(pending.len() as u64)
        .and_then(|n| n.checked_add(1))
        .ok_or(ContextTypeError::InferenceBudgetExceeded)?
        > steps
    {
        return Err(ContextTypeError::InferenceBudgetExceeded);
    }
    pending.push((child, next));
    Ok(())
}

fn check_expression_budget(
    expr: &type_realization::Expr,
    steps: u64,
    max_depth: u64,
) -> Result<(), ContextTypeError> {
    let steps = steps.min(HARD_MAX_INFERENCE_STEPS);
    let max_depth = max_depth.min(HARD_MAX_INFERENCE_DEPTH);
    let mut pending = vec![(expr, 1_u64)];
    let mut visited = 0_u64;
    while let Some((node, depth)) = pending.pop() {
        if depth > max_depth {
            return Err(ContextTypeError::InferenceBudgetExceeded);
        }
        visited = visited
            .checked_add(1)
            .ok_or(ContextTypeError::InferenceBudgetExceeded)?;
        if visited > steps {
            return Err(ContextTypeError::InferenceBudgetExceeded);
        }
        match node {
            type_realization::Expr::App(a, b)
            | type_realization::Expr::Arith(_, a, b)
            | type_realization::Expr::Cmp(_, a, b)
            | type_realization::Expr::Then(a, b)
            | type_realization::Expr::And(a, b) => {
                push_expr(a, depth, &mut pending, visited, steps)?;
                push_expr(b, depth, &mut pending, visited, steps)?;
            }
            type_realization::Expr::Lam(_, body)
            | type_realization::Expr::LamAnn(_, _, body)
            | type_realization::Expr::Fix(_, _, body)
            | type_realization::Expr::Field(body, _) => {
                push_expr(body, depth, &mut pending, visited, steps)?
            }
            type_realization::Expr::Record(fields) => {
                if visited
                    .checked_add(pending.len() as u64)
                    .and_then(|n| n.checked_add(fields.len() as u64))
                    .ok_or(ContextTypeError::InferenceBudgetExceeded)?
                    > steps
                {
                    return Err(ContextTypeError::InferenceBudgetExceeded);
                }
                for (_, child) in fields {
                    push_expr(child, depth, &mut pending, visited, steps)?;
                }
            }
            type_realization::Expr::Ctor(_, _, args) => {
                if visited
                    .checked_add(pending.len() as u64)
                    .and_then(|n| n.checked_add(args.len() as u64))
                    .ok_or(ContextTypeError::InferenceBudgetExceeded)?
                    > steps
                {
                    return Err(ContextTypeError::InferenceBudgetExceeded);
                }
                for child in args {
                    push_expr(child, depth, &mut pending, visited, steps)?;
                }
            }
            type_realization::Expr::Match(scrutinee, arms) => {
                let width = (arms.len() as u64)
                    .checked_add(1)
                    .ok_or(ContextTypeError::InferenceBudgetExceeded)?;
                if visited
                    .checked_add(pending.len() as u64)
                    .and_then(|n| n.checked_add(width))
                    .ok_or(ContextTypeError::InferenceBudgetExceeded)?
                    > steps
                {
                    return Err(ContextTypeError::InferenceBudgetExceeded);
                }
                push_expr(scrutinee, depth, &mut pending, visited, steps)?;
                for (_, child) in arms {
                    push_expr(child, depth, &mut pending, visited, steps)?;
                }
            }
            type_realization::Expr::Lit(_)
            | type_realization::Expr::Var(_)
            | type_realization::Expr::StrLit(_)
            | type_realization::Expr::FloatLit(_)
            | type_realization::Expr::BoolLit(_) => {}
        }
    }
    Ok(())
}

/// Encode a closed type using the existing canonical type encoding.
pub fn encode_closed_ty(ty: &Ty) -> Vec<u8> {
    use brix_canon::Canonical;
    let mut writer = brix_canon::CanonWriter::new();
    ty.canon_write(&mut writer);
    writer.finish()
}

/// Build a semantic typed assumption from a closed type.
pub fn typed_assumption(name: impl Into<String>, ty: &Ty) -> TypedAssumptionV2 {
    TypedAssumptionV2 {
        name: name.into(),
        schema: TYPE_ASSUMPTION_SCHEMA_V1.into(),
        canonical_value: encode_closed_ty(ty),
    }
}
