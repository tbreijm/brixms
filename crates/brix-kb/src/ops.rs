//! The knowledge base operations: `init`, `assert`, `retract`, `program`,
//! `log`, `show`, `audit`, `verify` (ADR-0041). `diff` lives in `diff.rs`
//! (it needs two loaded revisions plus the dependency graph).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_lower::finite_decision::{
    FiniteDecisionPlan, FiniteDecisionRun, FiniteDecisionRuntime, FiniteDecisionStop,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard_from_file, load_input_snapshot_from_paths,
    InputLimits, InputSnapshot,
};

use crate::error::KbError;
use crate::manifest::{Head, Manifest};
use crate::paths;
use crate::pipeline::{self, ReplayResult};
use crate::revision::{
    digest_decision_value, digest_dispositions, digest_facts, Change, RevisionRecord,
    RevisionResult, Status,
};
use crate::snapshot_io::encode_input_snapshot_v2;

/// The result of a write operation (or `show`): the revision it produced (or
/// named), the plan it was decided against, and the fresh replay result.
pub struct OpOutcome {
    pub record: RevisionRecord,
    pub plan: FiniteDecisionPlan,
    pub replay: ReplayResult,
}

/// The result of `brix kb audit`.
#[derive(Debug)]
pub struct AuditOutcome {
    pub record: RevisionRecord,
    pub bundle_id_hex: String,
    pub final_chain_hex: String,
    pub receipts_count: usize,
}

/// The result of `brix kb verify`.
#[derive(Debug)]
pub struct VerifyReport {
    pub revisions_checked: u64,
}

// ---------------------------------------------------------------------------
// Locking
// ---------------------------------------------------------------------------

struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    fn acquire(root: &Path) -> Result<Self, KbError> {
        let path = paths::lock(root);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "pid={}", std::process::id());
                Ok(Self { path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(KbError::usage(
                "kb-locked",
                format!(
                    "knowledge base '{}' is locked by another writer (remove '{}' if you are certain no other process is using it)",
                    root.display(),
                    path.display()
                ),
            )),
            Err(e) => Err(KbError::io(
                "kb-lock-io-error",
                format!("cannot create lock file '{}': {e}", path.display()),
            )),
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Low-level file I/O helpers
// ---------------------------------------------------------------------------

fn io_err(path: &Path, e: std::io::Error) -> KbError {
    if e.kind() == std::io::ErrorKind::NotFound {
        KbError::usage(
            "kb-not-found",
            format!("'{}' does not exist", path.display()),
        )
    } else {
        KbError::io(
            "kb-io-error",
            format!("cannot access '{}': {e}", path.display()),
        )
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), KbError> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                KbError::io(
                    "kb-write-conflict",
                    format!(
                        "'{}' already exists (revisions are write-once)",
                        path.display()
                    ),
                )
            } else {
                KbError::io(
                    "kb-write-error",
                    format!("cannot create '{}': {e}", path.display()),
                )
            }
        })?;
    f.write_all(bytes).and_then(|_| f.sync_all()).map_err(|e| {
        KbError::io(
            "kb-write-error",
            format!("cannot write '{}': {e}", path.display()),
        )
    })
}

/// Write `bytes` to `path` if absent; a no-op if a file already exists there
/// — correct for a content-addressed store, since the path names the exact
/// bytes it holds.
fn store_if_absent(path: &Path, bytes: &[u8]) -> Result<(), KbError> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => f.write_all(bytes).and_then(|_| f.sync_all()).map_err(|e| {
            KbError::io(
                "kb-write-error",
                format!("cannot write '{}': {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(KbError::io(
            "kb-write-error",
            format!("cannot create '{}': {e}", path.display()),
        )),
    }
}

fn write_head_atomic(root: &Path, head: &Head) -> Result<(), KbError> {
    let tmp = root.join(format!(".head.tmp.{}", std::process::id()));
    std::fs::write(&tmp, head.to_json_string()).map_err(|e| {
        KbError::io(
            "kb-write-error",
            format!("cannot write temporary HEAD: {e}"),
        )
    })?;
    std::fs::rename(&tmp, paths::head(root)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        KbError::io("kb-write-error", format!("cannot update HEAD: {e}"))
    })
}

