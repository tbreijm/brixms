//! Wall-clock cost of producing and verifying a finite-decision audit input
//! bundle (ADR-0026, ADR-0030) — the same library calls `brix audit` and
//! `brix verify` (`crates/brix-cli/src/commands/audit.rs`,
//! `crates/brix-cli/src/commands/verify.rs`) drive, called directly here so
//! the benchmark is a library-level measurement rather than a subprocess.
//!
//! `harness = false`, `std` only. Run with
//! `cargo bench -p brix-lower --bench audit_verify`.

use std::time::{Duration, Instant};

use brix_lower::audit_bundle::{
    check_finite_decision_audit_input_bundle_from_module_with_inputs_v1,
    produce_finite_decision_audit_input_bundle_v1,
};
use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionRuntime,
    FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{canonicalize_input_shards, decode_input_shard, InputLimits};
use brix_lower::{AuditDecodeLimits, PlanLimitsV1};
use brix_syntax::ast::{Item, Module};
use brix_syntax::{parse_bounded, ParseLimits};
use soc_core::audit::AuditResult;
use soc_core::audit_bundle::decode_audit_input_bundle_v1;

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

fn strip_show(module: &mut Module) {
    module.items.retain(|i| !matches!(i, Item::Show(_)));
}

const WARMUP: usize = 5;
const ITERS: usize = 100;

fn main() {
    println!("# Audit-bundle produce/verify wall-clock (brix-lower)\n");
    println!("Fixture: examples/shipping-input.brix + examples/shipping-input.json\n");
    println!("Method: warmup {WARMUP}, then median/p90 over {ITERS} iterations.\n");

    let source = include_str!("../../../examples/shipping-input.brix");
    let json = include_str!("../../../examples/shipping-input.json");

    let input_limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &input_limits).expect("fixture json decodes");
    let snapshot =
        canonicalize_input_shards(vec![shard], &input_limits).expect("fixture json canonicalizes");

    let mut module = parse_bounded(source, ParseLimits::strict()).expect("source parses");
    strip_show(&mut module);
    let plan =
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture plan lowers");
    let runtime =
        FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).expect("fixture runtime builds");
    let run = runtime.run();
    assert!(
        matches!(run.stop, FiniteDecisionStop::Selected(_)),
        "fixture must reach a selected decision for the audit bundle to be produced, got: {:?}",
        run.stop
    );
    let audit_results = runtime.audit(&run.journal);
    assert!(
        audit_results
            .iter()
            .all(|r| matches!(r, AuditResult::Audited(_))),
        "fixture's committed steps must all audit cleanly"
    );

    let decode_limits = AuditDecodeLimits::strict();
    let expected_program = finite_decision_program_id(&plan);
    let plan_limits = PlanLimitsV1::generous();

    // Sanity pass outside the timed loop, so a real failure panics with a
    // clear message rather than showing up as a confusing timing outlier.
    let bundle_check =
        produce_finite_decision_audit_input_bundle_v1(&runtime, &run).expect("bundle produces");
    let bytes_check = bundle_check.encode(&decode_limits).expect("bundle encodes");
    let decoded_check =
        decode_audit_input_bundle_v1(&bytes_check, &decode_limits).expect("bundle decodes");
    check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
        &module,
        expected_program,
        &plan_limits,
        &decoded_check,
        &decode_limits,
        &snapshot,
    )
    .expect("bundle verifies");
    let bundle_bytes_len = bytes_check.len();

    for _ in 0..WARMUP {
        let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run).unwrap();
        let bytes = bundle.encode(&decode_limits).unwrap();
        let decoded = decode_audit_input_bundle_v1(&bytes, &decode_limits).unwrap();
        check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
            &module,
            expected_program,
            &plan_limits,
            &decoded,
            &decode_limits,
            &snapshot,
        )
        .unwrap();
    }

    let mut produce_d = Vec::with_capacity(ITERS);
    let mut encode_d = Vec::with_capacity(ITERS);
    let mut decode_d = Vec::with_capacity(ITERS);
    let mut verify_d = Vec::with_capacity(ITERS);
    let mut total_d = Vec::with_capacity(ITERS);

    for _ in 0..ITERS {
        let total_start = Instant::now();

        let t0 = Instant::now();
        let bundle =
            produce_finite_decision_audit_input_bundle_v1(&runtime, &run).expect("bundle produces");
        produce_d.push(t0.elapsed());

        let t1 = Instant::now();
        let bytes = bundle.encode(&decode_limits).expect("bundle encodes");
        encode_d.push(t1.elapsed());

        let t2 = Instant::now();
        let decoded = decode_audit_input_bundle_v1(&bytes, &decode_limits).expect("bundle decodes");
        decode_d.push(t2.elapsed());

        let t3 = Instant::now();
        check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
            &module,
            expected_program,
            &plan_limits,
            &decoded,
            &decode_limits,
            &snapshot,
        )
        .expect("bundle verifies");
        verify_d.push(t3.elapsed());

        total_d.push(total_start.elapsed());
    }

    println!("bundle size: {bundle_bytes_len} bytes\n");
    println!("{:<28} | {:>12} | {:>12}", "phase", "median", "p90");
    println!("{}", "-".repeat(58));
    for (name, d) in [
        ("produce (run -> bundle)", produce_d),
        ("encode (bundle -> bytes)", encode_d),
        ("decode (bytes -> bundle)", decode_d),
        ("verify (re-lower + check)", verify_d),
        ("total (produce+encode+decode+verify)", total_d),
    ] {
        let (median, p90) = stats(d);
        println!(
            "{:<28} | {:>12} | {:>12}",
            name,
            fmt_ns(median),
            fmt_ns(p90)
        );
    }

    println!(
        "\n\"verify\" re-lowers the source module from scratch (as an offline verifier would) \
         before checking the bundle, so it is expected to be the more expensive of the two \
         end-to-end paths. See docs/performance.md for the captured numbers and interpretation."
    );
}
