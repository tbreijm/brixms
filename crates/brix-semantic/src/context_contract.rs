//! Canonical context envelope, structural validation, and identity correspondence (Issue #59).
//!
//! Governed by `spec/Context_Contract.md`.
//!
//! Implements the first bounded schema slice:
//! - Pinned canonical envelope marker and format version
//! - Identity correspondence to [`ContextId`] under [`Domain::Value`]
//! - Preservation of the frozen root digest [`ContextId::root`] and [`ContextId::extend`]
//! - Structured core profile: `brix.context.core@1`
//! - Fail-closed, bounded decoding and structural/ancestry validation against an independently expected [`ContextId`].
//!
//! Validation establishes canonical structure, content identity, and (when the resolver API is
//! used) ancestry and mode consistency. It does not type-check assumption payloads or authorize
//! evidence based on referenced program/world revisions.

use std::collections::BTreeSet;

use brix_canon::{CanonError, CanonReader, CanonWriter, Digest};

use crate::context::ContextId;

/// Pinned envelope marker opening every modern canonical context preimage.
pub const CONTEXT_MARKER: &[u8] = b"brix.context.envelope";

/// Envelope format version 1. Frozen ABI.
pub const CONTEXT_FORMAT_V1: u64 = 1;

/// The core context profile (Issue #59 first supported profile).
pub const CONTEXT_PROFILE_CORE_V1: &str = "brix.context.core@1";

/// Canonical list of supported regime profiles for `brix.context.core@1`.
pub const SUPPORTED_REGIME_PROFILES: &[&str] =
    &["brix.kernel.profile@1.2", "brix.type@1", "brix.test@1"];

/// Legacy root tag (frozen ABI, matches `ScopeId::root()` verbatim).
pub const LEGACY_ROOT_TAG: &str = "brix.ir.reflect.ScopeId.root";

/// Legacy extend tag (frozen ABI).
pub const LEGACY_EXTEND_TAG: &str = "brix.semantic.ContextId.extend";

/// Maximum size in bytes of a context envelope payload.
pub const MAX_CONTEXT_BYTES: usize = 65_536; // 64 KiB

/// Maximum number of assumptions in a single context level.
pub const MAX_CONTEXT_ASSUMPTIONS: usize = 256;

/// Maximum length of an assumption variable name.
pub const MAX_ASSUMPTION_NAME_BYTES: usize = 64;

/// Maximum size of an assumption's serialized canonical payload.
pub const MAX_ASSUMPTION_VALUE_BYTES: usize = 4_096;

/// Maximum parent context recursion depth.
pub const MAX_CONTEXT_DEPTH: usize = 128;

/// Maximum length of a regime profile string identifier.
pub const MAX_REGIME_PROFILE_BYTES: usize = 128;

/// Maximum permissible step budget for core context (prevents arithmetic overflow).
pub const MAX_ALLOWED_STEP_BUDGET: u64 = 1_000_000_000;

/// Maximum permissible depth budget for core context (prevents arithmetic overflow).
pub const MAX_ALLOWED_DEPTH_BUDGET: u64 = 100_000;

/// Resource bounds for decoding and validating context envelopes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContextBudget {
    /// Maximum allowed payload size in bytes.
    pub max_bytes: usize,
    /// Maximum allowed assumption count.
    pub max_assumptions: usize,
    /// Maximum allowed parent context recursion depth.
    pub max_depth: usize,
}

impl ContextBudget {
    /// Strict production defaults.
    pub const fn strict() -> Self {
        Self {
            max_bytes: MAX_CONTEXT_BYTES,
            max_assumptions: MAX_CONTEXT_ASSUMPTIONS,
            max_depth: MAX_CONTEXT_DEPTH,
        }
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::strict()
    }
}

/// Execution scenario mode: production vs. simulation/counterfactual.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ExecutionMode {
    /// Production execution.
    Production = 0,
    /// Counterfactual / simulation execution. Evidence cannot cross to production without bridge.
    Simulation = 1,
}

impl ExecutionMode {
    /// Canonical ABI ordinal.
    pub const fn ordinal(self) -> u64 {
        match self {
            ExecutionMode::Production => 0,
            ExecutionMode::Simulation => 1,
        }
    }

    /// Decode canonical ABI ordinal.
    pub fn from_ordinal(ord: u64) -> Result<Self, ContextValidationError> {
        match ord {
            0 => Ok(ExecutionMode::Production),
            1 => Ok(ExecutionMode::Simulation),
            other => Err(ContextValidationError::UnknownExecutionMode(other)),
        }
    }
}

