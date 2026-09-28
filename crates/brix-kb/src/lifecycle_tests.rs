//! End-to-end library-level tests for the knowledge base lifecycle (ADR-0041):
//! init → assert → retract → program → log → show → diff → audit → verify,
//! plus tamper detection, lock contention, and non-erasure.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::KbError;
use crate::ops;
use crate::paths;
use crate::pipeline::{self, ReplayResult};
use crate::revision::Status;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("brix_kb_test_{tag}_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples")).join(name)
}

fn assert_ran_selected(replay: &ReplayResult, expected_candidate: &str) {
    match replay {
        ReplayResult::Ran { run, .. } => {
            let decision = run.decision.as_ref().expect("expected a selected decision");
            assert_eq!(decision.candidate, expected_candidate);
        }
        ReplayResult::MissingInputs { missing } => {
            panic!("expected a decision, got missing inputs: {missing:?}")
        }
    }
}

fn write_input_shard(dir: &Path, filename: &str, json: &str) -> PathBuf {
    let path = dir.join(filename);
    std::fs::write(&path, json).unwrap();
    path
}

/// `OpOutcome`/`AuditOutcome`'s `Ok` payload embeds a `FiniteDecisionRuntime`,
/// which does not implement `Debug`, so `Result::unwrap_err` (which requires
/// `T: Debug`) cannot be used directly on these results.
fn expect_err<T>(result: Result<T, KbError>) -> KbError {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
    }
}

// ---------------------------------------------------------------------------
// init / assert / retract / program / log / show
// ---------------------------------------------------------------------------

#[test]
fn test_init_assert_retract_program_log_show_lifecycle() {
    let tmp = TempDir::new("lifecycle");
    let kb_root = tmp.path.join("kb");

    // init: revision 1, full input contract, decision selected.
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    let outcome = ops::init(&kb_root, &program, &[input], &[]).expect("init should succeed");
    assert_eq!(outcome.record.seq, 1);
    assert_eq!(outcome.record.parent, None);
    assert_eq!(outcome.record.result.status, Status::Selected);
    assert_ran_selected(&outcome.replay, "ship");

    // A second init on the same directory is refused.
    let err = expect_err(ops::init(&kb_root, &program, &[], &[]));
    assert_eq!(err.code, "kb-already-initialized");

    // assert: correct `stock` downward so `can_ship` flips, creating revision 2.
    let corrected = write_input_shard(
        &tmp.path,
        "correction.json",
        r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"3"}}}"#,
    );
    let outcome2 = ops::assert_inputs(&kb_root, &[corrected], &[]).expect("assert should succeed");
    assert_eq!(outcome2.record.seq, 2);
    assert_eq!(outcome2.record.parent, Some(outcome.record.digest()));
    match &outcome2.replay {
        ReplayResult::Ran { run, .. } => {
            let decision = run.decision.as_ref().unwrap();
            assert_eq!(
                decision.candidate, "hold",
                "low stock should now fall through to hold"
            );
        }
        ReplayResult::MissingInputs { .. } => panic!("expected a decision"),
    }

    // retract: removing `stock` makes the input contract incomplete.
    let outcome3 =
        ops::retract_inputs(&kb_root, &["stock".to_string()], &[]).expect("retract should succeed");
    assert_eq!(outcome3.record.seq, 3);
    assert_eq!(outcome3.record.result.status, Status::MissingInputs);
    match &outcome3.replay {
        ReplayResult::MissingInputs { missing } => assert_eq!(missing, &["stock".to_string()]),
        ReplayResult::Ran { .. } => panic!("expected missing inputs"),
    }

    // retracting a name that isn't currently set is a usage error, and does
    // not create a new revision.
    let err = expect_err(ops::retract_inputs(&kb_root, &["stock".to_string()], &[]));
    assert_eq!(err.code, "kb-retract-not-set");
    assert_eq!(err.status, "usage-error");
    let head = ops::read_head(&kb_root).unwrap();
    assert_eq!(head.seq, 3, "a rejected retract must not create a revision");

    // re-assert `stock` to make the contract complete again (a correction).
    let restock = write_input_shard(
        &tmp.path,
        "restock.json",
        r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"100"}}}"#,
    );
    let outcome4 = ops::assert_inputs(&kb_root, &[restock], &[]).expect("assert should succeed");
    assert_eq!(outcome4.record.seq, 4);
    assert_ran_selected(&outcome4.replay, "expedite");

    // program: switch to a program with an entirely different input contract;
    // every previously-set input is undeclared under the new program and
    // must be dropped.
    let new_program = example("order-policy.brix");
    let outcome5 =
        ops::set_program(&kb_root, &new_program, &[]).expect("program change should succeed");
    assert_eq!(outcome5.record.seq, 5);
    match &outcome5.record.change {
        crate::revision::Change::Program { dropped_inputs, .. } => {
            let mut dropped = dropped_inputs.clone();
            dropped.sort();
            assert_eq!(
                dropped,
                vec![
                    "eligible".to_string(),
                    "region".to_string(),
                    "stock".to_string()
                ]
            );
        }
        other => panic!("expected a Program change, got {other:?}"),
    }
    assert_eq!(outcome5.record.result.status, Status::MissingInputs);

    // log: every revision, oldest first, replayed fresh.
    let log = ops::log(&kb_root, &[]).expect("log should succeed");
    assert_eq!(log.len(), 5);
    for (i, entry) in log.iter().enumerate() {
        assert_eq!(entry.record.seq, (i + 1) as u64);
    }

    // show: defaults to HEAD.
    let shown = ops::show(&kb_root, None, &[]).expect("show should succeed");
    assert_eq!(shown.record.seq, 5);
    let shown_1 = ops::show(&kb_root, Some(1), &[]).expect("show rev 1 should succeed");
    assert_eq!(shown_1.record.seq, 1);
    assert_ran_selected(&shown_1.replay, "ship");
}

