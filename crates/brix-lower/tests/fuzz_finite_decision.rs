//! Property-based robustness ("fuzz") tests for finite-decision lowering and
//! the deliberation runtime (ADR-0030/ADR-0031): `lower_finite_decision_plan`
//! and `FiniteDecisionRuntime::build_with_inputs(...).run()` must never
//! panic on hostile or merely-generated input, and running the same plan
//! twice must be exactly deterministic.
//!
//! Two generation strategies:
//! - **Mutation**: small random edits to the shipped `examples/*.brix`
//!   programs, kept only when the result still parses (`prop_assume!`).
//! - **Construction**: small well-formed decision programs built from
//!   scratch — random rule arithmetic (`+`/`-`/`*`/`div_floor`/`mod_euclid`),
//!   comparisons, `&&`/`||`/`!`, and proposals with random priorities — which
//!   must always lower and run to a decision or a typed Unknown (e.g.
//!   division by zero), never a panic.
//!
//! Case counts are proptest's own default (256) unless overridden via
//! `PROPTEST_CASES`. See `docs/performance.md` for measured wall-clock cost.

use proptest::prelude::*;

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, InputLimits, InputSnapshot,
};
use brix_syntax::ast::{Item, Module};
use brix_syntax::{parse, parse_bounded, ParseLimits};

// ---------------------------------------------------------------------------
// (1) Mutated example programs that still parse.
// ---------------------------------------------------------------------------

const FIXTURES: &[(&str, &str, Option<&str>)] = &[
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
    (
        "shipping-functions.brix",
        include_str!("../../../examples/shipping-functions.brix"),
        Some(include_str!("../../../examples/shipping-functions.json")),
    ),
    (
        "shipping-input.brix",
        include_str!("../../../examples/shipping-input.brix"),
        Some(include_str!("../../../examples/shipping-input.json")),
    ),
    (
        "shipping.brix",
        include_str!("../../../examples/shipping.brix"),
        None,
    ),
    (
        "fulfillment.brix",
        include_str!("../../../examples/fulfillment.brix"),
        Some(include_str!("../../../examples/fulfillment.json")),
    ),
    (
        "order-book.brix",
        include_str!("../../../examples/order-book.brix"),
        Some(include_str!("../../../examples/order-book.json")),
    ),
];

/// A small vocabulary of real Brix tokens, so a mutated program has some
/// chance of still parsing (an insertion of pure noise almost never does).
const TOKEN_VOCAB: &[&str] = &[
    "config",
    "rule",
    "propose",
    "commit",
    "input",
    "fn",
    "when",
    "priority",
    "from",
    "let",
    "show",
    "true",
    "false",
    "match",
    "{",
    "}",
    "(",
    ")",
    ":",
    "=",
    "|",
    ",",
    ".",
    "+",
    "-",
    "*",
    "==",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "&&",
    "||",
    "!",
    "0",
    "1",
    "42",
    "A",
    "B",
    "x",
    "[",
    "]",
    "=>",
    "for",
    "in",
    "where",
    "yield",
    "filter",
    "map",
    "sum",
    "count",
    "len",
    "distinct",
    "min",
    "max",
    "List",
    "max",
    "decide",
    "otherwise",
];

#[derive(Clone, Debug)]
enum Mutation {
    DeleteLine(f64),
    InsertLine(f64, usize),
    DuplicateLine(f64),
    SwapLines(f64, f64),
    DeleteToken(f64),
    InsertToken(f64, usize),
    SwapTokens(f64, f64),
}

fn mutation_strategy() -> impl Strategy<Value = Mutation> {
    let unit = 0.0f64..1.0f64;
    prop_oneof![
        unit.clone().prop_map(Mutation::DeleteLine),
        (unit.clone(), 0..TOKEN_VOCAB.len()).prop_map(|(f, v)| Mutation::InsertLine(f, v)),
        unit.clone().prop_map(Mutation::DuplicateLine),
        (unit.clone(), unit.clone()).prop_map(|(a, b)| Mutation::SwapLines(a, b)),
        unit.clone().prop_map(Mutation::DeleteToken),
        (unit.clone(), 0..TOKEN_VOCAB.len()).prop_map(|(f, v)| Mutation::InsertToken(f, v)),
        (unit.clone(), unit).prop_map(|(a, b)| Mutation::SwapTokens(a, b)),
    ]
}