/// A single named, typed assumption within a context scope.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ContextAssumptionV1 {
    /// Variable or hypothesis identifier.
    pub name: String,
    /// Opaque framed bytes for an assumed type or proposition. This layer checks
    /// only the framing and boundedness; it does not type-check the payload.
    pub payload: Vec<u8>,
}

impl ContextAssumptionV1 {
    /// Create a new assumption binding.
    pub fn new(name: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            payload: payload.into(),
        }
    }
}

/// Materialized payload for context profile `brix.context.core@1`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MaterializedCoreContextV1 {
    /// Optional parent context identity.
    pub parent: Option<ContextId>,
    /// Optional program revision reference.
    pub program_revision: Option<Digest>,
    /// Optional world revision reference.
    pub world_revision: Option<Digest>,
    /// Ordered list of distinct local assumptions.
    pub assumptions: Vec<ContextAssumptionV1>,
    /// Calculus / typing regime profile name (e.g. "brix.kernel.profile@1.2").
    pub regime_profile: String,
    /// Execution mode (production vs simulation).
    pub execution_mode: ExecutionMode,
    /// Step evaluation limit.
    pub step_budget: u64,
    /// Recursion depth limit.
    pub depth_budget: u64,
}

/// Variants of valid context payloads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ContextPayloadV1 {
    /// Legacy root context: empty assumptions, default profile, frozen root digest.
    LegacyRoot,
    /// Structured core context profile (`brix.context.core@1`).
    Core(MaterializedCoreContextV1),
    /// Legacy extension wrapper over raw parent digest and assumption bytes.
    LegacyExtend {
        parent: ContextId,
        assumption: Vec<u8>,
    },
}

/// An authenticated context envelope whose cryptographic identity and local
/// syntactic structure have been verified, but whose parent chain has not been resolved.
///
/// **Sealed boundary:** Returned by [`authenticate_context_envelope_v1`].
/// Does not guarantee ancestry soundness or permit asserting production mode
/// when the parent is unknown or unverified.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuthenticatedContextEnvelopeV1 {
    id: ContextId,
    profile: String,
    payload: ContextPayloadV1,
}

impl AuthenticatedContextEnvelopeV1 {
    /// The verified, content-addressed identity.
    pub fn id(&self) -> ContextId {
        self.id
    }

    /// The context profile identifier.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The inner payload.
    pub fn payload(&self) -> &ContextPayloadV1 {
        &self.payload
    }

    /// Returns `true` if this is the root context.
    pub fn is_root(&self) -> bool {
        match &self.payload {
            ContextPayloadV1::LegacyRoot => true,
            ContextPayloadV1::Core(core) => core.parent.is_none() && core.assumptions.is_empty(),
            ContextPayloadV1::LegacyExtend { .. } => false,
        }
    }

    /// Optional parent context identity.
    pub fn parent(&self) -> Option<ContextId> {
        match &self.payload {
            ContextPayloadV1::Core(core) => core.parent,
            ContextPayloadV1::LegacyExtend { parent, .. } => Some(*parent),
            ContextPayloadV1::LegacyRoot => None,
        }
    }

    /// The execution mode explicitly declared by this single envelope, if any.
    ///
    /// For LegacyExtend, returns `None` because the mode is inherited from its parent.
    pub fn declared_mode(&self) -> Option<ExecutionMode> {
        match &self.payload {
            ContextPayloadV1::Core(core) => Some(core.execution_mode),
            ContextPayloadV1::LegacyRoot => Some(ExecutionMode::Production),
            ContextPayloadV1::LegacyExtend { .. } => None,
        }
    }

    /// Borrow local assumptions if in core profile.
    pub fn assumptions(&self) -> &[ContextAssumptionV1] {
        match &self.payload {
            ContextPayloadV1::Core(core) => &core.assumptions,
            _ => &[],
        }
    }
}

/// A structurally validated context whose content-addressed identity,
/// well-formed envelope, and parent ancestry chain up to root have been
/// checked against cycles, identity mismatches, and simulation leakage.
/// This handle does not establish typed assumption or profile semantics,
/// availability of referenced program/world artifacts, or authorization.
///
/// **Sealed boundary:** Private fields ensure this struct can only be obtained
/// through [`validate_root_context_v1`] (for root contexts) or
/// [`validate_context_with_parent_resolver`] (for full ancestry chains).
///
/// ```compile_fail
/// use brix_semantic::{ContextId, ContextPayloadV1, ExecutionMode, ValidatedContextV1};
/// let _forged = ValidatedContextV1::for_test(
///     ContextId::root(), "brix.test@1", ContextPayloadV1::LegacyRoot,
///     ExecutionMode::Production,
/// );
/// ```
///
/// ```compile_fail
/// use brix_semantic::{ContextId, ContextPayloadV1, ExecutionMode, ValidatedContextV1};
/// let _forged = ValidatedContextV1 {
///     id: ContextId::root(),
///     profile: String::new(),
///     payload: ContextPayloadV1::LegacyRoot,
///     execution_mode: ExecutionMode::Production,
/// };
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidatedContextV1 {
    id: ContextId,
    profile: String,
    payload: ContextPayloadV1,
    execution_mode: ExecutionMode,
}

