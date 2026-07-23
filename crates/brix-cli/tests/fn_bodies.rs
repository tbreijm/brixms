//! Issue #47 Slice 1: a total, expression-bodied user function is compiled
//! from its BrixMS source, type/effect/totality-checked, and **executed** —
//! identically on the oracle and the generated runtime — with **no**
//! hand-registered native implementation on either side.
//!
//! Slice 1 proved the unit-free pipeline; Slice 1.5 adds unit-literal scaling
//! (`150 EUR` -> 15000) so the flagship's `surcharge` compiles from source too.
//! `riskModel` still stays hand-registered (partial-result + runtime provenance,
//! deferred to Slice 2).

use brix_ast::parse_file;
use brix_oracle::dsl::{binop, edge_bind, evar, let_, row, rule, var};
use brix_oracle::dump::dump_bytes;
use brix_oracle::frontend::{program_from_source, FnLibrary, KindTable};
use brix_oracle::program::{
    BinOp as OracleBinOp, Expr as OracleExpr, Program as OracleProgram, RelKind, RelationDef,
};
use brix_oracle::store::Store as OracleStore;
use brix_oracle::txn::Transaction as OracleTxn;
use brix_oracle::value::Value;
use brix_rt::engine as rt;
use brixc::lower::RuntimeRelationKind;
use brixc::pipeline::PhaseAssign;
use brixc::{lower_file, AstPhase};

/// `bump` is a total, expression-bodied fn over `Int` — no units, no `?` — so
/// it lowers to a Core IR `FnDef` and runs from source. The rule calls it.
const SRC: &str = "package t @ 0.1.0\n\
rel Input { value: Int } key(value)\n\
rel Output { value: Int } key(value)\n\
fn bump(x: Int) -> Int = x + 1\n\
derive R: Output(value: y) from { Input(value: v); let y = bump(v) }\n";

