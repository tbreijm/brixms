//! R6: ordered, bounded list values across settlement and durable world boundaries.
use brix_kb::world::audit::{
    build_genesis_bundle_from_session, decode_world_audit_bundle_v1, ExecProfileV1,
    ProgramClosureV1, WorldAuditDecodeLimits,
};
use brix_kb::world::decision_codec::{decision_tuple, decode_settled_decision, Value};
use brix_kb::world::state_codec::StateCodec;
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyOptions};
use brix_kb::world::{OpenMode, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
use std::{collections::BTreeMap, path::PathBuf};

struct Dir(PathBuf);
impl Dir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "brix-list-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn create(dir: &Dir, source: &str) -> WorldSession {
    WorldSession::from_program_with_sources(
        &dir.0,
        "root",
        &BTreeMap::from([("root".into(), source.into())]),
    )
    .unwrap()
}
fn row(id: &str, n: i64) -> WorldBatchOp {
    let mut r = TupleRecord::new();
    r.set_str("id", id);
    r.set_str("n", &n.to_string());
    WorldBatchOp::Upsert {
        relation: "root::orders".into(),
        key: WorldKey::from_str(id),
        tuple: r.to_tuple(),
    }
}
fn audit(world: &WorldSession, source: &str) {
    let closure = ProgramClosureV1 {
        root_module: "root".into(),
        sources: vec![("root".into(), source.into())],
        program_manifest_digest: world.manifest().program_digest,
    };
    let bundle =
        build_genesis_bundle_from_session(world, closure, ExecProfileV1::default()).unwrap();
    let limits = WorldAuditDecodeLimits::default();
    let bytes = bundle.encode(&limits).unwrap();
    let decoded = decode_world_audit_bundle_v1(&bytes, &limits).unwrap();
    verify_world_audit_bundle(
        &decoded,
        &VerifyOptions {
            expect_head: world.current_revision_digest().unwrap(),
            expect_program: world.manifest().program_digest,
            trust_checkpoint: None,
            limits,
            max_work: None,
        },
    )
    .unwrap();
}

#[test]
fn unused_list_config_and_list_variant_roundtrip() {
    let source = r#"
config Rev = { score: Int }
config Rec = { reviews: List<Rev> }
config Out = Ok(List<Int>) | No
rel input orders: { id: Str, n: Int } key id
decide d for o in orders per id {
 propose ok priority 1 when o.n > 0 = Ok([o.n])
 propose no priority 2 when true = No
}
rel derived settled = select { id: d.id, value: d.value } from d in d
"#;
    let dir = Dir::new("variant");
    let mut world = create(&dir, source);
    world
        .apply_batch(WorldBatch::new(
            0,
            "first",
            vec![row("positive", 5), row("zero", 0)],
        ))
        .unwrap();
    let expected = world.all_settlements().unwrap();
    assert_eq!(
        expected["root::d"]["positive"].value,
        Value::Variant {
            nominal: "root::Out".into(),
            variant: "Ok".into(),
            args: vec![Value::List(vec![Value::Int(5)])]
        }
    );
    assert_eq!(
        expected["root::d"]["zero"].value,
        Value::Variant {
            nominal: "root::Out".into(),
            variant: "No".into(),
            args: vec![]
        }
    );
    for (id, settled) in &expected["root::d"] {
        assert_eq!(
            decode_settled_decision(id, decision_tuple(settled).as_bytes()).unwrap(),
            *settled
        );
        assert!(world.explain_decision_for("root::d", id).unwrap().is_some());
    }
    world.verify_oracle().unwrap();
    world.verify_state().unwrap();
    audit(&world, source);
    drop(world);
    let mut restored = WorldSession::open(&dir.0).unwrap();
    assert_eq!(restored.open_mode, OpenMode::Restored);
    assert_eq!(restored.all_settlements().unwrap(), expected);
    restored.verify_oracle().unwrap();
    restored.verify_state().unwrap();
    restored
        .apply_batch(WorldBatch::new(1, "next", vec![row("positive", 7)]))
        .unwrap();
    assert!(restored
        .historical_explain_decision_for(1, "root::d", "positive")
        .unwrap()
        .is_some());
    audit(&restored, source);
}

