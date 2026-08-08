//! Saturation certificates: canonical envelopes, fail-closed readers, and the
//! semantic checkers that re-derive a claim rather than trusting it
//! (ADR-0014 §6, Stage B; ⟨D-QCERT⟩ ratified 2026-08-03).
//!
//! # Two certificates, one discipline
//!
//! A [`QuiescenceCertificateV1`] asserts the `1` summand of `F_O`: at one
//! context, observation profile, presentation revision, policy, regime set, and
//! admissibility policy, a finite administrative prefix reaches a world whose
//! admissible frontier is **empty under a complete enumeration**. A
//! [`DivergenceCertificateV1`] asserts `↑_τ`: the administrative orbit closes a
//! lasso and therefore never reaches a realizing step. They are **not** duals
//! of one convenient enum — one is a decided negative, the other is
//! `Unknown`-graded for the completion question (⟨D-DIV⟩), and neither is ever
//! `Refuted`.
//!
//! Both follow ADR-0013's envelope discipline verbatim: a frozen marker, a
//! frozen format version, a frozen profile string, a frozen field order,
//! length-framed fields, a reader that rejects unknown versions and profiles
//! outright rather than best-effort parsing them, and frozen vectors with an
//! independent second construction path (`tests/saturation_vectors.rs`).
//!
//! Unlike ADR-0013's kernel envelope there is no opaque sub-payload here —
//! every field is a digest or a small integer — so there is no separate
//! "material" type holding un-typed bytes: [`decode_quiescence_v1`] hands back
//! the certificate struct itself.
//!
//! # What "checking" means
//!
//! [`check_quiescence_certificate`] does not verify a signature; it **re-runs
//! the claim**. It replays the recorded prefix from a fresh history, relabels
//! every step through the profile, re-enumerates the frontier at the terminal
//! world, and recomputes the judgement identity. A certificate that decodes
//! perfectly but describes a frontier that is not in fact empty is rejected.
//!
//! And what a pass means is deliberately modest. [`CertificateCheck::Verified`]
//! is `Derived`-grade **in the certificate's exact context, profile, and
//! revision** — never a theorem, and never a claim about any other
//! configuration (ADR-0014 §6.1). Everything else is
//! [`CertificateCheck::Unknown`]. There is no third answer, and in particular no
//! `Refuted`: failing to re-derive quiescence is not evidence of a live step.

use brix_canon::{CanonError, CanonReader, CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Evidence, JudgementId, Outcome, PropositionId, Quiescent,
};

use crate::exec::ExecConfig;
use crate::history::History;
use crate::journal::{CommittedStep, Journal};

use super::{
    enumerate_admissible, AssumptionId, ObservationProfileId, PresentationIdV1, PresentationV1,
    ProfileError, StepLabel,
};

/// Marker bytes opening every v1 quiescence envelope. Frozen ABI.
pub const QUIESCENCE_MARKER: &[u8] = b"brix.soc.quiescence";

/// Marker bytes opening every v1 divergence envelope. Frozen ABI.
pub const DIVERGENCE_MARKER: &[u8] = b"brix.soc.divergence";

/// Envelope format version for both certificates. Frozen; a different layout
/// takes a new number and never edits this one (ADR-0014 §6.2).
pub const CERTIFICATE_FORMAT_V1: u64 = 1;

/// The saturation profile these certificates are minted under — the analogue of
/// ADR-0013's kernel-profile string. Frozen for v1.
pub const SATURATION_PROFILE_V1: &str = "brix.soc.saturation@1";

// ---------------------------------------------------------------------------
// Ordinal-carrying field types
// ---------------------------------------------------------------------------

/// Whether the frontier enumeration backing a quiescence claim was exhaustive.
///
/// The load-bearing honesty field of the certificate: "the frontier is empty"
/// is a decided negative **only if** enumeration was complete. That holds in v1
/// solely because `Regime::candidates -> Vec<Candidate>` is unbounded and
/// total. A bounded or fallible regime API requires a v2 certificate and MUST
/// NOT emit v1 (ADR-0014 §6.2, risk 1) — which is why this enum has exactly one
/// variant and the reader accepts exactly one ordinal. Adding a second variant
/// here is not a compatible extension; it is a new format version.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum EnumerationCompleteness {
    /// The whole admissible frontier was enumerated.
    Complete,
}

impl EnumerationCompleteness {
    /// Canonical ABI ordinal. Append-only; never reorder.
    pub const fn ordinal(self) -> u64 {
        match self {
            EnumerationCompleteness::Complete => 0,
        }
    }

    /// The variant an ordinal names, or `None` for an ordinal this build does
    /// not implement.
    pub const fn from_ordinal(n: u64) -> Option<Self> {
        match n {
            0 => Some(EnumerationCompleteness::Complete),
            _ => None,
        }
    }
}

