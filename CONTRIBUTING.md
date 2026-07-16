# Contributing to the BrixMS toolchain (Ring 0)

Ring 0 is the only code that may touch engine internals. It is built by a small
set of lane owners (see each crate's `OWNER.md`) against one shared discipline.

## The spec is the single truth

`spec/BrixMS_v9_0.md` is normative. When behavior and spec disagree, the spec
wins — unless the spec is ambiguous, in which case you do **not** guess. Every
ambiguity becomes a drafted erratum in `spec/errata/` with a proposed ruling and
the affected conformance IDs; it is ruled by Tony and merged before the lane
proceeds. The document stays the single truth and every ruling improves the next
contributor's context.

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

CI green-gates all three. PRs are kept small (≤ ~500 generated lines); `insta`
snapshots make canon-vector and codegen drift reviewable at a glance. Frozen
artifacts — `vectors/` after G0, the oracle after G1 — change only through a
spec erratum plus, for canon, a new `CANON_VERSION` tag.

## Adding a dependency

Only from the whitelist in `DEPS.md`. Anything new needs a justification entry
there first.