impl ValidatedContextV1 {
    /// The verified, content-addressed identity.
    pub fn id(&self) -> ContextId {
        self.id
    }

    /// The validated context profile identifier.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The inner payload.
    pub fn payload(&self) -> &ContextPayloadV1 {
        &self.payload
    }

    /// Returns `true` if this is the root context (legacy or core with no parent/assumptions).
    pub fn is_root(&self) -> bool {
        match &self.payload {
            ContextPayloadV1::LegacyRoot => true,
            ContextPayloadV1::Core(core) => core.parent.is_none() && core.assumptions.is_empty(),
            ContextPayloadV1::LegacyExtend { .. } => false,
        }
    }

    /// Returns `true` if this context is marked for simulation.
    pub fn is_simulation(&self) -> bool {
        self.execution_mode == ExecutionMode::Simulation
    }

    /// The verified execution mode of this context.
    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    /// Require that this context matches the expected execution mode.
    ///
    /// Fails closed with [`ContextValidationError::ModeMismatch`] if a simulation context
    /// is presented where production is required.
    pub fn require_mode(&self, expected: ExecutionMode) -> Result<(), ContextValidationError> {
        if self.execution_mode != expected {
            Err(ContextValidationError::ModeMismatch {
                claimed: self.execution_mode,
                expected,
            })
        } else {
            Ok(())
        }
    }

    /// Borrow local assumptions if in core profile.
    pub fn assumptions(&self) -> &[ContextAssumptionV1] {
        match &self.payload {
            ContextPayloadV1::Core(core) => &core.assumptions,
            _ => &[],
        }
    }

    /// Optional parent context identity.
    pub fn parent(&self) -> Option<ContextId> {
        match &self.payload {
            ContextPayloadV1::Core(core) => core.parent,
            ContextPayloadV1::LegacyExtend { parent, .. } => Some(*parent),
            ContextPayloadV1::LegacyRoot => None,
        }
    }
}

/// Reasons why context bytes or payload failed validation.
///
/// Fail-closed: None of these variants is an epistemic `Outcome::Refuted`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ContextValidationError {
    /// Byte slice exceeded maximum budget before decoding began.
    PayloadTooLarge { length: usize, max: usize },
    /// Leading marker was not recognized.
    MalformedMarker,
    /// Format version is not supported by this build.
    UnknownVersion(u64),
    /// Envelope kind ordinal is not recognized.
    UnknownKind(u64),
    /// Profile is not supported.
    UnsupportedProfile(String),
    /// Reconstructed payload digest differs from expected ContextId.
    IdentityMismatch {
        claimed: ContextId,
        expected: ContextId,
    },
    /// Input truncated mid-field.
    Truncated,
    /// Trailing unconsumed bytes remained after envelope.
    TrailingBytes,
    /// Canonical integer was not in minimal form.
    NonMinimalInt,
    /// Length prefix was invalid or ran past the end of the buffer.
    BadLength,
    /// An assumption variable name appeared more than once in the same context scope.
    DuplicateAssumption(String),
    /// Assumption name exceeded byte limit.
    AssumptionNameTooLong { length: usize, max: usize },
    /// Assumption payload exceeded byte limit.
    AssumptionValueTooLong { length: usize, max: usize },
    /// Total assumption count exceeded allowed limit.
    ExcessiveAssumptions { count: usize, max: usize },
    /// Parent chain recursion exceeded maximum depth.
    DepthLimitExceeded { depth: usize, max: usize },
    /// A cycle was detected in the parent context hierarchy.
    CycleInParentHierarchy(ContextId),
    /// Assumption name was empty or malformed identifier.
    InvalidAssumptionName,
    /// Assumption payload was empty or not structurally valid canonical data.
    InvalidAssumptionPayload,
    /// Resource step or depth budget was zero.
    InvalidBudget,
    /// Regime profile string was empty.
    EmptyRegimeProfile,
    /// Regime profile string contained invalid characters or control bytes.
    MalformedRegimeProfile(String),
    /// Regime profile is not recognized or supported.
    UnsupportedRegimeProfile(String),
    /// Regime profile string exceeded length limit.
    RegimeProfileTooLong { length: usize, max: usize },
    /// Unknown execution mode ordinal.
    UnknownExecutionMode(u64),
    /// Simulation / production isolation boundary violated.
    ModeMismatch {
        claimed: ExecutionMode,
        expected: ExecutionMode,
    },
    /// A required parent context could not be resolved.
    UnresolvedParent(ContextId),
}