// ---------------------------------------------------------------------------
// non-erasure: old revisions stay readable and verifiable after later changes
// ---------------------------------------------------------------------------

#[test]
fn test_non_erasure_old_revisions_survive_later_changes() {
    let tmp = TempDir::new("non_erasure");
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    let first = ops::init(&kb_root, &program, &[input], &[]).unwrap();

    for i in 0..3 {
        let shard = write_input_shard(
            &tmp.path,
            &format!("shard_{i}.json"),
            &format!(
                r#"{{"schema":"brix.input@1","values":{{"stock":{{"type":"int","value":"{}"}}}}}}"#,
                5 + i
            ),
        );
        ops::assert_inputs(&kb_root, &[shard], &[]).unwrap();
    }

    // Revision 1 is exactly as it was originally, byte-for-byte re-derivable.
    let reread = ops::read_revision(&kb_root, 1).unwrap();
    assert_eq!(reread, first.record);
    let replayed = ops::show(&kb_root, Some(1), &[]).unwrap();
    assert_ran_selected(&replayed.replay, "ship");

    // The whole chain, including the untouched first revision, still verifies.
    let report = ops::verify(&kb_root, &[]).expect("verify should pass");
    assert_eq!(report.revisions_checked, 4);
}

// ---------------------------------------------------------------------------
// diff, with "why" chains
// ---------------------------------------------------------------------------

#[test]
fn test_diff_why_chain_stock_changes_can_ship_threshold_unchanged() {
    let tmp = TempDir::new("diff");
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json"); // stock=12, eligible=true, region=EU-NORTH
    ops::init(&kb_root, &program, &[input], &[]).unwrap();

    let corrected = write_input_shard(
        &tmp.path,
        "correction.json",
        r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"3"}}}"#,
    );
    ops::assert_inputs(&kb_root, &[corrected], &[]).unwrap();

    let report = crate::diff::diff(&kb_root, 1, 2, &[]).expect("diff should succeed");
    assert_eq!(report.inputs_changed.len(), 1);
    assert_eq!(report.inputs_changed[0].name, "stock");

    let can_ship = report
        .facts_changed
        .iter()
        .find(|f| f.name == "can_ship")
        .expect("can_ship must have changed");
    assert!(can_ship.why_inputs.contains(&"stock".to_string()));
    assert!(!can_ship.why_inputs.contains(&"threshold".to_string()));

    // `threshold` itself is a bare constant: it never changes, and must not
    // appear in the changed-facts list at all.
    assert!(!report.facts_changed.iter().any(|f| f.name == "threshold"));

    // `destination`/`valid_destination` are unaffected by a `stock`-only
    // change, so they too must be absent from facts_changed.
    assert!(!report.facts_changed.iter().any(|f| f.name == "destination"));

    assert_eq!(report.decision_a.as_ref().unwrap().0, "ship");
    assert_eq!(report.decision_b.as_ref().unwrap().0, "hold");
}

