//! Typed child-key ordering and prefix collection regressions for Alpha 9-6.
use brix_kb::world::chunk_policy::FixedChunkPolicy;
use brix_kb::world::decision_codec::Value;
use brix_kb::world::reference::{self, CollectOrdering, Value as ReferenceValue};
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldSession};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "brix-collect-take-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BASIC_PROGRAM: &str = r#"
config Page = Page(List<Str>, Bool)

rel input rows: { id: Str, parent: Str, sort: Int, value: Str } key id

rel derived page0 = select {
    parent: r.parent,
    page: collect(r.value, r.sort).take(0)
} from r in rows group by r.parent

rel derived page1 = select {
    parent: r.parent,
    page: collect(r.value, r.sort).take(1)
} from r in rows group by r.parent

rel derived page2 = select {
    parent: r.parent,
    page: collect(r.value, r.sort).take(2)
} from r in rows group by r.parent

rel derived page3 = select {
    parent: r.parent,
    page: collect(r.value, r.sort).take(3)
} from r in rows group by r.parent

rel derived page4 = select {
    parent: r.parent,
    page: collect(r.value, r.sort).take(4)
} from r in rows group by r.parent

decide d0 for p in page0 per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
decide d1 for p in page1 per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
decide d2 for p in page2 per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
decide d3 for p in page3 per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
decide d4 for p in page4 per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
"#;

fn row_upsert(id: &str, parent: &str, sort: i64, value: &str) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("parent", parent);
    row.set_str("sort", &sort.to_string());
    row.set_str("value", value);
    WorldBatchOp::Upsert {
        relation: "root::rows".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

fn row_remove(id: &str) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: "root::rows".into(),
        key: WorldKey::from_str(id),
    }
}

fn page(session: &WorldSession, decide: &str) -> (Vec<String>, bool) {
    page_for(session, decide, "P")
}

fn page_for(session: &WorldSession, decide: &str, entity: &str) -> (Vec<String>, bool) {
    let value = &session.all_settlements().unwrap()[&format!("root::{decide}")][entity].value;
    let Value::Variant { args, .. } = value else {
        panic!("expected Page variant, got {value:?}");
    };
    let [Value::List(items), Value::Bool(truncated)] = args.as_slice() else {
        panic!("expected Page(List<Str>, Bool), got {args:?}");
    };
    let values = items
        .iter()
        .map(|item| match item {
            Value::Str(value) => value.clone(),
            other => panic!("expected string item, got {other:?}"),
        })
        .collect();
    (values, *truncated)
}

fn apply_and_reopen(
    mut session: WorldSession,
    dir: &TestDir,
    description: &str,
    ops: Vec<WorldBatchOp>,
) -> WorldSession {
    let expected_revision = session.current_revision();
    session
        .apply_batch(WorldBatch::new(expected_revision, description, ops))
        .unwrap();
    session
        .verify_oracle()
        .expect("incremental/reference agreement");
    drop(session);
    let reopened = WorldSession::open(&dir.0).expect("cold reopen");
    reopened
        .verify_oracle()
        .expect("reopened incremental/reference agreement");
    reopened
}

