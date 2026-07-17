//! brix-cli — new/build/run/repl/test/sim/fmt/why/whynot/explain verbs.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! Only `build`/`run` are wired to real logic (issue #9); every other verb
//! is a legible "not yet implemented" rather than silently accepted.

use brix_cli::args::{parse, Invocation, ParsedArgs};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Invocation::Help) => print_help(),
        Ok(Invocation::Version) => println!("brix {}", env!("CARGO_PKG_VERSION")),
        Ok(Invocation::Verb(p)) => std::process::exit(dispatch(&p)),
        Err(e) => {
            eprintln!("brix: {e}");
            std::process::exit(1);
        }
    }
}

fn dispatch(p: &ParsedArgs) -> i32 {
    match p.verb.as_str() {
        "build" => run_build(p),
        "run" => run_run(p),
        other => {
            eprintln!("brix: `{other}` is not yet implemented (try `brix --help`)");
            1
        }
    }
}

fn run_build(p: &ParsedArgs) -> i32 {
    let Some(operand) = p.operand() else {
        eprintln!("brix build: expected a source file or package path");
        return 1;
    };
    match brix_cli::build::build(operand, profile_from(p)) {
        Ok(outcome) => {
            println!("brix: built {}", outcome.binary_path);
            0
        }
        Err(e) => {
            eprintln!("brix build: {e}");
            1
        }
    }
}

fn run_run(p: &ParsedArgs) -> i32 {
    let Some(operand) = p.operand() else {
        eprintln!("brix run: expected a source file or package path");
        return 1;
    };
    match brix_cli::build::run(operand) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("brix run: {e}");
            1
        }
    }
}

fn profile_from(p: &ParsedArgs) -> brixc::Profile {
    match p.value("profile") {
        Some("serve") => brixc::Profile::Serve,
        _ => brixc::Profile::Run,
    }
}

fn print_help() {
    println!(
        "brix: BrixMS toolchain (Ring 0)\n\n\
         Usage:\n\
         \x20\x20brix build <path>   Compile a package/source file to a Rust workspace\n\
         \x20\x20brix run <path>     Build, then execute the produced binary\n"
    );
}
