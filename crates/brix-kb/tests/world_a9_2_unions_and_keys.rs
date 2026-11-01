//! A9-2 Qualification Test Suite:
//! - Union schemas and support (plain union and union distinct)
//! - Multi-source union distinct, incremental support removal, same-id distinct rows, and downstream key conflict
//! - Select, group, and join consuming union schemas
//! - Composite decision keys `per (tenant, id)`: collision resistance, cross-tenant ID coexistence,
//!   history, explanation, and portable decision evidence round-trip
//! - Nested field path indexed equijoins (`f.value.metric.score == b.target_score`), winner replacement,
//!   nested updates, cold restore, oracle verification, and constant operator work under unrelated row growth.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn composite_key_json_preserves_numeric_type_tags() {
    use brix_kb::world::decision_codec::{
        encode_entity_id, entity_id_to_json_key_with_shape, Value,
    };
    let entity = encode_entity_id(&[
        Value::F64("0.5".parse().unwrap()),
        Value::Decimal(brix_canon::decimal_parse("0.50").unwrap()),
        Value::Str("0.5".into()),
    ]);
    assert_eq!(
        entity_id_to_json_key_with_shape(&entity, true),
        serde_json::json!([
            {"type": "f64", "value": "0.5"},
            {"type": "decimal", "value": "0.5"},
            "0.5"
        ])
    );
}

use brix_kb::world::decision_evidence::verify_decision_evidence;
use brix_kb::world::network::WorldNetwork;
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
use brix_lower::evidence::Authority;
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "brix-a9-2-{label}-{}-{}",
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

// ---------------------------------------------------------------------------
// 1. Union distinct semantics across 3 sources, incremental support, and key conflict
// ---------------------------------------------------------------------------

#[test]
fn test_union_distinct_three_sources_incremental_support_and_key_conflict() {
    let program = r#"
rel input catalog_a: { id: Str, sku: Str, price: Int } key id
rel input catalog_b: { id: Str, sku: Str, price: Int } key id
rel input catalog_c: { id: Str, sku: Str, price: Int } key id
rel input catalog_empty: { id: Str, sku: Str, price: Int } key id

rel derived catalog = union distinct catalog_a, catalog_b, catalog_c

// Plain union enforces key uniqueness: conflicting rows for the same key cause rejection
rel derived keyed_catalog = union catalog, catalog_empty key id
"#;

    let dir = TestDir::new("union-distinct-lifecycle");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let cat_op = |rel: &str, id: &str, sku: &str, price: i64| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        r.set_str("sku", sku);
        r.set_str("price", &price.to_string());
        WorldBatchOp::Upsert {
            relation: format!("root::{rel}"),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };
    let cat_del = |rel: &str, id: &str| WorldBatchOp::Remove {
        relation: format!("root::{rel}"),
        key: WorldKey::from_str(id),
    };

    // 1. Insert identical row (id="1", sku="SKU-1", price=100) across all 3 sources
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed-3-sources",
            vec![
                cat_op("catalog_a", "1", "SKU-1", 100),
                cat_op("catalog_b", "1", "SKU-1", 100),
                cat_op("catalog_c", "1", "SKU-1", 100),
            ],
        ))
        .expect("batch 1");

    let rows = session.get_derived("root::catalog").unwrap().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "identical rows across 3 sources deduplicate to 1"
    );
    let keyed = session.get_derived("root::keyed_catalog").unwrap().unwrap();
    assert_eq!(keyed.len(), 1);

    // 2. Retract support from catalog_a (3 -> 2 supports): output row remains
    session
        .apply_batch(WorldBatch::new(
            1,
            "retract-a",
            vec![cat_del("catalog_a", "1")],
        ))
        .expect("batch 2");
    let rows = session.get_derived("root::catalog").unwrap().unwrap();
    assert_eq!(rows.len(), 1, "row remains with 2 supports");

    // 3. Retract support from catalog_b (2 -> 1 support): output row remains
    session
        .apply_batch(WorldBatch::new(
            2,
            "retract-b",
            vec![cat_del("catalog_b", "1")],
        ))
        .expect("batch 3");
    let rows = session.get_derived("root::catalog").unwrap().unwrap();
    assert_eq!(rows.len(), 1, "row remains with 1 support");

    // 4. Retract final support from catalog_c (1 -> 0 support): output row is retracted
    session
        .apply_batch(WorldBatch::new(
            3,
            "retract-c",
            vec![cat_del("catalog_c", "1")],
        ))
        .expect("batch 4");
    let rows = session.get_derived("root::catalog").unwrap().unwrap();
    assert_eq!(rows.len(), 0, "withdrawing final support retracts output");

    // 5. Two rows with the SAME id but DIFFERENT values remain distinct in union distinct,
    // but the downstream plain union `keyed_catalog` rejects conflicting values for the same key
    session
        .apply_batch(WorldBatch::new(
            4,
            "different-rows-same-id",
            vec![
                cat_op("catalog_a", "2", "SKU-A", 100),
                cat_op("catalog_b", "2", "SKU-B", 200),
            ],
        ))
        .expect_err(
            "keyed_catalog downstream key constraint must reject conflicting values for id='2'",
        );

    drop(session);

    // Verify store and cold restore
    brix_kb::world::gc::verify_store(&dir.0).expect("verify store");
    let cold = WorldSession::open(&dir.0).expect("cold open");
    cold.verify_state().expect("verify state");
    cold.verify_oracle().expect("verify oracle");
    assert_eq!(cold.current_revision(), 4);
}

