//! R7 porting regressions and a reproducible parent-count cascade workload.
use brix_kb::world::decision_codec::Value;
use brix_kb::world::{reference, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldNetwork};
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits};
use std::collections::BTreeMap;

fn link(source: &str) -> (LinkedProgram, WorldNetwork) {
    let loader = |name: &str| (name == "root").then(|| source.to_string());
    let program = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let net = WorldNetwork::from_program(&program).unwrap();
    (program, net)
}
fn row(relation: &str, id: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut tuple = TupleRecord::new();
    tuple.set_str("id", id);
    for (name, value) in fields {
        tuple.set_str(*name, value);
    }
    WorldBatchOp::Upsert {
        relation: format!("root::{relation}"),
        key: WorldKey::from_str(id),
        tuple: tuple.to_tuple(),
    }
}
fn check_reference(program: &LinkedProgram, net: &WorldNetwork) {
    let state = net.current_state().unwrap();
    let expected = reference::evaluate(
        &reference::from_program(program).unwrap(),
        &state.base_relations,
    )
    .unwrap();
    assert_eq!(state.derived_relations, expected.derived_relations);
    let actual: Vec<_> = state
        .settlements
        .iter()
        .flat_map(|(name, rows)| {
            rows.iter().map(move |(entity, s)| {
                (
                    name,
                    entity,
                    &s.candidate_name,
                    s.calendar_key,
                    format!("{:?}", s.value),
                )
            })
        })
        .collect();
    let wanted: Vec<_> = expected
        .settlements
        .iter()
        .flat_map(|(name, rows)| {
            rows.iter().map(move |(entity, s)| {
                (
                    name,
                    entity,
                    &s.candidate_name,
                    s.calendar_key,
                    format!("{:?}", s.value),
                )
            })
        })
        .collect();
    assert_eq!(actual, wanted);
}

#[test]
fn aggregate_match_nested_record_projection_and_inferred_helper_return() {
    let source = r#"
config R = { price: Int }
config Out = Ok(Int) | No
fn mk(n: Int) = Ok(n)
rel input rows: { id: Str, owner: Str, act: Bool, e: R } key id
rel derived d0 = select { id: r.id, owner: r.owner, p: r.e.price, act: r.act } from r in rows
rel derived a = select { id: x.owner, t: sum(match x.act { true => x.p false => 0 }) } from x in d0 group by x.owner
decide d for o in a per id {
 propose ok priority 1 when o.t > 0 = mk(o.t)
 propose no priority 2 when true = No
}
"#;
    let (program, mut net) = link(source);
    let record = r#"{"type":"record","nominal":"root::R","fields":[{"name":"price","value":{"type":"int","value":"5"}}]}"#;
    net.apply_batch(&WorldBatch::new(
        0,
        "first",
        vec![
            row(
                "rows",
                "a",
                &[("owner", "o"), ("act", "true"), ("e", record)],
            ),
            row(
                "rows",
                "b",
                &[("owner", "o"), ("act", "false"), ("e", record)],
            ),
        ],
    ))
    .unwrap();
    assert_eq!(
        net.current_state().unwrap().settlements["root::d"]["o"].value,
        Value::Variant {
            nominal: "root::Out".into(),
            variant: "Ok".into(),
            args: vec![Value::Int(5)]
        }
    );
    check_reference(&program, &net);
}

#[test]
fn absence_conjunct_filters_and_tracks_insert_retract() {
    let source = r#"
rel input a: { id: Str, n: Int } key id
rel input b: { id: Str, a_id: Str } key id
rel derived lone = select { id: p.id } from p in a where p.n > 0 && not exists q in b where q.a_id == p.id
decide d for p in lone per id { propose yes priority 1 when true = p.id }
"#;
    let (program, mut net) = link(source);
    net.apply_batch(&WorldBatch::new(
        0,
        "first",
        vec![
            row("a", "positive", &[("n", "1")]),
            row("a", "zero", &[("n", "0")]),
        ],
    ))
    .unwrap();
    assert_eq!(net.current_state().unwrap().settlements["root::d"].len(), 1);
    check_reference(&program, &net);
    net.apply_batch(&WorldBatch::new(
        1,
        "second",
        vec![row("b", "block", &[("a_id", "positive")])],
    ))
    .unwrap();
    assert!(net
        .current_state()
        .unwrap()
        .settlements
        .get("root::d")
        .is_none_or(|s| s.is_empty()));
    check_reference(&program, &net);
    net.apply_batch(&WorldBatch::new(
        2,
        "third",
        vec![WorldBatchOp::Remove {
            relation: "root::b".into(),
            key: WorldKey::from_str("block"),
        }],
    ))
    .unwrap();
    assert_eq!(net.current_state().unwrap().settlements["root::d"].len(), 1);
    check_reference(&program, &net);
}