fn write_revision_and_head(root: &Path, record: &RevisionRecord) -> Result<(), KbError> {
    write_new(
        &paths::revision_file(root, record.seq),
        record.to_json_string().as_bytes(),
    )?;
    write_head_atomic(
        root,
        &Head {
            seq: record.seq,
            digest: record.digest(),
        },
    )
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

pub fn read_manifest(root: &Path) -> Result<Manifest, KbError> {
    let path = paths::kb_json(root);
    let bytes = std::fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            KbError::usage(
                "kb-not-found",
                format!(
                    "'{}' is not a knowledge base (no kb.json found)",
                    root.display()
                ),
            )
        } else {
            io_err(&path, e)
        }
    })?;
    Manifest::from_bytes(&bytes)
}

pub fn read_head(root: &Path) -> Result<Head, KbError> {
    let path = paths::head(root);
    let bytes = std::fs::read(&path).map_err(|e| io_err(&path, e))?;
    Head::from_bytes(&bytes)
}

pub fn read_revision(root: &Path, seq: u64) -> Result<RevisionRecord, KbError> {
    let path = paths::revision_file(root, seq);
    let bytes = std::fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            KbError::usage("kb-revision-not-found", format!("revision {seq} not found"))
        } else {
            io_err(&path, e)
        }
    })?;
    RevisionRecord::from_bytes(&bytes)
}

fn read_stored_program(root: &Path, id: Digest) -> Result<String, KbError> {
    let path = paths::program_file(root, id);
    std::fs::read_to_string(&path).map_err(|e| io_err(&path, e))
}

fn read_stored_snapshot(root: &Path, id: Digest) -> Result<InputSnapshot, KbError> {
    let path = paths::snapshot_file(root, id);
    let limits = InputLimits::default();
    let shard = decode_input_shard_from_file(&path, &limits)
        .map_err(|e| KbError::from(brix_lower::input::InputError::from(e)))?;
    canonicalize_input_shards(vec![shard], &limits).map_err(KbError::from)
}

/// Read the stored program and snapshot for a revision record, load the plan,
/// and replay it. Used by every read-only op (`log`, `show`, `diff`, `audit`).
pub fn load_and_replay(
    root: &Path,
    record: &RevisionRecord,
    package_paths: &[PathBuf],
) -> Result<(FiniteDecisionPlan, InputSnapshot, ReplayResult), KbError> {
    let source = read_stored_program(root, record.program_id.digest())?;
    let plan = pipeline::load_plan(&source, package_paths)?;
    let snapshot = read_stored_snapshot(root, record.snapshot_id.digest())?;
    let replay = pipeline::replay(&plan, &snapshot)?;
    Ok((plan, snapshot, replay))
}

