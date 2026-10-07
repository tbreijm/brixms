//! KB v1 → world import (P6 step 8, `docs/planning/p6-audit-contract.md` §5).
//!
//! `brix world import-kb <kb-dir> <world-dir>`: read-only on `<kb-dir>`,
//! refuses if `<world-dir>` exists. Builds a **storage-only** world
//! (`program_required = false`) with one relation `kb_inputs`, one world
//! revision per KB revision, and a provenance record attributing every KB
//! result to KB v1 as a claim — never as a world decision (`decision_root`
//! stays `None` throughout, since a storage-only world has no `decide`
//! blocks).
//!
//! # World-revision numbering
//!
//! A KB v1 revision log is numbered `1..=head` (`brix_kb::ops::init` writes
//! the first record as `seq: 1`; there is no KB revision 0). This importer
//! applies one [`WorldBatch`] per KB revision in that same range, starting
//! from the world's own unavoidable structural empty genesis (world revision
//! 0, written by [`WorldSession::create`] — a concurrent lane's file, not
//! edited here, and with no public constructor that seeds an initial batch).
//! Each applied batch therefore lands as world revision `kb_seq`, so
//! `world_seq == kb_seq` throughout, which the provenance record's
//! `world_seq` field makes checkable rather than merely asserted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brix_canon::{Digest, Domain};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, decode_input_shard_from_file, InputLimits,
    InputSnapshot, InputValue,
};

use crate::ops;
use crate::paths as kb_paths;
use crate::revision::Change;
use crate::snapshot_io::encode_input_snapshot_v2;

use super::batch::{WorldBatch, WorldBatchOp};
use super::codec::TupleRecord;
use super::error::WorldError;
use super::manifest::{RelationDecl, WorldManifest};
use super::session::WorldSession;
use super::types::{WorldKey, WorldTuple};

/// The one KB profile this importer accepts (§5 step 5).
pub const SUPPORTED_KB_PROFILE: &str = "brix.l3.finite-decision@1";

/// The storage-only world's one relation.
pub const KB_INPUTS_RELATION: &str = "kb_inputs";

/// Schema marker for the provenance sidecar (§5 step 4).
pub const IMPORT_PROVENANCE_SCHEMA: &str = "brix.world.import-kb@1";

/// Errors refusing a KB v1 → world import (§5 step 5: every refusal is
/// `Unknown(import-unsupported:<reason>)`, and no partial world directory is
/// left on disk).
#[derive(Debug)]
pub enum ImportKbError {
    /// `Unknown(import-unsupported:<reason>)`.
    Unsupported(String),
    World(WorldError),
    Io(std::io::Error),
}

impl std::fmt::Display for ImportKbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportKbError::Unsupported(reason) => {
                write!(f, "Unknown(import-unsupported:{reason})")
            }
            ImportKbError::World(e) => write!(f, "world error: {e}"),
            ImportKbError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for ImportKbError {}

fn unsupported(reason: impl Into<String>) -> ImportKbError {
    ImportKbError::Unsupported(reason.into())
}

/// The result of a successful import.
#[derive(Debug)]
pub struct ImportKbReport {
    pub world_dir: PathBuf,
    pub kb_head_seq: u64,
    /// `world_dir`'s HEAD after import: `kb_head_seq + 1` under the mapping
    /// documented on this module.
    pub world_head_seq: u64,
    pub provenance_path: PathBuf,
}

/// A conservative, ASCII identifier check for a `kb_inputs` key (§5 step 5:
/// "any input name not a valid `write_ident`"). `brix_canon::CanonWriter::write_ident`
/// itself never rejects a string (it only NFC-folds), so there is no existing
/// public validator to defer to; this mirrors the identifier grammar used
/// throughout `brix-syntax` source text (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_valid_world_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Encode a single named input value with the strict `snapshot_io` encoder
/// (exact, versioned), then decode it back and require byte-for-byte value
/// equality with the original (§5 step 2 and step 5's "non-round-tripping
/// snapshot" refusal). Returns `(schema, value_json)`.
fn encode_and_verify_single_value(
    name: &str,
    value: &InputValue,
    limits: &InputLimits,
) -> Result<(String, String), ImportKbError> {
    let mut singleton = BTreeMap::new();
    singleton.insert(name.to_string(), value.clone());
    let snapshot = InputSnapshot::from_values(singleton);
    let encoded = encode_input_snapshot_v2(&snapshot);

    let shard = decode_input_shard(encoded.as_bytes(), limits).map_err(|e| {
        unsupported(format!(
            "non-round-tripping snapshot for input '{name}': re-decode failed: {e}"
        ))
    })?;
    let decoded_snapshot = canonicalize_input_shards(vec![shard], limits).map_err(|e| {
        unsupported(format!(
            "non-round-tripping snapshot for input '{name}': canonicalize failed: {e}"
        ))
    })?;
    let roundtripped = decoded_snapshot.get(name).ok_or_else(|| {
        unsupported(format!(
            "non-round-tripping snapshot for input '{name}': value missing after re-decode"
        ))
    })?;
    if roundtripped != value {
        return Err(unsupported(format!(
            "non-round-tripping snapshot for input '{name}': re-decoded value does not match the original"
        )));
    }

    let schema_doc: serde_json::Value = serde_json::from_str(&encoded).map_err(|e| {
        unsupported(format!(
            "internal: snapshot_io encoder produced non-JSON output for '{name}': {e}"
        ))
    })?;
    let schema = schema_doc
        .get("schema")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string();
    Ok((schema, encoded))
}