/// Under which declared hypotheses a divergence claim was made.
///
/// A lasso is only a divergence proof if returning to the same
/// [`super::ObservableState`] genuinely means the engine repeats itself — which
/// needs P1 (candidates do not read `history`) and P6 (keying does not read
/// `phase`). Both are *declared* by the presentation and bounded-checked, never
/// proved (ADR-0014 §4.2, risk 2). Recording the mode in the certificate is
/// what stops a reader from mistaking a conditional result for an
/// unconditional one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AssumptionMode {
    /// The presentation declared both P1 ([`AssumptionId::HistoryIndependence`])
    /// and P6 ([`AssumptionId::PhaseStableKeying`]), and the bounded checks at
    /// the revisited state passed. The only mode v1 admits.
    DeclaredP1P6,
}

impl AssumptionMode {
    /// Canonical ABI ordinal. Append-only; never reorder.
    pub const fn ordinal(self) -> u64 {
        match self {
            AssumptionMode::DeclaredP1P6 => 0,
        }
    }

    /// The variant an ordinal names, or `None` for an unimplemented ordinal.
    pub const fn from_ordinal(n: u64) -> Option<Self> {
        match n {
            0 => Some(AssumptionMode::DeclaredP1P6),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------

/// Content-addressed identity of a [`QuiescenceCertificateV1`] — the
/// `Value`-domain digest of its pinned envelope.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct QuiescenceCertificateId(pub Digest);

impl QuiescenceCertificateId {
    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics, vectors).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

/// Content-addressed identity of a [`DivergenceCertificateV1`] — the
/// `Value`-domain digest of its pinned envelope.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DivergenceCertificateId(pub Digest);

impl DivergenceCertificateId {
    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics, vectors).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

// ---------------------------------------------------------------------------
// The certificates
// ---------------------------------------------------------------------------

/// A certified claim that no admissible candidate exists at the terminal world.
///
/// Asserts exactly (ADR-0014 §6.1): *in context `context`, under observation
/// profile `profile`, at presentation revision `presentation`, from `src_world`
/// under `policy`, the recorded finite administrative prefix reaches
/// `terminal_world`, at which the admissible frontier under `regime_set` and
/// `adm_id` is empty under a complete enumeration.* **It asserts nothing about
/// any other context, profile, revision, policy, or regime set.**
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuiescenceCertificateV1 {
    /// The declared observation boundary.
    pub profile: ObservationProfileId,
    /// The exact context.
    pub context: ContextId,
    /// The program/world revision.
    pub presentation: PresentationIdV1,
    /// The policy in force.
    pub policy: ConfigId,
    /// The world saturation started from.
    pub src_world: ConfigId,
    /// The world at which the frontier was found empty.
    pub terminal_world: ConfigId,
    /// The hidden administrative prefix, as committed-step digests in order.
    pub hidden: Vec<Digest>,
    /// The running chain digest after the hidden prefix, replayed from a fresh
    /// [`History`]. For an empty prefix this is [`History::empty`]'s digest —
    /// **not** an absent field, so the encoding stays total and the prefix
    /// length cannot disagree with a presence flag.
    pub prefix_chain: Digest,
    /// The ordered regime-set identity.
    pub regime_set: Digest,
    /// The admissibility-policy identity.
    pub adm_id: Digest,
    /// Whether enumeration was exhaustive.
    pub enumeration: EnumerationCompleteness,
    /// The grade this certificate claims. Always [`Outcome::Derived`]: a
    /// settlement-kernel certificate is never a proof-kernel theorem.
    pub grade: Outcome,
    /// The `Derived` quiescence judgement this certificate publishes —
    /// [`brix_semantic::Quiescent`] over the terminal world, supported by
    /// [`Evidence::SettlementReplay`] of the prefix chain.
    pub judgement: JudgementId,
}

/// A certified administrative divergence: a closed lasso in the administrative
/// orbit, so no realizing step is ever reached from `src_world`.
///
/// Graded `Unknown` for the completion/quiescence question (⟨D-DIV⟩). This is
/// **never** the `1` summand and **never** `Refuted` — it is a positive fact
/// about non-termination, which is a different claim from "there is nothing to
/// do".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DivergenceCertificateV1 {
    /// The declared observation boundary.
    pub profile: ObservationProfileId,
    /// The exact context.
    pub context: ContextId,
    /// The program/world revision.
    pub presentation: PresentationIdV1,
    /// The policy in force.
    pub policy: ConfigId,
    /// The world saturation started from.
    pub src_world: ConfigId,
    /// Administrative steps before the cycle closes.
    pub stem: u64,
    /// Length of the closed administrative cycle, at least 1.
    pub cycle: u64,
    /// The lasso, as committed-step digests in order. Exactly `stem + cycle`
    /// entries — the two counts are the length field, so a lasso that does not
    /// match them cannot be encoded.
    pub lasso: Vec<Digest>,
    /// The world the orbit revisits (the cycle entry point).
    pub cycle_world: ConfigId,
    /// The policy at the cycle entry point. Currently always equal to
    /// [`Self::policy`] — `oracle::apply` carries `policy` through unchanged —
    /// but the projection ⟨D-PROJ⟩ is defined over `(world, policy)`, so the
    /// field is carried and checked. An engine that ever mutates policy would
    /// fail this check rather than silently reuse a v1 certificate.
    pub cycle_policy: ConfigId,
    /// Which declared hypotheses the claim rests on.
    pub assumptions: AssumptionMode,
    /// The grade. Always [`Outcome::Unknown`].
    pub grade: Outcome,
}

// ---------------------------------------------------------------------------
// Envelope errors
// ---------------------------------------------------------------------------

/// Why saturation-certificate bytes were rejected.
///
/// Every variant is fail-closed: no [`CertificateCheck::Verified`], no
/// quiescence judgement, and no `SaturatedStep::Quiescent`/`Divergent` may be
/// constructed from bytes that produced one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CertEnvelopeError {
    /// The leading marker is not the expected one for this certificate kind.
    BadMarker,
    /// The version field names a format this build does not implement. Unknown
    /// versions are rejected outright, never best-effort parsed.
    UnknownVersion(u64),
    /// The saturation-profile field is not [`SATURATION_PROFILE_V1`].
    UnknownSaturationProfile,
    /// The observation-profile field differs from the expected profile.
    ObservationProfileMismatch,
    /// The context field differs from the expected context.
    ContextMismatch,
    /// The presentation field differs from the expected revision.
    PresentationMismatch,
    /// The enumeration-completeness ordinal is not one v1 implements. In v1
    /// only `Complete` exists, so this is how a bounded-enumeration certificate
    /// minted by a future engine is refused rather than downgraded.
    UnknownEnumerationOrdinal(u64),
    /// The assumption-mode ordinal is not one v1 implements.
    UnknownAssumptionOrdinal(u64),
    /// The outcome-grade ordinal is not a known [`Outcome`].
    UnknownOutcomeOrdinal(u64),
    /// A quiescence envelope carried a grade other than [`Outcome::Derived`].
    QuiescenceGradeNotDerived,
    /// A divergence envelope carried a grade other than [`Outcome::Unknown`].
    DivergenceGradeNotUnknown,
    /// A divergence envelope declared a zero-length cycle. A lasso with no
    /// cycle is not a lasso.
    ZeroCycleLength,
    /// `stem + cycle` overflowed, so the declared lasso length is unrepresentable.
    LassoLengthOverflow,
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

impl From<CanonError> for CertEnvelopeError {
    fn from(err: CanonError) -> Self {
        match err {
            CanonError::UnexpectedEof => CertEnvelopeError::Truncated,
            CanonError::NonMinimalInt => CertEnvelopeError::NonMinimalInt,
            CanonError::BadLength => CertEnvelopeError::BadLength,
        }
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Encode the pinned v1 quiescence envelope. The field order is frozen ABI
/// (ADR-0014 §6.2):
///
/// marker, version, saturation profile, observation-profile id, context,
/// presentation revision, policy, source world, terminal world, prefix length
/// `m`, the `m` prefix step digests, the prefix chain digest, regime-set
/// digest, `Adm` identity, enumeration-completeness ordinal, outcome grade,
/// quiescence `JudgementId`.
///
/// **Cost is deliberately excluded**, by ADR-0013 §4's argument: identity is a
/// property of the artifacts, not of the effort spent. Two runs under different
/// sufficient budgets identify the *same* certificate.
pub fn encode_quiescence_v1(cert: &QuiescenceCertificateV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(QUIESCENCE_MARKER);
    w.write_uint(CERTIFICATE_FORMAT_V1);
    w.write_str(SATURATION_PROFILE_V1);
    w.write_bytes(cert.profile.digest().as_bytes());
    w.write_bytes(cert.context.digest().as_bytes());
    w.write_bytes(cert.presentation.digest().as_bytes());
    w.write_bytes(cert.policy.digest().as_bytes());
    w.write_bytes(cert.src_world.digest().as_bytes());
    w.write_bytes(cert.terminal_world.digest().as_bytes());
    w.write_uint(cert.hidden.len() as u64);
    for digest in &cert.hidden {
        w.write_bytes(digest.as_bytes());
    }
    w.write_bytes(cert.prefix_chain.as_bytes());
    w.write_bytes(cert.regime_set.as_bytes());
    w.write_bytes(cert.adm_id.as_bytes());
    w.write_uint(cert.enumeration.ordinal());
    cert.grade.canon_write(&mut w);
    w.write_bytes(cert.judgement.digest().as_bytes());
    w.finish()
}

/// The v1 quiescence-certificate identity: the `Value`-domain digest of the
/// pinned envelope.
pub fn quiescence_certificate_id(cert: &QuiescenceCertificateV1) -> QuiescenceCertificateId {
    QuiescenceCertificateId(Digest::of(Domain::Value, &encode_quiescence_v1(cert)))
}

/// Encode the pinned v1 divergence envelope. The field order is frozen ABI
/// (ADR-0014 §6.2):
///
/// marker, version, saturation profile, observation-profile id, context,
/// presentation revision, policy, source world, stem length, cycle length, the
/// `stem + cycle` lasso step digests, cycle-entry world, cycle-entry policy,
/// assumption-mode ordinal, outcome grade.
///
/// Fields 1–8 are shared with the quiescence envelope; the markers keep the two
/// kinds from ever being confused despite the shared prefix.
///
/// The lasso carries no length field of its own — `stem + cycle` *is* its
/// length, so the counts and the digest list cannot disagree.
pub fn encode_divergence_v1(cert: &DivergenceCertificateV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(DIVERGENCE_MARKER);
    w.write_uint(CERTIFICATE_FORMAT_V1);
    w.write_str(SATURATION_PROFILE_V1);
    w.write_bytes(cert.profile.digest().as_bytes());
    w.write_bytes(cert.context.digest().as_bytes());
    w.write_bytes(cert.presentation.digest().as_bytes());
    w.write_bytes(cert.policy.digest().as_bytes());
    w.write_bytes(cert.src_world.digest().as_bytes());
    w.write_uint(cert.stem);
    w.write_uint(cert.cycle);
    for digest in &cert.lasso {
        w.write_bytes(digest.as_bytes());
    }
    w.write_bytes(cert.cycle_world.digest().as_bytes());
    w.write_bytes(cert.cycle_policy.digest().as_bytes());
    w.write_uint(cert.assumptions.ordinal());
    cert.grade.canon_write(&mut w);
    w.finish()
}

/// The v1 divergence-certificate identity: the `Value`-domain digest of the
/// pinned envelope.
pub fn divergence_certificate_id(cert: &DivergenceCertificateV1) -> DivergenceCertificateId {
    DivergenceCertificateId(Digest::of(Domain::Value, &encode_divergence_v1(cert)))
}

// ---------------------------------------------------------------------------
// Decoding — fail closed
// ---------------------------------------------------------------------------

/// Read one length-framed 32-byte digest field.
fn read_digest(r: &mut CanonReader<'_>) -> Result<Digest, CertEnvelopeError> {
    let bytes = r.read_bytes()?;
    let array: [u8; 32] = bytes.try_into().map_err(|_| CertEnvelopeError::BadLength)?;
    Ok(Digest::from_bytes(array))
}

/// Read the shared opening fields 1–8, having already matched `marker`.
struct Prelude {
    profile: ObservationProfileId,
    context: ContextId,
    presentation: PresentationIdV1,
    policy: ConfigId,
    src_world: ConfigId,
}

fn read_prelude(r: &mut CanonReader<'_>, marker: &[u8]) -> Result<Prelude, CertEnvelopeError> {
    if r.read_bytes()? != marker {
        return Err(CertEnvelopeError::BadMarker);
    }
    let version = r.read_uint()?;
    if version != CERTIFICATE_FORMAT_V1 {
        return Err(CertEnvelopeError::UnknownVersion(version));
    }
    if r.read_bytes()? != SATURATION_PROFILE_V1.as_bytes() {
        return Err(CertEnvelopeError::UnknownSaturationProfile);
    }
    Ok(Prelude {
        profile: ObservationProfileId(read_digest(r)?),
        context: ContextId(read_digest(r)?),
        presentation: PresentationIdV1(read_digest(r)?),
        policy: ConfigId(read_digest(r)?),
        src_world: ConfigId(read_digest(r)?),
    })
}

/// Read a grade written by [`Outcome::canon_write`] (a bare ordinal).
fn read_grade(r: &mut CanonReader<'_>) -> Result<Outcome, CertEnvelopeError> {
    let ordinal = r.read_uint()?;
    // Reproduced rather than imported: `brix-semantic` exposes the ordinal
    // mapping only through `Canonical`, and this reader must reject an
    // out-of-range ordinal instead of saturating to some default.
    match ordinal {
        0 => Ok(Outcome::Proven),
        1 => Ok(Outcome::Refuted),
        2 => Ok(Outcome::Derived),
        3 => Ok(Outcome::Measured),
        4 => Ok(Outcome::Unknown),
        5 => Ok(Outcome::Audited),
        n => Err(CertEnvelopeError::UnknownOutcomeOrdinal(n)),
    }
}

/// Structurally decode v1 quiescence bytes.
///
/// Accepts only the exact marker, version, saturation profile, enumeration
/// ordinal, and `Derived` grade, and rejects trailing bytes. It binds the
/// certificate to **no** expected context, profile, or revision —
/// [`validate_quiescence_v1`] does that, and
/// [`check_quiescence_certificate`] re-derives the claim itself.
pub fn decode_quiescence_v1(bytes: &[u8]) -> Result<QuiescenceCertificateV1, CertEnvelopeError> {
    let mut r = CanonReader::new(bytes);
    let prelude = read_prelude(&mut r, QUIESCENCE_MARKER)?;

    let terminal_world = ConfigId(read_digest(&mut r)?);

    let count = r.read_uint()?;
    // Grown by pushing, never `with_capacity(count)`: a hostile length field
    // must fail on the first missing digest, not allocate first.
    let mut hidden = Vec::new();
    for _ in 0..count {
        hidden.push(read_digest(&mut r)?);
    }

    let prefix_chain = read_digest(&mut r)?;
    let regime_set = read_digest(&mut r)?;
    let adm_id = read_digest(&mut r)?;

    let enumeration_ordinal = r.read_uint()?;
    let enumeration = EnumerationCompleteness::from_ordinal(enumeration_ordinal).ok_or(
        CertEnvelopeError::UnknownEnumerationOrdinal(enumeration_ordinal),
    )?;

    let grade = read_grade(&mut r)?;
    if grade != Outcome::Derived {
        return Err(CertEnvelopeError::QuiescenceGradeNotDerived);
    }

    let judgement = JudgementId(read_digest(&mut r)?);

    if !r.is_empty() {
        return Err(CertEnvelopeError::TrailingBytes);
    }

    Ok(QuiescenceCertificateV1 {
        profile: prelude.profile,
        context: prelude.context,
        presentation: prelude.presentation,
        policy: prelude.policy,
        src_world: prelude.src_world,
        terminal_world,
        hidden,
        prefix_chain,
        regime_set,
        adm_id,
        enumeration,
        grade,
        judgement,
    })
}

/// Structurally decode v1 divergence bytes.
///
/// Accepts only the exact marker, version, saturation profile, assumption-mode
/// ordinal, and `Unknown` grade; requires a cycle length of at least one; and
/// rejects trailing bytes.
pub fn decode_divergence_v1(bytes: &[u8]) -> Result<DivergenceCertificateV1, CertEnvelopeError> {
    let mut r = CanonReader::new(bytes);
    let prelude = read_prelude(&mut r, DIVERGENCE_MARKER)?;

    let stem = r.read_uint()?;
    let cycle = r.read_uint()?;
    if cycle == 0 {
        return Err(CertEnvelopeError::ZeroCycleLength);
    }
    let length = stem
        .checked_add(cycle)
        .ok_or(CertEnvelopeError::LassoLengthOverflow)?;

    let mut lasso = Vec::new();
    for _ in 0..length {
        lasso.push(read_digest(&mut r)?);
    }

    let cycle_world = ConfigId(read_digest(&mut r)?);
    let cycle_policy = ConfigId(read_digest(&mut r)?);

    let assumption_ordinal = r.read_uint()?;
    let assumptions = AssumptionMode::from_ordinal(assumption_ordinal).ok_or(
        CertEnvelopeError::UnknownAssumptionOrdinal(assumption_ordinal),
    )?;

    let grade = read_grade(&mut r)?;
    if grade != Outcome::Unknown {
        return Err(CertEnvelopeError::DivergenceGradeNotUnknown);
    }

    if !r.is_empty() {
        return Err(CertEnvelopeError::TrailingBytes);
    }

    Ok(DivergenceCertificateV1 {
        profile: prelude.profile,
        context: prelude.context,
        presentation: prelude.presentation,
        policy: prelude.policy,
        src_world: prelude.src_world,
        stem,
        cycle,
        lasso,
        cycle_world,
        cycle_policy,
        assumptions,
        grade,
    })
}

/// Decode `bytes`, then require the context, observation profile, and
/// presentation revision to be exactly the expected ones.
///
/// Returns the certificate identity only on an exact match, so a caller can
/// never accept an envelope that describes a *different* boundary than the one
/// it is reasoning about — the failure mode SOC-LAW-10's domain clause exists
/// to prevent.
pub fn validate_quiescence_v1(
    bytes: &[u8],
    expected_context: ContextId,
    expected_profile: ObservationProfileId,
    expected_presentation: PresentationIdV1,
) -> Result<QuiescenceCertificateId, CertEnvelopeError> {
    let cert = decode_quiescence_v1(bytes)?;
    check_binding(
        cert.context,
        cert.profile,
        cert.presentation,
        expected_context,
        expected_profile,
        expected_presentation,
    )?;
    Ok(QuiescenceCertificateId(Digest::of(Domain::Value, bytes)))
}

/// Decode `bytes`, then require the context, observation profile, and
/// presentation revision to be exactly the expected ones.
pub fn validate_divergence_v1(
    bytes: &[u8],
    expected_context: ContextId,
    expected_profile: ObservationProfileId,
    expected_presentation: PresentationIdV1,
) -> Result<DivergenceCertificateId, CertEnvelopeError> {
    let cert = decode_divergence_v1(bytes)?;
    check_binding(
        cert.context,
        cert.profile,
        cert.presentation,
        expected_context,
        expected_profile,
        expected_presentation,
    )?;
    Ok(DivergenceCertificateId(Digest::of(Domain::Value, bytes)))
}

fn check_binding(
    context: ContextId,
    profile: ObservationProfileId,
    presentation: PresentationIdV1,
    expected_context: ContextId,
    expected_profile: ObservationProfileId,
    expected_presentation: PresentationIdV1,
) -> Result<(), CertEnvelopeError> {
    if context != expected_context {
        return Err(CertEnvelopeError::ContextMismatch);
    }
    if profile != expected_profile {
        return Err(CertEnvelopeError::ObservationProfileMismatch);
    }
    if presentation != expected_presentation {
        return Err(CertEnvelopeError::PresentationMismatch);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Semantic checking
// ---------------------------------------------------------------------------

/// The result of independently re-deriving a certificate's claim.
///
/// Two arms, deliberately. There is no `Refuted`: failing to re-derive
/// quiescence does not establish that a live step exists, and failing to
/// re-derive divergence does not establish termination.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CertificateCheck<Id> {
    /// Independently re-derived. `Derived`-grade in the certificate's exact
    /// context, profile, and revision — **never a theorem**, and never a claim
    /// about any other configuration.
    Verified {
        /// The identity of the certificate that was re-derived.
        certificate_id: Id,
    },
    /// Never a pass, never `Refuted`.
    Unknown(CertificateCheckError),
}

impl<Id> CertificateCheck<Id> {
    /// The certificate identity if this is a pass.
    pub fn verified_id(self) -> Option<Id> {
        match self {
            CertificateCheck::Verified { certificate_id } => Some(certificate_id),
            CertificateCheck::Unknown(_) => None,
        }
    }

    /// Whether this is a pass.
    pub fn is_verified(&self) -> bool {
        matches!(self, CertificateCheck::Verified { .. })
    }
}

/// Why a certificate's claim could not be re-derived.
///
/// Every variant is graded `Unknown`; none is evidence for the negation of the
/// certificate's claim.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CertificateCheckError {
    /// The certificate does not re-encode to a decodable v1 envelope, or is
    /// bound to a different context/profile/revision than the presentation.
    Envelope(CertEnvelopeError),
    /// The certificate's regime-set identity is not the presentation's.
    RegimeSetMismatch,
    /// The certificate's admissibility identity is not the presentation's.
    AdmMismatch,
    /// The supplied step slice is not the length the certificate declares.
    StepCountMismatch {
        /// Steps the certificate declares.
        declared: u64,
        /// Steps supplied.
        supplied: u64,
    },
    /// A supplied step's canonical digest is not the one recorded.
    StepDigestMismatch {
        /// Index into the supplied slice.
        at_step: u64,
    },
    /// Replaying the supplied steps from a fresh history does not reproduce the
    /// recorded chain digest.
    ChainDigestMismatch,
    /// The observation profile could not classify a recorded step.
    ProfileError {
        /// Index into the supplied slice.
        at_step: u64,
        /// Why classification failed.
        error: ProfileError,
    },
    /// A recorded step is **realizing**, so the prefix is not administrative
    /// and the hiding claim is false. One such step invalidates the whole
    /// certificate.
    PrefixNotAdministrative {
        /// Index into the supplied slice.
        at_step: u64,
    },
    /// The recorded steps do not form a path: some step's destination is not
    /// its successor's source.
    StepsDoNotChain {
        /// Index of the step whose destination breaks the chain.
        at_step: u64,
    },
    /// The first recorded step does not start at the certificate's source world
    /// (or, for an empty prefix, source and terminal differ).
    SourceWorldMismatch,
    /// The last recorded step does not end at the certificate's terminal world.
    TerminalWorldMismatch,
    /// The supplied configuration is not the one the certificate names.
    ConfigMismatch,
    /// The frontier at the terminal world is **not** empty, so the claim is
    /// false as stated.
    FrontierNotEmpty {
        /// How many admissible candidates the re-enumeration found.
        candidates: u64,
    },
    /// The re-enumeration did no measurable work — there was no regime to scan,
    /// so "the frontier is empty" is vacuous rather than decided.
    EnumerationNotScanned,
    /// The recomputed quiescence judgement identity differs from the recorded
    /// one.
    JudgementMismatch,
    /// A divergence claim needs a hypothesis this presentation does not declare.
    UndeclaredAssumption(AssumptionId),
    /// The lasso's cycle does not close: the cycle-entry state is not revisited
    /// at the end of the cycle.
    CycleDoesNotClose,
    /// The cycle-entry world or policy is not the state the lasso actually
    /// revisits.
    CycleEntryMismatch,
}

impl From<CertEnvelopeError> for CertificateCheckError {
    fn from(err: CertEnvelopeError) -> Self {
        CertificateCheckError::Envelope(err)
    }
}

/// The quiescence judgement a certificate publishes: `Quiescent(x, p, 𝑅, A)`
/// in the certificate's context, `Derived`, supported by a settlement replay of
/// the hidden prefix's chain digest.
///
/// Exposed so a caller can construct the same judgement the checker recomputes,
/// without reaching into the encoder.
pub fn quiescence_judgement(cert: &QuiescenceCertificateV1) -> JudgementId {
    quiescence_judgement_of(
        cert.context,
        cert.terminal_world,
        cert.policy,
        cert.regime_set,
        cert.adm_id,
        cert.prefix_chain,
    )
}

/// The same judgement identity, from the parts — so a certificate can be built
/// with its judgement already correct rather than patched in afterwards.
///
/// Identity only, never publication (ADR-0016 §3). This function is called on
/// both sides of the certificate contract — once to state the claim, once by
/// `check_quiescence_certificate` to re-derive it from the presentation and
/// compare — and its support is the hidden prefix's chain digest, not a
/// `Decomposition`. The authority for a quiescence claim is the certificate
/// check (ADR-0014 §6.3), not the possession of a judgement value; no
/// `Judgement` is minted here.
pub(crate) fn quiescence_judgement_of(
    context: ContextId,
    terminal_world: ConfigId,
    policy: ConfigId,
    regime_set: Digest,
    adm_id: Digest,
    prefix_chain: Digest,
) -> JudgementId {
    let proposition: PropositionId =
        Quiescent::new(terminal_world, policy, regime_set, adm_id).proposition_id();
    let evidence = Evidence::SettlementReplay { body: prefix_chain }.id();
    JudgementId::recompute(context, proposition, Outcome::Derived, evidence)
}

/// Re-derive a quiescence certificate's claim from the presentation and the
/// recorded prefix. Total and fail-closed (ADR-0014 §6.3).
///
/// Checks, in order: the envelope decodes exactly and is bound to this
/// presentation's context/profile/revision; the regime set and `Adm` match; the
/// recorded step digests match the supplied steps; replaying those steps from a
/// fresh [`History`] reproduces the recorded chain digest; **every** recorded
/// step is labelled administrative; the steps chain from the source world to
/// the terminal world; the supplied configuration is the one named; the
/// frontier there is re-enumerated and required empty with measurable work; and
/// the quiescence judgement identity recomputes.
///
/// **Signature note.** ADR-0014 §6.3 sketched this without a configuration
/// parameter. It needs one: a certificate carries `ConfigId` *digests*, and
/// [`crate::intern::Interner`] has no digest→[`crate::intern::Handle`] reverse
/// lookup, so the terminal configuration cannot be reconstructed from the
/// certificate alone. Taking it explicitly and **verifying** it against the
/// certificate's digests is strictly stronger than assuming it: the checker
/// re-enumerates the frontier at a configuration it has proved is the one the
/// certificate names.
pub fn check_quiescence_certificate(
    cert: &QuiescenceCertificateV1,
    pres: &PresentationV1<'_>,
    terminal: &ExecConfig,
    prefix: &[CommittedStep],
) -> CertificateCheck<QuiescenceCertificateId> {
    match check_quiescence_inner(cert, pres, terminal, prefix) {
        Ok(certificate_id) => CertificateCheck::Verified { certificate_id },
        Err(error) => CertificateCheck::Unknown(error),
    }
}

fn check_quiescence_inner(
    cert: &QuiescenceCertificateV1,
    pres: &PresentationV1<'_>,
    terminal: &ExecConfig,
    prefix: &[CommittedStep],
) -> Result<QuiescenceCertificateId, CertificateCheckError> {
    // 1. Round-trip through the frozen envelope and bind it to this
    //    presentation. Encoding then decoding is not ceremony: it is how a
    //    struct carrying, say, `Outcome::Proven` is refused.
    let bytes = encode_quiescence_v1(cert);
    let certificate_id = validate_quiescence_v1(&bytes, pres.context, pres.profile.id(), pres.id)?;

    // 2. The claim names the presentation's governance, not some other's.
    if cert.regime_set != pres.regime_set {
        return Err(CertificateCheckError::RegimeSetMismatch);
    }
    if cert.adm_id != pres.adm_id {
        return Err(CertificateCheckError::AdmMismatch);
    }

    // 3. The supplied prefix is the recorded one, step for step.
    check_recorded_steps(&cert.hidden, prefix)?;

    // 4. Replay from a fresh history reproduces the recorded chain.
    if replay_chain_digest(prefix) != cert.prefix_chain {
        return Err(CertificateCheckError::ChainDigestMismatch);
    }

    // 5. Every hidden step really is administrative under this profile.
    check_all_administrative(pres, prefix)?;

    // 6. The prefix is a path from the source world to the terminal world.
    if walk_path(prefix, cert.src_world)? != cert.terminal_world {
        return Err(CertificateCheckError::TerminalWorldMismatch);
    }

    // 7. The configuration handed to us is the one the certificate names.
    if pres.interner.try_resolve(terminal.world) != Some(cert.terminal_world.digest())
        || pres.interner.try_resolve(terminal.policy) != Some(cert.policy.digest())
    {
        return Err(CertificateCheckError::ConfigMismatch);
    }

    // 8. Re-enumerate. This is the load-bearing step: everything above is
    //    bookkeeping about a path, this is the actual negative claim.
    let (frontier, work) = enumerate_admissible(pres.regimes, pres.adm, terminal);
    if work == 0 {
        return Err(CertificateCheckError::EnumerationNotScanned);
    }
    if !frontier.is_empty() {
        return Err(CertificateCheckError::FrontierNotEmpty {
            candidates: frontier.len() as u64,
        });
    }

    // 9. The published judgement is the one this claim entails.
    if quiescence_judgement(cert) != cert.judgement {
        return Err(CertificateCheckError::JudgementMismatch);
    }

    Ok(certificate_id)
}

/// Re-derive a divergence certificate's claim from the presentation and the
/// recorded lasso. Total and fail-closed.
///
/// Beyond the shared prefix checks it requires: both P1 and P6 declared (a
/// lasso is only a divergence proof under them); the cycle to close — the world
/// and policy entering the cycle are exactly those the cycle's last step
/// returns to; and the recorded cycle-entry state to be that state.
pub fn check_divergence_certificate(
    cert: &DivergenceCertificateV1,
    pres: &PresentationV1<'_>,
    lasso: &[CommittedStep],
) -> CertificateCheck<DivergenceCertificateId> {
    match check_divergence_inner(cert, pres, lasso) {
        Ok(certificate_id) => CertificateCheck::Verified { certificate_id },
        Err(error) => CertificateCheck::Unknown(error),
    }
}

fn check_divergence_inner(
    cert: &DivergenceCertificateV1,
    pres: &PresentationV1<'_>,
    lasso: &[CommittedStep],
) -> Result<DivergenceCertificateId, CertificateCheckError> {
    let bytes = encode_divergence_v1(cert);
    let certificate_id = validate_divergence_v1(&bytes, pres.context, pres.profile.id(), pres.id)?;

    // A conditional result may only be re-derived under the same declarations.
    if !pres.assumptions.declares(AssumptionId::HistoryIndependence) {
        return Err(CertificateCheckError::UndeclaredAssumption(
            AssumptionId::HistoryIndependence,
        ));
    }
    if !pres.assumptions.declares(AssumptionId::PhaseStableKeying) {
        return Err(CertificateCheckError::UndeclaredAssumption(
            AssumptionId::PhaseStableKeying,
        ));
    }

    check_recorded_steps(&cert.lasso, lasso)?;
    check_all_administrative(pres, lasso)?;

    // The lasso is a path from the source world; where it *ends* is the claim.
    let end = walk_path(lasso, cert.src_world)?;

    // `stem` indexes the lasso: the step at that index is the one that leaves
    // the cycle-entry world.
    let stem = usize::try_from(cert.stem)
        .map_err(|_| CertificateCheckError::Envelope(CertEnvelopeError::LassoLengthOverflow))?;
    let entry_src = if stem == 0 {
        cert.src_world
    } else {
        lasso[stem - 1].dst
    };
    if entry_src != cert.cycle_world {
        return Err(CertificateCheckError::CycleEntryMismatch);
    }
    // …and the cycle closes iff the lasso comes back to exactly that world.
    if end != cert.cycle_world {
        return Err(CertificateCheckError::CycleDoesNotClose);
    }
    // Policy is carried unchanged by every applied candidate, so the cycle
    // entry's policy is the run's policy. An engine that mutated policy would
    // land here rather than silently pass.
    if cert.cycle_policy != cert.policy {
        return Err(CertificateCheckError::CycleEntryMismatch);
    }

    Ok(certificate_id)
}

// ---------------------------------------------------------------------------
// Shared checking helpers
// ---------------------------------------------------------------------------

/// The chain digest after `steps`, replayed from a fresh [`History`]. Zero
/// steps give [`History::empty`]'s digest — the honest value, not a sentinel.
pub(crate) fn replay_chain_digest(steps: &[CommittedStep]) -> Digest {
    Journal::replay_chain(steps)
        .last()
        .copied()
        .unwrap_or_else(|| History::empty().digest())
}

fn check_recorded_steps(
    recorded: &[Digest],
    supplied: &[CommittedStep],
) -> Result<(), CertificateCheckError> {
    if recorded.len() != supplied.len() {
        return Err(CertificateCheckError::StepCountMismatch {
            declared: recorded.len() as u64,
            supplied: supplied.len() as u64,
        });
    }
    for (index, (digest, step)) in recorded.iter().zip(supplied).enumerate() {
        if step.canon_digest(Domain::Value) != *digest {
            return Err(CertificateCheckError::StepDigestMismatch {
                at_step: index as u64,
            });
        }
    }
    Ok(())
}

fn check_all_administrative(
    pres: &PresentationV1<'_>,
    steps: &[CommittedStep],
) -> Result<(), CertificateCheckError> {
    for (index, step) in steps.iter().enumerate() {
        match pres.profile.label(step) {
            Err(error) => {
                return Err(CertificateCheckError::ProfileError {
                    at_step: index as u64,
                    error,
                })
            }
            Ok(StepLabel::Realizing) => {
                return Err(CertificateCheckError::PrefixNotAdministrative {
                    at_step: index as u64,
                })
            }
            Ok(StepLabel::Administrative) => {}
        }
    }
    Ok(())
}

/// Require `steps` to be a path starting at `src`, and return the world it ends
/// at (`src` itself when there are no steps — an empty prefix hides nothing and
/// moves nowhere).
///
/// The endpoint is *returned* rather than compared here because the two
/// certificates want different things from it: quiescence wants a terminal
/// world, divergence wants the cycle-entry world it has come back to.
fn walk_path(steps: &[CommittedStep], src: ConfigId) -> Result<ConfigId, CertificateCheckError> {
    let Some(first) = steps.first() else {
        return Ok(src);
    };
    if first.src != src {
        return Err(CertificateCheckError::SourceWorldMismatch);
    }
    for (index, pair) in steps.windows(2).enumerate() {
        if pair[0].dst != pair[1].src {
            return Err(CertificateCheckError::StepsDoNotChain {
                at_step: index as u64,
            });
        }
    }
    Ok(steps[steps.len() - 1].dst)
}