fn compute_result(replay: &ReplayResult) -> Result<RevisionResult, KbError> {
    match replay {
        ReplayResult::MissingInputs { missing } => Ok(RevisionResult {
            status: Status::MissingInputs,
            candidate: None,
            decision_digest: None,
            context_id: None,
            facts_digest: None,
            dispositions_digest: None,
            diagnostics: missing
                .iter()
                .map(|n| format!("missing required input '{n}'"))
                .collect(),
        }),
        ReplayResult::Ran { run, .. } => {
            let facts_digest = Some(digest_facts(&run.facts)?);
            let dispositions_digest = Some(digest_dispositions(&run.dispositions));
            let (status, candidate, decision_digest, diagnostics) = match &run.stop {
                FiniteDecisionStop::Selected(sel) => (
                    Status::Selected,
                    Some(sel.candidate.clone()),
                    Some(digest_decision_value(&sel.value)?),
                    Vec::new(),
                ),
                FiniteDecisionStop::Quiescent { .. } => (Status::Quiescent, None, None, Vec::new()),
                FiniteDecisionStop::Unknown(reason) => {
                    (Status::Unknown, None, None, vec![reason.to_string()])
                }
            };
            Ok(RevisionResult {
                status,
                candidate,
                decision_digest,
                context_id: Some(run.context),
                facts_digest,
                dispositions_digest,
                diagnostics,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

pub fn init(
    root: &Path,
    program_path: &Path,
    input_paths: &[PathBuf],
    package_paths: &[PathBuf],
) -> Result<OpOutcome, KbError> {
    std::fs::create_dir_all(root).map_err(|e| {
        KbError::io(
            "kb-io-error",
            format!("cannot create '{}': {e}", root.display()),
        )
    })?;
    if paths::kb_json(root).exists() {
        return Err(KbError::usage(
            "kb-already-initialized",
            format!("'{}' is already a knowledge base", root.display()),
        ));
    }

    let _lock = LockGuard::acquire(root)?;
    for dir in [
        paths::programs_dir(root),
        paths::snapshots_dir(root),
        paths::revisions_dir(root),
    ] {
        std::fs::create_dir_all(&dir).map_err(|e| {
            KbError::io(
                "kb-io-error",
                format!("cannot create '{}': {e}", dir.display()),
            )
        })?;
    }

    let source = pipeline::read_program_source(program_path)?;
    let plan = pipeline::load_plan(&source, package_paths)?;
    let snapshot = load_input_snapshot_from_paths(input_paths, &InputLimits::default())
        .map_err(KbError::from)?;
    snapshot
        .validate_against_declarations(&plan)
        .map_err(KbError::from)?;

    let program_id = pipeline::program_id(&plan);
    store_if_absent(
        &paths::program_file(root, program_id.digest()),
        source.as_bytes(),
    )?;
    let snapshot_id = snapshot.id();
    store_if_absent(
        &paths::snapshot_file(root, snapshot_id.digest()),
        encode_input_snapshot_v2(&snapshot).as_bytes(),
    )?;

    let replay = pipeline::replay(&plan, &snapshot)?;
    let result = compute_result(&replay)?;

    let record = RevisionRecord {
        seq: 1,
        parent: None,
        program_id,
        program_path: paths::program_rel_path(program_id.digest()),
        snapshot_id,
        snapshot_path: paths::snapshot_rel_path(snapshot_id.digest()),
        change: Change::Init,
        result,
    };

    write_new(
        &paths::kb_json(root),
        Manifest {
            profile: brix_lower::finite_decision::FINITE_DECISION_PROFILE.to_string(),
        }
        .to_json_string()
        .as_bytes(),
    )
    .map_err(|e| {
        if e.code == "kb-write-conflict" {
            KbError::usage(
                "kb-already-initialized",
                format!("'{}' is already a knowledge base", root.display()),
            )
        } else {
            e
        }
    })?;
    write_revision_and_head(root, &record)?;

    Ok(OpOutcome {
        record,
        plan,
        replay,
    })
}

// ---------------------------------------------------------------------------
// assert
// ---------------------------------------------------------------------------

pub fn assert_inputs(
    root: &Path,
    input_paths: &[PathBuf],
    package_paths: &[PathBuf],
) -> Result<OpOutcome, KbError> {
    if input_paths.is_empty() {
        return Err(KbError::usage(
            "kb-assert-empty",
            "'brix kb assert' requires at least one --input file",
        ));
    }
    let _lock = LockGuard::acquire(root)?;
    read_manifest(root)?;
    let head = read_head(root)?;
    let parent = read_revision(root, head.seq)?;

    let source = read_stored_program(root, parent.program_id.digest())?;
    let plan = pipeline::load_plan(&source, package_paths)?;
    let base_snapshot = read_stored_snapshot(root, parent.snapshot_id.digest())?;

    let asserted = load_input_snapshot_from_paths(input_paths, &InputLimits::default())
        .map_err(KbError::from)?;
    if asserted.is_empty() {
        return Err(KbError::usage(
            "kb-assert-empty",
            "'brix kb assert' input file(s) contained no values",
        ));
    }
    let names: Vec<String> = asserted.values().keys().cloned().collect();

    let mut merged: BTreeMap<String, brix_lower::input::InputValue> =
        base_snapshot.values().clone();
    for (k, v) in asserted.values() {
        merged.insert(k.clone(), v.clone());
    }
    let new_snapshot = InputSnapshot::from_values(merged);
    new_snapshot
        .validate_against_declarations(&plan)
        .map_err(KbError::from)?;

    let snapshot_id = new_snapshot.id();
    store_if_absent(
        &paths::snapshot_file(root, snapshot_id.digest()),
        encode_input_snapshot_v2(&new_snapshot).as_bytes(),
    )?;

    let replay = pipeline::replay(&plan, &new_snapshot)?;
    let result = compute_result(&replay)?;
    let record = RevisionRecord {
        seq: head.seq + 1,
        parent: Some(head.digest),
        program_id: parent.program_id,
        program_path: parent.program_path.clone(),
        snapshot_id,
        snapshot_path: paths::snapshot_rel_path(snapshot_id.digest()),
        change: Change::Assert { names },
        result,
    };
    write_revision_and_head(root, &record)?;
    Ok(OpOutcome {
        record,
        plan,
        replay,
    })
}

// ---------------------------------------------------------------------------
// retract
// ---------------------------------------------------------------------------

pub fn retract_inputs(
    root: &Path,
    names: &[String],
    package_paths: &[PathBuf],
) -> Result<OpOutcome, KbError> {
    if names.is_empty() {
        return Err(KbError::usage(
            "kb-retract-empty",
            "'brix kb retract' requires at least one input name",
        ));
    }
    let _lock = LockGuard::acquire(root)?;
    read_manifest(root)?;
    let head = read_head(root)?;
    let parent = read_revision(root, head.seq)?;

    let source = read_stored_program(root, parent.program_id.digest())?;
    let plan = pipeline::load_plan(&source, package_paths)?;
    let base_snapshot = read_stored_snapshot(root, parent.snapshot_id.digest())?;

    let mut values = base_snapshot.values().clone();
    let mut not_set = Vec::new();
    for name in names {
        if values.remove(name).is_none() {
            not_set.push(name.clone());
        }
    }
    if !not_set.is_empty() {
        return Err(KbError::usage(
            "kb-retract-not-set",
            format!(
                "cannot retract input(s) not currently set: {}",
                not_set.join(", ")
            ),
        ));
    }

    let new_snapshot = InputSnapshot::from_values(values);
    let snapshot_id = new_snapshot.id();
    store_if_absent(
        &paths::snapshot_file(root, snapshot_id.digest()),
        encode_input_snapshot_v2(&new_snapshot).as_bytes(),
    )?;

    let replay = pipeline::replay(&plan, &new_snapshot)?;
    let result = compute_result(&replay)?;
    let record = RevisionRecord {
        seq: head.seq + 1,
        parent: Some(head.digest),
        program_id: parent.program_id,
        program_path: parent.program_path.clone(),
        snapshot_id,
        snapshot_path: paths::snapshot_rel_path(snapshot_id.digest()),
        change: Change::Retract {
            names: names.to_vec(),
        },
        result,
    };
    write_revision_and_head(root, &record)?;
    Ok(OpOutcome {
        record,
        plan,
        replay,
    })
}

// ---------------------------------------------------------------------------
// program
// ---------------------------------------------------------------------------

pub fn set_program(
    root: &Path,
    new_program_path: &Path,
    package_paths: &[PathBuf],
) -> Result<OpOutcome, KbError> {
    let _lock = LockGuard::acquire(root)?;
    read_manifest(root)?;
    let head = read_head(root)?;
    let parent = read_revision(root, head.seq)?;
    let base_snapshot = read_stored_snapshot(root, parent.snapshot_id.digest())?;

    let new_source = pipeline::read_program_source(new_program_path)?;
    let new_plan = pipeline::load_plan(&new_source, package_paths)?;
    let new_program_id = pipeline::program_id(&new_plan);

    let mut kept = BTreeMap::new();
    let mut dropped = Vec::new();
    for (name, value) in base_snapshot.values() {
        match new_plan.find_input(name) {
            Some(decl) if decl.ty == value.value_type() => {
                kept.insert(name.clone(), value.clone());
            }
            _ => dropped.push(name.clone()),
        }
    }
    let new_snapshot = InputSnapshot::from_values(kept);
    let snapshot_id = new_snapshot.id();

    store_if_absent(
        &paths::program_file(root, new_program_id.digest()),
        new_source.as_bytes(),
    )?;
    store_if_absent(
        &paths::snapshot_file(root, snapshot_id.digest()),
        encode_input_snapshot_v2(&new_snapshot).as_bytes(),
    )?;

    let replay = pipeline::replay(&new_plan, &new_snapshot)?;
    let result = compute_result(&replay)?;
    let record = RevisionRecord {
        seq: head.seq + 1,
        parent: Some(head.digest),
        program_id: new_program_id,
        program_path: paths::program_rel_path(new_program_id.digest()),
        snapshot_id,
        snapshot_path: paths::snapshot_rel_path(snapshot_id.digest()),
        change: Change::Program {
            previous_program_id: parent.program_id,
            dropped_inputs: dropped,
        },
        result,
    };
    write_revision_and_head(root, &record)?;
    Ok(OpOutcome {
        record,
        plan: new_plan,
        replay,
    })
}

// ---------------------------------------------------------------------------
// log / show
// ---------------------------------------------------------------------------

/// Every revision, replayed fresh, oldest first.
pub fn log(root: &Path, package_paths: &[PathBuf]) -> Result<Vec<OpOutcome>, KbError> {
    read_manifest(root)?;
    let head = read_head(root)?;
    let mut out = Vec::with_capacity(head.seq as usize);
    for seq in 1..=head.seq {
        let record = read_revision(root, seq)?;
        let (plan, _snapshot, replay) = load_and_replay(root, &record, package_paths)?;
        out.push(OpOutcome {
            record,
            plan,
            replay,
        });
    }
    Ok(out)
}

/// One revision (defaulting to HEAD), replayed fresh.
pub fn show(
    root: &Path,
    rev: Option<u64>,
    package_paths: &[PathBuf],
) -> Result<OpOutcome, KbError> {
    read_manifest(root)?;
    let seq = match rev {
        Some(s) => s,
        None => read_head(root)?.seq,
    };
    let record = read_revision(root, seq)?;
    let (plan, _snapshot, replay) = load_and_replay(root, &record, package_paths)?;
    Ok(OpOutcome {
        record,
        plan,
        replay,
    })
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------

pub fn audit_revision(
    root: &Path,
    seq: u64,
    bundle_out: &Path,
    force: bool,
    package_paths: &[PathBuf],
) -> Result<AuditOutcome, KbError> {
    read_manifest(root)?;
    let record = read_revision(root, seq)?;
    let (_plan, _snapshot, replay) = load_and_replay(root, &record, package_paths)?;
    let (runtime, run): (Box<FiniteDecisionRuntime>, Box<FiniteDecisionRun>) = match replay {
        ReplayResult::Ran { runtime, run } => (runtime, run),
        ReplayResult::MissingInputs { missing } => {
            return Err(KbError::rejected(
                "kb-audit-missing-inputs",
                format!(
                "revision {seq} has an incomplete input contract (missing: {}); nothing to audit",
                missing.join(", ")
            ),
            ))
        }
    };
    if run.is_unknown() {
        return Err(KbError::rejected(
            "kb-audit-unknown",
            format!("revision {seq}'s deliberation is Unknown; nothing to audit"),
        ));
    }

    let bundle =
        brix_lower::audit_bundle::produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
            .map_err(|e| {
                KbError::unknown(
                    "kb-audit-bundle-error",
                    format!("bundle production error: {e}"),
                )
            })?;
    let bytes = bundle
        .encode(&brix_lower::AuditDecodeLimits::strict())
        .map_err(|e| KbError::unknown("kb-audit-bundle-encode-error", format!("{e:?}")))?;

    write_bundle_atomic(bundle_out, &bytes, force)?;

    Ok(AuditOutcome {
        record,
        bundle_id_hex: bundle.id().digest().to_hex(),
        final_chain_hex: bundle.final_chain_digest.to_hex(),
        receipts_count: bundle.entries.len(),
    })
}

fn write_bundle_atomic(bundle_out: &Path, bytes: &[u8], force: bool) -> Result<(), KbError> {
    if bundle_out.exists() && !force {
        return Err(KbError::io(
            "kb-audit-bundle-exists",
            format!(
                "destination file '{}' already exists (use --force to overwrite)",
                bundle_out.display()
            ),
        ));
    }
    let parent = bundle_out.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        if !parent.exists() {
            return Err(KbError::io(
                "kb-audit-bundle-io-error",
                format!("parent directory '{}' does not exist", parent.display()),
            ));
        }
    }
    let dir = parent.unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".tmp_kb_bundle_{}_{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&tmp, bytes).map_err(|e| {
        KbError::io(
            "kb-audit-bundle-io-error",
            format!("cannot write bundle: {e}"),
        )
    })?;
    if bundle_out.exists() && !force {
        let _ = std::fs::remove_file(&tmp);
        return Err(KbError::io(
            "kb-audit-bundle-exists",
            format!(
                "destination file '{}' already exists (use --force to overwrite)",
                bundle_out.display()
            ),
        ));
    }
    std::fs::rename(&tmp, bundle_out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        KbError::io(
            "kb-audit-bundle-io-error",
            format!(
                "cannot write bundle file to '{}': {e}",
                bundle_out.display()
            ),
        )
    })
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

pub fn verify(root: &Path, package_paths: &[PathBuf]) -> Result<VerifyReport, KbError> {
    read_manifest(root)?;
    let head = read_head(root)?;
    let mut expected_parent: Option<Digest> = None;

    for seq in 1..=head.seq {
        let record = read_revision(root, seq)?;
        if record.seq != seq {
            return Err(KbError::unknown(
                "kb-verify-seq-mismatch",
                format!("revision file for seq {seq} records seq {}", record.seq),
            ));
        }
        if record.parent != expected_parent {
            return Err(KbError::unknown(
                "kb-verify-chain-broken",
                format!(
                    "revision {seq}'s parent digest does not match revision {}'s digest (chain broken, or a revision was edited)",
                    seq.saturating_sub(1)
                ),
            ));
        }

        let source = read_stored_program(root, record.program_id.digest())?;
        let plan = pipeline::load_plan(&source, package_paths)?;
        let recomputed_program_id = pipeline::program_id(&plan);
        if recomputed_program_id != record.program_id {
            return Err(KbError::unknown(
                "kb-verify-program-id-mismatch",
                format!(
                    "revision {seq}: stored program source no longer hashes to the recorded program id (the program file was edited)"
                ),
            ));
        }

        let snapshot = read_stored_snapshot(root, record.snapshot_id.digest())?;
        if snapshot.id() != record.snapshot_id {
            return Err(KbError::unknown(
                "kb-verify-snapshot-id-mismatch",
                format!(
                    "revision {seq}: stored snapshot no longer hashes to the recorded snapshot id (the snapshot file was edited)"
                ),
            ));
        }

        let replay = pipeline::replay(&plan, &snapshot)?;
        let recomputed_result = compute_result(&replay)?;
        if recomputed_result != record.result {
            return Err(KbError::unknown(
                "kb-verify-result-mismatch",
                format!(
                    "revision {seq}: a fresh replay does not reproduce the recorded result (the revision record was edited, or the decision is non-deterministic)"
                ),
            ));
        }

        expected_parent = Some(record.digest());
    }

    if expected_parent != Some(head.digest) {
        return Err(KbError::unknown(
            "kb-verify-head-mismatch",
            "HEAD does not point at the last revision's digest".to_string(),
        ));
    }

    Ok(VerifyReport {
        revisions_checked: head.seq,
    })
}
