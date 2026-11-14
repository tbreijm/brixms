//! Portable request-context binding for finite-decision audit bundles.
//!
//! The frozen V1 bundle remains byte-for-byte unchanged. This additive wrapper
//! carries those exact bytes, the already checked request-context bytes, and
//! the ordered input payloads needed to reconstruct the input snapshot during
//! replay. Every identity below is recomputed from a versioned Brix canonical
//! preimage; JSON fields are only a transport representation.

use brix_canon::{CanonWriter, Digest, Domain};
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub const REQUEST_AUDIT_SCHEMA: &str = "brix.audit-request-evidence@1";
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_INPUT_FILES: usize = brix_lower::input::MAX_INPUT_FILES;
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ONE_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTEXT_BYTES: usize = 1024 * 1024;
pub const MAX_REQUEST_AUDIT_WRAPPER_BYTES: usize = (2 * MAX_BUNDLE_BYTES)
    + (2 * MAX_INPUT_BYTES)
    + (2 * MAX_CONTEXT_BYTES)
    + (MAX_INPUT_FILES * 512)
    + (64 * 1024);
static TEMP_INPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One exact, ordered input supplied to `brix audit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableAuditInput {
    /// Stable transport label (`input-000000`, etc.); never a host path.
    pub name: String,
    /// Original file bytes, including evidence-environment files.
    pub bytes: Vec<u8>,
}

/// Decoded and integrity-checked request audit wrapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestAuditEvidenceV1 {
    pub v1_bundle: Vec<u8>,
    pub context_id: Option<String>,
    pub context_bytes: Option<Vec<u8>>,
    pub inputs: Vec<PortableAuditInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestAuditError(pub String);

impl std::fmt::Display for RequestAuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RequestAuditError {}

impl RequestAuditEvidenceV1 {
    /// Recognize the wrapper schema without decoding embedded payloads.
    pub fn is_wrapper_bytes(bytes: &[u8]) -> Result<bool, RequestAuditError> {
        if bytes.len() > MAX_REQUEST_AUDIT_WRAPPER_BYTES {
            return Err(RequestAuditError("audit bundle exceeds byte limit".into()));
        }
        // The canonical transport starts with the `inputs` array. Ordinary V1
        // bundle bytes are opaque and must not be fed into a JSON parser just
        // because a prefix happens to be valid UTF-8.
        if !bytes.starts_with(b"{\"inputs\":[") {
            return Ok(false);
        }
        let probe = match parse_wrapper_context_probe(bytes) {
            Ok(probe) => probe,
            Err(_) => return Ok(false),
        };
        Ok(probe.schema == REQUEST_AUDIT_SCHEMA)
    }

    /// Read only context metadata so callers can compare it to independently
    /// authorized expectations before decoding or materializing the payloads.
    pub fn inspect_context(
        bytes: &[u8],
    ) -> Result<(Option<String>, Option<Vec<u8>>), RequestAuditError> {
        let probe = parse_wrapper_context_probe(bytes)?;
        if probe.schema != REQUEST_AUDIT_SCHEMA {
            return Err(RequestAuditError(
                "unsupported request audit wrapper schema".into(),
            ));
        }
        let context_bytes = probe
            .context_hex
            .as_deref()
            .map(|hex| decode_hex_bounded(hex, MAX_CONTEXT_BYTES))
            .transpose()?;
        validate_context_pair(probe.context_id.as_deref(), context_bytes.as_deref())?;
        Ok((probe.context_id, context_bytes))
    }

    /// Encode a canonical portable wrapper around the unchanged V1 bundle.
    pub fn encode(&self) -> Result<Vec<u8>, RequestAuditError> {
        self.validate_bounds()?;
        validate_context_pair(self.context_id.as_deref(), self.context_bytes.as_deref())?;
        let mut root = Map::new();
        root.insert("schema".into(), Value::String(REQUEST_AUDIT_SCHEMA.into()));
        root.insert(
            "v1_bundle_hex".into(),
            Value::String(encode_hex(&self.v1_bundle)),
        );
        root.insert(
            "v1_bundle_digest".into(),
            Value::String(payload_digest("brix.audit-v1-bundle@1", &self.v1_bundle)),
        );
        root.insert(
            "request_context_id".into(),
            self.context_id.clone().map_or(Value::Null, Value::String),
        );
        root.insert(
            "request_context_hex".into(),
            self.context_bytes
                .as_ref()
                .map(|b| Value::String(encode_hex(b)))
                .unwrap_or(Value::Null),
        );
        root.insert(
            "inputs".into(),
            Value::Array(
                self.inputs
                    .iter()
                    .map(|input| {
                        let mut item = Map::new();
                        item.insert("name".into(), Value::String(input.name.clone()));
                        item.insert("bytes_hex".into(), Value::String(encode_hex(&input.bytes)));
                        item.insert(
                            "digest".into(),
                            Value::String(payload_digest("brix.audit-input-file@1", &input.bytes)),
                        );
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
        let identity = self.identity_preimage();
        root.insert(
            "wrapper_id".into(),
            Value::String(Digest::of(Domain::Value, &identity).to_hex()),
        );
        let bytes = serde_json::to_vec(&Value::Object(root))
            .map_err(|e| RequestAuditError(e.to_string()))?;
        if bytes.len() > MAX_REQUEST_AUDIT_WRAPPER_BYTES {
            return Err(RequestAuditError(
                "encoded request audit wrapper exceeds byte limit".into(),
            ));
        }
        Ok(bytes)
    }

    /// Decode a bounded canonical wrapper, rejecting duplicate/unknown fields,
    /// noncanonical JSON, forged payload digests, and an invalid wrapper ID.
    pub fn decode(bytes: &[u8]) -> Result<Self, RequestAuditError> {
        let value = parse_unique_wrapper(bytes)?;
        let canonical = serde_json::to_vec(&value)
            .map_err(|e| RequestAuditError(format!("wrapper JSON encoding failed: {e}")))?;
        if canonical != bytes {
            return Err(RequestAuditError(
                "request audit wrapper JSON is not canonical".into(),
            ));
        }
        let root = exact_object(
            &value,
            &[
                "schema",
                "v1_bundle_hex",
                "v1_bundle_digest",
                "request_context_id",
                "request_context_hex",
                "inputs",
                "wrapper_id",
            ],
        )?;
        if get_string(root, "schema")? != REQUEST_AUDIT_SCHEMA {
            return Err(RequestAuditError(
                "unsupported request audit wrapper schema".into(),
            ));
        }
        let v1_bundle = decode_hex_bounded(get_string(root, "v1_bundle_hex")?, MAX_BUNDLE_BYTES)?;
        if get_string(root, "v1_bundle_digest")?
            != payload_digest("brix.audit-v1-bundle@1", &v1_bundle)
        {
            return Err(RequestAuditError("V1 bundle digest mismatch".into()));
        }
        let context_id = nullable_string(root, "request_context_id")?;
        let context_hex = nullable_string(root, "request_context_hex")?;
        let context_bytes = context_hex
            .as_deref()
            .map(|hex| decode_hex_bounded(hex, MAX_CONTEXT_BYTES))
            .transpose()?;
        validate_context_pair(context_id.as_deref(), context_bytes.as_deref())?;

        let input_values = root
            .get("inputs")
            .and_then(Value::as_array)
            .ok_or_else(|| RequestAuditError("wrapper inputs must be an array".into()))?;
        if input_values.len() > MAX_INPUT_FILES {
            return Err(RequestAuditError("too many portable audit inputs".into()));
        }
        let mut inputs = Vec::with_capacity(input_values.len());
        let mut total_input_bytes = 0usize;
        for (index, value) in input_values.iter().enumerate() {
            let item = exact_object(value, &["name", "bytes_hex", "digest"])?;
            let name = get_string(item, "name")?.to_string();
            if name != format!("input-{index:06}") {
                return Err(RequestAuditError(
                    "portable input labels are not canonical".into(),
                ));
            }
            let input_bytes =
                decode_hex_bounded(get_string(item, "bytes_hex")?, MAX_ONE_INPUT_BYTES)?;
            total_input_bytes = total_input_bytes
                .checked_add(input_bytes.len())
                .ok_or_else(|| RequestAuditError("input byte count overflow".into()))?;
            if total_input_bytes > MAX_INPUT_BYTES {
                return Err(RequestAuditError(
                    "aggregate portable input byte limit exceeded".into(),
                ));
            }
            if get_string(item, "digest")?
                != payload_digest("brix.audit-input-file@1", &input_bytes)
            {
                return Err(RequestAuditError("portable input digest mismatch".into()));
            }
            inputs.push(PortableAuditInput {
                name,
                bytes: input_bytes,
            });
        }
        let decoded = Self {
            v1_bundle,
            context_id,
            context_bytes,
            inputs,
        };
        let want = Digest::of(Domain::Value, &decoded.identity_preimage()).to_hex();
        if get_string(root, "wrapper_id")? != want {
            return Err(RequestAuditError(
                "request audit wrapper identity mismatch".into(),
            ));
        }
        decoded.validate_bounds()?;
        Ok(decoded)
    }

    /// Enforce the independently selected per-request file and aggregate
    /// budgets after the wrapper context has been compared and before shard
    /// materialization/replay.
    pub fn validate_input_budgets(
        &self,
        limits: &crate::request_evidence::RequestEvidenceLimits,
    ) -> Result<(), RequestAuditError> {
        if self.v1_bundle.len() > limits.max_evidence_bundle_bytes() {
            return Err(RequestAuditError(
                "embedded V1 bundle exceeds request policy".into(),
            ));
        }
        if self.inputs.len() > limits.max_input_files() {
            return Err(RequestAuditError(
                "portable input count exceeds request policy".into(),
            ));
        }
        let mut input_total = 0usize;
        let mut evidence_total = 0usize;
        for input in &self.inputs {
            if input_is_evidence(&input.bytes) {
                if input.bytes.len() > limits.max_evidence_artifact_bytes() {
                    return Err(RequestAuditError(
                        "portable evidence file exceeds request policy".into(),
                    ));
                }
                evidence_total = evidence_total
                    .checked_add(input.bytes.len())
                    .ok_or_else(|| RequestAuditError("evidence byte count overflow".into()))?;
                if evidence_total > limits.max_evidence_aggregate_bytes() {
                    return Err(RequestAuditError(
                        "portable evidence aggregate exceeds request policy".into(),
                    ));
                }
            } else {
                if input.bytes.len() > limits.max_input_file_bytes() {
                    return Err(RequestAuditError(
                        "portable input file exceeds request policy".into(),
                    ));
                }
                input_total = input_total
                    .checked_add(input.bytes.len())
                    .ok_or_else(|| RequestAuditError("input byte count overflow".into()))?;
                if input_total > limits.max_input_aggregate_bytes() {
                    return Err(RequestAuditError(
                        "portable input aggregate exceeds request policy".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_bounds(&self) -> Result<(), RequestAuditError> {
        if self.v1_bundle.len() > MAX_BUNDLE_BYTES {
            return Err(RequestAuditError("V1 bundle exceeds byte limit".into()));
        }
        if self.inputs.len() > MAX_INPUT_FILES {
            return Err(RequestAuditError("too many portable audit inputs".into()));
        }
        if self
            .context_bytes
            .as_ref()
            .is_some_and(|b| b.len() > MAX_CONTEXT_BYTES)
        {
            return Err(RequestAuditError(
                "request context exceeds byte limit".into(),
            ));
        }
        let mut total = 0usize;
        for (index, input) in self.inputs.iter().enumerate() {
            if input.name != format!("input-{index:06}") {
                return Err(RequestAuditError(
                    "portable input labels are not canonical".into(),
                ));
            }
            if input.bytes.len() > MAX_ONE_INPUT_BYTES {
                return Err(RequestAuditError(
                    "portable input exceeds byte limit".into(),
                ));
            }
            total = total
                .checked_add(input.bytes.len())
                .ok_or_else(|| RequestAuditError("input byte count overflow".into()))?;
        }
        if total > MAX_INPUT_BYTES {
            return Err(RequestAuditError(
                "aggregate portable input byte limit exceeded".into(),
            ));
        }
        Ok(())
    }

    fn identity_preimage(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.write_tag(REQUEST_AUDIT_SCHEMA);
        w.write_bytes(&self.v1_bundle);
        match (&self.context_id, &self.context_bytes) {
            (Some(id), Some(bytes)) => {
                w.write_bool(true);
                w.write_str(id);
                w.write_bytes(bytes);
            }
            _ => w.write_bool(false),
        }
        w.write_uint(self.inputs.len() as u64);
        for input in &self.inputs {
            w.write_ident(&input.name);
            w.write_bytes(&input.bytes);
        }
        w.finish()
    }
}

fn input_is_evidence(bytes: &[u8]) -> bool {
    crate::request_evidence::parse_unique_evidence_json(bytes)
        .ok()
        .and_then(|value| {
            value
                .get("schema")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|schema| schema == "brix.evidence.env@1")
}

/// Read the exact ordered CLI inputs under the same aggregate bound used by
/// the request wrapper. The caller should do this only after policy and plan
/// authorization has succeeded.
pub fn capture_input_files(
    paths: &[PathBuf],
) -> Result<Vec<PortableAuditInput>, RequestAuditError> {
    if paths.len() > MAX_INPUT_FILES {
        return Err(RequestAuditError("too many audit input files".into()));
    }
    let mut inputs = Vec::with_capacity(paths.len());
    let mut total = 0usize;
    for (index, path) in paths.iter().enumerate() {
        let mut bytes = Vec::new();
        let mut file = std::fs::File::open(path)
            .map_err(|e| RequestAuditError(format!("cannot read audit input: {e}")))?;
        file.by_ref()
            .take((MAX_ONE_INPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| RequestAuditError(format!("cannot read audit input: {e}")))?;
        if bytes.len() > MAX_ONE_INPUT_BYTES {
            return Err(RequestAuditError(
                "audit input exceeds per-file byte limit".into(),
            ));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| RequestAuditError("input byte count overflow".into()))?;
        if total > MAX_INPUT_BYTES {
            return Err(RequestAuditError(
                "aggregate audit input byte limit exceeded".into(),
            ));
        }
        inputs.push(PortableAuditInput {
            name: format!("input-{index:06}"),
            bytes,
        });
    }
    Ok(inputs)
}

/// A short-lived private directory containing `create_new` materializations of
/// portable payloads. This gives the existing frozen shard/evidence loader an
/// exact byte stream without reopening caller paths.
pub struct MaterializedAuditInputs {
    directory: PathBuf,
    paths: Vec<PathBuf>,
}

impl MaterializedAuditInputs {
    pub fn create(inputs: &[PortableAuditInput]) -> Result<Self, RequestAuditError> {
        if inputs.len() > MAX_INPUT_FILES {
            return Err(RequestAuditError("too many portable audit inputs".into()));
        }
        let base = std::env::temp_dir();
        let mut directory = None;
        for _ in 0..64 {
            let sequence = TEMP_INPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = base.join(format!(
                "brix-audit-evidence-{}-{sequence}",
                std::process::id()
            ));
            match create_private_directory(&candidate) {
                Ok(()) => {
                    directory = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(RequestAuditError(format!(
                        "cannot create temporary audit input directory: {error}"
                    )))
                }
            }
        }
        let directory = directory.ok_or_else(|| {
            RequestAuditError("cannot allocate temporary audit input directory".into())
        })?;
        let mut result = Self {
            directory,
            paths: Vec::with_capacity(inputs.len()),
        };
        let mut total = 0usize;
        for (index, input) in inputs.iter().enumerate() {
            if input.name != format!("input-{index:06}") || input.bytes.len() > MAX_ONE_INPUT_BYTES
            {
                return Err(RequestAuditError("invalid portable audit input".into()));
            }
            total = total
                .checked_add(input.bytes.len())
                .ok_or_else(|| RequestAuditError("input byte count overflow".into()))?;
            if total > MAX_INPUT_BYTES {
                return Err(RequestAuditError(
                    "aggregate audit input byte limit exceeded".into(),
                ));
            }
            let path = result.directory.join(&input.name);
            let mut file = create_private_new_file(&path).map_err(|e| {
                RequestAuditError(format!("cannot create temporary audit input: {e}"))
            })?;
            std::io::Write::write_all(&mut file, &input.bytes).map_err(|e| {
                RequestAuditError(format!("cannot write temporary audit input: {e}"))
            })?;
            result.paths.push(path);
        }
        Ok(result)
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

#[cfg(unix)]
fn create_private_directory(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(unix)]
fn create_private_new_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(not(unix))]
fn create_private_new_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

impl Drop for MaterializedAuditInputs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn validate_context_pair(id: Option<&str>, bytes: Option<&[u8]>) -> Result<(), RequestAuditError> {
    match (id, bytes) {
        (None, None) => Ok(()),
        (Some(id), Some(bytes)) if !bytes.is_empty() && is_lower_hex_digest(id) => Ok(()),
        _ => Err(RequestAuditError(
            "request context ID and bytes must be present together and valid".into(),
        )),
    }
}

fn parse_unique_wrapper(bytes: &[u8]) -> Result<Value, RequestAuditError> {
    if bytes.len() > MAX_REQUEST_AUDIT_WRAPPER_BYTES {
        return Err(RequestAuditError(
            "request audit wrapper exceeds byte limit".into(),
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| RequestAuditError("request audit wrapper is not UTF-8".into()))?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = UniqueJson::deserialize(&mut deserializer)
        .map_err(|e| RequestAuditError(format!("invalid request audit wrapper: {e}")))?
        .0;
    deserializer
        .end()
        .map_err(|e| RequestAuditError(format!("invalid request audit wrapper: {e}")))?;
    let canonical = serde_json::to_vec(&value)
        .map_err(|e| RequestAuditError(format!("wrapper JSON encoding failed: {e}")))?;
    if canonical != bytes {
        return Err(RequestAuditError(
            "request audit wrapper JSON is not canonical".into(),
        ));
    }
    Ok(value)
}

#[derive(Debug)]
struct WrapperContextProbe {
    schema: String,
    context_id: Option<String>,
    context_hex: Option<String>,
}

impl<'de> Deserialize<'de> for WrapperContextProbe {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProbeVisitor;
        impl<'de> Visitor<'de> for ProbeVisitor {
            type Value = WrapperContextProbe;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a request audit evidence wrapper object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let allowed = [
                    "schema",
                    "v1_bundle_hex",
                    "v1_bundle_digest",
                    "request_context_id",
                    "request_context_hex",
                    "inputs",
                    "wrapper_id",
                ];
                let mut seen = std::collections::BTreeSet::new();
                let mut schema = None;
                let mut context_id = None;
                let mut context_hex = None;
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom("duplicate wrapper field"));
                    }
                    if !allowed.contains(&key.as_str()) {
                        return Err(serde::de::Error::custom("unknown wrapper field"));
                    }
                    match key.as_str() {
                        "schema" => schema = Some(map.next_value::<String>()?),
                        "request_context_id" => {
                            context_id = Some(map.next_value::<Option<String>>()?)
                        }
                        "request_context_hex" => {
                            context_hex = Some(map.next_value::<Option<String>>()?)
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                if seen.len() != allowed.len() {
                    return Err(serde::de::Error::custom("missing wrapper field"));
                }
                Ok(WrapperContextProbe {
                    schema: schema.ok_or_else(|| serde::de::Error::custom("missing schema"))?,
                    context_id: context_id
                        .ok_or_else(|| serde::de::Error::custom("missing request_context_id"))?,
                    context_hex: context_hex
                        .ok_or_else(|| serde::de::Error::custom("missing request_context_hex"))?,
                })
            }
        }
        deserializer.deserialize_map(ProbeVisitor)
    }
}

fn parse_wrapper_context_probe(bytes: &[u8]) -> Result<WrapperContextProbe, RequestAuditError> {
    if bytes.len() > MAX_REQUEST_AUDIT_WRAPPER_BYTES {
        return Err(RequestAuditError(
            "request audit wrapper exceeds byte limit".into(),
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let probe = WrapperContextProbe::deserialize(&mut deserializer)
        .map_err(|e| RequestAuditError(format!("invalid request audit wrapper: {e}")))?;
    deserializer
        .end()
        .map_err(|e| RequestAuditError(format!("invalid request audit wrapper: {e}")))?;
    Ok(probe)
}

fn payload_digest(domain: &str, bytes: &[u8]) -> String {
    let mut w = CanonWriter::new();
    w.write_tag(domain);
    w.write_bytes(bytes);
    Digest::of(Domain::Value, &w.finish()).to_hex()
}

fn exact_object<'a>(
    value: &'a Value,
    keys: &[&str],
) -> Result<&'a Map<String, Value>, RequestAuditError> {
    let map = value
        .as_object()
        .ok_or_else(|| RequestAuditError("expected JSON object".into()))?;
    if map.len() != keys.len() || map.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(RequestAuditError(
            "request audit wrapper has missing or unknown fields".into(),
        ));
    }
    Ok(map)
}

fn get_string<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str, RequestAuditError> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| RequestAuditError(format!("wrapper field '{key}' must be a string")))
}

fn nullable_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, RequestAuditError> {
    match map.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(RequestAuditError(format!(
            "wrapper field '{key}' must be a string or null"
        ))),
    }
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex_bounded(text: &str, max_bytes: usize) -> Result<Vec<u8>, RequestAuditError> {
    if !text.len().is_multiple_of(2) || text.len() / 2 > max_bytes {
        return Err(RequestAuditError(
            "hex payload exceeds its byte limit or has odd length".into(),
        ));
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks_exact(2) {
        let hi = hex_nibble(pair[0])
            .ok_or_else(|| RequestAuditError("invalid lowercase hex payload".into()))?;
        let lo = hex_nibble(pair[1])
            .ok_or_else(|| RequestAuditError("invalid lowercase hex payload".into()))?;
        out.push((hi << 4) | lo);
    }
    if encode_hex(&out) != text {
        return Err(RequestAuditError(
            "hex payload is not lowercase canonical hex".into(),
        ));
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(Number::from(value))))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(Number::from(value))))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueJson>()? {
                    values.push(value.0);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueJson>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate object key '{key}'"
                        )));
                    }
                    values.insert(key, value.0);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapper() -> RequestAuditEvidenceV1 {
        RequestAuditEvidenceV1 {
            v1_bundle: b"frozen-v1-bytes".to_vec(),
            context_id: None,
            context_bytes: None,
            inputs: vec![PortableAuditInput {
                name: "input-000000".into(),
                bytes: br#"{"schema":"brix.input-shard@1"}"#.to_vec(),
            }],
        }
    }

    #[test]
    fn wrapper_round_trips_canonical_payloads() {
        let input = wrapper();
        let bytes = input.encode().unwrap();
        let decoded = RequestAuditEvidenceV1::decode(&bytes).unwrap();
        assert_eq!(decoded, input);
        assert_eq!(bytes, decoded.encode().unwrap());
    }

    #[test]
    fn wrapper_rejects_duplicate_keys_and_changed_payload() {
        assert!(RequestAuditEvidenceV1::decode(br#"{"schema":"a","schema":"b"}"#).is_err());
        assert!(
            !RequestAuditEvidenceV1::is_wrapper_bytes(b"{arbitrary-valid-utf8-v1-prefix").unwrap()
        );
        let mut bytes = wrapper().encode().unwrap();
        let at = bytes
            .windows(b"66726f7a656e2d76312d6279746573".len())
            .position(|window| window == b"66726f7a656e2d76312d6279746573")
            .unwrap();
        bytes[at] ^= 1;
        assert!(RequestAuditEvidenceV1::decode(&bytes).is_err());
    }
}
