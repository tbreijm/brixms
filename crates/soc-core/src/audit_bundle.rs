//! The settlement audit-input transport bundle (ADR-0026, stages B–D).
//!
//! Closes ADR-0022 §5 residual 5 (complete transport of every audit input) and
//! residual 8 (journal snapshot inclusion).
//!
//! # Decisions enforced by this module
//!
//! - **⟨D-NOTAG⟩ (§2):** The wire format carrying step material contains no
//!   `DecompVerification` field. A decoder reconstructs a decomposition only
//!   through [`Decomposition::recorded`].
//! - **⟨D-SNAPSHOT⟩ (§3):** Carries the complete journal in commit order.
//!   Snapshot verification folds from [`History::empty`], requires ordinals
//!   to be exactly `0..n-1`, recomputes every prefix digest, and requires the
//!   recomputed final digest to equal the bundle's.
//! - **⟨D-BUNDLE⟩ (§5):** Content-addressed identity under marker
//!   `b"brix.soc.audit-input-bundle"`, version 1, profile
//!   `"brix.soc.audit-input-bundle@1"`, in [`brix_canon::Domain::Value`].
//!   Observation decoding admits only [`Outcome::Derived`].
//! - **⟨D-DECODELIMITS⟩ (§6):** Governed by noncanonical [`AuditDecodeLimits`].
//!   Every bound is enforced **before** the work it governs, with checked
//!   `u64 → usize` and checked cumulative addition. No maps, no recursion,
//!   no normalization, and no fallback.
//!
//! # Safe Bounded Framing Discipline with CanonReader
//!
//! In `brix-canon`, [`CanonReader`] encapsulates its internal position and buffer,
//! reading length-prefixed frames via [`CanonReader::read_bytes`] into zero-copy
//! borrowed subslices (`&'a [u8]`). Because the outer input buffer is strictly bounded
//! by `max_total_bundle_bytes` before reader construction, zero-copy slicing allocates
//! no heap memory. Frame-level bounds (`max_entry_bytes`, `max_receipt_bytes`) and
//! cumulative framed byte bounds are strictly enforced immediately upon obtaining each
//! frame slice, prior to any sub-frame parsing, vector allocation (`to_vec()`), or
//! iteration. Furthermore, neither producer nor verifier can bypass limits through
//! public typed construction.

use brix_canon::{CanonError, CanonReader, CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, DecompositionError, GeneratorId, GeneratorRegistry,
    GeneratorSemanticsV1, Outcome, WitnessId,
};

use crate::audit::{audit_step, AuditResult};
use crate::audit_receipt::{
    check_audit_receipt_bytes_v1, ReceiptError, SettlementAuditReceiptIdV1,
};
use crate::calendar::Key;
use crate::commit::Observation;
use crate::history::History;
use crate::journal::{CommittedStep, Journal};

/// The fixed marker opening a [`SettlementAuditInputBundleV1`] preimage
/// (ADR-0026 §5). Frozen v1 ABI.
pub const BUNDLE_MARKER_V1: &[u8] = b"brix.soc.audit-input-bundle";

/// The bundle format version (ADR-0026 §5).
pub const BUNDLE_VERSION_V1: u64 = 1;

/// The one v1 bundle profile (ADR-0026 §5).
pub const BUNDLE_PROFILE_V1: &str = "brix.soc.audit-input-bundle@1";

/// Bounds on the work the bundle decoder may perform (ADR-0026 §6, ⟨D-DECODELIMITS⟩).
///
/// Noncanonical: this type contributes to no identity and is never written by a
/// `CanonWriter`. Two verifiers running different limits over the same accepted
/// bundle produce the same outcome; they differ only in which inputs they refuse.
///
/// Every bound is enforced **before** the work it governs, not after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AuditDecodeLimits {
    /// Maximum total bundle length in bytes. Checked before reading or decoding.
    pub max_total_bundle_bytes: usize,
    /// Maximum number of journal steps in the bundle. Checked before allocating or looping.
    pub max_steps: usize,
    /// Maximum bytes per entry frame. Checked before decoding inside the entry.
    pub max_entry_bytes: usize,
    /// Maximum bytes per receipt frame. Checked before decoding or replaying.
    pub max_receipt_bytes: usize,
    /// Maximum generators in a step's decomposition. Checked before allocating or looping.
    pub max_generators_per_step: usize,
    /// Maximum configs in a step's decomposition. Checked before allocating or looping.
    pub max_configs_per_step: usize,
    /// Maximum cumulative decomposition links across all steps in the bundle.
    /// Charged with checked addition before processing generators.
    pub max_cumulative_links: usize,
    /// Maximum cumulative framed bytes across all nested frames in the bundle.
    /// Charged with checked addition before slicing, copying or reserving.
    pub max_cumulative_framed_bytes: usize,
}