fn frac_index(f: f64, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (((f.clamp(0.0, 1.0)) * len as f64) as usize).min(len - 1)
    }
}

fn apply_mutation(lines: &mut Vec<String>, m: &Mutation) {
    if lines.is_empty() {
        lines.push(String::new());
    }
    let n = lines.len();
    match m {
        Mutation::DeleteLine(f) => {
            lines.remove(frac_index(*f, n));
        }
        Mutation::InsertLine(f, v) => {
            let i = frac_index(*f, n + 1).min(lines.len());
            lines.insert(i, TOKEN_VOCAB[v % TOKEN_VOCAB.len()].to_string());
        }
        Mutation::DuplicateLine(f) => {
            let i = frac_index(*f, n);
            let l = lines[i].clone();
            lines.insert(i, l);
        }
        Mutation::SwapLines(fa, fb) => {
            lines.swap(frac_index(*fa, n), frac_index(*fb, n));
        }
        Mutation::DeleteToken(f) => {
            let i = frac_index(*f, n);
            let mut toks: Vec<&str> = lines[i].split_whitespace().collect();
            if !toks.is_empty() {
                toks.remove(frac_index(*f, toks.len()));
            }
            lines[i] = toks.join(" ");
        }
        Mutation::InsertToken(f, v) => {
            let i = frac_index(*f, n);
            let mut toks: Vec<String> = lines[i].split_whitespace().map(String::from).collect();
            let k = frac_index(*f, toks.len() + 1).min(toks.len());
            toks.insert(k, TOKEN_VOCAB[v % TOKEN_VOCAB.len()].to_string());
            lines[i] = toks.join(" ");
        }
        Mutation::SwapTokens(fa, fb) => {
            let i = frac_index(*fa, n);
            let mut toks: Vec<String> = lines[i].split_whitespace().map(String::from).collect();
            if toks.len() >= 2 {
                let a = frac_index(*fa, toks.len());
                let b = frac_index(*fb, toks.len());
                toks.swap(a, b);
            }
            lines[i] = toks.join(" ");
        }
    }
}

fn strip_show(module: &mut Module) {
    module.items.retain(|i| !matches!(i, Item::Show(_)));
}

fn snapshot_for(json: Option<&str>) -> InputSnapshot {
    match json {
        Some(j) => {
            let limits = InputLimits::default();
            let shard = decode_input_shard(j.as_bytes(), &limits).expect("fixture json decodes");
            canonicalize_input_shards(vec![shard], &limits).expect("fixture json canonicalizes")
        }
        None => InputSnapshot::empty(),
    }
}