// ---------------------------------------------------------------------------
// 2. Select, group, and join consuming union schemas (including derived unions)
// ---------------------------------------------------------------------------

#[test]
fn test_select_group_and_join_consuming_union_schemas() {
    let program = r#"
config Tag = Active | Suspended

rel input local_users: { id: Str, team: Str, status: Tag } key id
rel input partner_users: { id: Str, team: Str, status: Tag } key id
rel input teams: { team_id: Str, budget: Int } key team_id

rel derived all_users = union distinct local_users, partner_users

rel derived team_member_counts = select {
    team: u.team,
    members: count()
} from u in all_users group by u.team

rel derived funded_teams = select {
    team: t.team_id,
    budget: t.budget,
    members: c.members
} from t in teams, c in team_member_counts
where t.team_id == c.team
"#;

    let dir = TestDir::new("union-consumption");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let user_op = |rel: &str, id: &str, team: &str, variant: &str| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        r.set_str("team", team);
        r.set_str(
            "status",
            &format!(r#"{{"type":"sum","nominal":"root::Tag","variant":"{variant}","args":[]}}"#),
        );
        WorldBatchOp::Upsert {
            relation: format!("root::{rel}"),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };
    let team_op = |team: &str, budget: i64| {
        let mut r = TupleRecord::new();
        r.set_str("team_id", team);
        r.set_str("budget", &budget.to_string());
        WorldBatchOp::Upsert {
            relation: "root::teams".into(),
            key: WorldKey::from_str(team),
            tuple: r.to_tuple(),
        }
    };

    session
        .apply_batch(WorldBatch::new(
            0,
            "seed-users-and-teams",
            vec![
                user_op("local_users", "u1", "eng", "Active"),
                user_op("local_users", "u2", "eng", "Active"),
                user_op("partner_users", "u3", "eng", "Suspended"),
                user_op("partner_users", "u4", "design", "Active"),
                team_op("eng", 500_000),
                team_op("design", 200_000),
            ],
        ))
        .expect("batch seed");

    let counts = session
        .get_derived("root::team_member_counts")
        .unwrap()
        .unwrap();
    assert_eq!(counts.len(), 2);
    let funded = session.get_derived("root::funded_teams").unwrap().unwrap();
    assert_eq!(funded.len(), 2);

    drop(session);
    let cold = WorldSession::open(&dir.0).expect("cold open");
    cold.verify_state().expect("verify state");
    cold.verify_oracle().expect("verify oracle");
}

#[test]
fn test_json_looking_scalar_identity_uses_string_shape_through_upstream_explanation() {
    let program = r#"
rel input requests: { id: Str } key id
decide first for r in requests per id {
    propose yes priority 1 when true = 1
}
rel derived accepted = select { id: a.id, value: a.value } from a in first
decide second for a in accepted per id {
    propose forward priority 1 when true = a.value
}
"#;
    let dir = TestDir::new("json-looking-scalar-presentation");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");
    let literal = "[\"a\", \"b\"]";
    let mut request = TupleRecord::new();
    request.set_str("id", literal);
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed-json-looking-scalar",
            vec![WorldBatchOp::Upsert {
                relation: "root::requests".into(),
                key: WorldKey::from_str(literal),
                tuple: request.to_tuple(),
            }],
        ))
        .expect("seed");

    let settlements = session.all_settlements().expect("settlements");
    let settled = &settlements["root::first"][literal];
    assert_eq!(settled.to_json()["entity_key"], literal);
    assert!(!settled.is_composite);
    assert_eq!(
        format!("{settled:?}"),
        format!(
            "SettledDecision {{ entity_id: {literal:?}, candidate_name: \"yes\", priority: 1, phase: 0, value: Int(1), calendar_key: {:?} }}",
            settled.calendar_key
        ),
        "presentation metadata must stay out of the frozen Debug layout"
    );

    let target = session
        .decision_target(1, "root::first", literal, Vec::new())
        .expect("typed target");
    assert_eq!(target.to_json()["entity_key"], literal);
    let evidence = session
        .request_decision_evidence(1, "root::first", literal, Vec::new(), true)
        .expect("decision evidence");
    verify_decision_evidence(&evidence, &target).expect("verify scalar string target");
    let mut presentation_changed = target.clone();
    presentation_changed.is_composite = true;
    assert_eq!(target.id(), presentation_changed.id());
    verify_decision_evidence(&evidence, &presentation_changed)
        .expect("presentation metadata must not affect claim authority");

    let explanation = session
        .explain_decision_for("root::second", literal)
        .expect("explain")
        .expect("downstream explanation");
    assert!(!explanation.is_composite);
    let explanation_json = explanation.to_json();
    assert_eq!(explanation_json["entity_key"], literal);
    let links = explanation_json["upstream_settlements"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["entity_id"], literal);
    assert_eq!(links[0]["entity_key"], literal);
}

