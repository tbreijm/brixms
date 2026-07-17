//! Corpus conformance tests over the 57 spec fixtures in
//! `tests/fixtures/spec/` (mechanically carved from `spec/BrixMS_v9_0.md` by
//! `scripts/extract_fixtures.sh`; see OWNER.md).
//!
//! Three properties are checked over every fixture:
//!
//! 1. **parse** — the fixture parses with zero error diagnostics. A per-file
//!    pass/fail table is printed and the overall count asserted against a
//!    baseline so regressions are visible.
//! 2. **fmt idempotence** — `fmt(parse(x))` is a fixed point:
//!    `fmt(parse(fmt(parse(x)))) == fmt(parse(x))`.
//! 3. **fmt parse-stability** — the formatted output itself parses cleanly.
//!
//! Fixtures that expose a genuine Appendix D gap are listed in
//! `KNOWN_ERRATA` with the erratum that tracks them; they are excluded from
//! the clean-parse count but still must not panic and must format
//! idempotently.

use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("spec")
}

fn load_fixtures() -> Vec<(String, String)> {
    let dir = fixtures_dir();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("brix") {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let src = std::fs::read_to_string(&path).unwrap();
            out.push((name, src));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn corpus_parses() {
    let fixtures = load_fixtures();
    let mut clean = 0usize;
    let mut report = String::new();
    for (name, src) in &fixtures {
        let (_file, diags) = brix_ast::parse_file(src);
        let ok = !diags.has_errors();
        if ok {
            clean += 1;
            report.push_str(&format!("  ok      {name}\n"));
        } else {
            report.push_str(&format!(
                "  FAIL    {name}\n{}",
                indent(&diags.render(src, name), 10)
            ));
        }
    }
    let total = fixtures.len();
    eprintln!("\ncorpus: {clean}/{total} clean-parse of {total} fixtures\n{report}",);

    assert_eq!(
        clean, total,
        "every executable BrixMS block in the normative spec must parse cleanly"
    );
}

#[test]
fn corpus_never_panics_and_is_total() {
    // Every fixture must return a tree (parser is total) without panicking.
    for (name, src) in load_fixtures() {
        let (file, _diags) = brix_ast::parse_file(&src);
        // Touch the tree so nothing is optimized away.
        assert!(
            file.decls.len() + file.uses.len() < 100_000,
            "sanity for {name}"
        );
    }
}

#[test]
fn fmt_idempotent() {
    let mut failures = Vec::new();
    for (name, src) in load_fixtures() {
        let (f1, _) = brix_ast::parse_file(&src);
        let once = brix_ast::format_file(&f1);
        let (f2, d2) = brix_ast::parse_file(&once);
        let twice = brix_ast::format_file(&f2);
        if once != twice {
            failures.push(format!(
                "{name}: fmt not idempotent\n--- once ---\n{once}\n--- twice ---\n{twice}"
            ));
        }
        // Parse-stability: formatted output must itself parse without new
        // errors.
        if d2.has_errors() {
            failures.push(format!(
                "{name}: formatted output does not parse cleanly:\n{}",
                d2.render(&once, &name)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn indent(s: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    s.lines().map(|l| format!("{pad}{l}\n")).collect::<String>()
}
