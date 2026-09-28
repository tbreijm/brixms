//! Immutable revision records, schema `brix.kb.revision@1` (ADR-0041).
//!
//! Each revision is a single file (`revisions/<seq>.json`), never rewritten
//! once `HEAD` names it (ADR-0041 §2.6), chained to its parent by digest. The digest
//! is computed over a canonical `brix_canon` preimage (never over the JSON
//! bytes themselves — JSON has no canonical byte form in this codebase, see
//! `spec/adr/ADR-0041_Persistent_Knowledge_Base.md` §"Why canon, not JSON,
//! for the revision digest"), under a fresh domain tag
//! (`"brix.kb.revision@1"`, `Domain::Value` — the same pattern `ContextId`/
//! `ConfigId` already use for a generic identity, ADR-0031 §D-IDENTITY).
//!
//! The record deliberately stores *digests* of the facts and candidate
//! dispositions a revision produced, not a second structured copy of them.
//! Facts and dispositions are re-derived by replay (`pipeline::replay`) every
//! time they are needed for display (`log`/`show`/`diff`); the stored digest
//! exists solely so `brix kb verify` can confirm that a replay reproduces
//! exactly what was recorded, without a duplicate on-disk representation that
//! could quietly drift from the truth.

use brix_canon::{CanonWriter, Digest, Domain};
use brix_lower::finite_decision::{
    CandidateDisposition, CandidateStatus, DerivedFact, FiniteDecisionDecideStop,
    FiniteDecisionProgramId, FiniteDecisionRun, FiniteDecisionStop,
};
use brix_lower::input::{InputScalarValue, InputSnapshotId};
use brix_lower::l3_v2::L3ValueV2;
use brix_semantic::ContextId;
use serde_json::json;

use crate::error::KbError;
use crate::strict_json::{self, Value as JsonValue};

pub const REVISION_SCHEMA: &str = "brix.kb.revision@1";
const REVISION_TAG: &str = "brix.kb.revision@1";
const VALUE_TAG: &str = "brix.kb.decision-value@1";
const FACTS_TAG: &str = "brix.kb.facts@1";
const OUTCOMES_TAG: &str = "brix.kb.outcomes@1";

/// The change that produced a revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The knowledge base was created from a program (and optionally an
    /// initial, possibly incomplete, input set).
    Init,
    /// One or more named inputs were asserted (inserted, or corrected if the
    /// name already had a value).
    Assert { names: Vec<String> },
    /// One or more named inputs were retracted.
    Retract { names: Vec<String> },
    /// The program changed. `dropped_inputs` lists previously-set input names
    /// that the new program no longer declares (or declares at a different
    /// type), which were therefore dropped from the input set.
    Program {
        previous_program_id: FiniteDecisionProgramId,
        dropped_inputs: Vec<String>,
    },
}