impl From<CanonError> for ContextValidationError {
    fn from(err: CanonError) -> Self {
        match err {
            CanonError::UnexpectedEof => ContextValidationError::Truncated,
            CanonError::NonMinimalInt => ContextValidationError::NonMinimalInt,
            CanonError::BadLength => ContextValidationError::BadLength,
        }
    }
}

/// Helper to read a length-framed 32-byte digest from canonical reader.
fn read_framed_digest(r: &mut CanonReader<'_>) -> Result<Digest, ContextValidationError> {
    let bytes = r.read_bytes()?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ContextValidationError::BadLength)?;
    Ok(Digest::from_bytes(array))
}

/// Encode a context payload into canonical bytes.
pub fn encode_context_envelope_v1(payload: &ContextPayloadV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    match payload {
        ContextPayloadV1::LegacyRoot => {
            // Verbatim legacy root encoding: write_tag("brix.ir.reflect.ScopeId.root")
            w.write_tag(LEGACY_ROOT_TAG);
        }
        ContextPayloadV1::LegacyExtend { parent, assumption } => {
            // Verbatim legacy extend encoding:
            // write_tag("brix.semantic.ContextId.extend") || parent || assumption
            w.write_tag(LEGACY_EXTEND_TAG);
            w.write_bytes(parent.digest().as_bytes());
            w.write_bytes(assumption);
        }
        ContextPayloadV1::Core(core) => {
            w.write_bytes(CONTEXT_MARKER);
            w.write_uint(CONTEXT_FORMAT_V1);
            w.write_str(CONTEXT_PROFILE_CORE_V1);
            // Pinned kind ordinal 1 for MaterializedCoreV1 (Spec §2.2)
            w.write_uint(1);

            // Parent Option<ContextId>
            match &core.parent {
                None => w.write_enum(0, |_| {}),
                Some(pid) => w.write_enum(1, |w| {
                    w.write_bytes(pid.digest().as_bytes());
                }),
            }

            // Program revision Option<Digest>
            match &core.program_revision {
                None => w.write_enum(0, |_| {}),
                Some(d) => w.write_enum(1, |w| {
                    w.write_bytes(d.as_bytes());
                }),
            }

            // World revision Option<Digest>
            match &core.world_revision {
                None => w.write_enum(0, |_| {}),
                Some(d) => w.write_enum(1, |w| {
                    w.write_bytes(d.as_bytes());
                }),
            }

            // Assumptions
            w.write_uint(core.assumptions.len() as u64);
            for assumption in &core.assumptions {
                w.write_str(&assumption.name);
                w.write_bytes(&assumption.payload);
            }

            // Regime profile
            w.write_str(&core.regime_profile);

            // Execution mode
            w.write_uint(core.execution_mode.ordinal());

            // Budgets
            w.write_uint(core.step_budget);
            w.write_uint(core.depth_budget);
        }
    }
    w.finish()
}

/// Compute the content-addressed [`ContextId`] of a context payload.
pub fn context_id_of_payload_v1(payload: &ContextPayloadV1) -> ContextId {
    let bytes = encode_context_envelope_v1(payload);
    ContextId::from_canon(&bytes)
}

/// Validate that an assumption variable name is a non-empty, well-formed identifier.
fn is_valid_assumption_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Validate that an assumption payload is non-empty and well-formed canonical data
/// that is completely consumed without trailing bytes.
fn validate_canonical_assumption_payload(payload: &[u8]) -> Result<(), ContextValidationError> {
    if payload.is_empty() {
        return Err(ContextValidationError::InvalidAssumptionPayload);
    }
    if payload.len() > MAX_ASSUMPTION_VALUE_BYTES {
        return Err(ContextValidationError::AssumptionValueTooLong {
            length: payload.len(),
            max: MAX_ASSUMPTION_VALUE_BYTES,
        });
    }
    // Must be valid canonical data (framed bytes, canonical uint, or canonical int)
    // and must be completely consumed.
    let mut r = CanonReader::new(payload);
    if r.read_bytes().is_ok() && r.is_empty() {
        return Ok(());
    }
    let mut r = CanonReader::new(payload);
    if r.read_uint().is_ok() && r.is_empty() {
        return Ok(());
    }
    let mut r = CanonReader::new(payload);
    if r.read_int().is_ok() && r.is_empty() {
        return Ok(());
    }
    Err(ContextValidationError::InvalidAssumptionPayload)
}

