//! Acceptance corpus (issue #45): the supported-language contract as
//! executable pass/fail fixtures, so the parser, checker, compiler, runtime,
//! and diagnostics cannot drift from the spec without a reviewed fixture
//! change.
//!
//! This is the pass/fail superset of `crates/brix-ast/tests/corpus.rs`'s
//! parse-only driver: every one of the 52 spec fixtures under
//! `brix-ast/tests/fixtures/spec/` is driven through
//! parse -> lower -> phase (the fast, no-`cargo` path) and checked against a
//! declared outcome in [`manifest`], plus six new negative fixtures under
//! `tests/fixtures/negative/<phase>/` (one per required failure kind: syntax,
//! name resolution, type error, effect violation, phase cycle, unsupported/
//! invalid declaration) and two Build/Run fixtures gated behind `#[ignore]`
//! because they shell out to `cargo build`/`cargo run` (see module docs on
//! `crates/brix-cli/tests/build_run_smoke.rs`).
//!
//! # Why `Check` collapses onto `Lower` here
//!
//! `brixc::pipeline::Stage` has no separate `Check` seam: static semantics
//! (name resolution, type/effect inference, the Appendix E side conditions)
//! run *inside* `brixc::lower_file` (see `crates/brixc/src/lower/mod.rs`'s
//! doc comment: `lower_file -> check -> diags`, "run inside Lower stage").
//! [`Stage::Check`] is kept as its own manifest variant anyway because it is
//! a distinct spec-facing concept — a fixture can pass Lower's name
//! resolution and still fail a type or effect check — but its driver is
//! exactly [`Stage::Lower`]'s: [`normalize`] folds `Check` onto `Lower`
//! before comparing observed vs. declared outcomes.
//!
//! # Coverage report
//!
//! [`coverage_report_matches_committed`] recomputes a `BTreeMap`-keyed
//! section -> phase -> {pass, fail} summary from [`manifest`] and asserts it
//! is byte-identical to the committed `tests/acceptance_coverage.json`. Any
//! change to spec coverage, a fixture's support status, or its expected
//! phase/codes must show up as a diff in that committed file — silent drift
//! fails CI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brix_diag::{Diagnostics, Severity};
use brixc::pipeline::PhaseAssign;

// ---------------------------------------------------------------------
// Manifest schema
// ---------------------------------------------------------------------

/// The pipeline stage a fixture is declared to reach. `Build`/`Run` fixtures
/// are declarative-only in [`manifest`] (they feed the coverage report and
/// the two `#[ignore]`d subprocess tests) — the fast per-fixture driver in
/// [`acceptance_corpus_matches_manifest`] never attempts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Parse,
    Lower,
    Check,
    Phase,
    Build,
    Run,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Stage::Parse => "parse",
            Stage::Lower => "lower",
            Stage::Check => "check",
            Stage::Phase => "phase",
            Stage::Build => "build",
            Stage::Run => "run",
        }
    }
}