impl Change {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Change::Init => w.write_enum(0, |_| {}),
            Change::Assert { names } => {
                w.write_enum(1, |w| w.write_list(names.iter().map(ident_bytes)))
            }
            Change::Retract { names } => {
                w.write_enum(2, |w| w.write_list(names.iter().map(ident_bytes)))
            }
            Change::Program {
                previous_program_id,
                dropped_inputs,
            } => w.write_enum(3, |w| {
                w.write_bytes(previous_program_id.digest().as_bytes());
                w.write_list(dropped_inputs.iter().map(ident_bytes));
            }),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            Change::Init => json!({"kind": "init"}),
            Change::Assert { names } => json!({"kind": "assert", "names": names}),
            Change::Retract { names } => json!({"kind": "retract", "names": names}),
            Change::Program {
                previous_program_id,
                dropped_inputs,
            } => json!({
                "kind": "program",
                "previous_program_id": previous_program_id.digest().to_hex(),
                "dropped_inputs": dropped_inputs,
            }),
        }
    }

    fn from_json(v: &JsonValue) -> Result<Self, KbError> {
        let kind = v
            .field_str("kind")
            .map_err(|e| json_err("change.kind", e))?;
        match kind {
            "init" => {
                v.require_only_keys(&["kind"])
                    .map_err(|e| json_err("change", e))?;
                Ok(Change::Init)
            }
            "assert" => {
                v.require_only_keys(&["kind", "names"])
                    .map_err(|e| json_err("change", e))?;
                let names = v
                    .field_str_array("names")
                    .map_err(|e| json_err("change.names", e))?;
                Ok(Change::Assert { names })
            }
            "retract" => {
                v.require_only_keys(&["kind", "names"])
                    .map_err(|e| json_err("change", e))?;
                let names = v
                    .field_str_array("names")
                    .map_err(|e| json_err("change.names", e))?;
                Ok(Change::Retract { names })
            }
            "program" => {
                v.require_only_keys(&["kind", "previous_program_id", "dropped_inputs"])
                    .map_err(|e| json_err("change", e))?;
                let prev = v
                    .field_str("previous_program_id")
                    .map_err(|e| json_err("change.previous_program_id", e))?;
                let previous_program_id =
                    FiniteDecisionProgramId(hex_to_digest(prev, "change.previous_program_id")?);
                let dropped_inputs = v
                    .field_str_array("dropped_inputs")
                    .map_err(|e| json_err("change.dropped_inputs", e))?;
                Ok(Change::Program {
                    previous_program_id,
                    dropped_inputs,
                })
            }
            other => Err(KbError::rejected(
                "kb-revision-decode-error",
                format!("unknown change kind '{other}'"),
            )),
        }
    }
}

/// The outcome status of the decision recomputed for a revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Selected,
    Quiescent,
    Unknown,
    /// The input contract was incomplete: at least one declared input had no
    /// value, so the honest outcome is "missing input", not a fault inside a
    /// completed deliberation.
    MissingInputs,
}

impl Status {
    fn ordinal(self) -> u64 {
        match self {
            Status::Selected => 0,
            Status::Quiescent => 1,
            Status::Unknown => 2,
            Status::MissingInputs => 3,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Selected => "selected",
            Status::Quiescent => "quiescent",
            Status::Unknown => "unknown",
            Status::MissingInputs => "missing-inputs",
        }
    }

    fn from_str(s: &str) -> Result<Self, KbError> {
        match s {
            "selected" => Ok(Status::Selected),
            "quiescent" => Ok(Status::Quiescent),
            "unknown" => Ok(Status::Unknown),
            "missing-inputs" => Ok(Status::MissingInputs),
            other => Err(KbError::rejected(
                "kb-revision-decode-error",
                format!("unknown result status '{other}'"),
            )),
        }
    }
}

/// The recorded, re-derivable result of a revision's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionResult {
    pub status: Status,
    pub candidate: Option<String>,
    pub decision_digest: Option<Digest>,
    /// `None` only for [`Status::MissingInputs`]: a deliberation context binds
    /// a program, world, policy, and snapshot together, and nothing built one
    /// when the input contract was incomplete (`FiniteDecisionRuntime` was
    /// never constructed).
    pub context_id: Option<ContextId>,
    pub facts_digest: Option<Digest>,
    pub outcomes_digest: Option<Digest>,
    pub diagnostics: Vec<String>,
}

