//! `brix-kernel` — the dependent proof kernel for BrixMS (ADR-0003 Profile 1).
//!
//! Evaluates explicit proof terms against propositions in an assumption context
//! to produce an authoritative [`Verdict`].

mod certificate;
mod check;
mod prim_registry;
mod prim_schema;
mod term;
mod verdict;

pub use certificate::{
    certificate_id_v1, decode_material_v1, encode_material_v1, native_verifier,
    validate_material_v1, CertificateFormatError, CertificateMaterialV1, DecodedMaterialV1,
    CERTIFICATE_FORMAT_V1, CERTIFICATE_MARKER, KERNEL_PROFILE_V1, NATIVE_VERIFIER_NAME,
};
pub use check::{acceptance, Budget};
pub use prim_registry::{
    resolve as resolve_primitive_relation, typing_arith_v1, typing_arith_v2, JudgmentKind,
    PrimitiveRelation, PrimitiveRelationId, Row, PRIMITIVE_RELATION_MARKER_V1,
    PRIMITIVE_RELATION_VERSION_V1,
};
pub use prim_schema::{
    arith_typing_input_schema_id, numeric_result_type_schema_id, ArithOperatorV1,
    ArithTypingInputV1, CoercionEdgeV1, CoercionKind, NumericResultTypeV1, NumericTypeNameV1,
    SchemaId, ARITH_TYPING_INPUT_MARKER_V1, ARITH_TYPING_INPUT_VERSION_V1,
    NUMERIC_RESULT_TYPE_MARKER_V1, NUMERIC_RESULT_TYPE_VERSION_V1,
};
pub use term::{instantiate, ExplicitTerm, ObjectTerm, Prop, TermKind, Var};
pub use verdict::{
    Certificate, RejectionReason, ResourceBudgetReason, UnsupportedConstruct, Verdict,
};