/// Fold [`Stage::Check`] onto [`Stage::Lower`] — see the module docs: they
/// share one driver (`brixc::lower_file`), so a fixture declared to fail at
/// `Check` is observed to fail at `Lower`.
fn normalize(phase: Stage) -> Stage {
    match phase {
        Stage::Check => Stage::Lower,
        other => other,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Pass,
    Fail,
}

/// Where a fixture's source text lives.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// A file under `brix-ast/tests/fixtures/spec/` (the 52-fixture spec
    /// corpus `crates/brix-ast/tests/corpus.rs` already drives for parsing).
    Spec(&'static str),
    /// A file under this crate's `tests/fixtures/negative/<dir>/`.
    Negative {
        dir: &'static str,
        file: &'static str,
    },
    /// A literal source string, for the tiny Build/Run smoke fixture (kept
    /// inline rather than as a file — same idiom as
    /// `crates/brix-cli/tests/build_run_smoke.rs`'s `FIXTURE` const).
    Inline(&'static str),
}

impl Source {
    fn text(&self) -> String {
        match self {
            Source::Spec(file) => std::fs::read_to_string(spec_dir().join(file))
                .unwrap_or_else(|e| panic!("reading spec fixture `{file}`: {e}")),
            Source::Negative { dir, file } => {
                let path = negative_dir().join(dir).join(file);
                std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("reading negative fixture `{path:?}`: {e}"))
            }
            Source::Inline(src) => (*src).to_string(),
        }
    }

    /// A stable, machine-independent label used as the diagnostic-rendering
    /// path and in report/snapshot names.
    fn label(&self) -> String {
        match self {
            Source::Spec(file) => (*file).to_string(),
            Source::Negative { dir, file } => format!("{dir}/{file}"),
            Source::Inline(_) => "inline".to_string(),
        }
    }
}

/// One manifest row: a fixture, the pipeline phase it is declared to reach,
/// and whether it must pass cleanly through that phase or fail there with
/// exactly `codes` (sorted, deduplicated, error-severity only). `codes` is
/// empty for `Expect::Pass`.
struct Entry {
    /// The spec section (or negative-fixture category) this row covers, for
    /// the coverage report. Derived from the filename for `Source::Spec`
    /// (see [`spec_section`]); given explicitly otherwise.
    section: &'static str,
    source: Source,
    terminal: Stage,
    expect: Expect,
    codes: &'static [&'static str],
}

fn spec_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ parent")
        .join("brix-ast")
        .join("tests")
        .join("fixtures")
        .join("spec")
}

fn negative_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("negative")
}

