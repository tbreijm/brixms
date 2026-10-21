//! ADR-0048 stage 4: the application workflow, through real persistent worlds.
use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};

use brix_canon::{Digest, Domain};
use brix_kb::world::policy_evidence::{
    verify_dispatch_evidence, DispatchEvidence, DispatchTarget, PolicyError,
};
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldSession,
};
use brix_lower::evidence::{Authority, Disposition, DISPATCH_POLICY};

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("brix-policy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn world(&self, id: &str) -> WorldSession {
        let mut world = WorldSession::create(
            self.0.join(id),
            WorldManifest::new(
                id,
                "2026-10-18",
                Digest::of(Domain::Value, b"policy-workflow"),
                vec![RelationDecl::new(
                    "orders",
                    vec!["id".into()],
                    vec!["qty".into()],
                    vec![],
                )],
            ),
        )
        .unwrap();
        world
            .apply_batch(WorldBatch::new(
                0,
                "seed",
                vec![order("o1", "10"), order("o2", "10")],
            ))
            .unwrap();
        world
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn order(id: &str, qty: &str) -> WorldBatchOp {
    let mut tuple = TupleRecord::new();
    tuple.set_str("id", id);
    tuple.set_str("qty", qty);
    WorldBatchOp::Upsert {
        relation: "orders".into(),
        key: WorldKey::from_str(id),
        tuple: tuple.to_tuple(),
    }
}

#[test]
fn request_use_reload_and_independent_check_agree_across_world_updates() {
    let dir = TestDir::new("acceptance");
    let mut world = dir.world("A");
    let expected = world
        .dispatch_target(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    let evidence = world
        .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    let receipt = world
        .use_dispatch_evidence(&evidence, 1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert!(receipt.kernel_certificate.is_none());
    assert_eq!(
        receipt,
        verify_dispatch_evidence(&evidence, &expected).unwrap()
    );
    assert_eq!(evidence.certificate.quantity, 10);
    assert!(!evidence.nodes.is_empty());
    let path = world.dispatch_evidence_path(&expected);
    let original = fs::read(&path).unwrap();
    assert_eq!(
        original,
        world
            .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
            .unwrap()
            .encode()
            .unwrap()
    );

    // Even an unrelated edit changes the exact revision. Reuse needs a new request.
    world
        .apply_batch(WorldBatch::new(1, "unrelated", vec![order("o2", "20")]))
        .unwrap();
    assert!(world
        .use_dispatch_evidence(&evidence, 2, "orders", "o1", DISPATCH_POLICY)
        .is_err());
    let next = world
        .request_dispatch(2, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    assert!(world
        .use_dispatch_evidence(&next, 2, "orders", "o1", DISPATCH_POLICY)
        .is_ok());
    world
        .apply_batch(WorldBatch::new(2, "invalidate", vec![order("o1", "0")]))
        .unwrap();
    assert!(matches!(
        world.request_dispatch(3, "orders", "o1", DISPATCH_POLICY),
        Err(PolicyError::Refused(Disposition::Violated { .. }))
    ));
    assert!(world
        .use_dispatch_evidence(&evidence, 3, "orders", "o1", DISPATCH_POLICY)
        .is_err());
    world
        .apply_batch(WorldBatch::new(
            3,
            "retract",
            vec![WorldBatchOp::Remove {
                relation: "orders".into(),
                key: WorldKey::from_str("o1"),
            }],
        ))
        .unwrap();
    assert!(world
        .request_dispatch(4, "orders", "o1", DISPATCH_POLICY)
        .is_err());
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "old evidence must remain unchanged"
    );
    drop(world);

    let world = WorldSession::open(dir.0.join("A")).unwrap();
    let reloaded = world.load_dispatch_evidence(&expected).unwrap();
    assert_eq!(
        world
            .use_dispatch_evidence(&reloaded, 1, "orders", "o1", DISPATCH_POLICY)
            .unwrap(),
        receipt
    );
    // The child gets only portable bytes and separately selected trust anchors.
    // Remove the world entirely so neither cached decisions nor its store can help.
    let portable = dir.0.join("portable.json");
    fs::write(&portable, original).unwrap();
    drop(world);
    fs::remove_dir_all(dir.0.join("A")).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "independent_checker_process", "--nocapture"])
        .env("BRIX_POLICY_ARTIFACT", &portable)
        .env("BRIX_POLICY_WORLD", expected.world.to_hex())
        .env(
            "BRIX_POLICY_REVISION_DIGEST",
            expected.revision_digest.to_hex(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("independent-policy-verified"));
}

#[test]
fn independent_checker_process() {
    let Ok(path) = std::env::var("BRIX_POLICY_ARTIFACT") else {
        return;
    };
    let anchor = |name| {
        Digest::from_bytes(
            WorldKey::from_hex(&std::env::var(name).unwrap())
                .unwrap()
                .0
                .try_into()
                .unwrap(),
        )
    };
    let expected = DispatchTarget {
        world: anchor("BRIX_POLICY_WORLD"),
        revision: 1,
        revision_digest: anchor("BRIX_POLICY_REVISION_DIGEST"),
        relation: "orders".into(),
        order: "o1".into(),
        policy: DISPATCH_POLICY.into(),
    };
    let evidence = DispatchEvidence::decode(&fs::read(path).unwrap()).unwrap();
    let receipt = verify_dispatch_evidence(&evidence, &expected).unwrap();
    assert!(receipt.authentic());
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    println!("independent-policy-verified");
}

#[test]
fn use_refuses_every_changed_claim_and_context() {
    let dir = TestDir::new("context");
    let mut world = dir.world("A");
    let evidence = world
        .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    assert!(world
        .use_dispatch_evidence(&evidence, 1, "orders", "o2", DISPATCH_POLICY)
        .is_err());
    assert!(world
        .use_dispatch_evidence(&evidence, 1, "orders", "o1", "qty>=10")
        .is_err());
    assert!(world
        .use_dispatch_evidence(&evidence, 0, "orders", "o1", DISPATCH_POLICY)
        .is_err());
    assert!(world
        .use_dispatch_evidence(&evidence, 1, "other", "o1", DISPATCH_POLICY)
        .is_err());
    let other = dir.world("B");
    assert!(other
        .use_dispatch_evidence(&evidence, 1, "orders", "o1", DISPATCH_POLICY)
        .is_err());
    assert!(other.load_dispatch_evidence(&evidence.target).is_err());
    // Also reject a caller-supplied target that differs in world identity only.
    let mut expected = evidence.target.clone();
    expected.world = other.manifest.digest();
    assert!(verify_dispatch_evidence(&evidence, &expected).is_err());
}

#[test]
fn tampering_cannot_replace_the_snapshot_or_promote_authority() {
    let dir = TestDir::new("tamper");
    let mut world = dir.world("A");
    let evidence = world
        .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    let expected = evidence.target.clone();
    let mut bad = evidence.clone();
    bad.certificate.quantity = 999;
    bad.journal = bad
        .journal
        .replace("o1=10", "o1=999")
        .replace("qty=10", "qty=999");
    assert!(
        verify_dispatch_evidence(&bad, &expected).is_err(),
        "matching lies in journal and certificate must not override the Merkle proof"
    );
    let mut bad = evidence.clone();
    bad.revision.relation_roots = BTreeMap::new();
    assert!(verify_dispatch_evidence(&bad, &expected).is_err());
    let mut bad = evidence.clone();
    bad.nodes.clear();
    assert!(verify_dispatch_evidence(&bad, &expected).is_err());
    let mut bad = evidence.clone();
    for bytes in bad.nodes.values_mut() {
        bytes[0] ^= 1;
    }
    assert!(verify_dispatch_evidence(&bad, &expected).is_err());
    for modify in ["order", "policy", "revision", "world"] {
        let mut bad = evidence.clone();
        match modify {
            "order" => bad.certificate.order = "o2".into(),
            "policy" => bad.certificate.policy = "qty>=10".into(),
            "revision" => bad.certificate.revision += 1,
            _ => bad.certificate.world = "other-world".into(),
        }
        assert!(
            verify_dispatch_evidence(&bad, &expected).is_err(),
            "{modify}"
        );
    }
    // A cached authority label is ignored; the independent check can issue only runtime evidence.
    let mut label = evidence.clone();
    label.journal = label
        .journal
        .replace("authority=RuntimeCheck", "authority=ProofKernel");
    let receipt = verify_dispatch_evidence(&label, &expected).unwrap();
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert!(receipt.kernel_certificate.is_none());
    let mut bad = evidence.clone();
    bad.journal = bad.journal.replace("o1=10", "o1=0");
    fs::write(
        world.dispatch_evidence_path(&expected),
        bad.encode().unwrap(),
    )
    .unwrap();
    assert!(world.load_dispatch_evidence(&expected).is_err());
    assert!(
        world
            .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
            .is_err(),
        "a corrupt retained artifact must not be silently replaced"
    );
}

#[test]
fn self_referencing_proof_nodes_are_refused_without_panicking() {
    let dir = TestDir::new("lazy-proof");
    let mut world = dir.world("A");
    let evidence = world
        .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    for digest in evidence.nodes.keys() {
        let mut corrupted = evidence.clone();
        let mut writer = brix_canon::CanonWriter::new();
        writer.write_uint(4); // Node::Lazy has the digest of its referenced node.
        writer.write_bytes(digest.as_bytes());
        corrupted.nodes.insert(*digest, writer.finish());
        let checked =
            std::panic::catch_unwind(|| verify_dispatch_evidence(&corrupted, &evidence.target));
        assert!(
            checked.is_ok(),
            "untrusted lazy proof node panicked the verifier"
        );
        assert!(checked.unwrap().is_err(), "lazy proof node was accepted");
    }
}

#[test]
fn unsupported_missing_and_malformed_inputs_never_get_certificates() {
    let dir = TestDir::new("refusals");
    let mut world = dir.world("A");
    assert!(world
        .request_dispatch(1, "orders", "o1", "custom-policy")
        .is_err());
    assert!(world
        .request_dispatch(1, "orders", "absent", DISPATCH_POLICY)
        .is_err());
    world
        .apply_batch(WorldBatch::new(1, "bad-quantity", vec![order("o1", "1.5")]))
        .unwrap();
    assert!(world
        .request_dispatch(2, "orders", "o1", DISPATCH_POLICY)
        .is_err());
    let mut mismatch = TupleRecord::new();
    mismatch.set_str("id", "another-order");
    mismatch.set_str("qty", "10");
    // The world layer may itself reject the key/id mismatch; if admitted by the
    // storage-only schema, policy checking must refuse it.
    if world
        .apply_batch(WorldBatch::new(
            2,
            "bad-id",
            vec![WorldBatchOp::Upsert {
                relation: "orders".into(),
                key: WorldKey::from_str("o1"),
                tuple: mismatch.to_tuple(),
            }],
        ))
        .is_ok()
    {
        assert!(world
            .request_dispatch(3, "orders", "o1", DISPATCH_POLICY)
            .is_err());
    }
}

#[test]
fn executable_world_requests_read_the_committed_base_relation() {
    use brix_kb::world::WorldNetwork;
    use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
    let dir = TestDir::new("executable");
    let source = "rel input orders: { id: Str, qty: Int } key id\n\
        decide dispatch for o in orders per id {\n\
          propose ship priority 10 when o.qty >= 1 = true\n\
          propose hold priority 20 when true = false\n}\n";
    let graph = ModuleGraph::load(
        "root",
        &|_: &str| Some(source.to_string()),
        ModuleLoaderLimits::default(),
    )
    .unwrap();
    let mut world = WorldSession::create(
        dir.0.join("world"),
        WorldManifest::new(
            "executable",
            "2026-10-18",
            Digest::of(Domain::Value, b"initial"),
            vec![RelationDecl::new(
                "root::orders",
                vec!["id".into()],
                vec!["qty".into()],
                vec![],
            )],
        ),
    )
    .unwrap();
    world
        .set_network(WorldNetwork::from_program(&graph.link().unwrap()).unwrap())
        .unwrap();
    world
        .save_program_closure("root", &[("root".to_string(), source.to_string())].into())
        .unwrap();
    let WorldBatchOp::Upsert { key, tuple, .. } = order("o1", "10") else {
        unreachable!()
    };
    world
        .apply_batch(WorldBatch::new(
            0,
            "seed",
            vec![WorldBatchOp::Upsert {
                relation: "root::orders".into(),
                key,
                tuple,
            }],
        ))
        .unwrap();
    let expected = world
        .dispatch_target(1, "root::orders", "o1", DISPATCH_POLICY)
        .unwrap();
    let evidence = world
        .request_dispatch(1, "root::orders", "o1", DISPATCH_POLICY)
        .unwrap();
    assert_eq!(evidence.certificate.quantity, 10);
    drop(world);
    let world = WorldSession::open(dir.0.join("world")).unwrap();
    let loaded = world.load_dispatch_evidence(&expected).unwrap();
    assert_eq!(
        verify_dispatch_evidence(&loaded, &expected).unwrap(),
        world
            .use_dispatch_evidence(&evidence, 1, "root::orders", "o1", DISPATCH_POLICY)
            .unwrap()
    );
}

#[test]
fn retained_evidence_survives_compaction_of_its_original_trie() {
    use brix_kb::world::gc::{gc_world, GcOptions};
    let dir = TestDir::new("gc");
    let mut world = dir.world("A");
    let evidence = world
        .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    world
        .apply_batch(WorldBatch::new(1, "change", vec![order("o1", "0")]))
        .unwrap();
    drop(world);
    gc_world(
        dir.0.join("A"),
        &GcOptions {
            keep_revisions: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    let world = WorldSession::open(dir.0.join("A")).unwrap();
    assert!(world.pin_revision(1).is_err());
    let retained = world.load_dispatch_evidence(&evidence.target).unwrap();
    assert!(verify_dispatch_evidence(&retained, &evidence.target).is_ok());
}

#[test]
fn concurrent_requests_publish_one_complete_artifact() {
    use std::sync::{Arc, Barrier};
    let dir = TestDir::new("concurrent");
    let first = dir.world("A");
    let second = WorldSession::open(dir.0.join("A")).unwrap();
    let gate = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [first, second]
        .into_iter()
        .map(|mut world| {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                gate.wait();
                world.request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
            })
        })
        .collect();
    let mut successful = Vec::new();
    for handle in handles {
        match handle.join().unwrap() {
            Ok(evidence) => successful.push(evidence),
            Err(PolicyError::World(error))
                if matches!(*error, brix_kb::world::WorldError::WorldLocked(_)) => {}
            Err(other) => panic!("unexpected concurrent request error: {other}"),
        }
    }
    assert!(!successful.is_empty());
    let mut reopened = WorldSession::open(dir.0.join("A")).unwrap();
    let final_evidence = reopened
        .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
        .unwrap();
    for evidence in successful {
        assert_eq!(evidence.encode().unwrap(), final_evidence.encode().unwrap());
    }
    assert_eq!(
        fs::read(reopened.dispatch_evidence_path(&final_evidence.target)).unwrap(),
        final_evidence.encode().unwrap()
    );
}

#[test]
fn interrupted_policy_writer_process() {
    use brix_kb::world::session::WorldLockGuard;
    let Some(root) = std::env::var_os("BRIX_POLICY_INTERRUPTED_WORLD") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut world = WorldSession::open(&root).unwrap();
    if std::env::var("BRIX_POLICY_INTERRUPT_AT").unwrap() == "before-publication" {
        let _guard = WorldLockGuard::acquire(&root).unwrap();
        let expected = world
            .dispatch_target(1, "orders", "o1", DISPATCH_POLICY)
            .unwrap();
        let temporary = world
            .dispatch_evidence_path(&expected)
            .with_extension(format!("tmp.{}", std::process::id()));
        // Model termination while atomic_write is still writing its temporary file.
        fs::write(temporary, b"{\"schema\":").unwrap();
        std::process::exit(0);
    } else {
        world
            .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
            .unwrap();
    }
    // Deliberately bypass destructors, as an abruptly terminated writer would.
    std::process::exit(0);
}

#[test]
fn interrupted_publication_and_post_commit_exit_recover_on_reopen() {
    for phase in ["before-publication", "after-publication"] {
        let dir = TestDir::new(phase);
        let world = dir.world("A");
        let expected = world
            .dispatch_target(1, "orders", "o1", DISPATCH_POLICY)
            .unwrap();
        let path = world.dispatch_evidence_path(&expected);
        drop(world);
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "interrupted_policy_writer_process"])
            .env("BRIX_POLICY_INTERRUPTED_WORLD", dir.0.join("A"))
            .env("BRIX_POLICY_INTERRUPT_AT", phase)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(path.exists(), phase == "after-publication");
        let mut reopened = WorldSession::open(dir.0.join("A")).unwrap();
        let evidence = reopened
            .request_dispatch(1, "orders", "o1", DISPATCH_POLICY)
            .unwrap();
        assert!(verify_dispatch_evidence(&evidence, &expected).is_ok());
        assert_eq!(fs::read(path).unwrap(), evidence.encode().unwrap());
        assert_eq!(
            reopened.current_revision(),
            1,
            "publication must not advance the world"
        );
    }
}