/// Validate that a regime profile string is a well-formed, supported ASCII identifier.
fn validate_regime_profile(profile: &str) -> Result<(), ContextValidationError> {
    if profile.is_empty() {
        return Err(ContextValidationError::EmptyRegimeProfile);
    }
    if profile.len() > MAX_REGIME_PROFILE_BYTES {
        return Err(ContextValidationError::RegimeProfileTooLong {
            length: profile.len(),
            max: MAX_REGIME_PROFILE_BYTES,
        });
    }
    let is_valid = profile.chars().all(|c| {
        (c.is_ascii_alphanumeric() || c == '.' || c == '@' || c == '_' || c == '-') && c != '\0'
    });
    if !is_valid {
        return Err(ContextValidationError::MalformedRegimeProfile(
            profile.to_string(),
        ));
    }
    if !SUPPORTED_REGIME_PROFILES.contains(&profile) {
        return Err(ContextValidationError::UnsupportedRegimeProfile(
            profile.to_string(),
        ));
    }
    Ok(())
}

/// Decode and validate structural well-formedness of context bytes within given resource limits.
pub fn decode_context_envelope_v1(
    bytes: &[u8],
    budget: &ContextBudget,
) -> Result<(String, ContextPayloadV1), ContextValidationError> {
    let max_bytes = budget.max_bytes.min(MAX_CONTEXT_BYTES);
    if bytes.len() > max_bytes {
        return Err(ContextValidationError::PayloadTooLarge {
            length: bytes.len(),
            max: max_bytes,
        });
    }

    let mut r = CanonReader::new(bytes);
    let marker = r.read_bytes()?;

    if marker == LEGACY_ROOT_TAG.as_bytes() {
        if !r.is_empty() {
            return Err(ContextValidationError::TrailingBytes);
        }
        return Ok((
            "brix.context.legacy-root@1".to_string(),
            ContextPayloadV1::LegacyRoot,
        ));
    }

    if marker == LEGACY_EXTEND_TAG.as_bytes() {
        let parent_digest = read_framed_digest(&mut r)?;
        let assumption = r.read_bytes()?.to_vec();
        if !r.is_empty() {
            return Err(ContextValidationError::TrailingBytes);
        }
        return Ok((
            "brix.context.legacy-extend@1".to_string(),
            ContextPayloadV1::LegacyExtend {
                parent: ContextId(parent_digest),
                assumption,
            },
        ));
    }

    if marker != CONTEXT_MARKER {
        return Err(ContextValidationError::MalformedMarker);
    }

    let version = r.read_uint()?;
    if version != CONTEXT_FORMAT_V1 {
        return Err(ContextValidationError::UnknownVersion(version));
    }

    let profile_bytes = r.read_bytes()?;
    let profile = std::str::from_utf8(profile_bytes)
        .map_err(|_| ContextValidationError::MalformedMarker)?
        .to_string();

    if profile != CONTEXT_PROFILE_CORE_V1 {
        return Err(ContextValidationError::UnsupportedProfile(profile));
    }

    let kind = r.read_uint()?;
    if kind != 1 {
        return Err(ContextValidationError::UnknownKind(kind));
    }

    // Decode parent Option<ContextId>
    let parent = match r.read_uint()? {
        0 => None,
        1 => {
            let digest = read_framed_digest(&mut r)?;
            Some(ContextId(digest))
        }
        _ => return Err(ContextValidationError::BadLength),
    };

    // Decode program_revision Option<Digest>
    let program_revision = match r.read_uint()? {
        0 => None,
        1 => Some(read_framed_digest(&mut r)?),
        _ => return Err(ContextValidationError::BadLength),
    };

    // Decode world_revision Option<Digest>
    let world_revision = match r.read_uint()? {
        0 => None,
        1 => Some(read_framed_digest(&mut r)?),
        _ => return Err(ContextValidationError::BadLength),
    };

    // Decode assumptions
    let count_u64 = r.read_uint()?;
    let count = usize::try_from(count_u64).map_err(|_| ContextValidationError::BadLength)?;
    let max_assumptions = budget.max_assumptions.min(MAX_CONTEXT_ASSUMPTIONS);
    if count > max_assumptions {
        return Err(ContextValidationError::ExcessiveAssumptions {
            count,
            max: max_assumptions,
        });
    }

    let mut assumptions = Vec::with_capacity(count);
    let mut seen_names = BTreeSet::new();

    for _ in 0..count {
        let name_bytes = r.read_bytes()?;
        if name_bytes.is_empty() {
            return Err(ContextValidationError::InvalidAssumptionName);
        }
        if name_bytes.len() > MAX_ASSUMPTION_NAME_BYTES {
            return Err(ContextValidationError::AssumptionNameTooLong {
                length: name_bytes.len(),
                max: MAX_ASSUMPTION_NAME_BYTES,
            });
        }
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| ContextValidationError::InvalidAssumptionName)?
            .to_string();

        if !is_valid_assumption_ident(&name) {
            return Err(ContextValidationError::InvalidAssumptionName);
        }

        if !seen_names.insert(name.clone()) {
            return Err(ContextValidationError::DuplicateAssumption(name));
        }

        let payload_bytes = r.read_bytes()?;
        if payload_bytes.len() > MAX_ASSUMPTION_VALUE_BYTES {
            return Err(ContextValidationError::AssumptionValueTooLong {
                length: payload_bytes.len(),
                max: MAX_ASSUMPTION_VALUE_BYTES,
            });
        }
        validate_canonical_assumption_payload(payload_bytes)?;

        assumptions.push(ContextAssumptionV1 {
            name,
            payload: payload_bytes.to_vec(),
        });
    }

    // Decode regime profile
    let regime_bytes = r.read_bytes()?;
    let regime_profile = std::str::from_utf8(regime_bytes)
        .map_err(|_| ContextValidationError::MalformedRegimeProfile(String::new()))?
        .to_string();
    validate_regime_profile(&regime_profile)?;

    // Decode execution mode
    let mode_ord = r.read_uint()?;
    let execution_mode = ExecutionMode::from_ordinal(mode_ord)?;

    // Decode budgets
    let step_budget = r.read_uint()?;
    let depth_budget = r.read_uint()?;
    if step_budget == 0
        || depth_budget == 0
        || step_budget > MAX_ALLOWED_STEP_BUDGET
        || depth_budget > MAX_ALLOWED_DEPTH_BUDGET
    {
        return Err(ContextValidationError::InvalidBudget);
    }

    if !r.is_empty() {
        return Err(ContextValidationError::TrailingBytes);
    }

    let core = MaterializedCoreContextV1 {
        parent,
        program_revision,
        world_revision,
        assumptions,
        regime_profile,
        execution_mode,
        step_budget,
        depth_budget,
    };

    Ok((profile, ContextPayloadV1::Core(core)))
}