/// Strip a spec fixture's `NNNN-` numeric prefix and `.brix` suffix, so
/// several fixtures carved from the same spec section (e.g. `0028`-`0030`,
/// all "26-2-model-validity-envelopes") share one coverage-report section
/// key.
fn spec_section(file: &'static str) -> &'static str {
    let stem = file.strip_suffix(".brix").unwrap_or(file);
    let after_digits = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    after_digits.strip_prefix('-').unwrap_or(after_digits)
}

// ---------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------

/// The whole-corpus manifest: the 52 spec fixtures (40 clean through
/// `Phase`, 12 declared known-gap `Lower` failures — spec-section fragments
/// that reference types/relations declared elsewhere in the full normative
/// spec and are not part of the block this fixture-extraction carved out;
/// mirrors `crates/brix-ast/tests/corpus.rs`'s `KNOWN_ERRATA` handling: these
/// are declared, not surprises), the 6 negative fixtures (one per required
/// failure kind), and 2 Build/Run fixtures (flagship + one tiny extra).
fn manifest() -> Vec<Entry> {
    let mut m = Vec::new();

    // --- 40 spec fixtures that lower, check, and phase-assign cleanly. ---
    for file in CLEAN_SPEC_FIXTURES {
        m.push(Entry {
            section: spec_section(file),
            source: Source::Spec(file),
            terminal: Stage::Phase,
            expect: Expect::Pass,
            codes: &[],
        });
    }

    // --- 12 spec fixtures with a declared known-gap Lower failure. -------
    for (file, codes) in KNOWN_GAP_SPEC_FIXTURES {
        m.push(Entry {
            section: spec_section(file),
            source: Source::Spec(file),
            terminal: Stage::Lower,
            expect: Expect::Fail,
            codes,
        });
    }

    // --- 6 negative fixtures, one per required failure kind. -------------
    m.push(Entry {
        section: "negative-syntax-error",
        source: Source::Negative {
            dir: "parse",
            file: "syntax_error.brix",
        },
        terminal: Stage::Parse,
        expect: Expect::Fail,
        codes: &["BRX-AST-0001"],
    });
    m.push(Entry {
        section: "negative-name-resolution",
        source: Source::Negative {
            dir: "lower",
            file: "name_resolution.brix",
        },
        terminal: Stage::Lower,
        expect: Expect::Fail,
        codes: &["BRX-LOW-0003"],
    });
    m.push(Entry {
        section: "negative-unsupported-declaration",
        source: Source::Negative {
            dir: "lower",
            file: "unsupported_decl.brix",
        },
        terminal: Stage::Lower,
        expect: Expect::Fail,
        codes: &["BRX-LOW-0001"],
    });
    m.push(Entry {
        section: "negative-type-error",
        source: Source::Negative {
            dir: "check",
            file: "type_error.brix",
        },
        terminal: Stage::Check,
        expect: Expect::Fail,
        codes: &["BRX-IR-0005"],
    });
    m.push(Entry {
        section: "negative-effect-violation",
        source: Source::Negative {
            dir: "check",
            file: "effect_violation.brix",
        },
        terminal: Stage::Check,
        expect: Expect::Fail,
        codes: &["BRX-IR-0006"],
    });
    m.push(Entry {
        section: "negative-phase-cycle",
        source: Source::Negative {
            dir: "phase",
            file: "phase_cycle.brix",
        },
        terminal: Stage::Phase,
        expect: Expect::Fail,
        codes: &["BRX4001"],
    });

    // --- 2 Build/Run fixtures (declarative here; driven by the ignored
    //     subprocess tests below). ----------------------------------------
    m.push(Entry {
        section: spec_section(FLAGSHIP_FILE),
        source: Source::Spec(FLAGSHIP_FILE),
        terminal: Stage::Build,
        expect: Expect::Pass,
        codes: &[],
    });
    m.push(Entry {
        section: "build-run-tiny",
        source: Source::Inline(TINY_RUN_FIXTURE),
        terminal: Stage::Run,
        expect: Expect::Pass,
        codes: &[],
    });

    m
}

const FLAGSHIP_FILE: &str = "0001-part-i-the-flagship-program.brix";

/// A minimal program driven end to end (`brix build` + `brix run`) in
/// [`tiny_program_builds_and_runs`] — same idiom as
/// `crates/brix-cli/tests/build_run_smoke.rs`'s `FIXTURE`.
const TINY_RUN_FIXTURE: &str = "package smoke.acceptance @ 0.1.0\n\
\n\
rel Input { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive R: Output(value: value) from { Input(value) }\n";

/// The 40 spec fixtures that parse, lower, check, and phase-assign cleanly
/// with zero error diagnostics. Discovered by driving every fixture in
/// `brix-ast/tests/fixtures/spec/` through `parse_file` -> `lower_file` ->
/// `AstPhase::assign_phases` and recording which ones raise no error.
#[rustfmt::skip]
const CLEAN_SPEC_FIXTURES: &[&str] = &[
    "0001-part-i-the-flagship-program.brix",
    "0002-6-the-mask-primitive.brix",
    "0006-3-pattern-language.brix",
    "0008-4-aggregation.brix",
    "0012-2-transactions.brix",
    "0013-2-declaration.brix",
    "0014-3-lowering.brix",
    "0017-1-stocks-flows-and-auxiliaries.brix",
    "0018-1-one-world-several-disciplines.brix",
    "0020-19-3-logic.brix",
    "0022-19-6-native-language-model-support.brix",
    "0024-21-6-decision-intelligence.brix",
    "0025-21-7-workflows-and-human-tasks.brix",
    "0026-22-1-the-component-thesis.brix",
    "0027-24-1-unified-external-apis.brix",
    "0028-26-2-model-validity-envelopes.brix",
    "0029-26-2-model-validity-envelopes.brix",
    "0032-26-3-corrections-retroactive-truth-and-historica.brix",
    "0033-26-4-canonical-brix-interchange.brix",
    "0034-26-6-transaction-and-consistency-profiles.brix",
    "0035-26-6-transaction-and-consistency-profiles.brix",
    "0036-26-7-failure-cancellation-and-uncertain-external.brix",
    "0037-26-7-failure-cancellation-and-uncertain-external.brix",
    "0038-26-8-reproducibility-tiers.brix",
    "0039-26-11-trust-profiles-and-threat-model.brix",
    "0040-27-2-relation-frames.brix",
    "0041-27-2-relation-frames.brix",
    "0042-27-3-typed-missingness-and-data-quality.brix",
    "0043-27-4-immutable-cleaning-and-preparation-recipes.brix",
    "0044-27-5-factors-and-statistical-formulas.brix",
    "0045-27-5-factors-and-statistical-formulas.brix",
    "0046-27-6-feature-semantics.brix",
    "0048-27-6-feature-semantics.brix",
    "0049-27-7-immutable-datasets-and-temporal-correctness.brix",
    "0050-27-8-estimator-and-workflow-contracts.brix",
    "0051-27-8-estimator-and-workflow-contracts.brix",
    "0052-27-10-tuning-and-experiments.brix",
    "0054-27-12-predictions-calibration-and-decision-thres.brix",
    "0056-27-15-declarative-visualization-and-reports.brix",
    "0057-27-18-data-science-bricks.brix",
];

/// The 12 spec fixtures that raise at least one error-severity diagnostic
/// during `lower_file` (name resolution / static-semantics checks / type
/// inference), paired with the exact sorted, deduplicated set of BRX-* codes
/// they raise today. These are single-block excerpts carved out of a larger
/// spec section (`scripts/extract_fixtures.sh`) that reference types,
/// relations, or names declared elsewhere in the full normative spec — a
/// real, already-tracked v0-lowering/extraction gap (see
/// `crates/brix-ast/tests/corpus.rs`'s `KNOWN_ERRATA` doc comment for the
/// same pattern applied to parsing), not a surprise. If a future extraction
/// or lowering change closes one of these gaps, this table must be updated
/// in the same PR — that is the point of pinning the codes here instead of
/// only asserting `has_errors()`.
#[rustfmt::skip]
const KNOWN_GAP_SPEC_FIXTURES: &[(&str, &[&str])] = &[
    ("0004-2-derived-relations-and-rules.brix", &["BRX-IR-0001", "BRX-LOW-0012"]),
    ("0007-4-aggregation.brix", &["BRX-LOW-0001"]),
    ("0015-4-invocation.brix", &["BRX-IR-0001", "BRX-LOW-0012"]),
    ("0016-5-authority.brix", &["BRX-IR-0003", "BRX-LOW-0003"]),
    ("0019-19-2-common-reasoning-records.brix", &["BRX-LOW-0012"]),
    ("0021-19-4-mathematics.brix", &["BRX-LOW-0001"]),
    ("0023-21-2-observations-and-claims.brix", &["BRX-IR-0001", "BRX-LOW-0012"]),
    ("0030-26-2-model-validity-envelopes.brix", &["BRX-LOW-0012"]),
    ("0031-26-3-corrections-retroactive-truth-and-historica.brix", &["BRX-IR-0001", "BRX-LOW-0012"]),
    ("0047-27-6-feature-semantics.brix", &["BRX-LOW-0012"]),
    ("0053-27-12-predictions-calibration-and-decision-thres.brix", &["BRX-LOW-0012"]),
    ("0055-27-13-evaluation-explainability-and-fairness.brix", &["BRX-LOW-0012"]),
];

// ---------------------------------------------------------------------
// The fast driver: parse -> lower -> phase, no `cargo` subprocess.
// ---------------------------------------------------------------------

/// The furthest phase [`drive_to`] attempted, whether it was clean (zero
/// error diagnostics) there, and the diagnostics observed at that point
/// (only lowering/phase diagnostics that fired; empty when `clean`).
struct Reached {
    phase: Stage,
    clean: bool,
    diagnostics: Diagnostics,
}

impl Reached {
    fn error_codes(&self) -> Vec<&'static str> {
        let mut codes: Vec<&'static str> = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.code)
            .collect();
        codes.sort_unstable();
        codes.dedup();
        codes
    }
}

