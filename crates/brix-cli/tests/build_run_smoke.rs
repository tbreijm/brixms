//! Acceptance test (issue #9): `brix build path/to/world.brix` produces a
//! Rust workspace that compiles; `brix run` executes it; a warm rebuild is
//! a cache hit. Drives the real `brix` binary as a subprocess — this is
//! the only way to exercise `main.rs`'s dispatch and the real `cargo`
//! invocation end to end.

use std::io::Write;
use std::process::{Command, Stdio};

use brix_ast::parse_file;
use brix_oracle::dsl::row;
use brix_oracle::dump::dump_bytes;
use brix_oracle::frontend::{program_from_source, FnLibrary, KindTable};
use brix_oracle::program::RelKind as OracleRelKind;
use brix_oracle::program::{Head, Program, RelationDef, Rule, Term};
use brix_oracle::store::Store;
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;
use brixc::lower::RuntimeRelationKind;
use brixc::pipeline::PhaseAssign;
use brixc::AstPhase;
use camino::Utf8PathBuf;

const FIXTURE: &str = "package smoke.build @ 0.1.0\n\
\n\
rel Input { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive R: Output(value: value) from { Input(value) }\n";

const UNSUPPORTED_FIXTURE: &str = "package smoke.unsupported @ 0.1.0\n\
rel Input { value: I64 } key(value)\n\
rel Other { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive Join: Output(value: value) from { Input(value); Other(value) }\n";

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut p =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("system temp dir must be UTF-8");
    p.push(format!(
        "brix-cli-smoke-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn brix(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

#[test]
fn build_then_run_then_cache_hit() {
    let root = tmp_dir("build-run");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, FIXTURE).unwrap();

    // 1. `brix build` on a bare file, no brix.toml alongside it — exercises
    //    the PackageDecl-synthesis path directly.
    let build_out = brix(&["build", source_path.as_str()]);
    assert!(
        build_out.status.success(),
        "brix build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr),
    );

    let cache_root = root.join(".brix-cache");
    let hex_dir = std::fs::read_dir(&cache_root)
        .unwrap_or_else(|e| panic!("no .brix-cache dir at {cache_root}: {e}"))
        .next()
        .expect("at least one cache entry")
        .unwrap()
        .path();
    let hex_dir = Utf8PathBuf::from_path_buf(hex_dir).unwrap();
    for f in ["Cargo.toml", "src/generated.rs", "src/main.rs"] {
        assert!(hex_dir.join(f).exists(), "missing generated file: {f}");
    }

    // 2. `brix run` on the same path — builds (cache hit this time) and
    //    executes the harness binary, whose fixed marker line must appear.
    let run_out = brix(&["run", source_path.as_str()]);
    assert!(
        run_out.status.success(),
        "brix run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr),
    );
    assert!(String::from_utf8_lossy(&run_out.stdout).contains("brix: generated workspace OK"));
    assert!(String::from_utf8_lossy(&run_out.stderr).contains("cache hit"));

    // 3. A second `brix build` is the concrete, assertable form of "a warm
    //    rebuild is a cache hit."
    let rebuild_out = brix(&["build", source_path.as_str()]);
    assert!(rebuild_out.status.success());
    assert!(String::from_utf8_lossy(&rebuild_out.stderr).contains("cache hit"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn diagnostic_formats_and_exit_codes_are_public_contract() {
    let root = tmp_dir("diagnostics");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("broken.brix");
    std::fs::write(&source_path, "package broken @ 0.1.0\nrel Input {").unwrap();

    let json = brix(&["build", source_path.as_str(), "--diagnostic-format", "json"]);
    assert_eq!(json.status.code(), Some(1));
    let json_output = String::from_utf8_lossy(&json.stdout);
    assert!(
        json_output.starts_with("{\"diagnostics\":"),
        "{json_output}"
    );
    assert!(json_output.contains("BRX-AST-"), "{json_output}");

    let check_json = brix(&["check", source_path.as_str(), "--diagnostic-format", "json"]);
    assert_eq!(check_json.status.code(), Some(1));
    let check_output = String::from_utf8_lossy(&check_json.stdout);
    assert!(
        check_output.starts_with("{\"diagnostics\":"),
        "{check_output}"
    );
    assert!(check_output.contains("BRX-AST-"), "{check_output}");

    let sarif = brix(&["build", source_path.as_str(), "--diagnostic-format=sarif"]);
    assert_eq!(sarif.status.code(), Some(1));
    let sarif_output = String::from_utf8_lossy(&sarif.stdout);
    assert!(
        sarif_output.contains("\"version\":\"2.1.0\""),
        "{sarif_output}"
    );
    assert!(sarif_output.contains("BRX-AST-"), "{sarif_output}");

    let usage = brix(&["build"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("expected a source file"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn check_is_non_emitting_and_fmt_supports_check_and_write() {
    let root = tmp_dir("check-fmt");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    let unformatted = "package smoke.format @ 0.1.0\nrel Input {value:I64} key(value)\n";
    std::fs::write(&source_path, unformatted).unwrap();

    let checked = brix(&["check", source_path.as_str()]);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(!root.join(".brix-cache").exists());

    let fmt_check = brix(&["fmt", source_path.as_str(), "--check"]);
    assert_eq!(fmt_check.status.code(), Some(1));

    let fmt_write = brix(&["fmt", source_path.as_str(), "--write"]);
    assert!(fmt_write.status.success());
    let formatted = std::fs::read_to_string(&source_path).unwrap();
    assert_ne!(formatted, unformatted);

    let fmt_check = brix(&["fmt", source_path.as_str(), "--check"]);
    assert!(fmt_check.status.success());
    assert!(!root.join(".brix-cache").exists());

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_accepts_a_join_rule_for_the_native_program_runtime() {
    let root = tmp_dir("unsupported-runtime-rule");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, UNSUPPORTED_FIXTURE).unwrap();

    let output = brix(&["build", source_path.as_str()]);
    assert!(
        output.status.success(),
        "brix build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn generated_binary_consumes_a_transaction_stream_and_emits_canon_dump() {
    let root = tmp_dir("runtime-stream");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, FIXTURE).unwrap();

    let build_out = brix(&["build", source_path.as_str()]);
    assert!(build_out.status.success());
    let cache_entry = std::fs::read_dir(root.join(".brix-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let binary = Utf8PathBuf::from_path_buf(cache_entry)
        .unwrap()
        .join("target")
        .join("debug")
        .join(format!("smoke_build{}", std::env::consts::EXE_SUFFIX));

    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"assert Input value=int:7\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("1 "), "{stdout}");
    assert!(stdout.contains("brix: generated workspace OK"), "{stdout}");

    let native_hex = stdout
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("native dump line must carry canonical bytes");
    assert_eq!(native_hex, hex(&oracle_identity_dump()));

    std::fs::remove_dir_all(&root).ok();
}

fn oracle_identity_dump() -> Vec<u8> {
    let program = Program::new()
        .with_relation(RelationDef::ground("Input", &["value"], &["value"]))
        .with_relation(RelationDef::derived("Output", &["value"], &["value"]))
        .with_relation(RelationDef::ground("brix.sim.Now", &["at"], &[]))
        .with_rule(Rule {
            id: "R".into(),
            head: Head::Tuple {
                rel: "Output".into(),
                args: vec![("value".into(), Term::Var("value".into()))],
            },
            body: vec![brix_oracle::program::Clause::Edge {
                rel: "Input".into(),
                bind_id: None,
                args: vec![("value".into(), Term::Var("value".into()))],
            }],
        });
    let mut store = Store::new(program).expect("identity program is stratified");
    let settled = store
        .commit(
            &Transaction::new(b"brix-stdin-0".to_vec())
                .assert("Input", row(&[("value", Value::Int(7))])),
        )
        .expect("identity transaction commits");
    dump_bytes(settled)
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

/// The issue #10 acceptance criterion, in full: not just "the compiled
/// flagship's dump matches the oracle's" on a trivial one-relation
/// transaction, but a multi-relation revision that actually exercises
/// masking (`Override` masking `ComputedPrice` behind a `ManualPrice`) and
/// a second revision that a `strict` constraint (`Capacity`) correctly
/// rejects. This is the same driving data as
/// `crates/brix-oracle/tests/flagship.rs`'s revision 1/2 (issue #24),
/// replayed here against the *compiled* binary instead of the oracle
/// directly, over the real generated transaction-stream protocol.
#[test]
fn compiled_flagship_transaction_dump_matches_the_oracle() {
    const FLAGSHIP: &str =
        include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix");
    let (file, diagnostics) = parse_file(FLAGSHIP);
    assert!(!diagnostics.has_errors());
    let lowered = brixc::lower_file(&file, &diagnostics);
    assert!(!lowered.has_errors());
    AstPhase
        .assign_phases(lowered)
        .expect("flagship phases must assign");

    let root = tmp_dir("flagship-oracle");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, FLAGSHIP).unwrap();
    let build = brix(&["build", source_path.as_str()]);
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let cache_entry = std::fs::read_dir(root.join(".brix-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let binary = Utf8PathBuf::from_path_buf(cache_entry)
        .unwrap()
        .join("target")
        .join("debug")
        .join(format!("demo_logistics{}", std::env::consts::EXE_SUFFIX));

    // --- Revision 1: masking, via the compiled binary --------------------
    let (oracle_dump, stream_rev1) = flagship_oracle_dump_and_stream(FLAGSHIP);
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream_rev1.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let native_hex = stdout
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("compiled flagship must emit a canonical dump");
    assert_eq!(native_hex, hex(&oracle_dump));

    // --- Revision 2: a `Capacity`-violating order must reject the whole
    //     stream (the generated binary only prints output on full success —
    //     see `emit::workspace::main_rs`) with no partial output leaked, and
    //     snapshot isolation is then just "revision 1 was already proven
    //     above and revision 2 never printed anything to contradict it".
    let stream_rev1_then_2 = format!("{stream_rev1}\n{}", flagship_capacity_violation_stream());
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream_rev1_then_2.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "a Capacity-violating revision must not succeed"
    );
    assert!(
        output.stdout.is_empty(),
        "a rejected revision must leak no prior output: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("strict constraint violated"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&root).ok();
}

fn flagship_kinds(lowered: &brixc::Lowered) -> KindTable {
    let mut kinds = KindTable::new();
    for relation in lowered.resolver.relations() {
        if relation.derived {
            continue;
        }
        let kind = match lowered.resolver.relation_kind(&relation.name) {
            RuntimeRelationKind::Entity => OracleRelKind::Entity,
            RuntimeRelationKind::Ground => OracleRelKind::Ground,
            RuntimeRelationKind::State => OracleRelKind::State,
            RuntimeRelationKind::Event => OracleRelKind::Event,
        };
        kinds.insert(relation.name.to_string(), kind);
    }
    kinds
}

fn node_hex(program: &Program, rel: &str, key_row: brix_oracle::row::Row) -> String {
    program.relations[rel].node_id(&key_row).digest().to_hex()
}

// `surcharge` is compiled from BrixMS source (issue #47 Slice 1.5) and runs via
// `Program::fn_defs`, so it is no longer hand-registered. `riskModel` remains
// hand-transcribed (still deferred) — same function, and same integer-floor
// fixed-point ruling (issue #47 Part 2), as
// `crates/brix-oracle/tests/flagship.rs` (issue #24).
fn risk_model(args: &[Value]) -> Result<Value, Value> {
    let due = args[0].as_i128().expect("riskModel: non-numeric due");
    let now = args[1].as_i128().expect("riskModel: non-numeric now");
    let remaining = due - now;
    let risk = if remaining <= 0 {
        10_000i64
    } else {
        (((24 - remaining).clamp(0, 24) * 10_000) / 24) as i64
    };
    Ok(Value::Int(risk))
}

fn fn_library() -> FnLibrary {
    FnLibrary::new().with_partial_fn("riskModel", risk_model)
}

fn flagship_oracle_dump_and_stream(source: &str) -> (Vec<u8>, String) {
    let (file, diagnostics) = parse_file(source);
    let lowered = brixc::lower_file(&file, &diagnostics);
    let kinds = flagship_kinds(&lowered);
    let program = program_from_source(&lowered.source, &lowered.resolver, &kinds, fn_library())
        .expect("flagship must adapt to the oracle");

    let node = |rel: &str, key_row: brix_oracle::row::Row| {
        Value::Node(program.relations[rel].node_id(&key_row))
    };
    let ams = node("Location", row(&[("code", Value::Str("AMS".into()))]));
    let rtm = node("Location", row(&[("code", Value::Str("RTM".into()))]));
    let acme = node("Client", row(&[("code", Value::Str("ACME".into()))]));
    let v01 = node("Vehicle", row(&[("plate", Value::Str("V-01".into()))]));
    let v02 = node("Vehicle", row(&[("plate", Value::Str("V-02".into()))]));
    let tariff_standard = node("Tariff", row(&[("class", vehicle_class("Standard", 1))]));
    let tariff_suv = node("Tariff", row(&[("class", vehicle_class("SUV", 2))]));
    let ord1 = node("Order", row(&[("ref", Value::Str("ord-1".into()))]));
    let ord2 = node("Order", row(&[("ref", Value::Str("ord-2".into()))]));
    let ord3 = node("Order", row(&[("ref", Value::Str("ord-3".into()))]));
    let hex_of = |value: &Value| match value {
        Value::Node(id) => id.digest().to_hex(),
        _ => unreachable!(),
    };

    let transaction = Transaction::new(b"brix-stdin-0".to_vec())
        .ensure("Location", row(&[("code", Value::Str("AMS".into()))]))
        .ensure("Location", row(&[("code", Value::Str("RTM".into()))]))
        .ensure(
            "Client",
            row(&[
                ("code", Value::Str("ACME".into())),
                ("tier", tier("Key", 1)),
            ]),
        )
        .ensure(
            "Vehicle",
            row(&[
                ("plate", Value::Str("V-01".into())),
                ("class", vehicle_class("Standard", 1)),
                ("capacity", Value::Nat(2_000)),
            ]),
        )
        .ensure(
            "Vehicle",
            row(&[
                ("plate", Value::Str("V-02".into())),
                ("class", vehicle_class("SUV", 2)),
                ("capacity", Value::Nat(3_500)),
            ]),
        )
        .ensure("Tariff", row(&[("class", vehicle_class("Standard", 1))]))
        .ensure("Tariff", row(&[("class", vehicle_class("SUV", 2))]))
        .set(
            "TariffRate",
            row(&[
                ("tariff", tariff_standard.clone()),
                ("rate", Value::Int(120)),
            ]),
        )
        .set(
            "TariffRate",
            row(&[("tariff", tariff_suv.clone()), ("rate", Value::Int(165))]),
        )
        .assert(
            "Distance",
            row(&[
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("length", Value::Nat(78)),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-1".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(1_500)),
                ("due", Value::Nat(20)),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-2".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(3_000)),
                ("due", Value::Nat(100)),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-3".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(800)),
                ("due", Value::Nat(2)),
            ]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord1.clone()), ("value", status("Open", 0))]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord2.clone()), ("value", status("Open", 0))]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord3.clone()), ("value", status("Open", 0))]),
        )
        .assert(
            "AssignOrder.Chosen",
            row(&[("order", ord1.clone()), ("vehicle", v01.clone())]),
        )
        .assert(
            "AssignOrder.Chosen",
            row(&[("order", ord2.clone()), ("vehicle", v02.clone())]),
        )
        // Overrides ord-2's computed price (exercises Override/masking).
        .set(
            "ManualPrice",
            row(&[("order", ord2.clone()), ("amount", Value::Int(9_500))]),
        )
        .assert("brix.sim.Now", row(&[("at", Value::Nat(12))]));

    let mut store = Store::new(program).expect("flagship phases must assign");
    let dump = dump_bytes(
        store
            .commit(&transaction)
            .expect("flagship revision 1 transaction commits"),
    );

    let (ams, rtm, acme, v01, v02, tariff_standard, tariff_suv, ord1, ord2, ord3) = (
        hex_of(&ams),
        hex_of(&rtm),
        hex_of(&acme),
        hex_of(&v01),
        hex_of(&v02),
        hex_of(&tariff_standard),
        hex_of(&tariff_suv),
        hex_of(&ord1),
        hex_of(&ord2),
        hex_of(&ord3),
    );
    let stream = format!(
        "ensure Location code=str:AMS\n\
         ensure Location code=str:RTM\n\
         ensure Client code=str:ACME,tier=enum:Tier#1\n\
         ensure Vehicle plate=str:V-01,class=enum:VehicleClass#1,capacity=nat:2000\n\
         ensure Vehicle plate=str:V-02,class=enum:VehicleClass#2,capacity=nat:3500\n\
         ensure Tariff class=enum:VehicleClass#1\n\
         ensure Tariff class=enum:VehicleClass#2\n\
         set TariffRate tariff=node:{tariff_standard},rate=int:120\n\
         set TariffRate tariff=node:{tariff_suv},rate=int:165\n\
         assert Distance from=node:{ams},to=node:{rtm},length=nat:78\n\
         ensure Order ref=str:ord-1,client=node:{acme},from=node:{ams},to=node:{rtm},weight=nat:1500,due=nat:20\n\
         ensure Order ref=str:ord-2,client=node:{acme},from=node:{ams},to=node:{rtm},weight=nat:3000,due=nat:100\n\
         ensure Order ref=str:ord-3,client=node:{acme},from=node:{ams},to=node:{rtm},weight=nat:800,due=nat:2\n\
         set OrderStatus order=node:{ord1},value=enum:Status#0\n\
         set OrderStatus order=node:{ord2},value=enum:Status#0\n\
         set OrderStatus order=node:{ord3},value=enum:Status#0\n\
         assert AssignOrder.Chosen order=node:{ord1},vehicle=node:{v01}\n\
         assert AssignOrder.Chosen order=node:{ord2},vehicle=node:{v02}\n\
         set ManualPrice order=node:{ord2},amount=int:9500\n\
         assert brix.sim.Now at=nat:12\n",
    );
    (dump, stream)
}

/// A revision-2 stream for the same flagship program: an order whose weight
/// exceeds every vehicle's capacity, assigned anyway — `Capacity strict`
/// must reject the whole stream (`CommitError::StrictViolation` on the
/// oracle side, matching `crates/brix-oracle/tests/flagship.rs`'s tx2).
/// Node hex references are recomputed fresh here (the fixture's node
/// identity is a pure function of relation name + key, so this need not
/// share state with `flagship_oracle_dump_and_stream`).
fn flagship_capacity_violation_stream() -> String {
    const FLAGSHIP: &str =
        include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix");
    let (file, diagnostics) = parse_file(FLAGSHIP);
    let lowered = brixc::lower_file(&file, &diagnostics);
    let kinds = flagship_kinds(&lowered);
    let program = program_from_source(&lowered.source, &lowered.resolver, &kinds, fn_library())
        .expect("flagship must adapt to the oracle");

    let ams = node_hex(
        &program,
        "Location",
        row(&[("code", Value::Str("AMS".into()))]),
    );
    let rtm = node_hex(
        &program,
        "Location",
        row(&[("code", Value::Str("RTM".into()))]),
    );
    let acme = node_hex(
        &program,
        "Client",
        row(&[("code", Value::Str("ACME".into()))]),
    );
    let v02 = node_hex(
        &program,
        "Vehicle",
        row(&[("plate", Value::Str("V-02".into()))]),
    );
    let ord4 = node_hex(
        &program,
        "Order",
        row(&[("ref", Value::Str("ord-4".into()))]),
    );

    format!(
        "ensure Order ref=str:ord-4,client=node:{acme},from=node:{ams},to=node:{rtm},weight=nat:5000,due=nat:200\n\
         set OrderStatus order=node:{ord4},value=enum:Status#0\n\
         assert AssignOrder.Chosen order=node:{ord4},vehicle=node:{v02}\n",
    )
}

fn tier(name: &str, ordinal: u32) -> Value {
    Value::Enum {
        ty: "Tier".into(),
        ordinal,
        name: name.into(),
    }
}

fn vehicle_class(name: &str, ordinal: u32) -> Value {
    Value::Enum {
        ty: "VehicleClass".into(),
        ordinal,
        name: name.into(),
    }
}

fn status(name: &str, ordinal: u32) -> Value {
    Value::Enum {
        ty: "Status".into(),
        ordinal,
        name: name.into(),
    }
}
