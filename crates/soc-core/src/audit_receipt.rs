//! The settlement audit receipt (ADR-0020 D5/D6/D7) — what a successful
//! [`crate::audit::audit_step`] *identifies*, as opposed to what it publishes.
//!
//! # Why this exists
//!
//! ADR-0019 made the `ReplayVerified` tag earnable only by executing the
//! relation over every link, and ADR-0020 Stages A–C made the relation itself
//! canonical data. Neither step recorded **which** audit environment ran: the
//! `Audited` judgement's evidence names only the verified
//! [`Decomposition`](brix_semantic::Decomposition), so two audits of the same
//! chain under different registries or semantics declarations are
//! indistinguishable after the fact (ADR-0019 §6 residual 2).
//!
//! A receipt binds the five things a checker can independently re-derive:
//!
//! | # | field | why it is here |
//! |---|---|---|
//! | 1 | [`ContextId`] | an argument to `audit_step`, not part of the step |
//! | 2 | committed-step digest | binds observation *and* endpoints in one frozen field |
//! | 3 | verified `DecompositionId` | the stage-3 result, not derivable by re-tagging |
//! | 4 | `GeneratorRegistryId` | which `𝒢` membership was checked against |
//! | 5 | `GeneratorSemanticsIdV1` | **which oracle ran** |
//!
//! There is deliberately no separate observation field: `CommittedStep`
//! already canonically contains `key, observation, decomposition, src, dst,
//! witness` in frozen order, so its digest binds the exact observation and
//! endpoint claims `audit_step` checked. Repeating them would add no
//! independently re-derivable distinction (ADR-0020 D6).
//!
//! # What a receipt is *not*
//!
//! **It is not evidence, and it does not change what `Audited` means**
//! (ADR-0020 D1). The judgement, its evidence id, and its `JudgementId` are
//! byte-identical to what they were before this module existed. No `Evidence`
//! ordinal is appended: an unused variant would be decorative, and a used one
//! would move every affected `JudgementId` and create a second authority
//! route. A consumer wanting oracle-bound provenance must *keep and validate*
//! the receipt; one that discards it retains exactly the ADR-0019 guarantee.
//!
//! **It does not attest journal inclusion** (ADR-0020 §5 residual 4).
//! `audit_step` holds one `CommittedStep`, not a `Journal` — so a receipt
//! cannot honestly name a journal ordinal or prefix-chain digest without
//! widening the API. A journal receipt would be a different artifact minted by
//! `audit_journal`, not a field quietly added here.
//!
//! # Checked by replay, never trusted as a record
//!
//! [`check_audit_receipt_v1`] does not read a field and believe it. It
//! re-derives every id from independently supplied typed values, reruns the
//! audit, and compares. A receipt whose semantics id a consumer adopts *from
//! the receipt itself* has authenticated nothing (ADR-0020 §2) — the expected
//! registry and semantics are parameters for exactly that reason.

use brix_canon::{CanonReader, CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ContextId, DecompositionId, GeneratorRegistry, GeneratorRegistryId, GeneratorSemanticsIdV1,
    GeneratorSemanticsV1,
};

use crate::audit::{audit_step, AuditResult};
use crate::audit_bundle::AuditDecodeLimits;
use crate::journal::CommittedStep;

/// The fixed marker opening a [`SettlementAuditReceiptV1`] preimage
/// (ADR-0020 D5 field 1). Frozen v1 ABI.
pub const AUDIT_RECEIPT_MARKER_V1: &[u8] = b"brix.soc.audit-receipt";

/// The receipt format version (ADR-0020 D5 field 2).
pub const AUDIT_RECEIPT_VERSION_V1: u64 = 1;

/// The one v1 settlement audit checking algorithm (ADR-0020 D5 field 3).
///
/// A separate `VerifierId` is deliberately *not* added: that type identifies
/// proof kernels, and v1 has exactly one fixed settlement audit profile, so an
/// extra field holding another fixed digest would add repetition without an
/// independent choice to validate.
pub const AUDIT_PROFILE_V1: &str = "brix.soc.audit-factorization@1";