impl RevisionResult {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.status.ordinal());
        match &self.candidate {
            None => w.write_enum(0, |_| {}),
            Some(name) => w.write_enum(1, |w| w.write_ident(name)),
        }
        match &self.decision_digest {
            None => w.write_enum(0, |_| {}),
            Some(d) => w.write_enum(1, |w| w.write_bytes(d.as_bytes())),
        }
        match &self.context_id {
            None => w.write_enum(0, |_| {}),
            Some(c) => w.write_enum(1, |w| w.write_bytes(c.digest().as_bytes())),
        }
        match &self.facts_digest {
            None => w.write_enum(0, |_| {}),
            Some(d) => w.write_enum(1, |w| w.write_bytes(d.as_bytes())),
        }
        match &self.outcomes_digest {
            None => w.write_enum(0, |_| {}),
            Some(d) => w.write_enum(1, |w| w.write_bytes(d.as_bytes())),
        }
        w.write_list(self.diagnostics.iter().map(str_bytes));
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "status": self.status.as_str(),
            "candidate": self.candidate,
            "decision_digest": self.decision_digest.map(|d| d.to_hex()),
            "context_id": self.context_id.map(|c| c.digest().to_hex()),
            "facts_digest": self.facts_digest.map(|d| d.to_hex()),
            "outcomes_digest": self.outcomes_digest.map(|d| d.to_hex()),
            "diagnostics": self.diagnostics,
        })
    }

    fn from_json(v: &JsonValue) -> Result<Self, KbError> {
        v.require_only_keys(&[
            "status",
            "candidate",
            "decision_digest",
            "context_id",
            "facts_digest",
            "outcomes_digest",
            "diagnostics",
        ])
        .map_err(|e| json_err("result", e))?;
        let status = Status::from_str(
            v.field_str("status")
                .map_err(|e| json_err("result.status", e))?,
        )?;
        let candidate = v
            .field_opt_str("candidate")
            .map_err(|e| json_err("result.candidate", e))?
            .map(str::to_string);
        let decision_digest = v
            .field_opt_str("decision_digest")
            .map_err(|e| json_err("result.decision_digest", e))?
            .map(|h| hex_to_digest(h, "result.decision_digest"))
            .transpose()?;
        let context_id = v
            .field_opt_str("context_id")
            .map_err(|e| json_err("result.context_id", e))?
            .map(|h| hex_to_digest(h, "result.context_id"))
            .transpose()?
            .map(ContextId);
        let facts_digest = v
            .field_opt_str("facts_digest")
            .map_err(|e| json_err("result.facts_digest", e))?
            .map(|h| hex_to_digest(h, "result.facts_digest"))
            .transpose()?;
        let outcomes_digest = v
            .field_opt_str("outcomes_digest")
            .map_err(|e| json_err("result.outcomes_digest", e))?
            .map(|h| hex_to_digest(h, "result.outcomes_digest"))
            .transpose()?;
        let diagnostics = v
            .field_str_array("diagnostics")
            .map_err(|e| json_err("result.diagnostics", e))?;
        Ok(RevisionResult {
            status,
            candidate,
            decision_digest,
            context_id,
            facts_digest,
            outcomes_digest,
            diagnostics,
        })
    }
}

/// An immutable revision record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionRecord {
    pub seq: u64,
    pub parent: Option<Digest>,
    pub program_id: FiniteDecisionProgramId,
    pub program_path: String,
    pub snapshot_id: InputSnapshotId,
    pub snapshot_path: String,
    pub change: Change,
    pub result: RevisionResult,
}

