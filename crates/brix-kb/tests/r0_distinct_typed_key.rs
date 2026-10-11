//! R0 M2 part 2: the Distinct output derivation key is typed (`IntermediateTuple`'s
//! injective state encoding) instead of `Value::to_bytes` (Display based, so `Int(1)` and
//! `Str("1")`, `Bool(true)` and `Str("true")`, `Null` and `Str("null")` collided).
//!
//! The language's static column types make a mixed-type column unconstructible from source
//! today, so the collision itself is exercised at the key level; the end-to-end test checks
//! that explanation through a downstream Distinct (now a keyed lookup, not a scan) lists
//! exactly the right base facts.

use std::collections::{BTreeMap, BTreeSet};

use brix_kb::world::network::distinct_key;
use brix_kb::world::{
    IntermediateTuple, TupleRecord, Value, WorldBatch, WorldBatchOp, WorldKey, WorldNetwork,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

fn tuple(v: Value) -> IntermediateTuple {
    let mut t = IntermediateTuple::new();
    t.insert("v", v);
    t
}

#[test]
fn distinct_key_is_typed_where_display_bytes_collide() {
    let pairs = [
        (Value::Int(1), Value::Str("1".into())),
        (Value::Bool(true), Value::Str("true".into())),
        (Value::Null, Value::Str("null".into())),
    ];
    for (a, b) in pairs {
        let (ta, tb) = (tuple(a), tuple(b));
        // The old key: identical for the two (the bug).
        assert_eq!(
            ta.to_tuple_record().to_tuple().0,
            tb.to_tuple_record().to_tuple().0
        );
        // The new key separates them and is stable.
        assert_ne!(distinct_key(&ta), distinct_key(&tb));
        assert_eq!(distinct_key(&ta), distinct_key(&ta.clone()));
    }
}

const SRC: &str = r#"
rel input t: { id: Str, grp: Str } key id

rel derived ids =
    select { g: t.grp }
    from t in t

rel derived downstream =
    select { g: m.g }
    from m in ids
"#;

fn net() -> WorldNetwork {
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), SRC.to_string());
    let loader = |n: &str| sources.get(n).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).unwrap();
    WorldNetwork::from_program(&graph.link().unwrap()).unwrap()
}

fn up(key: u64, id: &str, grp: &str) -> WorldBatchOp {
    let mut rec = TupleRecord::new();
    rec.set_str("id", id);
    rec.set_str("grp", grp);
    WorldBatchOp::Upsert {
        relation: "root::t".to_string(),
        key: WorldKey::from_u64(key),
        tuple: rec.to_tuple(),
    }
}

fn apply(net: &mut WorldNetwork, ops: Vec<WorldBatchOp>) {
    let b = WorldBatch::new(net.current_revision, "b", ops);
    net.apply_batch(&b).unwrap();
}

#[test]
fn explanation_through_downstream_distinct_lists_exactly_the_right_facts() {
    let mut n = net();
    // Rows 1 and 2 share group "x" (two supports for one distinct tuple); row 3 is "y".
    apply(
        &mut n,
        vec![up(1, "A", "x"), up(2, "B", "x"), up(3, "C", "y")],
    );

    let state = n.current_state().unwrap();
    let mut facts_by_tuple: BTreeMap<String, BTreeSet<WorldKey>> = BTreeMap::new();
    for supports in state.distinct_supports.values() {
        for (rec, sups) in supports {
            let mut out = BTreeSet::new();
            for d in sups {
                n.collect_base_facts_for_derivation(d, &mut out).unwrap();
            }
            let key = String::from_utf8_lossy(rec.fields.get("g").unwrap()).to_string();
            let ids: BTreeSet<WorldKey> = out.iter().map(|(_, k)| k.clone()).collect();
            facts_by_tuple.entry(key).or_default().extend(ids);
        }
    }
    assert_eq!(
        facts_by_tuple["x"],
        BTreeSet::from([WorldKey::from_u64(1), WorldKey::from_u64(2)])
    );
    assert_eq!(facts_by_tuple["y"], BTreeSet::from([WorldKey::from_u64(3)]));

    // Removing one of two supports keeps the tuple; removing the last retracts it.
    apply(
        &mut n,
        vec![WorldBatchOp::Remove {
            relation: "root::t".to_string(),
            key: WorldKey::from_u64(1),
        }],
    );
    n.verify_differential_correctness().unwrap();
    assert_eq!(
        n.current_state().unwrap().derived_relations["root::downstream"].len(),
        2
    );
}