/// Drive `src` through parse -> lower -> phase, stopping at the first error
/// or at `target` (`Stage::Build`/`Stage::Run` are treated as `Stage::Phase`
/// — the fast driver never shells out; see [`Source::Inline`]'s Build/Run
/// entries, which the manifest-driven test below skips).
fn drive_to(src: &str, target: Stage) -> Reached {
    let (file, diags) = brix_ast::parse_file(src);
    if diags.has_errors() || target == Stage::Parse {
        let clean = !diags.has_errors();
        return Reached {
            phase: Stage::Parse,
            clean,
            diagnostics: diags,
        };
    }

    let lowered = brixc::lower_file(&file, &diags);
    if lowered.has_errors() || matches!(target, Stage::Lower | Stage::Check) {
        let clean = !lowered.has_errors();
        let mut diagnostics = Diagnostics::new();
        diagnostics.extend(lowered.diags.clone());
        return Reached {
            phase: Stage::Lower,
            clean,
            diagnostics,
        };
    }

    match brixc::AstPhase.assign_phases(lowered) {
        Ok(_) => Reached {
            phase: Stage::Phase,
            clean: true,
            diagnostics: Diagnostics::new(),
        },
        Err(brixc::PipelineError::Diagnostic { diagnostic, .. }) => {
            let mut diagnostics = Diagnostics::new();
            diagnostics.push(*diagnostic);
            Reached {
                phase: Stage::Phase,
                clean: false,
                diagnostics,
            }
        }
        Err(other) => panic!("unexpected pipeline error driving to {target:?}: {other}"),
    }
}