fn kb_input_tuple(schema: &str, value_json: &str) -> WorldTuple {
    let mut rec = TupleRecord::new();
    rec.set_str("schema", schema);
    rec.set_str("value_json", value_json);
    rec.to_tuple()
}

/// Read and canonicalize the stored KB input snapshot for `snapshot_digest`,
/// using only `brix-lower::input`'s public decode entry points (mirrors
/// `brix_kb::ops`'s own private `read_stored_snapshot`, which is not `pub`).
fn read_kb_snapshot(
    kb_dir: &Path,
    snapshot_digest: Digest,
    limits: &InputLimits,
) -> Result<InputSnapshot, ImportKbError> {
    let path = kb_paths::snapshot_file(kb_dir, snapshot_digest);
    let shard = decode_input_shard_from_file(&path, limits)
        .map_err(|e| unsupported(format!("failed to read stored KB snapshot: {e}")))?;
    canonicalize_input_shards(vec![shard], limits)
        .map_err(|e| unsupported(format!("failed to canonicalize stored KB snapshot: {e}")))
}

/// A placeholder program digest for a storage-only world
/// (`program_required = false`, so this digest is never checked against a
/// source closure — mirrors the convention in
/// `crates/brix-kb/tests/world_program_integrity.rs`'s
/// `storage_only_world_remains_valid_without_source_closure`).
fn storage_only_program_digest(kb_dir: &Path) -> Digest {
    Digest::of(
        Domain::Value,
        format!("brix.world.import-kb@1:{}", kb_dir.display()).as_bytes(),
    )
}

/// A Unix-epoch-seconds timestamp string. Not a fabricated calendar date:
/// `WorldManifest::created_at` is an uninterpreted free string field (no
/// format is validated by `WorldManifest::from_json`), and this importer has
/// no independent source of wall-clock time for the KB directory it reads.
fn import_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Import KB v1 directory `kb_dir` into a brand-new world directory
/// `world_dir` (§5).
pub fn import_kb(kb_dir: &Path, world_dir: &Path) -> Result<ImportKbReport, ImportKbError> {
    let mut owns_destination = false;
    match import_kb_inner(kb_dir, world_dir, &mut owns_destination) {
        Ok(report) => Ok(report),
        Err(e) => {
            // §5: "no partial directory left". Best-effort: if nothing was
            // ever created (refusal before `WorldSession::create`), this is a
            // no-op; `remove_dir_all` on a nonexistent path is not an error
            // worth propagating over the original one.
            if owns_destination {
                let _ = std::fs::remove_dir_all(world_dir);
            }
            Err(e)
        }
    }
}

