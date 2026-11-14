use brix_kb::world::{world_revision_artifact_digest_v1, world_revision_artifact_preimage_v1};
use brix_kb::world::{
    AuthorizedHostScope, HostContextExpectationV2, HostQueryLimitsV1, HostScopeProfileV1,
    NamedQueryArgsV1, QueryValueV1, ScopeError, ScopedCursorBindingsV1, ScopedQueryCursorV1,
    ScopedQueryPageRequestV1,
};
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
use brix_semantic::*;
use std::collections::{BTreeMap, BTreeSet};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn dims(tenant: &str) -> ContextDimensionsV2 {
    let revision = revision_digest();
    let program = program_digest();
    ContextDimensionsV2 {
        parent: None,
        program: Some(ArtifactRefV2 {
            kind: "program".into(),
            digest: program,
        }),
        world: Some(ArtifactRefV2 {
            kind: "world.revision@1".into(),
            digest: world_revision_artifact_digest_v1("world-test", 7, revision, program),
        }),
        namespace: "catalogue".into(),
        tenant: tenant.into(),
        principal: "host-principal".into(),
        capability_view: profile().context_capability_view().unwrap(),
        scenario_mode: ScenarioModeV2::Production,
        branch: None,
        valid_time_start: 0,
        valid_time_end: 100,
        transaction_time: None,
        correction_time: None,
        simulation_time: None,
        observation_profile: "none@1".into(),
        redaction_profile: "none@1".into(),
        kernel_profile: "none".into(),
        regime_profile: "none".into(),
        axiom_profile: "none".into(),
        validity_start: 0,
        validity_end: 100,
        step_budget: 50_000,
        depth_budget: 32,
    }
}

fn revision_digest() -> brix_canon::Digest {
    brix_canon::Digest::of(brix_canon::Domain::Value, b"world-revision")
}
fn program_digest() -> brix_canon::Digest {
    brix_canon::Digest::of(brix_canon::Domain::Value, b"program-preimage")
}

struct Resolver;
impl ContextResolverV2 for Resolver {
    fn context_bytes(&self, _: &ContextId) -> Option<Vec<u8>> {
        None
    }
    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>> {
        match reference.kind.as_str() {
            "program" => Some(b"program-preimage".to_vec()),
            "world.revision@1" => Some(world_revision_artifact_preimage_v1(
                "world-test",
                7,
                revision_digest(),
                program_digest(),
            )),
            _ => None,
        }
    }
}
struct NoAssumptions;
impl AssumptionValidatorV2 for NoAssumptions {
    fn validate(&self, _: &str, _: &[u8], _: &mut usize) -> bool {
        false
    }
}

fn profile() -> HostScopeProfileV1 {
    HostScopeProfileV1 {
        profile_id: brix_kb::world::HOST_SCOPE_PROFILE_PARTITION_V1.into(),
        partition_fields: BTreeMap::from([("catalogue_entries".into(), "tenant".into())]),
        global_relations: BTreeSet::new(),
        query_limits: HostQueryLimitsV1 {
            page_rows: 128,
            max_input_rows: 1000,
            max_input_bytes: 1_000_000,
            max_eval_work: 10_000,
            max_eval_matches: 10_000,
            max_output_rows: 100,
        },
    }
}

fn context(tenant: &str) -> (ContextDimensionsV2, Vec<u8>, ContextId) {
    let dimensions = dims(tenant);
    let model = ContextModelV2::new(dimensions.clone(), vec![]).unwrap();
    let bytes = encode_context_model_v2(&model);
    let id = model.id();
    (dimensions, bytes, id)
}

