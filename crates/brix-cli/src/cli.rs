//! Command-line argument parsing and invocation structure.

use std::path::PathBuf;

/// Exit codes according to the alpha CLI specification:
/// - Exit 0: accepted or verified. Help and version exit 0.
/// - Exit 1: rejected or Unknown.
/// - Exit 2: usage error or IO error.
pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_REJECTED_OR_UNKNOWN: u8 = 1;
pub const EXIT_USAGE_OR_IO: u8 = 2;

/// Parsed CLI subcommand invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Check {
        file: PathBuf,
        json: bool,
        package_paths: Vec<PathBuf>,
    },
    Run {
        file: PathBuf,
        json: bool,
        package_paths: Vec<PathBuf>,
    },
    Audit {
        file: PathBuf,
        bundle_out: PathBuf,
        force: bool,
        json: bool,
        package_paths: Vec<PathBuf>,
    },
    Verify {
        expect_program: String,
        file: PathBuf,
        bundle: PathBuf,
        profile: VerifyProfile,
        json: bool,
        package_paths: Vec<PathBuf>,
    },
    Why {
        file: PathBuf,
        candidate: String,
        json: bool,
        package_paths: Vec<PathBuf>,
    },
    WhyNot {
        file: PathBuf,
        candidate: String,
        json: bool,
        package_paths: Vec<PathBuf>,
    },
    Help,
    Version,
}

/// Verification profile option for `brix verify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyProfile {
    FiniteDecision,
    L3V1,
}

impl VerifyProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FiniteDecision => "finite-decision",
            Self::L3V1 => "l3-v1",
        }
    }
}

/// CLI parsing error (categorized as usage 2 or IO 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliUsageError {
    pub message: String,
    pub is_json: bool,
    pub command: Option<String>,
}

impl CliUsageError {
    pub fn usage(message: impl Into<String>, is_json: bool, command: Option<String>) -> Self {
        Self {
            message: message.into(),
            is_json,
            command,
        }
    }
}

/// Parse command-line arguments into a [`Command`] or return a usage error (exit code 2).
pub fn parse_args<I, T>(args: I) -> Result<Command, CliUsageError>
where
    I: IntoIterator<Item = T>,
    T: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let json_requested = args.iter().any(|a| a == "--json");

    if args.is_empty() {
        return Err(CliUsageError::usage(
            "usage: brix <check|run|audit|verify|why|whynot> [options] [operands]\nRun 'brix --help' for usage details.",
            json_requested,
            None,
        ));
    }

    // Exact surface: only --help and --version globally
    let first = &args[0];
    if first == "--help" {
        if args.len() > 1 {
            return Err(CliUsageError::usage(
                "unexpected arguments after '--help'",
                json_requested,
                None,
            ));
        }
        return Ok(Command::Help);
    }
    if first == "--version" {
        if args.len() > 1 {
            return Err(CliUsageError::usage(
                "unexpected arguments after '--version'",
                json_requested,
                None,
            ));
        }
        return Ok(Command::Version);
    }

    // Reject extra help/version subcommands and short aliases
    if first == "help" || first == "version" {
        return Err(CliUsageError::usage(
            format!("unknown command: '{first}'\nRun 'brix --help' for usage details."),
            json_requested,
            Some(first.clone()),
        ));
    }
    if first == "-h" || first == "-V" || first == "-v" {
        return Err(CliUsageError::usage(
            format!("unknown option: '{first}'\nRun 'brix --help' for usage details."),
            json_requested,
            None,
        ));
    }

    let subcmd = first.as_str();
    let subcmd_args = &args[1..];

    match subcmd {
        "check" => parse_check_args(subcmd_args, json_requested),
        "run" => parse_run_args(subcmd_args, json_requested),
        "audit" => parse_audit_args(subcmd_args, json_requested),
        "verify" => parse_verify_args(subcmd_args, json_requested),
        "why" => parse_why_args(subcmd_args, json_requested, false),
        "whynot" => parse_why_args(subcmd_args, json_requested, true),
        _ => Err(CliUsageError::usage(
            format!("unknown command: '{subcmd}'\nRun 'brix --help' for usage details."),
            json_requested,
            Some(subcmd.to_string()),
        )),
    }
}

