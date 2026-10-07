use std::collections::BTreeMap;

use brix_kb::world::{reference, TupleRecord, WorldError, WorldKey};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

fn run(depth: usize, max_work: Option<u64>) -> Result<reference::ReferenceWorkMeter, WorldError> {
    let mut src = String::from("fn h0(x: Int): Int = x\n");
    for i in 1..=depth {
        src.push_str(&format!(
            "fn h{i}(x: Int): Int = h{}(x) + h{}(x)\n",
            i - 1,
            i - 1
        ));
    }
    src.push_str(&format!(
        "rel input rows: {{ id: Str, n: Int }} key id\ndecide d for r in rows per id {{ propose p priority 1 when true = h{depth}(r.n) }}\n"
    ));
    let sources = BTreeMap::from([("root".to_string(), src)]);
    let graph = ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .unwrap();
    let prog = reference::from_program(&graph.link().unwrap()).unwrap();
    let mut rec = TupleRecord::new();
    rec.set_str("id", "a");
    rec.set_str("n", "1");
    let rows = BTreeMap::from([(
        "root::rows".to_string(),
        BTreeMap::from([(WorldKey::from_str("a"), rec.to_tuple())]),
    )]);
    let mut meter = reference::ReferenceWorkMeter::default();
    let state = reference::evaluate_with_budget(&prog, &rows, &mut meter, max_work, 0)?;
    assert_eq!(
        state.settlements["root::d"]["a"].value,
        reference::Value::Int(1i64 << depth)
    );
    Ok(meter)
}

#[test]
fn scalar_helper_work_is_charged_and_exhausts_budget() {
    // With small budget of 20, 511 helper calls must exhaust budget
    let result = run(8, Some(20));
    assert!(
        matches!(result, Err(WorldError::BudgetExhausted)),
        "expected BudgetExhausted, got {result:?}"
    );

    // With depth 0 and budget 20, it easily succeeds
    let shallow = run(0, Some(20)).unwrap();

    // With adequate budget, depth 8 succeeds and expressions_evaluated accounts for recursive calls
    let deep = run(8, Some(5000)).unwrap();
    assert!(
        deep.expressions_evaluated > shallow.expressions_evaluated,
        "deep ({}) must charge more expressions than shallow ({})",
        deep.expressions_evaluated,
        shallow.expressions_evaluated
    );
    assert!(
        deep.expressions_evaluated >= 511,
        "deep must account for at least 511 helper evaluation steps, got {}",
        deep.expressions_evaluated
    );
}