proptest! {
    // A small edit is far more likely than a large one to still parse (this
    // is exactly the filter `prop_assume!` below applies), so proptest's
    // default reject budget (1024 discards per 256 cases) is raised here —
    // otherwise a legitimately low, but non-zero, still-parses rate trips
    // "too many global rejects" rather than reflecting a generator problem.
    #![proptest_config(ProptestConfig { max_global_rejects: 65_536, ..ProptestConfig::default() })]

    /// Only mutations that still *parse* are in scope (`prop_assume!`
    /// discards the rest without failing the property, matching "for mutated
    /// example programs that still parse" exactly). For those, lowering must
    /// never panic, and whenever it succeeds, building the runtime and
    /// running it must never panic either — a build/run `Err` is a fine,
    /// typed outcome (e.g. the mutation changed which inputs are declared, so
    /// the fixture's original snapshot no longer validates against it).
    ///
    /// When the runtime *does* build, this also checks determinism: building
    /// and running the identical plan+snapshot twice must reproduce the same
    /// program id, context id, decision, and dispositions.
    #[test]
    fn mutated_example_lowering_and_run_never_panics(
        fixture_idx in 0..FIXTURES.len(),
        muts in proptest::collection::vec(mutation_strategy(), 0..6),
    ) {
        let (_, src, json) = FIXTURES[fixture_idx];
        let mut lines: Vec<String> = src.lines().map(String::from).collect();
        for m in &muts {
            apply_mutation(&mut lines, m);
        }
        let mutated = lines.join("\n");

        let parsed = parse_bounded(&mutated, ParseLimits::strict());
        prop_assume!(parsed.is_ok());
        let mut module = parsed.unwrap();
        strip_show(&mut module);

        if let Ok(plan) = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE) {
            let snapshot = snapshot_for(json);
            if let Ok(runtime) = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot) {
                let run = runtime.run(); // must not panic

                let runtime2 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
                    .expect("building the identical plan+snapshot again must also succeed");
                let run2 = runtime2.run();

                prop_assert_eq!(run.program, run2.program, "program id must be deterministic");
                prop_assert_eq!(run.context, run2.context, "context id must be deterministic");
                prop_assert_eq!(&run.decision, &run2.decision, "decision must be deterministic");
                prop_assert_eq!(
                    &run.dispositions,
                    &run2.dispositions,
                    "candidate dispositions must be deterministic"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// (2) Small well-formed decision programs, generated from scratch.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum IntExpr {
    Lit(i64),
    Ref(usize),
    Add(Box<IntExpr>, Box<IntExpr>),
    Sub(Box<IntExpr>, Box<IntExpr>),
    Mul(Box<IntExpr>, Box<IntExpr>),
    DivFloor(Box<IntExpr>, Box<IntExpr>),
    ModEuclid(Box<IntExpr>, Box<IntExpr>),
}

/// `n_avail` is the number of previously declared rule names this expression
/// may reference by name (rule `r_i`'s body may read `r_0..r_{i-1}` — Brix's
/// dependency-by-declared-parameter discipline, `finite_decision/plan.rs`).
/// Small literal range (`-20..=20`, inclusive of 0) so `div_floor`/
/// `mod_euclid` hit their division-by-zero fault path a meaningful fraction
/// of the time — exactly the "or a typed Unknown" half of this property.
fn int_expr_strategy(n_avail: usize) -> BoxedStrategy<IntExpr> {
    let leaf = if n_avail > 0 {
        prop_oneof![
            (-20i64..=20).prop_map(IntExpr::Lit),
            (0..n_avail).prop_map(IntExpr::Ref),
        ]
        .boxed()
    } else {
        (-20i64..=20).prop_map(IntExpr::Lit).boxed()
    };
    leaf.prop_recursive(3, 20, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| IntExpr::Add(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| IntExpr::Sub(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| IntExpr::Mul(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| IntExpr::DivFloor(Box::new(a), Box::new(b))),
            (inner.clone(), inner).prop_map(|(a, b)| IntExpr::ModEuclid(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

fn render_int(e: &IntExpr, names: &[String]) -> String {
    match e {
        IntExpr::Lit(n) if *n < 0 => format!("({n})"),
        IntExpr::Lit(n) => n.to_string(),
        IntExpr::Ref(i) => names[*i].clone(),
        IntExpr::Add(a, b) => format!("({} + {})", render_int(a, names), render_int(b, names)),
        IntExpr::Sub(a, b) => format!("({} - {})", render_int(a, names), render_int(b, names)),
        IntExpr::Mul(a, b) => format!("({} * {})", render_int(a, names), render_int(b, names)),
        IntExpr::DivFloor(a, b) => format!(
            "div_floor({}, {})",
            render_int(a, names),
            render_int(b, names)
        ),
        IntExpr::ModEuclid(a, b) => format!(
            "mod_euclid({}, {})",
            render_int(a, names),
            render_int(b, names)
        ),
    }
}

#[derive(Clone, Copy, Debug)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn text(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

fn cmp_op_strategy() -> impl Strategy<Value = CmpOp> {
    prop_oneof![
        Just(CmpOp::Eq),
        Just(CmpOp::Ne),
        Just(CmpOp::Lt),
        Just(CmpOp::Le),
        Just(CmpOp::Gt),
        Just(CmpOp::Ge),
    ]
}

#[derive(Clone, Debug)]
enum BoolExpr {
    Lit(bool),
    Cmp(CmpOp, IntExpr, IntExpr),
    Not(Box<BoolExpr>),
    And(Box<BoolExpr>, Box<BoolExpr>),
    Or(Box<BoolExpr>, Box<BoolExpr>),
}

fn bool_expr_strategy(n_avail: usize) -> BoxedStrategy<BoolExpr> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(BoolExpr::Lit),
        (
            cmp_op_strategy(),
            int_expr_strategy(n_avail),
            int_expr_strategy(n_avail)
        )
            .prop_map(|(op, a, b)| BoolExpr::Cmp(op, a, b)),
    ];
    leaf.prop_recursive(3, 20, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|b| BoolExpr::Not(Box::new(b))),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| BoolExpr::And(Box::new(a), Box::new(b))),
            (inner.clone(), inner).prop_map(|(a, b)| BoolExpr::Or(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

fn render_bool(e: &BoolExpr, names: &[String]) -> String {
    match e {
        BoolExpr::Lit(b) => b.to_string(),
        BoolExpr::Cmp(op, a, b) => format!(
            "({} {} {})",
            render_int(a, names),
            op.text(),
            render_int(b, names)
        ),
        BoolExpr::Not(b) => format!("!({})", render_bool(b, names)),
        BoolExpr::And(a, b) => format!("({} && {})", render_bool(a, names), render_bool(b, names)),
        BoolExpr::Or(a, b) => format!("({} || {})", render_bool(a, names), render_bool(b, names)),
    }
}

#[derive(Clone, Debug)]
struct GeneratedProgram {
    rule_exprs: Vec<IntExpr>,
    guard: BoolExpr,
    priorities: [u64; 3],
}

/// `n_rules` int-valued rules `r_0..r_{n-1}` in a strict dependency chain
/// (`r_i` may reference any of `r_0..r_{i-1}`), each an arbitrary small
/// arithmetic expression; capped at 4 to keep generated programs small and
/// this strategy's construction simple (a fixed match per count rather than a
/// general dependently-typed `Vec` combinator).
fn rules_strategy(n_rules: usize) -> BoxedStrategy<Vec<IntExpr>> {
    match n_rules {
        1 => int_expr_strategy(0).prop_map(|a| vec![a]).boxed(),
        2 => (int_expr_strategy(0), int_expr_strategy(1))
            .prop_map(|(a, b)| vec![a, b])
            .boxed(),
        3 => (
            int_expr_strategy(0),
            int_expr_strategy(1),
            int_expr_strategy(2),
        )
            .prop_map(|(a, b, c)| vec![a, b, c])
            .boxed(),
        _ => (
            int_expr_strategy(0),
            int_expr_strategy(1),
            int_expr_strategy(2),
            int_expr_strategy(3),
        )
            .prop_map(|(a, b, c, d)| vec![a, b, c, d])
            .boxed(),
    }
}

fn generated_program_strategy() -> impl Strategy<Value = GeneratedProgram> {
    (1usize..=4)
        .prop_flat_map(|n_rules| {
            rules_strategy(n_rules).prop_flat_map(|rule_exprs| {
                let n = rule_exprs.len();
                (
                    Just(rule_exprs),
                    bool_expr_strategy(n),
                    1u64..=1000,
                    1u64..=1000,
                    1u64..=1000,
                )
            })
        })
        .prop_map(|(rule_exprs, guard, p0, p1, p2)| GeneratedProgram {
            rule_exprs,
            guard,
            priorities: [p0, p1, p2],
        })
}

/// Renders `p` to a complete Brix decision program: `n` int rules in a
/// dependency chain, a boolean `guard` rule over them, three candidate
/// proposals (`A` when the guard holds, `B` when it does not, and an
/// unconditional `C` fallback so the commit pool is never empty), and a
/// commit of all three.
fn render_program(p: &GeneratedProgram) -> String {
    let mut s = String::from("config Decision = A | B | C\n\n");
    let mut names: Vec<String> = Vec::new();
    for (i, e) in p.rule_exprs.iter().enumerate() {
        let name = format!("r{i}");
        let deps = names.join(", ");
        s.push_str(&format!(
            "rule {name}({deps}) = {}\n",
            render_int(e, &names)
        ));
        names.push(name);
    }
    let guard_deps = names.join(", ");
    s.push_str(&format!(
        "\nrule guard({guard_deps}) = {}\n\n",
        render_bool(&p.guard, &names)
    ));
    s.push_str(&format!(
        "propose take_a(guard) priority {} when guard = A\n",
        p.priorities[0]
    ));
    s.push_str(&format!(
        "propose take_b(guard) priority {} when !guard = B\n",
        p.priorities[1]
    ));
    s.push_str(&format!(
        "propose fallback() priority {} when true = C\n\n",
        p.priorities[2]
    ));
    s.push_str("commit decision from (take_a, take_b, fallback)\n");
    s
}

proptest! {
    /// A generated program is well-formed *by construction* (valid
    /// dependency chain, valid types throughout), so — unlike the mutation
    /// property above — lowering and building are asserted to succeed
    /// outright; only the final `run()` outcome branches (a selected
    /// decision, certified quiescence, or a typed Unknown such as division by
    /// zero from a generated `div_floor`/`mod_euclid` with a zero divisor),
    /// and none of the three is a panic.
    ///
    /// Also checks determinism, as the mutation property above does.
    #[test]
    fn well_formed_random_decision_programs_lower_and_run_deterministically(
        p in generated_program_strategy()
    ) {
        let src = render_program(&p);
        let module = parse(&src)
            .unwrap_or_else(|e| panic!("generated program must parse: {e}\n---\n{src}"));
        let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
            .unwrap_or_else(|e| panic!("generated program must lower: {e}\n---\n{src}"));

        let runtime = FiniteDecisionRuntime::build(&plan)
            .unwrap_or_else(|e| panic!("generated program (no inputs) must build: {e}\n---\n{src}"));
        let run = runtime.run(); // must not panic

        prop_assert!(
            matches!(
                run.stop,
                FiniteDecisionStop::Selected(_)
                    | FiniteDecisionStop::Quiescent { .. }
                    | FiniteDecisionStop::Unknown(_)
            ),
            "run must terminate in one of the three defined stop states, program:\n{src}"
        );

        let runtime2 = FiniteDecisionRuntime::build(&plan).expect("rebuilds identically");
        let run2 = runtime2.run();
        prop_assert_eq!(run.program, run2.program, "program id must be deterministic");
        prop_assert_eq!(run.context, run2.context, "context id must be deterministic");
        prop_assert_eq!(&run.decision, &run2.decision, "decision must be deterministic");
        prop_assert_eq!(
            &run.dispositions,
            &run2.dispositions,
            "candidate dispositions must be deterministic"
        );
    }
}

// ---------------------------------------------------------------------------
// (3) A fixed program exercising every list/relational form (ADR-0037,
//     ADR-0040) over a random `List<Int> max 8` snapshot: sum/count/all/any/
//     min/max, filter/map, `in`, `len`, `distinct`. `min`/`max` fault on an
//     empty list (`EmptyAggregate`), which this property allows — it is a
//     typed `Unknown`, not a panic.
// ---------------------------------------------------------------------------

const LIST_PROGRAM: &str = r#"
config Decision = A | B | C

input xs: List<Int> max 8

rule total() = sum(xs, x => x)
rule cnt() = len(xs)
rule biggest() = max(xs, x => x)
rule smallest() = min(xs, x => x)
rule positives() = filter(xs, x => x > 0)
rule doubled() = map(xs, x => x * 2)
rule uniq() = distinct(xs)
rule has_zero() = 0 in xs
rule any_neg() = any(xs, x => x < 0)
rule all_pos() = all(xs, x => x > 0)

propose take_a(cnt) priority 10 when cnt > 0 = A
propose take_b(cnt) priority 20 when cnt == 0 = B
propose fallback() priority 30 when true = C

commit decision from (take_a, take_b, fallback)
"#;

fn list_snapshot(values: &[i64]) -> InputSnapshot {
    let items = values
        .iter()
        .map(|v| format!(r#"{{"type": "int", "value": "{v}"}}"#))
        .collect::<Vec<_>>()
        .join(", ");
    let json = format!(
        r#"{{"schema": "brix.input@3", "values": {{"xs": {{"type": "list", "items": [{items}]}}}}}}"#
    );
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("generated @3 shard decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("generated @3 shard canonicalizes")
}

proptest! {
    /// Every list/relational form over a random-length, random-valued
    /// `List<Int> max 8`: lowering and building are asserted to succeed (the
    /// program is well-formed by construction); `run()` must never panic and
    /// must land in one of the three defined stop states — including
    /// `Unknown` from `min`/`max`'s empty-list fault — and, as above,
    /// running the identical plan+snapshot twice must be exactly
    /// deterministic.
    #[test]
    fn list_program_lowers_and_runs_deterministically(
        values in proptest::collection::vec(-1000i64..=1000, 0..=8),
    ) {
        let module = parse(LIST_PROGRAM)
            .unwrap_or_else(|e| panic!("list fixture program must parse: {e}"));
        let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
            .unwrap_or_else(|e| panic!("list fixture program must lower: {e}"));

        let snapshot = list_snapshot(&values);
        let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
            .unwrap_or_else(|e| panic!("list fixture program must build with any valid List<Int> max 8 snapshot: {e}"));
        let run = runtime.run(); // must not panic, even when xs is empty

        prop_assert!(
            matches!(
                run.stop,
                FiniteDecisionStop::Selected(_)
                    | FiniteDecisionStop::Quiescent { .. }
                    | FiniteDecisionStop::Unknown(_)
            ),
            "run must terminate in one of the three defined stop states for xs = {values:?}"
        );

        let runtime2 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
            .expect("rebuilds identically");
        let run2 = runtime2.run();
        prop_assert_eq!(run.program, run2.program, "program id must be deterministic");
        prop_assert_eq!(run.context, run2.context, "context id must be deterministic");
        prop_assert_eq!(&run.decision, &run2.decision, "decision must be deterministic");
        prop_assert_eq!(
            &run.dispositions,
            &run2.dispositions,
            "candidate dispositions must be deterministic"
        );
    }
}

// ---------------------------------------------------------------------------
// (4) A fixed program with a per-entity `decide` block (ADR-0043) over a
//     random `List<Int> max 8` snapshot: one instance per element, each
//     independently classified `small`/`big`/`negative`. `run()` must never
//     panic, the block must either fully settle (one instance per element,
//     in order) or report a single typed Unknown, and repeated runs over
//     the same snapshot must be exactly deterministic.
// ---------------------------------------------------------------------------

const DECIDE_PROGRAM: &str = r#"
config Decision = Small | Big | Negative

input xs: List<Int> max 8

decide classification for x in xs {
  propose negative priority 1 when x < 0 = Negative
  propose small priority 2 when x >= 0 && x < 100 = Small
  propose big otherwise = Big
}
"#;

proptest! {
    /// A `decide` block over a random-length, random-valued `List<Int> max
    /// 8`: lowering and building are asserted to succeed (well-formed by
    /// construction, and every proposal admits unconditionally on at least
    /// one of the three disjoint ranges plus `otherwise`, so no instance can
    /// ever fault or quiesce); `run()` must never panic, must produce
    /// exactly one settled instance per input element, in order, with the
    /// classification implied by its own value, and running the identical
    /// plan+snapshot twice must be exactly deterministic — including the
    /// per-instance decisions and the journal length.
    #[test]
    fn decide_program_lowers_and_runs_deterministically(
        values in proptest::collection::vec(-1000i64..=1000, 0..=8),
    ) {
        let module = parse(DECIDE_PROGRAM)
            .unwrap_or_else(|e| panic!("decide fixture program must parse: {e}"));
        let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
            .unwrap_or_else(|e| panic!("decide fixture program must lower: {e}"));

        let snapshot = list_snapshot(&values);
        let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
            .unwrap_or_else(|e| panic!("decide fixture program must build with any valid List<Int> max 8 snapshot: {e}"));
        let run = runtime.run(); // must not panic, even when xs is empty

        prop_assert_eq!(run.decides.len(), 1, "exactly one decide block");
        let decide_run = &run.decides[0];
        prop_assert!(
            !decide_run.is_unknown(),
            "every instance admits unconditionally on one of its three disjoint ranges, so the block can never be Unknown for xs = {values:?}"
        );
        prop_assert_eq!(
            decide_run.instances.len(),
            values.len(),
            "one settled instance per input element"
        );
        for (idx, (inst, &v)) in decide_run.instances.iter().zip(values.iter()).enumerate() {
            prop_assert_eq!(inst.index, idx);
            let expected = if v < 0 {
                "negative"
            } else if v < 100 {
                "small"
            } else {
                "big"
            };
            let actual = inst
                .decision
                .as_ref()
                .map(|d| d.candidate.as_str())
                .unwrap_or("(none)");
            prop_assert_eq!(
                actual,
                expected,
                "instance {} (value {}) must classify as '{}'",
                idx,
                v,
                expected
            );
        }

        let runtime2 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
            .expect("rebuilds identically");
        let run2 = runtime2.run();
        prop_assert_eq!(run.program, run2.program, "program id must be deterministic");
        prop_assert_eq!(run.journal.len(), run2.journal.len(), "journal length must be deterministic");
        let names1: Vec<_> = decide_run
            .instances
            .iter()
            .map(|i| i.decision.as_ref().map(|d| d.candidate.clone()))
            .collect();
        let names2: Vec<_> = run2.decides[0]
            .instances
            .iter()
            .map(|i| i.decision.as_ref().map(|d| d.candidate.clone()))
            .collect();
        prop_assert_eq!(names1, names2, "per-instance decisions must be deterministic");
    }
}