/// Authenticate that untrusted context bytes match `expected_id` and decode into
/// a structurally valid [`AuthenticatedContextEnvelopeV1`]. This does not validate
/// assumption meaning or authorize evidence from referenced revisions.
///
/// **Boundary:** Does NOT resolve or validate parent ancestry chains. To fully validate
/// a context including its parent hierarchy and execution mode invariants, use
/// [`validate_context_with_parent_resolver`]. The simpler [`validate_context_v1`]
/// only authenticates and decodes the current envelope.
pub fn authenticate_context_envelope_v1(
    bytes: &[u8],
    expected_id: &ContextId,
    budget: &ContextBudget,
) -> Result<AuthenticatedContextEnvelopeV1, ContextValidationError> {
    let max_bytes = budget.max_bytes.min(MAX_CONTEXT_BYTES);
    if bytes.len() > max_bytes {
        return Err(ContextValidationError::PayloadTooLarge {
            length: bytes.len(),
            max: max_bytes,
        });
    }

    // Authenticate identity: reconstructed digest MUST equal expected_id.
    let reconstructed_id = ContextId::from_canon(bytes);
    if reconstructed_id != *expected_id {
        return Err(ContextValidationError::IdentityMismatch {
            claimed: reconstructed_id,
            expected: *expected_id,
        });
    }

    let (profile, payload) = decode_context_envelope_v1(bytes, budget)?;

    Ok(AuthenticatedContextEnvelopeV1 {
        id: *expected_id,
        profile,
        payload,
    })
}