// ---------------------------------------------------------------------
// Deliverable 2 + 4: positive path, fail-closed negative path.
// ---------------------------------------------------------------------

/// Every `Parse`/`Lower`/`Check`/`Phase` fixture in [`manifest`] reaches
/// exactly its declared outcome: `Pass` fixtures are clean through
/// `terminal`; `Fail` fixtures fail at exactly `terminal` (never earlier,
/// never later — a fixture that fails at the wrong phase is just as much a
/// drift signal as one that fails with the wrong code) with exactly the
/// declared sorted/deduplicated BRX-* codes. `Build`/`Run` entries are
/// skipped here (see [`flagship_builds`] / [`tiny_program_builds_and_runs`]).
#[test]
fn acceptance_corpus_matches_manifest() {
    let entries = manifest();
    let mut failures = Vec::new();
    let mut report = String::new();
    let mut checked = 0usize;

    for entry in &entries {
        if matches!(entry.terminal, Stage::Build | Stage::Run) {
            continue;
        }
        checked += 1;
        let label = entry.source.label();
        let src = entry.source.text();
        let reached = drive_to(&src, entry.terminal);
        let observed_phase = normalize(reached.phase);
        let declared_phase = normalize(entry.terminal);

        match entry.expect {
            Expect::Pass => {
                if reached.clean && observed_phase == declared_phase {
                    report.push_str(&format!(
                        "  ok      {label} (Pass @ {})\n",
                        entry.terminal.label()
                    ));
                } else {
                    failures.push(format!(
                        "{label}: expected Pass through {:?}, observed phase={:?} clean={} codes={:?}",
                        entry.terminal,
                        reached.phase,
                        reached.clean,
                        reached.error_codes(),
                    ));
                    report.push_str(&format!("  FAIL    {label}\n"));
                }
            }
            Expect::Fail => {
                let codes = reached.error_codes();
                if !reached.clean && observed_phase == declared_phase && codes == entry.codes {
                    report.push_str(&format!(
                        "  ok      {label} (Fail @ {} {:?})\n",
                        entry.terminal.label(),
                        codes
                    ));
                } else {
                    failures.push(format!(
                        "{label}: expected Fail at {:?} with codes {:?}, observed phase={:?} clean={} codes={:?}",
                        entry.terminal,
                        entry.codes,
                        reached.phase,
                        reached.clean,
                        codes,
                    ));
                    report.push_str(&format!("  FAIL    {label}\n"));
                }
            }
        }
    }

    eprintln!(
        "\nacceptance corpus: {}/{checked} fixtures matched their manifest entry\n{report}",
        checked - failures.len()
    );

    assert!(
        failures.is_empty(),
        "acceptance corpus drifted from its manifest:\n{}",
        failures.join("\n")
    );
}

