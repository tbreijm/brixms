# Contributing to the BrixMS toolchain (Ring 0)

Ring 0 is the only code that may touch engine internals. It is built by a small
set of lane owners (see each crate's `OWNER.md`) against one shared discipline.

## The governing documents are the single truth

`spec/BrixMS_v9_0.md` described the pre-SOC "Living Model" v9 language and is
**not** normative; it is archived at
[`spec/archive/BrixMS_v9_0.md`](./spec/archive/BrixMS_v9_0.md) for historical
reference only. The current normative set is:

- [`spec/adr/ADR-0002_SOC_Constitution.md`](./spec/adr/ADR-0002_SOC_Constitution.md)
  — the accepted engineering constitution (one category of configurations and
  witnesses, the epistemic outcome lattice, the O(Δ) invariant);
- [`spec/SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md) — the law
  registry and executable conformance map;
- [`spec/Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md)
  — the native typing regime's contract, per-clause evidence status;
- the accepted ADRs for the current language surface,
  [`ADR-0030`](./spec/adr/ADR-0030_Finite_Decision_Alpha.md) through
  [`ADR-0037`](./spec/adr/ADR-0037_Bounded_Lists_And_Folds.md) (check each
  ADR's own Status line — not all of this range are ratified yet; several are
  "Proposed implementation" or "Proposed design").

When behavior and one of these disagree, the governing document wins — unless
it is ambiguous, in which case you do **not** guess. Every ambiguity becomes a
drafted erratum in `spec/errata/` with a proposed ruling and the affected
conformance IDs; it is ruled by Tony and merged before the lane proceeds. See
[`spec/README.md`](./spec/README.md) for the full document map and reading
order.

## The feedback protocol (the only coupling)

Every failure triages into exactly one bin:

1. **Package bug** (Ring 1) → the owning package fixes it; nobody else notices.
2. **Toolchain bug** (Ring 0) → a minimal repro *as a fixture* attached to its
   `diag` code, filed to the Ring 0 queue; fixes ride the versioned toolchain
   release train, lockfile-pinned. Ring 1 upgrades deliberately, never ambiently.
3. **Spec ambiguity** → an erratum, as above.

## Determinism discipline (enforced mechanically)

- `HashMap`/`HashSet` are clippy-denied in semantic paths (`clippy.toml`); use
  `BTreeMap`/`BTreeSet` or a sorted `IndexMap`. Observable order = canon byte order.
- `unsafe` is denied workspace-wide except an allowlisted arena module.
- No floats in a semantic path except behind the strict-IEEE ops module.
- Everything semantic is serialized through **brix-canon** and nothing else.
  Never introduce a second encoder (`DEPS.md`, Ring0 §1.7).

## The bar for every change

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test  --workspace
```

For faster local test runs, use the same parallel runner as CI:

```sh
python3 scripts/test_workspace.py
# Focused iteration; this does not replace the workspace release gate:
python3 scripts/test_workspace.py -p brix-kb -j 4
```

This requires [cargo-nextest](https://nexte.st/docs/installation/). The script
runs all non-ignored unit/integration tests, then Cargo doctests, preserving a
failure from either phase. It reports phase timings and writes test timings and
failures to `target/nextest/local/junit.xml` under the workspace root (nextest's
report directory is separate from Cargo's configurable build directory).
Use `--target-dir` or `CARGO_TARGET_DIR` to reuse an existing build; `--build-jobs`
controls compilation concurrency and `-j` controls test concurrency. The default
is two build jobs and four test processes. There are no automatic retries or
new timeouts that terminate slow tests. Ignored large acceptance workloads must
still be run explicitly for release qualification. Avoid running the full suite
twice concurrently against an actively changing checkout.

On macOS, a long pause even for a test binary's `--list` command is launch
overhead, not time spent in its test bodies. Nextest documents
[XProtect/Gatekeeper startup delays and Developer Tools settings](https://nexte.st/docs/installation/macos/).
Check that separately from compile time; rerun unchanged binaries before
attributing a cold-to-warm improvement to the test runner. The script does not
change system security settings.

CI green-gates all three. PRs are kept small (≤ ~500 generated lines); `insta`
snapshots make canon-vector and codegen drift reviewable at a glance. Frozen
artifacts — `vectors/` after G0, the oracle after G1 — change only through a
spec erratum plus, for canon, a new `CANON_VERSION` tag.

## Adding a dependency

Only from the whitelist in `DEPS.md`. Anything new needs a justification entry
there first.