/// Validate untrusted context bytes against `expected_id` and recursively verify
/// that every parent in the ancestry chain is available, bound to the requested ID,
/// free of cycles, and obeys simulation isolation at every transition edge.
pub fn validate_context_with_parent_resolver<F>(
    bytes: &[u8],
    expected_id: &ContextId,
    budget: &ContextBudget,
    parent_resolver: F,
) -> Result<ValidatedContextV1, ContextValidationError>
where
    F: Fn(&ContextId) -> Option<ValidatedContextV1>,
{
    let auth = authenticate_context_envelope_v1(bytes, expected_id, budget)?;
    let max_depth = budget.max_depth.min(MAX_CONTEXT_DEPTH);

    let mut depth = 0;
    let mut seen_parents = BTreeSet::new();

    // Determine initial child mode, next parent to resolve, and effective context mode.
    let (mut current_child_mode, mut current_parent, effective_mode) = match &auth.payload {
        ContextPayloadV1::LegacyRoot => {
            (ExecutionMode::Production, None, ExecutionMode::Production)
        }
        ContextPayloadV1::Core(core) => (core.execution_mode, core.parent, core.execution_mode),
        ContextPayloadV1::LegacyExtend { parent, .. } => {
            // For LegacyExtend, mode is inherited from parent, which MUST be resolved!
            if depth >= max_depth {
                return Err(ContextValidationError::DepthLimitExceeded {
                    depth: depth + 1,
                    max: max_depth,
                });
            }
            let parent_ctx =
                parent_resolver(parent).ok_or(ContextValidationError::UnresolvedParent(*parent))?;
            if parent_ctx.id() != *parent {
                return Err(ContextValidationError::IdentityMismatch {
                    claimed: parent_ctx.id(),
                    expected: *parent,
                });
            }
            seen_parents.insert(*parent);
            depth += 1;
            let inherited_mode = parent_ctx.execution_mode();
            (inherited_mode, parent_ctx.parent(), inherited_mode)
        }
    };

    while let Some(parent_id) = current_parent {
        if !seen_parents.insert(parent_id) {
            return Err(ContextValidationError::CycleInParentHierarchy(parent_id));
        }

        depth += 1;
        if depth > max_depth {
            return Err(ContextValidationError::DepthLimitExceeded {
                depth,
                max: max_depth,
            });
        }

        let parent_ctx = parent_resolver(&parent_id)
            .ok_or(ContextValidationError::UnresolvedParent(parent_id))?;

        // Issue 1: Bind resolver results strictly to requested parent ID!
        if parent_ctx.id() != parent_id {
            return Err(ContextValidationError::IdentityMismatch {
                claimed: parent_ctx.id(),
                expected: parent_id,
            });
        }

        // Issue 3: Ancestry checks compare each edge!
        // A simulation parent cannot produce a production child.
        if parent_ctx.is_simulation() && current_child_mode == ExecutionMode::Production {
            return Err(ContextValidationError::ModeMismatch {
                claimed: ExecutionMode::Production,
                expected: ExecutionMode::Simulation,
            });
        }

        current_child_mode = parent_ctx.execution_mode();
        current_parent = parent_ctx.parent();
    }

    Ok(ValidatedContextV1 {
        id: *expected_id,
        profile: auth.profile,
        payload: auth.payload,
        execution_mode: effective_mode,
    })
}

/// The simple context validator.
///
/// Authenticates that untrusted context bytes match `expected_id` and decode into
/// a structurally valid [`AuthenticatedContextEnvelopeV1`].
///
/// **Sealed boundary:** Does NOT resolve parent ancestry chains or permit
/// asserting production mode when parents are unverified. To perform full
/// ancestry validation and obtain a [`ValidatedContextV1`], use
/// [`validate_context_with_parent_resolver`].
pub fn validate_context_v1(
    bytes: &[u8],
    expected_id: &ContextId,
    budget: &ContextBudget,
) -> Result<AuthenticatedContextEnvelopeV1, ContextValidationError> {
    authenticate_context_envelope_v1(bytes, expected_id, budget)
}

