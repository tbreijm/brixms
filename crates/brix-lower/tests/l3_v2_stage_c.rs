//! ADR-0027 Stage C gates: eligibility, and the first genuinely derived fact
//! reachable in a run.

use brix_lower::l3_v2::{
    lower_l3_plan_v2, run_l3_plan_v2, ArithOpV2, EvalFault, L3PlanV2, L3ValueV2, RunStopV2,
    L3_PROFILE_MARKER_V2,
};
use brix_syntax::parse;

fn plan(src: &str) -> L3PlanV2 {
    let module = parse(src).expect("fixture parses");
    lower_l3_plan_v2(&module, L3_PROFILE_MARKER_V2).expect("fixture lowers")
}

/// **The thing v1 cannot do.** A run produces a fact derived from another
/// fact, in dependency order.
#[test]
fn a_run_produces_a_derived_fact() {
    let run = run_l3_plan_v2(&plan(
        "rule base()        = 1500\n\
         rule boosted(base) = base + 500\n",
    ));

    assert_eq!(run.stop, RunStopV2::AllRulesCommitted);
    assert_eq!(run.facts.len(), 2);

    assert_eq!(run.facts[0].rule, "base");
    assert_eq!(run.facts[0].value, L3ValueV2::Int(1500));
    assert!(run.facts[0].depends_on.is_empty());

    assert_eq!(run.facts[1].rule, "boosted");
    assert_eq!(
        run.facts[1].value,
        L3ValueV2::Int(2000),
        "the second fact is DERIVED from the first"
    );
    assert_eq!(run.facts[1].depends_on, vec!["base".to_string()]);
    assert_eq!(run.facts[1].ordinal, 1);
}

/// A rule commits only once its dependencies have, whatever order they are
/// declared in.
#[test]
fn commit_order_follows_dependencies() {
    let run = run_l3_plan_v2(&plan(
        "rule a()      = 1\n\
         rule b(a)     = a + 1\n\
         rule c(a, b)  = a + b\n",
    ));
    assert_eq!(run.stop, RunStopV2::AllRulesCommitted);
    let order: Vec<&str> = run.facts.iter().map(|f| f.rule.as_str()).collect();
    assert_eq!(order, vec!["a", "b", "c"]);
    assert_eq!(run.facts[2].value, L3ValueV2::Int(3));
}

/// A run is deterministic: the same plan produces the same facts, in the same
/// order, every time. That ordering is semantic — a later stage's journal and
/// certificate identities are built from it.
#[test]
fn a_run_is_deterministic() {
    let p = plan("rule a() = 2\nrule b(a) = a * 3\nrule c(a, b) = b - a\n");
    let first = run_l3_plan_v2(&p);
    for _ in 0..16 {
        assert_eq!(run_l3_plan_v2(&p), first);
    }
    assert_eq!(first.facts[2].value, L3ValueV2::Int(4));
}

/// A faulting rule stops the run rather than publishing a world in which its
/// dependents silently never resolved.
#[test]
fn a_fault_stops_the_run() {
    let run = run_l3_plan_v2(&plan(
        "let big = 9223372036854775807\n\
         rule overflows()          = big + big\n\
         rule dependent(overflows) = overflows\n",
    ));

    assert_eq!(
        run.stop,
        RunStopV2::Faulted {
            rule: "overflows".to_string(),
            fault: EvalFault::Overflow(ArithOpV2::Add),
        }
    );
    assert!(
        run.facts.is_empty(),
        "nothing is published once the run faults"
    );
}

/// Termination is structural (ADR-0027 ⟨D-TERMINATES⟩): each rule commits at
/// most once over an acyclic graph, so a plan of N rules admits at most N
/// commits. There is no budget because there is no unbounded case.
#[test]
fn a_run_terminates_in_at_most_one_commit_per_rule() {
    let src: String = (0..64)
        .map(|i| {
            if i == 0 {
                "rule r0() = 0\n".to_string()
            } else {
                format!("rule r{i}(r{}) = r{} + 1\n", i - 1, i - 1)
            }
        })
        .collect();
    let run = run_l3_plan_v2(&plan(&src));

    assert_eq!(run.stop, RunStopV2::AllRulesCommitted);
    assert_eq!(run.facts.len(), 64, "at most one commit per rule");
    assert_eq!(run.facts[63].value, L3ValueV2::Int(63));
}

/// A run over a plan with no rules commits nothing and says so, rather than
/// reporting success over an empty world.
#[test]
fn an_empty_plan_commits_nothing() {
    let run = run_l3_plan_v2(&plan("config Z = Hand | Field\n"));
    assert_eq!(run.stop, RunStopV2::AllRulesCommitted);
    assert!(run.facts.is_empty());
}

/// The run's stop status is **not** a quiescence certificate, and its name
/// deliberately does not borrow the word.
///
/// ADR-0012 §5 makes `Quiescent` the only decided negative *and* makes it
/// certificate-backed: an empty frontier is decided only when a checker can
/// re-derive that it was empty. This runner is driven directly rather than
/// through `run_saturated`, so it reports what it observed and claims no more.
#[test]
fn the_stop_status_claims_no_certificate() {
    let run = run_l3_plan_v2(&plan("rule a() = 1\n"));
    let rendered = format!("{:?}", run.stop);
    for word in ["Quiescent", "quiescent", "Proven", "certificate"] {
        assert!(
            !rendered.contains(word),
            "the stop status must not claim '{word}': {rendered}"
        );
    }
}