fn cascade(stages: usize) -> String {
    let mut source = String::from("rel input owners: { id: Str } key id\nrel input rows: { id: Str, owner: Str, v: Int } key id\nrel derived y0 = select { id: r.id, owner: r.owner, v: r.v } from r in rows\n");
    for i in 1..=stages {
        let prev = i - 1;
        source.push_str(&format!("rel derived x{i} = select {{ id: r.id, owner: r.owner, v: r.v }} from r in y{prev} where r.v > {i}\nrel derived c{i} = select {{ id: o.id, count: count_or_zero() }} from o in owners, l in x{i} where l.owner == o.id group by o.id\nrel derived y{i} = select {{ id: r.id, owner: r.owner, v: r.v }} from r in y{prev}, h in c{i} where h.id == r.owner && ((h.count > 0 && r.v > {i}) || (h.count == 0 && r.v >= 0))\n"));
    }
    source
}
#[test]
#[ignore = "R7.1 release-mode operator-work profile"]
fn count_or_zero_cascade_profile() {
    for stages in [2, 4, 6, 8] {
        let (program, mut net) = link(&cascade(stages));
        let mut ops = vec![row("owners", "o", &[])];
        for i in 0..8 {
            ops.push(row(
                "rows",
                &format!("r{i}"),
                &[("owner", "o"), ("v", &i.to_string())],
            ));
        }
        let start = std::time::Instant::now();
        let report = net.apply_batch(&WorldBatch::new(0, "first", ops)).unwrap();
        let mut work_by_kind: BTreeMap<String, u64> = BTreeMap::new();
        for (op, work) in &report.diagnostics.operator_work {
            let kind = format!("{:?}", net.dag.nodes[op.0])
                .split_whitespace()
                .next()
                .unwrap()
                .to_string();
            *work_by_kind.entry(kind).or_default() += work;
        }
        eprintln!(
            "stages={stages} elapsed={:?} deltas={} work={work_by_kind:?}",
            start.elapsed(),
            report.intermediate_deltas_count
        );
        check_reference(&program, &net);
    }
}

#[test]
fn constructor_ambiguity_requires_qualification_in_both_profiles() {
    use brix_lower::finite_decision::{
        lower_finite_decision_plan, run_finite_decision_plan, FINITE_DECISION_PROFILE,
    };
    let definitions = "config Value = Text(Str) | Num(Int)\nconfig OptStr = Text(Str) | NoText\n";
    let world = format!("{definitions}rel input rows: {{ id: Str }} key id\ndecide d for r in rows per id {{ propose p priority 1 when true = Text(r.id) }}");
    let loader = |name: &str| (name == "root").then(|| world.clone());
    let error = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap_err()
        .to_string();
    assert!(error.contains("ambiguous symbol 'Text'"), "{error}");
    link(&world.replace("= Text(r.id)", "= OptStr::Text(r.id)"));
    let finite = format!("{definitions}propose p priority 1 when true = Text(\"a\")\ncommit outcome from (p)\nshow outcome");
    let error = lower_finite_decision_plan(
        &brix_syntax::parse(&finite).unwrap(),
        FINITE_DECISION_PROFILE,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("ambiguous constructor 'Text'"), "{error}");
    let finite = lower_finite_decision_plan(
        &brix_syntax::parse(&finite.replace("= Text(\"a\")", "= OptStr::Text(\"a\")")).unwrap(),
        FINITE_DECISION_PROFILE,
    )
    .unwrap();
    assert!(run_finite_decision_plan(&finite)
        .unwrap()
        .decision
        .is_some());
    // A local declaration cannot silently shadow an imported nominal constructor.
    let imported = "export config Value = Text(Str) | Num(Int)";
    let root = "use vocab\nconfig OptStr = Text(Str) | NoText\nrel input rows: { id: Str } key id\ndecide d for r in rows per id { propose p priority 1 when true = Text(r.id) }";
    let loader = |name: &str| match name {
        "root" => Some(root.to_string()),
        "vocab" => Some(imported.to_string()),
        _ => None,
    };
    let error = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap_err()
        .to_string();
    assert!(error.contains("ambiguous symbol 'Text'"), "{error}");
}

#[test]
fn keyword_module_names_have_actionable_errors() {
    let loader = |_name: &str| Some("rel input rows: { id: Str } key id".into());
    let error = ModuleGraph::load("match", &loader, ModuleLoaderLimits::default())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("reserved keyword 'match' cannot be a module name"),
        "{error}"
    );
    let error = brix_syntax::parse("use match").unwrap_err().to_string();
    assert!(error.contains("cannot be a module name"), "{error}");
}

#[test]
fn absence_disjunction_requires_explicit_parentheses() {
    let source = "rel input a: { id: Str, n: Int } key id\nrel input b: { id: Str, a_id: Str } key id\nrel derived lone = select { id: p.id } from p in a where p.n > 0 || p.n < 0 && not exists q in b where q.a_id == p.id";
    assert!(brix_syntax::parse(source)
        .unwrap_err()
        .to_string()
        .contains("top-level conjunct"));
    link(&source.replace("p.n > 0 || p.n < 0 &&", "(p.n > 0 || p.n < 0) &&"));
}
