//! Canonical native proof-certificate envelope, format version 1 (ADR-0013).
//!
//! A native accepted certificate's identity is the `Value`-domain digest of a
//! **pinned canonical preimage** over exactly the artifacts the acceptance was
//! requested for. Before ADR-0013 this payload was `format!("{context:?}:…")`,
//! which made a durable theorem identity depend on Rust `Debug` output — audit
//! finding C-1 (`docs/audit/issue-63/README.md`), and the reason SOC-LAW-01 and
//! SOC-LAW-12 were held at `Partial`.
//!
//! # The v1 preimage
//!
//! Written in exactly this order; the order, the marker, the version number,
//! and the profile string are frozen ABI (ADR-0013 §7):
//!
//! 1. [`CanonWriter::write_bytes`] of [`CERTIFICATE_MARKER`];
//! 2. [`CanonWriter::write_uint`] of [`CERTIFICATE_FORMAT_V1`];
//! 3. [`CanonWriter::write_str`] of [`KERNEL_PROFILE_V1`];
//! 4. canonical [`native_verifier`];
//! 5. canonical requested [`ContextId`];
//! 6. `write_bytes` **around** the canonical bytes of the requested [`Prop`];
//! 7. `write_bytes` **around** the canonical bytes of the accepted
//!    [`ExplicitTerm`].
//!
//! Fields 6 and 7 are length-framed deliberately: an envelope reader can then
//! reject truncation, misalignment, and trailing bytes without a general
//! recursive proof-term decoder (that stays out of scope, tracked by #56).
//!
//! The resource [`crate::Budget`] is deliberately **excluded**. Two successful
//! checks of the same verifier/profile/context/proposition/term under different
//! sufficient budgets identify the same proof certificate — identity is a
//! property of the artifacts, not of the effort spent checking them.

use brix_canon::{CanonError, CanonReader, CanonWriter, Canonical, Digest};
use brix_semantic::{CertificateId, ContextId, VerifierId};

use crate::term::{ExplicitTerm, Prop};

/// Marker bytes opening every native certificate preimage. Frozen ABI.
pub const CERTIFICATE_MARKER: &[u8] = b"brix.kernel.certificate";

/// Envelope format version. Frozen; a different layout takes a new number and
/// never edits this one (ADR-0013 §7).
pub const CERTIFICATE_FORMAT_V1: u64 = 1;

/// The calculus profile the accepting kernel implements — ADR-0003 as extended
/// by Profiles 1.1 (ADR-0004) and 1.2 (ADR-0006). Frozen for v1.
pub const KERNEL_PROFILE_V1: &str = "brix.kernel.profile@1.2";

/// Canonical name of the native proof verifier (ADR-0003 §6).
pub const NATIVE_VERIFIER_NAME: &str = "brix.kernel@0.1";

/// The native verifier's identity. The single source of truth for
/// [`NATIVE_VERIFIER_NAME`] — call this instead of repeating the literal.
pub fn native_verifier() -> VerifierId {
    VerifierId::named(NATIVE_VERIFIER_NAME)
}

/// The exact artifacts a v1 certificate is bound to.
///
/// Borrowed rather than owned: the encoder runs on the kernel's acceptance path
/// and must not clone proof terms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CertificateMaterialV1<'a> {
    /// The context acceptance was requested in.
    pub context: &'a ContextId,
    /// The proposition proved.
    pub proposition: &'a Prop,
    /// The accepted explicit proof term.
    pub term: &'a ExplicitTerm,
}

impl<'a> CertificateMaterialV1<'a> {
    /// Bind material to a requested context, proposition, and accepted term.
    ///
    /// Callers are expected to have already established
    /// `term.context == *context` (as [`crate::acceptance`] does before any
    /// checking begins). The envelope *reader* re-establishes it independently
    /// for bytes of unknown provenance — see [`decode_material_v1`].
    pub fn new(context: &'a ContextId, proposition: &'a Prop, term: &'a ExplicitTerm) -> Self {
        Self {
            context,
            proposition,
            term,
        }
    }
}