impl AuditDecodeLimits {
    /// Default limits according to ADR-0026 §6:
    /// - 64 MiB total bundle
    /// - 100,000 steps
    /// - 1 MiB per entry
    /// - 1 MiB per receipt
    /// - 4,096 generators per step
    /// - 4,097 configs per step
    /// - 1,000,000 cumulative links
    /// - 64 MiB cumulative framed bytes
    pub const fn strict() -> Self {
        AuditDecodeLimits {
            max_total_bundle_bytes: 64 * 1024 * 1024,
            max_steps: 100_000,
            max_entry_bytes: 1024 * 1024,
            max_receipt_bytes: 1024 * 1024,
            max_generators_per_step: 4096,
            max_configs_per_step: 4097,
            max_cumulative_links: 1_000_000,
            max_cumulative_framed_bytes: 64 * 1024 * 1024,
        }
    }
}

impl Default for AuditDecodeLimits {
    fn default() -> Self {
        Self::strict()
    }
}

/// The transport projection of a committed step's verifiable material
/// (ADR-0026 §5, ⟨D-NOTAG⟩).
///
/// Contains NO `DecompVerification` field in any encoding in any version.
/// A decoder reconstructs a decomposition solely through
/// [`Decomposition::recorded`].
///
/// # Compile-fail gate (ADR-0026 ⟨D-NOTAG⟩)
///
/// ```compile_fail
/// use soc_core::audit_bundle::RecordedStepMaterialV1;
/// fn no_verification_tag(m: &RecordedStepMaterialV1) {
///     let _ = m.verification;
/// }
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecordedStepMaterialV1 {
    pub key: Key,
    pub observation: Observation,
    pub generators: Vec<GeneratorId>,
    pub configs: Vec<ConfigId>,
    pub src: ConfigId,
    pub dst: ConfigId,
    pub witness: WitnessId,
}

impl RecordedStepMaterialV1 {
    /// Reconstruct a [`CommittedStep`] from this transport material.
    ///
    /// Reconstructs ONLY via [`Decomposition::recorded`] (ADR-0026 ⟨D-NOTAG⟩).
    pub fn to_committed_step(&self) -> Result<CommittedStep, DecompositionError> {
        let decomposition = Decomposition::recorded(self.generators.clone(), self.configs.clone())?;
        Ok(CommittedStep {
            key: self.key,
            observation: self.observation,
            decomposition,
            src: self.src,
            dst: self.dst,
            witness: self.witness,
        })
    }

    /// Decode recorded step material under the given limits.
    pub fn decode(
        r: &mut CanonReader<'_>,
        limits: &AuditDecodeLimits,
        cumulative_links: &mut usize,
        cumulative_framed_bytes: &mut usize,
    ) -> Result<Self, BundleDecodeError> {
        let phase = r.read_uint().map_err(BundleDecodeError::from)?;
        let priority = r.read_uint().map_err(BundleDecodeError::from)?;
        let tiebreak = read_digest_framed(
            r,
            cumulative_framed_bytes,
            limits.max_cumulative_framed_bytes,
        )?;
        let key = Key::new(phase, priority, tiebreak);

        let outcome_ordinal = r.read_uint().map_err(BundleDecodeError::from)?;
        // ADR-0026 §5: decode admits Derived ONLY. Every other outcome and every
        // unknown ordinal is a format refusal at decode time.
        if outcome_ordinal != 2 {
            return Err(BundleDecodeError::ObservationNotDerived {
                found: outcome_ordinal,
            });
        }
        let judgement_digest = read_digest_framed(
            r,
            cumulative_framed_bytes,
            limits.max_cumulative_framed_bytes,
        )?;
        let observation = Observation {
            outcome_class: Outcome::Derived,
            judgement_digest,
        };

        let g_count_u64 = r.read_uint().map_err(BundleDecodeError::from)?;
        let g_count = usize::try_from(g_count_u64).map_err(|_| BundleDecodeError::CountOverflow)?;
        if g_count > limits.max_generators_per_step {
            return Err(BundleDecodeError::GeneratorsPerStepExceeded {
                limit: limits.max_generators_per_step,
                found: g_count,
            });
        }
        *cumulative_links = cumulative_links
            .checked_add(g_count)
            .ok_or(BundleDecodeError::CumulativeLinksOverflow)?;
        if *cumulative_links > limits.max_cumulative_links {
            return Err(BundleDecodeError::CumulativeLinksExceeded {
                limit: limits.max_cumulative_links,
                found: *cumulative_links,
            });
        }

        let mut generators = Vec::with_capacity(g_count);
        for _ in 0..g_count {
            let d = read_id_from_list_item(
                r,
                cumulative_framed_bytes,
                limits.max_cumulative_framed_bytes,
            )?;
            generators.push(GeneratorId(d));
        }

        let c_count_u64 = r.read_uint().map_err(BundleDecodeError::from)?;
        let c_count = usize::try_from(c_count_u64).map_err(|_| BundleDecodeError::CountOverflow)?;
        if c_count > limits.max_configs_per_step {
            return Err(BundleDecodeError::ConfigsPerStepExceeded {
                limit: limits.max_configs_per_step,
                found: c_count,
            });
        }
        // ADR-0026 §6: require configs == generators + 1 before constructing vectors
        if c_count != g_count + 1 {
            return Err(BundleDecodeError::ConfigGeneratorMismatch {
                generators: g_count,
                configs: c_count,
            });
        }

        let mut configs = Vec::with_capacity(c_count);
        for _ in 0..c_count {
            let d = read_id_from_list_item(
                r,
                cumulative_framed_bytes,
                limits.max_cumulative_framed_bytes,
            )?;
            configs.push(ConfigId(d));
        }

        let src = ConfigId(read_digest_framed(
            r,
            cumulative_framed_bytes,
            limits.max_cumulative_framed_bytes,
        )?);
        let dst = ConfigId(read_digest_framed(
            r,
            cumulative_framed_bytes,
            limits.max_cumulative_framed_bytes,
        )?);
        let witness = WitnessId(read_digest_framed(
            r,
            cumulative_framed_bytes,
            limits.max_cumulative_framed_bytes,
        )?);

        Ok(RecordedStepMaterialV1 {
            key,
            observation,
            generators,
            configs,
            src,
            dst,
            witness,
        })
    }
}