fn import_kb_inner(
    kb_dir: &Path,
    world_dir: &Path,
    owns_destination: &mut bool,
) -> Result<ImportKbReport, ImportKbError> {
    if world_dir.exists() {
        return Err(unsupported(format!(
            "world directory '{}' already exists",
            world_dir.display()
        )));
    }

    // §5 step 1: `brix_kb::ops::verify` first; any `Unknown` refuses the import.
    ops::verify(kb_dir, &[])
        .map_err(|e| unsupported(format!("kb v1 verify failed: {}", e.diagnostic())))?;

    let manifest = ops::read_manifest(kb_dir)
        .map_err(|e| unsupported(format!("failed to read kb.json: {}", e.diagnostic())))?;
    if manifest.profile != SUPPORTED_KB_PROFILE {
        return Err(unsupported(format!(
            "unsupported KB profile '{}': only '{SUPPORTED_KB_PROFILE}' is importable",
            manifest.profile
        )));
    }

    let head = ops::read_head(kb_dir)
        .map_err(|e| unsupported(format!("failed to read HEAD: {}", e.diagnostic())))?;

    // Walk the full chain read-only first: `Change::Program` anywhere,
    // chain breaks, and a HEAD digest mismatch all refuse before any world
    // directory is created.
    let mut records = Vec::with_capacity(head.seq as usize);
    let mut expected_parent: Option<Digest> = None;
    for seq in 1..=head.seq {
        let record = ops::read_revision(kb_dir, seq).map_err(|e| {
            unsupported(format!(
                "broken KB chain at revision {seq}: {}",
                e.diagnostic()
            ))
        })?;
        if record.seq != seq {
            return Err(unsupported(format!(
                "KB revision file for seq {seq} records seq {} instead",
                record.seq
            )));
        }
        if record.parent != expected_parent {
            return Err(unsupported(format!(
                "broken KB chain: revision {seq}'s parent does not match the previous revision's digest"
            )));
        }
        if matches!(record.change, Change::Program { .. }) {
            return Err(unsupported(format!(
                "revision {seq} changes the program (Change::Program): a finite-decision \
                 program is not a relational program, and re-targeting it would only approximate one"
            )));
        }
        expected_parent = Some(record.digest());
        records.push(record);
    }
    if expected_parent != Some(head.digest) {
        return Err(unsupported(
            "KB HEAD digest does not match the recomputed revision chain",
        ));
    }

    // §5 step 2: storage-only world, one relation `kb_inputs`.
    let world_manifest = WorldManifest::new(
        format!("kb-import:{}", kb_dir.display()),
        import_timestamp(),
        storage_only_program_digest(kb_dir),
        vec![RelationDecl::new(
            KB_INPUTS_RELATION,
            vec!["name".to_string()],
            vec!["schema".to_string(), "value_json".to_string()],
            Vec::new(),
        )],
    );
    debug_assert!(!world_manifest.program_required);
    // Atomically claim the destination so cleanup cannot remove a directory
    // created by another process after the initial existence check.
    if let Some(parent) = world_dir.parent() {
        std::fs::create_dir_all(parent).map_err(ImportKbError::Io)?;
    }
    std::fs::create_dir(world_dir).map_err(ImportKbError::Io)?;
    *owns_destination = true;
    // `WorldSession::create` may leave partial files if a later initialization
    // step fails; the outer guard now owns this newly-created directory.
    let mut session =
        WorldSession::create(world_dir, world_manifest).map_err(ImportKbError::World)?;

    let value_limits = InputLimits::default();
    let mut current_values: BTreeMap<String, InputValue> = BTreeMap::new();
    let mut provenance_entries = Vec::with_capacity(records.len());

    for record in &records {
        let snapshot = read_kb_snapshot(kb_dir, record.snapshot_id.digest(), &value_limits)?;
        let new_values = snapshot.values().clone();

        let mut batch_ops = Vec::new();
        for (name, value) in &new_values {
            if current_values.get(name) == Some(value) {
                continue;
            }
            if !is_valid_world_ident(name) {
                return Err(unsupported(format!(
                    "input name '{name}' is not a valid identifier"
                )));
            }
            let (schema, value_json) = encode_and_verify_single_value(name, value, &value_limits)?;
            batch_ops.push(WorldBatchOp::Upsert {
                relation: KB_INPUTS_RELATION.to_string(),
                key: WorldKey::from_str(name),
                tuple: kb_input_tuple(&schema, &value_json),
            });
        }
        for name in current_values.keys() {
            if !new_values.contains_key(name) {
                batch_ops.push(WorldBatchOp::Remove {
                    relation: KB_INPUTS_RELATION.to_string(),
                    key: WorldKey::from_str(name),
                });
            }
        }

        // §5 step 3.
        let idempotency_key = format!("kb-import:{}", record.digest().to_hex());
        let base = session.current_revision();
        let batch = WorldBatch::new(base, idempotency_key, batch_ops);
        let receipt = session.apply_batch(batch).map_err(ImportKbError::World)?;

        // §5 step 4: the KB result is carried as a claim, never a world
        // decision; `decision_root` is never touched by this importer (the
        // world has no `decide` blocks at all).
        provenance_entries.push(serde_json::json!({
            "kb_seq": record.seq,
            "kb_revision_digest": record.digest().to_hex(),
            "program_id": record.program_id.digest().to_hex(),
            "snapshot_id": record.snapshot_id.digest().to_hex(),
            "context_id": record.result.context_id.map(|c| c.digest().to_hex()),
            "result": {
                "status": record.result.status.as_str(),
                "candidate": record.result.candidate,
                "decision_digest": record.result.decision_digest.map(|d| d.to_hex()),
            },
            "world_seq": receipt.revision_seq,
            "world_revision_digest": receipt.revision_digest.to_hex(),
        }));

        current_values = new_values;
    }

    let provenance_doc = serde_json::json!({
        "schema": IMPORT_PROVENANCE_SCHEMA,
        "source_dir": kb_dir.display().to_string(),
        "kb_profile": manifest.profile,
        "kb_head_seq": head.seq,
        "kb_head_digest": head.digest.to_hex(),
        "revisions": provenance_entries,
    });
    let provenance_bytes = serde_json::to_vec_pretty(&provenance_doc)
        .map_err(|e| unsupported(format!("provenance JSON encoding failed: {e}")))?;
    let provenance_path = world_dir.join("import-provenance.json");
    std::fs::write(&provenance_path, provenance_bytes).map_err(ImportKbError::Io)?;

    Ok(ImportKbReport {
        world_dir: world_dir.to_path_buf(),
        kb_head_seq: head.seq,
        world_head_seq: session.current_revision(),
        provenance_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops as kb_ops;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "brix_world_import_kb_{name}_{}_{}",
            std::process::id(),
            name.len()
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    const PROGRAM_SOURCE: &str = "config Decision = Accept\n\n\
         input stock: Int\n\
         input eligible: Bool\n\n\
         propose accept() priority 1 when true = Accept\n\
         commit result from (accept)\n";

    /// A minimal finite-decision program this test suite can `kb init` and
    /// `kb assert` against, independent of the persistent-world grammar used
    /// elsewhere in this crate's world tests.
    fn write_program(dir: &Path) -> PathBuf {
        let path = dir.join("program.brix");
        std::fs::write(&path, PROGRAM_SOURCE).unwrap();
        path
    }

    fn write_input(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Remove `kb_root` and the sibling scratch directories `init_kb`/the
    /// program-change test created alongside it.
    fn cleanup_kb(kb_root: &Path) {
        std::fs::remove_dir_all(kb_root).ok();
        std::fs::remove_dir_all(kb_root.with_extension("src")).ok();
        std::fs::remove_dir_all(kb_root.with_extension("inputs")).ok();
        std::fs::remove_dir_all(kb_root.with_extension("src2")).ok();
    }

    fn init_kb(kb_root: &Path) {
        let src_dir = kb_root.with_extension("src");
        let inputs_dir = kb_root.with_extension("inputs");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&inputs_dir).unwrap();
        let program_path = write_program(&src_dir);
        let input_path = write_input(
            &inputs_dir,
            "initial.json",
            r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"5"},"eligible":{"type":"bool","value":true}}}"#,
        );
        std::fs::create_dir_all(kb_root).unwrap();
        kb_ops::init(kb_root, &program_path, &[input_path], &[]).expect("kb init");
    }

    fn append_stock_revision(kb_root: &Path, stock: &str) {
        let update_path = kb_root.with_extension("update.json");
        std::fs::write(
            &update_path,
            format!(
                r#"{{"schema":"brix.input@1","values":{{"stock":{{"type":"int","value":"{stock}"}}}}}}"#
            ),
        )
        .unwrap();
        kb_ops::assert_inputs(kb_root, std::slice::from_ref(&update_path), &[]).expect("kb assert");
        std::fs::remove_file(update_path).unwrap();
    }

    fn hash_dir(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    let rel = path.strip_prefix(base).unwrap().to_path_buf();
                    let bytes = std::fs::read(&path).unwrap();
                    out.push((rel, bytes));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    #[test]
    fn kb_import_preserves_old_directory_bytes() {
        let kb_dir = test_dir("preserve_src");
        let world_dir = test_dir("preserve_world");
        init_kb(&kb_dir);

        let before = hash_dir(&kb_dir);
        let report = import_kb(&kb_dir, &world_dir).expect("import");
        let after = hash_dir(&kb_dir);

        assert_eq!(
            before, after,
            "KB source directory must be byte-identical after import"
        );
        assert_eq!(report.kb_head_seq, 1);
        assert_eq!(report.world_head_seq, 1);

        cleanup_kb(&kb_dir);
        std::fs::remove_dir_all(&world_dir).ok();
    }

    #[test]
    fn kb_import_round_trips_every_snapshot() {
        let kb_dir = test_dir("roundtrip_src");
        let world_dir = test_dir("roundtrip_world");
        init_kb(&kb_dir);
        append_stock_revision(&kb_dir, "8");

        let report = import_kb(&kb_dir, &world_dir).expect("import");
        let session = WorldSession::open(&world_dir).expect("open imported world");

        let rev1 = session.pin_revision(1).expect("revision 1");
        let rev1_stock = rev1
            .get(KB_INPUTS_RELATION, &WorldKey::from_str("stock"))
            .expect("get")
            .expect("stock at revision 1");
        let rev1_rec = TupleRecord::from_tuple(&rev1_stock).expect("decode revision 1 tuple");
        assert!(rev1_rec.get_str("value_json").unwrap().contains("\"5\""));

        let rev2 = session.pin_revision(2).expect("revision 2");
        let rev2_stock = rev2
            .get(KB_INPUTS_RELATION, &WorldKey::from_str("stock"))
            .expect("get")
            .expect("stock at revision 2");
        let rev2_rec = TupleRecord::from_tuple(&rev2_stock).expect("decode revision 2 tuple");
        assert!(rev2_rec.get_str("value_json").unwrap().contains("\"8\""));

        let stock_tuple = session
            .get(KB_INPUTS_RELATION, &WorldKey::from_str("stock"))
            .expect("get")
            .expect("stock present");
        let rec = TupleRecord::from_tuple(&stock_tuple).expect("decode tuple");
        let value_json = rec.get_str("value_json").expect("value_json field");
        assert!(value_json.contains("\"8\""), "value_json: {value_json}");

        let eligible_tuple = session
            .get(KB_INPUTS_RELATION, &WorldKey::from_str("eligible"))
            .expect("get")
            .expect("eligible present");
        let rec2 = TupleRecord::from_tuple(&eligible_tuple).expect("decode tuple");
        assert!(rec2.get_str("value_json").unwrap().contains("true"));

        let _ = session.close();
        cleanup_kb(&kb_dir);
        std::fs::remove_dir_all(&world_dir).ok();
        assert_eq!(report.kb_head_seq, 2);
        assert_eq!(report.world_head_seq, 2);
    }

    #[test]
    fn kb_import_refuses_program_change() {
        let kb_dir = test_dir("program_change_src");
        let world_dir = test_dir("program_change_world");
        init_kb(&kb_dir);

        let changed_program = kb_dir.with_extension("src2");
        std::fs::create_dir_all(&changed_program).unwrap();
        let program_path = changed_program.join("program2.brix");
        std::fs::write(
            &program_path,
            "config Decision = Accept\n\n\
             input stock: Int\n\
             input eligible: Bool\n\
             input extra: Int\n\n\
             propose accept() priority 1 when true = Accept\n\
             commit result from (accept)\n",
        )
        .unwrap();
        kb_ops::set_program(&kb_dir, &program_path, &[]).expect("kb program");

        let err = import_kb(&kb_dir, &world_dir).unwrap_err();
        match err {
            ImportKbError::Unsupported(reason) => {
                assert!(reason.contains("Change::Program"), "reason: {reason}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        assert!(!world_dir.exists(), "no partial directory must be left");

        cleanup_kb(&kb_dir);
    }

    #[test]
    fn old_kb_commands_still_read_imported_source() {
        let kb_dir = test_dir("still_readable_src");
        let world_dir = test_dir("still_readable_world");
        init_kb(&kb_dir);

        import_kb(&kb_dir, &world_dir).expect("import");

        // The import is read-only: every original `brix kb` read operation
        // must still work against `kb_dir` exactly as before.
        let outcomes = kb_ops::log(&kb_dir, &[]).expect("kb log still works");
        assert_eq!(outcomes.len(), 1);
        kb_ops::verify(&kb_dir, &[]).expect("kb verify still works");

        cleanup_kb(&kb_dir);
        std::fs::remove_dir_all(&world_dir).ok();
    }

    #[test]
    fn kb_import_refuses_existing_world_dir() {
        let kb_dir = test_dir("existing_world_src");
        let world_dir = test_dir("existing_world_world");
        init_kb(&kb_dir);
        std::fs::create_dir_all(&world_dir).unwrap();
        let sentinel = world_dir.join("keep.bin");
        std::fs::write(&sentinel, b"caller-owned bytes").unwrap();

        let err = import_kb(&kb_dir, &world_dir).unwrap_err();
        assert!(matches!(err, ImportKbError::Unsupported(_)));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"caller-owned bytes");

        cleanup_kb(&kb_dir);
        std::fs::remove_dir_all(&world_dir).ok();
    }
}