/// Deliverable 4's fail-closed check, stated directly: no `Expect::Fail`
/// fixture is ever silently accepted. This re-walks only the `Fail` rows —
/// duplicated with (part of) [`acceptance_corpus_matches_manifest`]
/// deliberately, so this property has its own always-green-or-red signal
/// even if the combined table assertion above is ever loosened.
#[test]
fn no_fail_fixture_is_silently_accepted() {
    for entry in manifest()
        .into_iter()
        .filter(|e| e.expect == Expect::Fail && !matches!(e.terminal, Stage::Build | Stage::Run))
    {
        let label = entry.source.label();
        let src = entry.source.text();
        let reached = drive_to(&src, entry.terminal);
        assert!(
            !reached.clean,
            "fail-closed violation: `{label}` was declared Fail at {:?} but was accepted cleanly",
            entry.terminal
        );
    }
}

/// Completeness guard: the set of spec files referenced by [`manifest`] must
/// equal the set of `.brix` files under `brix-ast/tests/fixtures/spec/`.
/// Without this, a newly-added spec fixture would be silently *uncovered* by
/// the corpus — precisely the "spec coverage drift without an explicit fixture
/// update" that issue #45 requires CI to catch. (`crates/brix-ast/tests/
/// corpus.rs` gets this for free by enumerating the directory; this manifest
/// is hand-listed, so the guard is explicit.) A fixture legitimately appears
/// under more than one row — e.g. the flagship is both a `Phase`-pass fixture
/// and a `Build` fixture — so this compares *sets*, not multiplicities.
#[test]
fn manifest_covers_every_spec_fixture() {
    use std::collections::BTreeSet;

    let on_disk: BTreeSet<String> = std::fs::read_dir(spec_dir())
        .expect("spec fixtures dir")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".brix"))
        .collect();

    let referenced: BTreeSet<String> = manifest()
        .into_iter()
        .filter_map(|e| match e.source {
            Source::Spec(file) => Some(file.to_string()),
            _ => None,
        })
        .collect();

    let missing: Vec<&String> = on_disk.difference(&referenced).collect();
    let extra: Vec<&String> = referenced.difference(&on_disk).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "manifest is out of sync with brix-ast/tests/fixtures/spec/:\n  \
         spec fixtures not in the manifest (add them to CLEAN_SPEC_FIXTURES or \
         KNOWN_GAP_SPEC_FIXTURES): {missing:?}\n  \
         manifest entries with no such spec file (remove or rename): {extra:?}"
    );
}

// ---------------------------------------------------------------------
// Deliverable 3: negative-fixture diagnostic snapshots, all three formats.
// ---------------------------------------------------------------------

fn assert_negative_snapshots(name: &str, dir: &'static str, file: &'static str, terminal: Stage) {
    let path = negative_dir().join(dir).join(file);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let reached = drive_to(&src, terminal);
    assert!(
        !reached.clean,
        "negative fixture `{file}` must fail, not pass, at {terminal:?}"
    );
    let label = format!("{dir}/{file}");

    insta::assert_snapshot!(
        format!("{name}_human"),
        reached
            .diagnostics
            .render_format(brix_diag::DiagnosticFormat::Human, &src, &label)
    );
    insta::assert_snapshot!(
        format!("{name}_json"),
        reached
            .diagnostics
            .render_format(brix_diag::DiagnosticFormat::Json, &src, &label)
    );
    insta::assert_snapshot!(
        format!("{name}_sarif"),
        reached
            .diagnostics
            .render_format(brix_diag::DiagnosticFormat::Sarif, &src, &label)
    );
}

#[test]
fn negative_syntax_error_snapshots() {
    assert_negative_snapshots(
        "negative_syntax_error",
        "parse",
        "syntax_error.brix",
        Stage::Parse,
    );
}