fn parse_check_args(args: &[String], json: bool) -> Result<Command, CliUsageError> {
    let mut positionals = Vec::new();
    let mut package_paths = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            // Handled
        } else if arg == "--package-path" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--package-path'",
                    json,
                    Some("check".to_string()),
                ));
            }
            package_paths.push(PathBuf::from(&args[i]));
        } else if let Some(stripped) = arg.strip_prefix("--package-path=") {
            package_paths.push(PathBuf::from(stripped));
        } else if arg.starts_with('-') {
            return Err(CliUsageError::usage(
                format!("unknown option: '{arg}'"),
                json,
                Some("check".to_string()),
            ));
        } else {
            positionals.push(PathBuf::from(arg));
        }
        i += 1;
    }

    if positionals.is_empty() {
        return Err(CliUsageError::usage(
            "missing required file operand for 'brix check'",
            json,
            Some("check".to_string()),
        ));
    }
    if positionals.len() > 1 {
        return Err(CliUsageError::usage(
            format!("unexpected extra operand: '{}'", positionals[1].display()),
            json,
            Some("check".to_string()),
        ));
    }

    Ok(Command::Check {
        file: positionals.remove(0),
        json,
        package_paths,
    })
}

fn parse_run_args(args: &[String], json: bool) -> Result<Command, CliUsageError> {
    let mut positionals = Vec::new();
    let mut package_paths = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            // Handled
        } else if arg == "--package-path" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--package-path'",
                    json,
                    Some("run".to_string()),
                ));
            }
            package_paths.push(PathBuf::from(&args[i]));
        } else if let Some(stripped) = arg.strip_prefix("--package-path=") {
            package_paths.push(PathBuf::from(stripped));
        } else if arg.starts_with('-') {
            return Err(CliUsageError::usage(
                format!("unknown option: '{arg}'"),
                json,
                Some("run".to_string()),
            ));
        } else {
            positionals.push(PathBuf::from(arg));
        }
        i += 1;
    }

    if positionals.is_empty() {
        return Err(CliUsageError::usage(
            "missing required file operand for 'brix run'",
            json,
            Some("run".to_string()),
        ));
    }
    if positionals.len() > 1 {
        return Err(CliUsageError::usage(
            format!("unexpected extra operand: '{}'", positionals[1].display()),
            json,
            Some("run".to_string()),
        ));
    }

    Ok(Command::Run {
        file: positionals.remove(0),
        json,
        package_paths,
    })
}

fn parse_audit_args(args: &[String], json: bool) -> Result<Command, CliUsageError> {
    let mut positionals = Vec::new();
    let mut package_paths = Vec::new();
    let mut bundle_out: Option<PathBuf> = None;
    let mut force = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            // Handled
        } else if arg == "--force" {
            force = true;
        } else if arg == "--bundle" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--bundle'",
                    json,
                    Some("audit".to_string()),
                ));
            }
            bundle_out = Some(PathBuf::from(&args[i]));
        } else if let Some(stripped) = arg.strip_prefix("--bundle=") {
            bundle_out = Some(PathBuf::from(stripped));
        } else if arg == "--package-path" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--package-path'",
                    json,
                    Some("audit".to_string()),
                ));
            }
            package_paths.push(PathBuf::from(&args[i]));
        } else if let Some(stripped) = arg.strip_prefix("--package-path=") {
            package_paths.push(PathBuf::from(stripped));
        } else if arg.starts_with('-') {
            return Err(CliUsageError::usage(
                format!("unknown option: '{arg}'"),
                json,
                Some("audit".to_string()),
            ));
        } else {
            positionals.push(PathBuf::from(arg));
        }
        i += 1;
    }

    if positionals.is_empty() {
        return Err(CliUsageError::usage(
            "missing required file operand for 'brix audit'",
            json,
            Some("audit".to_string()),
        ));
    }
    if positionals.len() > 1 {
        return Err(CliUsageError::usage(
            format!("unexpected extra operand: '{}'", positionals[1].display()),
            json,
            Some("audit".to_string()),
        ));
    }

    let bundle_out = match bundle_out {
        Some(b) => b,
        None => {
            return Err(CliUsageError::usage(
                "missing required '--bundle <out>' option for 'brix audit'",
                json,
                Some("audit".to_string()),
            ));
        }
    };

    Ok(Command::Audit {
        file: positionals.remove(0),
        bundle_out,
        force,
        json,
        package_paths,
    })
}