impl From<&CommittedStep> for RecordedStepMaterialV1 {
    fn from(step: &CommittedStep) -> Self {
        RecordedStepMaterialV1 {
            key: step.key,
            observation: step.observation,
            generators: step.decomposition.generators().to_vec(),
            configs: step.decomposition.configs().to_vec(),
            src: step.src,
            dst: step.dst,
            witness: step.witness,
        }
    }
}

impl Canonical for RecordedStepMaterialV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.key.canon_write(w);
        self.observation.canon_write(w);
        w.write_list(self.generators.iter().map(|g| g.canon_bytes()));
        w.write_list(self.configs.iter().map(|c| c.canon_bytes()));
        self.src.canon_write(w);
        self.dst.canon_write(w);
        self.witness.canon_write(w);
    }
}

/// One step entry in a [`SettlementAuditInputBundleV1`] (ADR-0026 §5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// Step ordinal within the journal (exactly 0..n-1).
    pub ordinal: u64,
    /// The chain state immediately before this step (`History::digest`).
    pub prefix_digest: Digest,
    /// The step's verifiable material (no verification tag).
    pub step_material: RecordedStepMaterialV1,
    /// The exact canonical receipt bytes issued for this step.
    pub receipt_bytes: Vec<u8>,
}

impl Entry {
    /// Decode one entry from reader `r` (reading its length-prefixed frame).
    pub fn decode(
        r: &mut CanonReader<'_>,
        expected_ordinal: u64,
        limits: &AuditDecodeLimits,
        cumulative_links: &mut usize,
        cumulative_framed_bytes: &mut usize,
    ) -> Result<Self, BundleDecodeError> {
        let entry_frame = r.read_bytes().map_err(BundleDecodeError::from)?;
        if entry_frame.len() > limits.max_entry_bytes {
            return Err(BundleDecodeError::EntryBytesExceeded {
                limit: limits.max_entry_bytes,
                found: entry_frame.len(),
            });
        }
        *cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(entry_frame.len())
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if *cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: *cumulative_framed_bytes,
            });
        }

        let mut er = CanonReader::new(entry_frame);
        let ordinal = er.read_uint().map_err(BundleDecodeError::from)?;
        if ordinal != expected_ordinal {
            return Err(BundleDecodeError::OrdinalMismatch {
                expected: expected_ordinal,
                found: ordinal,
            });
        }

        let prefix_digest = read_digest_framed(
            &mut er,
            cumulative_framed_bytes,
            limits.max_cumulative_framed_bytes,
        )?;
        let step_material = RecordedStepMaterialV1::decode(
            &mut er,
            limits,
            cumulative_links,
            cumulative_framed_bytes,
        )?;

        let receipt_frame = er.read_bytes().map_err(BundleDecodeError::from)?;
        if receipt_frame.len() > limits.max_receipt_bytes {
            return Err(BundleDecodeError::ReceiptBytesExceeded {
                limit: limits.max_receipt_bytes,
                found: receipt_frame.len(),
            });
        }
        *cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(receipt_frame.len())
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if *cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: *cumulative_framed_bytes,
            });
        }
        let receipt_bytes = receipt_frame.to_vec();

        // ADR-0026 §6: fully consume every nested frame
        if !er.is_empty() {
            return Err(BundleDecodeError::TrailingBytesInEntry);
        }

        Ok(Entry {
            ordinal,
            prefix_digest,
            step_material,
            receipt_bytes,
        })
    }
}