#[test]
fn ordered_list_helpers_records_variants_and_direct_payloads() {
    let source = r#"
config Item = { score: Int }
config Caveat = Missing(Str) | Fine
config Review = { reviews: List<Item>, caveats: List<Caveat> }
fn selected(xs: List<Int>): List<Int> = distinct(map(filter(xs, x => x > 0), x => x * 2))
fn reviews(n: Int): Review = Review { reviews: [Item { score: n }, Item { score: n }], caveats: [Fine, Missing("x")] }
rel input orders: { id: Str, n: Int } key id
decide d for o in orders per id { propose p priority 1 when true = selected([o.n, 1, o.n, 2, 0]) }
decide r for o in orders per id { propose p priority 1 when len([o.n]) == 1 && all([1, 2], x => x > 0) && any([0, 2], x => x > 0) = reviews(o.n) }
rel derived settled = select { id: d.id, value: d.value } from d in d
"#;
    let dir = Dir::new("helpers");
    let mut world = create(&dir, source);
    world
        .apply_batch(WorldBatch::new(0, "first", vec![row("a", 3)]))
        .unwrap();
    let values = world.all_settlements().unwrap();
    assert_eq!(
        values["root::d"]["a"].value,
        Value::List(vec![Value::Int(6), Value::Int(2), Value::Int(4)])
    );
    let Value::Record { fields, .. } = &values["root::r"]["a"].value else {
        panic!("record")
    };
    let Value::List(reviews) = &fields["reviews"] else {
        panic!("list")
    };
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0], reviews[1]);
    world.verify_oracle().unwrap();
    world.verify_state().unwrap();
    audit(&world, source);
    drop(world);
    let restored = WorldSession::open(&dir.0).unwrap();
    assert_eq!(restored.open_mode, OpenMode::Restored);
    assert_eq!(restored.all_settlements().unwrap(), values);
    restored.verify_state().unwrap();
}

#[test]
fn list_order_and_duplicates_are_canonical_identity() {
    let a = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(1)]);
    let b = Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(2)]);
    assert_ne!(a.to_bytes(), b.to_bytes());
    assert_ne!(a.state_bytes(), b.state_bytes());
    assert_eq!(Value::state_from_bytes(&a.state_bytes()).unwrap(), a);
    let source = r#"
rel input orders: { id: Str, n: Int } key id
fn ordered(n: Int): List<Int> = match n == 0 { true => [1, 2, 1] false => [1, 1, 2] }
decide d for o in orders per id { propose p priority 1 when true = ordered(o.n) }
"#;
    let mut roots = Vec::new();
    for n in [0, 1] {
        let dir = Dir::new("identity");
        let mut world = create(&dir, source);
        world
            .apply_batch(WorldBatch::new(0, "same", vec![row("a", n)]))
            .unwrap();
        roots.push((
            world.current_revision_digest().unwrap(),
            world.all_settlements().unwrap()["root::d"]["a"]
                .value
                .clone(),
        ));
    }
    assert_ne!(roots[0].0, roots[1].0);
    assert_eq!(roots[0].1, a);
    assert_eq!(roots[1].1, b);
}

#[test]
fn list_width_refusal_is_explicit_deterministic_and_atomic() {
    for width in [256, 257] {
        let items = vec!["o.n"; width].join(",");
        let source = format!("rel input orders: {{ id: Str, n: Int }} key id\ndecide d for o in orders per id {{ propose p priority 1 when true = [{items}] }}");
        let dir = Dir::new("bounds");
        let mut world = create(&dir, &source);
        let before = world.current_revision_digest().unwrap();
        let batch = WorldBatch::new(0, "first", vec![row("a", 1)]);
        if width == 256 {
            world.apply_batch(batch).unwrap();
            world.verify_state().unwrap();
            audit(&world, &source);
        } else {
            let first = world.apply_batch(batch.clone()).unwrap_err().to_string();
            assert!(first.contains("list value exceeds"), "{first}");
            assert_eq!(world.apply_batch(batch).unwrap_err().to_string(), first);
            assert_eq!(world.current_revision_digest().unwrap(), before);
            assert!(world
                .all_settlements()
                .unwrap()
                .values()
                .all(|m| m.is_empty()));
        }
    }
}