/// Why a receipt was refused (ADR-0020 D7, ADR-0026 ⟨D-REISSUE⟩).
///
/// Rust-side validation only — never canonically encoded, so no ABI ordinal.
/// Every variant means **the receipt is not accepted**; none of them
/// constructs `Audited`, produces a receipt, or yields `Refuted`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReceiptError {
    /// The supplied semantics declaration is not the one the consumer
    /// expected. This is the check that makes authentication real: a receipt
    /// naming its *own* expectation authenticates nothing.
    UnexpectedSemantics {
        expected: GeneratorSemanticsIdV1,
        found: GeneratorSemanticsIdV1,
    },
    /// The supplied registry is not the one the consumer expected.
    UnexpectedRegistry {
        expected: GeneratorRegistryId,
        found: GeneratorRegistryId,
    },
    /// The declared semantics does not cover exactly the registry
    /// (ADR-0020 D2) — so the receipt would name a subset of the audit
    /// environment while claiming to name the environment.
    SemanticsRegistryDisagreement,
    /// Re-running the audit under the supplied inputs did not reproduce an
    /// `Audited` result. Carries the checker's own fail-closed reason.
    ReplayFailed(&'static str),
    /// The audit replayed, but a re-derived field does not match the receipt.
    FieldMismatch {
        /// Which field disagreed, as a fixed name.
        field: &'static str,
    },
    /// Bad fixed marker bytes opening receipt payload (ADR-0026 §6).
    BadMarker,
    /// Unknown receipt format version (ADR-0026 §6).
    UnknownVersion(u64),
    /// Unknown receipt profile (ADR-0026 §6).
    UnknownProfile,
    /// Receipt frame carried trailing unconsumed bytes (ADR-0026 §6).
    TrailingBytes,
    /// Receipt length in bytes exceeded the configured limit (ADR-0026 §6).
    ReceiptBytesTooLarge { limit: usize, found: usize },
    /// Canonical framing error or malformed structure.
    MalformedReceipt(&'static str),
}

/// A settlement audit receipt: the exact inputs and checker profile a
/// successful [`audit_step`] ran under (ADR-0020 D5).
///
/// Fields are private, following ADR-0019 D1 — this artifact's identity *is*
/// the claim, so a caller able to set a field could mint a receipt naming an
/// audit environment that never ran.
///
/// # No public constructor from decoded bytes (ADR-0026 ⟨D-REISSUE⟩)
///
/// ```compile_fail
/// use soc_core::audit_receipt::SettlementAuditReceiptV1;
/// // Fields are private; no public decoded-bytes constructor exists.
/// let _ = SettlementAuditReceiptV1 {
///     context: todo!(),
///     committed_step: todo!(),
///     verified_decomposition: todo!(),
///     registry: todo!(),
///     semantics: todo!(),
/// };
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettlementAuditReceiptV1 {
    context: ContextId,
    committed_step: Digest,
    verified_decomposition: DecompositionId,
    registry: GeneratorRegistryId,
    semantics: GeneratorSemanticsIdV1,
}

impl SettlementAuditReceiptV1 {
    /// The context the audit ran under.
    pub const fn context(&self) -> ContextId {
        self.context
    }

    /// The canonical digest of the exact committed step audited — which binds
    /// its observation and endpoints (ADR-0020 D6).
    pub const fn committed_step(&self) -> Digest {
        self.committed_step
    }

    /// The earned `ReplayVerified` decomposition's id.
    pub const fn verified_decomposition(&self) -> DecompositionId {
        self.verified_decomposition
    }

    /// Which `𝒢` membership was checked against.
    pub const fn registry(&self) -> GeneratorRegistryId {
        self.registry
    }

    /// **Which oracle ran** — the field ADR-0019 could not provide.
    pub const fn semantics(&self) -> GeneratorSemanticsIdV1 {
        self.semantics
    }

    /// The content-addressed id of this receipt.
    pub fn id(&self) -> SettlementAuditReceiptIdV1 {
        SettlementAuditReceiptIdV1::of(self)
    }
}

impl Canonical for SettlementAuditReceiptV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Frozen v1 preimage (ADR-0020 D5). Field order is ABI.
        w.write_bytes(AUDIT_RECEIPT_MARKER_V1);
        w.write_uint(AUDIT_RECEIPT_VERSION_V1);
        w.write_str(AUDIT_PROFILE_V1);
        self.context.canon_write(w);
        w.write_bytes(self.committed_step.as_bytes());
        self.verified_decomposition.canon_write(w);
        self.registry.canon_write(w);
        self.semantics.canon_write(w);
    }
}