#[test]
fn test_diff_every_fact_outside_the_affected_set_is_unchanged() {
    // "Impact analysis must be honest": every fact NOT named by `why` must be
    // byte-identical across the two revisions, confirmed independently of the
    // dependency graph by literally comparing the two fresh replays.
    let tmp = TempDir::new("diff_honesty");
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    ops::init(&kb_root, &program, &[input], &[]).unwrap();
    let corrected = write_input_shard(
        &tmp.path,
        "correction.json",
        r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"3"}}}"#,
    );
    ops::assert_inputs(&kb_root, &[corrected], &[]).unwrap();

    let report = crate::diff::diff(&kb_root, 1, 2, &[]).unwrap();
    let changed_names: std::collections::BTreeSet<&str> = report
        .facts_changed
        .iter()
        .map(|f| f.name.as_str())
        .collect();

    let rev1 = ops::show(&kb_root, Some(1), &[]).unwrap();
    let rev2 = ops::show(&kb_root, Some(2), &[]).unwrap();
    let (ReplayResult::Ran { run: run1, .. }, ReplayResult::Ran { run: run2, .. }) =
        (&rev1.replay, &rev2.replay)
    else {
        panic!("expected both revisions to run")
    };
    let facts1: std::collections::BTreeMap<&str, &brix_lower::l3_v2::L3ValueV2> = run1
        .facts
        .iter()
        .map(|f| (f.rule.as_str(), &f.value))
        .collect();
    let facts2: std::collections::BTreeMap<&str, &brix_lower::l3_v2::L3ValueV2> = run2
        .facts
        .iter()
        .map(|f| (f.rule.as_str(), &f.value))
        .collect();
    for (name, v1) in &facts1 {
        if changed_names.contains(name) {
            continue;
        }
        assert_eq!(
            Some(*v1),
            facts2.get(name).copied(),
            "fact '{name}' outside the changed set must have an identical value"
        );
    }
}

// ---------------------------------------------------------------------------
// audit + verify (library-level equivalent of `brix verify`)
// ---------------------------------------------------------------------------

#[test]
fn test_audit_bundle_verifies_like_brix_verify() {
    let tmp = TempDir::new("audit");
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    ops::init(&kb_root, &program, &[input], &[]).unwrap();

    let bundle_path = tmp.path.join("bundle.bin");
    let audit =
        ops::audit_revision(&kb_root, 1, &bundle_path, false, &[]).expect("audit should succeed");
    assert!(bundle_path.exists());

    // Re-derive everything from the stored program + snapshot, exactly the
    // way `brix verify` does, and confirm the bundle checks out.
    let record = ops::read_revision(&kb_root, 1).unwrap();
    let (_plan, snapshot, _replay) = ops::load_and_replay(&kb_root, &record, &[]).unwrap();
    let source =
        std::fs::read_to_string(paths::program_file(&kb_root, record.program_id.digest())).unwrap();
    let resolved_module = pipeline::load_resolved_module(&source, &[]).unwrap();

    let decode_limits = brix_lower::AuditDecodeLimits::strict();
    let bytes = std::fs::read(&bundle_path).unwrap();
    let decoded = brix_lower::decode_audit_input_bundle_v1(&bytes, &decode_limits).unwrap();
    let plan_limits = brix_lower::PlanLimitsV1::generous();
    let report = brix_lower::audit_bundle::check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
        &resolved_module,
        record.program_id,
        &plan_limits,
        &decoded,
        &decode_limits,
        &snapshot,
    )
    .expect("a `brix kb audit` bundle must verify exactly like `brix verify`");
    assert_eq!(report.bundle_id.digest().to_hex(), audit.bundle_id_hex);
    assert_eq!(report.final_chain.to_hex(), audit.final_chain_hex);

    // --bundle already exists, without --force: refused.
    let err = expect_err(ops::audit_revision(&kb_root, 1, &bundle_path, false, &[]));
    assert_eq!(err.code, "kb-audit-bundle-exists");
    // With --force it succeeds again.
    ops::audit_revision(&kb_root, 1, &bundle_path, true, &[])
        .expect("forced overwrite should succeed");
}

#[test]
fn test_verify_passes_on_untampered_chain() {
    let tmp = TempDir::new("verify_ok");
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    ops::init(&kb_root, &program, &[input], &[]).unwrap();
    let corrected = write_input_shard(
        &tmp.path,
        "c.json",
        r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"3"}}}"#,
    );
    ops::assert_inputs(&kb_root, &[corrected], &[]).unwrap();

    let report = ops::verify(&kb_root, &[]).expect("verify should pass");
    assert_eq!(report.revisions_checked, 2);
}

fn init_single_revision(tmp: &TempDir) -> PathBuf {
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    ops::init(&kb_root, &program, &[input], &[]).unwrap();
    kb_root
}

