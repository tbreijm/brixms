//! brix-cli — new/build/run/repl/test/sim/fmt/why/whynot/explain verbs.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! Only `build`/`run` are wired to real logic (issue #9); every other verb
//! is a legible "not yet implemented" rather than silently accepted.

use brix_cli::args::{parse, Invocation, ParsedArgs};
use brix_diag::DiagnosticFormat;

const EXIT_SUCCESS: i32 = 0;
const EXIT_FAILURE: i32 = 1;
const EXIT_USAGE: i32 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Invocation::Help) => print_help(),
        Ok(Invocation::Version) => println!("brix {}", env!("CARGO_PKG_VERSION")),
        Ok(Invocation::Verb(p)) => std::process::exit(dispatch(&p)),
        Err(e) => {
            eprintln!("brix: {e}");
            std::process::exit(EXIT_USAGE);
        }
    }
}

fn dispatch(p: &ParsedArgs) -> i32 {
    match p.verb.as_str() {
        "build" => run_build(p),
        "run" => run_run(p),
        other => {
            eprintln!("brix: `{other}` is not yet implemented (try `brix --help`)");
            EXIT_USAGE
        }
    }
}

fn run_build(p: &ParsedArgs) -> i32 {
    let format = match diagnostic_format(p) {
        Ok(format) => format,
        Err(message) => {
            eprintln!("brix build: {message}");
            return EXIT_USAGE;
        }
    };
    let Some(operand) = p.operand() else {
        eprintln!("brix build: expected a source file or package path");
        return EXIT_USAGE;
    };
    match brix_cli::build::build(operand, profile_from(p)) {
        Ok(outcome) => {
            println!("brix: built {}", outcome.binary_path);
            EXIT_SUCCESS
        }
        Err(e) => {
            report_error("build", &e, format);
            EXIT_FAILURE
        }
    }
}

fn run_run(p: &ParsedArgs) -> i32 {
    let format = match diagnostic_format(p) {
        Ok(format) => format,
        Err(message) => {
            eprintln!("brix run: {message}");
            return EXIT_USAGE;
        }
    };
    let Some(operand) = p.operand() else {
        eprintln!("brix run: expected a source file or package path");
        return EXIT_USAGE;
    };
    match brix_cli::build::run(operand) {
        Ok(code) => code,
        Err(e) => {
            report_error("run", &e, format);
            EXIT_FAILURE
        }
    }
}

fn diagnostic_format(p: &ParsedArgs) -> Result<DiagnosticFormat, String> {
    match p.value("diagnostic-format") {
        None => Ok(DiagnosticFormat::Human),
        Some(value) => DiagnosticFormat::parse(value).ok_or_else(|| {
            format!("unsupported diagnostic format `{value}` (expected human, json, or sarif)")
        }),
    }
}

fn report_error(command: &str, error: &brix_cli::build::BuildError, format: DiagnosticFormat) {
    match format {
        DiagnosticFormat::Human => eprintln!("brix {command}: {error}"),
        DiagnosticFormat::Json | DiagnosticFormat::Sarif => println!("{}", error.render(format)),
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
         \x20\x20brix run <path>     Build, then execute the produced binary\n\
         \n\
         Build/run accept --diagnostic-format human|json|sarif. Exit codes: 0 success,\n\
         1 build/runtime failure, 2 command-line usage error.\n"
    );
}