impl Canonical for Entry {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.ordinal);
        w.write_bytes(self.prefix_digest.as_bytes());
        self.step_material.canon_write(w);
        w.write_bytes(&self.receipt_bytes);
    }
}

/// The settlement audit input bundle (ADR-0026 §5, ⟨D-BUNDLE⟩).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettlementAuditInputBundleV1 {
    pub context: ContextId,
    pub entries: Vec<Entry>,
    pub final_chain_digest: Digest,
}

impl SettlementAuditInputBundleV1 {
    /// Content-addressed identity of this bundle.
    pub fn id(&self) -> SettlementAuditInputBundleIdV1 {
        SettlementAuditInputBundleIdV1::of(self)
    }

    /// Validate the complete snapshot folding from [`History::empty`] (ADR-0026 §3).
    pub fn validate_snapshot(&self) -> Result<History, SnapshotValidationError> {
        validate_audit_input_bundle_snapshot_v1(self)
    }

    /// Encode this bundle into canonical bytes under `limits` (ADR-0026 §6).
    pub fn encode(&self, limits: &AuditDecodeLimits) -> Result<Vec<u8>, BundleDecodeError> {
        encode_audit_input_bundle_v1(self, limits)
    }
}

impl Canonical for SettlementAuditInputBundleV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(BUNDLE_MARKER_V1);
        w.write_uint(BUNDLE_VERSION_V1);
        w.write_str(BUNDLE_PROFILE_V1);
        self.context.canon_write(w);
        w.write_list(self.entries.iter().map(|e| e.canon_bytes()));
        w.write_bytes(self.final_chain_digest.as_bytes());
    }
}

/// Content-addressed identity of a [`SettlementAuditInputBundleV1`] (ADR-0026 §5).
///
/// # Compile-fail gate
///
/// Accepts only [`SettlementAuditInputBundleV1`], not arbitrary Canonical values.
///
/// ```compile_fail
/// use soc_core::audit_bundle::SettlementAuditInputBundleIdV1;
/// use soc_core::calendar::Key;
/// fn cannot_hash_arbitrary_canonical(k: &Key) {
///     let _ = SettlementAuditInputBundleIdV1::of(k);
/// }
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SettlementAuditInputBundleIdV1(pub Digest);

impl SettlementAuditInputBundleIdV1 {
    pub fn of(bundle: &SettlementAuditInputBundleV1) -> Self {
        let mut w = CanonWriter::new();
        bundle.canon_write(&mut w);
        SettlementAuditInputBundleIdV1(Digest::of(Domain::Value, &w.finish()))
    }

    pub fn from_digest(d: Digest) -> Self {
        Self(d)
    }

    pub fn digest(&self) -> Digest {
        self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl Canonical for SettlementAuditInputBundleIdV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

fn read_digest_framed(
    r: &mut CanonReader<'_>,
    cumulative_framed_bytes: &mut usize,
    max_cumulative_framed_bytes: usize,
) -> Result<Digest, BundleDecodeError> {
    let bytes = r.read_bytes().map_err(BundleDecodeError::from)?;
    *cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(bytes.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if *cumulative_framed_bytes > max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: max_cumulative_framed_bytes,
            found: *cumulative_framed_bytes,
        });
    }
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| BundleDecodeError::BadDigestLength(bytes.len()))?;
    Ok(Digest::from_bytes(array))
}

fn read_id_from_list_item(
    r: &mut CanonReader<'_>,
    cumulative_framed_bytes: &mut usize,
    max_cumulative_framed_bytes: usize,
) -> Result<Digest, BundleDecodeError> {
    let item = r.read_bytes().map_err(BundleDecodeError::from)?;
    *cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(item.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if *cumulative_framed_bytes > max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: max_cumulative_framed_bytes,
            found: *cumulative_framed_bytes,
        });
    }

    let mut ir = CanonReader::new(item);
    let bytes = ir.read_bytes().map_err(BundleDecodeError::from)?;
    *cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(bytes.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if *cumulative_framed_bytes > max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: max_cumulative_framed_bytes,
            found: *cumulative_framed_bytes,
        });
    }
    if !ir.is_empty() {
        return Err(BundleDecodeError::TrailingBytesInEntry);
    }
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| BundleDecodeError::BadDigestLength(bytes.len()))?;
    Ok(Digest::from_bytes(array))
}