/// Fully validate a root context against an independently supplied [`ContextId`].
///
/// For root contexts (which have no parent dependencies), this completes full validation.
/// If the context requires parent context resolution, this function fails closed with
/// [`ContextValidationError::UnresolvedParent`]. Full-chain verification must be performed via
/// [`validate_context_with_parent_resolver`].
pub fn validate_root_context_v1(
    bytes: &[u8],
    expected_id: &ContextId,
    budget: &ContextBudget,
) -> Result<ValidatedContextV1, ContextValidationError> {
    validate_context_with_parent_resolver(bytes, expected_id, budget, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_parent_cycle_detected() {
        let budget = ContextBudget::strict();
        let cycle_id = ContextId::from_canon(b"context_self_cycle");

        // Construct mock parent that names itself as its own parent
        let val_cyclic = ValidatedContextV1 {
            id: cycle_id,
            profile: "brix.test@1".to_string(),
            payload: ContextPayloadV1::Core(MaterializedCoreContextV1 {
                parent: Some(cycle_id),
                program_revision: None,
                world_revision: None,
                assumptions: vec![],
                regime_profile: "brix.test@1".to_string(),
                execution_mode: ExecutionMode::Production,
                step_budget: 100,
                depth_budget: 10,
            }),
            execution_mode: ExecutionMode::Production,
        };

        let child = MaterializedCoreContextV1 {
            parent: Some(cycle_id),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.test@1".to_string(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        };
        let child_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(child));
        let child_id = ContextId::from_canon(&child_bytes);

        let err = validate_context_with_parent_resolver(&child_bytes, &child_id, &budget, |pid| {
            if *pid == cycle_id {
                Some(val_cyclic.clone())
            } else {
                None
            }
        })
        .unwrap_err();

        assert_eq!(
            err,
            ContextValidationError::CycleInParentHierarchy(cycle_id)
        );
    }

    #[test]
    fn test_internal_ancestry_mode_validation_checks_every_edge() {
        let budget = ContextBudget::strict();

        let grand_id = ContextId::from_canon(b"grand");
        let mid_id = ContextId::from_canon(b"mid");

        let gh = ValidatedContextV1 {
            id: grand_id,
            profile: "brix.test@1".into(),
            payload: ContextPayloadV1::Core(MaterializedCoreContextV1 {
                parent: None,
                program_revision: None,
                world_revision: None,
                assumptions: vec![],
                regime_profile: "brix.test@1".into(),
                execution_mode: ExecutionMode::Simulation,
                step_budget: 100,
                depth_budget: 10,
            }),
            execution_mode: ExecutionMode::Simulation,
        };

        let mh = ValidatedContextV1 {
            id: mid_id,
            profile: "brix.test@1".into(),
            payload: ContextPayloadV1::Core(MaterializedCoreContextV1 {
                parent: Some(grand_id),
                program_revision: None,
                world_revision: None,
                assumptions: vec![],
                regime_profile: "brix.test@1".into(),
                execution_mode: ExecutionMode::Production,
                step_budget: 100,
                depth_budget: 10,
            }),
            execution_mode: ExecutionMode::Production,
        };

        let top = MaterializedCoreContextV1 {
            parent: Some(mid_id),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.test@1".into(),
            execution_mode: ExecutionMode::Simulation,
            step_budget: 100,
            depth_budget: 10,
        };
        let top_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(top));
        let top_id = ContextId::from_canon(&top_bytes);

        // Validating top must fail at the mid -> grand edge even though top is simulation!
        let err = validate_context_with_parent_resolver(&top_bytes, &top_id, &budget, |pid| {
            if *pid == mid_id {
                Some(mh.clone())
            } else if *pid == grand_id {
                Some(gh.clone())
            } else {
                None
            }
        })
        .unwrap_err();

        assert_eq!(
            err,
            ContextValidationError::ModeMismatch {
                claimed: ExecutionMode::Production,
                expected: ExecutionMode::Simulation,
            }
        );
    }

    #[test]
    fn fabricated_ancestry_handle_cannot_hide_simulation_ancestor() {
        let grand_payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: None,
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.test@1".into(),
            execution_mode: ExecutionMode::Simulation,
            step_budget: 100,
            depth_budget: 10,
        });
        let grand_bytes = encode_context_envelope_v1(&grand_payload);
        let grand_id = ContextId::from_canon(&grand_bytes);
        let grand =
            validate_root_context_v1(&grand_bytes, &grand_id, &ContextBudget::strict()).unwrap();

        let mid_payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: Some(grand_id),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.test@1".into(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        });
        let mid_bytes = encode_context_envelope_v1(&mid_payload);
        let mid_id = ContextId::from_canon(&mid_bytes);
        let forged_mid = ValidatedContextV1 {
            id: mid_id,
            profile: CONTEXT_PROFILE_CORE_V1.into(),
            payload: mid_payload,
            execution_mode: ExecutionMode::Production,
        };
        let top_payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: Some(mid_id),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.test@1".into(),
            execution_mode: ExecutionMode::Simulation,
            step_budget: 100,
            depth_budget: 10,
        });
        let top_bytes = encode_context_envelope_v1(&top_payload);
        let top_id = ContextId::from_canon(&top_bytes);

        assert_eq!(
            validate_context_with_parent_resolver(
                &top_bytes,
                &top_id,
                &ContextBudget::strict(),
                |id| {
                    if *id == mid_id {
                        Some(forged_mid.clone())
                    } else if *id == grand_id {
                        Some(grand.clone())
                    } else {
                        None
                    }
                }
            )
            .unwrap_err(),
            ContextValidationError::ModeMismatch {
                claimed: ExecutionMode::Production,
                expected: ExecutionMode::Simulation,
            }
        );
    }
}
