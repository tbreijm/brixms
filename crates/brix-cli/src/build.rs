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

use brix_ast::parse_file;
use brix_canon::{CanonWriter, Digest, Domain};
use brix_diag::{DiagnosticFormat, Diagnostics};
use brixc::pipeline::PhaseAssign;
use brixc::{AstPhase, CacheInputs, CacheKey, PipelineError, Profile};
use camino::{Utf8Path, Utf8PathBuf};

use crate::{package, toolchain};

#[derive(Debug)]
pub enum BuildError {
    Locate(package::LocateError),
    Io(std::io::Error),
    Diagnostics(DiagnosticReport),
    Phase(PipelineError),
    Toolchain(toolchain::ToolchainError),
    CargoBuildFailed(std::process::ExitStatus),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Locate(e) => write!(f, "{e}"),
            BuildError::Io(e) => write!(f, "I/O error: {e}"),
            BuildError::Diagnostics(report) => f.write_str(&report.render(DiagnosticFormat::Human)),
            BuildError::Phase(e) => write!(f, "{e}"),
            BuildError::Toolchain(e) => write!(f, "{e}"),
            BuildError::CargoBuildFailed(status) => write!(f, "cargo build failed: {status}"),
        }
    }
}

/// A source-labelled collection emitted by an attempted compiler stage.
#[derive(Debug)]
pub struct DiagnosticReport {
    pub source: String,
    pub path: String,
    pub diagnostics: Diagnostics,
}

impl DiagnosticReport {
    pub fn render(&self, format: DiagnosticFormat) -> String {
        self.diagnostics
            .render_format(format, &self.source, &self.path)
    }
}

