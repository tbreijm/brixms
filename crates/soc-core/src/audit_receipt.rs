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

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ContextId, DecompositionId, GeneratorRegistry, GeneratorRegistryId, GeneratorSemanticsIdV1,
    GeneratorSemanticsV1,
};

use crate::audit::{audit_step, AuditResult};
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

/// Why a receipt was refused (ADR-0020 D7).
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
}

/// A settlement audit receipt: the exact inputs and checker profile a
/// successful [`audit_step`] ran under (ADR-0020 D5).
///
/// Fields are private, following ADR-0019 D1 — this artifact's identity *is*
/// the claim, so a caller able to set a field could mint a receipt naming an
/// audit environment that never ran.
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