fn parse_verify_args(args: &[String], json: bool) -> Result<Command, CliUsageError> {
    let mut positionals = Vec::new();
    let mut package_paths = Vec::new();
    let mut expect_program: Option<String> = None;
    let mut profile = VerifyProfile::FiniteDecision;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            // Handled
        } else if arg == "--expect-program" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--expect-program'",
                    json,
                    Some("verify".to_string()),
                ));
            }
            expect_program = Some(args[i].clone());
        } else if let Some(stripped) = arg.strip_prefix("--expect-program=") {
            expect_program = Some(stripped.to_string());
        } else if arg == "--profile" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--profile'",
                    json,
                    Some("verify".to_string()),
                ));
            }
            match args[i].as_str() {
                "finite-decision" => profile = VerifyProfile::FiniteDecision,
                "l3-v1" => profile = VerifyProfile::L3V1,
                other => {
                    return Err(CliUsageError::usage(
                        format!(
                            "invalid profile: '{other}' (expected 'finite-decision' or 'l3-v1')"
                        ),
                        json,
                        Some("verify".to_string()),
                    ));
                }
            }
        } else if let Some(stripped) = arg.strip_prefix("--profile=") {
            match stripped {
                "finite-decision" => profile = VerifyProfile::FiniteDecision,
                "l3-v1" => profile = VerifyProfile::L3V1,
                other => {
                    return Err(CliUsageError::usage(
                        format!(
                            "invalid profile: '{other}' (expected 'finite-decision' or 'l3-v1')"
                        ),
                        json,
                        Some("verify".to_string()),
                    ));
                }
            }
        } else if arg == "--package-path" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--package-path'",
                    json,
                    Some("verify".to_string()),
                ));
            }
            package_paths.push(PathBuf::from(&args[i]));
        } else if let Some(stripped) = arg.strip_prefix("--package-path=") {
            package_paths.push(PathBuf::from(stripped));
        } else if arg.starts_with('-') {
            return Err(CliUsageError::usage(
                format!("unknown option: '{arg}'"),
                json,
                Some("verify".to_string()),
            ));
        } else {
            positionals.push(PathBuf::from(arg));
        }
        i += 1;
    }

    let expect_program = match expect_program {
        Some(pin) => {
            if pin.len() != 64 || !pin.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(CliUsageError::usage(
                    format!("invalid hex for '--expect-program': expected 64 hex characters, found '{pin}'"),
                    json,
                    Some("verify".to_string()),
                ));
            }
            pin
        }
        None => {
            return Err(CliUsageError::usage(
                "missing required '--expect-program <hex>' option for 'brix verify'",
                json,
                Some("verify".to_string()),
            ));
        }
    };

    if positionals.len() < 2 {
        return Err(CliUsageError::usage(
            "missing required operands for 'brix verify': requires FILE and BUNDLE",
            json,
            Some("verify".to_string()),
        ));
    }
    if positionals.len() > 2 {
        return Err(CliUsageError::usage(
            format!("unexpected extra operand: '{}'", positionals[2].display()),
            json,
            Some("verify".to_string()),
        ));
    }

    let bundle = positionals.pop().unwrap();
    let file = positionals.pop().unwrap();

    Ok(Command::Verify {
        expect_program,
        file,
        bundle,
        profile,
        json,
        package_paths,
    })
}

