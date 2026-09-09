//! Strict, bounded external input artifact decoder and canonical snapshot representation (ADR-0031).
//!
//! Governs the external input contract for BrixMS `0.1.0-alpha.3` (Wave 1).
//!
//! # Boundaries and Guarantees
//! - **Strict JSON Schema `brix.input@1`:** Root object envelope requiring `"schema"` and `"values"`.
//!   Rejects unknown fields.
//! - **Duplicate Key Rejection:** Zero tolerance for duplicate JSON keys at envelope, values, and
//!   tagged value levels. Parsed duplicate-safely without deserializing into an unchecked map.
//! - **Bounded Resource Enforcement:** Enforces limits on file bytes, shard count, aggregate bytes,
//!   input count, identifier length, and string lengths to prevent unbounded allocation or read amplification.
//! - **Disjoint Shards:** Multiple input files must declare mutually exclusive inputs. Order
//!   of shards cannot affect canonical identity.
//! - **Deterministic Snapshot Identity:** Content-addressed cryptographic identity [`InputSnapshotId`]
//!   under `Domain::Snapshot` using canonical encoding, sorted strictly by NFC-normalized name.
//! - **Separable Validation:** Separates declaration validation ([`InputSnapshot::validate_against_declarations`])
//!   from completeness validation ([`InputSnapshot::validate_completeness`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId};

use crate::finite_decision::plan::{FiniteDecisionPlan, FiniteDecisionProgramId};
use crate::finite_decision::runtime::L3ValueType;
use crate::l3_v2::L3ValueV2;

/// The canonical schema identifier for external input artifacts (ADR-0031 ⟨D-SCHEMA⟩).
pub const INPUT_SCHEMA_V1: &str = "brix.input@1";

/// The canonical domain tag for input snapshot identity (ADR-0031 ⟨D-IDENTITY⟩).
pub const INPUT_SNAPSHOT_TAG: &str = "brix.input.snapshot@1";

/// The canonical domain tag for input-extended deliberation context identity (ADR-0031 ⟨D-IDENTITY⟩).
pub const INPUT_CONTEXT_TAG: &str = "brix.l3.finite-decision.context.input@1";

/// Maximum allowed bytes per individual input file (1 MiB).
pub const MAX_INPUT_FILE_BYTES: usize = 1024 * 1024;

/// Maximum allowed number of disjoint input shard files (16).
pub const MAX_INPUT_FILES: usize = 16;

/// Maximum allowed aggregate bytes across all input shards (4 MiB).
pub const MAX_AGGREGATE_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of inputs across all shards (256).
pub const MAX_INPUT_COUNT: usize = 256;

/// Maximum allowed byte length for an input identifier name (64 B).
pub const MAX_INPUT_NAME_BYTES: usize = 64;

/// Maximum allowed byte length for an input string value (64 KiB).
pub const MAX_STRING_VALUE_BYTES: usize = 65536;

/// Maximum allowed JSON parser nesting depth.
pub const MAX_JSON_DEPTH: usize = 8;

/// Resource limits governing external input artifact decoding and shard aggregation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputLimits {
    pub max_file_bytes: usize,
    pub max_files: usize,
    pub max_aggregate_bytes: usize,
    pub max_input_count: usize,
    pub max_name_bytes: usize,
    pub max_string_value_bytes: usize,
    pub max_depth: usize,
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: MAX_INPUT_FILE_BYTES,
            max_files: MAX_INPUT_FILES,
            max_aggregate_bytes: MAX_AGGREGATE_BYTES,
            max_input_count: MAX_INPUT_COUNT,
            max_name_bytes: MAX_INPUT_NAME_BYTES,
            max_string_value_bytes: MAX_STRING_VALUE_BYTES,
            max_depth: MAX_JSON_DEPTH,
        }
    }
}

/// An admitted scalar value from an external input artifact (ADR-0031 ⟨D-TYPES⟩).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputScalarValue {
    Int(i64),
    Bool(bool),
    Str(String),
}

impl InputScalarValue {
    /// Return the corresponding [`L3ValueType`].
    pub fn value_type(&self) -> L3ValueType {
        match self {
            Self::Int(_) => L3ValueType::Int,
            Self::Bool(_) => L3ValueType::Bool,
            Self::Str(_) => L3ValueType::Str,
        }
    }

    /// Convert into a runtime [`L3ValueV2`].
    pub fn to_l3_value(&self) -> L3ValueV2 {
        match self {
            Self::Int(n) => L3ValueV2::Int(*n),
            Self::Bool(b) => L3ValueV2::Bool(*b),
            Self::Str(s) => L3ValueV2::Str(s.clone()),
        }
    }

    /// Attempt conversion from a runtime [`L3ValueV2`]. Returns `None` for non-scalar types.
    pub fn from_l3_value(val: &L3ValueV2) -> Option<Self> {
        match val {
            L3ValueV2::Int(n) => Some(Self::Int(*n)),
            L3ValueV2::Bool(b) => Some(Self::Bool(*b)),
            L3ValueV2::Str(s) => Some(Self::Str(s.clone())),
            L3ValueV2::Ctor { .. } | L3ValueV2::Record { .. } => None,
        }
    }
}

impl Canonical for InputScalarValue {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Self::Int(n) => w.write_enum(0, |w| w.write_int(*n)),
            Self::Bool(b) => w.write_enum(1, |w| w.write_bool(*b)),
            Self::Str(s) => w.write_enum(2, |w| w.write_str(s)),
        }
    }
}

/// A decoded single input artifact shard before aggregation (ADR-0031 ⟨D-SHARDS⟩).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputShard {
    schema: String,
    values: BTreeMap<String, InputScalarValue>,
    byte_count: usize,
}

impl InputShard {
    /// The schema marker in the envelope.
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// The decoded values map.
    pub fn values(&self) -> &BTreeMap<String, InputScalarValue> {
        &self.values
    }

    /// Number of bytes in the raw shard.
    pub fn byte_count(&self) -> usize {
        self.byte_count
    }