impl RevisionRecord {
    /// This record's own content-addressed identity: a `brix_canon` digest
    /// over every other field, under `Domain::Value` with tag
    /// `"brix.kb.revision@1"`, chaining through `parent`.
    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(REVISION_TAG);
        w.write_uint(self.seq);
        match &self.parent {
            None => w.write_enum(0, |_| {}),
            Some(d) => w.write_enum(1, |w| w.write_bytes(d.as_bytes())),
        }
        w.write_bytes(self.program_id.digest().as_bytes());
        w.write_str(&self.program_path);
        w.write_bytes(self.snapshot_id.digest().as_bytes());
        w.write_str(&self.snapshot_path);
        self.change.canon_write(&mut w);
        self.result.canon_write(&mut w);
        w.digest(Domain::Value)
    }

    pub fn to_json_string(&self) -> String {
        let doc = json!({
            "schema": REVISION_SCHEMA,
            "seq": self.seq,
            "parent": self.parent.map(|d| d.to_hex()),
            "program_id": self.program_id.digest().to_hex(),
            "program_path": self.program_path,
            "snapshot_id": self.snapshot_id.digest().to_hex(),
            "snapshot_path": self.snapshot_path,
            "change": self.change.to_json(),
            "result": self.result.to_json(),
            "digest": self.digest().to_hex(),
        });
        serde_json::to_string_pretty(&doc).expect("revision JSON encoding cannot fail")
    }

    /// Strictly decode a revision record from bytes, verifying the stored
    /// `digest` field matches a fresh recomputation over the other fields —
    /// so decode alone catches a single-field edit that leaves `digest` stale.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KbError> {
        let v = strict_json::parse_document(bytes)
            .map_err(|e| KbError::unknown("kb-revision-decode-error", e.to_string()))?;
        v.require_only_keys(&[
            "schema",
            "seq",
            "parent",
            "program_id",
            "program_path",
            "snapshot_id",
            "snapshot_path",
            "change",
            "result",
            "digest",
        ])
        .map_err(|e| json_err("revision", e))?;
        let schema = v.field_str("schema").map_err(|e| json_err("schema", e))?;
        if schema != REVISION_SCHEMA {
            return Err(KbError::rejected(
                "kb-revision-schema-mismatch",
                format!("expected schema '{REVISION_SCHEMA}', found '{schema}'"),
            ));
        }
        let seq = v.field_u64("seq").map_err(|e| json_err("seq", e))?;
        let parent = v
            .field_opt_str("parent")
            .map_err(|e| json_err("parent", e))?
            .map(|h| hex_to_digest(h, "parent"))
            .transpose()?;
        let program_id = FiniteDecisionProgramId(hex_to_digest(
            v.field_str("program_id")
                .map_err(|e| json_err("program_id", e))?,
            "program_id",
        )?);
        let program_path = v
            .field_str("program_path")
            .map_err(|e| json_err("program_path", e))?
            .to_string();
        let snapshot_id = InputSnapshotId(hex_to_digest(
            v.field_str("snapshot_id")
                .map_err(|e| json_err("snapshot_id", e))?,
            "snapshot_id",
        )?);
        let snapshot_path = v
            .field_str("snapshot_path")
            .map_err(|e| json_err("snapshot_path", e))?
            .to_string();
        let change = Change::from_json(v.field_obj("change").map_err(|e| json_err("change", e))?)?;
        let result =
            RevisionResult::from_json(v.field_obj("result").map_err(|e| json_err("result", e))?)?;
        let stored_digest_hex = v.field_str("digest").map_err(|e| json_err("digest", e))?;
        let stored_digest = hex_to_digest(stored_digest_hex, "digest")?;

        let record = RevisionRecord {
            seq,
            parent,
            program_id,
            program_path,
            snapshot_id,
            snapshot_path,
            change,
            result,
        };
        let recomputed = record.digest();
        if recomputed != stored_digest {
            return Err(KbError::unknown(
                "kb-revision-digest-mismatch",
                format!(
                    "revision {seq} record digest mismatch: stored {}, recomputed {} (the file was edited after being written)",
                    stored_digest.to_hex(),
                    recomputed.to_hex()
                ),
            ));
        }
        Ok(record)
    }
}

fn ident_bytes(s: impl AsRef<str>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_ident(s.as_ref());
    w.finish()
}

fn str_bytes(s: impl AsRef<str>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_str(s.as_ref());
    w.finish()
}

fn json_err(field: &str, e: strict_json::JsonError) -> KbError {
    KbError::unknown(
        "kb-revision-decode-error",
        format!("revision record field '{field}': {e}"),
    )
}

/// Decode a 64-hex-character digest, in a named field, for a diagnostic that
/// says which field was malformed.
pub fn hex_to_digest(hex: &str, field: &str) -> Result<Digest, KbError> {
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(KbError::unknown(
            "kb-revision-decode-error",
            format!("field '{field}': expected 64 hex characters, found '{hex}'"),
        ));
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| {
            KbError::unknown(
                "kb-revision-decode-error",
                format!("field '{field}': invalid hex digit in '{hex}'"),
            )
        })?;
    }
    Ok(Digest::from_bytes(bytes))
}

