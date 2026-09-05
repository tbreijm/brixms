//! `brix` — the Brix Alpha CLI driver (ADR-0010, ADR-0026, ADR-0030).
//!
//! Subcommands:
//! - `brix check <file.brix> [--json] [--package-path <dir>...]`
//! - `brix run <file.brix> [--json] [--package-path <dir>...]`
//! - `brix audit <file.brix> --bundle <out> [--force] [--json] [--package-path <dir>...]`
//! - `brix verify --expect-program <hex> <file.brix> <bundle> [--profile <finite-decision|l3-v1>] [--json] [--package-path <dir>...]`
//! - `brix why <file.brix> --candidate <name> [--json] [--package-path <dir>...]`
//! - `brix whynot <file.brix> --candidate <name> [--json] [--package-path <dir>...]`
//! - `brix help` / `brix --help` / `brix -h`
//! - `brix version` / `brix --version` / `brix -V` / `brix -v`
//!
//! Exit codes:
//! - 0: Accepted or verified. Help and version exit 0.
//! - 1: Rejected or Unknown.
//! - 2: Usage error or IO error.

use std::process::ExitCode;

pub mod cli;
pub mod commands;
pub mod json;
pub mod packages;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit_val = run_with_args(args);
    ExitCode::from(exit_val)
}

/// Run the CLI with the provided arguments and return the exit code byte.
pub fn run_with_args<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    match cli::parse_args(args) {
        Ok(cli::Command::Help) => {
            cli::print_help();
            cli::EXIT_SUCCESS
        }
        Ok(cli::Command::Version) => {
            cli::print_version();
            cli::EXIT_SUCCESS
        }
        Ok(cli::Command::Check {
            file,
            json,
            package_paths,
        }) => commands::check::execute_check(&file, json, &package_paths),
        Ok(cli::Command::Run {
            file,
            json,
            package_paths,
        }) => commands::run::execute_run(&file, json, &package_paths),
        Ok(cli::Command::Audit {
            file,
            bundle_out,
            force,
            json,
            package_paths,
        }) => commands::audit::execute_audit(&file, &bundle_out, force, json, &package_paths),
        Ok(cli::Command::Verify {
            expect_program,
            file,
            bundle,
            profile,
            json,
            package_paths,
        }) => commands::verify::execute_verify(
            &expect_program,
            &file,
            &bundle,
            profile,
            json,
            &package_paths,
        ),
        Ok(cli::Command::Why {
            file,
            candidate,
            json,
            package_paths,
        }) => commands::why::execute_why_or_whynot(&file, &candidate, json, &package_paths, false),
        Ok(cli::Command::WhyNot {
            file,
            candidate,
            json,
            package_paths,
        }) => commands::why::execute_why_or_whynot(&file, &candidate, json, &package_paths, true),
        Err(err) => {
            if err.is_json {
                let cmd_name = err.command.unwrap_or_else(|| "brix".to_string());
                let res = json::CliResultJson::failure(
                    cmd_name,
                    None,
                    None,
                    None,
                    "usage-error",
                    vec![err.message],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix: {}", err.message);
            }
            cli::EXIT_USAGE_OR_IO
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_and_version_exit_success() {
        assert_eq!(run_with_args(["--help"]), cli::EXIT_SUCCESS);
        assert_eq!(run_with_args(["--version"]), cli::EXIT_SUCCESS);

        // Extra help/version subcommands and short aliases exit 2
        assert_eq!(run_with_args(["help"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["version"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["-h"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["-V"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["-v"]), cli::EXIT_USAGE_OR_IO);
    }

    #[test]
    fn test_unknown_and_missing_arguments_exit_usage_2() {
        // No arguments
        assert_eq!(run_with_args(Vec::<&str>::new()), cli::EXIT_USAGE_OR_IO);

        // Unknown subcommand
        assert_eq!(run_with_args(["nonexistent"]), cli::EXIT_USAGE_OR_IO);

        // Prove command removed
        assert_eq!(run_with_args(["prove", "foo.brix"]), cli::EXIT_USAGE_OR_IO);

        // Run with --profile rejected (L3 v1 run not exposed)
        assert_eq!(
            run_with_args(["run", "--profile", "l3-v1", "foo.brix"]),
            cli::EXIT_USAGE_OR_IO
        );

        // Missing required operands
        assert_eq!(run_with_args(["check"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["run"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["audit", "f.brix"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(
            run_with_args(["verify", "f.brix", "b.bin"]),
            cli::EXIT_USAGE_OR_IO
        );
        assert_eq!(run_with_args(["why", "f.brix"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["whynot", "f.brix"]), cli::EXIT_USAGE_OR_IO);
    }

    #[test]
    fn test_json_flag_usage_error_exits_2() {
        assert_eq!(run_with_args(["check", "--json"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["verify", "--json"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["--json"]), cli::EXIT_USAGE_OR_IO);
    }
}