    /// Look up an input value by name.
    pub fn get(&self, name: &str) -> Option<&InputScalarValue> {
        self.values.get(name)
    }
}

/// Content-addressed cryptographic identity for an input snapshot (ADR-0031 ⟨D-IDENTITY⟩).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct InputSnapshotId(pub Digest);

impl InputSnapshotId {
    pub fn digest(&self) -> Digest {
        self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    pub fn from_canon(payload: &[u8]) -> Self {
        InputSnapshotId(Digest::of(Domain::Snapshot, payload))
    }
}

impl Canonical for InputSnapshotId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// Canonical, disjoint, lexicographically sorted input snapshot (ADR-0031 ⟨D-IDENTITY⟩, ⟨D-SHARDS⟩).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSnapshot {
    values: BTreeMap<String, InputScalarValue>,
    total_bytes: usize,
    shard_count: usize,
}

impl InputSnapshot {
    /// Construct an empty snapshot.
    pub fn empty() -> Self {
        Self {
            values: BTreeMap::new(),
            total_bytes: 0,
            shard_count: 0,
        }
    }

    /// The sorted values map.
    pub fn values(&self) -> &BTreeMap<String, InputScalarValue> {
        &self.values
    }

    /// The number of supplied input entries.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Look up an input value by name.
    pub fn get(&self, name: &str) -> Option<&InputScalarValue> {
        self.values.get(name)
    }

    /// Total bytes across constituent shards.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Number of shards combined into this snapshot.
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// Canonical preimage bytes uniquely binding sorted NFC-normalized names and canonical values.
    pub fn preimage(&self) -> Vec<u8> {
        input_snapshot_preimage(self)
    }

    /// Cryptographic identity [`InputSnapshotId`] under `Domain::Snapshot`.
    pub fn id(&self) -> InputSnapshotId {
        input_snapshot_id(self)
    }

    /// Validate that all supplied inputs correspond to declared inputs with matching types (ADR-0031 ⟨D-CHECK⟩).
    ///
    /// Separable: allows partial input snapshots where not all declared inputs are present.
    pub fn validate_against_declarations(
        &self,
        plan: &FiniteDecisionPlan,
    ) -> Result<(), InputValidationError> {
        for (name, val) in &self.values {
            let Some(decl) = plan.find_input(name) else {
                return Err(InputValidationError::UndeclaredInput { name: name.clone() });
            };
            if decl.ty != val.value_type() {
                return Err(InputValidationError::TypeMismatch {
                    name: name.clone(),
                    declared: decl.ty.clone(),
                    supplied: val.value_type(),
                });
            }
        }
        Ok(())
    }