#[test]
fn host_scope_requires_independent_v2_context_binding() {
    let (dimensions, bytes, id) = context("tenant-a");
    let authorized = AuthorizedHostScope::authorize(
        profile(),
        HostContextExpectationV2 {
            bytes: &bytes,
            id,
            dimensions: &dimensions,
            limits: ModelLimits::default(),
        },
        &Resolver,
        &NoAssumptions,
    )
    .unwrap();
    assert_eq!(authorized.tenant(), "tenant-a");
    authorized
        .check_execution_bindings("world-test", 7, revision_digest(), program_digest())
        .unwrap();
    assert!(matches!(
        authorized.check_execution_bindings("other-world", 7, revision_digest(), program_digest(),),
        Err(ScopeError::WorldBindingMismatch)
    ));
    let (other_dims, other_bytes, other_id) = context("tenant-b");
    let other = AuthorizedHostScope::authorize(
        profile(),
        HostContextExpectationV2 {
            bytes: &other_bytes,
            id: other_id,
            dimensions: &other_dims,
            limits: ModelLimits::default(),
        },
        &Resolver,
        &NoAssumptions,
    )
    .unwrap();
    assert_ne!(authorized.identity(), other.identity());

    let (_, other_bytes, _) = context("tenant-b");
    assert!(matches!(
        AuthorizedHostScope::authorize(
            profile(),
            HostContextExpectationV2 {
                bytes: &other_bytes,
                id,
                dimensions: &dimensions,
                limits: ModelLimits::default()
            },
            &Resolver,
            &NoAssumptions,
        ),
        Err(ScopeError::InvalidContext(
            ModelError::IdentityMismatch | ModelError::BindingMismatch
        ))
    ));
}

#[test]
fn profile_rejects_ambiguous_and_empty_relation_bindings() {
    let mut bad = profile();
    bad.global_relations.insert("catalogue_entries".into());
    assert!(bad.validate().is_err());
    let mut bad = profile();
    bad.partition_fields.insert("".into(), "tenant".into());
    assert!(bad.validate().is_err());
}

fn files(root: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files(&entry.path(), base, out);
        } else {
            out.insert(
                entry.path().strip_prefix(base).unwrap().to_path_buf(),
                fs::read(entry.path()).unwrap(),
            );
        }
    }
}

