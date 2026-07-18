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

    let (oracle_dump, stream) = flagship_oracle_dump_and_stream(FLAGSHIP);
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
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
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
    std::fs::remove_dir_all(&root).ok();
}

fn flagship_oracle_dump_and_stream(source: &str) -> (Vec<u8>, String) {
    let (file, diagnostics) = parse_file(source);
    let lowered = brixc::lower_file(&file, &diagnostics);
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
    let program = program_from_source(&lowered.source, &lowered.resolver, &kinds, FnLibrary::new())
        .expect("flagship must adapt to the oracle");
    let location = Value::Node(
        program.relations["Location"].node_id(&row(&[("code", Value::Str("AMS".into()))])),
    );
    let client = Value::Node(
        program.relations["Client"].node_id(&row(&[("code", Value::Str("ACME".into()))])),
    );
    let order = Value::Node(
        program.relations["Order"].node_id(&row(&[("ref", Value::Str("ord-1".into()))])),
    );
    let transaction = Transaction::new(b"brix-stdin-0".to_vec())
        .ensure("Location", row(&[("code", Value::Str("AMS".into()))]))
        .ensure(
            "Client",
            row(&[
                ("code", Value::Str("ACME".into())),
                (
                    "tier",
                    Value::Enum {
                        ty: "Tier".into(),
                        ordinal: 1,
                        name: "Key".into(),
                    },
                ),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-1".into())),
                ("client", client.clone()),
                ("from", location.clone()),
                ("to", location.clone()),
                ("weight", Value::Nat(2_000)),
                ("due", Value::Int(6)),
            ]),
        )
        .set(
            "OrderStatus",
            row(&[
                ("order", order.clone()),
                (
                    "value",
                    Value::Enum {
                        ty: "Status".into(),
                        ordinal: 0,
                        name: "Open".into(),
                    },
                ),
            ]),
        );
    let mut store = Store::new(program).expect("flagship phases must assign");
    let dump = dump_bytes(
        store
            .commit(&transaction)
            .expect("flagship transaction commits"),
    );
    let node = |value: &Value| match value {
        Value::Node(id) => id.digest().to_hex(),
        _ => unreachable!(),
    };
    let stream = format!(
        "ensure Location code=str:AMS\nensure Client code=str:ACME,tier=enum:Tier#1\nensure Order ref=str:ord-1,client=node:{client},from=node:{location},to=node:{location},weight=nat:2000,due=int:6\nset OrderStatus order=node:{order},value=enum:Status#0\n",
        client = node(&client),
        location = node(&location),
        order = node(&order),
    );
    (dump, stream)
}