    /// Validate complete input fulfillment for execution (ADR-0031 ⟨D-CHECK⟩).
    ///
    /// Requires that all supplied inputs match declarations AND every declared input is present.
    pub fn validate_completeness(
        &self,
        plan: &FiniteDecisionPlan,
    ) -> Result<(), InputValidationError> {
        self.validate_against_declarations(plan)?;
        for decl in &plan.inputs {
            if !self.values.contains_key(&decl.name) {
                return Err(InputValidationError::MissingInput {
                    name: decl.name.clone(),
                    declared: decl.ty.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Validate that all supplied inputs correspond to declared inputs with matching types (ADR-0031 ⟨D-CHECK⟩).
pub fn validate_against_declarations(
    snapshot: &InputSnapshot,
    plan: &FiniteDecisionPlan,
) -> Result<(), InputValidationError> {
    snapshot.validate_against_declarations(plan)
}

/// Validate complete input fulfillment for execution (ADR-0031 ⟨D-CHECK⟩).
pub fn validate_completeness(
    snapshot: &InputSnapshot,
    plan: &FiniteDecisionPlan,
) -> Result<(), InputValidationError> {
    snapshot.validate_completeness(plan)
}

/// Compute the canonical preimage for an input snapshot (ADR-0031 ⟨D-IDENTITY⟩).
pub fn input_snapshot_preimage(snapshot: &InputSnapshot) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag(INPUT_SNAPSHOT_TAG);
    w.write_uint(snapshot.values.len() as u64);
    for (name, val) in &snapshot.values {
        w.write_ident(name);
        val.canon_write(&mut w);
    }
    w.finish()
}

/// Compute the canonical snapshot identity for an input snapshot (ADR-0031 ⟨D-IDENTITY⟩).
pub fn input_snapshot_id(snapshot: &InputSnapshot) -> InputSnapshotId {
    InputSnapshotId::from_canon(&input_snapshot_preimage(snapshot))
}

/// Compute the deliberation [`ContextId`] binding program, world, policy, and optional input snapshot (ADR-0031 ⟨D-IDENTITY⟩).
///
/// When `snapshot` is `None` or empty, reproduces the exact alpha.2 context identity byte-for-byte.
pub fn input_context_id(
    program: FiniteDecisionProgramId,
    initial: ConfigId,
    policy: ConfigId,
    snapshot: Option<&InputSnapshot>,
) -> ContextId {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.finite-decision.context");
    w.write_uint(1);
    w.write_bytes(program.digest().as_bytes());
    w.write_bytes(initial.digest().as_bytes());
    w.write_bytes(policy.digest().as_bytes());
    if let Some(snap) = snapshot {
        if !snap.is_empty() {
            let snap_id = input_snapshot_id(snap);
            w.write_tag(INPUT_CONTEXT_TAG);
            w.write_bytes(snap_id.digest().as_bytes());
        }
    }
    ContextId::from_canon(&w.finish())
}

// ---------------------------------------------------------------------------
// Error Types
// ---------------------------------------------------------------------------

/// Errors encountered while decoding an individual input artifact shard (ADR-0031 ⟨D-BOUNDS⟩, ⟨D-NODUPKEYS⟩).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputDecodeError {
    FileTooLarge {
        limit: usize,
        found: u64,
    },
    IoError {
        path: String,
        message: String,
    },
    NotARegularFile(String),
    DuplicateKey {
        key: String,
        offset: usize,
    },
    UnknownField {
        field: String,
        offset: usize,
    },
    MissingField(&'static str),
    InvalidSchema {
        expected: &'static str,
        found: String,
    },
    InvalidType {
        expected: &'static str,
        found: String,
        offset: usize,
    },
    NameTooLong {
        limit: usize,
    },
    InvalidIdentifier {
        name: String,
        reason: &'static str,
    },
    StringValueTooLong {
        limit: usize,
        found: usize,
    },
    IntegerOverflow {
        raw: String,
    },
    InvalidIntegerFormat {
        raw: String,
    },
    LimitExceeded(&'static str),
    UnexpectedEof,
    TrailingBytes {
        offset: usize,
    },
    SyntaxError {
        message: String,
        offset: usize,
    },
}

impl fmt::Display for InputDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooLarge { limit, found } => {
                write!(
                    f,
                    "input file size ({found} bytes) exceeds limit ({limit} bytes)"
                )
            }
            Self::IoError { path, message } => {
                write!(f, "I/O error reading input '{path}': {message}")
            }
            Self::NotARegularFile(path) => write!(f, "path is not a regular file: {path}"),
            Self::DuplicateKey { key, offset } => {
                write!(f, "duplicate JSON key '{key}' at byte offset {offset}")
            }
            Self::UnknownField { field, offset } => {
                write!(f, "unknown field '{field}' at byte offset {offset}")
            }
            Self::MissingField(field) => write!(f, "missing required field '{field}'"),
            Self::InvalidSchema { expected, found } => {
                write!(f, "schema mismatch: expected '{expected}', found '{found}'")
            }
            Self::InvalidType {
                expected,
                found,
                offset,
            } => {
                write!(
                    f,
                    "invalid type at offset {offset}: expected {expected}, found '{found}'"
                )
            }
            Self::NameTooLong { limit } => {
                write!(f, "input name exceeds length limit ({limit} bytes)")
            }
            Self::InvalidIdentifier { name, reason } => {
                write!(f, "invalid identifier '{name}': {reason}")
            }
            Self::StringValueTooLong { limit, found } => {
                write!(
                    f,
                    "string value length ({found} bytes) exceeds limit ({limit} bytes)"
                )
            }
            Self::IntegerOverflow { raw } => {
                write!(
                    f,
                    "integer literal '{raw}' overflows 64-bit signed integer range"
                )
            }
            Self::InvalidIntegerFormat { raw } => {
                write!(
                    f,
                    "invalid integer format '{raw}' (must be base-10 decimal string)"
                )
            }
            Self::LimitExceeded(limit) => write!(f, "resource limit exceeded: {limit}"),
            Self::UnexpectedEof => write!(f, "unexpected end of JSON input"),
            Self::TrailingBytes { offset } => {
                write!(
                    f,
                    "unexpected trailing non-whitespace bytes at offset {offset}"
                )
            }
            Self::SyntaxError { message, offset } => {
                write!(f, "JSON syntax error at offset {offset}: {message}")
            }
        }
    }
}

impl std::error::Error for InputDecodeError {}

/// Errors occurring during shard aggregation or validation (ADR-0031 ⟨D-SHARDS⟩).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    Decode(InputDecodeError),
    DuplicateAcrossShards { name: String },
    TooManyFiles { limit: usize, found: usize },
    AggregateBytesExceeded { limit: usize, found: usize },
    TotalInputCountExceeded { limit: usize, found: usize },
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "decode error: {e}"),
            Self::DuplicateAcrossShards { name } => {
                write!(
                    f,
                    "duplicate input '{name}' supplied across multiple disjoint shards"
                )
            }
            Self::TooManyFiles { limit, found } => {
                write!(f, "number of input files ({found}) exceeds limit ({limit})")
            }
            Self::AggregateBytesExceeded { limit, found } => {
                write!(
                    f,
                    "aggregate input size ({found} bytes) exceeds limit ({limit} bytes)"
                )
            }
            Self::TotalInputCountExceeded { limit, found } => {
                write!(f, "total input count ({found}) exceeds limit ({limit})")
            }
        }
    }
}

impl std::error::Error for InputError {}

impl From<InputDecodeError> for InputError {
    fn from(err: InputDecodeError) -> Self {
        Self::Decode(err)
    }
}

/// Errors validating an input snapshot against program declarations (ADR-0031 ⟨D-CHECK⟩).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputValidationError {
    UndeclaredInput {
        name: String,
    },
    MissingInput {
        name: String,
        declared: L3ValueType,
    },
    TypeMismatch {
        name: String,
        declared: L3ValueType,
        supplied: L3ValueType,
    },
}

impl fmt::Display for InputValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndeclaredInput { name } => {
                write!(f, "supplied input '{name}' is not declared in the program")
            }
            Self::MissingInput { name, declared } => {
                write!(
                    f,
                    "declared input '{name}' of type {declared} was not supplied"
                )
            }
            Self::TypeMismatch {
                name,
                declared,
                supplied,
            } => {
                write!(
                    f,
                    "type mismatch for input '{name}': declared {declared}, supplied {supplied}"
                )
            }
        }
    }
}

impl std::error::Error for InputValidationError {}

// ---------------------------------------------------------------------------
// Decoder and Shard Helpers
// ---------------------------------------------------------------------------

/// Decode a single input artifact shard from pre-bounded in-memory bytes (ADR-0031 ⟨D-SCHEMA⟩, ⟨D-BOUNDS⟩).
pub fn decode_input_shard(
    bytes: &[u8],
    limits: &InputLimits,
) -> Result<InputShard, InputDecodeError> {
    if bytes.len() > limits.max_file_bytes {
        return Err(InputDecodeError::FileTooLarge {
            limit: limits.max_file_bytes,
            found: bytes.len() as u64,
        });
    }

    let text = std::str::from_utf8(bytes).map_err(|e| InputDecodeError::SyntaxError {
        message: format!("invalid UTF-8 in document: {e}"),
        offset: e.valid_up_to(),
    })?;

    let mut parser = StrictJsonParser::new(text, limits);
    parser.parse_input_shard()
}