fn parse_why_args(args: &[String], json: bool, is_whynot: bool) -> Result<Command, CliUsageError> {
    let cmd_name = if is_whynot { "whynot" } else { "why" };
    let mut positionals = Vec::new();
    let mut package_paths = Vec::new();
    let mut candidate: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--json" {
            // Handled
        } else if arg == "--candidate" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--candidate'",
                    json,
                    Some(cmd_name.to_string()),
                ));
            }
            candidate = Some(args[i].clone());
        } else if let Some(stripped) = arg.strip_prefix("--candidate=") {
            candidate = Some(stripped.to_string());
        } else if arg == "--package-path" {
            i += 1;
            if i >= args.len() {
                return Err(CliUsageError::usage(
                    "missing argument for '--package-path'",
                    json,
                    Some(cmd_name.to_string()),
                ));
            }
            package_paths.push(PathBuf::from(&args[i]));
        } else if let Some(stripped) = arg.strip_prefix("--package-path=") {
            package_paths.push(PathBuf::from(stripped));
        } else if arg.starts_with('-') {
            return Err(CliUsageError::usage(
                format!("unknown option: '{arg}'"),
                json,
                Some(cmd_name.to_string()),
            ));
        } else {
            positionals.push(PathBuf::from(arg));
        }
        i += 1;
    }

    if positionals.is_empty() {
        return Err(CliUsageError::usage(
            format!("missing required file operand for 'brix {cmd_name}'"),
            json,
            Some(cmd_name.to_string()),
        ));
    }
    if positionals.len() > 1 {
        return Err(CliUsageError::usage(
            format!("unexpected extra operand: '{}'", positionals[1].display()),
            json,
            Some(cmd_name.to_string()),
        ));
    }

    let candidate = match candidate {
        Some(c) => c,
        None => {
            return Err(CliUsageError::usage(
                format!("missing required '--candidate <name>' option for 'brix {cmd_name}'"),
                json,
                Some(cmd_name.to_string()),
            ));
        }
    };

    let file = positionals.remove(0);
    if is_whynot {
        Ok(Command::WhyNot {
            file,
            candidate,
            json,
            package_paths,
        })
    } else {
        Ok(Command::Why {
            file,
            candidate,
            json,
            package_paths,
        })
    }
}

pub fn print_help() {
    println!(
        "\
brix — the Brix toolchain command-line driver

Usage: brix <command> [options] [operands]

Commands:
  check <file.brix> [--json] [--package-path <dir>...]
      Parse, resolve imports, and check a Brix module. Runs profile preflight for finite-decision.

  run <file.brix> [--json] [--package-path <dir>...]
      Execute a finite-decision deliberation plan to completion.

  audit <file.brix> --bundle <out> [--force] [--json] [--package-path <dir>...]
      Run and audit a finite-decision plan, producing an audit input bundle on success.

  verify --expect-program <hex> <file.brix> <bundle> [--profile <finite-decision|l3-v1>] [--json] [--package-path <dir>...]
      Verify an audit input bundle against source and external expected program pin.

  why <file.brix> --candidate <name> [--json] [--package-path <dir>...]
      Explain why a candidate was admitted or selected in deliberation.

  whynot <file.brix> --candidate <name> [--json] [--package-path <dir>...]
      Explain why a candidate was not admitted or not selected in deliberation.

Global Options:
  --help       Print help information
  --version    Print version information
"
    );
}