#[test]
fn take_prefix_truncation_and_retractions_use_typed_primary_key_order() {
    let dir = TestDir::new("prefix-retract");
    let sources = BTreeMap::from([("root".into(), BASIC_PROGRAM.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create world");

    // The opaque canonical WorldKey order differs from typed string order for
    // unequal string lengths. Equal sort values must follow declared key values.
    session = apply_and_reopen(
        session,
        &dir,
        "seed equal-sort children",
        vec![
            row_upsert("aa", "P", 0, "AA"),
            row_upsert("b", "P", 0, "B"),
            row_upsert("c", "P", 0, "same"),
            row_upsert("z", "P", 0, "same"),
        ],
    );
    assert_eq!(page(&session, "d0"), (vec![], true));
    assert_eq!(page(&session, "d1"), (vec!["AA".into()], true));
    assert_eq!(page(&session, "d2"), (vec!["AA".into(), "B".into()], true));
    assert_eq!(
        page(&session, "d3"),
        (vec!["AA".into(), "B".into(), "same".into()], true)
    );
    assert_eq!(
        page(&session, "d4"),
        (
            vec!["AA".into(), "B".into(), "same".into(), "same".into()],
            false
        ),
        "equal item values from distinct primary keys preserve multiplicity"
    );

    // Replacing a tuple in place keeps the child's declared key identity. Its
    // updated payload appears at the same typed-key position for equal sorts.
    session = apply_and_reopen(
        session,
        &dir,
        "replace payload without changing the key",
        vec![row_upsert("c", "P", 0, "C-replaced")],
    );
    assert_eq!(
        page(&session, "d4"),
        (
            vec!["AA".into(), "B".into(), "C-replaced".into(), "same".into()],
            false
        )
    );

    // Retracting outside the current prefix does not perturb its contents.
    session = apply_and_reopen(
        session,
        &dir,
        "retract outside prefix",
        vec![row_remove("z")],
    );
    assert_eq!(page(&session, "d2"), (vec!["AA".into(), "B".into()], true));

    // Removing the first typed key promotes the next child into the prefix.
    session = apply_and_reopen(
        session,
        &dir,
        "retract inside prefix",
        vec![row_remove("aa")],
    );
    assert_eq!(
        page(&session, "d2"),
        (vec!["B".into(), "C-replaced".into()], false)
    );

    session = apply_and_reopen(
        session,
        &dir,
        "retract promoted child",
        vec![row_remove("b")],
    );
    assert_eq!(page(&session, "d2"), (vec!["C-replaced".into()], false));

    // Removing the last member removes both the group result and its decisions.
    session = apply_and_reopen(session, &dir, "retract final child", vec![row_remove("c")]);
    let settlements = session.all_settlements().unwrap();
    for decide in ["d0", "d1", "d2", "d3", "d4"] {
        assert!(
            settlements
                .get(&format!("root::{decide}"))
                .is_none_or(|rows| rows.is_empty()),
            "final child removal must remove {decide}'s group decision"
        );
    }

    // A different insertion order reaches the same page contents and oracle
    // state for the same set of keyed child tuples.
    let permutation_dir = TestDir::new("insertion-permutation");
    let mut permutation =
        WorldSession::from_program_with_sources(&permutation_dir.0, "root", &sources)
            .expect("create insertion permutation world");
    permutation = apply_and_reopen(
        permutation,
        &permutation_dir,
        "seed the same rows in reverse order",
        vec![
            row_upsert("z", "P", 0, "same"),
            row_upsert("c", "P", 0, "C-replaced"),
            row_upsert("b", "P", 0, "B"),
            row_upsert("aa", "P", 0, "AA"),
        ],
    );
    assert_eq!(page(&permutation, "d0"), (vec![], true));
    assert_eq!(page(&permutation, "d1"), (vec!["AA".into()], true));
    assert_eq!(
        page(&permutation, "d2"),
        (vec!["AA".into(), "B".into()], true)
    );
    assert_eq!(
        page(&permutation, "d3"),
        (vec!["AA".into(), "B".into(), "C-replaced".into()], true)
    );
    assert_eq!(
        page(&permutation, "d4"),
        (
            vec!["AA".into(), "B".into(), "C-replaced".into(), "same".into()],
            false
        )
    );
}

#[test]
fn typed_collect_take_state_root_is_independent_of_internal_chunking() {
    let run = |label: &str, chunk_size: Option<usize>| {
        let dir = TestDir::new(label);
        let sources = BTreeMap::from([("root".into(), BASIC_PROGRAM.into())]);
        let mut session =
            WorldSession::from_program_with_sources(&dir.0, "root", &sources).unwrap();
        if let Some(size) = chunk_size {
            session.chunk_policy = Arc::new(FixedChunkPolicy(size));
        }
        let ops = (0..32)
            .map(|i| row_upsert(&format!("chunk-{i:02}"), "P", i % 4, &format!("value-{i}")))
            .collect();
        session
            .apply_batch(WorldBatch::new(
                session.current_revision(),
                "same typed collect take batch",
                ops,
            ))
            .unwrap();
        let chunks = session.last_commit_profile.as_ref().unwrap().chunks;
        if chunk_size.is_some() {
            assert!(
                chunks > 1,
                "fixed chunk policy should exercise chunked apply"
            );
        } else {
            assert_eq!(chunks, 1, "unbounded apply should use one chunk");
        }
        let root = session.network_state_root;
        session.verify_oracle().unwrap();
        drop(session);
        let reopened = WorldSession::open(&dir.0).unwrap();
        assert_eq!(reopened.network_state_root, root);
        reopened.verify_oracle().unwrap();
        root
    };

    assert_eq!(
        run("typed-whole", None),
        run("typed-chunked", Some(7)),
        "internal chunk boundaries must not alter canonical typed collect state"
    );
}

#[test]
fn nested_grouped_collect_take_resolves_qualified_child_identity() {
    let program = r#"
config Page = Page(List<Int>, Bool)
rel input rows: { id: Str, parent: Str, group: Str, value: Str } key id

rel derived child_groups = select {
    parent: r.parent, group: r.group, total: count()
} from r in rows group by r.parent, r.group

rel derived parent_pages = select {
    parent: g.parent, page: collect(g.total, g.group).take(2)
} from g in child_groups group by g.parent

decide page_decision for p in parent_pages per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
"#;
    let dir = TestDir::new("nested-groups");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session = WorldSession::from_program_with_sources(&dir.0, "root", &sources)
        .expect("create nested grouped world");
    session = apply_and_reopen(
        session,
        &dir,
        "seed nested groups",
        vec![
            row_with_group("a1", "P", "A", "one"),
            row_with_group("a2", "P", "A", "two"),
            row_with_group("b1", "P", "B", "three"),
            row_with_group("c1", "P", "C", "four"),
        ],
    );
    let value = &session.all_settlements().unwrap()["root::page_decision"]["P"].value;
    let Value::Variant { args, .. } = value else {
        panic!("expected Page variant, got {value:?}");
    };
    let [Value::List(items), Value::Bool(truncated)] = args.as_slice() else {
        panic!("expected Page(List<Int>, Bool), got {args:?}");
    };
    assert_eq!(items, &[Value::Int(2), Value::Int(1)]);
    assert!(*truncated);
}

#[test]
fn decision_source_collect_take_uses_the_full_composite_per_key() {
    let program = r#"
config Page = Page(List<Str>, Bool)
rel input rows: { tenant: Str, id: Str, parent: Str, sort: Int, value: Str } key id

decide winner for r in rows per (tenant, id) {
    propose keep priority 1 when true = r.value
}

rel derived tenant_page = select {
    tenant: w.tenant, page: collect(w.value, 0).take(3)
} from w in winner group by w.tenant

decide page_decision for p in tenant_page per tenant {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
"#;
    let dir = TestDir::new("decision-composite-key");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session = WorldSession::from_program_with_sources(&dir.0, "root", &sources)
        .expect("create composite decision-key world");
    session = apply_and_reopen(
        session,
        &dir,
        "seed same-first-component decision entities",
        vec![
            row_with_tenant("z", "T", "M"),
            row_with_tenant("b", "T", "A"),
            row_with_tenant("aa", "T", "Z"),
        ],
    );
    assert_eq!(
        page_for(&session, "page_decision", "T"),
        (vec!["Z".into(), "A".into(), "M".into()], false),
        "decision row identity must use ordered (tenant, id) fields, not candidate/priority/value"
    );
}

#[test]
fn anonymous_collect_take_record_survives_decision_row_replay() {
    let program = r#"
rel input rows: { id: Str, parent: Str, sort: Int, value: Str } key id
rel derived grouped = select {
    parent: r.parent, result: collect(r.value, r.sort).take(2)
} from r in rows group by r.parent
decide d for p in grouped per parent {
    propose page priority 1 when true = p.result
}
"#;
    let dir = TestDir::new("anonymous-record");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session = WorldSession::from_program_with_sources(&dir.0, "root", &sources)
        .expect("create anonymous record world");
    session = apply_and_reopen(
        session,
        &dir,
        "settle anonymous collect record",
        vec![row_upsert("a", "P", 0, "A"), row_upsert("b", "P", 1, "B")],
    );
    let value = &session.all_settlements().unwrap()["root::d"]["P"].value;
    let Value::Record { nominal, fields } = value else {
        panic!("expected anonymous record decision value, got {value:?}");
    };
    assert!(nominal.is_empty(), "record should remain anonymous");
    assert_eq!(
        fields["items"],
        Value::List(vec![Value::Str("A".into()), Value::Str("B".into())])
    );
    assert_eq!(fields["truncated"], Value::Bool(false));
}

fn row_with_group(id: &str, parent: &str, group: &str, value: &str) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("parent", parent);
    row.set_str("group", group);
    row.set_str("value", value);
    WorldBatchOp::Upsert {
        relation: "root::rows".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

fn row_with_tenant(id: &str, tenant: &str, value: &str) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("tenant", tenant);
    row.set_str("id", id);
    row.set_str("parent", tenant);
    row.set_str("sort", "0");
    row.set_str("value", value);
    WorldBatchOp::Upsert {
        relation: "root::rows".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

#[test]
fn take_stable_identity_composes_across_composite_keys_joins_and_distinct_union() {
    let program = r#"
config Page = Page(List<Str>, Bool)
rel input parents: { tenant: Str, parent: Str } key (tenant, parent)
rel input left: { tenant: Str, id: Str, parent: Str, sort: Int, value: Str } key (tenant, id)
rel input right: { tenant: Str, id: Str, parent: Str, sort: Int, value: Str } key (tenant, id)

rel derived joined = select {
    tenant: p.tenant, parent: p.parent, id: c.id, sort: c.sort, value: c.value
} from p in parents, c in left where p.tenant == c.tenant and p.parent == c.parent

rel derived joined_page = select {
    parent: j.parent, page: collect(j.value, j.sort).take(4)
} from j in joined group by j.parent

rel derived all_rows = union distinct left, right
rel derived union_page = select {
    parent: u.parent, page: collect(u.value, u.sort).take(4)
} from u in all_rows group by u.parent

decide joined_decision for p in joined_page per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
decide union_decision for p in union_page per parent {
    propose page priority 1 when true = Page(p.page.items, p.page.truncated)
}
"#;
    let dir = TestDir::new("composite-join-union");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session = WorldSession::from_program_with_sources(&dir.0, "root", &sources)
        .expect("create joined/union world");
    let op = |relation: &str, tenant: &str, id: &str, parent: &str, value: &str| {
        let mut row = TupleRecord::new();
        row.set_str("tenant", tenant);
        row.set_str("id", id);
        row.set_str("parent", parent);
        row.set_str("sort", "0");
        row.set_str("value", value);
        WorldBatchOp::Upsert {
            relation: relation.into(),
            key: WorldKey::from_str(&format!("{tenant}/{id}")),
            tuple: row.to_tuple(),
        }
    };
    session = apply_and_reopen(
        session,
        &dir,
        "seed composite keys and duplicate distinct supports",
        vec![
            {
                let mut row = TupleRecord::new();
                row.set_str("tenant", "t");
                row.set_str("parent", "P");
                WorldBatchOp::Upsert {
                    relation: "root::parents".into(),
                    key: WorldKey::from_str("t/P"),
                    tuple: row.to_tuple(),
                }
            },
            op("root::left", "t", "b", "P", "B"),
            op("root::left", "t", "z", "P", "Z"),
            op("root::right", "t", "aa", "P", "AA"),
            op("root::right", "t", "c", "P", "C"),
        ],
    );
    assert_eq!(
        page(&session, "joined_decision"),
        (vec!["B".into(), "Z".into()], false)
    );
    assert_eq!(
        page(&session, "union_decision"),
        (vec!["AA".into(), "B".into(), "C".into(), "Z".into()], false),
        "typed tuple keys precede qualified source-relation tie breakers across union inputs"
    );
}

#[test]
fn legacy_full_collect_overflow_remains_atomic() {
    let program = r#"
rel input rows: { id: Str, parent: Str, sort: Int, value: Str } key id
rel derived all = select {
    parent: r.parent, items: collect(r.value, r.sort, 2)
} from r in rows group by r.parent
"#;
    let dir = TestDir::new("legacy-overflow");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create world");
    session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "seed full legacy collect",
            vec![row_upsert("a", "P", 0, "A"), row_upsert("b", "P", 1, "B")],
        ))
        .unwrap();
    let before_digest = session.current_revision_digest().unwrap();
    let before_revision = session.current_revision();
    let error = session
        .apply_batch(WorldBatch::new(
            before_revision,
            "overflow after retraction legacy full collect",
            vec![
                row_remove("a"),
                row_upsert("c", "P", 2, "C"),
                row_upsert("d", "P", 3, "D"),
            ],
        ))
        .unwrap_err();
    assert!(matches!(error, WorldError::NetworkError(_)), "{error:?}");
    assert_eq!(session.current_revision(), before_revision);
    assert_eq!(session.current_revision_digest().unwrap(), before_digest);
    session
        .verify_oracle()
        .expect("failed batch leaves oracle state unchanged");
}

#[test]
fn reference_legacy_and_typed_profiles_use_their_recorded_ordering_modes() {
    let source = r#"
config Page = Page(List<Str>, Bool)
rel input rows: { id: Str, parent: Str, sort: Int, value: Str } key id
rel derived page = select {
    parent: r.parent, result: collect(r.value, r.sort, 2)
} from r in rows group by r.parent
decide d for p in page per parent {
    propose page priority 1 when true = Page(p.result, false)
}
"#;
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    let linked = brix_lower::module_graph::ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        brix_lower::module_graph::ModuleLoaderLimits::default(),
    )
    .unwrap()
    .link()
    .unwrap();
    let mut rows = BTreeMap::new();
    for (id, value) in [("aa", "AA"), ("b", "B")] {
        let mut tuple = TupleRecord::new();
        tuple.set_str("id", id);
        tuple.set_str("parent", "P");
        tuple.set_str("sort", "0");
        tuple.set_str("value", value);
        rows.insert(WorldKey::from_str(id), tuple.to_tuple());
    }
    let bases = BTreeMap::from([("root::rows".to_string(), rows)]);
    let evaluate_mode = |mode| {
        let program = reference::from_program_with_collect_ordering(&linked, mode).unwrap();
        reference::evaluate(&program, &bases).unwrap().settlements["root::d"]["P"]
            .value
            .clone()
    };
    assert_eq!(
        evaluate_mode(CollectOrdering::LegacyV1),
        ReferenceValue::Variant {
            nominal: "root::Page".into(),
            variant: "Page".into(),
            args: vec![
                ReferenceValue::List(vec![
                    ReferenceValue::Str("B".into()),
                    ReferenceValue::Str("AA".into())
                ]),
                ReferenceValue::Bool(false),
            ],
        },
        "legacy mode keeps the historical opaque-key tie break"
    );
    assert_eq!(
        evaluate_mode(CollectOrdering::TypedKeyV1),
        ReferenceValue::Variant {
            nominal: "root::Page".into(),
            variant: "Page".into(),
            args: vec![
                ReferenceValue::List(vec![
                    ReferenceValue::Str("AA".into()),
                    ReferenceValue::Str("B".into())
                ]),
                ReferenceValue::Bool(false),
            ],
        },
        "typed mode orders equal sort values by natural declared string key"
    );
}

#[test]
fn reference_legacy_profile_rejects_collect_take_syntax() {
    let source = r#"
rel input rows: { id: Str, parent: Str, value: Str } key id
rel derived page = select {
    parent: r.parent, result: collect(r.value).take(2)
} from r in rows group by r.parent
"#;
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    let linked = brix_lower::module_graph::ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        brix_lower::module_graph::ModuleLoaderLimits::default(),
    )
    .unwrap()
    .link()
    .unwrap();
    let error = reference::from_program_with_collect_ordering(&linked, CollectOrdering::LegacyV1)
        .expect_err("historical profile cannot admit post-profile syntax");
    assert!(error
        .to_string()
        .contains("does not support collect(...).take()"));
    reference::from_program_with_collect_ordering(&linked, CollectOrdering::TypedKeyV1)
        .expect("typed profile supports collect-take syntax");
}