/// Content-addressed identity of a [`SettlementAuditReceiptV1`].
///
/// Hand-written rather than produced by `brix-semantic`'s `digest_id!`, which
/// is crate-private there — but deliberately the same shape (a distinct
/// newtype over a `Domain::Value` digest), so it cannot be passed where
/// another id is wanted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SettlementAuditReceiptIdV1(pub Digest);

impl SettlementAuditReceiptIdV1 {
    /// The content-addressed id of any canonically-encodable value.
    pub fn of(value: &impl Canonical) -> Self {
        let mut w = CanonWriter::new();
        value.canon_write(&mut w);
        SettlementAuditReceiptIdV1(Digest::of(Domain::Value, &w.finish()))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl Canonical for SettlementAuditReceiptIdV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// The canonical digest of a committed step, as the receipt binds it.
pub fn committed_step_digest(step: &CommittedStep) -> Digest {
    let mut w = CanonWriter::new();
    step.canon_write(&mut w);
    w.digest(Domain::Value)
}

/// Mint the receipt for an audit that has already succeeded.
///
/// `pub(crate)` by intent, and it takes the **earned** `DecompositionId`
/// rather than a chain it could tag itself: a receipt is only ever produced
/// alongside a real `Audited` result, so there is no public constructor a
/// caller could use to describe an audit that did not happen. This is the same
/// discipline ADR-0019 D1 applied to verification tags — the artifact is an
/// output of the work, never an input to it.
pub(crate) fn issue_receipt(
    step: &CommittedStep,
    context: ContextId,
    registry: &GeneratorRegistry,
    semantics: &GeneratorSemanticsV1,
    verified: DecompositionId,
) -> SettlementAuditReceiptV1 {
    SettlementAuditReceiptV1 {
        context,
        committed_step: committed_step_digest(step),
        verified_decomposition: verified,
        registry: registry.id(),
        semantics: semantics.id(),
    }
}

/// Validate a receipt **by replay** (ADR-0020 D7).
///
/// The expected registry and semantics are supplied by the caller and compared
/// against the receipt — they are never read out of it. That asymmetry is the
/// whole mechanism: a consumer that adopts the receipt's own semantics id as
/// its expectation has authenticated nothing (ADR-0020 §2).
///
/// Order matters. The expectation checks run **first**, so a receipt from an
/// unexpected audit environment is refused before any replay work, and the
/// refusal names the environment rather than a downstream symptom.
///
/// Fails closed: every rejection is a typed [`ReceiptError`], no judgement is
/// constructed, and nothing produces `Refuted`.
pub fn check_audit_receipt_v1(
    receipt: &SettlementAuditReceiptV1,
    step: &CommittedStep,
    context: ContextId,
    expected_registry: &GeneratorRegistry,
    expected_semantics: &GeneratorSemanticsV1,
) -> Result<SettlementAuditReceiptIdV1, ReceiptError> {
    // 1. The consumer's expectation, independently held.
    let expected_semantics_id = expected_semantics.id();
    if receipt.semantics != expected_semantics_id {
        return Err(ReceiptError::UnexpectedSemantics {
            expected: expected_semantics_id,
            found: receipt.semantics,
        });
    }
    let expected_registry_id = expected_registry.id();
    if receipt.registry != expected_registry_id {
        return Err(ReceiptError::UnexpectedRegistry {
            expected: expected_registry_id,
            found: receipt.registry,
        });
    }

    // 2. The environment must be internally coherent (ADR-0020 D2).
    if expected_semantics
        .require_matches_registry(expected_registry)
        .is_err()
    {
        return Err(ReceiptError::SemanticsRegistryDisagreement);
    }

    // 3. Contextual fields, re-derived from the supplied typed values.
    if receipt.context != context {
        return Err(ReceiptError::FieldMismatch { field: "context" });
    }
    if receipt.committed_step != committed_step_digest(step) {
        return Err(ReceiptError::FieldMismatch {
            field: "committed_step",
        });
    }

    // 4. Rerun the real audit — the same function that issues receipts, so
    //    there is exactly one replay algorithm (ADR-0020 Stage D item 4).
    let audited = match audit_step(step, context, expected_registry, expected_semantics) {
        AuditResult::Audited(a) => a,
        AuditResult::Unknown(reason) => return Err(ReceiptError::ReplayFailed(reason)),
    };

    // 5. The stage-3 result must be the one the receipt names.
    if receipt.verified_decomposition != audited.verified.id() {
        return Err(ReceiptError::FieldMismatch {
            field: "verified_decomposition",
        });
    }

    // 6. And the whole receipt must reproduce, byte for byte.
    let rederived = issue_receipt(
        step,
        context,
        expected_registry,
        expected_semantics,
        audited.verified.id(),
    );
    if &rederived != receipt {
        return Err(ReceiptError::FieldMismatch { field: "receipt" });
    }

    Ok(receipt.id())
}

/// The crate-private claimed view of receipt bytes (ADR-0026 ⟨D-REISSUE⟩).
///
/// Has NO public conversion to [`SettlementAuditReceiptV1`]. Acceptance is by
/// re-running the audit, locally reissuing, and byte-for-byte equality.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ClaimedReceiptV1 {
    pub(crate) context: ContextId,
    pub(crate) committed_step: Digest,
    pub(crate) verified_decomposition: DecompositionId,
    pub(crate) registry: GeneratorRegistryId,
    pub(crate) semantics: GeneratorSemanticsIdV1,
}

impl ClaimedReceiptV1 {
    pub(crate) fn decode(bytes: &[u8], max_receipt_bytes: usize) -> Result<Self, ReceiptError> {
        if bytes.len() > max_receipt_bytes {
            return Err(ReceiptError::ReceiptBytesTooLarge {
                limit: max_receipt_bytes,
                found: bytes.len(),
            });
        }
        let mut r = CanonReader::new(bytes);
        let marker = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read marker"))?;
        if marker != AUDIT_RECEIPT_MARKER_V1 {
            return Err(ReceiptError::BadMarker);
        }
        let version = r
            .read_uint()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read version"))?;
        if version != AUDIT_RECEIPT_VERSION_V1 {
            return Err(ReceiptError::UnknownVersion(version));
        }
        let profile = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read profile"))?;
        if profile != AUDIT_PROFILE_V1.as_bytes() {
            return Err(ReceiptError::UnknownProfile);
        }

        let context_bytes = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read context"))?;
        let context_arr: [u8; 32] = context_bytes
            .try_into()
            .map_err(|_| ReceiptError::MalformedReceipt("context digest not 32 bytes"))?;
        let context = ContextId(Digest::from_bytes(context_arr));

        let step_bytes = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read committed_step"))?;
        let step_arr: [u8; 32] = step_bytes
            .try_into()
            .map_err(|_| ReceiptError::MalformedReceipt("committed_step digest not 32 bytes"))?;
        let committed_step = Digest::from_bytes(step_arr);

        let decomp_bytes = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read verified_decomposition"))?;
        let decomp_arr: [u8; 32] = decomp_bytes.try_into().map_err(|_| {
            ReceiptError::MalformedReceipt("verified_decomposition digest not 32 bytes")
        })?;
        let verified_decomposition = DecompositionId(Digest::from_bytes(decomp_arr));

        let reg_bytes = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read registry"))?;
        let reg_arr: [u8; 32] = reg_bytes
            .try_into()
            .map_err(|_| ReceiptError::MalformedReceipt("registry digest not 32 bytes"))?;
        let registry = GeneratorRegistryId(Digest::from_bytes(reg_arr));

        let sem_bytes = r
            .read_bytes()
            .map_err(|_| ReceiptError::MalformedReceipt("cannot read semantics"))?;
        let sem_arr: [u8; 32] = sem_bytes
            .try_into()
            .map_err(|_| ReceiptError::MalformedReceipt("semantics digest not 32 bytes"))?;
        let semantics = GeneratorSemanticsIdV1(Digest::from_bytes(sem_arr));

        if !r.is_empty() {
            return Err(ReceiptError::TrailingBytes);
        }

        Ok(ClaimedReceiptV1 {
            context,
            committed_step,
            verified_decomposition,
            registry,
            semantics,
        })
    }
}

/// Validate receipt bytes by **reissue and byte-for-byte compare** (ADR-0026 ⟨D-REISSUE⟩).
///
/// Decodes receipt bytes ONLY to a crate-private claimed view, verifies expectations
/// and contextual values, replays the audit, reissues the receipt locally, and requires
/// byte-for-byte equality with `receipt_bytes`.
///
/// There is NO public conversion from decoded bytes to [`SettlementAuditReceiptV1`].
pub fn check_audit_receipt_bytes_v1(
    receipt_bytes: &[u8],
    step: &CommittedStep,
    context: ContextId,
    expected_registry: &GeneratorRegistry,
    expected_semantics: &GeneratorSemanticsV1,
    limits: &AuditDecodeLimits,
) -> Result<SettlementAuditReceiptIdV1, ReceiptError> {
    // 0. Enforce limit before decoding
    if receipt_bytes.len() > limits.max_receipt_bytes {
        return Err(ReceiptError::ReceiptBytesTooLarge {
            limit: limits.max_receipt_bytes,
            found: receipt_bytes.len(),
        });
    }

    // 1. Decode to crate-private claimed view (rejects bad marker, version, profile, lengths, trailing bytes)
    let claimed = ClaimedReceiptV1::decode(receipt_bytes, limits.max_receipt_bytes)?;

    // 2. Expectations check first (ADR-0020 D7)
    let expected_semantics_id = expected_semantics.id();
    if claimed.semantics != expected_semantics_id {
        return Err(ReceiptError::UnexpectedSemantics {
            expected: expected_semantics_id,
            found: claimed.semantics,
        });
    }
    let expected_registry_id = expected_registry.id();
    if claimed.registry != expected_registry_id {
        return Err(ReceiptError::UnexpectedRegistry {
            expected: expected_registry_id,
            found: claimed.registry,
        });
    }

    // 3. Environment coherence (ADR-0020 D2)
    if expected_semantics
        .require_matches_registry(expected_registry)
        .is_err()
    {
        return Err(ReceiptError::SemanticsRegistryDisagreement);
    }

    // 4. Contextual fields
    if claimed.context != context {
        return Err(ReceiptError::FieldMismatch { field: "context" });
    }
    if claimed.committed_step != committed_step_digest(step) {
        return Err(ReceiptError::FieldMismatch {
            field: "committed_step",
        });
    }

    // 5. Rerun audit
    let audited = match audit_step(step, context, expected_registry, expected_semantics) {
        AuditResult::Audited(a) => a,
        AuditResult::Unknown(reason) => return Err(ReceiptError::ReplayFailed(reason)),
    };

    if claimed.verified_decomposition != audited.verified.id() {
        return Err(ReceiptError::FieldMismatch {
            field: "verified_decomposition",
        });
    }

    // 6. Reissue and byte-for-byte compare (ADR-0026 ⟨D-REISSUE⟩)
    let reissued = issue_receipt(
        step,
        context,
        expected_registry,
        expected_semantics,
        audited.verified.id(),
    );

    if reissued.canon_bytes() != receipt_bytes {
        return Err(ReceiptError::FieldMismatch { field: "receipt" });
    }

    Ok(reissued.id())
}