#[test]
fn readonly_open_and_reads_leave_world_files_unchanged() {
    use brix_kb::world::{RelationDecl, WorldManifest, WorldSession};
    let root = std::env::temp_dir().join(format!("brix_scope_readonly_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let manifest = WorldManifest::new(
        "scope",
        "2026-11-01",
        brix_canon::Digest::of(brix_canon::Domain::Value, b"init"),
        vec![RelationDecl::new(
            "rows",
            vec!["id".into()],
            vec!["tenant".into()],
            vec![],
        )],
    );
    drop(WorldSession::create(&root, manifest).unwrap());
    let mut before = BTreeMap::new();
    files(&root, &root, &mut before);
    let mut read = WorldSession::open_read_only(&root).unwrap();
    assert_eq!(read.current_revision, 0);
    assert!(
        read.network.is_none(),
        "read-only open must not replay globally"
    );
    assert!(matches!(
        read.persist_idempotency_index(),
        Err(brix_kb::world::WorldError::ReadOnlySession)
    ));
    let mut after = BTreeMap::new();
    files(&root, &root, &mut after);
    assert_eq!(before, after);
    fs::remove_dir_all(root).unwrap();
}

fn cursor_bindings() -> ScopedCursorBindingsV1 {
    let (_, _, context_identity) = context("tenant-a");
    let d = |s: &[u8]| brix_canon::Digest::of(brix_canon::Domain::Value, s);
    ScopedCursorBindingsV1 {
        query_identity: d(b"query"),
        program_identity: d(b"program"),
        typed_arguments: d(b"args"),
        scope_identity: d(b"scope"),
        context_identity,
        world_identity: "world-1".into(),
        revision_seq: 7,
        revision_digest: d(b"revision"),
        ordering_version: 1,
    }
}

#[test]
fn bound_cursor_rejects_tampering_and_every_changed_binding() {
    let bindings = cursor_bindings();
    let cursor = ScopedQueryCursorV1::new(bindings.clone(), 23);
    let token = cursor.to_token();
    let parsed = ScopedQueryCursorV1::from_token(&token, &bindings).unwrap();
    assert_eq!(parsed.position(), 23);
    let mut tampered = token.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'0' { b'1' } else { b'0' };
    assert!(
        ScopedQueryCursorV1::from_token(std::str::from_utf8(&tampered).unwrap(), &bindings)
            .is_err()
    );

    let mut changed = Vec::new();
    let mut b = bindings.clone();
    b.query_identity = brix_canon::Digest::of(brix_canon::Domain::Value, b"other-query");
    changed.push(b);
    let mut b = bindings.clone();
    b.program_identity = brix_canon::Digest::of(brix_canon::Domain::Value, b"other-program");
    changed.push(b);
    let mut b = bindings.clone();
    b.typed_arguments = brix_canon::Digest::of(brix_canon::Domain::Value, b"other-args");
    changed.push(b);
    let mut b = bindings.clone();
    b.scope_identity = brix_canon::Digest::of(brix_canon::Domain::Value, b"other-scope");
    changed.push(b);
    let mut b = bindings.clone();
    b.context_identity = context("tenant-b").2;
    changed.push(b);
    let mut b = bindings.clone();
    b.world_identity = "world-2".into();
    changed.push(b);
    let mut b = bindings.clone();
    b.revision_seq += 1;
    changed.push(b);
    let mut b = bindings.clone();
    b.revision_digest = brix_canon::Digest::of(brix_canon::Domain::Value, b"other-revision");
    changed.push(b);
    let mut b = bindings.clone();
    b.ordering_version += 1;
    changed.push(b);
    for changed in changed {
        assert!(matches!(
            ScopedQueryCursorV1::from_token(&token, &changed),
            Err(ScopeError::CursorBindingMismatch)
        ));
    }
    assert!(ScopedQueryCursorV1::from_token("legacy-alpha8-cursor", &bindings).is_err());
}

const QUERY_PROGRAM: &str = r#"
rel input catalogue: { row: Str, id: Str, tenant: Str, kind: Str } key row
rel input blocked: { row: Str, id: Str, tenant: Str } key row
rel input labels: { row: Str, id: Str, tenant: Str } key row
query catalogue_for(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue where c.kind == kind && not exists b in blocked where b.id == c.id
query catalogue_count(kind: Str) = select { kind: c.kind, total: count() }
  from c in catalogue where c.kind == kind group by c.kind
query catalogue_join(kind: Str) = select { id: c.id, label_id: l.id }
  from c in catalogue, l in labels where c.kind == kind && c.id == l.id
"#;

struct ArtifactStore(BTreeMap<brix_canon::Digest, Vec<u8>>);
impl ContextResolverV2 for ArtifactStore {
    fn context_bytes(&self, _: &ContextId) -> Option<Vec<u8>> {
        None
    }
    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>> {
        self.0.get(&reference.digest).cloned()
    }
}

fn query_context(
    world: &WorldSession,
    profile: HostScopeProfileV1,
    revision_seq: u64,
    tenant: &str,
) -> AuthorizedHostScope {
    let revision = world.pin_revision(revision_seq).unwrap();
    let program_bytes = world.program_artifact_preimage().unwrap();
    let world_bytes = world_revision_artifact_preimage_v1(
        &world.manifest.world_id,
        revision_seq,
        revision.revision_digest(),
        world.manifest.program_digest,
    );
    let mut dims = dims(tenant);
    dims.capability_view = profile.context_capability_view().unwrap();
    dims.world = Some(ArtifactRefV2 {
        kind: "world.revision@1".into(),
        digest: brix_canon::Digest::of(brix_canon::Domain::Value, &world_bytes),
    });
    dims.program = Some(ArtifactRefV2 {
        kind: "program".into(),
        digest: brix_canon::Digest::of(brix_canon::Domain::Value, &program_bytes),
    });
    let model = ContextModelV2::new(dims.clone(), vec![]).unwrap();
    let bytes = encode_context_model_v2(&model);
    let resolver = ArtifactStore(BTreeMap::from([
        (dims.world.as_ref().unwrap().digest, world_bytes),
        (dims.program.as_ref().unwrap().digest, program_bytes),
    ]));
    AuthorizedHostScope::authorize(
        profile,
        HostContextExpectationV2 {
            bytes: &bytes,
            id: model.id(),
            dimensions: &dims,
            limits: ModelLimits::default(),
        },
        &resolver,
        &NoAssumptions,
    )
    .unwrap()
}

#[test]
fn scoped_query_filters_bases_before_absence_and_pins_revision() {
    let root = std::env::temp_dir().join(format!("brix_scope_query_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let sources = BTreeMap::from([("root".to_string(), QUERY_PROGRAM.to_string())]);
    let mut world = WorldSession::from_program_with_sources(&root, "root", &sources).unwrap();
    let mut catalog_a = TupleRecord::new();
    catalog_a.set_str("row", "a-row");
    catalog_a.set_str("id", "shared");
    catalog_a.set_str("tenant", "tenant-a");
    catalog_a.set_str("kind", "book");
    let mut catalog_b = TupleRecord::new();
    catalog_b.set_str("row", "b-row");
    catalog_b.set_str("id", "shared");
    catalog_b.set_str("tenant", "tenant-b");
    catalog_b.set_str("kind", "book");
    let mut blocked_b = TupleRecord::new();
    blocked_b.set_str("row", "blocked-b");
    blocked_b.set_str("id", "shared");
    blocked_b.set_str("tenant", "tenant-b");
    let mut catalog_a2 = TupleRecord::new();
    catalog_a2.set_str("row", "a-row-2");
    catalog_a2.set_str("id", "another");
    catalog_a2.set_str("tenant", "tenant-a");
    catalog_a2.set_str("kind", "book");
    let mut label_a = TupleRecord::new();
    label_a.set_str("row", "label-a");
    label_a.set_str("id", "shared");
    label_a.set_str("tenant", "tenant-a");
    let mut label_b = TupleRecord::new();
    label_b.set_str("row", "label-b");
    label_b.set_str("id", "shared");
    label_b.set_str("tenant", "tenant-b");
    world
        .apply_batch(WorldBatch::new(
            0,
            "seed",
            vec![
                WorldBatchOp::Upsert {
                    relation: "root::catalogue".into(),
                    key: WorldKey::from_str("a-row"),
                    tuple: catalog_a.to_tuple(),
                },
                WorldBatchOp::Upsert {
                    relation: "root::catalogue".into(),
                    key: WorldKey::from_str("b-row"),
                    tuple: catalog_b.to_tuple(),
                },
                WorldBatchOp::Upsert {
                    relation: "root::catalogue".into(),
                    key: WorldKey::from_str("a-row-2"),
                    tuple: catalog_a2.to_tuple(),
                },
                WorldBatchOp::Upsert {
                    relation: "root::blocked".into(),
                    key: WorldKey::from_str("blocked-b"),
                    tuple: blocked_b.to_tuple(),
                },
                WorldBatchOp::Upsert {
                    relation: "root::labels".into(),
                    key: WorldKey::from_str("label-a"),
                    tuple: label_a.to_tuple(),
                },
                WorldBatchOp::Upsert {
                    relation: "root::labels".into(),
                    key: WorldKey::from_str("label-b"),
                    tuple: label_b.to_tuple(),
                },
            ],
        ))
        .unwrap();
    drop(world);
    let world = WorldSession::open_read_only(&root).unwrap();
    let profile = HostScopeProfileV1 {
        profile_id: brix_kb::world::HOST_SCOPE_PROFILE_PARTITION_V1.into(),
        partition_fields: BTreeMap::from([
            ("root::catalogue".into(), "tenant".into()),
            ("root::blocked".into(), "tenant".into()),
            ("root::labels".into(), "tenant".into()),
        ]),
        global_relations: BTreeSet::new(),
        query_limits: HostQueryLimitsV1 {
            page_rows: 1,
            max_input_rows: 100,
            max_input_bytes: 1_000_000,
            max_eval_work: 10_000,
            max_eval_matches: 10_000,
            max_output_rows: 100,
        },
    };
    let scope = query_context(&world, profile.clone(), 1, "tenant-a");
    let request = ScopedQueryPageRequestV1 {
        query_name: "catalogue_for".into(),
        arguments: NamedQueryArgsV1(BTreeMap::from([(
            "kind".into(),
            QueryValueV1::Str("book".into()),
        )])),
        revision: None,
        limit: 1,
        max_input_rows: 100,
        max_input_bytes: 1_000_000,
        max_eval_work: 10_000,
        max_eval_matches: 10_000,
        max_output_rows: 100,
        cursor: None,
    };
    let mut before_query = BTreeMap::new();
    files(&root, &root, &mut before_query);
    let page = world.query_named_scoped_page(&request, &scope).unwrap();
    let mut after_query = BTreeMap::new();
    files(&root, &root, &mut after_query);
    assert_eq!(
        before_query, after_query,
        "scoped execution must not write cache/world files"
    );
    assert_eq!(page.revision_seq, 1);
    assert_eq!(page.typed_rows.len(), 1);
    assert_eq!(page.result_schema.len(), 2);
    assert!(page.has_more);
    let cursor = page.next_cursor.clone().unwrap();

    // Advance HEAD between pages. Continuation follows its original pin.
    let mut writer = WorldSession::open(&root).unwrap();
    let mut later = TupleRecord::new();
    later.set_str("row", "a-row-later");
    later.set_str("id", "later");
    later.set_str("tenant", "tenant-a");
    later.set_str("kind", "book");
    writer
        .apply_batch(WorldBatch::new(
            1,
            "later",
            vec![WorldBatchOp::Upsert {
                relation: "root::catalogue".into(),
                key: WorldKey::from_str("a-row-later"),
                tuple: later.to_tuple(),
            }],
        ))
        .unwrap();
    drop(writer);
    let reopened = WorldSession::open_read_only(&root).unwrap();
    let page2 = reopened
        .query_named_scoped_page(
            &ScopedQueryPageRequestV1 {
                cursor: Some(cursor),
                ..request.clone()
            },
            &scope,
        )
        .unwrap();
    assert_eq!(page2.revision_seq, 1);
    assert_eq!(page2.typed_rows.len(), 1);
    assert!(!page2.has_more);

    let count = world
        .query_named_scoped_page(
            &ScopedQueryPageRequestV1 {
                query_name: "catalogue_count".into(),
                cursor: None,
                limit: 1,
                ..request.clone()
            },
            &scope,
        )
        .unwrap();
    assert_eq!(
        count.typed_rows,
        vec![serde_json::json!({"kind":"book","total":2})]
    );
    let joined = world
        .query_named_scoped_page(
            &ScopedQueryPageRequestV1 {
                query_name: "catalogue_join".into(),
                cursor: None,
                limit: 1,
                ..request.clone()
            },
            &scope,
        )
        .unwrap();
    assert_eq!(joined.typed_rows.len(), 1);

    std::thread::scope(|threads| {
        let left = threads.spawn(|| world.query_named_scoped_page(&request, &scope).unwrap());
        let right = threads.spawn(|| world.query_named_scoped_page(&request, &scope).unwrap());
        assert_eq!(
            left.join().unwrap().typed_rows,
            right.join().unwrap().typed_rows
        );
    });

    let scope_b = query_context(&world, profile, 1, "tenant-b");
    let for_b = world
        .query_named_scoped_page(
            &ScopedQueryPageRequestV1 {
                cursor: None,
                ..request.clone()
            },
            &scope_b,
        )
        .unwrap();
    assert!(for_b.typed_rows.is_empty());

    let over_budget = ScopedQueryPageRequestV1 {
        max_input_rows: 101,
        ..request
    };
    assert!(world.query_named_scoped_page(&over_budget, &scope).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn named_query_arguments_require_exact_names_and_scalar_types() {
    let module = brix_syntax::parse(
        "query q(kind: Str) = select { id: c.id } from c in catalogue where c.kind == kind",
    )
    .unwrap();
    let brix_syntax::ast::Item::Query(query) = &module.items[0] else {
        panic!("query declaration")
    };
    assert!(matches!(
        NamedQueryArgsV1::default().validate(query),
        Err(ScopeError::QueryArgumentSetMismatch)
    ));
    assert!(matches!(
        NamedQueryArgsV1(BTreeMap::from([("kind".into(), QueryValueV1::Int(1))])).validate(query),
        Err(ScopeError::QueryArgumentTypeMismatch(_))
    ));
    assert!(matches!(
        NamedQueryArgsV1(BTreeMap::from([
            ("kind".into(), QueryValueV1::Str("a".into())),
            ("extra".into(), QueryValueV1::Bool(true))
        ]))
        .validate(query),
        Err(ScopeError::QueryArgumentSetMismatch)
    ));
}