#[test]
fn direct_input_list_fields_remain_unsupported() {
    let dir = Dir::new("input");
    let source = "rel input rows: { id: Str, xs: List<Int> } key id";
    let error = WorldSession::from_program_with_sources(
        &dir.0,
        "root",
        &BTreeMap::from([("root".into(), source.into())]),
    )
    .err()
    .expect("input list field rejected")
    .to_string();
    assert!(error.contains("list-valued input tuple field"), "{error}");
}

#[test]
fn disjoint_union_combines_empty_and_nonempty_aggregates_durably() {
    let source = r#"
rel input orders: { id: Str, n: Int } key id
rel input children: { id: Str, owner: Str, amount: Int } key id
rel derived nonempty = select { id: c.owner, total: sum(c.amount) } from c in children group by c.owner
rel derived empty = select { id: o.id, total: 0 } from o in orders where not exists c in children where c.owner == o.id
rel derived totals = union nonempty, empty key id
decide d for t in totals per id { propose p priority 1 when true = t.total }
"#;
    let dir = Dir::new("union");
    let mut world = create(&dir, source);
    world
        .apply_batch(WorldBatch::new(0, "parent", vec![row("a", 0)]))
        .unwrap();
    assert_eq!(
        world.all_settlements().unwrap()["root::d"]["a"].value,
        Value::Int(0)
    );
    let mut child = TupleRecord::new();
    child.set_str("id", "c");
    child.set_str("owner", "a");
    child.set_str("amount", "5");
    world
        .apply_batch(WorldBatch::new(
            1,
            "child",
            vec![WorldBatchOp::Upsert {
                relation: "root::children".into(),
                key: WorldKey::from_str("c"),
                tuple: child.to_tuple(),
            }],
        ))
        .unwrap();
    assert_eq!(
        world.all_settlements().unwrap()["root::d"]["a"].value,
        Value::Int(5)
    );
    world.verify_state().unwrap();
    world.verify_oracle().unwrap();
    audit(&world, source);
    drop(world);
    let mut world = WorldSession::open(&dir.0).unwrap();
    assert_eq!(world.open_mode, OpenMode::Restored);
    world
        .apply_batch(WorldBatch::new(
            2,
            "remove",
            vec![WorldBatchOp::Remove {
                relation: "root::children".into(),
                key: WorldKey::from_str("c"),
            }],
        ))
        .unwrap();
    assert_eq!(
        world.all_settlements().unwrap()["root::d"]["a"].value,
        Value::Int(0)
    );
    world.verify_state().unwrap();
    audit(&world, source);
}

#[test]
fn disjoint_union_rejects_duplicate_keys_atomically() {
    let source = r#"
rel input orders: { id: Str, n: Int } key id
rel derived a = select { id: o.id, n: o.n } from o in orders
rel derived b = select { id: o.id, n: o.n + 1 } from o in orders
rel derived both = union a, b key id
decide d for t in both per id { propose p priority 1 when true = t.n }
"#;
    let dir = Dir::new("union-duplicate");
    let mut world = create(&dir, source);
    let before = world.current_revision_digest();
    let batch = WorldBatch::new(0, "conflict", vec![row("a", 5)]);
    let error = world.apply_batch(batch.clone()).unwrap_err().to_string();
    assert!(error.contains("duplicate key in disjoint union"), "{error}");
    assert_eq!(world.apply_batch(batch).unwrap_err().to_string(), error);
    assert_eq!(world.current_revision_digest(), before);
    assert_eq!(world.current_revision(), 0);
}

fn local_nominals(value: &mut brix_lower::l3_v2::L3ValueV2) {
    use brix_lower::l3_v2::L3ValueV2 as V;
    match value {
        V::Record {
            nominal_config,
            fields,
        } => {
            *nominal_config = nominal_config.trim_start_matches("root::").into();
            for (_, value) in fields {
                local_nominals(value);
            }
        }
        V::Ctor {
            nominal_sum, args, ..
        } => {
            *nominal_sum = nominal_sum.trim_start_matches("root::").into();
            for value in args {
                local_nominals(value);
            }
        }
        V::List(items) => {
            for value in std::sync::Arc::make_mut(items) {
                local_nominals(value);
            }
        }
        _ => {}
    }
}

