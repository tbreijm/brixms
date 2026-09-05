//! Package loading and bounded source reading for `brix`.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Strict 1 MiB source read limit applied before allocation.
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024; // 1 MiB

/// Compile-time embedded reserved `brix.soc` package.
/// This package is reserved and cannot be overridden by any ambient or explicit package root.
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
///
/// Precedence & Resolution Rules:
/// 1. `brix.soc` is reserved and embedded at compile time; it ALWAYS wins over any explicit path.
/// 2. Other packages are resolved strictly through explicit `package_paths` roots in order.
/// 3. Ambient lookup is prohibited: current directory and parent directories are NEVER consulted
///    unless explicitly passed in `package_paths`.
pub fn make_package_loader(package_paths: &[PathBuf]) -> impl Fn(&str) -> Option<String> + '_ {
    move |pkg_name: &str| {
        // 1. Reserved embedded brix.soc package wins over everything.
        if pkg_name == "brix.soc" {
            return Some(EMBEDDED_BRIX_SOC.to_string());
        }

        // 2. Resolve only through explicit package-path roots.
        let tail = pkg_name.rsplit('.').next()?;
        for root in package_paths {
            // Standard package layout: <root>/<pkg_name>/src/<tail>.brix
            let candidate_std = root.join(pkg_name).join("src").join(format!("{tail}.brix"));
            if let Ok(src) = read_source_bounded(&candidate_std) {
                return Some(src);
            }
        }

        // 3. Never falls back to ambient CWD or parent directory.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTestDir {
        path: PathBuf,
    }

    impl TempTestDir {
        fn new(prefix: &str) -> Self {
            let count = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let mut path = std::env::temp_dir();
            path.push(format!("brix_cli_test_{prefix}_{pid}_{count}"));
            std::fs::create_dir_all(&path).expect("failed to create temp test dir");
            Self { path }
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_embedded_brix_soc_is_valid_and_nonempty() {
        assert!(!EMBEDDED_BRIX_SOC.is_empty());
        assert!(EMBEDDED_BRIX_SOC.contains("brix.soc"));
        assert!(EMBEDDED_BRIX_SOC.contains("honest_outcome"));
        assert!(EMBEDDED_BRIX_SOC.contains("config Outcome"));
    }

    #[test]
    fn test_embedded_brix_soc_precedence_over_explicit_paths() {
        let temp = TempTestDir::new("precedence");
        // Construct an explicit fake brix.soc package on disk
        let fake_soc_dir = temp.path.join("brix.soc").join("src");
        std::fs::create_dir_all(&fake_soc_dir).unwrap();
        let fake_soc_file = fake_soc_dir.join("soc.brix");
        std::fs::write(&fake_soc_file, "// FAKE OVERRIDE BRIX.SOC").unwrap();

        // Pass the explicit root to the package loader
        let explicit_roots = vec![temp.path.clone()];
        let loader = make_package_loader(&explicit_roots);

        let loaded = loader("brix.soc").expect("brix.soc must resolve");
        // Must return EMBEDDED_BRIX_SOC, NOT the fake file content!
        assert_eq!(loaded, EMBEDDED_BRIX_SOC);
        assert!(!loaded.contains("FAKE OVERRIDE"));
    }

    #[test]
    fn test_explicit_path_resolution() {
        let temp = TempTestDir::new("explicit_res");
        let pkg_dir = temp.path.join("custom.domain").join("src");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let pkg_file = pkg_dir.join("domain.brix");
        let expected_source = "config MyConfig = { id: Str }\n";
        std::fs::write(&pkg_file, expected_source).unwrap();

        let explicit_roots = vec![temp.path.clone()];
        let loader = make_package_loader(&explicit_roots);

        let loaded = loader("custom.domain").expect("custom.domain must resolve");
        assert_eq!(loaded, expected_source);
    }

    #[test]
    fn test_flat_layout_rejected() {
        let temp = TempTestDir::new("flat_rejected");
        // Create flat layout <root>/<pkg_name>/<tail>.brix instead of src/<tail>.brix
        let pkg_dir = temp.path.join("custom.flat");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let pkg_file = pkg_dir.join("flat.brix");
        std::fs::write(&pkg_file, "config Flat = { id: Str }\n").unwrap();

        let explicit_roots = vec![temp.path.clone()];
        let loader = make_package_loader(&explicit_roots);

        // Must NOT resolve flat layout
        assert!(loader("custom.flat").is_none());
    }

    #[test]
    fn test_explicit_path_order_resolution() {
        let temp1 = TempTestDir::new("order1");
        let temp2 = TempTestDir::new("order2");

        let dir1 = temp1.path.join("shared.pkg").join("src");
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::write(dir1.join("pkg.brix"), "// Version 1").unwrap();

        let dir2 = temp2.path.join("shared.pkg").join("src");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("pkg.brix"), "// Version 2").unwrap();

        let roots1 = vec![temp1.path.clone(), temp2.path.clone()];
        let loader1 = make_package_loader(&roots1);
        assert_eq!(loader1("shared.pkg").unwrap(), "// Version 1");

        let roots2 = vec![temp2.path.clone(), temp1.path.clone()];
        let loader2 = make_package_loader(&roots2);
        assert_eq!(loader2("shared.pkg").unwrap(), "// Version 2");
    }

    #[test]
    fn test_no_ambient_lookup_when_package_paths_empty() {
        // Without explicit package paths, custom packages must NOT resolve
        let loader = make_package_loader(&[]);
        assert!(
            loader("nonexistent.pkg").is_none(),
            "empty package-path must not resolve arbitrary packages"
        );
        // Even packages directory from repo should not resolve without explicit flag
        assert!(
            loader("custom.pkg").is_none(),
            "must never perform ambient lookup"
        );

        // But reserved embedded brix.soc continues to resolve
        assert!(loader("brix.soc").is_some());
    }

    #[test]
    fn test_read_source_bounded_under_limit() {
        let temp = TempTestDir::new("read_ok");
        let file = temp.path.join("small.brix");
        std::fs::write(&file, "let x = 1\n").unwrap();

        let content = read_source_bounded(&file).expect("should read small file");
        assert_eq!(content, "let x = 1\n");
    }

    #[test]
    fn test_read_source_bounded_over_limit() {
        let temp = TempTestDir::new("read_over");
        let file = temp.path.join("huge.brix");
        // Create a file exceeding 1 MiB
        let huge_data = vec![b'a'; MAX_SOURCE_BYTES + 10];
        std::fs::write(&file, huge_data).unwrap();

        let err = read_source_bounded(&file).expect_err("should reject huge file");
        assert!(err.contains("exceeds maximum size limit"));
    }

    #[test]
    fn test_read_source_bounded_invalid_utf8() {
        let temp = TempTestDir::new("read_utf8");
        let file = temp.path.join("invalid.brix");
        std::fs::write(&file, [0xFF, 0xFE, 0xFD]).unwrap();

        let err = read_source_bounded(&file).expect_err("should reject invalid utf8");
        assert!(err.contains("not valid UTF-8"));
    }
}
