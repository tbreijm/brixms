# Core package status

The north-star for this directory is **`brix.type` — a self-hosted BrixMS
type checker that retires the Rust core entirely.** The Rust checkers
(`crates/brix-ir/src/{infer,reflect}.rs`) are the trusted reference the
native package mirrors; the goal is to grow `brix.type` slice-by-slice until
it is authoritative and the Rust reference can be removed.

| Package | Content | State |
|---|---|---|
| [`brix.type`](brix.type) | shadow-mode type checker (slices 1–2: role-binding `HasType`, literal + var-at-two-roles mismatch), proven `FactId`-for-`FactId` equal to `reflect.rs` | **The real track.** Runs as a compiled + executed BrixMS program in the native engine, shadow-mode only (never gates a build). |

## Removed (2026-07-21)

`brix.core`, `brix.math`, `brix.rel`, `brix.time` were throwaway output of
the overnight BrixBuilder loop and produced nothing of lasting value. They
have been cleaned out; each needs a proper ground-up design before it comes
back (e.g. `brix.math` needs a new design — `Decimal`, units, `%` are still
Ring 0 gaps, and nominal newtypes for `Instant`/`Duration` aren't landed).
Do not resurrect the deleted sources; redesign from the spec.

Note: the compiler still carries Rust-core intrinsics that a future
`brix.math`/`brix.core` must eventually subsume and retire — e.g. the
`brix.math.clamp` builtin registered in `crates/brixc/src/lower/resolve.rs`
and the unit intrinsics exercised by `crates/brixc/tests/lower_units.rs`.
These are string-literal builtins, not dependencies on the deleted package
sources (removal is build-safe), but they mark surface the self-hosted
packages will need to reclaim.
