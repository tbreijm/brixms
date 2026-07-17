//! `brix build` / `brix run`: drive the `brixc` pipeline, write a
//! standalone generated Cargo workspace to disk, and shell out to `cargo`
//! — cached by `brixc::CacheKey` so a warm rebuild skips both.
//!
//! Real BrixMS semantic execution (the delta-driving loop) is not wired to
//! `brixc::emit`'s output anywhere in the repo yet (every `delta_from_*`
//! body is still `todo!()`) — "run executes it" here means the generated
//! crate compiles, links, and runs a real (if minimal) binary, not that it
//! simulates a BrixMS program. See `brixc::emit::assemble_workspace`'s
//! harness.

use std::collections::BTreeMap;
use std::fmt;
use std::process::Command;

use brix_ast::{parse_file, Diagnostic, Severity};
use brix_canon::{Digest, Domain};
use brixc::pipeline::PhaseAssign;
use brixc::{AstPhase, CacheInputs, CacheKey, PipelineError, Profile};
use camino::{Utf8Path, Utf8PathBuf};

use crate::{package, toolchain};

#[derive(Debug)]
pub enum BuildError {
    Locate(package::LocateError),
    Io(std::io::Error),
    ParseFailed(String),
    LowerFailed(String),
    Phase(PipelineError),
    Toolchain(toolchain::ToolchainError),
    CargoBuildFailed(std::process::ExitStatus),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Locate(e) => write!(f, "{e}"),
            BuildError::Io(e) => write!(f, "I/O error: {e}"),
            BuildError::ParseFailed(s) => write!(f, "{s}"),
            BuildError::LowerFailed(s) => write!(f, "{s}"),
            BuildError::Phase(e) => write!(f, "{e}"),
            BuildError::Toolchain(e) => write!(f, "{e}"),
            BuildError::CargoBuildFailed(status) => write!(f, "cargo build failed: {status}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<std::io::Error> for BuildError {
    fn from(e: std::io::Error) -> Self {
        BuildError::Io(e)
    }
}

/// The result of a successful `build`.
pub struct BuildOutcome {
    pub cache_dir: Utf8PathBuf,
    pub binary_path: Utf8PathBuf,
    pub cache_hit: bool,
}

/// Run the full pipeline for the package named by `operand` (parse -> lower
/// -> phase-assign -> emit), write the generated workspace, and `cargo
/// build` it — unless `brixc::CacheKey` says a matching build already
/// exists, in which case both are skipped entirely.
pub fn build(operand: &str, profile: Profile) -> Result<BuildOutcome, BuildError> {
    let located = package::locate(operand).map_err(BuildError::Locate)?;
    let source = std::fs::read_to_string(&located.source_path)?;

    let (file, parse_diags) = parse_file(&source);
    if parse_diags.has_errors() {
        return Err(BuildError::ParseFailed(
            parse_diags.render(&source, located.source_path.as_str()),
        ));
    }

    let lowered = brixc::lower_file(&file, &parse_diags);
    if lowered.has_errors() {
        return Err(BuildError::LowerFailed(format_diagnostics(&lowered.diags)));
    }

    let phased = AstPhase.assign_phases(lowered).map_err(BuildError::Phase)?;
    let (relations, rules) = brixc::emit::project(&phased.lowered);

    let crate_name = brixc::emit::sanitize_crate_name(located.manifest.name.as_str());

    let canonical_source = Digest::of(Domain::Value, brix_ast::format_file(&file).as_bytes());
    let lockfile = brixpkg::Lockfile {
        format_version: brixpkg::LOCK_FORMAT_VERSION,
        root: located.manifest.name.clone(),
        entries: BTreeMap::new(),
    };
    let toolchain = toolchain::detect().map_err(BuildError::Toolchain)?;
    let cache_inputs = CacheInputs {
        canonical_source,
        lockfile: lockfile.digest(),
        toolchain,
        profile,
    };
    let cache_key = CacheKey::compute(&cache_inputs);

    let cache_dir = located
        .pkg_root
        .join(".brix-cache")
        .join(cache_key.to_hex());
    let profile_dir = match profile {
        Profile::Run => "debug",
        Profile::Serve => "release",
    };
    let binary_path = cache_dir
        .join("target")
        .join(profile_dir)
        .join(format!("{crate_name}{}", std::env::consts::EXE_SUFFIX));

    let cache_hit = binary_path.exists();
    if cache_hit {
        eprintln!("brix: cache hit ({})", cache_key.to_hex());
    } else {
        let files =
            brixc::emit::assemble_workspace(located.manifest.name.as_str(), &relations, &rules);
        write_files(&cache_dir, &files)?;
        run_cargo_build(&cache_dir, profile)?;
    }

    Ok(BuildOutcome {
        cache_dir,
        binary_path,
        cache_hit,
    })
}

/// `build` the package named by `operand`, then execute the produced
/// binary, propagating its exit code. A nonzero child exit is not an
/// `Err` here — it's a successful invocation of `run` whose result is
/// "the program exited with that code," same as a shell would report it.
pub fn run(operand: &str) -> Result<i32, BuildError> {
    let outcome = build(operand, Profile::Run)?;
    let status = Command::new(&outcome.binary_path).status()?;
    Ok(status.code().unwrap_or(1))
}

fn write_files(root: &Utf8Path, files: &BTreeMap<Utf8PathBuf, String>) -> std::io::Result<()> {
    for (rel, contents) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(())
}

fn run_cargo_build(cache_dir: &Utf8Path, profile: Profile) -> Result<(), BuildError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if matches!(profile, Profile::Serve) {
        cmd.arg("--release");
    }
    cmd.arg("--manifest-path").arg(cache_dir.join("Cargo.toml"));
    cmd.arg("--target-dir").arg(cache_dir.join("target"));
    let status = cmd.status()?;
    if !status.success() {
        return Err(BuildError::CargoBuildFailed(status));
    }
    Ok(())
}

fn format_diagnostics(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("\n")
}