#[test]
fn negative_name_resolution_snapshots() {
    assert_negative_snapshots(
        "negative_name_resolution",
        "lower",
        "name_resolution.brix",
        Stage::Lower,
    );
}

#[test]
fn negative_unsupported_decl_snapshots() {
    assert_negative_snapshots(
        "negative_unsupported_decl",
        "lower",
        "unsupported_decl.brix",
        Stage::Lower,
    );
}

#[test]
fn negative_type_error_snapshots() {
    assert_negative_snapshots(
        "negative_type_error",
        "check",
        "type_error.brix",
        Stage::Check,
    );
}

#[test]
fn negative_effect_violation_snapshots() {
    assert_negative_snapshots(
        "negative_effect_violation",
        "check",
        "effect_violation.brix",
        Stage::Check,
    );
}

#[test]
fn negative_phase_cycle_snapshots() {
    assert_negative_snapshots(
        "negative_phase_cycle",
        "phase",
        "phase_cycle.brix",
        Stage::Phase,
    );
}

/// Byte-stability (deliverable "Diagnostic snapshots are byte-stable across
/// repeated runs"): rendering the same negative fixture twice, independently,
/// must produce identical bytes in every format. `insta`'s committed
/// snapshots already pin this across `cargo test` invocations; this test
/// pins it within one process too, so a source of nondeterminism (iteration
/// order, a timestamp, an address) would fail here even before touching a
/// snapshot file.
#[test]
fn negative_diagnostics_render_deterministically_across_independent_runs() {
    let fixtures: &[(&str, &str, Stage)] = &[
        ("parse", "syntax_error.brix", Stage::Parse),
        ("lower", "name_resolution.brix", Stage::Lower),
        ("lower", "unsupported_decl.brix", Stage::Lower),
        ("check", "type_error.brix", Stage::Check),
        ("check", "effect_violation.brix", Stage::Check),
        ("phase", "phase_cycle.brix", Stage::Phase),
    ];
    for (dir, file, terminal) in fixtures {
        let path = negative_dir().join(dir).join(file);
        let src = std::fs::read_to_string(&path).unwrap();
        let label = format!("{dir}/{file}");
        let a = drive_to(&src, *terminal);
        let b = drive_to(&src, *terminal);
        for format in [
            brix_diag::DiagnosticFormat::Human,
            brix_diag::DiagnosticFormat::Json,
            brix_diag::DiagnosticFormat::Sarif,
        ] {
            assert_eq!(
                a.diagnostics.render_format(format, &src, &label),
                b.diagnostics.render_format(format, &src, &label),
                "{label}: {format} rendering must be byte-stable across independent runs"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Deliverable 5: deterministic coverage report.
// ---------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Counts {
    pass: u32,
    fail: u32,
}

fn coverage_report() -> BTreeMap<&'static str, BTreeMap<&'static str, Counts>> {
    let mut report: BTreeMap<&'static str, BTreeMap<&'static str, Counts>> = BTreeMap::new();
    for entry in manifest() {
        let phases = report.entry(entry.section).or_default();
        let counts = phases.entry(entry.terminal.label()).or_default();
        match entry.expect {
            Expect::Pass => counts.pass += 1,
            Expect::Fail => counts.fail += 1,
        }
    }
    report
}

fn coverage_json(report: &BTreeMap<&'static str, BTreeMap<&'static str, Counts>>) -> String {
    let mut out = String::from("{\n");
    for (si, (section, phases)) in report.iter().enumerate() {
        out.push_str(&format!("  \"{section}\": {{\n"));
        for (pi, (phase, counts)) in phases.iter().enumerate() {
            out.push_str(&format!(
                "    \"{phase}\": {{ \"pass\": {}, \"fail\": {} }}",
                counts.pass, counts.fail
            ));
            out.push_str(if pi + 1 == phases.len() { "\n" } else { ",\n" });
        }
        out.push_str("  }");
        out.push_str(if si + 1 == report.len() { "\n" } else { ",\n" });
    }
    out.push_str("}\n");
    out
}

fn coverage_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("acceptance_coverage.json")
}

/// The CI-facing drift gate: the coverage report freshly computed from
/// [`manifest`] must equal the committed `tests/acceptance_coverage.json`
/// exactly. Any manifest edit that changes a fixture's section, terminal
/// phase, or pass/fail status must ship with an update to this committed
/// file in the same PR (or CI fails here) — the mechanical form of "CI fails
/// when spec coverage, support status, or expected diagnostics drift
/// without an explicit fixture update."
#[test]
fn coverage_report_matches_committed() {
    let fresh = coverage_json(&coverage_report());
    let committed = std::fs::read_to_string(coverage_path()).unwrap_or_else(|e| {
        panic!(
            "committed coverage report missing at {:?}: {e}",
            coverage_path()
        )
    });
    assert_eq!(
        fresh, committed,
        "tests/acceptance_coverage.json is stale — regenerate it from the current `manifest()` \
         (coverage_json(&coverage_report())) and commit the update alongside the fixture change \
         that caused this drift"
    );
}

// ---------------------------------------------------------------------
// Build/Run: gated behind #[ignore] — shells out to `cargo build`/`cargo
// run`, which is slow (see crates/brix-cli/tests/build_run_smoke.rs).
// ---------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ parent")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Run the real `brix` binary via `cargo run -p brix-cli --bin brix --`.
/// `brix-conformance` is a separate package from `brix-cli`, so (unlike
/// `crates/brix-cli/tests/build_run_smoke.rs`, which is *in* `brix-cli` and
/// can use `env!("CARGO_BIN_EXE_brix")`) there is no compile-time bin-exe
/// path available here; `cargo run` is the portable way to invoke a sibling
/// package's binary target from an integration test.
fn brix(args: &[&str]) -> std::process::Output {
    let mut full = vec!["run", "--quiet", "-p", "brix-cli", "--bin", "brix", "--"];
    full.extend_from_slice(args);
    std::process::Command::new("cargo")
        .current_dir(workspace_root())
        .args(&full)
        .output()
        .expect("cargo must be spawnable")
}

fn tmp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "brix-conformance-acceptance-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Build/Run positive evidence, fixture 1 of 2: the flagship spec fixture
/// compiles to a Rust workspace via `brix build`. Slow (shells out to
/// `cargo build` twice, once for `brix` itself and once for the generated
/// workspace) — run explicitly with `cargo test -p brix-conformance --test
/// acceptance_corpus -- --ignored`.
#[test]
#[ignore = "shells out to `cargo build` (brix-cli, then the generated workspace); slow"]
fn flagship_builds() {
    let src = Source::Spec(FLAGSHIP_FILE).text();
    let root = tmp_dir("flagship-build");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, &src).unwrap();

    let out = brix(&["build", source_path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "brix build failed for the flagship fixture:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    std::fs::remove_dir_all(&root).ok();
}

/// Build/Run positive evidence, fixture 2 of 2: a tiny one-relation program
/// builds and runs end to end, and the generated binary emits the expected
/// marker line — Run-stage evidence distinct from `flagship_builds`'s
/// Build-stage-only check. Also slow; `--ignored` as above.
#[test]
#[ignore = "shells out to `cargo build`/`cargo run`; slow"]
fn tiny_program_builds_and_runs() {
    let root = tmp_dir("tiny-run");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, TINY_RUN_FIXTURE).unwrap();

    let build_out = brix(&["build", source_path.to_str().unwrap()]);
    assert!(
        build_out.status.success(),
        "brix build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr),
    );

    let run_out = brix(&["run", source_path.to_str().unwrap()]);
    assert!(
        run_out.status.success(),
        "brix run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr),
    );
    assert!(
        String::from_utf8_lossy(&run_out.stdout).contains("brix: generated workspace OK"),
        "stdout: {}",
        String::from_utf8_lossy(&run_out.stdout)
    );

    std::fs::remove_dir_all(&root).ok();
}