/// Canonically decode untrusted bytes into a [`SettlementAuditInputBundleV1`]
/// under the given limits (ADR-0026 §6).
///
/// Enforces every limit **before** the governed hashing, allocation, copy, or loop.
pub fn decode_audit_input_bundle_v1(
    bytes: &[u8],
    limits: &AuditDecodeLimits,
) -> Result<SettlementAuditInputBundleV1, BundleDecodeError> {
    if bytes.len() > limits.max_total_bundle_bytes {
        return Err(BundleDecodeError::TotalBundleBytesExceeded {
            limit: limits.max_total_bundle_bytes,
            found: bytes.len(),
        });
    }

    let mut cumulative_framed_bytes = 0usize;
    let mut r = CanonReader::new(bytes);

    let marker = r.read_bytes().map_err(BundleDecodeError::from)?;
    cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(marker.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: limits.max_cumulative_framed_bytes,
            found: cumulative_framed_bytes,
        });
    }
    if marker != BUNDLE_MARKER_V1 {
        return Err(BundleDecodeError::BadMarker);
    }

    let version = r.read_uint().map_err(BundleDecodeError::from)?;
    if version != BUNDLE_VERSION_V1 {
        return Err(BundleDecodeError::UnknownVersion(version));
    }

    let profile = r.read_bytes().map_err(BundleDecodeError::from)?;
    cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(profile.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: limits.max_cumulative_framed_bytes,
            found: cumulative_framed_bytes,
        });
    }
    if profile != BUNDLE_PROFILE_V1.as_bytes() {
        return Err(BundleDecodeError::UnknownProfile);
    }

    let context_digest = read_digest_framed(
        &mut r,
        &mut cumulative_framed_bytes,
        limits.max_cumulative_framed_bytes,
    )?;
    let context = ContextId(context_digest);

    let steps_count_u64 = r.read_uint().map_err(BundleDecodeError::from)?;
    let steps_count =
        usize::try_from(steps_count_u64).map_err(|_| BundleDecodeError::CountOverflow)?;
    if steps_count > limits.max_steps {
        return Err(BundleDecodeError::StepsExceeded {
            limit: limits.max_steps,
            found: steps_count,
        });
    }

    let mut cumulative_links = 0usize;
    let mut entries = Vec::with_capacity(steps_count);
    for i in 0..steps_count {
        let entry = Entry::decode(
            &mut r,
            i as u64,
            limits,
            &mut cumulative_links,
            &mut cumulative_framed_bytes,
        )?;
        entries.push(entry);
    }

    let final_chain_digest = read_digest_framed(
        &mut r,
        &mut cumulative_framed_bytes,
        limits.max_cumulative_framed_bytes,
    )?;

    // ADR-0026 §6: reject outer trailing bytes
    if !r.is_empty() {
        return Err(BundleDecodeError::TrailingBytes);
    }

    Ok(SettlementAuditInputBundleV1 {
        context,
        entries,
        final_chain_digest,
    })
}

/// Validate a complete snapshot by folding from [`History::empty`] (ADR-0026 §3).
///
/// Requires ordinals to be exactly 0..n-1, recomputes every prefix digest,
/// and requires the recomputed final digest to equal the bundle's.
pub fn validate_audit_input_bundle_snapshot_v1(
    bundle: &SettlementAuditInputBundleV1,
) -> Result<History, SnapshotValidationError> {
    let mut chain = History::empty();

    for (i, entry) in bundle.entries.iter().enumerate() {
        let expected_ordinal = i as u64;
        if entry.ordinal != expected_ordinal {
            return Err(SnapshotValidationError::OrdinalMismatch {
                expected: expected_ordinal,
                found: entry.ordinal,
            });
        }

        if entry.prefix_digest != chain.digest() {
            return Err(SnapshotValidationError::PrefixDigestMismatch {
                ordinal: entry.ordinal,
                expected: chain.digest(),
                found: entry.prefix_digest,
            });
        }

        let step = entry
            .step_material
            .to_committed_step()
            .map_err(SnapshotValidationError::Decomposition)?;
        chain = chain.append(&step);
    }

    if chain.digest() != bundle.final_chain_digest {
        return Err(SnapshotValidationError::FinalChainDigestMismatch {
            expected: chain.digest(),
            found: bundle.final_chain_digest,
        });
    }

    Ok(chain)
}

