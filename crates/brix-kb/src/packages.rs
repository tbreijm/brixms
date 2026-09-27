//! Bounded source reading and package loading for `brix-kb`.
//!
//! Deliberately duplicated (in miniature) from `crates/brix-cli/src/packages.rs`
//! rather than shared, because `brix-cli` depends on `brix-kb` and a shared
//! helper would need to live in a third place; the CLI's copy is off limits
//! for this change (see ADR-0041 §6). Behavior matches exactly: `brix.soc` is
//! reserved and embedded, other packages resolve only through explicit
//! `package_paths` roots under `<root>/<pkg_name>/src/<tail>.brix`, and there
//! is never an ambient filesystem lookup.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Strict 1 MiB source read limit applied before allocation (matches `brix-cli`).
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;

/// Compile-time embedded reserved `brix.soc` package.
pub const EMBEDDED_BRIX_SOC: &str = include_str!("../../../packages/brix.soc/src/soc.brix");

/// Read source text from `path` enforcing a strict 1 MiB limit before allocation.
pub fn read_source_bounded<P: AsRef<Path>>(path: P) -> Result<String, String> {
    let path = path.as_ref();
    let meta =
        std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if meta.len() > MAX_SOURCE_BYTES as u64 {
        return Err(format!(
            "source file {} exceeds maximum size limit (1 MiB): {} bytes",
            path.display(),
            meta.len()
        ));
    }
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut buf = Vec::new();
    file.by_ref()
        .take((MAX_SOURCE_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if buf.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "source file {} exceeds maximum size limit (1 MiB): {} bytes",
            path.display(),
            buf.len()
        ));
    }
    String::from_utf8(buf).map_err(|_| format!("source file {} is not valid UTF-8", path.display()))
}

/// Create a package loader closure using explicit package paths and embedded `brix.soc`.
pub fn make_package_loader(package_paths: &[PathBuf]) -> impl Fn(&str) -> Option<String> + '_ {
    move |pkg_name: &str| {
        if pkg_name == "brix.soc" {
            return Some(EMBEDDED_BRIX_SOC.to_string());
        }
        let tail = pkg_name.rsplit('.').next()?;
        for root in package_paths {
            let candidate_std = root.join(pkg_name).join("src").join(format!("{tail}.brix"));
            if let Ok(src) = read_source_bounded(&candidate_std) {
                return Some(src);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_brix_soc_is_valid_and_nonempty() {
        assert!(!EMBEDDED_BRIX_SOC.is_empty());
        assert!(EMBEDDED_BRIX_SOC.contains("brix.soc"));
    }

    #[test]
    fn test_no_ambient_lookup_when_package_paths_empty() {
        let loader = make_package_loader(&[]);
        assert!(loader("nonexistent.pkg").is_none());
        assert!(loader("brix.soc").is_some());
    }
}