#[cfg(unix)]
fn open_input_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_input_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(true).open(path)
}

/// Decode a single input artifact shard from a regular file using a bounded reader (ADR-0031 ⟨D-BOUNDS⟩).
///
/// Security properties:
/// - Opens files non-blocking on Unix (`O_NONBLOCK`) to prevent indefinite blocking on named pipes/FIFOs
///   that have no active writer, and to harden against TOCTOU swap races between preflight and open.
/// - Validates the opened file handle's metadata (`fstat`) before reading, rejecting non-regular files
///   (directories, fifos, sockets, character/block devices).
/// - Validates file size from the opened file handle before reading.
/// - Reads at most `max_file_bytes + 1` using [`std::io::Read::take`], ensuring that an unbounded read
///   or memory amplification is physically impossible even under concurrent truncation/expansion.
/// - Rejects with [`InputDecodeError::FileTooLarge`] if the sentinel byte exists.
pub fn decode_input_shard_from_file<P: AsRef<Path>>(
    path: P,
    limits: &InputLimits,
) -> Result<InputShard, InputDecodeError> {
    let p = path.as_ref();
    let safe_path = p.display().to_string();
    let file = open_input_file(p).map_err(|e| InputDecodeError::IoError {
        path: safe_path.clone(),
        message: e.to_string(),
    })?;
    let meta = file.metadata().map_err(|e| InputDecodeError::IoError {
        path: safe_path.clone(),
        message: e.to_string(),
    })?;
    if !meta.file_type().is_file() {
        return Err(InputDecodeError::NotARegularFile(safe_path));
    }
    let file_len = meta.len();
    if file_len > limits.max_file_bytes as u64 {
        return Err(InputDecodeError::FileTooLarge {
            limit: limits.max_file_bytes,
            found: file_len,
        });
    }

    let read_limit = limits
        .max_file_bytes
        .checked_add(1)
        .ok_or(InputDecodeError::LimitExceeded("max_file_bytes overflow"))?;

    use std::io::Read;
    let mut buf = Vec::new();
    file.take(read_limit as u64)
        .read_to_end(&mut buf)
        .map_err(|e| InputDecodeError::IoError {
            path: safe_path.clone(),
            message: e.to_string(),
        })?;

    if buf.len() > limits.max_file_bytes {
        return Err(InputDecodeError::FileTooLarge {
            limit: limits.max_file_bytes,
            found: buf.len() as u64,
        });
    }

    decode_input_shard(&buf, limits)
}

/// Load, validate, and canonicalize a complete set of input shard files from the filesystem (ADR-0031 ⟨D-SHARDS⟩, ⟨D-BOUNDS⟩).
///
/// This is the primary safe entry point for loading external inputs (e.g. from CLI `--input` flags).
///
/// # Security and Resource Guarantees
/// - **Preflight `max_files`:** Enforces that the number of paths does not exceed [`InputLimits::max_files`]
///   before opening any files.
/// - **Metadata Preflight:** Iterates input file metadata using [`usize::checked_add`] to reject aggregate
///   file sizes exceeding [`InputLimits::max_aggregate_bytes`] before allocating file buffers.
///   - *What metadata guarantees:* Fast, early rejection of non-regular files and oversized shard sets
///     before allocating file buffers.
///   - *What metadata CANNOT guarantee:* Filesystem metadata is non-atomic and vulnerable to TOCTOU races.
///     Files can grow or be swapped concurrently, and special pseudo-filesystems may report deceptive lengths.
/// - **Authoritative Bounded Reads:** Each shard is read using a regular-file bounded reader (`take(max_file_bytes + 1)`),
///   ensuring memory allocation is strictly bounded regardless of metadata accuracy.
/// - **Zero Buffer Retention:** Raw file buffers are immediately dropped after each shard is decoded.
/// - **Actual Aggregate Recheck:** The exact byte count of each decoded shard is accumulated with
///   [`usize::checked_add`] and rechecked against [`InputLimits::max_aggregate_bytes`].
/// - **Disjointness & Canonicalization:** Shards must be strictly disjoint. Returns an [`InputSnapshot`]
///   with canonical sorting and cryptographic identity.
pub fn load_input_snapshot_from_paths<P: AsRef<Path>>(
    paths: &[P],
    limits: &InputLimits,
) -> Result<InputSnapshot, InputError> {
    if paths.len() > limits.max_files {
        return Err(InputError::TooManyFiles {
            limit: limits.max_files,
            found: paths.len(),
        });
    }

    if paths.is_empty() {
        return Ok(InputSnapshot::empty());
    }

    // Step 1: Preflight aggregate metadata sizes with checked_add.
    let mut metadata_aggregate: usize = 0;
    for path in paths {
        let p = path.as_ref();
        let safe_path = p.display().to_string();
        let meta = std::fs::metadata(p).map_err(|e| InputDecodeError::IoError {
            path: safe_path.clone(),
            message: e.to_string(),
        })?;
        if !meta.file_type().is_file() {
            return Err(InputError::Decode(InputDecodeError::NotARegularFile(
                safe_path,
            )));
        }
        let len = meta.len();
        if len > limits.max_file_bytes as u64 {
            return Err(InputError::Decode(InputDecodeError::FileTooLarge {
                limit: limits.max_file_bytes,
                found: len,
            }));
        }
        metadata_aggregate = metadata_aggregate.checked_add(len as usize).ok_or(
            InputError::AggregateBytesExceeded {
                limit: limits.max_aggregate_bytes,
                found: usize::MAX,
            },
        )?;
        if metadata_aggregate > limits.max_aggregate_bytes {
            return Err(InputError::AggregateBytesExceeded {
                limit: limits.max_aggregate_bytes,
                found: metadata_aggregate,
            });
        }
    }

    // Step 2: Bounded read and decode shards, tracking actual aggregate bytes.
    // Raw file byte buffers are dropped inside `decode_input_shard_from_file`.
    let mut actual_aggregate: usize = 0;
    let mut shards = Vec::with_capacity(paths.len());
    for path in paths {
        let shard = decode_input_shard_from_file(path, limits)?;
        actual_aggregate = actual_aggregate.checked_add(shard.byte_count()).ok_or(
            InputError::AggregateBytesExceeded {
                limit: limits.max_aggregate_bytes,
                found: usize::MAX,
            },
        )?;
        if actual_aggregate > limits.max_aggregate_bytes {
            return Err(InputError::AggregateBytesExceeded {
                limit: limits.max_aggregate_bytes,
                found: actual_aggregate,
            });
        }
        shards.push(shard);
    }

    // Step 3: Canonicalize shards (validates disjointness and total input count).
    canonicalize_input_shards(shards, limits)
}

