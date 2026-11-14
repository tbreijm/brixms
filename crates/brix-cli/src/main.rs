//! `brix` — the Brix Alpha CLI driver (ADR-0010, ADR-0026, ADR-0030).
//!
//! Subcommands:
//! - `brix check <file.brix> [--json] [--package-path <dir>...]`
//! - `brix run <file.brix> [--json] [--package-path <dir>...]`
//! - `brix audit <file.brix> --bundle <out> [--force] [--json] [--package-path <dir>...]`
//! - `brix verify --expect-program <hex> <file.brix> <bundle> [--profile <finite-decision|l3-v1>] [--json] [--package-path <dir>...]`
//! - `brix why <file.brix> --candidate <name> [--json] [--package-path <dir>...]`
//! - `brix whynot <file.brix> --candidate <name> [--json] [--package-path <dir>...]`
//! - `brix test <file.test.json>... [--json]`
//! - `brix kb <init|assert|retract|program|log|show|diff|audit|verify> ...` —
//!   the persistent, revisable knowledge base (ADR-0041); see `brix --help`.
//! - `brix help` / `brix --help` / `brix -h`
//! - `brix version` / `brix --version` / `brix -V` / `brix -v`
//!
//! Exit codes:
//! - 0: Accepted or verified. Help and version exit 0. For `test`, every case in every suite passed.
//! - 1: Rejected or Unknown. For `test`, at least one case's assertions did not match.
//! - 2: Usage error or IO error. For `test`, this also covers a malformed/oversized/empty suite
//!   file, or a program a suite names that cannot be read, parsed, import-resolved, or lowered.

use std::process::ExitCode;

pub mod cli;
pub mod commands;
pub mod host_context;
pub mod json;
pub mod named_query;
pub mod packages;
pub mod pipeline;
pub mod request_audit;
pub mod request_evidence;
pub mod serve;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Run the whole command on one evaluation thread, so every evaluation it
    // performs (including a whole `serve` session) runs inline instead of
    // spawning a bounded-stack thread per expression
    // (`brix_lower::l3_v2::with_eval_stack`).
    let exit_val = brix_lower::l3_v2::with_eval_stack(move || run_with_args(args))
        // The thread panicked (the panic message is already printed) or
        // could not be created; 101 is the exit code Rust uses for a panic.
        .unwrap_or(101);
    ExitCode::from(exit_val)
}

/// Run the CLI with the provided arguments and return the exit code byte.
pub fn run_with_args<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let raw_args: Vec<String> = args.into_iter().map(Into::into).collect();
    let json_out = raw_args.iter().any(|arg| arg == "--json");
    let (args, policy_path) = match extract_request_context_args(raw_args) {
        Ok(value) => value,
        Err(message) => return pipeline::invocation_evidence_error("brix", message, json_out),
    };
    let command = cli::parse_args(args);
    let supports_context = matches!(
        &command,
        Ok(cli::Command::Run { .. }
            | cli::Command::Why { .. }
            | cli::Command::WhyNot { .. }
            | cli::Command::Audit { .. }
            | cli::Command::Verify { .. })
    );
    if policy_path.is_some() && !supports_context {
        return pipeline::invocation_evidence_error(
            "brix",
            "--request-context is supported by run, why, whynot, audit and verify".into(),
            json_out,
        );
    }
    let invocation = if supports_context {
        let loaded = if let Some(path) = policy_path {
            load_invocation_policy(&path)
        } else {
            request_evidence::InvocationEvidenceContext::from_environment()
        };
        match loaded {
            Ok(context) => context,
            Err(message) => return pipeline::invocation_evidence_error("brix", message, json_out),
        }
    } else {
        request_evidence::InvocationEvidenceContext::empty()
    };
    match command {
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
            input_paths,
        }) => commands::check::execute_check(&file, json, &package_paths, &input_paths),
        Ok(cli::Command::Run {
            file,
            json,
            package_paths,
            input_paths,
        }) => commands::run::execute_run_with_context(
            &file,
            json,
            &package_paths,
            &input_paths,
            &invocation,
        ),
        Ok(cli::Command::Audit {
            file,
            bundle_out,
            force,
            json,
            package_paths,
            input_paths,
        }) => commands::audit::execute_audit_with_context(
            &file,
            &bundle_out,
            force,
            json,
            &package_paths,
            &input_paths,
            &invocation,
        ),
        Ok(cli::Command::Verify {
            expect_program,
            file,
            bundle,
            profile,
            json,
            package_paths,
            input_paths,
        }) => commands::verify::execute_verify_with_context(
            &expect_program,
            &file,
            &bundle,
            profile,
            json,
            &package_paths,
            &input_paths,
            &invocation,
        ),
        Ok(cli::Command::Why {
            file,
            candidate,
            entity,
            json,
            package_paths,
            input_paths,
        }) => commands::why::execute_why_or_whynot_with_context(
            &file,
            &candidate,
            entity,
            json,
            &package_paths,
            &input_paths,
            false,
            &invocation,
        ),
        Ok(cli::Command::WhyNot {
            file,
            candidate,
            entity,
            json,
            package_paths,
            input_paths,
        }) => commands::why::execute_why_or_whynot_with_context(
            &file,
            &candidate,
            entity,
            json,
            &package_paths,
            &input_paths,
            true,
            &invocation,
        ),
        Ok(cli::Command::Context {
            file,
            expect,
            store,
            profile,
            simulation,
            json,
        }) => commands::context::execute(
            &file,
            expect.as_deref(),
            store.as_deref(),
            &profile,
            simulation,
            json,
        ),
        Ok(cli::Command::Test { files, json }) => commands::test::execute_test(&files, json),
        Ok(cli::Command::Kb { op, json }) => commands::kb::execute_kb(&op, json),
        Ok(cli::Command::World { op, json }) => commands::world::execute_world(&op, json),
        Ok(cli::Command::Serve) => serve::run_stdio(),
        Ok(cli::Command::ServeHost) => serve::run_stdio_with_host(true),
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

