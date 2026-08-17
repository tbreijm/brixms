# Packaged Brix sources

Brix programs that ship with the toolchain. Each one must keep checking —
`crates/brix-lower/tests/packaged_brix.rs` is the gate, and it exists because
a `.brix` file nobody runs rots exactly like a stale comment. The shipped
examples declared a `base: Money` field for months with no such type in the
language.

| Package | Content | State |
|---|---|---|
| [`brix.soc`](brix.soc) | SOC's own core: the identities, the outcome lattice, `Generator`/`Chain`, `Judgement`, and `honest_outcome` — the honesty rule the substrate turns on | **Live.** Checked on every build. |

## Removed 2026-08-18

`brix.core`, `brix.math`, `brix.ops`, `brix.sim`, `brix.type`, `brix.music`.

None of them parsed. They were written against the pre-SOC language — `pub`,
`measure`, `unit`, `enum`, `Result<T, E>`, `F64`, `package … @ version` — none
of which is in the current surface. `brix.music` was an empty directory.

The previous version of this file described `brix.type` as "the real track", a
self-hosted type checker shadowing `crates/brix-ir`. **That crate no longer
exists** — the legacy IR and its checkers were deleted when the native
type-realization regime became authoritative. The file was describing a world
that had been gone for weeks, which is the same failure mode the gate above
now prevents for the sources themselves.

Do not resurrect the deleted sources. Anything worth having from them is worth
redesigning against the current spec.