#[test]
fn test_verify_fails_after_editing_revision_file() {
    let tmp = TempDir::new("tamper_revision");
    let kb_root = init_single_revision(&tmp);
    let path = paths::revision_file(&kb_root, 1);
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("\"selected\"", "\"quiescent\"");
    assert_ne!(
        text, tampered,
        "the sample must actually contain the status to tamper"
    );
    std::fs::write(&path, tampered).unwrap();

    let err = ops::verify(&kb_root, &[]).unwrap_err();
    assert_eq!(err.status, "unknown");
}

#[test]
fn test_verify_fails_after_editing_snapshot_file() {
    let tmp = TempDir::new("tamper_snapshot");
    let kb_root = init_single_revision(&tmp);
    let record = ops::read_revision(&kb_root, 1).unwrap();
    let path = paths::snapshot_file(&kb_root, record.snapshot_id.digest());
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("\"12\"", "\"999\"");
    assert_ne!(text, tampered);
    std::fs::write(&path, tampered).unwrap();

    let err = ops::verify(&kb_root, &[]).unwrap_err();
    assert_eq!(err.code, "kb-verify-snapshot-id-mismatch");
}

#[test]
fn test_verify_fails_after_editing_program_file() {
    let tmp = TempDir::new("tamper_program");
    let kb_root = init_single_revision(&tmp);
    let record = ops::read_revision(&kb_root, 1).unwrap();
    let path = paths::program_file(&kb_root, record.program_id.digest());
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\nrule extra() = 1\n");
    std::fs::write(&path, text).unwrap();

    let err = ops::verify(&kb_root, &[]).unwrap_err();
    assert_eq!(err.code, "kb-verify-program-id-mismatch");
}

// ---------------------------------------------------------------------------
// strict decoding / usage errors
// ---------------------------------------------------------------------------

#[test]
fn test_strict_decoding_rejects_duplicate_key_in_stored_revision() {
    let tmp = TempDir::new("strict_dup");
    let kb_root = init_single_revision(&tmp);
    let path = paths::revision_file(&kb_root, 1);
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replacen("\"seq\": 1,", "\"seq\": 1,\n  \"seq\": 1,", 1);
    std::fs::write(&path, tampered).unwrap();

    let err = ops::read_revision(&kb_root, 1).unwrap_err();
    assert_eq!(err.code, "kb-revision-decode-error");
}

#[test]
fn test_lock_contention_is_refused() {
    let tmp = TempDir::new("lock");
    let kb_root = tmp.path.join("kb");
    std::fs::create_dir_all(&kb_root).unwrap();
    // Simulate a concurrent writer by pre-creating the lock file.
    std::fs::write(paths::lock(&kb_root), b"pid=999999").unwrap();

    let program = example("shipping-input.brix");
    let err: KbError = expect_err(ops::init(&kb_root, &program, &[], &[]));
    assert_eq!(err.code, "kb-locked");
    assert_eq!(err.status, "usage-error");

    // Once the lock is released, the operation succeeds.
    std::fs::remove_file(paths::lock(&kb_root)).unwrap();
    ops::init(&kb_root, &program, &[], &[]).expect("init should succeed once unlocked");
}

#[test]
fn test_assert_with_undeclared_input_is_rejected_and_creates_no_revision() {
    let tmp = TempDir::new("assert_bad");
    let kb_root = init_single_revision(&tmp);
    let bad = write_input_shard(
        &tmp.path,
        "bad.json",
        r#"{"schema":"brix.input@1","values":{"not_a_real_input":{"type":"int","value":"1"}}}"#,
    );
    let err = expect_err(ops::assert_inputs(&kb_root, &[bad], &[]));
    assert_eq!(err.status, "rejected");
    let head = ops::read_head(&kb_root).unwrap();
    assert_eq!(head.seq, 1, "a rejected assert must not create a revision");
}

#[test]
fn test_init_with_incomplete_inputs_is_honest_missing_inputs() {
    let tmp = TempDir::new("init_incomplete");
    let kb_root = tmp.path.join("kb");
    let program = example("shipping-input.brix");
    // No --input at all: a knowledge base may start incomplete.
    let outcome =
        ops::init(&kb_root, &program, &[], &[]).expect("init should succeed with no inputs");
    assert_eq!(outcome.record.result.status, Status::MissingInputs);
    match outcome.replay {
        ReplayResult::MissingInputs { missing } => {
            let mut missing = missing;
            missing.sort();
            assert_eq!(
                missing,
                vec![
                    "eligible".to_string(),
                    "region".to_string(),
                    "stock".to_string()
                ]
            );
        }
        ReplayResult::Ran { .. } => panic!("expected missing inputs"),
    }
}

// ---------------------------------------------------------------------------
// stored snapshot size
// ---------------------------------------------------------------------------