// ---------------------------------------------------------------------------
// 3. Composite decision keys per (tenant, id): collision resistance, explanation, evidence
// ---------------------------------------------------------------------------

#[test]
fn test_composite_decision_keys_and_evidence_round_trip() {
    let program = r#"
config Decision = Allowed(Int) | Denied

rel input limits: { tenant: Str, id: Str, amount: Int } key id
rel input requests: { tenant: Str, id: Str, requested: Int } key id

rel derived pending = select {
    tenant: r.tenant,
    id: r.id,
    requested: r.requested,
    limit: l.amount
} from r in requests, l in limits
where r.tenant == l.tenant && r.id == l.id

decide grant for p in pending per (tenant, id) {
    propose allow priority 1 when p.requested <= p.limit = Decision::Allowed(p.requested)
    propose deny priority 2 when true = Decision::Denied
}

rel derived settled_grants = select {
    tenant: g.tenant,
    id: g.id,
    cand: g.candidate
} from g in grant
"#;

    let dir = TestDir::new("composite-keys-evidence");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let req_op = |tenant: &str, id: &str, requested: i64| {
        let mut r = TupleRecord::new();
        r.set_str("tenant", tenant);
        r.set_str("id", id);
        r.set_str("requested", &requested.to_string());
        WorldBatchOp::Upsert {
            relation: "root::requests".into(),
            key: WorldKey::from_str(&format!("{tenant}::{id}")),
            tuple: r.to_tuple(),
        }
    };
    let lim_op = |tenant: &str, id: &str, amount: i64| {
        let mut r = TupleRecord::new();
        r.set_str("tenant", tenant);
        r.set_str("id", id);
        r.set_str("amount", &amount.to_string());
        WorldBatchOp::Upsert {
            relation: "root::limits".into(),
            key: WorldKey::from_str(&format!("{tenant}::{id}")),
            tuple: r.to_tuple(),
        }
    };

    // 1. Insert collision test cases: ("a", "bc") vs ("ab", "c") and equal local IDs across tenants ("t1", "item1") vs ("t2", "item1")
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed-composite",
            vec![
                req_op("a", "bc", 50),
                lim_op("a", "bc", 100),
                req_op("ab", "c", 150),
                lim_op("ab", "c", 100),
                req_op("t1", "item1", 20),
                lim_op("t1", "item1", 50),
                req_op("t2", "item1", 80),
                lim_op("t2", "item1", 30),
            ],
        ))
        .expect("batch seed");

    let settlements = session.all_settlements().expect("all settlements");
    let grant_settlements = &settlements["root::grant"];
    assert_eq!(grant_settlements.len(), 4);

    // ("a", "bc") is Allowed(50)
    let s_a_bc = &grant_settlements["[\"a\",\"bc\"]"];
    assert_eq!(s_a_bc.candidate_name, "allow");
    // ("ab", "c") is Denied
    let s_ab_c = &grant_settlements["[\"ab\",\"c\"]"];
    assert_eq!(s_ab_c.candidate_name, "deny");
    // ("t1", "item1") is Allowed(20)
    let s_t1 = &grant_settlements["[\"t1\",\"item1\"]"];
    assert_eq!(s_t1.candidate_name, "allow");
    // ("t2", "item1") is Denied
    let s_t2 = &grant_settlements["[\"t2\",\"item1\"]"];
    assert_eq!(s_t2.candidate_name, "deny");

    // Explanation test for composite entity
    let expl = session
        .explain_decision_for("root::grant", "[\"a\",\"bc\"]")
        .expect("explanation query")
        .expect("explanation for ('a', 'bc')");
    assert_eq!(expl.winning_candidate.as_deref(), Some("allow"));
    let json_expl = expl.to_json();
    assert_eq!(json_expl["entity_key"], serde_json::json!(["a", "bc"]));

    // Portable evidence request & verification
    let evidence = session
        .request_decision_evidence(1, "root::grant", "[\"a\",\"bc\"]", vec![], true)
        .expect("request evidence for composite key");
    assert_eq!(evidence.target.candidate, "allow");
    let target_json = evidence.target.to_json();
    assert_eq!(target_json["entity_key"], serde_json::json!(["a", "bc"]));

    let verified = verify_decision_evidence(&evidence, &evidence.target)
        .expect("independent replay and verification of composite key decision");
    assert_eq!(verified.authority, Authority::RuntimeCheck);
    assert_eq!(verified.subject, "[\"a\",\"bc\"]");

    // Cold restore & oracle verification
    drop(session);
    brix_kb::world::gc::verify_store(&dir.0).expect("verify store");
    let cold = WorldSession::open(&dir.0).expect("cold open");
    cold.verify_state().expect("verify state");
    cold.verify_oracle().expect("verify oracle");
}