/// Combine multiple input shards into a canonical snapshot, enforcing disjointness and aggregate limits (ADR-0031 ⟨D-SHARDS⟩).
pub fn canonicalize_input_shards(
    shards: Vec<InputShard>,
    limits: &InputLimits,
) -> Result<InputSnapshot, InputError> {
    if shards.len() > limits.max_files {
        return Err(InputError::TooManyFiles {
            limit: limits.max_files,
            found: shards.len(),
        });
    }

    let shard_count = shards.len();
    let mut total_bytes = 0usize;
    let mut values = BTreeMap::new();

    for shard in shards {
        total_bytes = total_bytes.checked_add(shard.byte_count()).ok_or(
            InputError::AggregateBytesExceeded {
                limit: limits.max_aggregate_bytes,
                found: usize::MAX,
            },
        )?;

        if total_bytes > limits.max_aggregate_bytes {
            return Err(InputError::AggregateBytesExceeded {
                limit: limits.max_aggregate_bytes,
                found: total_bytes,
            });
        }

        for (k, v) in shard.values {
            if values.contains_key(&k) {
                return Err(InputError::DuplicateAcrossShards { name: k });
            }
            values.insert(k, v);
            if values.len() > limits.max_input_count {
                return Err(InputError::TotalInputCountExceeded {
                    limit: limits.max_input_count,
                    found: values.len(),
                });
            }
        }
    }

    Ok(InputSnapshot {
        values,
        total_bytes,
        shard_count,
    })
}

// ---------------------------------------------------------------------------
// Strict, Bounded Duplicate-Rejecting JSON Parser
// ---------------------------------------------------------------------------

struct StrictJsonParser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    limits: &'a InputLimits,
    depth: usize,
}