/// Validate all eight [`AuditDecodeLimits`] on a [`SettlementAuditInputBundleV1`] (ADR-0026 §6).
///
/// Ensures that typed construction cannot bypass decode bounds in the public
/// checker, producer, or encoder. Every bound is enforced before the work it
/// governs.
pub fn validate_bundle_limits(
    bundle: &SettlementAuditInputBundleV1,
    limits: &AuditDecodeLimits,
) -> Result<(), BundleDecodeError> {
    if bundle.entries.len() > limits.max_steps {
        return Err(BundleDecodeError::StepsExceeded {
            limit: limits.max_steps,
            found: bundle.entries.len(),
        });
    }

    let mut cumulative_links = 0usize;
    let mut cumulative_framed_bytes = 0usize;

    // Header framed bytes: marker (27), profile (29), context (32)
    cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(BUNDLE_MARKER_V1.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: limits.max_cumulative_framed_bytes,
            found: cumulative_framed_bytes,
        });
    }

    cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(BUNDLE_PROFILE_V1.len())
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: limits.max_cumulative_framed_bytes,
            found: cumulative_framed_bytes,
        });
    }

    cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(32) // context digest
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: limits.max_cumulative_framed_bytes,
            found: cumulative_framed_bytes,
        });
    }

    for entry in &bundle.entries {
        if entry.step_material.observation.outcome_class != Outcome::Derived {
            return Err(BundleDecodeError::ObservationNotDerived {
                found: entry.step_material.observation.outcome_class as u64,
            });
        }

        let g_count = entry.step_material.generators.len();
        if g_count > limits.max_generators_per_step {
            return Err(BundleDecodeError::GeneratorsPerStepExceeded {
                limit: limits.max_generators_per_step,
                found: g_count,
            });
        }

        cumulative_links = cumulative_links
            .checked_add(g_count)
            .ok_or(BundleDecodeError::CumulativeLinksOverflow)?;
        if cumulative_links > limits.max_cumulative_links {
            return Err(BundleDecodeError::CumulativeLinksExceeded {
                limit: limits.max_cumulative_links,
                found: cumulative_links,
            });
        }

        let c_count = entry.step_material.configs.len();
        if c_count > limits.max_configs_per_step {
            return Err(BundleDecodeError::ConfigsPerStepExceeded {
                limit: limits.max_configs_per_step,
                found: c_count,
            });
        }

        if c_count != g_count + 1 {
            return Err(BundleDecodeError::ConfigGeneratorMismatch {
                generators: g_count,
                configs: c_count,
            });
        }

        if entry.receipt_bytes.len() > limits.max_receipt_bytes {
            return Err(BundleDecodeError::ReceiptBytesExceeded {
                limit: limits.max_receipt_bytes,
                found: entry.receipt_bytes.len(),
            });
        }

        let entry_bytes_len = entry.canon_bytes().len();
        if entry_bytes_len > limits.max_entry_bytes {
            return Err(BundleDecodeError::EntryBytesExceeded {
                limit: limits.max_entry_bytes,
                found: entry_bytes_len,
            });
        }

        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(entry_bytes_len)
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        // Inside entry frame: prefix_digest (32), tiebreak (32), judgement_digest (32)
        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(32) // prefix_digest
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(32) // tiebreak
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(32) // judgement_digest
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        let gen_bytes = g_count
            .checked_mul(34 + 32)
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(gen_bytes)
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        let cfg_bytes = c_count
            .checked_mul(34 + 32)
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(cfg_bytes)
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        // src, dst, witness (3 * 32 = 96)
        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(96)
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }

        cumulative_framed_bytes = cumulative_framed_bytes
            .checked_add(entry.receipt_bytes.len())
            .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
        if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
            return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: limits.max_cumulative_framed_bytes,
                found: cumulative_framed_bytes,
            });
        }
    }

    // Footer: final_chain_digest (32)
    cumulative_framed_bytes = cumulative_framed_bytes
        .checked_add(32)
        .ok_or(BundleDecodeError::CumulativeFramedBytesOverflow)?;
    if cumulative_framed_bytes > limits.max_cumulative_framed_bytes {
        return Err(BundleDecodeError::CumulativeFramedBytesExceeded {
            limit: limits.max_cumulative_framed_bytes,
            found: cumulative_framed_bytes,
        });
    }

    let total_bytes = bundle.canon_bytes().len();
    if total_bytes > limits.max_total_bundle_bytes {
        return Err(BundleDecodeError::TotalBundleBytesExceeded {
            limit: limits.max_total_bundle_bytes,
            found: total_bytes,
        });
    }

    Ok(())
}

/// Encode a [`SettlementAuditInputBundleV1`] into canonical bytes, enforcing
/// all [`AuditDecodeLimits`] (ADR-0026 §6).
///
/// Validates bounds before returning the byte payload. Used by CLI bundle emission.
pub fn encode_audit_input_bundle_v1(
    bundle: &SettlementAuditInputBundleV1,
    limits: &AuditDecodeLimits,
) -> Result<Vec<u8>, BundleDecodeError> {
    validate_bundle_limits(bundle, limits)?;
    Ok(bundle.canon_bytes())
}