#[test]
fn structured_list_values_match_finite_profile_under_mutation() {
    use brix_lower::finite_decision::{
        lower_finite_decision_plan, run_finite_decision_plan, FINITE_DECISION_PROFILE,
    };
    let definitions = r#"
config Item = { score: Int }
config Caveat = Note(List<Str>) | Clear
config Report = { reviews: List<Item>, caveats: List<Caveat> }
fn build(n: Int) = Report {
 reviews: map(filter([n, 3, 1, n, -1, 2], x => x > 0), x => Item { score: x }),
 caveats: [Note(distinct(map(filter(["b", "a", "b", "c"], x => x != "c"), x => x))), Clear]
}
"#;
    let source = format!("{definitions}\nrel input orders: {{ id: Str, n: Int }} key id\ndecide d for o in orders per id {{ propose p priority 1 when true = build(o.n) }}");
    for seed in [1u64, 73, 4099] {
        let dir = Dir::new("finite-battery");
        let mut world = create(&dir, &source);
        let mut rng = seed;
        for revision in 0..20 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let n = (rng % 11) as i64 - 3;
            world
                .apply_batch(WorldBatch::new(
                    revision,
                    format!("{seed}-{revision}"),
                    vec![row("a", n)],
                ))
                .unwrap();
            let finite_source = format!("{definitions}\npropose p priority 1 when true = build({n})\ncommit outcome from (p)\nshow outcome");
            let finite = lower_finite_decision_plan(
                &brix_syntax::parse(&finite_source).unwrap(),
                FINITE_DECISION_PROFILE,
            )
            .unwrap();
            let expected = run_finite_decision_plan(&finite)
                .unwrap()
                .decision
                .expect("finite decision")
                .value;
            let mut actual = world.all_settlements().unwrap()["root::d"]["a"]
                .value
                .to_scalar()
                .unwrap();
            local_nominals(&mut actual);
            assert_eq!(actual, expected, "seed={seed} revision={revision}");
            world.verify_oracle().unwrap();
            if revision % 5 == 4 {
                drop(world);
                world = WorldSession::open(&dir.0).unwrap();
                assert_eq!(world.open_mode, OpenMode::Restored);
            }
        }
        world.verify_state().unwrap();
        audit(&world, &source);
    }
}

#[test]
fn list_node_and_depth_limits_hold_at_evaluation_and_decode() {
    for final_width in [239, 240] {
        let full = vec!["1"; 256].join(",");
        let tail = vec!["1"; final_width].join(",");
        let outer = (0..16).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
        let source = format!("rel input orders: {{ id: Str, n: Int }} key id\nfn grid(n: Int): List<List<Int>> = map([{outer}], x => match x < 15 {{ true => [{full}] false => [{tail}] }})\ndecide d for o in orders per id {{ propose p priority 1 when true = grid(o.n) }}");
        let dir = Dir::new("node-limit");
        let mut world = create(&dir, &source);
        let batch = WorldBatch::new(0, "nodes", vec![row("a", 1)]);
        if final_width == 239 {
            world.apply_batch(batch).unwrap();
            world.verify_state().unwrap();
            drop(world);
            WorldSession::open(&dir.0).unwrap().verify_state().unwrap();
        } else {
            let error = world.apply_batch(batch).unwrap_err().to_string();
            assert!(error.contains("node limit"), "{error}");
            assert_eq!(world.current_revision(), 0);
        }
    }
    // Readers refuse both budget violations and huge declared lengths before allocation.
    for nodes in [
        Value::List(vec![Value::List(vec![Value::Int(1); 256]); 16]),
        (0..32).fold(Value::Int(1), |v, _| Value::List(vec![v])),
    ] {
        assert!(Value::state_from_bytes(&nodes.state_bytes()).is_err());
        assert!(Value::from_typed_bytes(
            &nodes.to_bytes(),
            &brix_syntax::ast::Ty::App(
                "List".into(),
                vec![brix_syntax::ast::Ty::Named("Int".into())]
            )
        )
        .is_err());
    }
    let mut frame = brix_canon::CanonWriter::new();
    frame.write_uint(8);
    frame.write_uint(u64::MAX);
    assert!(Value::state_from_bytes(&frame.finish()).is_err());
}
