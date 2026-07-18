//! Issue #47 Slice 1: a total, expression-bodied user function is compiled
//! from its BrixMS source, type/effect/totality-checked, and **executed** —
//! identically on the oracle and the generated runtime — with **no**
//! hand-registered native implementation on either side.
//!
//! `surcharge`/`riskModel` stay hand-registered for now (they need unit-value
//! scaling / partial-result + runtime provenance, deferred to Slice 2); this
//! fixture is the unit-free proof that the whole pipeline — lower → check →
//! project → execute → oracle==generated — works end to end.

use brix_ast::parse_file;
use brix_oracle::dsl::row;
use brix_oracle::dump::dump_bytes;
use brix_oracle::frontend::{program_from_source, FnLibrary, KindTable};
use brix_oracle::program::RelKind;
use brix_oracle::store::Store as OracleStore;
use brix_oracle::txn::Transaction as OracleTxn;
use brix_oracle::value::Value;
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