/// Validate a bundle's complete snapshot and verify every receipt by local replay
/// (ADR-0026 Stages B–D).
///
/// Limit enforcement ordering: bounds fire before allocation, copying, or looping.
/// Verifier does not bypass decode limits even when consuming a bundle constructed
/// via public typed constructors.
pub fn check_audit_input_bundle_v1(
    bundle: &SettlementAuditInputBundleV1,
    expected_registry: &GeneratorRegistry,
    expected_semantics: &GeneratorSemanticsV1,
    limits: &AuditDecodeLimits,
) -> Result<Vec<SettlementAuditReceiptIdV1>, BundleCheckError> {
    validate_bundle_limits(bundle, limits).map_err(BundleCheckError::LimitExceeded)?;

    validate_audit_input_bundle_snapshot_v1(bundle)
        .map_err(BundleCheckError::SnapshotValidation)?;

    let mut receipt_ids = Vec::with_capacity(bundle.entries.len());
    for entry in &bundle.entries {
        let step = entry.step_material.to_committed_step().map_err(|e| {
            BundleCheckError::Decomposition {
                ordinal: entry.ordinal,
                error: e,
            }
        })?;
        let receipt_id = check_audit_receipt_bytes_v1(
            &entry.receipt_bytes,
            &step,
            bundle.context,
            expected_registry,
            expected_semantics,
            limits,
        )
        .map_err(|e| BundleCheckError::Receipt {
            ordinal: entry.ordinal,
            error: e,
        })?;
        receipt_ids.push(receipt_id);
    }

    Ok(receipt_ids)
}

/// Emit a complete, audited [`SettlementAuditInputBundleV1`] for `journal`
/// under explicit [`AuditDecodeLimits`] (ADR-0026 §8).
///
/// A bundle is emitted only when:
/// 1. Every step returns [`AuditResult::Audited`].
/// 2. Every step's observation is [`Outcome::Derived`].
/// 3. Prefix and final digests recomputed from [`History::empty`] match the journal.
/// 4. Decode bounds are satisfied (bounds fire before allocation/loop).
///
/// If any step yields [`AuditResult::Unknown`], NO bundle is produced.
pub fn produce_audit_input_bundle_with_limits_v1(
    journal: &Journal,
    context: ContextId,
    registry: &GeneratorRegistry,
    semantics: &GeneratorSemanticsV1,
    limits: &AuditDecodeLimits,
) -> Result<SettlementAuditInputBundleV1, BundleProducerError> {
    if journal.steps().len() > limits.max_steps {
        return Err(BundleProducerError::LimitExceeded(
            BundleDecodeError::StepsExceeded {
                limit: limits.max_steps,
                found: journal.steps().len(),
            },
        ));
    }

    let mut cumulative_links = 0usize;
    for (i, step) in journal.steps().iter().enumerate() {
        if step.observation.outcome_class != Outcome::Derived {
            return Err(BundleProducerError::ObservationNotDerived {
                ordinal: i as u64,
                found: step.observation.outcome_class,
            });
        }

        let g_count = step.decomposition.generators().len();
        if g_count > limits.max_generators_per_step {
            return Err(BundleProducerError::LimitExceeded(
                BundleDecodeError::GeneratorsPerStepExceeded {
                    limit: limits.max_generators_per_step,
                    found: g_count,
                },
            ));
        }

        cumulative_links =
            cumulative_links
                .checked_add(g_count)
                .ok_or(BundleProducerError::LimitExceeded(
                    BundleDecodeError::CumulativeLinksOverflow,
                ))?;
        if cumulative_links > limits.max_cumulative_links {
            return Err(BundleProducerError::LimitExceeded(
                BundleDecodeError::CumulativeLinksExceeded {
                    limit: limits.max_cumulative_links,
                    found: cumulative_links,
                },
            ));
        }

        let c_count = step.decomposition.configs().len();
        if c_count > limits.max_configs_per_step {
            return Err(BundleProducerError::LimitExceeded(
                BundleDecodeError::ConfigsPerStepExceeded {
                    limit: limits.max_configs_per_step,
                    found: c_count,
                },
            ));
        }

        if c_count != g_count + 1 {
            return Err(BundleProducerError::LimitExceeded(
                BundleDecodeError::ConfigGeneratorMismatch {
                    generators: g_count,
                    configs: c_count,
                },
            ));
        }
    }

    let mut chain = History::empty();
    let mut entries = Vec::with_capacity(journal.steps().len());

    for (i, step) in journal.steps().iter().enumerate() {
        let prefix_digest = chain.digest();

        let audited = match audit_step(step, context, registry, semantics) {
            AuditResult::Audited(a) => a,
            AuditResult::Unknown(reason) => {
                return Err(BundleProducerError::StepAuditUnknown {
                    ordinal: i as u64,
                    reason,
                });
            }
        };

        let step_material = RecordedStepMaterialV1::from(step);
        let receipt_bytes = audited.receipt.canon_bytes();

        if receipt_bytes.len() > limits.max_receipt_bytes {
            return Err(BundleProducerError::LimitExceeded(
                BundleDecodeError::ReceiptBytesExceeded {
                    limit: limits.max_receipt_bytes,
                    found: receipt_bytes.len(),
                },
            ));
        }

        let entry = Entry {
            ordinal: i as u64,
            prefix_digest,
            step_material,
            receipt_bytes,
        };

        let entry_bytes_len = entry.canon_bytes().len();
        if entry_bytes_len > limits.max_entry_bytes {
            return Err(BundleProducerError::LimitExceeded(
                BundleDecodeError::EntryBytesExceeded {
                    limit: limits.max_entry_bytes,
                    found: entry_bytes_len,
                },
            ));
        }

        entries.push(entry);
        chain = chain.append(step);
    }

    let final_chain_digest = chain.digest();
    if final_chain_digest != journal.chain_digest() {
        return Err(BundleProducerError::ChainDigestMismatch {
            expected: journal.chain_digest(),
            recomputed: final_chain_digest,
        });
    }

    let bundle = SettlementAuditInputBundleV1 {
        context,
        entries,
        final_chain_digest,
    };

    validate_bundle_limits(&bundle, limits).map_err(BundleProducerError::LimitExceeded)?;

    Ok(bundle)
}

