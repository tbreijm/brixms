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

/// Fixtures known to hit a genuine Appendix D grammar gap (tracked as
/// errata). They must still not panic and must format idempotently, but are
/// not counted toward the clean-parse total.
fn is_known_errata(name: &str) -> bool {
    // 0003 is a *documentation* block: it uses literal `...`, `<...>`, and a
    // second `entity Name { ... }` redeclaration as prose placeholders, not
    // real syntax (errata 0001). We parse it structurally but don't require
    // zero diagnostics.
    name.starts_with("0003-")
        // 0005 is the Part III §3 pattern-language *catalog*: a bare list of
        // clause forms (`R(role: x, other)`, `let v = pureExpr`, ...) at
        // top level, one per line with a trailing comment — never wrapped
        // in a `derive`/`query`/etc. body, so it isn't a legal top-level
        // program at all, just documentation shorthand.
        || name.starts_with("0005-")
        // 0009 is a `query` *template*: `query Name(args) -> Rel<Row> =
        // from { ... } yield { ... }` uses `Name`/`args`/`Row` as prose
        // placeholders for "a name here" / "a param list here" — `args`
        // alone is not a valid `Ident : Type` parameter.
        || name.starts_with("0009-")
        // 0010 is a `constraint` *header template*:
        // `constraint Name (advisory | strict | audit) { ...pattern... }`
        // — the `(a | b | c)` alternation and `...pattern...` are prose
        // notation for "pick one kind" / "a pattern block here", not
        // Appendix D grammar.
        || name.starts_with("0010-")
        // 0011 is a `scenario` *template*: `bind P to <adapter>` uses
        // `<adapter>` as a literal placeholder token (not a real generic),
        // and `setup { ...transactions... }` / `step ... { ...transactions
        // per tick... }` use prose inside the transaction bodies.
        || name.starts_with("0011-")
}

#[test]
fn corpus_parses() {
    let fixtures = load_fixtures();
    let mut pass = 0usize;
    let mut clean = 0usize;
    let mut errata = 0usize;
    let mut report = String::new();
    for (name, src) in &fixtures {
        let (_file, diags) = brix_ast::parse_file(src);
        let ok = !diags.has_errors();
        if is_known_errata(name) {
            errata += 1;
            report.push_str(&format!(
                "  ERRATA  {name} ({} diagnostic(s))\n",
                diags.len()
            ));
            continue;
        }
        if ok {
            clean += 1;
            report.push_str(&format!("  ok      {name}\n"));
        } else {
            report.push_str(&format!(
                "  FAIL    {name}\n{}",
                indent(&diags.render(src, name), 10)
            ));
        }
        pass += 1;
    }
    let total = fixtures.len();
    eprintln!(
        "\ncorpus: {clean}/{} clean-parse (+{errata} tracked errata) of {total} fixtures\n{report}",
        pass
    );

    // Baseline: keep the number that parse cleanly from regressing. Raise
    // this as coverage improves.
    const BASELINE_CLEAN: usize = 50;
    assert!(
        clean >= BASELINE_CLEAN,
        "clean-parse regression: {clean} < baseline {BASELINE_CLEAN}"
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
        // errors (errata fixtures excepted — their diagnostics are expected).
        if d2.has_errors() && !is_known_errata(&name) {
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
