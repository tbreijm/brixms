//! brix-cli — new/build/run/repl/test/sim/fmt/why/whynot/explain verbs.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! `check`/`fmt`/`build`/`run`/`test`/`quality` are wired to real logic;
//! every other verb is a legible "not yet implemented" rather than accepted.

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
        "check" => run_check(p),
        "fmt" => run_fmt(p),
        "test" => run_test(p),
        "quality" => run_quality(p),
        "build" => run_build(p),
        "run" => run_run(p),
        other => {
            eprintln!("brix: `{other}` is not yet implemented (try `brix --help`)");
            EXIT_USAGE
        }
    }
}

fn run_test(p: &ParsedArgs) -> i32 {
    let format = match diagnostic_format(p) {
        Ok(format) => format,
        Err(message) => {
            eprintln!("brix test: {message}");
            return EXIT_USAGE;
        }
    };
    let Some(operand) = p.operand() else {
        eprintln!("brix test: expected a source file or package path");
        return EXIT_USAGE;
    };
    if p.options
        .keys()
        .any(|key| key.as_str() != "diagnostic-format")
    {
        eprintln!("brix test: unsupported option");
        return EXIT_USAGE;
    }

    let selectors = p.positionals[1..].to_vec();
    match brix_cli::test::run(operand, &selectors) {
        Ok(outcome) => {
            println!(
                "brix: {} tests passed for {}",
                outcome.passed, outcome.source_path
            );
            EXIT_SUCCESS
        }
        Err(error) => {
            report_error("test", &error, format);
            EXIT_FAILURE
        }
    }
}

fn run_check(p: &ParsedArgs) -> i32 {
    let format = match diagnostic_format(p) {
        Ok(format) => format,
        Err(message) => {
            eprintln!("brix check: {message}");
            return EXIT_USAGE;
        }
    };
    let Some(operand) = p.operand() else {
        eprintln!("brix check: expected a source file or package path");
        return EXIT_USAGE;
    };
    match brix_cli::build::check(operand) {
        Ok(outcome) => {
            println!("brix: checked {}", outcome.source_path);
            EXIT_SUCCESS
        }
        Err(e) => {
            report_error("check", &e, format);
            EXIT_FAILURE
        }
    }
}

fn run_fmt(p: &ParsedArgs) -> i32 {
    if p.flag("write") && p.flag("check") {
        eprintln!("brix fmt: --write and --check are mutually exclusive");
        return EXIT_USAGE;
    }
    let Some(operand) = p.operand() else {
        eprintln!("brix fmt: expected a source file or package path");
        return EXIT_USAGE;
    };
    match brix_cli::build::format_all(operand) {
        Ok(outcomes) if p.flag("check") => {
            let unformatted: Vec<&str> = outcomes
                .iter()
                .filter(|o| o.changed)
                .map(|o| o.source_path.as_str())
                .collect();
            if unformatted.is_empty() {
                EXIT_SUCCESS
            } else {
                for path in &unformatted {
                    eprintln!("brix fmt: {path} is not canonically formatted");
                }
                EXIT_FAILURE
            }
        }
        Ok(outcomes) if p.flag("write") => {
            let mut ok = true;
            for outcome in outcomes.into_iter().filter(|o| o.changed) {
                match std::fs::write(&outcome.source_path, outcome.formatted) {
                    Ok(()) => println!("brix: formatted {}", outcome.source_path),
                    Err(error) => {
                        eprintln!("brix fmt: I/O error: {error}");
                        ok = false;
                    }
                }
            }
            if ok {
                EXIT_SUCCESS
            } else {
                EXIT_FAILURE
            }
        }
        Ok(outcomes) => {
            for outcome in &outcomes {
                print!("{}", outcome.formatted);
            }
            EXIT_SUCCESS
        }
        Err(e) => {
            report_error("fmt", &e, DiagnosticFormat::Human);
            EXIT_FAILURE
        }
    }
}

fn run_quality(p: &ParsedArgs) -> i32 {
    let format = match diagnostic_format(p) {
        Ok(format) => format,
        Err(message) => {
            eprintln!("brix quality: {message}");
            return EXIT_USAGE;
        }
    };
    let profile = match p.value("profile") {
        None => brix_cli::quality::QualityProfile::Standard,
        Some(value) => match brix_cli::quality::QualityProfile::parse(value) {
            Some(profile) => profile,
            None => {
                eprintln!(
                    "brix quality: unsupported profile `{value}` (expected prototype, standard, production, or critical)"
                );
                return EXIT_USAGE;
            }
        },
    };
    let Some(operand) = p.operand() else {
        eprintln!("brix quality: expected a source file or package path");
        return EXIT_USAGE;
    };
    if p.positionals.len() != 1 {
        eprintln!("brix quality: expected exactly one source file or package path");
        return EXIT_USAGE;
    }
    if p.options
        .keys()
        .any(|key| !matches!(key.as_str(), "profile" | "diagnostic-format"))
    {
        eprintln!("brix quality: unsupported option");
        return EXIT_USAGE;
    }

    match brix_cli::quality::evaluate(operand, profile) {
        Ok(outcome) => {
            println!(
                "brix: quality {} passed for {}",
                outcome.profile.as_str(),
                outcome.source_path
            );
            EXIT_SUCCESS
        }
        Err(error) => {
            report_error("quality", &error, format);
            EXIT_FAILURE
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
         \x20\x20brix check <path>   Parse and run static/semantic checks\n\
         \x20\x20brix fmt <path>     Print canonical source (--write or --check)\n\
         \x20\x20brix test <path> [selector ...]  Check, then execute selected tests\n\
         \x20\x20brix quality <path> Run compiler checks and the selected quality profile\n\
         \x20\x20brix build <path>   Compile a package/source file to a Rust workspace\n\
         \x20\x20brix run <path>     Build, then execute the produced binary\n\
         \n\
         Check/test/quality/build/run accept --diagnostic-format human|json|sarif. Quality accepts\n\
         --profile prototype|standard|production|critical (default: standard). Exit codes: 0 success,\n\
         1 build/runtime failure, 2 command-line usage error.\n"
    );
}