// ---------------------------------------------------------------------------
// 4. Nested field path indexed equijoins with work isolation assertions
// ---------------------------------------------------------------------------

#[test]
fn test_nested_field_path_indexed_join_and_work_isolation() {
    let program = r#"
config Metric = { score: Int, tag: Str }
config Wrapper = { metric: Metric, status: Str }

rel input profiles: { id: Str, prio: Int } key id
rel input benchmarks: { id: Str, target_score: Int } key id

decide evaluate_profile for p in profiles per id {
    propose high priority 1 when p.prio >= 10 = Wrapper { metric: Metric { score: 100, tag: "high" }, status: "active" }
    propose low priority 2 when true = Wrapper { metric: Metric { score: 50, tag: "low" }, status: "active" }
}

// Equijoin on nested field path f.value.metric.score == b.target_score
rel derived matches = select {
    profile: p.id,
    benchmark: b.id
} from p in evaluate_profile, b in benchmarks
where p.value.metric.score == b.target_score
"#;

    let dir = TestDir::new("nested-join-work");
    let sources = BTreeMap::from([("root".into(), program.into())]);

    let build_network = || -> WorldNetwork {
        let loader = |name: &str| sources.get(name).cloned();
        let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
            .unwrap()
            .link()
            .unwrap();
        WorldNetwork::from_program(&linked).unwrap()
    };

    let prof_op = |id: &str, prio: i64| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        r.set_str("prio", &prio.to_string());
        WorldBatchOp::Upsert {
            relation: "root::profiles".into(),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };
    let bench_op = |id: &str, target: i64| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        r.set_str("target_score", &target.to_string());
        WorldBatchOp::Upsert {
            relation: "root::benchmarks".into(),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };

    // Helper to measure update work with N unrelated resident rows using WorldNetwork directly
    let measure_work = |unrelated_count: usize| -> u64 {
        let mut net = build_network();
        let mut init_ops = vec![
            prof_op("p_active", 5), // score = 50
            bench_op("b_active", 50),
        ];
        for i in 0..unrelated_count {
            init_ops.push(prof_op(&format!("p_unrelated_{i}"), 1)); // score = 50
            init_ops.push(bench_op(&format!("b_unrelated_{i}"), 9999)); // target = 9999 (no match)
        }
        net.apply_ops(&init_ops).unwrap();

        // Update the active profile so score changes from 50 -> 100
        let report = net.apply_ops(&[prof_op("p_active", 15)]).unwrap();
        let work: u64 = report.diagnostics.operator_work.values().sum();
        work
    };

    let small_work = measure_work(0);
    let large_work = measure_work(200);
    assert_eq!(
        small_work, large_work,
        "nested field path join must use maintained index and not scan unrelated rows"
    );

    // Initial batch in main session
    let mut session = WorldSession::from_program_with_sources(&dir.0, "root", &sources).unwrap();
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed",
            vec![
                prof_op("p1", 15), // Wrapper(metric: score: 100)
                bench_op("b1", 100),
                bench_op("b2", 50),
            ],
        ))
        .unwrap();

    let matches = session.get_derived("root::matches").unwrap().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].get_str("profile").unwrap(), "p1");
    assert_eq!(matches[0].get_str("benchmark").unwrap(), "b1");

    // Winner replacement: update p1 priority to 5 -> Wrapper(metric: score: 50)
    session
        .apply_batch(WorldBatch::new(1, "replace-winner", vec![prof_op("p1", 5)]))
        .unwrap();

    let matches = session.get_derived("root::matches").unwrap().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].get_str("profile").unwrap(), "p1");
    assert_eq!(
        matches[0].get_str("benchmark").unwrap(),
        "b2",
        "join incrementally updated to target_score=50"
    );

    // Retraction of p1
    session
        .apply_batch(WorldBatch::new(
            2,
            "retract-p1",
            vec![WorldBatchOp::Remove {
                relation: "root::profiles".into(),
                key: WorldKey::from_str("p1"),
            }],
        ))
        .unwrap();

    let matches = session.get_derived("root::matches").unwrap().unwrap();
    assert_eq!(matches.len(), 0, "retracting profile retracts joined match");

    drop(session);
    brix_kb::world::gc::verify_store(&dir.0).expect("verify store");
    let cold = WorldSession::open(&dir.0).expect("cold open");
    cold.verify_state().expect("verify state");
    cold.verify_oracle().expect("verify oracle");
}