/// Canonical digest of a single decision value, reusing [`InputScalarValue`]'s
/// real `Canonical` encoding (not a bespoke one) — the same bytes `brix-kb`
/// would use if this value were ever re-supplied as an input.
pub fn digest_decision_value(value: &L3ValueV2) -> Result<Digest, KbError> {
    let scalar = InputScalarValue::from_l3_value(value).ok_or_else(|| {
        KbError::unknown(
            "kb-value-digest-error",
            "decision value could not be converted to a transport value for digesting".to_string(),
        )
    })?;
    let mut w = CanonWriter::new();
    w.write_tag(VALUE_TAG);
    brix_canon::Canonical::canon_write(&scalar, &mut w);
    Ok(w.digest(Domain::Value))
}

/// A single combined digest over every derived fact, in the run's own
/// (declaration-ordinal) order — this is an internal integrity check specific
/// to `brix-kb`'s tamper detection, not a portable semantic identity of the
/// kind `SOC-LAW-01` governs.
pub fn digest_facts(facts: &[DerivedFact]) -> Result<Digest, KbError> {
    let mut w = CanonWriter::new();
    w.write_tag(FACTS_TAG);
    w.write_uint(facts.len() as u64);
    for f in facts {
        w.write_ident(&f.rule);
        w.write_uint(f.ordinal);
        let scalar = InputScalarValue::from_l3_value(&f.value).ok_or_else(|| {
            KbError::unknown(
                "kb-value-digest-error",
                format!(
                    "fact '{}' value could not be converted for digesting",
                    f.rule
                ),
            )
        })?;
        brix_canon::Canonical::canon_write(&scalar, &mut w);
    }
    Ok(w.digest(Domain::Value))
}

/// A single combined digest over every decision the run declares: each
/// commit pool (ADR-0039) and each `decide` block instance (ADR-0043), with
/// its stop, its selected candidate and value, and every candidate's
/// disposition, in journal order. `kb verify` compares this against a fresh
/// replay, so a change in any decision, not only the first commit pool's, is
/// detected.
///
/// `CandidateStatus`'s and the Unknown reason's `Display` renderings are used
/// as encodings here, which is fine for this internal drift check (both are
/// hand-written and deterministic) even though `Display` is not in general
/// suitable for a canonical semantic identity.
pub fn digest_outcomes(run: &FiniteDecisionRun) -> Result<Digest, KbError> {
    let mut w = CanonWriter::new();
    w.write_tag(OUTCOMES_TAG);
    w.write_uint(run.commits.len() as u64);
    for pool in &run.commits {
        w.write_ident(&pool.commit);
        write_stop(&mut w, &pool.stop)?;
        write_dispositions(&mut w, &pool.dispositions);
    }
    w.write_uint(run.decides.len() as u64);
    for block in &run.decides {
        w.write_ident(&block.decide);
        match &block.stop {
            FiniteDecisionDecideStop::Settled => w.write_enum(0, |_| {}),
            FiniteDecisionDecideStop::Unknown(reason) => {
                let reason = reason.to_string();
                w.write_enum(1, |w| w.write_str(&reason))
            }
        }
        w.write_uint(block.instances.len() as u64);
        for inst in &block.instances {
            w.write_uint(inst.index as u64);
            w.write_bytes(digest_decision_value(&inst.binder)?.as_bytes());
            write_stop(&mut w, &inst.stop)?;
            write_dispositions(&mut w, &inst.dispositions);
        }
    }
    Ok(w.digest(Domain::Value))
}

fn write_stop(w: &mut CanonWriter, stop: &FiniteDecisionStop) -> Result<(), KbError> {
    match stop {
        FiniteDecisionStop::Selected(sel) => {
            let value = digest_decision_value(&sel.value)?;
            w.write_enum(0, |w| {
                w.write_ident(&sel.candidate);
                w.write_bytes(value.as_bytes());
            });
        }
        FiniteDecisionStop::Quiescent { .. } => w.write_enum(1, |_| {}),
        FiniteDecisionStop::Unknown(reason) => {
            let reason = reason.to_string();
            w.write_enum(2, |w| w.write_str(&reason));
        }
    }
    Ok(())
}

