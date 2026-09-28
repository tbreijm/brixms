//! End-to-end wall-clock: parse -> lower -> build runtime -> run, for the
//! shipped `examples/*.brix` programs and for synthetic programs scaled
//! toward the finite-decision profile's declared `MAX_*` bounds
//! (`crates/brix-lower/src/finite_decision/plan.rs`,
//! `crates/brix-lower/src/input.rs`).
//!
//! `harness = false`, `std` only — no `criterion` (not Ring-0 whitelisted).
//! Run with `cargo bench -p brix-lower --bench finite_decision_end_to_end`.

use std::time::{Duration, Instant};

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionPlan, FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, InputLimits, InputSnapshot,
};
use brix_syntax::ast::{Item, Module};
use brix_syntax::{parse_bounded, ParseLimits};

fn stats(mut durations: Vec<Duration>) -> (Duration, Duration) {
    durations.sort();
    let median = durations[durations.len() / 2];
    let p90_idx = ((durations.len() * 90) / 100).min(durations.len() - 1);
    (median, durations[p90_idx])
}

fn fmt_ns(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns >= 1_000_000 {
        format!("{:.3} ms", d.as_secs_f64() * 1e3)
    } else if ns >= 1_000 {
        format!("{:.3} us", ns as f64 / 1e3)
    } else {
        format!("{ns} ns")
    }
}

/// Same retain-Show-items step `brix-cli`'s `prepare_finite_decision_module`
/// performs before lowering (that helper lives in `brix-cli`, a downstream
/// crate this bench does not depend on, so it is inlined here — one line,
/// same behavior).
fn strip_show(module: &mut Module) {
    module.items.retain(|i| !matches!(i, Item::Show(_)));
}

fn snapshot_from_json(json: &str) -> InputSnapshot {
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("fixture json decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("fixture json canonicalizes")
}

struct PhaseTimes {
    parse: Duration,
    lower: Duration,
    build: Duration,
    run: Duration,
}

/// Time one parse -> lower -> build -> run pass. Panics (failing the bench,
/// loudly) if any phase does not succeed — every fixture here is expected to
/// lower and build cleanly; a run may legitimately end Unknown/Quiescent,
/// which is not a failure.
fn time_pass(source: &str, snapshot: &InputSnapshot) -> (PhaseTimes, FiniteDecisionPlan) {
    let t0 = Instant::now();
    let mut module = parse_bounded(source, ParseLimits::strict()).expect("source parses");
    let parse = t0.elapsed();

    strip_show(&mut module);

    let t1 = Instant::now();
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("plan lowers");
    let lower = t1.elapsed();

    let t2 = Instant::now();
    let runtime =
        FiniteDecisionRuntime::build_with_inputs(&plan, snapshot).expect("runtime builds");
    let build = t2.elapsed();

    let t3 = Instant::now();
    let _run = runtime.run();
    let run = t3.elapsed();

    (
        PhaseTimes {
            parse,
            lower,
            build,
            run,
        },
        plan,
    )
}

const WARMUP: usize = 5;
const ITERS: usize = 100;

fn bench_fixture(name: &str, source: &str, snapshot: &InputSnapshot) {
    for _ in 0..WARMUP {
        time_pass(source, snapshot);
    }
    let mut parse_d = Vec::with_capacity(ITERS);
    let mut lower_d = Vec::with_capacity(ITERS);
    let mut build_d = Vec::with_capacity(ITERS);
    let mut run_d = Vec::with_capacity(ITERS);
    let mut total_d = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let start = Instant::now();
        let (t, _plan) = time_pass(source, snapshot);
        total_d.push(start.elapsed());
        parse_d.push(t.parse);
        lower_d.push(t.lower);
        build_d.push(t.build);
        run_d.push(t.run);
    }
    let (pm, _) = stats(parse_d);
    let (lm, _) = stats(lower_d);
    let (bm, _) = stats(build_d);
    let (rm, _) = stats(run_d);
    let (tm, tp90) = stats(total_d);
    println!(
        "{:<28} | {:>10} | {:>10} | {:>10} | {:>10} | {:>10} | {:>10}",
        name,
        fmt_ns(pm),
        fmt_ns(lm),
        fmt_ns(bm),
        fmt_ns(rm),
        fmt_ns(tm),
        fmt_ns(tp90)
    );
}

// ---------------------------------------------------------------------------
// Synthetic programs scaled toward the finite-decision `MAX_*` bounds.
// ---------------------------------------------------------------------------

/// `n` rules in a strict dependency chain: `r_i` depends on `r_{i-1}` alone.
fn chain_of_rules(n: usize) -> String {
    let mut s = String::from("config Decision = A | B\n\nrule r0() = 1\n");
    for i in 1..n {
        let prev = i - 1;
        s.push_str(&format!("rule r{i}(r{prev}) = r{prev} + 1\n"));
    }
    let last = n - 1;
    s.push_str(&format!(
        "\npropose take(r{last}) priority 1 when r{last} >= 0 = A\n"
    ));
    s.push_str("propose fallback() priority 1000 when true = B\n\n");
    s.push_str("commit d from (take, fallback)\n");
    s
}