/// Structurally valid v1 envelope fields, as read back from bytes.
///
/// The proposition and term stay **opaque length-framed payloads**: v1 frames
/// each with `write_bytes`, and this issue deliberately adds no recursive
/// `Prop`/`TermKind` reader. Callers compare these against the canonical bytes
/// of typed artifacts they already hold — see [`validate_material_v1`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecodedMaterialV1<'a> {
    /// The verifier named by the envelope, already checked equal to
    /// [`native_verifier`].
    pub verifier: VerifierId,
    /// The context named by the envelope, already checked equal to the context
    /// the term embeds.
    pub context: ContextId,
    /// Opaque canonical [`Prop`] payload.
    pub proposition_bytes: &'a [u8],
    /// Opaque canonical [`ExplicitTerm`] payload.
    pub term_bytes: &'a [u8],
}

/// Why v1 envelope bytes were rejected.
///
/// Every variant is fail-closed: no [`crate::Certificate`],
/// `Evidence::KernelCertificate`, `Outcome::Proven`, or `Outcome::Refuted` may
/// be constructed from bytes that produced one (SOC-LAW-01, SOC-LAW-12).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CertificateFormatError {
    /// The leading marker is not [`CERTIFICATE_MARKER`].
    BadMarker,
    /// The version field names a format this build does not implement. Unknown
    /// versions are rejected outright, never best-effort parsed.
    UnknownVersion(u64),
    /// The profile field is not [`KERNEL_PROFILE_V1`].
    UnknownProfile,
    /// The verifier field is not [`native_verifier`].
    VerifierMismatch,
    /// The context field differs from the expected context.
    ContextMismatch,
    /// The proposition payload differs from the expected proposition's
    /// canonical bytes.
    PropositionMismatch,
    /// The term payload differs from the expected term's canonical bytes.
    TermMismatch,
    /// The context the term embeds differs from the envelope's context field.
    TermContextMismatch,
    /// Input ended in the middle of a field.
    Truncated,
    /// A length prefix ran past the end of the buffer, or a digest field was
    /// not exactly 32 bytes.
    BadLength,
    /// A magnitude carried a non-minimal leading zero, so the value had more
    /// than one encoding.
    NonMinimalInt,
    /// Bytes remained after the last field.
    TrailingBytes,
}

impl From<CanonError> for CertificateFormatError {
    fn from(err: CanonError) -> Self {
        match err {
            CanonError::UnexpectedEof => CertificateFormatError::Truncated,
            CanonError::NonMinimalInt => CertificateFormatError::NonMinimalInt,
            CanonError::BadLength => CertificateFormatError::BadLength,
        }
    }
}

/// Encode the pinned v1 certificate preimage. The field order is frozen ABI.
///
/// Field 4 always writes [`native_verifier`], never a caller-supplied verifier:
/// this encoder is structurally incapable of minting a foreign-verifier
/// identity.
pub fn encode_material_v1(material: &CertificateMaterialV1<'_>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(CERTIFICATE_MARKER);
    w.write_uint(CERTIFICATE_FORMAT_V1);
    w.write_str(KERNEL_PROFILE_V1);
    native_verifier().canon_write(&mut w);
    material.context.canon_write(&mut w);
    w.write_bytes(&material.proposition.canon_bytes());
    w.write_bytes(&material.term.canon_bytes());
    w.finish()
}

/// The v1 certificate identity: the `Value`-domain digest of the pinned
/// preimage produced by [`encode_material_v1`].
pub fn certificate_id_v1(material: &CertificateMaterialV1<'_>) -> CertificateId {
    CertificateId::from_canon(&encode_material_v1(material))
}

/// Read one length-framed 32-byte digest field.
fn read_digest(r: &mut CanonReader<'_>) -> Result<Digest, CertificateFormatError> {
    let bytes = r.read_bytes()?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CertificateFormatError::BadLength)?;
    Ok(Digest::from_bytes(array))
}