impl<'a> StrictJsonParser<'a> {
    fn new(text: &'a str, limits: &'a InputLimits) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            limits,
            depth: 0,
        }
    }

    fn parse_input_shard(&mut self) -> Result<InputShard, InputDecodeError> {
        self.skip_whitespace();
        let shard = self.parse_envelope()?;
        self.skip_whitespace();
        if self.pos < self.bytes.len() {
            return Err(InputDecodeError::TrailingBytes { offset: self.pos });
        }
        Ok(shard)
    }

    fn enter_depth(&mut self) -> Result<(), InputDecodeError> {
        if self.depth >= self.limits.max_depth {
            return Err(InputDecodeError::LimitExceeded("JSON nesting depth limit"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn consume(&mut self, expected: u8, ctx: &'static str) -> Result<(), InputDecodeError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b) if b == expected => {
                self.pos += 1;
                Ok(())
            }
            Some(other) => Err(InputDecodeError::SyntaxError {
                message: format!(
                    "expected '{}' for {}, found '{}'",
                    expected as char, ctx, other as char
                ),
                offset: self.pos,
            }),
            None => Err(InputDecodeError::UnexpectedEof),
        }
    }

    fn parse_string(&mut self, max_len: usize) -> Result<String, InputDecodeError> {
        self.skip_whitespace();
        if self.peek() != Some(b'"') {
            return Err(InputDecodeError::SyntaxError {
                message: "expected opening quote '\"'".to_string(),
                offset: self.pos,
            });
        }
        self.pos += 1; // consume '"'

        let start_offset = self.pos;
        let mut s = String::new();

        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1; // consume closing '"'
                return Ok(s);
            } else if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    return Err(InputDecodeError::UnexpectedEof);
                }
                let esc = self.bytes[self.pos];
                self.pos += 1;
                match esc {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'b' => s.push('\x08'),
                    b'f' => s.push('\x0c'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => {
                        let ch = self.parse_unicode_escape()?;
                        s.push(ch);
                    }
                    other => {
                        return Err(InputDecodeError::SyntaxError {
                            message: format!("unrecognized escape character '\\{}'", other as char),
                            offset: self.pos - 1,
                        });
                    }
                }
            } else if b < 0x20 {
                return Err(InputDecodeError::SyntaxError {
                    message: "control character in string literal".to_string(),
                    offset: self.pos,
                });
            } else {
                // b is a valid UTF-8 scalar byte (since document was validated once upfront)
                let remaining = &self.text[self.pos..];
                let ch = remaining.chars().next().unwrap();
                s.push(ch);
                self.pos += ch.len_utf8();
            }

            if s.len() > max_len {
                return Err(InputDecodeError::StringValueTooLong {
                    limit: max_len,
                    found: s.len(),
                });
            }
        }

        Err(InputDecodeError::SyntaxError {
            message: "unterminated string literal".to_string(),
            offset: start_offset,
        })
    }

    fn parse_unicode_escape(&mut self) -> Result<char, InputDecodeError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(InputDecodeError::UnexpectedEof);
        }
        let code = parse_hex_4(&self.bytes[self.pos..self.pos + 4]).ok_or_else(|| {
            InputDecodeError::SyntaxError {
                message: "invalid hex digits in \\u escape".to_string(),
                offset: self.pos,
            }
        })?;
        self.pos += 4;

        if (0xD800..=0xDBFF).contains(&code) {
            // High surrogate: MUST be immediately followed by \uXXXX with low surrogate
            if self.pos + 2 > self.bytes.len() || &self.bytes[self.pos..self.pos + 2] != b"\\u" {
                return Err(InputDecodeError::SyntaxError {
                    message: "lone high surrogate in \\u escape without trailing low surrogate"
                        .to_string(),
                    offset: self.pos - 6,
                });
            }
            self.pos += 2;
            if self.pos + 4 > self.bytes.len() {
                return Err(InputDecodeError::UnexpectedEof);
            }
            let low_code = parse_hex_4(&self.bytes[self.pos..self.pos + 4]).ok_or_else(|| {
                InputDecodeError::SyntaxError {
                    message: "invalid hex digits in low surrogate \\u escape".to_string(),
                    offset: self.pos,
                }
            })?;
            self.pos += 4;

            if !(0xDC00..=0xDFFF).contains(&low_code) {
                return Err(InputDecodeError::SyntaxError {
                    message: format!(
                        "invalid low surrogate 0x{low_code:04X} following high surrogate 0x{code:04X}"
                    ),
                    offset: self.pos - 6,
                });
            }
            let scalar = 0x10000 + (((code - 0xD800) as u32) << 10) + ((low_code - 0xDC00) as u32);
            char::from_u32(scalar).ok_or_else(|| InputDecodeError::SyntaxError {
                message: format!("invalid unicode scalar 0x{scalar:X} from surrogate pair"),
                offset: self.pos - 12,
            })
        } else if (0xDC00..=0xDFFF).contains(&code) {
            // Lone low surrogate (or reversed pair)
            Err(InputDecodeError::SyntaxError {
                message: format!("lone low surrogate 0x{code:04X} in \\u escape"),
                offset: self.pos - 6,
            })
        } else {
            // Valid BMP code point
            char::from_u32(code as u32).ok_or_else(|| InputDecodeError::SyntaxError {
                message: format!("invalid unicode scalar 0x{code:04X} in \\u escape"),
                offset: self.pos - 4,
            })
        }
    }

    fn parse_input_name(&mut self) -> Result<String, InputDecodeError> {
        self.skip_whitespace();
        if self.peek() != Some(b'"') {
            return Err(InputDecodeError::SyntaxError {
                message: "expected opening quote '\"'".to_string(),
                offset: self.pos,
            });
        }
        self.pos += 1;
        let start_offset = self.pos;
        let mut s = String::new();

        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                validate_identifier(&s, self.limits.max_name_bytes)?;
                return Ok(s);
            } else if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    return Err(InputDecodeError::UnexpectedEof);
                }
                let esc = self.bytes[self.pos];
                self.pos += 1;
                match esc {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'b' => s.push('\x08'),
                    b'f' => s.push('\x0c'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => {
                        let ch = self.parse_unicode_escape()?;
                        s.push(ch);
                    }
                    other => {
                        return Err(InputDecodeError::SyntaxError {
                            message: format!("unrecognized escape character '\\{}'", other as char),
                            offset: self.pos - 1,
                        });
                    }
                }
            } else if b < 0x20 {
                return Err(InputDecodeError::SyntaxError {
                    message: "control character in input name".to_string(),
                    offset: self.pos,
                });
            } else {
                let remaining = &self.text[self.pos..];
                let ch = remaining.chars().next().unwrap();
                s.push(ch);
                self.pos += ch.len_utf8();
            }

            // Enforce max_name_bytes while scanning! Stop immediately without storing huge data.
            if s.len() > self.limits.max_name_bytes {
                return Err(InputDecodeError::NameTooLong {
                    limit: self.limits.max_name_bytes,
                });
            }
        }

        Err(InputDecodeError::SyntaxError {
            message: "unterminated string literal in input name".to_string(),
            offset: start_offset,
        })
    }

    fn parse_envelope(&mut self) -> Result<InputShard, InputDecodeError> {
        self.enter_depth()?;
        self.consume(b'{', "root envelope object")?;

        let mut seen_keys = BTreeSet::new();
        let mut schema: Option<String> = None;
        let mut values: Option<BTreeMap<String, InputScalarValue>> = None;

        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave_depth();
            return Err(InputDecodeError::MissingField("schema"));
        }

        loop {
            self.skip_whitespace();
            let key_offset = self.pos;
            let key = self.parse_string(self.limits.max_name_bytes)?;
            if !seen_keys.insert(key.clone()) {
                return Err(InputDecodeError::DuplicateKey {
                    key,
                    offset: key_offset,
                });
            }

            self.consume(b':', "envelope field separator")?;

            match key.as_str() {
                "schema" => {
                    let schema_val = self.parse_string(64)?;
                    if schema_val != INPUT_SCHEMA_V1 {
                        return Err(InputDecodeError::InvalidSchema {
                            expected: INPUT_SCHEMA_V1,
                            found: schema_val,
                        });
                    }
                    schema = Some(schema_val);
                }
                "values" => {
                    let vals = self.parse_values_object()?;
                    values = Some(vals);
                }
                _ => {
                    return Err(InputDecodeError::UnknownField {
                        field: key,
                        offset: key_offset,
                    });
                }
            }

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(other) => {
                    return Err(InputDecodeError::SyntaxError {
                        message: format!(
                            "expected ',' or '}}' in envelope, found '{}'",
                            other as char
                        ),
                        offset: self.pos,
                    });
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }

        self.leave_depth();

        let schema = schema.ok_or(InputDecodeError::MissingField("schema"))?;
        let values = values.ok_or(InputDecodeError::MissingField("values"))?;

        Ok(InputShard {
            schema,
            values,
            byte_count: self.bytes.len(),
        })
    }

    fn parse_values_object(
        &mut self,
    ) -> Result<BTreeMap<String, InputScalarValue>, InputDecodeError> {
        self.enter_depth()?;
        self.consume(b'{', "values object")?;

        let mut seen_names = BTreeSet::new();
        let mut map = BTreeMap::new();

        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave_depth();
            return Ok(map);
        }

        loop {
            self.skip_whitespace();
            let name_offset = self.pos;
            let name = self.parse_input_name()?;

            if !seen_names.insert(name.clone()) {
                return Err(InputDecodeError::DuplicateKey {
                    key: name,
                    offset: name_offset,
                });
            }

            if seen_names.len() > self.limits.max_input_count {
                return Err(InputDecodeError::LimitExceeded("input count per shard"));
            }

            self.consume(b':', "input value separator")?;

            let scalar = self.parse_tagged_value()?;
            map.insert(name, scalar);

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(other) => {
                    return Err(InputDecodeError::SyntaxError {
                        message: format!(
                            "expected ',' or '}}' in values object, found '{}'",
                            other as char
                        ),
                        offset: self.pos,
                    });
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }

        self.leave_depth();
        Ok(map)
    }

    fn parse_tagged_value(&mut self) -> Result<InputScalarValue, InputDecodeError> {
        self.enter_depth()?;
        self.consume(b'{', "tagged value object")?;

        let mut seen_fields = BTreeSet::new();
        let mut raw_type: Option<(String, usize)> = None;
        let mut raw_value: Option<RawValue> = None;

        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave_depth();
            return Err(InputDecodeError::MissingField("type"));
        }

        loop {
            self.skip_whitespace();
            let field_offset = self.pos;
            let field = self.parse_string(64)?;

            if !seen_fields.insert(field.clone()) {
                return Err(InputDecodeError::DuplicateKey {
                    key: field,
                    offset: field_offset,
                });
            }

            self.consume(b':', "tagged value field separator")?;

            match field.as_str() {
                "type" => {
                    let t_offset = self.pos;
                    let t_str = self.parse_string(32)?;
                    raw_type = Some((t_str, t_offset));
                }
                "value" => {
                    let val = self.parse_raw_scalar_field()?;
                    raw_value = Some(val);
                }
                _ => {
                    return Err(InputDecodeError::UnknownField {
                        field,
                        offset: field_offset,
                    });
                }
            }

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(other) => {
                    return Err(InputDecodeError::SyntaxError {
                        message: format!(
                            "expected ',' or '}}' in tagged value, found '{}'",
                            other as char
                        ),
                        offset: self.pos,
                    });
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }

        self.leave_depth();

        let (type_name, type_offset) = raw_type.ok_or(InputDecodeError::MissingField("type"))?;
        let value = raw_value.ok_or(InputDecodeError::MissingField("value"))?;

        match type_name.as_str() {
            "int" => match value {
                RawValue::String(s) => {
                    validate_decimal_int(&s)?;
                    let n = s
                        .parse::<i64>()
                        .map_err(|_| InputDecodeError::IntegerOverflow { raw: s })?;
                    Ok(InputScalarValue::Int(n))
                }
                _ => Err(InputDecodeError::InvalidType {
                    expected: "decimal string for int value",
                    found: value.type_name().to_string(),
                    offset: type_offset,
                }),
            },
            "bool" => match value {
                RawValue::Bool(b) => Ok(InputScalarValue::Bool(b)),
                _ => Err(InputDecodeError::InvalidType {
                    expected: "boolean (true or false) for bool value",
                    found: value.type_name().to_string(),
                    offset: type_offset,
                }),
            },
            "string" => match value {
                RawValue::String(s) => Ok(InputScalarValue::Str(s)),
                _ => Err(InputDecodeError::InvalidType {
                    expected: "string for string value",
                    found: value.type_name().to_string(),
                    offset: type_offset,
                }),
            },
            other => Err(InputDecodeError::InvalidType {
                expected: "one of ['int', 'bool', 'string']",
                found: other.to_string(),
                offset: type_offset,
            }),
        }
    }

    fn parse_raw_scalar_field(&mut self) -> Result<RawValue, InputDecodeError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => {
                let s = self.parse_string(self.limits.max_string_value_bytes)?;
                Ok(RawValue::String(s))
            }
            Some(b't') => {
                self.consume_exact(b"true", "boolean true")?;
                Ok(RawValue::Bool(true))
            }
            Some(b'f') => {
                self.consume_exact(b"false", "boolean false")?;
                Ok(RawValue::Bool(false))
            }
            Some(b'n') => {
                self.consume_exact(b"null", "null")?;
                Ok(RawValue::Null)
            }
            Some(b'[') => {
                self.skip_array()?;
                Ok(RawValue::Array)
            }
            Some(b'{') => {
                self.skip_object()?;
                Ok(RawValue::Object)
            }
            Some(b'-' | b'0'..=b'9') => {
                self.parse_json_number()?;
                Ok(RawValue::Number)
            }
            Some(other) => Err(InputDecodeError::SyntaxError {
                message: format!("unexpected scalar value byte '{}'", other as char),
                offset: self.pos,
            }),
            None => Err(InputDecodeError::UnexpectedEof),
        }
    }

    fn parse_json_number(&mut self) -> Result<(), InputDecodeError> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }

        // Must have at least one digit
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                // If starts with 0, next char cannot be another digit (RFC 8259 leading zero rejection)
                if let Some(b'0'..=b'9') = self.peek() {
                    return Err(InputDecodeError::SyntaxError {
                        message: "leading zero in JSON number".to_string(),
                        offset: self.pos,
                    });
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            }
            Some(_) => {
                return Err(InputDecodeError::SyntaxError {
                    message: "expected digit in JSON number".to_string(),
                    offset: self.pos,
                });
            }
            None => return Err(InputDecodeError::UnexpectedEof),
        }

        // Fractional part
        if self.peek() == Some(b'.') {
            self.pos += 1;
            match self.peek() {
                Some(b'0'..=b'9') => {
                    self.pos += 1;
                    while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                        self.pos += 1;
                    }
                }
                Some(_) => {
                    return Err(InputDecodeError::SyntaxError {
                        message: "expected digit after decimal point in JSON number".to_string(),
                        offset: self.pos,
                    });
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }

        // Exponent part
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            self.pos += 1;
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                self.pos += 1;
            }
            match self.peek() {
                Some(b'0'..=b'9') => {
                    self.pos += 1;
                    while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                        self.pos += 1;
                    }
                }
                Some(_) => {
                    return Err(InputDecodeError::SyntaxError {
                        message: "expected digit in exponent of JSON number".to_string(),
                        offset: self.pos,
                    });
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }

        Ok(())
    }

    fn skip_array(&mut self) -> Result<(), InputDecodeError> {
        self.enter_depth()?;
        self.pos += 1; // consume '['
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    self.leave_depth();
                    return Ok(());
                }
                Some(_) => {
                    self.skip_any_value()?;
                    self.skip_whitespace();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            self.leave_depth();
                            return Ok(());
                        }
                        Some(other) => {
                            return Err(InputDecodeError::SyntaxError {
                                message: format!(
                                    "expected ',' or ']' in array, found '{}'",
                                    other as char
                                ),
                                offset: self.pos,
                            });
                        }
                        None => return Err(InputDecodeError::UnexpectedEof),
                    }
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }
    }

    fn skip_object(&mut self) -> Result<(), InputDecodeError> {
        self.enter_depth()?;
        self.pos += 1; // consume '{'
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    self.leave_depth();
                    return Ok(());
                }
                Some(b'"') => {
                    let _ = self.parse_string(self.limits.max_string_value_bytes)?;
                    self.consume(b':', "object key separator")?;
                    self.skip_any_value()?;
                    self.skip_whitespace();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            self.leave_depth();
                            return Ok(());
                        }
                        Some(other) => {
                            return Err(InputDecodeError::SyntaxError {
                                message: format!(
                                    "expected ',' or '}}' in object, found '{}'",
                                    other as char
                                ),
                                offset: self.pos,
                            });
                        }
                        None => return Err(InputDecodeError::UnexpectedEof),
                    }
                }
                Some(other) => {
                    return Err(InputDecodeError::SyntaxError {
                        message: format!(
                            "expected string key or '}}' in object, found '{}'",
                            other as char
                        ),
                        offset: self.pos,
                    });
                }
                None => return Err(InputDecodeError::UnexpectedEof),
            }
        }
    }

    fn skip_any_value(&mut self) -> Result<(), InputDecodeError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => {
                let _ = self.parse_string(self.limits.max_string_value_bytes)?;
                Ok(())
            }
            Some(b't') => self.consume_exact(b"true", "boolean true"),
            Some(b'f') => self.consume_exact(b"false", "boolean false"),
            Some(b'n') => self.consume_exact(b"null", "null"),
            Some(b'[') => self.skip_array(),
            Some(b'{') => self.skip_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_json_number(),
            Some(other) => Err(InputDecodeError::SyntaxError {
                message: format!("unexpected value token '{}'", other as char),
                offset: self.pos,
            }),
            None => Err(InputDecodeError::UnexpectedEof),
        }
    }

    fn consume_exact(
        &mut self,
        expected: &[u8],
        ctx: &'static str,
    ) -> Result<(), InputDecodeError> {
        if self.pos + expected.len() > self.bytes.len() {
            return Err(InputDecodeError::UnexpectedEof);
        }
        if &self.bytes[self.pos..self.pos + expected.len()] == expected {
            self.pos += expected.len();
            Ok(())
        } else {
            Err(InputDecodeError::SyntaxError {
                message: format!("expected {ctx}"),
                offset: self.pos,
            })
        }
    }
}