fn write_dispositions(w: &mut CanonWriter, dispositions: &[CandidateDisposition]) {
    w.write_uint(dispositions.len() as u64);
    for d in dispositions {
        w.write_ident(&d.name);
        w.write_uint(d.priority);
        w.write_str(&status_display(&d.status));
    }
}

fn status_display(status: &CandidateStatus) -> String {
    status.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(seq: u64, parent: Option<Digest>) -> RevisionRecord {
        RevisionRecord {
            seq,
            parent,
            program_id: FiniteDecisionProgramId(Digest::of(Domain::Value, b"program")),
            program_path: "programs/aa.brix".to_string(),
            snapshot_id: InputSnapshotId(Digest::of(Domain::Value, b"snapshot")),
            snapshot_path: "snapshots/bb.json".to_string(),
            change: Change::Init,
            result: RevisionResult {
                status: Status::Selected,
                candidate: Some("ship".to_string()),
                decision_digest: Some(Digest::of(Domain::Value, b"decision")),
                context_id: Some(ContextId(Digest::of(Domain::Value, b"context"))),
                facts_digest: Some(Digest::of(Domain::Value, b"facts")),
                outcomes_digest: Some(Digest::of(Domain::Value, b"dispositions")),
                diagnostics: vec![],
            },
        }
    }

    #[test]
    fn test_roundtrip_json() {
        let rec = sample_record(1, None);
        let json = rec.to_json_string();
        let decoded = RevisionRecord::from_bytes(json.as_bytes()).expect("decode");
        assert_eq!(decoded, rec);
    }

    #[test]
    fn test_roundtrip_with_parent_and_assert_change() {
        let parent_digest = sample_record(1, None).digest();
        let mut rec = sample_record(2, Some(parent_digest));
        rec.change = Change::Assert {
            names: vec!["stock".to_string(), "eligible".to_string()],
        };
        let json = rec.to_json_string();
        let decoded = RevisionRecord::from_bytes(json.as_bytes()).expect("decode");
        assert_eq!(decoded, rec);
    }

    #[test]
    fn test_tamper_detected_via_digest_mismatch() {
        let rec = sample_record(1, None);
        let json = rec.to_json_string();
        let tampered = json.replace("\"programs/aa.brix\"", "\"programs/evil.brix\"");
        let err = RevisionRecord::from_bytes(tampered.as_bytes()).unwrap_err();
        assert_eq!(err.code, "kb-revision-digest-mismatch");
    }

    #[test]
    fn test_rejects_duplicate_key() {
        let rec = sample_record(1, None);
        let json = rec.to_json_string();
        // Inject a duplicate "seq" key right after the schema field.
        let tampered = json.replacen("\"seq\": 1,", "\"seq\": 1,\n  \"seq\": 1,", 1);
        let err = RevisionRecord::from_bytes(tampered.as_bytes()).unwrap_err();
        assert_eq!(err.code, "kb-revision-decode-error");
    }

    #[test]
    fn test_rejects_unknown_field() {
        let rec = sample_record(1, None);
        let json = rec.to_json_string();
        let tampered = json.replacen("\"seq\": 1,", "\"seq\": 1,\n  \"bogus\": 1,", 1);
        let err = RevisionRecord::from_bytes(tampered.as_bytes()).unwrap_err();
        assert_eq!(err.code, "kb-revision-decode-error");
    }

    #[test]
    fn test_different_seq_or_parent_changes_digest() {
        let a = sample_record(1, None);
        let b = sample_record(2, None);
        assert_ne!(a.digest(), b.digest());

        let c = sample_record(2, Some(Digest::of(Domain::Value, b"other-parent")));
        assert_ne!(b.digest(), c.digest());
    }
}
