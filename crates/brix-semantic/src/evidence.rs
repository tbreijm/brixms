//! [`Evidence`] — the support behind a [`crate::Judgement`] — and its
//! content-addressed [`EvidenceId`] (ADR-0001 §5.3).
//!
//! Evidence splits on a **durability axis** that governs retraction:
//!
//! - **Durable** (revision-invariant): a kernel certificate or refutation. A
//!   proof stays a proof, closed over its own explicit context — it survives
//!   revision changes.
//! - **Revision-scoped** (invalidates on retraction): a ground assertion, a
//!   settlement replay, a measurement/simulation, a suggestion, or a certified
//!   external result. These hold *at a revision* and fall when their support is
//!   retracted (ADR §7).
//!
//! The substrate fixes the *taxonomy* and *durability*; the evidence *body*
//! (the actual step, certificate, or measurement) is an opaque
//! [`brix_canon::Digest`] owned by the producing subsystem. Kernel evidence
//! additionally names its **verifier** ([`VerifierId`]) — which kernel
//! certified it (`brix-kernel`, Lean, …) — so a durable certificate is
//! attributable (this is what makes "a mathlib theorem = durable evidence whose
//! verifier is Lean" representable).

use brix_canon::{CanonWriter, Canonical, Digest};

use crate::id::digest_id;

digest_id!(
    /// Identity of a proof **verifier** (a kernel). The publishing *role* is
    /// `Authority::ProofKernel`; this names *which* kernel — `brix-kernel`, an
    /// external Lean, … — content-addressed from its name+version.
    VerifierId
);

impl VerifierId {
    /// A verifier identified by a `name@version`-style string (e.g.
    /// `"brix.kernel@0.1"`, `"lean@4"`).
    pub fn named(name: &str) -> Self {
        let mut w = CanonWriter::new();
        w.write_str(name);
        VerifierId::from_canon(&w.finish())
    }
}

digest_id!(
    /// Opaque identity of a proof **certificate** — the explicit term the
    /// kernel accepted. Its internal encoding is the kernel's concern; the
    /// substrate only references it (a kernel-agnostic handle, so `brix-kernel`
    /// and an external Lean adapter produce the same shape).
    CertificateId
);

/// Whether a piece of [`Evidence`] survives a revision change.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Durability {
    /// Revision-invariant — a kernel-accepted proof/refutation.
    Durable,
    /// Holds at a revision; invalidates when its support is retracted.
    RevisionScoped,
}

/// The support behind a judgement. The canonical enum ordinals below are
/// **ABI** — append-only, never reordered.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Evidence {
    // --- durable (revision-invariant) ---
    /// A proof-kernel-accepted certificate that the proposition holds.
    KernelCertificate {
        verifier: VerifierId,
        certificate: CertificateId,
    },
    /// A proof-kernel-accepted refutation.
    KernelRefutation {
        verifier: VerifierId,
        certificate: CertificateId,
    },
    // --- revision-scoped (invalidates on retraction) ---
    /// A raw Ground assertion at the current revision.
    GroundAssertion { body: Digest },
    /// A replay of the settlement derivation that produced the fact.
    SettlementReplay { body: Digest },
    /// A measurement, simulation, or estimate (carries its own error profile
    /// in its body).
    Measurement { body: Digest },
    /// A non-authoritative suggestion (e.g. a resolver candidate).
    Suggestion { body: Digest },
    /// A result certified by a named external system, envelope-wrapped.
    CertifiedExternalResult { body: Digest },
}

impl Evidence {
    /// Whether this evidence survives a revision change (ADR §5.3 / §7). The
    /// two kernel variants are durable; everything else is revision-scoped.
    pub const fn durability(&self) -> Durability {
        match self {
            Evidence::KernelCertificate { .. } | Evidence::KernelRefutation { .. } => {
                Durability::Durable
            }
            Evidence::GroundAssertion { .. }
            | Evidence::SettlementReplay { .. }
            | Evidence::Measurement { .. }
            | Evidence::Suggestion { .. }
            | Evidence::CertifiedExternalResult { .. } => Durability::RevisionScoped,
        }
    }

    /// The content-addressed id of this evidence.
    pub fn id(&self) -> EvidenceId {
        EvidenceId::of(self)
    }

    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(&self) -> u64 {
        match self {
            Evidence::KernelCertificate { .. } => 0,
            Evidence::KernelRefutation { .. } => 1,
            Evidence::GroundAssertion { .. } => 2,
            Evidence::SettlementReplay { .. } => 3,
            Evidence::Measurement { .. } => 4,
            Evidence::Suggestion { .. } => 5,
            Evidence::CertifiedExternalResult { .. } => 6,
        }
    }
}

impl Canonical for Evidence {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |w| match self {
            Evidence::KernelCertificate {
                verifier,
                certificate,
            }
            | Evidence::KernelRefutation {
                verifier,
                certificate,
            } => {
                verifier.canon_write(w);
                certificate.canon_write(w);
            }
            Evidence::GroundAssertion { body }
            | Evidence::SettlementReplay { body }
            | Evidence::Measurement { body }
            | Evidence::Suggestion { body }
            | Evidence::CertifiedExternalResult { body } => {
                w.write_bytes(body.as_bytes());
            }
        });
    }
}

digest_id!(
    /// Content-addressed identity of a piece of [`Evidence`].
    EvidenceId
);