/// Structurally decode and self-validate v1 envelope bytes.
///
/// Accepts only the exact marker, version, profile, and native verifier;
/// requires the context the term embeds to equal the envelope's context field;
/// and rejects trailing bytes. It does **not** bind the payloads to any
/// particular typed proposition or term — [`validate_material_v1`] does that.
///
/// The [`ContextId`] and [`VerifierId`] handed back are reconstructed from
/// canon-framed digest bytes. They are identities to *compare*, never authority
/// to act on (ADR-0013 §6).
pub fn decode_material_v1(bytes: &[u8]) -> Result<DecodedMaterialV1<'_>, CertificateFormatError> {
    let mut r = CanonReader::new(bytes);

    if r.read_bytes()? != CERTIFICATE_MARKER {
        return Err(CertificateFormatError::BadMarker);
    }
    let version = r.read_uint()?;
    if version != CERTIFICATE_FORMAT_V1 {
        return Err(CertificateFormatError::UnknownVersion(version));
    }
    if r.read_bytes()? != KERNEL_PROFILE_V1.as_bytes() {
        return Err(CertificateFormatError::UnknownProfile);
    }

    let verifier = VerifierId(read_digest(&mut r)?);
    if verifier != native_verifier() {
        return Err(CertificateFormatError::VerifierMismatch);
    }

    let context = ContextId(read_digest(&mut r)?);
    let proposition_bytes = r.read_bytes()?;
    let term_bytes = r.read_bytes()?;

    if !r.is_empty() {
        return Err(CertificateFormatError::TrailingBytes);
    }

    // `ExplicitTerm` canon-writes its embedded context first, as a length-framed
    // digest, so the term's own claim about its context is readable without
    // decoding the proof tree at all.
    let embedded = ContextId(read_digest(&mut CanonReader::new(term_bytes))?);
    if embedded != context {
        return Err(CertificateFormatError::TermContextMismatch);
    }

    Ok(DecodedMaterialV1 {
        verifier,
        context,
        proposition_bytes,
        term_bytes,
    })
}