/// Two inputs that are each within the CLI's single-file limit merge into a
/// stored snapshot larger than that limit; later operations must still read
/// it back.
#[test]
fn test_snapshot_merged_from_several_inputs_stays_readable() {
    let tmp = TempDir::new("big_snapshot");
    let kb_root = tmp.path.join("kb");
    let program = tmp.path.join("big.brix");
    std::fs::write(
        &program,
        "config D = Go\ninput a: List<Str> max 16\ninput b: List<Str> max 16\n\
         propose go priority 1 when true = Go\ncommit d from (go)\n",
    )
    .unwrap();
    let text = "x".repeat(60_000);
    let shard = |name: &str| {
        let items = vec![format!(r#"{{"type":"string","value":"{text}"}}"#); 12].join(",");
        format!(
            r#"{{"schema":"brix.input@3","values":{{"{name}":{{"type":"list","items":[{items}]}}}}}}"#
        )
    };
    let a = write_input_shard(&tmp.path, "a.json", &shard("a"));
    let b = write_input_shard(&tmp.path, "b.json", &shard("b"));

    ops::init(&kb_root, &program, &[a, b], &[]).expect("init should succeed");
    let snapshot_bytes: u64 = std::fs::read_dir(kb_root.join("snapshots"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(
        snapshot_bytes > brix_lower::input::MAX_INPUT_FILE_BYTES as u64,
        "the stored snapshot must exceed one CLI input file for this test to mean anything"
    );

    let revisions = ops::log(&kb_root, &[]).expect("log must read the stored snapshot back");
    assert_eq!(revisions.len(), 1);
    ops::verify(&kb_root, &[]).expect("verify must replay the stored snapshot");
}

/// A new program that tightens a list bound drops the stored list instead of
/// failing to replay it.
#[test]
fn test_program_change_drops_values_the_new_declarations_reject() {
    let tmp = TempDir::new("program_tightens_list");
    let kb_root = tmp.path.join("kb");
    let source = |max: u32| {
        format!(
            "config D = Go | Wait\ninput xs: List<Int> max {max}\n\
             propose go priority 1 when len(xs) > 0 = Go\npropose wait otherwise = Wait\n\
             commit d from (go, wait)\n"
        )
    };
    let wide = tmp.path.join("wide.brix");
    let narrow = tmp.path.join("narrow.brix");
    std::fs::write(&wide, source(8)).unwrap();
    std::fs::write(&narrow, source(3)).unwrap();
    let items = [r#"{"type":"int","value":"1"}"#; 5].join(",");
    let input = write_input_shard(
        &tmp.path,
        "xs.json",
        &format!(
            r#"{{"schema":"brix.input@3","values":{{"xs":{{"type":"list","items":[{items}]}}}}}}"#
        ),
    );

    ops::init(&kb_root, &wide, &[input], &[]).expect("init should succeed");
    let outcome = ops::set_program(&kb_root, &narrow, &[])
        .unwrap_or_else(|e| panic!("program change must not fail on a rejected value: {e}"));
    match &outcome.record.change {
        crate::revision::Change::Program { dropped_inputs, .. } => {
            assert_eq!(dropped_inputs, &vec!["xs".to_string()]);
        }
        other => panic!("expected a program change, got {other:?}"),
    }
    assert_eq!(outcome.record.result.status, Status::MissingInputs);
}

/// A crash after writing a revision file but before updating HEAD leaves an
/// uncommitted file past HEAD; the next write replaces it instead of failing.
#[test]
fn test_uncommitted_revision_past_head_does_not_wedge_writes() {
    let tmp = TempDir::new("orphan_revision");
    let kb_root = init_single_revision(&tmp);
    std::fs::write(paths::revision_file(&kb_root, 2), b"{ partial").unwrap();

    let input = example("shipping-input.json");
    let outcome = ops::assert_inputs(&kb_root, &[input], &[])
        .unwrap_or_else(|e| panic!("assert must replace the uncommitted revision: {e}"));
    assert_eq!(outcome.record.seq, 2);
    ops::verify(&kb_root, &[]).expect("the chain must verify after recovery");
}

/// `kb.json` is written last, so an `init` interrupted before it can be rerun.
#[test]
fn test_interrupted_init_can_be_rerun() {
    let tmp = TempDir::new("interrupted_init");
    let kb_root = init_single_revision(&tmp);
    std::fs::remove_file(paths::kb_json(&kb_root)).unwrap();

    let program = example("shipping-input.brix");
    let input = example("shipping-input.json");
    ops::init(&kb_root, &program, &[input], &[])
        .unwrap_or_else(|e| panic!("init must succeed over an interrupted init: {e}"));
    ops::verify(&kb_root, &[]).expect("the rerun init must verify");
}