enum RawValue {
    String(String),
    Bool(bool),
    Number,
    Null,
    Array,
    Object,
}

impl RawValue {
    fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Bool(_) => "bool",
            Self::Number => "number",
            Self::Null => "null",
            Self::Array => "array",
            Self::Object => "object",
        }
    }
}

fn validate_identifier(name: &str, max_len: usize) -> Result<(), InputDecodeError> {
    if name.is_empty() {
        return Err(InputDecodeError::InvalidIdentifier {
            name: name.to_string(),
            reason: "identifier cannot be empty",
        });
    }
    if name.len() > max_len {
        return Err(InputDecodeError::NameTooLong { limit: max_len });
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(InputDecodeError::InvalidIdentifier {
            name: name.to_string(),
            reason: "identifier must start with ASCII letter or underscore",
        });
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' {
            return Err(InputDecodeError::InvalidIdentifier {
                name: name.to_string(),
                reason: "identifier may only contain ASCII alphanumeric characters and underscores",
            });
        }
    }
    Ok(())
}

fn validate_decimal_int(raw: &str) -> Result<(), InputDecodeError> {
    if raw.is_empty() {
        return Err(InputDecodeError::InvalidIntegerFormat {
            raw: raw.to_string(),
        });
    }
    let bytes = raw.as_bytes();
    let (is_neg, digits) = if bytes[0] == b'-' {
        (true, &bytes[1..])
    } else {
        (false, bytes)
    };
    if digits.is_empty() {
        return Err(InputDecodeError::InvalidIntegerFormat {
            raw: raw.to_string(),
        });
    }
    // Reject leading zeros: "0" is valid, "-0", "01", "-01" are invalid
    if digits[0] == b'0' && (is_neg || digits.len() > 1) {
        return Err(InputDecodeError::InvalidIntegerFormat {
            raw: raw.to_string(),
        });
    }
    for &b in digits {
        if !b.is_ascii_digit() {
            return Err(InputDecodeError::InvalidIntegerFormat {
                raw: raw.to_string(),
            });
        }
    }
    Ok(())
}

fn parse_hex_4(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 4 {
        return None;
    }
    let mut val: u16 = 0;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u16,
            b'a'..=b'f' => (b - b'a' + 10) as u16,
            b'A'..=b'F' => (b - b'A' + 10) as u16,
            _ => return None,
        };
        val = (val << 4) | digit;
    }
    Some(val)
}
