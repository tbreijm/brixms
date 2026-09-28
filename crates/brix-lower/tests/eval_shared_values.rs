//! Reading a value the evaluator already holds is not an allocation: list
//! inputs and facts are shared, so a program may read a large list input
//! many times, and iterate one list per element of another, within the
//! value budget.

use brix_lower::{
    canonicalize_input_shards, decode_input_shard, lower_finite_decision_plan,
    FiniteDecisionRuntime, InputLimits, FINITE_DECISION_PROFILE,
};

fn run(source: &str, input_json: &str) -> brix_lower::FiniteDecisionRun {
    let module = brix_syntax::parse(source).expect("parses");
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("lowers");
    let shard =
        decode_input_shard(input_json.as_bytes(), &InputLimits::default()).expect("decodes");
    let snapshot =
        canonicalize_input_shards(vec![shard], &InputLimits::default()).expect("snapshot");
    FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
        .expect("runtime builds")
        .run()
}

/// About 960 KB of strings, read three times: three deep copies would pass
/// the 1,000,000-byte value budget; shared reads do not.
#[test]
fn a_large_list_input_can_be_read_repeatedly() {
    let text = "x".repeat(60_000);
    let items = vec![format!(r#"{{"type":"string","value":"{text}"}}"#); 16].join(",");
    let input = format!(
        r#"{{"schema":"brix.input@3","values":{{"a":{{"type":"list","items":[{items}]}}}}}}"#
    );
    let source = "config D = Go\ninput a: List<Str> max 16\n\
                  propose go priority 1 when len(a) + len(a) + len(a) == 48 = Go\n\
                  commit d from (go)\n";
    let run = run(source, &input);
    assert!(run.first_fault().is_none(), "{:?}", run.first_fault());
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("go"));
}

/// A join iterates the whole inner list once per outer element: 256 x 256
/// pairs, each reading the inner list input, stays within budget.
#[test]
fn a_full_join_of_two_bounded_lists_stays_within_budget() {
    let ints = |n: usize| {
        (0..n)
            .map(|i| format!(r#"{{"type":"int","value":"{i}"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    };
    let input = format!(
        r#"{{"schema":"brix.input@3","values":{{"xs":{{"type":"list","items":[{}]}},"ys":{{"type":"list","items":[{}]}}}}}}"#,
        ints(256),
        ints(256)
    );
    let source = "config D = Go\ninput xs: List<Int> max 256\ninput ys: List<Int> max 256\n\
                  rule matches() = count(xs, x => any(ys, y => y == x))\n\
                  propose go priority 1 when matches == 256 = Go\ncommit d from (go)\n";
    let run = run(source, &input);
    assert!(run.first_fault().is_none(), "{:?}", run.first_fault());
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("go"));
}

fn ints(n: usize) -> String {
    (0..n)
        .map(|i| format!(r#"{{"type":"int","value":"{i}"}}"#))
        .collect::<Vec<_>>()
        .join(",")
}

fn assert_step_limit(run: &brix_lower::FiniteDecisionRun, detail: &str) {
    let fault = format!("{:?}", run.first_fault().expect("the run must fail closed"));
    assert!(fault.contains(detail), "{fault}");
}

/// A three-way join of maximum-size lists (16.7M combinations) is refused
/// by the per-evaluation step bound, as Unknown.
#[test]
fn a_three_way_join_exceeds_the_evaluation_step_limit() {
    let input = format!(
        r#"{{"schema":"brix.input@3","values":{{"xs":{{"type":"list","items":[{}]}}}}}}"#,
        ints(256)
    );
    let source = "config D = Go\ninput xs: List<Int> max 256\n\
                  rule n() = count(xs, a => any(xs, b => any(xs, c => a + b + c < 0)))\n\
                  propose go priority 1 when n == 0 = Go\ncommit d from (go)\n";
    assert_step_limit(&run(source, &input), "evaluation step limit exceeded");
}

/// Work is also bounded across a whole run: 256 decide instances that each
/// do a full join fit individually but not together.
#[test]
fn per_entity_work_is_bounded_across_the_run() {
    let input = format!(
        r#"{{"schema":"brix.input@3","values":{{"xs":{{"type":"list","items":[{}]}}}}}}"#,
        ints(256)
    );
    let source = "config D = Go | Stop\ninput xs: List<Int> max 256\n\
                  decide each for v in xs {\n  \
                  propose go priority 1 when count(xs, a => any(xs, b => a + b == v)) >= 0 = Go\n  \
                  propose stop otherwise = Stop\n}\n";
    assert_step_limit(&run(source, &input), "run step limit exceeded");
}
