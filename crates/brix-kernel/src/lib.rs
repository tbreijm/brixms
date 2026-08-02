//! `brix-kernel` — the dependent proof kernel for BrixMS (ADR-0003 Profile 1).
//!
//! Evaluates explicit proof terms against propositions in an assumption context
//! to produce an authoritative [`Verdict`].

mod certificate;
mod check;
mod term;
mod verdict;

pub use certificate::{
    certificate_id_v1, decode_material_v1, encode_material_v1, native_verifier,
    validate_material_v1, CertificateFormatError, CertificateMaterialV1, DecodedMaterialV1,
    CERTIFICATE_FORMAT_V1, CERTIFICATE_MARKER, KERNEL_PROFILE_V1, NATIVE_VERIFIER_NAME,
};
pub use check::{acceptance, Budget};
pub use term::{instantiate, ExplicitTerm, ObjectTerm, Prop, TermKind, Var};
pub use verdict::{
    Certificate, RejectionReason, ResourceBudgetReason, UnsupportedConstruct, Verdict,
};
