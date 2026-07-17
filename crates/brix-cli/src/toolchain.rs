//! Detect the toolchain driving pass 2, for `brixc::cache::CacheInputs`.
//!
//! No `env!`/`build.rs` target-triple mechanism exists anywhere in the
//! repo; `rustc -vV` is the one source of truth for "which rustc, which
//! target" that's guaranteed to match whatever `cargo build` actually
//! invokes (Part XXVIII §28.2: "moving toolchains is deliberate, never
//! ambient" — querying the live toolchain, not a baked-in constant, is
//! what makes a silent `rustc` bump a cache miss rather than a
//! reproducibility bug).

use std::fmt;
use std::process::Command;

use brixc::ToolchainId;

#[derive(Debug)]
pub enum ToolchainError {
    Spawn(std::io::Error),
    /// `rustc -vV`'s output didn't contain the expected `release:`/`host:`
    /// lines — a legible error beats guessing at a malformed toolchain.
    UnparsableOutput(String),
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolchainError::Spawn(e) => write!(f, "could not run `rustc -vV`: {e}"),
            ToolchainError::UnparsableOutput(out) => {
                write!(f, "could not parse `rustc -vV` output:\n{out}")
            }
        }
    }
}

impl std::error::Error for ToolchainError {}

/// Detect the toolchain by shelling `rustc -vV` and parsing its `release:`
/// and `host:` lines.
pub fn detect() -> Result<ToolchainId, ToolchainError> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(ToolchainError::Spawn)?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    let mut rustc_version = None;
    let mut target = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("release: ") {
            rustc_version = Some(v.trim().to_string());
        }
        if let Some(t) = line.strip_prefix("host: ") {
            target = Some(t.trim().to_string());
        }
    }

    match (rustc_version, target) {
        (Some(rustc_version), Some(target)) => Ok(ToolchainId {
            brixc_version: env!("CARGO_PKG_VERSION").to_string(),
            rustc_version,
            target,
        }),
        _ => Err(ToolchainError::UnparsableOutput(text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_finds_a_real_rustc() {
        let toolchain = detect().expect("rustc -vV must be runnable in the test environment");
        assert!(!toolchain.rustc_version.is_empty());
        assert!(!toolchain.target.is_empty());
    }
}