pub fn print_version() {
    println!("brix {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_PIN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn test_global_help_forms() {
        let cmd = parse_args(["--help"]).expect("--help should parse");
        assert_eq!(cmd, Command::Help);

        // Extra subcommand or short alias rejected
        assert!(parse_args(["help"]).is_err());
        assert!(parse_args(["-h"]).is_err());
    }

    #[test]
    fn test_global_version_forms() {
        let cmd = parse_args(["--version"]).expect("--version should parse");
        assert_eq!(cmd, Command::Version);

        // Extra subcommand or short alias rejected
        assert!(parse_args(["version"]).is_err());
        assert!(parse_args(["-V"]).is_err());
        assert!(parse_args(["-v"]).is_err());
    }

    #[test]
    fn test_subcommand_help_and_version_rejected() {
        // Subcommands reject help/version options and subcommands
        assert!(parse_args(["check", "--help"]).is_err());
        assert!(parse_args(["run", "-h"]).is_err());
        assert!(parse_args(["audit", "-V"]).is_err());
        assert!(parse_args(["verify", "--version"]).is_err());
    }

    #[test]
    fn test_check_command_shapes() {
        // Plain check
        let cmd = parse_args(["check", "program.brix"]).unwrap();
        assert_eq!(
            cmd,
            Command::Check {
                file: PathBuf::from("program.brix"),
                json: false,
                package_paths: vec![],
            }
        );

        // Check with json flag after operand
        let cmd = parse_args(["check", "program.brix", "--json"]).unwrap();
        assert_eq!(
            cmd,
            Command::Check {
                file: PathBuf::from("program.brix"),
                json: true,
                package_paths: vec![],
            }
        );

        // Check with json flag before operand
        let cmd = parse_args(["check", "--json", "program.brix"]).unwrap();
        assert_eq!(
            cmd,
            Command::Check {
                file: PathBuf::from("program.brix"),
                json: true,
                package_paths: vec![],
            }
        );

        // Check with repeatable package-path
        let cmd = parse_args([
            "check",
            "program.brix",
            "--package-path",
            "/dir1",
            "--package-path=/dir2",
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::Check {
                file: PathBuf::from("program.brix"),
                json: false,
                package_paths: vec![PathBuf::from("/dir1"), PathBuf::from("/dir2")],
            }
        );
    }

    #[test]
    fn test_run_command_shapes() {
        let cmd = parse_args(["run", "plan.brix"]).unwrap();
        assert_eq!(
            cmd,
            Command::Run {
                file: PathBuf::from("plan.brix"),
                json: false,
                package_paths: vec![],
            }
        );

        let cmd = parse_args(["run", "plan.brix", "--json", "--package-path", "pkgs"]).unwrap();
        assert_eq!(
            cmd,
            Command::Run {
                file: PathBuf::from("plan.brix"),
                json: true,
                package_paths: vec![PathBuf::from("pkgs")],
            }
        );
    }

    #[test]
    fn test_audit_command_shapes() {
        // Audit requiring bundle
        let cmd = parse_args(["audit", "delib.brix", "--bundle", "bundle.bin"]).unwrap();
        assert_eq!(
            cmd,
            Command::Audit {
                file: PathBuf::from("delib.brix"),
                bundle_out: PathBuf::from("bundle.bin"),
                force: false,
                json: false,
                package_paths: vec![],
            }
        );

        // Audit with long flags and --json
        let cmd = parse_args([
            "audit",
            "delib.brix",
            "--bundle",
            "bundle.bin",
            "--force",
            "--json",
            "--package-path",
            "p1",
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::Audit {
                file: PathBuf::from("delib.brix"),
                bundle_out: PathBuf::from("bundle.bin"),
                force: true,
                json: true,
                package_paths: vec![PathBuf::from("p1")],
            }
        );

        // Audit with --bundle=OUT syntax
        let cmd = parse_args(["audit", "delib.brix", "--bundle=out.bin", "--force"]).unwrap();
        assert_eq!(
            cmd,
            Command::Audit {
                file: PathBuf::from("delib.brix"),
                bundle_out: PathBuf::from("out.bin"),
                force: true,
                json: false,
                package_paths: vec![],
            }
        );

        // Short flags -b and -f are rejected
        assert!(parse_args(["audit", "delib.brix", "-b", "bundle.bin"]).is_err());
        assert!(parse_args(["audit", "delib.brix", "--bundle", "b.bin", "-f"]).is_err());
    }

    #[test]
    fn test_verify_command_shapes() {
        // Verify default profile (finite-decision)
        let cmd = parse_args([
            "verify",
            "--expect-program",
            VALID_PIN,
            "source.brix",
            "input.bundle",
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::Verify {
                expect_program: VALID_PIN.to_string(),
                file: PathBuf::from("source.brix"),
                bundle: PathBuf::from("input.bundle"),
                profile: VerifyProfile::FiniteDecision,
                json: false,
                package_paths: vec![],
            }
        );

        // Verify with explicit --profile l3-v1
        let cmd = parse_args([
            "verify",
            "--expect-program",
            VALID_PIN,
            "source.brix",
            "input.bundle",
            "--profile",
            "l3-v1",
            "--json",
            "--package-path",
            "root_dir",
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::Verify {
                expect_program: VALID_PIN.to_string(),
                file: PathBuf::from("source.brix"),
                bundle: PathBuf::from("input.bundle"),
                profile: VerifyProfile::L3V1,
                json: true,
                package_paths: vec![PathBuf::from("root_dir")],
            }
        );

        // Verify with --profile=finite-decision
        let cmd = parse_args([
            "verify",
            &format!("--expect-program={VALID_PIN}"),
            "--profile=finite-decision",
            "source.brix",
            "input.bundle",
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::Verify {
                expect_program: VALID_PIN.to_string(),
                file: PathBuf::from("source.brix"),
                bundle: PathBuf::from("input.bundle"),
                profile: VerifyProfile::FiniteDecision,
                json: false,
                package_paths: vec![],
            }
        );

        // Short flag -e rejected
        assert!(parse_args(["verify", "-e", VALID_PIN, "source.brix", "input.bundle",]).is_err());
    }

    #[test]
    fn test_why_and_whynot_command_shapes() {
        // why
        let cmd = parse_args(["why", "module.brix", "--candidate", "step_one"]).unwrap();
        assert_eq!(
            cmd,
            Command::Why {
                file: PathBuf::from("module.brix"),
                candidate: "step_one".to_string(),
                json: false,
                package_paths: vec![],
            }
        );

        // why with --json
        let cmd = parse_args([
            "why",
            "module.brix",
            "--candidate",
            "step_one",
            "--json",
            "--package-path",
            "pkgs",
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::Why {
                file: PathBuf::from("module.brix"),
                candidate: "step_one".to_string(),
                json: true,
                package_paths: vec![PathBuf::from("pkgs")],
            }
        );

        // Short flag -c rejected
        assert!(parse_args(["why", "module.brix", "-c", "step_one"]).is_err());

        // whynot
        let cmd = parse_args(["whynot", "module.brix", "--candidate=step_two"]).unwrap();
        assert_eq!(
            cmd,
            Command::WhyNot {
                file: PathBuf::from("module.brix"),
                candidate: "step_two".to_string(),
                json: false,
                package_paths: vec![],
            }
        );
    }

    #[test]
    fn test_missing_operands_classification() {
        // Empty args
        let err = parse_args(Vec::<&str>::new()).unwrap_err();
        assert!(err.message.contains("usage: brix"));

        // Missing file on check
        let err = parse_args(["check"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required file operand for 'brix check'"));

        // Missing file on run
        let err = parse_args(["run"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required file operand for 'brix run'"));

        // Missing file on audit
        let err = parse_args(["audit", "--bundle", "out.bin"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required file operand for 'brix audit'"));

        // Missing bundle on audit
        let err = parse_args(["audit", "file.brix"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required '--bundle <out>' option"));

        // Missing expect-program on verify
        let err = parse_args(["verify", "file.brix", "bundle.bin"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required '--expect-program <hex>' option"));

        // Missing bundle operand on verify
        let err = parse_args(["verify", "--expect-program", VALID_PIN, "file.brix"]).unwrap_err();
        assert!(err.message.contains("requires FILE and BUNDLE"));

        // Missing candidate on why
        let err = parse_args(["why", "file.brix"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required '--candidate <name>' option"));

        // Missing candidate on whynot
        let err = parse_args(["whynot", "file.brix"]).unwrap_err();
        assert!(err
            .message
            .contains("missing required '--candidate <name>' option"));
    }

    #[test]
    fn test_invalid_operands_and_flags() {
        // Extra operand on check
        let err = parse_args(["check", "f1.brix", "f2.brix"]).unwrap_err();
        assert!(err.message.contains("unexpected extra operand"));

        // Extra operand on run
        let err = parse_args(["run", "f1.brix", "f2.brix"]).unwrap_err();
        assert!(err.message.contains("unexpected extra operand"));

        // Extra operand on verify
        let err = parse_args([
            "verify",
            "--expect-program",
            VALID_PIN,
            "f.brix",
            "b.bin",
            "extra",
        ])
        .unwrap_err();
        assert!(err.message.contains("unexpected extra operand"));

        // Unknown option on check
        let err = parse_args(["check", "f.brix", "--bogus"]).unwrap_err();
        assert!(err.message.contains("unknown option: '--bogus'"));

        // Unknown subcommand
        let err = parse_args(["not-a-command"]).unwrap_err();
        assert!(err.message.contains("unknown command: 'not-a-command'"));

        // Invalid hex for expect-program (too short)
        let err =
            parse_args(["verify", "--expect-program", "1234abcd", "f.brix", "b.bin"]).unwrap_err();
        assert!(err.message.contains("invalid hex"));

        // Invalid hex for expect-program (non-hex characters)
        let non_hex = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        let err =
            parse_args(["verify", "--expect-program", non_hex, "f.brix", "b.bin"]).unwrap_err();
        assert!(err.message.contains("invalid hex"));

        // Invalid profile for verify
        let err = parse_args([
            "verify",
            "--expect-program",
            VALID_PIN,
            "--profile",
            "unsupported",
            "f.brix",
            "b.bin",
        ])
        .unwrap_err();
        assert!(err.message.contains("invalid profile: 'unsupported'"));
    }

    #[test]
    fn test_prove_command_removed() {
        // Must refuse 'prove' as an unknown command
        let err = parse_args(["prove", "module.brix"]).unwrap_err();
        assert!(err.message.contains("unknown command: 'prove'"));
    }

    #[test]
    fn test_l3_v1_run_not_exposed() {
        // 'run' does not expose --profile or L3 v1 run
        let err = parse_args(["run", "--profile", "l3-v1", "module.brix"]).unwrap_err();
        assert!(err.message.contains("unknown option: '--profile'"));
    }

    #[test]
    fn test_missing_option_arguments() {
        let err = parse_args(["check", "f.brix", "--package-path"]).unwrap_err();
        assert!(err
            .message
            .contains("missing argument for '--package-path'"));

        let err = parse_args(["audit", "f.brix", "--bundle"]).unwrap_err();
        assert!(err.message.contains("missing argument for '--bundle'"));

        let err = parse_args(["why", "f.brix", "--candidate"]).unwrap_err();
        assert!(err.message.contains("missing argument for '--candidate'"));

        let err = parse_args(["verify", "--expect-program"]).unwrap_err();
        assert!(err
            .message
            .contains("missing argument for '--expect-program'"));

        let err = parse_args([
            "verify",
            "--expect-program",
            VALID_PIN,
            "f.brix",
            "b.bin",
            "--profile",
        ])
        .unwrap_err();
        assert!(err.message.contains("missing argument for '--profile'"));
    }

    #[test]
    fn test_json_flag_recognized_in_usage_error() {
        let err = parse_args(["check", "--json"]).unwrap_err();
        assert!(err.is_json);
        assert_eq!(err.command, Some("check".to_string()));

        let err2 = parse_args(["unknown-cmd", "--json"]).unwrap_err();
        assert!(err2.is_json);

        let err3 = parse_args(["--json"]).unwrap_err();
        assert!(err3.is_json);
    }
}