impl BuildError {
    /// Machine-facing renderings are meaningful only for diagnostics. Other
    /// operational errors retain their human `Display` representation.
    pub fn render(&self, format: DiagnosticFormat) -> String {
        match self {
            Self::Diagnostics(report) => report.render(format),
            _ => self.to_string(),
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

/// The result of a successful static/semantic check.
///
/// `check` deliberately stops before planning, code generation, and Cargo. It
/// is the narrow compiler oracle used by editors and local package agents.
pub struct CheckOutcome {
    pub source_path: Utf8PathBuf,
}

/// Parse, lower, type/effect-check, and phase-check a package without emitting
/// or executing anything.
pub fn check(operand: &str) -> Result<CheckOutcome, BuildError> {
    let located = package::locate(operand).map_err(BuildError::Locate)?;
    let source = std::fs::read_to_string(&located.source_path)?;
    let (file, parse_diags) = parse_file(&source);
    if parse_diags.has_errors() {
        return Err(BuildError::Diagnostics(DiagnosticReport {
            source,
            path: located.source_path.to_string(),
            diagnostics: parse_diags,
        }));
    }

    let lowered = brixc::lower_file(&file, &parse_diags);
    if lowered.has_errors() {
        return Err(BuildError::Diagnostics(DiagnosticReport {
            source,
            path: located.source_path.to_string(),
            diagnostics: Diagnostics::from_items(lowered.diags),
        }));
    }

    match AstPhase.assign_phases(lowered) {
        Ok(_) => Ok(CheckOutcome {
            source_path: located.source_path,
        }),
        Err(PipelineError::Diagnostic { diagnostic, .. }) => {
            Err(BuildError::Diagnostics(DiagnosticReport {
                source,
                path: located.source_path.to_string(),
                diagnostics: Diagnostics::from_items(vec![diagnostic]),
            }))
        }
        Err(error) => Err(BuildError::Phase(error)),
    }
}

/// Canonical source produced by the real BrixMS parser/formatter.
pub struct FormatOutcome {
    pub source_path: Utf8PathBuf,
    pub formatted: String,
    pub changed: bool,
}

/// Parse and canonically format a package entry source without writing it.
pub fn format(operand: &str) -> Result<FormatOutcome, BuildError> {
    let located = package::locate(operand).map_err(BuildError::Locate)?;
    let source = std::fs::read_to_string(&located.source_path)?;
    let (file, parse_diags) = parse_file(&source);
    if parse_diags.has_errors() {
        return Err(BuildError::Diagnostics(DiagnosticReport {
            source,
            path: located.source_path.to_string(),
            diagnostics: parse_diags,
        }));
    }
    let formatted = brix_ast::format_file(&file);
    Ok(FormatOutcome {
        source_path: located.source_path,
        changed: formatted != source,
        formatted,
    })
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
        return Err(BuildError::Diagnostics(DiagnosticReport {
            source,
            path: located.source_path.to_string(),
            diagnostics: parse_diags,
        }));
    }

    let lowered = brixc::lower_file(&file, &parse_diags);
    if lowered.has_errors() {
        return Err(BuildError::Diagnostics(DiagnosticReport {
            source,
            path: located.source_path.to_string(),
            diagnostics: Diagnostics::from_items(lowered.diags),
        }));
    }

    let phased = match AstPhase.assign_phases(lowered) {
        Ok(phased) => phased,
        Err(PipelineError::Diagnostic { diagnostic, .. }) => {
            return Err(BuildError::Diagnostics(DiagnosticReport {
                source,
                path: located.source_path.to_string(),
                diagnostics: Diagnostics::from_items(vec![diagnostic]),
            }));
        }
        Err(error) => return Err(BuildError::Phase(error)),
    };
    let (relations, rules) = brixc::emit::project_phased(&phased);
    // This runtime-owned IR is generated alongside the typed-store scaffold.
    // The binary never reconstructs schema from untyped transaction input.
    let native_program = brixc::emit::project_program(&phased);
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

    // Emit the generated workspace unconditionally — it is pure, fast codegen
    // (no `cargo`), and having the exact file set in hand is what lets us
    // decide a cache *hit* safely (issue #41 / spec §26.8 "cache corruption or
    // an incomplete cache entry cannot be treated as a successful build
    // artifact"): a hit requires not just that the binary exists, but that the
    // completion marker (written last) carries the expected file-set digest and
    // that every generated source on disk still matches byte for byte. An
    // interrupted or tampered cache entry therefore rebuilds rather than being
    // trusted.
    let runtime_path = camino::Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("brix-rt");
    let files = brixc::emit::assemble_workspace_with_runtime(
        located.manifest.name.as_str(),
        &relations,
        &rules,
        runtime_path.as_str(),
        &native_program,
    );
    let files_digest_hex = digest_of_files(&files).to_hex();

    let cache_hit = cache_entry_is_valid(&cache_dir, &binary_path, &files, &files_digest_hex);
    if cache_hit {
        eprintln!("brix: cache hit ({})", cache_key.to_hex());
    } else {
        // Drop any stale marker first, so an interrupted rebuild never leaves a
        // marker that would validate a half-written entry.
        let marker = cache_dir.join(CACHE_MARKER);
        std::fs::remove_file(&marker).ok();
        write_files(&cache_dir, &files)?;
        run_cargo_build(&cache_dir, profile)?;
        // The completion marker is written LAST: its presence, carrying the
        // file-set digest, is the atomic "this entry is complete" signal.
        std::fs::write(&marker, &files_digest_hex)?;
    }

    Ok(BuildOutcome {
        cache_dir,
        binary_path,
        cache_hit,
    })
}

/// Filename of the cache-completion marker written inside each `.brix-cache`
/// entry after a successful build (see [`cache_entry_is_valid`]).
const CACHE_MARKER: &str = ".brix-manifest";

/// A canonical digest over the full generated file set (each path plus its
/// contents, in `BTreeMap` order — length-prefixed via [`CanonWriter`] so no
/// concatenation ambiguity can collide two different file sets). Stored in the
/// completion marker and re-derived on every build to validate a candidate hit.
fn digest_of_files(files: &BTreeMap<Utf8PathBuf, String>) -> Digest {
    let mut w = CanonWriter::new();
    for (path, contents) in files {
        w.write_str(path.as_str());
        w.write_bytes(contents.as_bytes());
    }
    w.digest(Domain::Value)
}

/// Whether an on-disk cache entry may be trusted as a completed, uncorrupted
/// build for `files`. Requires all three: the built binary exists, the
/// completion marker is present with exactly `expected_hex` (written only after
/// a successful `cargo build`, so an interrupted build leaves no valid marker),
/// and every generated source on disk is byte-identical to what we would emit
/// now (so a truncated or tampered entry is rebuilt, never trusted). Reads only
/// the small generated sources, not the `cargo` target tree, so a warm hit stays
/// fast.
fn cache_entry_is_valid(
    cache_dir: &Utf8Path,
    binary_path: &Utf8Path,
    files: &BTreeMap<Utf8PathBuf, String>,
    expected_hex: &str,
) -> bool {
    if !binary_path.exists() {
        return false;
    }
    match std::fs::read_to_string(cache_dir.join(CACHE_MARKER)) {
        Ok(found) if found == expected_hex => {}
        _ => return false,
    }
    for (rel, contents) in files {
        match std::fs::read_to_string(cache_dir.join(rel)) {
            Ok(on_disk) if &on_disk == contents => {}
            _ => return false,
        }
    }
    true
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
    // Generated workspaces are built only from the already-resolved local
    // toolchain graph.  Do not let a cache build acquire a fresh registry
    // version (or depend on network availability) behind the caller's back.
    cmd.arg("--offline");
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