fn extract_request_context_args(
    args: Vec<String>,
) -> Result<(Vec<String>, Option<std::path::PathBuf>), String> {
    let mut rest = Vec::new();
    let mut policy = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let path = if arg == "--request-context" {
            Some(
                args.next()
                    .ok_or("--request-context requires a host policy file")?,
            )
        } else {
            arg.strip_prefix("--request-context=").map(str::to_owned)
        };
        if let Some(path) = path {
            if path.is_empty() || policy.replace(std::path::PathBuf::from(path)).is_some() {
                return Err("--request-context must name exactly one host policy file".into());
            }
        } else {
            rest.push(arg);
        }
    }
    Ok((rest, policy))
}

fn load_invocation_policy(
    path: &std::path::Path,
) -> Result<request_evidence::InvocationEvidenceContext, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take((request_evidence::MAX_REQUEST_EVIDENCE_POLICY_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("cannot read host request policy: {error}"))?;
    if bytes.len() > request_evidence::MAX_REQUEST_EVIDENCE_POLICY_BYTES {
        return Err("host request policy exceeds byte limit".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "host request policy is not UTF-8")?;
    let value = host_context::parse_unique_json(text)?;
    request_evidence::InvocationEvidenceContext::from_host_policy_json(&value)
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

        // Missing required operand for test
        assert_eq!(run_with_args(["test"]), cli::EXIT_USAGE_OR_IO);
    }

    #[test]
    fn test_json_flag_usage_error_exits_2() {
        assert_eq!(run_with_args(["check", "--json"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["verify", "--json"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["test", "--json"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["--json"]), cli::EXIT_USAGE_OR_IO);
    }

    #[test]
    fn test_test_command_usage_errors_exit_2() {
        // Unknown option
        assert_eq!(
            run_with_args(["test", "suite.test.json", "--bogus"]),
            cli::EXIT_USAGE_OR_IO
        );

        // Subcommand help/version rejected like other subcommands
        assert_eq!(run_with_args(["test", "--help"]), cli::EXIT_USAGE_OR_IO);
        assert_eq!(run_with_args(["test", "-h"]), cli::EXIT_USAGE_OR_IO);

        // Missing suite file (nonexistent path) is an IO error, exit 2
        assert_eq!(
            run_with_args(["test", "/nonexistent/path/suite.test.json"]),
            cli::EXIT_USAGE_OR_IO
        );
    }
    #[test]
    fn invocation_policy_option_cannot_be_ambiguous_or_ignored() {
        assert!(
            extract_request_context_args(vec!["run".into(), "--request-context".into()]).is_err()
        );
        assert!(extract_request_context_args(vec![
            "--request-context=a".into(),
            "--request-context=b".into()
        ])
        .is_err());
        let (args, policy) = extract_request_context_args(vec![
            "run".into(),
            "--request-context=host.json".into(),
            "a.brix".into(),
        ])
        .unwrap();
        assert_eq!(args, vec!["run", "a.brix"]);
        assert_eq!(policy, Some(std::path::PathBuf::from("host.json")));
        assert_eq!(
            run_with_args(["serve", "--stdio", "--request-context=host.json"]),
            cli::EXIT_REJECTED_OR_UNKNOWN
        );
    }
}