/// Emit a complete, audited [`SettlementAuditInputBundleV1`] for `journal`
/// (ADR-0026 §8).
///
/// A bundle is emitted only when:
/// 1. Every step returns [`AuditResult::Audited`].
/// 2. Every step's observation is [`Outcome::Derived`].
/// 3. Prefix and final digests recomputed from [`History::empty`] match the journal.
/// 4. Decode bounds are satisfied (bounds fire before allocation/loop).
///
/// If any step yields [`AuditResult::Unknown`], NO bundle is produced.
pub fn produce_audit_input_bundle_v1(
    journal: &Journal,
    context: ContextId,
    registry: &GeneratorRegistry,
    semantics: &GeneratorSemanticsV1,
) -> Result<SettlementAuditInputBundleV1, BundleProducerError> {
    produce_audit_input_bundle_with_limits_v1(
        journal,
        context,
        registry,
        semantics,
        &AuditDecodeLimits::strict(),
    )
}

/// Errors during bundle decoding (ADR-0026 §6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BundleDecodeError {
    TotalBundleBytesExceeded { limit: usize, found: usize },
    StepsExceeded { limit: usize, found: usize },
    EntryBytesExceeded { limit: usize, found: usize },
    ReceiptBytesExceeded { limit: usize, found: usize },
    GeneratorsPerStepExceeded { limit: usize, found: usize },
    ConfigsPerStepExceeded { limit: usize, found: usize },
    CumulativeLinksExceeded { limit: usize, found: usize },
    CumulativeFramedBytesExceeded { limit: usize, found: usize },
    CumulativeLinksOverflow,
    CumulativeFramedBytesOverflow,
    CountOverflow,
    BadMarker,
    UnknownVersion(u64),
    UnknownProfile,
    BadDigestLength(usize),
    ObservationNotDerived { found: u64 },
    ConfigGeneratorMismatch { generators: usize, configs: usize },
    OrdinalMismatch { expected: u64, found: u64 },
    TrailingBytesInEntry,
    TrailingBytes,
    UnexpectedEof,
    NonMinimalInt,
    BadLength,
}

impl From<CanonError> for BundleDecodeError {
    fn from(e: CanonError) -> Self {
        match e {
            CanonError::UnexpectedEof => BundleDecodeError::UnexpectedEof,
            CanonError::NonMinimalInt => BundleDecodeError::NonMinimalInt,
            CanonError::BadLength => BundleDecodeError::BadLength,
        }
    }
}

/// Errors during complete snapshot validation (ADR-0026 §3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SnapshotValidationError {
    OrdinalMismatch {
        expected: u64,
        found: u64,
    },
    PrefixDigestMismatch {
        ordinal: u64,
        expected: Digest,
        found: Digest,
    },
    FinalChainDigestMismatch {
        expected: Digest,
        found: Digest,
    },
    Decomposition(DecompositionError),
}

/// Errors during checked bundle production (ADR-0026 §8).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BundleProducerError {
    StepAuditUnknown {
        ordinal: u64,
        reason: &'static str,
    },
    ObservationNotDerived {
        ordinal: u64,
        found: Outcome,
    },
    ChainDigestMismatch {
        expected: Digest,
        recomputed: Digest,
    },
    LimitExceeded(BundleDecodeError),
}

/// Errors during full bundle verification (snapshot + receipts).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BundleCheckError {
    SnapshotValidation(SnapshotValidationError),
    Receipt {
        ordinal: u64,
        error: ReceiptError,
    },
    Decomposition {
        ordinal: u64,
        error: DecompositionError,
    },
    LimitExceeded(BundleDecodeError),
}