/// Decode `bytes`, then require every field to match `expected`.
///
/// Returns the certificate identity only on an exact match, so a caller can
/// never mint evidence from an envelope that does not describe the artifacts it
/// holds.
pub fn validate_material_v1(
    bytes: &[u8],
    expected: &CertificateMaterialV1<'_>,
) -> Result<CertificateId, CertificateFormatError> {
    let decoded = decode_material_v1(bytes)?;

    if decoded.context != *expected.context {
        return Err(CertificateFormatError::ContextMismatch);
    }
    if decoded.proposition_bytes != expected.proposition.canon_bytes() {
        return Err(CertificateFormatError::PropositionMismatch);
    }
    if decoded.term_bytes != expected.term.canon_bytes() {
        return Err(CertificateFormatError::TermMismatch);
    }

    Ok(CertificateId::from_canon(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_semantic::PropositionId;

    use crate::term::{TermKind, Var};

    fn sample_context_a() -> ContextId {
        ContextId::from_canon(b"context_a")
    }

    fn sample_context_b() -> ContextId {
        ContextId::from_canon(b"context_b")
    }

    fn sample_prop_p() -> Prop {
        Prop::Atom(PropositionId::from_canon(b"P"))
    }

    fn sample_prop_q() -> Prop {
        Prop::Atom(PropositionId::from_canon(b"Q"))
    }

    /// `\x. x`
    fn sample_term(ctx: ContextId) -> ExplicitTerm {
        ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("x".into()),
                body: Box::new(TermKind::Hyp(Var::Index(0))),
            },
        )
    }

    /// `\y. y` — structurally distinct from [`sample_term`] (different bound
    /// name) but canonically identical in de Bruijn shape; used where a
    /// *different* term artifact is wanted.
    fn sample_term_alt(ctx: ContextId) -> ExplicitTerm {
        ExplicitTerm::new(
            ctx,
            TermKind::Lam {
                var_name: Some("y".into()),
                body: Box::new(TermKind::Inl(Box::new(TermKind::Hyp(Var::Index(0))))),
            },
        )
    }

    /// Hand-assemble a v1 envelope field-by-field, independent of
    /// [`encode_material_v1`], so malformed-envelope tests do not depend on the
    /// encoder they are exercising.
    #[allow(clippy::too_many_arguments)]
    fn build_envelope(
        marker: &[u8],
        version: u64,
        profile: &[u8],
        verifier_digest: [u8; 32],
        context_digest: [u8; 32],
        prop_bytes: &[u8],
        term_bytes: &[u8],
    ) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.write_bytes(marker);
        w.write_uint(version);
        w.write_bytes(profile);
        w.write_bytes(&verifier_digest);
        w.write_bytes(&context_digest);
        w.write_bytes(prop_bytes);
        w.write_bytes(term_bytes);
        w.finish()
    }

    #[test]
    fn encoded_material_opens_with_the_frozen_marker_version_and_profile() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term);
        let bytes = encode_material_v1(&material);

        let mut r = CanonReader::new(&bytes);
        assert_eq!(r.read_bytes().unwrap(), CERTIFICATE_MARKER);
        assert_eq!(r.read_uint().unwrap(), CERTIFICATE_FORMAT_V1);
        assert_eq!(r.read_bytes().unwrap(), KERNEL_PROFILE_V1.as_bytes());
    }

    #[test]
    fn encoded_material_embeds_the_native_verifier_and_requested_context() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term);
        let bytes = encode_material_v1(&material);

        let mut r = CanonReader::new(&bytes);
        let _marker = r.read_bytes().unwrap();
        let _version = r.read_uint().unwrap();
        let _profile = r.read_bytes().unwrap();

        assert_eq!(r.read_bytes().unwrap(), native_verifier().0.as_bytes());
        assert_eq!(r.read_bytes().unwrap(), ctx.0.as_bytes());
    }

    #[test]
    fn equal_inputs_give_byte_identical_material() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term_1 = sample_term(ctx);
        let term_2 = sample_term(ctx);

        let m1 = CertificateMaterialV1::new(&ctx, &prop, &term_1);
        let m2 = CertificateMaterialV1::new(&ctx, &prop, &term_2);

        assert_eq!(encode_material_v1(&m1), encode_material_v1(&m2));
    }

    #[test]
    fn changing_context_changes_the_certificate_id() {
        let ctx_a = sample_context_a();
        let ctx_b = sample_context_b();
        let prop = sample_prop_p();
        let term_a = sample_term(ctx_a);
        let term_b = sample_term(ctx_b);

        let id_a = certificate_id_v1(&CertificateMaterialV1::new(&ctx_a, &prop, &term_a));
        let id_b = certificate_id_v1(&CertificateMaterialV1::new(&ctx_b, &prop, &term_b));

        assert_ne!(id_a, id_b);
    }

    #[test]
    fn changing_proposition_changes_the_certificate_id() {
        let ctx = sample_context_a();
        let term = sample_term(ctx);

        let id_p = certificate_id_v1(&CertificateMaterialV1::new(&ctx, &sample_prop_p(), &term));
        let id_q = certificate_id_v1(&CertificateMaterialV1::new(&ctx, &sample_prop_q(), &term));

        assert_ne!(id_p, id_q);
    }

    #[test]
    fn changing_term_changes_the_certificate_id() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term_a = sample_term(ctx);
        let term_b = sample_term_alt(ctx);

        let id_a = certificate_id_v1(&CertificateMaterialV1::new(&ctx, &prop, &term_a));
        let id_b = certificate_id_v1(&CertificateMaterialV1::new(&ctx, &prop, &term_b));

        assert_ne!(id_a, id_b);
    }

    #[test]
    fn decode_round_trips_encoder_output() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term);
        let bytes = encode_material_v1(&material);

        let decoded = decode_material_v1(&bytes).unwrap();

        assert_eq!(decoded.verifier, native_verifier());
        assert_eq!(decoded.context, ctx);
        assert_eq!(decoded.proposition_bytes.to_vec(), prop.canon_bytes());
        assert_eq!(decoded.term_bytes.to_vec(), term.canon_bytes());
    }

    #[test]
    fn validate_accepts_matching_material_and_returns_the_id() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term);
        let bytes = encode_material_v1(&material);

        let id = validate_material_v1(&bytes, &material).unwrap();

        assert_eq!(id, certificate_id_v1(&material));
    }

    #[test]
    fn decode_rejects_bad_marker() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let bytes = build_envelope(
            b"not.the.marker",
            CERTIFICATE_FORMAT_V1,
            KERNEL_PROFILE_V1.as_bytes(),
            *native_verifier().0.as_bytes(),
            *ctx.0.as_bytes(),
            &prop.canon_bytes(),
            &term.canon_bytes(),
        );

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::BadMarker)
        );
    }

    #[test]
    fn decode_rejects_unknown_version() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let bytes = build_envelope(
            CERTIFICATE_MARKER,
            999,
            KERNEL_PROFILE_V1.as_bytes(),
            *native_verifier().0.as_bytes(),
            *ctx.0.as_bytes(),
            &prop.canon_bytes(),
            &term.canon_bytes(),
        );

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::UnknownVersion(999))
        );
    }

    #[test]
    fn decode_rejects_unknown_profile() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let bytes = build_envelope(
            CERTIFICATE_MARKER,
            CERTIFICATE_FORMAT_V1,
            b"not.the.profile",
            *native_verifier().0.as_bytes(),
            *ctx.0.as_bytes(),
            &prop.canon_bytes(),
            &term.canon_bytes(),
        );

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::UnknownProfile)
        );
    }

    #[test]
    fn decode_rejects_foreign_verifier() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let foreign = VerifierId::named("foreign.verifier@1");
        let bytes = build_envelope(
            CERTIFICATE_MARKER,
            CERTIFICATE_FORMAT_V1,
            KERNEL_PROFILE_V1.as_bytes(),
            *foreign.0.as_bytes(),
            *ctx.0.as_bytes(),
            &prop.canon_bytes(),
            &term.canon_bytes(),
        );

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::VerifierMismatch)
        );
    }

    #[test]
    fn decode_rejects_term_context_disagreement() {
        let ctx_a = sample_context_a();
        let ctx_b = sample_context_b();
        let prop = sample_prop_p();
        // Envelope's context field claims A, but the embedded term was built
        // against B.
        let term_b = sample_term(ctx_b);
        let bytes = build_envelope(
            CERTIFICATE_MARKER,
            CERTIFICATE_FORMAT_V1,
            KERNEL_PROFILE_V1.as_bytes(),
            *native_verifier().0.as_bytes(),
            *ctx_a.0.as_bytes(),
            &prop.canon_bytes(),
            &term_b.canon_bytes(),
        );

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::TermContextMismatch)
        );
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term);
        let mut bytes = encode_material_v1(&material);
        bytes.push(0);

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::TrailingBytes)
        );
    }

    #[test]
    fn decode_rejects_every_truncated_prefix() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term);
        let bytes = encode_material_v1(&material);

        for n in 0..bytes.len() {
            assert!(
                decode_material_v1(&bytes[..n]).is_err(),
                "prefix of length {n} should be rejected"
            );
        }
    }

    #[test]
    fn decode_rejects_length_prefix_past_end() {
        let ctx = sample_context_a();

        let mut w = CanonWriter::new();
        w.write_bytes(CERTIFICATE_MARKER);
        w.write_uint(CERTIFICATE_FORMAT_V1);
        w.write_str(KERNEL_PROFILE_V1);
        w.write_bytes(native_verifier().0.as_bytes());
        w.write_bytes(ctx.0.as_bytes());
        // A length prefix with no payload bytes following it.
        w.write_uint(u64::MAX);
        let bytes = w.finish();

        assert_eq!(
            decode_material_v1(&bytes),
            Err(CertificateFormatError::BadLength)
        );
    }

    #[test]
    fn validate_rejects_context_mismatch() {
        let ctx_a = sample_context_a();
        let ctx_b = sample_context_b();
        let prop = sample_prop_p();
        let term_a = sample_term(ctx_a);
        let material_a = CertificateMaterialV1::new(&ctx_a, &prop, &term_a);
        let bytes = encode_material_v1(&material_a);

        let term_b = sample_term(ctx_b);
        let expected = CertificateMaterialV1::new(&ctx_b, &prop, &term_b);

        assert_eq!(
            validate_material_v1(&bytes, &expected),
            Err(CertificateFormatError::ContextMismatch)
        );
    }

    #[test]
    fn validate_rejects_proposition_mismatch() {
        let ctx = sample_context_a();
        let term = sample_term(ctx);
        let prop_p = sample_prop_p();
        let prop_q = sample_prop_q();
        let material = CertificateMaterialV1::new(&ctx, &prop_p, &term);
        let bytes = encode_material_v1(&material);

        let expected = CertificateMaterialV1::new(&ctx, &prop_q, &term);

        assert_eq!(
            validate_material_v1(&bytes, &expected),
            Err(CertificateFormatError::PropositionMismatch)
        );
    }

    #[test]
    fn validate_rejects_term_mismatch() {
        let ctx = sample_context_a();
        let prop = sample_prop_p();
        let term_a = sample_term(ctx);
        let material = CertificateMaterialV1::new(&ctx, &prop, &term_a);
        let bytes = encode_material_v1(&material);

        let term_b = sample_term_alt(ctx);
        let expected = CertificateMaterialV1::new(&ctx, &prop, &term_b);

        assert_eq!(
            validate_material_v1(&bytes, &expected),
            Err(CertificateFormatError::TermMismatch)
        );
    }
}