/// `n` independent candidate proposals in one commit pool.
fn many_proposals(n: usize) -> String {
    let mut s = String::from("config Decision = A | B\n\nrule base() = 1\n\n");
    for i in 0..n {
        s.push_str(&format!(
            "propose p{i}(base) priority {} when base >= 0 = A\n",
            i + 1
        ));
    }
    s.push_str(&format!(
        "propose fallback() priority {} when true = B\n\n",
        n + 1000
    ));
    s.push_str("commit d from (");
    let names: Vec<String> = (0..n)
        .map(|i| format!("p{i}"))
        .chain(std::iter::once("fallback".to_string()))
        .collect();
    s.push_str(&names.join(", "));
    s.push_str(")\n");
    s
}

/// `n` declared helper `fn`s (near `MAX_FUNCTION_COUNT`); only three are
/// actually called, so this isolates the cost of *declaring* many helpers
/// (schema/arity bookkeeping at lowering time) from evaluation cost.
fn many_helper_functions(n: usize) -> String {
    let mut s = String::from("config Decision = A | B\n\n");
    for i in 0..n {
        s.push_str(&format!("fn f{i}(x: Int): Int = x + {i}\n"));
    }
    let mid = n / 2;
    let last = n - 1;
    s.push_str(&format!("\nrule r() = (f0(1) + f{mid}(2)) + f{last}(3)\n"));
    s.push_str("\npropose take(r) priority 1 when r >= 0 = A\n");
    s.push_str("propose fallback() priority 1000 when true = B\n\n");
    s.push_str("commit d from (take, fallback)\n");
    s
}

/// A single `brix.input@2` record input with `n_fields` `Int` fields (up to
/// `MAX_INPUT_CONTAINER_WIDTH`), plus a matching JSON snapshot.
fn large_record_input(n_fields: usize) -> (String, String) {
    let mut src = String::from("config Decision = A | B\n\nconfig BigRecord = {\n");
    for i in 0..n_fields {
        let sep = if i + 1 < n_fields { "," } else { "" };
        src.push_str(&format!("  f{i}: Int{sep}\n"));
    }
    src.push_str("}\n\ninput batch: BigRecord\n\n");
    src.push_str(&format!(
        "rule sum2() = batch.f0 + batch.f{}\n\n",
        n_fields - 1
    ));
    src.push_str("propose take(sum2) priority 1 when sum2 >= 0 = A\n");
    src.push_str("propose fallback() priority 1000 when true = B\n\n");
    src.push_str("commit d from (take, fallback)\n");

    let mut json = String::from(
        r#"{"schema":"brix.input@2","values":{"batch":{"type":"record","nominal":"BigRecord","fields":["#,
    );
    for i in 0..n_fields {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            r#"{{"name":"f{i}","value":{{"type":"int","value":"{i}"}}}}"#
        ));
    }
    json.push_str("]}}}");
    (src, json)
}

fn main() {
    println!("# Finite-decision end-to-end wall-clock (brix-lower)\n");
    println!("Method: warmup {WARMUP}, then median/p90 (total column) over {ITERS} iterations.\n");
    println!(
        "{:<28} | {:>10} | {:>10} | {:>10} | {:>10} | {:>10} | {:>10}",
        "fixture", "parse", "lower", "build", "run", "total(med)", "total(p90)"
    );
    println!("{}", "-".repeat(115));

    // Shipped examples, each with its own input snapshot (empty for
    // shipping.brix, which declares no inputs).
    let fixtures: [(&str, &str, Option<&str>); 5] = [
        (
            "shipping.brix",
            include_str!("../../../examples/shipping.brix"),
            None,
        ),
        (
            "shipping-input.brix",
            include_str!("../../../examples/shipping-input.brix"),
            Some(include_str!("../../../examples/shipping-input.json")),
        ),
        (
            "shipping-functions.brix",
            include_str!("../../../examples/shipping-functions.brix"),
            Some(include_str!("../../../examples/shipping-functions.json")),
        ),
        (
            "allocation.brix",
            include_str!("../../../examples/allocation.brix"),
            Some(include_str!("../../../examples/allocation.json")),
        ),
        (
            "order-policy.brix",
            include_str!("../../../examples/order-policy.brix"),
            Some(include_str!("../../../examples/order-policy.json")),
        ),
    ];
    for (name, src, json) in fixtures {
        let snapshot = match json {
            Some(j) => snapshot_from_json(j),
            None => InputSnapshot::empty(),
        };
        bench_fixture(name, src, &snapshot);
    }

    println!();
    let empty = InputSnapshot::empty();

    let n_rules = 200;
    let chain_src = chain_of_rules(n_rules);
    bench_fixture(
        &format!("synthetic: {n_rules}-rule chain"),
        &chain_src,
        &empty,
    );

    let n_proposals = 200;
    let proposals_src = many_proposals(n_proposals);
    bench_fixture(
        &format!("synthetic: {n_proposals} proposals"),
        &proposals_src,
        &empty,
    );

    let n_functions = 200;
    let functions_src = many_helper_functions(n_functions);
    bench_fixture(
        &format!("synthetic: {n_functions} helper fns"),
        &functions_src,
        &empty,
    );

    let n_fields = 256; // MAX_INPUT_CONTAINER_WIDTH
    let (record_src, record_json) = large_record_input(n_fields);
    let record_snapshot = snapshot_from_json(&record_json);
    bench_fixture(
        &format!("synthetic: {n_fields}-field record input"),
        &record_src,
        &record_snapshot,
    );

    println!(
        "\nSee docs/performance.md for the captured numbers and interpretation — in particular \
         whether each phase's cost is flat or grows with program size, and what that implies for \
         the \"one program -> one decision over <=256 inputs\" profile these limits describe."
    );
}