fn kinds(lowered: &brixc::Lowered) -> KindTable {
    let mut table = KindTable::new();
    for relation in lowered.resolver.relations() {
        if relation.derived {
            continue;
        }
        let kind = match lowered.resolver.relation_kind(&relation.name) {
            RuntimeRelationKind::Entity => RelKind::Entity,
            RuntimeRelationKind::Ground => RelKind::Ground,
            RuntimeRelationKind::State => RelKind::State,
            RuntimeRelationKind::Event => RelKind::Event,
        };
        table.insert(relation.name.to_string(), kind);
    }
    table
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[test]
fn total_fn_compiled_from_source_matches_oracle_dump_for_dump() {
    let (file, diags) = parse_file(SRC);
    assert!(!diags.has_errors(), "must parse cleanly: {diags:?}");
    let lowered = lower_file(&file, &diags);
    assert!(
        !lowered.has_errors(),
        "must lower + type/effect-check cleanly: {:#?}",
        lowered.diags
    );

    // The body was actually lowered into Core IR (not skipped / deferred).
    assert!(
        lowered
            .source
            .functions
            .iter()
            .any(|f| f.name.to_string() == "bump"),
        "bump must be lowered to a checked FnDef, got {:?}",
        lowered
            .source
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect::<Vec<_>>()
    );

    let phased = AstPhase
        .assign_phases(lowered)
        .expect("fixture must be well-stratified");

    // --- Oracle: empty FnLibrary — `bump` executes from its compiled body. ---
    let oracle_program = program_from_source(
        &phased.lowered.source,
        &phased.lowered.resolver,
        &kinds(&phased.lowered),
        FnLibrary::new(),
    )
    .expect("flagship must adapt to the oracle");
    let mut store = OracleStore::new(oracle_program).expect("program is stratified");
    let settled = store
        .commit(
            &OracleTxn::new(b"brix-stdin-0".to_vec())
                .assert("Input", row(&[("value", Value::Int(7))])),
        )
        .expect("transaction commits");
    let oracle_hex = hex(&dump_bytes(settled));

    // --- Generated runtime: `bump` executes from its projected fn_def. ---
    let rt_program = brixc::emit::project_program(&phased);
    let out = brix_rt::engine::run_text(rt_program, "assert Input value=int:7\n")
        .expect("generated runtime runs the transaction");
    let rt_hex = out
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("run_text emits a canonical dump line");

    assert_eq!(
        rt_hex, oracle_hex,
        "a fn compiled from source must settle identically on both engines"
    );
}

/// Issue #47 Slice 1.5: a fn with **unit literals** (`3500 kg`, `150 EUR`)
/// compiles from source — the money scale (`150 EUR` -> 15000) is folded at
/// lowering and both engines agree. This is the faithful proof that the
/// `surcharge` shape works, since the flagship's own orders never cross the
/// 3500 kg threshold (so the `150 EUR` branch is never exercised there).
#[test]
fn money_unit_fn_compiled_from_source_matches_oracle() {
    // `fee` is exactly `surcharge`'s shape. Driven with a heavy order (5000 kg
    // -> 15000) and a light one (1000 kg -> 0).
    const FEE_SRC: &str = "package t @ 0.1.0\n\
rel Order { id: Int; weight: Quantity<Mass> } key(id)\n\
rel Fee { id: Int; amount: Money<EUR> } key(id)\n\
fn fee(w: Quantity<Mass>) -> Money<EUR> = if w > 3500 kg then 150 EUR else 0 EUR\n\
derive R: Fee(id: i, amount: a) from { Order(id: i, weight: w); let a = fee(w) }\n";

    let (file, diags) = parse_file(FEE_SRC);
    assert!(!diags.has_errors(), "must parse: {diags:?}");
    let lowered = lower_file(&file, &diags);
    assert!(
        !lowered.has_errors(),
        "must lower + check cleanly (fee compiles from source): {:#?}",
        lowered.diags
    );
    assert!(
        lowered
            .source
            .functions
            .iter()
            .any(|f| f.name.to_string() == "fee"),
        "fee must be lowered to a FnDef (unit literals folded, not deferred)"
    );
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("fixture must be well-stratified");

    // Two orders: 5000 kg (over threshold -> 15000) and 1000 kg (-> 0).
    let stream = "assert Order id=int:1,weight=int:5000\nassert Order id=int:2,weight=int:1000\n";

    // Oracle (empty FnLibrary — fee runs from source).
    let oracle_program = program_from_source(
        &phased.lowered.source,
        &phased.lowered.resolver,
        &kinds(&phased.lowered),
        FnLibrary::new(),
    )
    .expect("adapts to oracle");
    let mut store = OracleStore::new(oracle_program).expect("stratified");
    let settled = store
        .commit(
            &OracleTxn::new(b"brix-stdin-0".to_vec())
                .assert(
                    "Order",
                    row(&[("id", Value::Int(1)), ("weight", Value::Int(5000))]),
                )
                .assert(
                    "Order",
                    row(&[("id", Value::Int(2)), ("weight", Value::Int(1000))]),
                ),
        )
        .expect("commits");
    let oracle_hex = hex(&dump_bytes(settled));

    // Generated runtime.
    let rt_program = brixc::emit::project_program(&phased);
    let out = brix_rt::engine::run_text(rt_program, stream).expect("runtime runs");
    let rt_hex = out
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("dump line");

    assert_eq!(
        rt_hex, oracle_hex,
        "a money-unit fn compiled from source (150 EUR -> 15000) must match the oracle"
    );
}

/// Issue #47 Slice 2: a total, **block-bodied** fn with a `let`-statement
/// sequence and a **block-bodied `if`** (both `then` and `else` blocks)
/// compiles from source and executes identically on both engines.
#[test]
fn block_bodied_fn_compiled_from_source_matches_oracle() {
    const BLOCK_SRC: &str = "package t @ 0.1.0\n\
rel Input { value: Int } key(value)\n\
rel Output { value: Int } key(value)\n\
fn step(x: Int) -> Int {\n\
  let a = x + 1\n\
  let b = a + a\n\
  if b > 10 { b } else { 0 }\n\
}\n\
derive R: Output(value: y) from { Input(value: v); let y = step(v) }\n";

    let (file, diags) = parse_file(BLOCK_SRC);
    assert!(!diags.has_errors(), "must parse: {diags:?}");
    let lowered = lower_file(&file, &diags);
    assert!(
        !lowered.has_errors(),
        "block-bodied fn must lower + check cleanly: {:#?}",
        lowered.diags
    );
    assert!(
        lowered
            .source
            .functions
            .iter()
            .any(|f| f.name.to_string() == "step"),
        "step must be lowered to a FnDef (block body -> nested lets)"
    );
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("fixture must be well-stratified");

    // value 7 -> a=8, b=16, 16>10 -> 16 (then-block); value 2 -> a=3, b=6,
    // 6>10 false -> 0 (else-block). Exercises both block arms.
    let oracle_program = program_from_source(
        &phased.lowered.source,
        &phased.lowered.resolver,
        &kinds(&phased.lowered),
        FnLibrary::new(),
    )
    .expect("adapts to oracle");
    let mut store = OracleStore::new(oracle_program).expect("stratified");
    let settled = store
        .commit(
            &OracleTxn::new(b"brix-stdin-0".to_vec())
                .assert("Input", row(&[("value", Value::Int(7))]))
                .assert("Input", row(&[("value", Value::Int(2))])),
        )
        .expect("commits");
    let oracle_hex = hex(&dump_bytes(settled));

    let rt_program = brixc::emit::project_program(&phased);
    let out = brix_rt::engine::run_text(
        rt_program,
        "assert Input value=int:7\nassert Input value=int:2\n",
    )
    .expect("runtime runs");
    let rt_hex = out
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("dump line");

    assert_eq!(
        rt_hex, oracle_hex,
        "a block-bodied fn (let sequence + block-if) must match the oracle"
    );
}

/// Issue #47 Part 2: the fixed-point ruling's building blocks — `div`
/// (`BinOp::Div`, truncating integer division) and `brix.math.clamp` (the
/// prelude-seeded builtin total, issue #47 Part 2's `builtin_total`) —
/// exercised directly at the basis-point-integer level `riskModel` itself
/// needs, on both engines with **no** hand-registered `FnLibrary` entry
/// (clamp resolves via `builtin_total` on both sides). Hand-built `Program`s
/// (not compiled from BrixMS surface syntax) because the surface checker's
/// `F64`/dimension algebra is a separate, larger piece of infra than this
/// ruling covers (see the plan's Part 2 scope) — this proves the two
/// *evaluators* agree, independent of that.
///
/// `(24 - remaining) * 10000 / 24` then clamped to `[0, 10000]` is exactly
/// `riskModel`'s reconciled integer-floor form (mirrors
/// `crates/brix-rt/src/engine.rs`'s `builtin_partial` and the reconciled
/// oracle/CLI hand-regs). `remaining = 8` is the former divergence case:
/// the old f64 round-half hand-reg gave 6667, the runtime's integer-floor
/// gave 6666 — this fixture proves 6666 on both engines via the real `Div`/
/// `clamp` evaluators, not a hand-transcription of either.
#[test]
fn div_and_clamp_evaluators_agree_on_riskmodels_reconciled_vectors() {
    // remaining -> expected basis-point risk, per the reconciled formula.
    // 8 is the former 6667-vs-6666 divergence; 30 and -10 exercise clamp's
    // low/high saturation (a pre-clamp value outside [0, 10000]).
    const VECTORS: &[(&str, i64, i64)] = &[
        ("ord-0", 0, 10_000),
        ("ord-8", 8, 6_666),
        ("ord-24", 24, 0),
        ("ord-30", 30, 0),
        ("ord-neg10", -10, 10_000),
    ];

    // Self-check the vector table against the formula in plain Rust
    // arithmetic, independent of either engine — the two engines agreeing
    // with each other is meaningless if the shared vector table itself is
    // wrong.
    for (order_ref, remaining, expected) in VECTORS {
        let bp = (((24 - remaining) * 10_000) / 24).clamp(0, 10_000);
        assert_eq!(bp, *expected, "vector table self-check for {order_ref}");
    }

    // --- Oracle: hand-built Program, empty FnLibrary (clamp resolves via
    // the new `builtin_total` fallback in `brix-oracle/src/eval.rs`). ---
    let oracle_program = OracleProgram::new()
        .with_relation(RelationDef::ground(
            "Order",
            &["ref", "remaining"],
            &["ref"],
        ))
        .with_relation(RelationDef::derived("Risk", &["order", "risk"], &["order"]))
        .with_rule(rule(
            "Risk",
            "Risk",
            &[("order", var("o")), ("risk", var("risk"))],
            vec![
                edge_bind(
                    "o",
                    "Order",
                    &[("ref", var("orderRef")), ("remaining", var("r"))],
                ),
                let_("risk", oracle_risk_expr()),
            ],
        ));
    let mut store = OracleStore::new(oracle_program).expect("program is stratified");
    let mut txn = OracleTxn::new(b"brix-stdin-0".to_vec());
    for (order_ref, remaining, _) in VECTORS {
        txn = txn.assert(
            "Order",
            row(&[
                ("ref", Value::Str((*order_ref).into())),
                ("remaining", Value::Int(*remaining)),
            ]),
        );
    }
    let settled = store.commit(&txn).expect("transaction commits");
    let oracle_hex = hex(&dump_bytes(settled));

    // --- Generated runtime: hand-built `engine::Program`, no `builtin_partial`
    // involved — only `Div` and the `brix.math.clamp` `builtin_total`. ---
    let mut rt_program = rt::Program::default();
    rt_program.relations.insert(
        "Order".to_string(),
        rt::Relation {
            name: "Order".to_string(),
            kind: rt::RelationKind::Ground,
            roles: vec!["ref".to_string(), "remaining".to_string()],
            key: vec!["ref".to_string()],
            open: false,
        },
    );
    rt_program.relations.insert(
        "Risk".to_string(),
        rt::Relation {
            name: "Risk".to_string(),
            kind: rt::RelationKind::Derived,
            roles: vec!["order".to_string(), "risk".to_string()],
            key: vec!["order".to_string()],
            open: false,
        },
    );
    rt_program.rules.insert(
        "Risk".to_string(),
        rt::Rule {
            id: "Risk".to_string(),
            phase: 0,
            head: rt::Head::Tuple {
                relation: "Risk".to_string(),
                args: vec![
                    ("order".to_string(), rt::Term::Var("o".to_string())),
                    ("risk".to_string(), rt::Term::Var("risk".to_string())),
                ],
            },
            body: vec![
                rt::Clause::Edge {
                    relation: "Order".to_string(),
                    bind_id: Some("o".to_string()),
                    args: vec![
                        ("ref".to_string(), rt::Term::Var("orderRef".to_string())),
                        ("remaining".to_string(), rt::Term::Var("r".to_string())),
                    ],
                },
                rt::Clause::Let("risk".to_string(), rt_risk_expr()),
            ],
        },
    );
    let mut stream = String::new();
    for (order_ref, remaining, _) in VECTORS {
        stream.push_str(&format!(
            "assert Order ref=str:{order_ref},remaining=int:{remaining}\n"
        ));
    }
    let out = rt::run_text(rt_program, &stream).expect("generated runtime runs the transaction");
    let rt_hex = out
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("run_text emits a canonical dump line");

    assert_eq!(
        rt_hex, oracle_hex,
        "div + clamp must settle identically on both engines for riskModel's reconciled vectors"
    );
}

/// `(24 - remaining) * 10000 / 24` — the oracle side, `remaining` bound by
/// the rule body's `edge_bind` to `r`.
fn oracle_risk_expr() -> OracleExpr {
    OracleExpr::Call(
        "brix.math.clamp".to_string(),
        vec![
            binop(
                OracleBinOp::Div,
                binop(
                    OracleBinOp::Mul,
                    binop(
                        OracleBinOp::Sub,
                        OracleExpr::Const(Value::Int(24)),
                        evar("r"),
                    ),
                    OracleExpr::Const(Value::Int(10_000)),
                ),
                OracleExpr::Const(Value::Int(24)),
            ),
            OracleExpr::Const(Value::Int(0)),
            OracleExpr::Const(Value::Int(10_000)),
        ],
    )
}

/// Same formula as [`oracle_risk_expr`], built for the generated runtime's
/// own `engine::Expr`/`engine::BinOp`.
fn rt_risk_expr() -> rt::Expr {
    rt::Expr::Call(
        "brix.math.clamp".to_string(),
        vec![
            rt::Expr::BinOp(
                rt::BinOp::Div,
                Box::new(rt::Expr::BinOp(
                    rt::BinOp::Mul,
                    Box::new(rt::Expr::BinOp(
                        rt::BinOp::Sub,
                        Box::new(rt::Expr::Const(rt::Value::Int(24))),
                        Box::new(rt::Expr::Var("r".to_string())),
                    )),
                    Box::new(rt::Expr::Const(rt::Value::Int(10_000))),
                )),
                Box::new(rt::Expr::Const(rt::Value::Int(24))),
            ),
            rt::Expr::Const(rt::Value::Int(0)),
            rt::Expr::Const(rt::Value::Int(10_000)),
        ],
    )
}

/// Diagnostic codes raised by lowering + checking `src`.
fn codes(src: &str) -> Vec<String> {
    let (file, diags) = parse_file(src);
    let lowered = lower_file(&file, &diags);
    lowered.diags.iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn fn_body_type_error_fails_closed() {
    // Body computes `Int`, but the return type is declared `Bool`.
    let src = "package t @ 0.1.0\nfn f(x: Int) -> Bool = x + 1\n";
    assert!(
        codes(src).iter().any(|c| c == "BRX-IR-0005"),
        "a fn body whose type mismatches its return type must fail: {:?}",
        codes(src)
    );
}

#[test]
fn fn_body_effect_violation_fails_closed() {
    // `f` declares no effects but calls `noisy`, whose row carries `console`.
    let src = "package t @ 0.1.0\n\
        fn noisy(x: Int) -> Int ! { console } = x\n\
        fn f(x: Int) -> Int = noisy(x)\n";
    assert!(
        codes(src).iter().any(|c| c == "BRX-IR-0011"),
        "a total fn realizing an undeclared effect must fail: {:?}",
        codes(src)
    );
}

#[test]
fn total_fn_using_try_fails_closed() {
    // `f` is total but uses `?` over a partial fn — only a `partial fn` may fail.
    let src = "package t @ 0.1.0\n\
        partial fn risky(x: Int) -> Result<Int, Int> = risky(x)\n\
        fn f(x: Int) -> Int = risky(x)?\n";
    assert!(
        codes(src).iter().any(|c| c == "BRX-IR-0012"),
        "a total fn that can fail (`?`) must fail closed: {:?}",
        codes(src)
    );
}

// --- Issue #47 Part 3 (Slice 3a): partial-fn body lowering -------------

/// A `partial fn` now lowers its body into a Core IR `FnDef` (was deferred /
/// hand-registered). `Probability.try(x)` is the builtin validated constructor,
/// typed `Result<Probability, ValidationError>`; the fn is marked `is_partial`.
/// This proves the body lowers cleanly and enters the program (execution is a
/// later slice).
#[test]
fn partial_fn_lowers_into_source_functions() {
    let src = "package t @ 0.1.0\n\
partial fn clampProb(x: Float) -> Result<Probability, ValidationError> = Probability.try(x)\n";
    let (file, diags) = parse_file(src);
    assert!(!diags.has_errors(), "fixture parses: {diags:#?}");
    let lowered = lower_file(&file, &diags);
    assert!(
        !lowered.has_errors(),
        "a partial fn body must lower cleanly: {:#?}",
        lowered.diags
    );
    let f = lowered
        .source
        .functions
        .iter()
        .find(|f| f.name.to_string() == "clampProb")
        .expect("clampProb must lower into source.functions, not be deferred");
    assert!(f.is_partial, "clampProb must be marked partial");
}
